use retype_dict::{annotate, format_syllables, Learner, UserDict};
use retype_types::{InputSource, LearningEvent};
use serde::{Deserialize, Serialize};

pub const VERSION: u32 = 1;
pub const MAX_FRAME: usize = 32 * 1024 * 1024;
pub const MAX_BATCH: usize = 128;

/// Stable spellings, never dictionary-specific numeric syllable IDs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Event {
    Chosen {
        text: String,
        pinyin: String,
        index: usize,
    },
    Coinage {
        text: String,
        pinyin: String,
    },
    Corrected {
        from: String,
        to: String,
    },
    Voice {
        text: String,
    },
}
impl Event {
    pub fn from_learning(event: LearningEvent) -> Option<Self> {
        let converted = match event {
            LearningEvent::CandidateChosen {
                text,
                syllables,
                index,
                ..
            } => Self::Chosen {
                text,
                pinyin: format_syllables(&syllables),
                index,
            },
            LearningEvent::Coinage { text, syllables } => Self::Coinage {
                text,
                pinyin: format_syllables(&syllables),
            },
            LearningEvent::Corrected { from, to, .. } => Self::Corrected { from, to },
            LearningEvent::VoiceCommit { text } => Self::Voice { text },
        };
        converted.to_learning().map(|_| converted)
    }
    pub fn to_learning(&self) -> Option<LearningEvent> {
        fn valid(text: &str) -> bool {
            !text.is_empty() && text.chars().count() <= 256
        }
        fn reading(pinyin: &str) -> Option<Vec<retype_types::SyllableId>> {
            if pinyin.len() > 512 {
                return None;
            }
            let ids = annotate::parse_pinyin(&pinyin.replace('\'', " "))?;
            (!ids.is_empty() && ids.len() <= 64).then_some(ids)
        }
        match self {
            Self::Chosen {
                text,
                pinyin,
                index,
            } if valid(text) && *index < 10000 => Some(LearningEvent::CandidateChosen {
                source: InputSource::Keyboard,
                text: text.clone(),
                syllables: reading(pinyin)?,
                index: *index,
            }),
            Self::Coinage { text, pinyin } if valid(text) => Some(LearningEvent::Coinage {
                text: text.clone(),
                syllables: reading(pinyin)?,
            }),
            Self::Corrected { from, to } if valid(from) && valid(to) => {
                Some(LearningEvent::Corrected {
                    from: from.clone(),
                    to: to.clone(),
                    context_hash: 0,
                })
            }
            Self::Voice { text } if valid(text) => {
                Some(LearningEvent::VoiceCommit { text: text.clone() })
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub pinyin: String,
    pub text: String,
    pub logp: f32,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Snapshot {
    pub entries: Vec<Entry>,
    pub corrections: Vec<(String, String, u32)>,
    #[serde(default)]
    pub ranking: Option<Ranking>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Ranking {
    pub tick: u64,
    pub records: Vec<Usage>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub pinyin: String,
    pub text: String,
    pub count: u64,
    pub recent: f64,
    pub last_tick: u64,
    pub prior_logp: Option<f32>,
}
impl Snapshot {
    pub fn capture(user: &UserDict, learner: &Learner) -> Self {
        let mut snapshot = Self {
            entries: user
                .snapshot()
                .into_iter()
                .map(|(ids, text, logp)| Entry {
                    pinyin: format_syllables(&ids),
                    text,
                    logp,
                })
                .collect(),
            corrections: learner.top_corrections(usize::MAX),
            ranking: {
                let history = user.usage_snapshot();
                Some(Ranking {
                    tick: history.tick,
                    records: history
                        .records
                        .into_iter()
                        .map(|record| Usage {
                            pinyin: format_syllables(&record.syllables),
                            text: record.text,
                            count: record.count,
                            recent: record.recent,
                            last_tick: record.last_tick,
                            prior_logp: record.prior_logp,
                        })
                        .collect(),
                })
            },
        };
        snapshot
            .entries
            .sort_by(|a, b| a.pinyin.cmp(&b.pinyin).then_with(|| a.text.cmp(&b.text)));
        if let Some(ranking) = &mut snapshot.ranking {
            ranking
                .records
                .sort_by(|a, b| a.pinyin.cmp(&b.pinyin).then_with(|| a.text.cmp(&b.text)));
        }
        snapshot
    }
    pub fn restore(&self, user: &UserDict, learner: &Learner) {
        let entries = self
            .entries
            .iter()
            .filter_map(|e| {
                let key = annotate::parse_pinyin(&e.pinyin.replace('\'', " "))?;
                Some((key, e.text.clone(), e.logp))
            })
            .collect();
        user.replace(entries);
        if let Some(ranking) = &self.ranking {
            user.restore_usage(retype_dict::adaptive::UsageSnapshot {
                tick: ranking.tick,
                records: ranking
                    .records
                    .iter()
                    .filter_map(|record| {
                        Some(retype_dict::adaptive::Usage {
                            syllables: annotate::parse_pinyin(&record.pinyin.replace('\'', " "))?,
                            text: record.text.clone(),
                            count: record.count,
                            recent: record.recent,
                            last_tick: record.last_tick,
                            prior_logp: record.prior_logp,
                        })
                    })
                    .collect(),
            });
        }
        learner.restore_corrections(self.corrections.clone());
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SequencedEvent {
    pub sequence: u64,
    pub event: Event,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Request {
    pub version: u32,
    pub client: String,
    pub known_revision: Option<u64>,
    pub events: Vec<SequencedEvent>,
    #[serde(default)]
    pub stop: bool,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Response {
    pub version: u32,
    pub revision: u64,
    pub acknowledged: u64,
    pub snapshot: Option<Snapshot>,
}
