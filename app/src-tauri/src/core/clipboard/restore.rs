use std::{error::Error, fmt};

use super::StorageError;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ClipboardRestorePayload {
    Text(String),
    Html {
        html: String,
        text: String,
    },
    Image {
        width: u32,
        height: u32,
        rgba8: Vec<u8>,
    },
    Files(Vec<String>),
}

#[derive(Debug)]
pub(crate) enum RestorePayloadError {
    Storage(StorageError),
    Invalid(String),
}

impl fmt::Display for RestorePayloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Storage(error) => write!(f, "{error}"),
            Self::Invalid(message) => f.write_str(message),
        }
    }
}

impl Error for RestorePayloadError {}
