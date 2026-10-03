//! One learning cache and worker per host process, shared by all TSF activations.
use crate::protocol::{Event, Request, Response, SequencedEvent, Snapshot, MAX_BATCH, VERSION};
use crate::transport;
use retype_dict::{Learner, UserDict};
use retype_pinyin::Lexicon;
use retype_types::{LearningEvent, LearningStore};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub struct Client {
    english: Arc<Learner>,
    pub user: Arc<UserDict>,
    sender: SyncSender<Event>,
}
impl Client {
    pub fn start(system: Arc<dyn Lexicon>, fallback_host: Option<PathBuf>) -> Arc<Self> {
        Self::start_worker(system, fallback_host, None)
    }

    /// Isolated diagnostic endpoint; never launches the installed broker.
    #[doc(hidden)]
    pub fn for_endpoint(system: Arc<dyn Lexicon>, endpoint: String) -> Arc<Self> {
        Self::start_worker(system, None, Some(endpoint))
    }

    fn start_worker(
        system: Arc<dyn Lexicon>,
        fallback_host: Option<PathBuf>,
        endpoint: Option<String>,
    ) -> Arc<Self> {
        let user = Arc::new(UserDict::new());
        let (sender, receiver) = mpsc::sync_channel(1024);
        let learner = Arc::new(Learner::with_system(Arc::new(UserDict::new()), system));
        let client = Arc::new(Self {
            user: Arc::clone(&user),
            sender,
            english: Arc::clone(&learner),
        });
        let spawned = std::thread::Builder::new()
            .name("retype-learning-cache".into())
            .spawn(move || {
                run(user, learner, receiver, fallback_host, endpoint);
            });
        if spawned.is_err() {
            tracing::error!("could not start shared learning worker");
        }
        client
    }
}
impl LearningStore for Client {
    fn english_words(&self, prefix: &str, limit: usize) -> Vec<(String, u64)> {
        self.english.english_words(prefix, limit)
    }
    fn record(&self, event: LearningEvent) {
        if let Some(event) = Event::from_learning(event) {
            if self.sender.try_send(event).is_err() {
                tracing::warn!("learning queue unavailable; input remains usable");
            }
        }
    }
}

pub fn launch_host(fallback: Option<&std::path::Path>) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    let host = transport::read_machine_registry("ActiveDir")
        .map(|dir| PathBuf::from(dir).join("retype-learning-host.exe"))
        .filter(|path| path.is_file())
        .or_else(|| {
            fallback
                .filter(|path| path.is_file())
                .map(std::path::Path::to_path_buf)
        })
        .ok_or(std::io::ErrorKind::NotFound)?;
    std::process::Command::new(host)
        .arg("--serve")
        .creation_flags(0x08000000)
        .spawn()?;
    Ok(())
}

fn run(
    user: Arc<UserDict>,
    learner: Arc<Learner>,
    receiver: Receiver<Event>,
    fallback: Option<PathBuf>,
    endpoint: Option<String>,
) {
    let Ok(sid) = transport::user_sid() else {
        return;
    };
    let isolated = endpoint.is_some();
    let pipe = endpoint.unwrap_or_else(|| transport::pipe_name(&sid));
    let epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let id = format!("{}-{epoch}", std::process::id());
    let mut pending = VecDeque::new();
    let mut next_sequence = 1;
    let mut revision = None;
    let mut snapshot = Snapshot::default();
    let mut last_exchange = Instant::now() - Duration::from_secs(2);
    let mut last_launch = Instant::now() - Duration::from_secs(20);
    let mut disconnected = false;
    loop {
        // Backpressure is bounded and never reaches the input thread.
        if pending.len() < 4096 && !disconnected {
            match receiver.recv_timeout(Duration::from_millis(50)) {
                Ok(event) => {
                    if let Some(learning) = event.to_learning() {
                        learner.record(learning);
                        user.copy_from(learner.user());
                    }
                    pending.push_back(SequencedEvent {
                        sequence: next_sequence,
                        event,
                    });
                    next_sequence += 1;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => disconnected = true,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        } else {
            std::thread::sleep(Duration::from_millis(50));
        }
        let interval = if pending.is_empty() {
            Duration::from_secs(1)
        } else {
            Duration::from_millis(200)
        };
        if last_exchange.elapsed() < interval {
            continue;
        }
        last_exchange = Instant::now();
        let request = Request {
            version: VERSION,
            client: id.clone(),
            known_revision: revision,
            events: pending.iter().take(MAX_BATCH).cloned().collect(),
            stop: false,
        };
        let response = serde_json::to_vec(&request)
            .ok()
            .and_then(|bytes| transport::exchange(&pipe, &bytes).ok())
            .and_then(|bytes| serde_json::from_slice::<Response>(&bytes).ok())
            .filter(|response| {
                response.version == VERSION
                    && response.acknowledged
                        <= request
                            .events
                            .last()
                            .map(|event| event.sequence)
                            .unwrap_or_else(|| {
                                pending
                                    .front()
                                    .map(|event| event.sequence - 1)
                                    .unwrap_or(next_sequence - 1)
                            })
                    && (response.snapshot.is_some() || revision == Some(response.revision))
            });
        if let Some(response) = response {
            while pending
                .front()
                .is_some_and(|event| event.sequence <= response.acknowledged)
            {
                pending.pop_front();
            }
            if let Some(updated) = response.snapshot {
                snapshot = updated;
            }
            // Rebuild from authoritative values plus only still-unacknowledged local events.
            // This prevents a persisted event being learned twice when its response was lost.
            if revision != Some(response.revision) || !request.events.is_empty() {
                snapshot.restore(learner.user(), &learner);
                for event in &pending {
                    if let Some(learning) = event.event.to_learning() {
                        learner.record(learning);
                    }
                }
                user.copy_from(learner.user());
            }
            revision = Some(response.revision);
        } else if !isolated && last_launch.elapsed() >= Duration::from_secs(10) {
            last_launch = Instant::now();
            let _ = launch_host(fallback.as_deref());
        }
        if learner.refresh_priors() {
            user.copy_from(learner.user());
        }
        if disconnected && pending.is_empty() {
            break;
        }
    }
}
