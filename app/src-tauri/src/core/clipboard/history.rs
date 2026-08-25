use std::{path::Path, sync::Arc};

use serde::Serialize;

use super::{ClipboardImagePreview, ClipboardItem, ClipboardKind, ClipboardStorage, StorageError};

const PREVIEW_TEXT_LIMIT: usize = 240;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ClipboardHistorySummary {
    pub(crate) id: i64,
    pub(crate) kind: ClipboardKind,
    pub(crate) preview_text: String,
    pub(crate) source_application: Option<String>,
    pub(crate) created_at_ms: i64,
    pub(crate) last_used_at_ms: i64,
    pub(crate) is_favorite: bool,
    pub(crate) file_count: usize,
    pub(crate) image_available: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ClipboardHistoryDetail {
    pub(crate) id: i64,
    pub(crate) kind: ClipboardKind,
    pub(crate) text_content: Option<String>,
    pub(crate) html_content: Option<String>,
    pub(crate) files: Vec<String>,
    pub(crate) source_application: Option<String>,
    pub(crate) created_at_ms: i64,
    pub(crate) last_used_at_ms: i64,
    pub(crate) is_favorite: bool,
    pub(crate) image_available: bool,
}

#[derive(Clone)]
pub(crate) struct ClipboardHistoryService {
    storage: Arc<dyn ClipboardStorage>,
}

impl ClipboardHistoryService {
    pub(crate) fn new(storage: Arc<dyn ClipboardStorage>) -> Self {
        Self { storage }
    }

    pub(crate) fn list(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<ClipboardHistorySummary>, StorageError> {
        self.storage
            .list_page(offset, limit)
            .map(summaries_from_items)
    }

    pub(crate) fn search(
        &self,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<ClipboardHistorySummary>, StorageError> {
        self.storage
            .search_page(query, offset, limit)
            .map(summaries_from_items)
    }

    pub(crate) fn count(&self, query: Option<&str>) -> Result<usize, StorageError> {
        self.storage.count(query)
    }

    pub(crate) fn get(&self, id: i64) -> Result<Option<ClipboardHistoryDetail>, StorageError> {
        self.storage.get(id).map(|item| item.map(detail_from_item))
    }

    pub(crate) fn delete(&self, id: i64) -> Result<bool, StorageError> {
        self.storage.delete(id)
    }

    pub(crate) fn set_favorite(&self, id: i64, favorite: bool) -> Result<bool, StorageError> {
        self.storage.set_favorite(id, favorite)
    }

    pub(crate) fn image_preview(
        &self,
        id: i64,
        max_width: u32,
        max_height: u32,
    ) -> Result<Option<ClipboardImagePreview>, StorageError> {
        self.storage.image_preview(id, max_width, max_height)
    }
}

fn summaries_from_items(items: Vec<ClipboardItem>) -> Vec<ClipboardHistorySummary> {
    items.into_iter().map(summary_from_item).collect()
}

fn summary_from_item(item: ClipboardItem) -> ClipboardHistorySummary {
    let preview_text = match item.kind {
        ClipboardKind::Text | ClipboardKind::Html => item
            .text_content
            .as_deref()
            .map(compact_preview)
            .unwrap_or_default(),
        ClipboardKind::Image => String::new(),
        ClipboardKind::Files => files_preview(&item.files),
    };
    ClipboardHistorySummary {
        id: item.id,
        kind: item.kind,
        preview_text,
        source_application: item.source_app,
        created_at_ms: item.created_at_ms,
        last_used_at_ms: item.last_used_at_ms,
        is_favorite: item.favorite,
        file_count: item.files.len(),
        image_available: item.image_path.as_deref().is_some_and(Path::is_file),
    }
}

fn detail_from_item(item: ClipboardItem) -> ClipboardHistoryDetail {
    ClipboardHistoryDetail {
        id: item.id,
        kind: item.kind,
        text_content: item.text_content,
        html_content: item.html_content,
        files: item.files,
        source_application: item.source_app,
        created_at_ms: item.created_at_ms,
        last_used_at_ms: item.last_used_at_ms,
        is_favorite: item.favorite,
        image_available: item.image_path.as_deref().is_some_and(Path::is_file),
    }
}

fn compact_preview(value: &str) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= PREVIEW_TEXT_LIMIT {
        return compact;
    }
    let mut preview = compact
        .chars()
        .take(PREVIEW_TEXT_LIMIT.saturating_sub(3))
        .collect::<String>();
    preview.push_str("...");
    preview
}

fn files_preview(files: &[String]) -> String {
    let Some(first) = files.first() else {
        return String::new();
    };
    let name = first
        .rsplit(['/', '\\'])
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or(first);
    if files.len() > 1 {
        format!("{name} +{}", files.len() - 1)
    } else {
        name.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    use crate::core::clipboard::{
        ClipboardInput, PrivacyPolicy, RetentionPolicy, SqliteClipboardStorage,
    };

    use super::*;

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new(name: &str) -> Self {
            let nonce = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "litesnap-history-{name}-{}-{timestamp}-{nonce}",
                std::process::id()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn service(directory: &TestDirectory) -> ClipboardHistoryService {
        ClipboardHistoryService::new(Arc::new(
            SqliteClipboardStorage::open(
                &directory.0,
                PrivacyPolicy::default(),
                RetentionPolicy::default(),
            )
            .unwrap(),
        ))
    }

    #[test]
    fn history_summary_maps_compact_safe_fields() {
        let directory = TestDirectory::new("summary");
        let service = service(&directory);
        service
            .storage
            .record(
                ClipboardInput::Html {
                    html: "<script>alert(1)</script>".into(),
                    text: Some("Release\n  notes".into()),
                    source_app: Some("Browser".into()),
                },
                10,
            )
            .unwrap();

        let summary = service.list(0, 1).unwrap().remove(0);
        assert_eq!(summary.preview_text, "Release notes");
        assert_eq!(summary.source_application.as_deref(), Some("Browser"));
        assert!(!summary.image_available);
    }

    #[test]
    fn favorite_service_updates_item_and_survives_deduplication() {
        let directory = TestDirectory::new("favorite");
        let service = service(&directory);
        let input = ClipboardInput::Text {
            text: "keep".into(),
            source_app: None,
        };
        service.storage.record(input.clone(), 10).unwrap();
        let id = service.list(0, 1).unwrap()[0].id;
        assert!(service.set_favorite(id, true).unwrap());
        service.storage.record(input, 20).unwrap();
        assert!(service.get(id).unwrap().unwrap().is_favorite);
    }

    #[test]
    fn missing_history_item_returns_none() {
        let directory = TestDirectory::new("missing");
        assert!(service(&directory).get(99_999).unwrap().is_none());
    }
}
