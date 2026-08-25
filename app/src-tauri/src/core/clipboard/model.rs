use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ClipboardKind {
    Text,
    Html,
    Image,
    Files,
}

impl ClipboardKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Html => "html",
            Self::Image => "image",
            Self::Files => "files",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "text" => Some(Self::Text),
            "html" => Some(Self::Html),
            "image" => Some(Self::Image),
            "files" => Some(Self::Files),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ClipboardInput {
    Text {
        text: String,
        source_app: Option<String>,
    },
    Html {
        html: String,
        text: Option<String>,
        source_app: Option<String>,
    },
    Image {
        width: u32,
        height: u32,
        rgba8: Vec<u8>,
        source_app: Option<String>,
    },
    Files {
        files: Vec<String>,
        source_app: Option<String>,
    },
}

impl ClipboardInput {
    pub(crate) const fn kind(&self) -> ClipboardKind {
        match self {
            Self::Text { .. } => ClipboardKind::Text,
            Self::Html { .. } => ClipboardKind::Html,
            Self::Image { .. } => ClipboardKind::Image,
            Self::Files { .. } => ClipboardKind::Files,
        }
    }

    pub(crate) fn source_app(&self) -> Option<&str> {
        match self {
            Self::Text { source_app, .. }
            | Self::Html { source_app, .. }
            | Self::Image { source_app, .. }
            | Self::Files { source_app, .. } => source_app.as_deref(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ClipboardItem {
    pub(crate) id: i64,
    pub(crate) kind: ClipboardKind,
    pub(crate) text_content: Option<String>,
    pub(crate) html_content: Option<String>,
    pub(crate) image_path: Option<PathBuf>,
    pub(crate) files: Vec<String>,
    pub(crate) hash: String,
    pub(crate) source_app: Option<String>,
    pub(crate) created_at_ms: i64,
    pub(crate) last_used_at_ms: i64,
    pub(crate) favorite: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ClipboardImagePreview {
    pub(crate) png: Vec<u8>,
    pub(crate) width: u32,
    pub(crate) height: u32,
}
