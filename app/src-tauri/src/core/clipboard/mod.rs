mod hash;
mod model;
mod privacy;
mod sqlite;
mod storage;

pub(crate) use hash::content_hash;
pub(crate) use model::{ClipboardInput, ClipboardItem, ClipboardKind};
pub(crate) use privacy::{PrivacyPolicy, PrivacyRejection};
pub(crate) use sqlite::SqliteClipboardStorage;
pub(crate) use storage::{
    ClipboardStorage, RecordOutcome, RetentionPolicy, RetentionResult, StorageError,
};

#[cfg(test)]
mod tests;
