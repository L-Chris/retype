#![cfg(feature = "broker")]
#![allow(clippy::unwrap_used, clippy::expect_used)]
use retype_dict::{Learner, UserDict};
use retype_learning::protocol::*;
use retype_learning::store::Store;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("retype-learning-{}-{id}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn db(&self) -> PathBuf {
        self.0.join("user.db")
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn store(path: &std::path::Path) -> Store {
    Store::open(
        path,
        Arc::new(retype_dict::from_pairs([("你好", "ni hao", 100.)]).0),
    )
    .unwrap()
}
fn request(client: &str, sequence: u64, event: Event) -> Request {
    Request {
        version: VERSION,
        client: client.into(),
        known_revision: None,
        events: vec![SequencedEvent { sequence, event }],
        stop: false,
    }
}
fn chosen() -> Event {
    Event::Chosen {
        pinyin: "ni'hao".into(),
        text: "你好".into(),
        index: 2,
    }
}

#[test]
fn cloud_roundtrips_merge_counts_once_and_keep_local_recent_clock() {
    let a_dir = Directory::new();
    let b_dir = Directory::new();
    let mut a = store(&a_dir.db());
    let mut b = store(&b_dir.db());
    a.handle(&request("a", 1, chosen())).unwrap();
    let own_origin = a.sync_export().unwrap().words[0].origin.clone();
    b.handle(&request("b", 1, chosen())).unwrap();
    let b_snapshot = b.sync_export().unwrap();
    a.sync_merge(&b_snapshot).unwrap();
    a.sync_merge(&b_snapshot).unwrap();
    let after = a.sync_export().unwrap();
    assert_eq!(after.words.iter().map(|w| w.count).sum::<u64>(), 2);
    b.sync_merge(&after).unwrap();
    b.handle(&request("b", 2, chosen())).unwrap();
    a.sync_merge(&b.sync_export().unwrap()).unwrap();
    assert_eq!(
        a.sync_export()
            .unwrap()
            .words
            .iter()
            .map(|w| w.count)
            .sum::<u64>(),
        3
    );
    drop(a);
    let mut a = store(&a_dir.db());
    a.sync_merge(&b.sync_export().unwrap()).unwrap();
    let before = a.sync_export().unwrap();
    assert_eq!(before.words.iter().map(|w| w.count).sum::<u64>(), 3);
    let result = a.handle(&request("a", 2, chosen())).unwrap();
    let ranking = result.snapshot.unwrap().ranking.unwrap();
    assert_eq!(ranking.tick, 2);
    assert_eq!(ranking.records[0].count, 4);
    assert!(ranking.records[0].recent > 1.0 && ranking.records[0].recent < 2.1);
    let valid = a.sync_export().unwrap();
    let mut invalid = valid.clone();
    invalid.words[0].count = u64::MAX;
    assert!(a.sync_merge(&invalid).is_err());
    assert_eq!(a.sync_export().unwrap(), valid);
    let mut restored = valid.clone();
    for word in &mut restored.words {
        if word.origin == own_origin {
            word.count += 3;
        }
    }
    a.sync_merge(&restored).unwrap();
    a.sync_merge(&restored).unwrap();
    assert_eq!(
        a.sync_export()
            .unwrap()
            .words
            .iter()
            .map(|w| w.count)
            .sum::<u64>(),
        7
    );
}

#[test]
fn restart_keeps_scores_readings_receipts_and_usage_facts() {
    let directory = Directory::new();
    let mut broker = store(&directory.db());
    let event = request("notepad", 1, chosen());
    let first = broker.handle(&event).unwrap();
    let score = first.snapshot.unwrap().entries[0].logp;
    drop(broker);
    let mut broker = store(&directory.db());
    let replay = broker.handle(&event).unwrap();
    assert_eq!(replay.revision, 1);
    assert_eq!(replay.acknowledged, 1);
    assert_eq!(replay.snapshot.unwrap().entries[0].logp, score);
    let db = rusqlite::Connection::open(directory.db()).unwrap();
    let facts: (String, i64, i64) = db
        .query_row(
            "SELECT pinyin,selections,last_used FROM entries",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(facts.0, "ni'hao");
    assert_eq!(facts.1, 1);
    assert!(facts.2 > 0);
    assert!(directory.db().with_extension("backup.db").is_file());
}

#[test]
fn different_apps_accumulate_without_overwriting_each_other() {
    let directory = Directory::new();
    let mut broker = store(&directory.db());
    let a = broker.handle(&request("app-a", 1, chosen())).unwrap();
    let b = broker.handle(&request("app-b", 1, chosen())).unwrap();
    assert_eq!(
        b.snapshot
            .as_ref()
            .unwrap()
            .ranking
            .as_ref()
            .unwrap()
            .records[0]
            .count,
        2
    );
    assert_eq!(
        a.snapshot
            .as_ref()
            .unwrap()
            .ranking
            .as_ref()
            .unwrap()
            .records[0]
            .count,
        1
    );
    let poll = Request {
        version: VERSION,
        client: "app-a".into(),
        known_revision: Some(a.revision),
        events: vec![],
        stop: false,
    };
    let refreshed = broker.handle(&poll).unwrap();
    assert_eq!(refreshed.revision, b.revision);
    assert_eq!(refreshed.acknowledged, 1);
    assert_eq!(
        refreshed.snapshot.unwrap().entries[0].logp,
        b.snapshot.unwrap().entries[0].logp
    );
}

#[test]
fn invalid_batch_rolls_back_memory_and_disk_together() {
    let directory = Directory::new();
    let mut broker = store(&directory.db());
    let mut batch = request("app", 1, chosen());
    batch.events.push(SequencedEvent {
        sequence: 3,
        event: chosen(),
    });
    assert!(broker.handle(&batch).is_err());
    let valid = broker.handle(&request("app", 1, chosen())).unwrap();
    assert_eq!(valid.revision, 1);
    assert_eq!(valid.snapshot.unwrap().entries.len(), 1);
    let db = rusqlite::Connection::open(directory.db()).unwrap();
    assert_eq!(
        db.query_row("SELECT selections FROM entries", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn confirmed_corrections_span_apps_and_survive_restart() {
    let directory = Directory::new();
    let correction = Event::Corrected {
        from: "泥好".into(),
        to: "你好".into(),
    };
    let mut broker = store(&directory.db());
    assert!(broker
        .handle(&request("a", 1, correction.clone()))
        .unwrap()
        .snapshot
        .unwrap()
        .entries
        .is_empty());
    drop(broker);
    let mut broker = store(&directory.db());
    let result = broker
        .handle(&request("b", 1, correction))
        .unwrap()
        .snapshot
        .unwrap();
    assert_eq!(result.corrections[0].2, 2);
    assert_eq!(result.entries[0].text, "你好");
}

#[test]
fn full_pinyin_and_double_pinyin_restore_the_same_lexicon() {
    let source = LearningEventFixture::chosen();
    let event = Event::from_learning(source).unwrap();
    let reconstructed = event.to_learning().unwrap();
    let user = Arc::new(UserDict::new());
    let learner = Learner::new(Arc::clone(&user));
    retype_types::LearningStore::record(&learner, reconstructed);
    let snapshot = Snapshot::capture(&user, &learner);
    let other = Arc::new(UserDict::new());
    snapshot.restore(&other, &Learner::new(Arc::clone(&other)));
    let mut entries = Vec::new();
    retype_pinyin::Lexicon::lookup(
        &*other,
        &retype_dict::annotate::parse_pinyin("ni hao").unwrap(),
        &mut entries,
    );
    assert_eq!(&*entries[0].text, "你好");
}
struct LearningEventFixture;
impl LearningEventFixture {
    fn chosen() -> retype_types::LearningEvent {
        retype_types::LearningEvent::CandidateChosen {
            source: retype_types::InputSource::Keyboard,
            text: "你好".into(),
            syllables: retype_dict::annotate::parse_pinyin("ni hao").unwrap(),
            index: 2,
        }
    }
}

#[test]
fn newer_schema_and_corrupt_files_are_never_silently_reset() {
    let directory = Directory::new();
    let db = rusqlite::Connection::open(directory.db()).unwrap();
    db.pragma_update(None, "user_version", 999).unwrap();
    drop(db);
    assert!(Store::open(&directory.db(), Arc::new(UserDict::new())).is_err());
    let db = rusqlite::Connection::open(directory.db()).unwrap();
    assert_eq!(
        db.pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
            .unwrap(),
        999
    );
    drop(db);
    let corrupt = directory.0.join("corrupt.db");
    std::fs::write(&corrupt, b"keep this damaged file").unwrap();
    assert!(Store::open(&corrupt, Arc::new(UserDict::new())).is_err());
    assert_eq!(std::fs::read(corrupt).unwrap(), b"keep this damaged file");
}

#[test]
fn migration_preserves_v1_counts_and_receipts_but_does_not_reuse_saturated_scores() {
    let directory = Directory::new();
    let db = rusqlite::Connection::open(directory.db()).unwrap();
    db.execute_batch("CREATE TABLE entries(pinyin TEXT,text TEXT,logp REAL,selections INTEGER,last_used INTEGER,PRIMARY KEY(pinyin,text));
        CREATE TABLE corrections(original TEXT,replacement TEXT,count INTEGER,PRIMARY KEY(original,replacement));
        CREATE TABLE clients(id TEXT PRIMARY KEY,sequence INTEGER,seen INTEGER);
        CREATE TABLE metadata(id INTEGER PRIMARY KEY,revision INTEGER);
        INSERT INTO entries VALUES('ni''hao','你好',2.0,100,1234);
        INSERT INTO clients VALUES('old',1,1234); INSERT INTO metadata VALUES(1,7);
        PRAGMA user_version=1;").unwrap();
    drop(db);
    let mut broker = store(&directory.db());
    let replay = broker
        .handle(&request("old", 1, chosen()))
        .unwrap()
        .snapshot
        .unwrap();
    let usage = &replay.ranking.unwrap().records[0];
    assert_eq!(usage.count, 100);
    assert_eq!(
        usage.recent, 0.,
        "v1 did not record recency; do not invent it"
    );
    assert_eq!(
        usage.prior_logp,
        Some(0.),
        "the fixture's only dictionary word has probability 1, not old +2"
    );
    let db = rusqlite::Connection::open(directory.db()).unwrap();
    let facts: (i64, i64) = db
        .query_row("SELECT selections,last_used FROM entries", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!(facts, (100, 1234));
    assert_eq!(
        db.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        2
    );
}

#[test]
fn recency_clock_and_preference_survive_restart_and_replays() {
    let directory = Directory::new();
    let mut broker = store(&directory.db());
    let mut last = None;
    for sequence in 1..=8 {
        last = Some(broker.handle(&request("app", sequence, chosen())).unwrap());
    }
    let original = last.unwrap().snapshot.unwrap().ranking.unwrap();
    drop(broker);
    let mut broker = store(&directory.db());
    let replay = broker
        .handle(&request("app", 8, chosen()))
        .unwrap()
        .snapshot
        .unwrap()
        .ranking
        .unwrap();
    assert_eq!(replay.tick, original.tick);
    assert_eq!(replay.records, original.records);
}
