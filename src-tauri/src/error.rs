//! BLACKVAULT — error type. Every fallible core operation returns `BvResult`,
//! which serialises to a plain string across the IPC boundary. Nothing internal
//! (paths, key material, SQL) is ever leaked verbatim to the UI: we map to a
//! short, human-readable message.

use serde::{Serialize, Serializer};

#[derive(Debug, thiserror::Error)]
pub enum BvError {
    #[error("{0}")]
    Message(String),

    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),

    #[error("serialisation error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("cryptographic error")]
    Crypto,

    #[error("not authenticated")]
    Unauthenticated,

    #[error("rate limited")]
    RateLimited,

    #[error("not found")]
    NotFound,

    #[error("unsupported on this platform: {0}")]
    Unsupported(String),
}

impl BvError {
    pub fn msg(s: impl Into<String>) -> Self {
        BvError::Message(s.into())
    }
}

impl Serialize for BvError {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

pub type BvResult<T> = Result<T, BvError>;
