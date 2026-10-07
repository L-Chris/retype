use serde::{Deserialize, Serialize};
pub const MAX_TEXT: usize = 65_536;
#[derive(Clone, Serialize, Deserialize)]
pub enum Operation {
    Translate {
        text: String,
    },
    Models {
        provider: crate::config::Provider,
    },
    Test {
        provider: crate::config::Provider,
        model: String,
    },
}
#[derive(Serialize, Deserialize)]
pub enum Request {
    Voice(crate::voice::Command),
    Start { id: String, operation: Operation },
    Poll { id: String },
    Cancel { id: String },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Response {
    Voice(crate::voice::Snapshot),
    Pending,
    Translation { text: String, preview: bool },
    Models(Vec<String>),
    Tested,
    Error(String),
}
