//! Single-writer transactional storage. Raw keystrokes and event payloads are not logged.
use crate::protocol::{Event, Request, Response, Snapshot, MAX_BATCH, VERSION};
use retype_dict::{Learner, UserDict};
use retype_pinyin::Lexicon;
use retype_types::LearningStore;
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const SCHEMA_VERSION: u32 = 2;

pub struct Store {
    db: Connection,
    user: Arc<UserDict>,
    learner: Learner,
    revision: u64,
    snapshot: Snapshot,
    backup_path: PathBuf,
}
impl Store {
    pub fn open(path: &Path, system: Arc<dyn Lexicon>) -> rusqlite::Result<Self> {
        let db = Connection::open(path)?;
        db.busy_timeout(Duration::from_secs(2))?;
        let schema: u32 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if schema > SCHEMA_VERSION {
            return Err(rusqlite::Error::InvalidQuery);
        }
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            BEGIN IMMEDIATE;
            CREATE TABLE IF NOT EXISTS entries (
                pinyin TEXT NOT NULL, text TEXT NOT NULL, logp REAL NOT NULL,
                selections INTEGER NOT NULL DEFAULT 0, last_used INTEGER NOT NULL DEFAULT 0,
                usage_count INTEGER NOT NULL DEFAULT 0, recent REAL NOT NULL DEFAULT 0,
                last_tick INTEGER NOT NULL DEFAULT 0, prior_logp REAL,
                PRIMARY KEY(pinyin,text));
            CREATE TABLE IF NOT EXISTS corrections (
                original TEXT NOT NULL, replacement TEXT NOT NULL, count INTEGER NOT NULL,
                PRIMARY KEY(original,replacement));
            CREATE TABLE IF NOT EXISTS clients (id TEXT PRIMARY KEY, sequence INTEGER NOT NULL, seen INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS metadata (id INTEGER PRIMARY KEY CHECK(id=1), revision INTEGER NOT NULL, tick INTEGER NOT NULL DEFAULT 0);
            INSERT OR IGNORE INTO metadata(id,revision) VALUES(1,0);
            CREATE TABLE IF NOT EXISTS sync_identity (id TEXT PRIMARY KEY, value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS sync_english (origin TEXT NOT NULL,text TEXT NOT NULL,count INTEGER NOT NULL,PRIMARY KEY(origin,text));
            CREATE TABLE IF NOT EXISTS sync_words (origin TEXT NOT NULL,pinyin TEXT NOT NULL,text TEXT NOT NULL,count INTEGER NOT NULL,PRIMARY KEY(origin,pinyin,text));
            CREATE INDEX IF NOT EXISTS sync_words_key ON sync_words(pinyin,text);
            CREATE TABLE IF NOT EXISTS sync_corrections (origin TEXT NOT NULL,original TEXT NOT NULL,replacement TEXT NOT NULL,count INTEGER NOT NULL,PRIMARY KEY(origin,original,replacement));")?;
        // Preserve exact v1 selection counts. Old additive logp values are not a prior;
        // resolve real dictionary/segmented scores asynchronously instead of reusing them.
        if schema == 1 {
            db.execute_batch(
                "ALTER TABLE entries ADD COLUMN usage_count INTEGER NOT NULL DEFAULT 0;
                ALTER TABLE entries ADD COLUMN recent REAL NOT NULL DEFAULT 0;
                ALTER TABLE entries ADD COLUMN last_tick INTEGER NOT NULL DEFAULT 0;
                ALTER TABLE entries ADD COLUMN prior_logp REAL;
                ALTER TABLE metadata ADD COLUMN tick INTEGER NOT NULL DEFAULT 0;
                UPDATE entries SET usage_count=MAX(selections,1);
                UPDATE metadata SET tick=(SELECT COALESCE(SUM(usage_count),0) FROM entries);",
            )?;
        }
        db.execute_batch("PRAGMA user_version=2; COMMIT;")?;
        // Seed existing evidence exactly once; subsequent selections are owned
        // by a stable local origin, separate from imported cumulative counts.
        db.execute_batch("BEGIN IMMEDIATE;
            INSERT OR IGNORE INTO sync_identity VALUES('origin',lower(hex(randomblob(16))));
            INSERT INTO sync_words SELECT 'seed-'||(SELECT value FROM sync_identity WHERE id='origin'),pinyin,text,MAX(usage_count,selections,1) FROM entries WHERE NOT EXISTS(SELECT 1 FROM sync_identity WHERE id='seeded');
            INSERT INTO sync_corrections SELECT 'seed-'||(SELECT value FROM sync_identity WHERE id='origin'),original,replacement,count FROM corrections WHERE NOT EXISTS(SELECT 1 FROM sync_identity WHERE id='seeded');
            INSERT OR IGNORE INTO sync_identity VALUES('seeded','1'); COMMIT;")?;
        let user = Arc::new(UserDict::new());
        let learner = Learner::with_system(Arc::clone(&user), system);
        let mut store = Self {
            db,
            user,
            learner,
            revision: 0,
            snapshot: Snapshot::default(),
            backup_path: path.with_extension("backup.db"),
        };
        store.reload()?;
        Ok(store)
    }

    fn reload(&mut self) -> rusqlite::Result<()> {
        self.snapshot.english = self
            .db
            .prepare(
                "SELECT text,SUM(count) FROM sync_english GROUP BY text ORDER BY text LIMIT 10000",
            )?
            .query_map([], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as u64)))?
            .collect::<rusqlite::Result<_>>()?;
        self.revision =
            self.db
                .query_row("SELECT revision FROM metadata WHERE id=1", [], |row| {
                    row.get::<_, i64>(0)
                })? as u64;
        self.snapshot.entries = self
            .db
            .prepare("SELECT pinyin,text,logp FROM entries ORDER BY pinyin,text")?
            .query_map([], |row| {
                Ok(crate::protocol::Entry {
                    pinyin: row.get(0)?,
                    text: row.get(1)?,
                    logp: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        self.snapshot.corrections = self
            .db
            .prepare("SELECT original,replacement,count FROM corrections")?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let tick = self
            .db
            .query_row("SELECT tick FROM metadata WHERE id=1", [], |row| {
                row.get::<_, i64>(0)
            })? as u64;
        let records = self.db.prepare("SELECT pinyin,text,usage_count,recent,last_tick,prior_logp FROM entries WHERE usage_count>0")?
            .query_map([], |row| Ok(crate::protocol::Usage { pinyin: row.get(0)?, text: row.get(1)?,
                count: row.get::<_, i64>(2)? as u64, recent: row.get(3)?, last_tick: row.get::<_, i64>(4)? as u64,
                prior_logp: row.get(5)?, }))?.collect::<rusqlite::Result<_>>()?;
        self.snapshot.ranking = Some(crate::protocol::Ranking { tick, records });
        self.snapshot.restore(&self.user, &self.learner);
        Ok(())
    }

    pub fn handle(&mut self, request: &Request) -> rusqlite::Result<Response> {
        if request.version != VERSION
            || request.client.is_empty()
            || request.client.len() > 128
            || request.events.len() > MAX_BATCH
        {
            return Err(rusqlite::Error::InvalidParameterName(
                "learning protocol".into(),
            ));
        }
        let acknowledged = self
            .db
            .query_row(
                "SELECT sequence FROM clients WHERE id=?1",
                [&request.client],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .unwrap_or(0) as u64;
        self.apply(request, acknowledged)
    }

    fn apply(&mut self, request: &Request, mut acknowledged: u64) -> rusqlite::Result<Response> {
        let original_ack = acknowledged;
        let resolved = self.learner.refresh_priors();
        let mut applied = Vec::new();
        for event in &request.events {
            if event.sequence <= acknowledged {
                continue;
            }
            if event.sequence != acknowledged + 1 || event.sequence > i64::MAX as u64 {
                self.snapshot.restore(&self.user, &self.learner);
                return Err(rusqlite::Error::InvalidQuery);
            }
            let Some(learning) = event.event.to_learning() else {
                self.snapshot.restore(&self.user, &self.learner);
                return Err(rusqlite::Error::InvalidQuery);
            };
            self.learner.record(learning);
            acknowledged = event.sequence;
            applied.push(&event.event);
        }
        if acknowledged != original_ack || resolved {
            let before_usage: HashMap<_, _> = self
                .snapshot
                .ranking
                .as_ref()
                .into_iter()
                .flat_map(|ranking| &ranking.records)
                .map(|record| ((record.pinyin.clone(), record.text.clone()), record.clone()))
                .collect();
            let before: HashMap<_, _> = self
                .snapshot
                .entries
                .iter()
                .map(|entry| ((entry.pinyin.clone(), entry.text.clone()), entry.logp))
                .collect();
            let next = Snapshot::capture(&self.user, &self.learner);
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                .min(i64::MAX as u64) as i64;
            let committed = (|| {
                let tx = self.db.transaction()?;
                for entry in &next.entries {
                    if before
                        .get(&(entry.pinyin.clone(), entry.text.clone()))
                        .copied()
                        != Some(entry.logp)
                    {
                        tx.execute(
                            "INSERT INTO entries(pinyin,text,logp,last_used) VALUES(?1,?2,?3,?4)
                            ON CONFLICT(pinyin,text) DO UPDATE SET logp=excluded.logp",
                            params![entry.pinyin, entry.text, entry.logp, now],
                        )?;
                    }
                }
                if let Some(ranking) = &next.ranking {
                    for record in &ranking.records {
                        if before_usage.get(&(record.pinyin.clone(), record.text.clone()))
                            == Some(record)
                        {
                            continue;
                        }
                        tx.execute("UPDATE entries SET usage_count=?3,recent=?4,last_tick=?5,prior_logp=?6 WHERE pinyin=?1 AND text=?2",
                            params![record.pinyin, record.text, record.count.min(i64::MAX as u64) as i64,
                                record.recent, record.last_tick.min(i64::MAX as u64) as i64, record.prior_logp])?;
                        if before_usage
                            .get(&(record.pinyin.clone(), record.text.clone()))
                            .is_none_or(|old| old.count != record.count)
                        {
                            tx.execute(
                                "UPDATE entries SET last_used=?3 WHERE pinyin=?1 AND text=?2",
                                params![record.pinyin, record.text, now],
                            )?;
                        }
                    }
                    tx.execute(
                        "UPDATE metadata SET tick=?1 WHERE id=1",
                        [ranking.tick.min(i64::MAX as u64) as i64],
                    )?;
                }
                for event in &applied {
                    if let Event::Chosen { text, pinyin, .. } | Event::Coinage { text, pinyin } =
                        event
                    {
                        tx.execute("UPDATE entries SET selections=selections+1,last_used=?3 WHERE pinyin=?1 AND text=?2", params![pinyin,text,now])?;
                    }
                }
                let origin: String = tx.query_row(
                    "SELECT value FROM sync_identity WHERE id='origin'",
                    [],
                    |r| r.get(0),
                )?;
                for event in &applied {
                    if let Event::English { text } = event {
                        tx.execute("INSERT INTO sync_english VALUES(?1,?2,1) ON CONFLICT(origin,text) DO UPDATE SET count=count+1",params![origin,text])?;
                    }
                }
                if let Some(ranking) = &next.ranking {
                    for record in &ranking.records {
                        let previous = before_usage
                            .get(&(record.pinyin.clone(), record.text.clone()))
                            .map_or(0, |r| r.count);
                        let delta = record.count.saturating_sub(previous);
                        if delta > 0 {
                            tx.execute("INSERT INTO sync_words VALUES(?1,?2,?3,?4) ON CONFLICT(origin,pinyin,text) DO UPDATE SET count=count+excluded.count",params![origin,record.pinyin,record.text,delta.min(i64::MAX as u64) as i64])?;
                        }
                    }
                }
                for (from, to, count) in &next.corrections {
                    tx.execute("INSERT INTO corrections VALUES(?1,?2,?3) ON CONFLICT(original,replacement) DO UPDATE SET count=excluded.count", params![from,to,count])?;
                    let previous = self
                        .snapshot
                        .corrections
                        .iter()
                        .find(|(a, b, _)| a == from && b == to)
                        .map_or(0, |(_, _, n)| *n);
                    let delta = count.saturating_sub(previous);
                    if delta > 0 {
                        tx.execute("INSERT INTO sync_corrections VALUES(?1,?2,?3,?4) ON CONFLICT(origin,original,replacement) DO UPDATE SET count=count+excluded.count",params![origin,from,to,delta])?;
                    }
                }
                tx.execute("INSERT INTO clients VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET sequence=excluded.sequence,seen=excluded.seen", params![request.client, acknowledged as i64, now])?;
                tx.execute("UPDATE metadata SET revision=revision+1 WHERE id=1", [])?;
                tx.commit()
            })();
            if let Err(error) = committed {
                self.reload()?;
                return Err(error);
            }
            self.revision += 1;
            self.snapshot = next;
            // SQLite's backup API copies a consistent committed snapshot, including WAL data.
            // Failure to refresh the optional backup never rolls back an acknowledged commit.
            let due = std::fs::metadata(&self.backup_path)
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|time| time.elapsed().ok())
                .is_none_or(|elapsed| elapsed >= Duration::from_secs(86400));
            if due {
                if let Err(error) = self.db.backup(rusqlite::MAIN_DB, &self.backup_path, None) {
                    tracing::warn!(%error, "learning backup could not be refreshed");
                }
            }
        }
        Ok(Response {
            version: VERSION,
            revision: self.revision,
            acknowledged,
            snapshot: (request.known_revision != Some(self.revision))
                .then(|| self.snapshot.clone()),
        })
    }

    pub fn sync_export(&self) -> rusqlite::Result<crate::protocol::SyncLearning> {
        let words = self
            .db
            .prepare("SELECT origin,pinyin,text,count FROM sync_words ORDER BY origin,pinyin,text")?
            .query_map([], |r| {
                Ok(crate::protocol::SyncWord {
                    origin: r.get(0)?,
                    pinyin: r.get(1)?,
                    text: r.get(2)?,
                    count: r.get::<_, i64>(3)? as u64,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        let corrections = self.db.prepare("SELECT origin,original,replacement,count FROM sync_corrections ORDER BY origin,original,replacement")?
            .query_map([], |r| Ok(crate::protocol::SyncCorrection {origin:r.get(0)?,from:r.get(1)?,to:r.get(2)?,count:r.get(3)?}))?.collect::<rusqlite::Result<_>>()?;
        let english = self
            .db
            .prepare("SELECT origin,text,count FROM sync_english ORDER BY origin,text")?
            .query_map([], |r| {
                Ok(crate::protocol::SyncEnglish {
                    origin: r.get(0)?,
                    text: r.get(1)?,
                    count: r.get::<_, i64>(2)? as u64,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(crate::protocol::SyncLearning {
            words,
            corrections,
            english,
        })
    }

    pub fn sync_merge(&mut self, incoming: &crate::protocol::SyncLearning) -> rusqlite::Result<()> {
        if !incoming.valid() {
            return Err(rusqlite::Error::InvalidQuery);
        }
        use crate::protocol::Event;
        if incoming.words.len() > 100_000 || incoming.corrections.len() > 100_000 {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let origin_valid = |s: &str| {
            !s.is_empty()
                && s.len() <= 128
                && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        };
        for word in &incoming.words {
            if !origin_valid(&word.origin)
                || word.count == 0
                || word.count > 1_000_000_000
                || (Event::Coinage {
                    text: word.text.clone(),
                    pinyin: word.pinyin.clone(),
                })
                .to_learning()
                .is_none()
            {
                return Err(rusqlite::Error::InvalidQuery);
            }
        }
        for c in &incoming.corrections {
            if !origin_valid(&c.origin)
                || c.count == 0
                || c.count > 1_000_000_000
                || (Event::Corrected {
                    from: c.from.clone(),
                    to: c.to.clone(),
                })
                .to_learning()
                .is_none()
            {
                return Err(rusqlite::Error::InvalidQuery);
            }
        }
        let tick: i64 = self
            .db
            .query_row("SELECT tick FROM metadata WHERE id=1", [], |r| r.get(0))?;
        let tx = self.db.transaction()?;
        let mut changed = 0;
        for word in &incoming.english {
            changed+=tx.execute("INSERT INTO sync_english VALUES(?1,?2,?3) ON CONFLICT(origin,text) DO UPDATE SET count=excluded.count WHERE excluded.count>sync_english.count",params![word.origin,word.text,word.count as i64])?;
        }
        for word in &incoming.words {
            // Max also restores this device's own evidence after a local backup
            // rollback; a stale remote copy can never reduce live counters.
            changed+=tx.execute("INSERT INTO sync_words VALUES(?1,?2,?3,?4) ON CONFLICT(origin,pinyin,text) DO UPDATE SET count=excluded.count WHERE excluded.count>sync_words.count",params![word.origin,word.pinyin,word.text,word.count as i64])?;
            tx.execute("INSERT OR IGNORE INTO entries(pinyin,text,logp,usage_count,recent,last_tick) VALUES(?1,?2,-20,0,0,?3)",params![word.pinyin,word.text,tick])?;
        }
        for c in &incoming.corrections {
            changed+=tx.execute("INSERT INTO sync_corrections VALUES(?1,?2,?3,?4) ON CONFLICT(origin,original,replacement) DO UPDATE SET count=excluded.count WHERE excluded.count>sync_corrections.count",params![c.origin,c.from,c.to,c.count])?;
        }
        if changed > 0 {
            tx.execute_batch("UPDATE entries SET usage_count=(SELECT SUM(count) FROM sync_words WHERE pinyin=entries.pinyin AND text=entries.text);
                INSERT INTO corrections SELECT original,replacement,MIN(SUM(count),4294967295) FROM sync_corrections GROUP BY original,replacement HAVING 1 ON CONFLICT(original,replacement) DO UPDATE SET count=excluded.count;
                UPDATE metadata SET revision=revision+1 WHERE id=1;")?;
        }
        tx.commit()?;
        self.reload()
    }
}
