use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use rusqlite::Connection;

use super::*;

static NEXT_TEST_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(name: &str) -> Self {
        let nonce = NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "litesnap-clipboard-{name}-{}-{timestamp}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create test directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn storage(
    directory: &TestDirectory,
    privacy: PrivacyPolicy,
    retention: RetentionPolicy,
) -> SqliteClipboardStorage {
    SqliteClipboardStorage::open(directory.path(), privacy, retention).expect("open storage")
}

fn text(value: &str, source_app: Option<&str>) -> ClipboardInput {
    ClipboardInput::Text {
        text: value.into(),
        source_app: source_app.map(str::to_owned),
    }
}

fn inserted(outcome: RecordOutcome) -> ClipboardItem {
    match outcome {
        RecordOutcome::Inserted(item) => item,
        other => panic!("expected inserted item, got {other:?}"),
    }
}

#[test]
fn canonical_text_hash_is_stable_and_ignores_source_metadata() {
    let first = text("hello", Some("WindowsApp.exe"));
    let second = text("hello", Some("com.example.macos"));
    assert_eq!(content_hash(&first), content_hash(&second));
    assert_eq!(
        content_hash(&first),
        "baf3b18850afed9d27a03be193f1b7ad4b81de141ce8c0fbd2670ab91a86eb6b"
    );
}

#[test]
fn canonical_image_hash_uses_dimensions_and_rgba8_pixels() {
    let pixels = vec![255, 0, 0, 255, 0, 255, 0, 128];
    let first = ClipboardInput::Image {
        width: 2,
        height: 1,
        rgba8: pixels.clone(),
        source_app: Some("windows".into()),
    };
    let same = ClipboardInput::Image {
        width: 2,
        height: 1,
        rgba8: pixels.clone(),
        source_app: Some("macos".into()),
    };
    let different_dimensions = ClipboardInput::Image {
        width: 1,
        height: 2,
        rgba8: pixels.clone(),
        source_app: None,
    };
    let mut changed_pixels = pixels;
    changed_pixels[7] = 127;
    let different_pixels = ClipboardInput::Image {
        width: 2,
        height: 1,
        rgba8: changed_pixels,
        source_app: None,
    };

    assert_eq!(content_hash(&first), content_hash(&same));
    assert_ne!(content_hash(&first), content_hash(&different_dimensions));
    assert_ne!(content_hash(&first), content_hash(&different_pixels));
}

#[test]
fn privacy_guards_reject_excluded_empty_oversized_and_invalid_inputs() {
    let mut policy = PrivacyPolicy::default();
    policy.exclude_source_app("Password Manager");
    policy.max_text_bytes = 4;
    policy.max_files = 1;

    assert_eq!(
        policy.validate(&text("secret", Some(" password MANAGER "))),
        Err(PrivacyRejection::ExcludedSource)
    );
    assert_eq!(
        policy.validate(&text("", None)),
        Err(PrivacyRejection::EmptyContent)
    );
    assert_eq!(
        policy.validate(&text("12345", None)),
        Err(PrivacyRejection::ContentTooLarge)
    );
    assert_eq!(
        policy.validate(&ClipboardInput::Image {
            width: 2,
            height: 2,
            rgba8: vec![0; 15],
            source_app: None,
        }),
        Err(PrivacyRejection::InvalidImage)
    );
    assert_eq!(
        policy.validate(&ClipboardInput::Files {
            files: vec!["one".into(), "two".into()],
            source_app: None,
        }),
        Err(PrivacyRejection::TooManyFiles)
    );
}

#[test]
fn migration_creates_v1_schema_and_indexes() {
    let directory = TestDirectory::new("migration");
    let store = storage(
        &directory,
        PrivacyPolicy::default(),
        RetentionPolicy::default(),
    );
    assert_eq!(store.database_path(), directory.path().join("clipboard.db"));
    assert_eq!(
        store.image_directory(),
        directory.path().join("ClipboardImages")
    );

    let connection = Connection::open(store.database_path()).expect("inspect database");
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("schema version");
    let table_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'clipboard_items'",
            [],
            |row| row.get(0),
        )
        .expect("table count");
    assert_eq!(version, 1);
    assert_eq!(table_count, 1);
}

#[test]
fn newer_schema_is_rejected_without_mutation() {
    let directory = TestDirectory::new("future-schema");
    let database = directory.path().join("clipboard.db");
    let connection = Connection::open(&database).expect("create database");
    connection
        .pragma_update(None, "user_version", 2)
        .expect("set future version");
    drop(connection);

    let error = match SqliteClipboardStorage::open(
        directory.path(),
        PrivacyPolicy::default(),
        RetentionPolicy::default(),
    ) {
        Ok(_) => panic!("future schema should be rejected"),
        Err(error) => error,
    };
    assert!(matches!(error, StorageError::UnsupportedSchema(2)));
}

#[test]
fn duplicate_content_reuses_item_and_updates_last_used_time() {
    let directory = TestDirectory::new("dedup");
    let store = storage(
        &directory,
        PrivacyPolicy::default(),
        RetentionPolicy::default(),
    );
    let first = inserted(store.record(text("same", Some("first")), 10).unwrap());
    let duplicate = match store.record(text("same", Some("second")), 25).unwrap() {
        RecordOutcome::Duplicate(item) => item,
        other => panic!("expected duplicate item, got {other:?}"),
    };

    assert_eq!(first.id, duplicate.id);
    assert_eq!(first.hash, duplicate.hash);
    assert_eq!(duplicate.created_at_ms, 10);
    assert_eq!(duplicate.last_used_at_ms, 25);
    assert_eq!(duplicate.source_app.as_deref(), Some("second"));
    assert_eq!(store.list(20).unwrap().len(), 1);
}

#[test]
fn canonical_image_deduplicates_and_round_trips_rgba8() {
    let directory = TestDirectory::new("image");
    let store = storage(
        &directory,
        PrivacyPolicy::default(),
        RetentionPolicy::default(),
    );
    let rgba8 = vec![255, 0, 0, 255, 0, 255, 0, 128];
    let item = inserted(
        store
            .record(
                ClipboardInput::Image {
                    width: 2,
                    height: 1,
                    rgba8: rgba8.clone(),
                    source_app: None,
                },
                10,
            )
            .unwrap(),
    );
    let item_id = item.id;
    let image_path = item.image_path.expect("image path");
    assert_eq!(image_path.parent(), Some(store.image_directory()));
    assert_eq!(image_path.file_name().unwrap().to_string_lossy().len(), 68);

    let decoded = image::open(&image_path).expect("decode PNG").into_rgba8();
    assert_eq!(decoded.dimensions(), (2, 1));
    assert_eq!(decoded.into_raw(), rgba8);

    let duplicate = store
        .record(
            ClipboardInput::Image {
                width: 2,
                height: 1,
                rgba8: vec![255, 0, 0, 255, 0, 255, 0, 128],
                source_app: Some("macos".into()),
            },
            20,
        )
        .unwrap();
    let duplicate = match duplicate {
        RecordOutcome::Duplicate(item) => item,
        other => panic!("expected duplicate image, got {other:?}"),
    };
    assert_eq!(duplicate.id, item_id);
    assert_eq!(duplicate.image_path.as_deref(), Some(image_path.as_path()));
    assert_eq!(store.list(10).unwrap().len(), 1);
}

#[test]
fn html_files_search_and_favorites_round_trip() {
    let directory = TestDirectory::new("search");
    let store = storage(
        &directory,
        PrivacyPolicy::default(),
        RetentionPolicy::default(),
    );
    let html = inserted(
        store
            .record(
                ClipboardInput::Html {
                    html: "<strong>Release Notes</strong>".into(),
                    text: Some("Release Notes".into()),
                    source_app: Some("Browser".into()),
                },
                10,
            )
            .unwrap(),
    );
    let files = inserted(
        store
            .record(
                ClipboardInput::Files {
                    files: vec!["C:/Reports/quarterly.csv".into()],
                    source_app: Some("Explorer".into()),
                },
                20,
            )
            .unwrap(),
    );

    assert_eq!(store.search("release", 10).unwrap(), vec![html.clone()]);
    assert_eq!(store.search("quarterly", 10).unwrap(), vec![files.clone()]);
    assert!(store.set_favorite(html.id, true).unwrap());
    assert!(!store.set_favorite(99_999, true).unwrap());
    let listed = store.list(10).unwrap();
    assert_eq!(listed[0].id, files.id);
    assert!(store.get(html.id).unwrap().unwrap().favorite);
    assert_eq!(store.get(files.id).unwrap(), Some(files));
}

#[test]
fn retention_preserves_favorites_and_removes_old_or_excess_images() {
    let directory = TestDirectory::new("retention");
    let store = storage(
        &directory,
        PrivacyPolicy::default(),
        RetentionPolicy {
            max_items: 2,
            max_age: Some(Duration::from_millis(50)),
        },
    );
    let image = inserted(
        store
            .record(
                ClipboardInput::Image {
                    width: 1,
                    height: 1,
                    rgba8: vec![1, 2, 3, 255],
                    source_app: None,
                },
                0,
            )
            .unwrap(),
    );
    let image_path = image.image_path.expect("image path");
    let favorite = inserted(store.record(text("favorite", None), 0).unwrap());
    assert!(store.set_favorite(favorite.id, true).unwrap());
    let older_regular = inserted(store.record(text("older regular", None), 100).unwrap());
    let newest_regular = inserted(store.record(text("newest regular", None), 101).unwrap());

    let result = store.enforce_retention(100).unwrap();
    assert_eq!(result.deleted_items, 2);
    assert_eq!(result.deleted_images, 1);
    assert!(!image_path.exists());
    assert!(store.get(favorite.id).unwrap().unwrap().favorite);
    assert!(store.get(older_regular.id).unwrap().is_none());
    assert!(store.get(newest_regular.id).unwrap().is_some());
}

#[test]
fn storage_serializes_concurrent_deduplication() {
    let directory = TestDirectory::new("concurrent");
    let store = Arc::new(storage(
        &directory,
        PrivacyPolicy::default(),
        RetentionPolicy::default(),
    ));
    let workers = (0..8)
        .map(|index| {
            let store = Arc::clone(&store);
            thread::spawn(move || {
                store
                    .record(text("concurrent", Some("worker")), index)
                    .expect("record")
            })
        })
        .collect::<Vec<_>>();
    let outcomes = workers
        .into_iter()
        .map(|worker| worker.join().expect("join"))
        .collect::<Vec<_>>();

    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, RecordOutcome::Inserted(_)))
            .count(),
        1
    );
    assert_eq!(store.list(20).unwrap().len(), 1);
}

#[test]
fn list_pagination_uses_recent_order_without_loading_all_items() {
    let directory = TestDirectory::new("list-pagination");
    let store = storage(
        &directory,
        PrivacyPolicy::default(),
        RetentionPolicy::default(),
    );
    let first = inserted(store.record(text("first", None), 10).unwrap());
    let second = inserted(store.record(text("second", None), 20).unwrap());
    let third = inserted(store.record(text("third", None), 30).unwrap());

    assert_eq!(
        store
            .list_page(0, 2)
            .unwrap()
            .into_iter()
            .map(|item| item.id)
            .collect::<Vec<_>>(),
        vec![third.id, second.id]
    );
    assert_eq!(store.list_page(2, 2).unwrap()[0].id, first.id);
    assert_eq!(store.count(None).unwrap(), 3);
}

#[test]
fn search_pagination_and_count_share_the_same_filter() {
    let directory = TestDirectory::new("search-pagination");
    let store = storage(
        &directory,
        PrivacyPolicy::default(),
        RetentionPolicy::default(),
    );
    for (value, timestamp) in [("match one", 10), ("other", 20), ("match two", 30)] {
        store.record(text(value, None), timestamp).unwrap();
    }

    assert_eq!(store.count(Some("match")).unwrap(), 2);
    let first_page = store.search_page("match", 0, 1).unwrap();
    let second_page = store.search_page("match", 1, 1).unwrap();
    assert_eq!(first_page[0].text_content.as_deref(), Some("match two"));
    assert_eq!(second_page[0].text_content.as_deref(), Some("match one"));
}

#[test]
fn delete_removes_database_row_and_managed_image() {
    let directory = TestDirectory::new("delete-image");
    let store = storage(
        &directory,
        PrivacyPolicy::default(),
        RetentionPolicy::default(),
    );
    let item = inserted(
        store
            .record(
                ClipboardInput::Image {
                    width: 1,
                    height: 1,
                    rgba8: vec![1, 2, 3, 255],
                    source_app: None,
                },
                10,
            )
            .unwrap(),
    );
    let image_path = item.image_path.unwrap();

    assert!(store.delete(item.id).unwrap());
    assert!(!store.delete(item.id).unwrap());
    assert!(store.get(item.id).unwrap().is_none());
    assert!(!image_path.exists());
}

#[test]
fn image_preview_rejects_unsafe_managed_path() {
    let directory = TestDirectory::new("unsafe-preview");
    let store = storage(
        &directory,
        PrivacyPolicy::default(),
        RetentionPolicy::default(),
    );
    let item = inserted(
        store
            .record(
                ClipboardInput::Image {
                    width: 1,
                    height: 1,
                    rgba8: vec![1, 2, 3, 255],
                    source_app: None,
                },
                10,
            )
            .unwrap(),
    );
    let connection = Connection::open(store.database_path()).unwrap();
    connection
        .execute(
            "UPDATE clipboard_items SET image_file = '../../outside.png' WHERE id = ?1",
            [item.id],
        )
        .unwrap();

    assert!(store.image_preview(item.id, 100, 100).is_err());
}

#[test]
fn image_preview_resizes_with_aspect_ratio() {
    let directory = TestDirectory::new("resize-preview");
    let store = storage(
        &directory,
        PrivacyPolicy::default(),
        RetentionPolicy::default(),
    );
    let item = inserted(
        store
            .record(
                ClipboardInput::Image {
                    width: 400,
                    height: 200,
                    rgba8: vec![127; 400 * 200 * 4],
                    source_app: None,
                },
                10,
            )
            .unwrap(),
    );

    let preview = store.image_preview(item.id, 100, 100).unwrap().unwrap();
    assert_eq!((preview.width, preview.height), (100, 50));
    let decoded = image::load_from_memory(&preview.png).unwrap().into_rgba8();
    assert_eq!(decoded.dimensions(), (100, 50));
}

#[test]
fn non_image_preview_request_is_rejected() {
    let directory = TestDirectory::new("non-image-preview");
    let store = storage(
        &directory,
        PrivacyPolicy::default(),
        RetentionPolicy::default(),
    );
    let item = inserted(store.record(text("not an image", None), 10).unwrap());

    assert!(matches!(
        store.image_preview(item.id, 100, 100),
        Err(StorageError::InvalidData(_))
    ));
}
