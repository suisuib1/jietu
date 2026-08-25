use std::{error::Error, fmt, io, time::Duration};

use super::{ClipboardInput, ClipboardItem, PrivacyRejection};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum RecordOutcome {
    Inserted(ClipboardItem),
    Duplicate(ClipboardItem),
    Ignored(PrivacyRejection),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RetentionPolicy {
    pub(crate) max_items: usize,
    pub(crate) max_age: Option<Duration>,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            max_items: 1_000,
            max_age: Some(Duration::from_secs(30 * 24 * 60 * 60)),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct RetentionResult {
    pub(crate) deleted_items: usize,
    pub(crate) deleted_images: usize,
}

#[derive(Debug)]
pub(crate) enum StorageError {
    Database(rusqlite::Error),
    Io(io::Error),
    InvalidData(String),
    LockPoisoned,
    UnsupportedSchema(i64),
}

impl fmt::Display for StorageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Database(error) => write!(formatter, "clipboard database error: {error}"),
            Self::Io(error) => write!(formatter, "clipboard image storage error: {error}"),
            Self::InvalidData(message) => write!(formatter, "invalid clipboard data: {message}"),
            Self::LockPoisoned => formatter.write_str("clipboard storage lock was poisoned"),
            Self::UnsupportedSchema(version) => {
                write!(formatter, "unsupported clipboard schema version: {version}")
            }
        }
    }
}

impl Error for StorageError {}

impl From<rusqlite::Error> for StorageError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Database(value)
    }
}

impl From<io::Error> for StorageError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

pub(crate) trait ClipboardStorage: Send + Sync {
    fn record(&self, input: ClipboardInput, now_ms: i64) -> Result<RecordOutcome, StorageError>;
    fn get(&self, id: i64) -> Result<Option<ClipboardItem>, StorageError>;
    fn list(&self, limit: usize) -> Result<Vec<ClipboardItem>, StorageError>;
    fn search(&self, query: &str, limit: usize) -> Result<Vec<ClipboardItem>, StorageError>;
    fn set_favorite(&self, id: i64, favorite: bool) -> Result<bool, StorageError>;
    fn enforce_retention(&self, now_ms: i64) -> Result<RetentionResult, StorageError>;
}
