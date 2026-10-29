#![cfg(feature = "sqlite")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use retype_dict::{
    adaptive::PRIOR_MODEL_VERSION, from_pairs, AsyncDict, LayeredDict, Learner, UserDict,
};
use retype_learning_store::{protocol::*, store::Store};
use retype_pinyin::{Decoder, Lexicon};
use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

struct Database(PathBuf);
impl Database {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "retype-prior-migration-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir_all(&root).unwrap();
        Self(root.join("user.db"))
    }
    fn seed_v2(&self) {
        Connection::open(&self.0).unwrap().execute_batch(
            "CREATE TABLE entries(pinyin TEXT,text TEXT,logp REAL,selections INTEGER,last_used INTEGER,
                usage_count INTEGER,recent REAL,last_tick INTEGER,prior_logp REAL,PRIMARY KEY(pinyin,text));
             CREATE TABLE clients(id TEXT PRIMARY KEY,sequence INTEGER,seen INTEGER);
             CREATE TABLE metadata(id INTEGER PRIMARY KEY,revision INTEGER,tick INTEGER);
             CREATE TABLE corrections(original TEXT,replacement TEXT,count INTEGER,PRIMARY KEY(original,replacement));
             CREATE TABLE sync_identity(id TEXT PRIMARY KEY,value TEXT NOT NULL);
             CREATE TABLE sync_english(origin TEXT,text TEXT,count INTEGER,PRIMARY KEY(origin,text));
             CREATE TABLE sync_words(origin TEXT,pinyin TEXT,text TEXT,count INTEGER,PRIMARY KEY(origin,pinyin,text));
             CREATE TABLE sync_corrections(origin TEXT,original TEXT,replacement TEXT,count INTEGER,PRIMARY KEY(origin,original,replacement));
             INSERT INTO entries VALUES('la''qu','啦去',-12.312003,29,1234,29,3.14436549817969,19062,-12.312003);
             INSERT INTO entries VALUES('la''qu','拉取',-13.687226,40,1250,40,8.857002036172819,19067,-13.687226);
             INSERT INTO clients VALUES('existing-client',1,1234);
             INSERT INTO metadata VALUES(1,17,19093);
             INSERT INTO corrections VALUES('人工只能','人工智能',2);
             INSERT INTO sync_identity VALUES('origin','existing-origin'),('seeded','1');
             INSERT INTO sync_english VALUES('existing-origin','Retype',3);
             INSERT INTO sync_words VALUES('existing-origin','la''qu','啦去',29),
                ('existing-origin','la''qu','拉取',35),('another-device','la''qu','拉取',5);
             INSERT INTO sync_corrections VALUES('existing-origin','人工只能','人工智能',2);
             PRAGMA user_version=2;",
        ).unwrap();
    }
}
impl Drop for Database {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.0.parent().unwrap());
    }
}

fn poll(revision: Option<u64>) -> Request {
    Request {
        version: VERSION,
        client: "reader".into(),
        known_revision: revision,
        events: vec![],
        stop: false,
    }
}
fn dictionary() -> Arc<dyn Lexicon> {
    Arc::new(
        from_pairs([
            ("拉取", "la qu", 1347.),
            ("拉去", "la qu", 618.),
            ("啦", "la", 181128.),
            ("去", "qu", 1049335.),
            ("参考", "can kao", 1_183_767_572.),
        ])
        .0,
    )
}

#[test]
fn v2_priors_are_invalidated_then_resolved_without_relearning_or_losing_history() {
    verify_v2_migration(dictionary());
}

fn verify_v2_migration(base: Arc<dyn Lexicon>) {
    let database = Database::new();
    database.seed_v2();
    let system = AsyncDict::empty();
    let mut store = Store::open(&database.0, Arc::clone(&system) as Arc<dyn Lexicon>).unwrap();
    let pending = store.handle(&poll(Some(17))).unwrap();
    assert!(
        pending.revision > 17,
        "Existing clients must receive invalidation even before the dictionary loads"
    );
    let pending_snapshot = pending.snapshot.unwrap();
    let old_ranking = pending_snapshot.ranking.as_ref().unwrap();
    assert_eq!(old_ranking.tick, 19093);
    assert!(old_ranking.records.iter().all(|r| r.prior_logp.is_none()));
    assert!(
        pending_snapshot.entries.iter().all(|e| e.logp < -80.),
        "Legacy snapshots must not expose stale boosted scores"
    );
    let exported = store.sync_export().unwrap();
    assert_eq!(exported.words.iter().map(|w| w.count).sum::<u64>(), 69);
    assert_eq!(exported.words.len(), 3);
    assert!(exported
        .words
        .iter()
        .any(|w| w.origin == "another-device" && w.count == 5));
    assert_eq!(pending_snapshot.english, vec![("Retype".into(), 3)]);
    assert_eq!(
        pending_snapshot.corrections,
        vec![("人工只能".into(), "人工智能".into(), 2)]
    );

    // Production loads its dictionary after opening SQLite; resolving the old
    // priors must publish a new snapshot even with no new selections.
    system.install(base);
    let resolved = store.handle(&poll(Some(pending.revision))).unwrap();
    assert!(resolved.revision > pending.revision);
    let snapshot = resolved.snapshot.unwrap();
    let ranking = snapshot.ranking.as_ref().unwrap();
    assert_eq!(ranking.tick, old_ranking.tick);
    for old in &old_ranking.records {
        let new = ranking.records.iter().find(|r| r.text == old.text).unwrap();
        assert_eq!(
            (new.count, new.recent, new.last_tick),
            (old.count, old.recent, old.last_tick)
        );
        assert!(new.prior_logp.is_some());
    }
    let user = Arc::new(UserDict::new());
    let learner = Learner::with_system(Arc::clone(&user), Arc::clone(&system) as Arc<dyn Lexicon>);
    snapshot.restore(&user, &learner);
    let lex = LayeredDict::with_system_and_user(Arc::clone(&system) as Arc<dyn Lexicon>, user, 0.);
    assert_eq!(
        Decoder::new().decode("laqu", &lex).candidates[0].text,
        "拉取"
    );
    assert_eq!(
        retype_pinyin::shuangpin::decode("laqu", &lex, &Default::default()).candidates[0].text,
        "拉取"
    );

    // Old receipts remain valid: a retry cannot manufacture another selection.
    let retry = store
        .handle(&Request {
            version: VERSION,
            client: "existing-client".into(),
            known_revision: Some(resolved.revision),
            events: vec![SequencedEvent {
                sequence: 1,
                event: Event::Chosen {
                    text: "啦去".into(),
                    pinyin: "la'qu".into(),
                    index: 0,
                },
            }],
            stop: false,
        })
        .unwrap();
    assert_eq!(retry.acknowledged, 1);
    assert_eq!(retry.revision, resolved.revision);
    assert!(retry.snapshot.is_none());
    assert_eq!(store.sync_export().unwrap(), exported);
    drop(store);

    let mut reopened = Store::open(&database.0, system as Arc<dyn Lexicon>).unwrap();
    assert!(reopened
        .handle(&poll(Some(resolved.revision)))
        .unwrap()
        .snapshot
        .is_none());
    let restored = reopened.handle(&poll(None)).unwrap().snapshot.unwrap();
    assert_eq!(
        serde_json::to_value(&restored).unwrap(),
        serde_json::to_value(&snapshot).unwrap()
    );
    assert_eq!(reopened.sync_export().unwrap(), exported);
    let db = Connection::open(&database.0).unwrap();
    assert_eq!(
        db.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        3
    );
    assert_eq!(
        db.query_row("SELECT prior_version FROM metadata", [], |r| r
            .get::<_, u32>(0))
            .unwrap(),
        PRIOR_MODEL_VERSION
    );
    for (word, count, time) in [("啦去", 29, 1234), ("拉取", 40, 1250)] {
        let facts: (i64, i64) = db
            .query_row(
                "SELECT selections,last_used FROM entries WHERE text=?1",
                [word],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(facts, (count, time));
    }
}

#[test]
fn interrupted_prior_migration_rolls_back_schema_scores_and_revision_together() {
    let database = Database::new();
    database.seed_v2();
    let db = Connection::open(&database.0).unwrap();
    db.execute_batch(
        "CREATE TRIGGER deny_prior_migration BEFORE UPDATE OF prior_logp ON entries WHEN NEW.text='拉取'
        BEGIN SELECT RAISE(ABORT,'injected migration failure'); END;",
    )
    .unwrap();
    assert!(Store::open(&database.0, dictionary()).is_err());
    assert_eq!(
        db.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        db.query_row("SELECT revision FROM metadata", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        17
    );
    assert_eq!(
        db.query_row(
            "SELECT COUNT(*) FROM entries WHERE prior_logp IS NOT NULL",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        2
    );
    assert_eq!(
        db.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('metadata') WHERE name='prior_version'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

#[test]
fn newer_prior_models_are_rejected_without_resetting_the_database() {
    let database = Database::new();
    drop(Store::open(&database.0, dictionary()).unwrap());
    let db = Connection::open(&database.0).unwrap();
    db.execute(
        "UPDATE metadata SET prior_version=?1",
        [PRIOR_MODEL_VERSION + 1],
    )
    .unwrap();
    assert!(Store::open(&database.0, dictionary()).is_err());
    assert_eq!(
        db.query_row("SELECT prior_version FROM metadata", [], |r| r
            .get::<_, u32>(0))
            .unwrap(),
        PRIOR_MODEL_VERSION + 1
    );
}

#[test]
#[ignore = "requires RETYPE_TEST_DICTIONARY and optionally RETYPE_TEST_PACK_ROOT"]
fn installed_dictionary_replays_the_migration_without_accessing_user_storage() {
    let path =
        std::env::var_os("RETYPE_TEST_DICTIONARY").expect("set the installed base .bin path");
    let base = retype_dict::binary::open_shared(std::path::Path::new(&path)).unwrap();
    let total = base.total_frequency();
    let mut layers = LayeredDict::new();
    layers.push(retype_dict::Layer {
        name: "base",
        dict: base,
        boost: 0.,
    });
    if let Some(root) = std::env::var_os("RETYPE_TEST_PACK_ROOT") {
        for name in [
            "mingren", "renming", "yiren", "diming", "yixue", "yaopin", "huaxue",
        ] {
            let dict = retype_dict::binary::open_shared(
                &std::path::Path::new(&root).join(format!("{name}.bin")),
            )
            .unwrap();
            let boost = (dict.total_frequency() / total.max(1.)).ln() as f32;
            layers.push(retype_dict::Layer { name, dict, boost });
        }
    }
    verify_v2_migration(Arc::new(layers));
}
