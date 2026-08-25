use std::{
    error::Error,
    fmt, io,
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
    time::{SystemTime, UNIX_EPOCH},
};

use tauri::{AppHandle, Emitter, Manager};

use crate::core::clipboard::{
    ClipboardHistoryService, ClipboardInput, ClipboardStorage, PrivacyPolicy, RecordOutcome,
    RetentionPolicy, SqliteClipboardStorage, StorageError,
};

#[cfg(target_os = "macos")]
type PlatformClipboardWatcher = crate::platform::macos::clipboard::MacClipboardWatcher;
#[cfg(target_os = "windows")]
type PlatformClipboardWatcher = crate::platform::windows::clipboard::WindowsClipboardWatcher;

const CLIPBOARD_DIRECTORY: &str = "clipboard";
const RETENTION_WRITE_INTERVAL: usize = 50;
const HISTORY_CHANGED_EVENT: &str = "clipboard-history-changed";

pub(crate) type StorageHandle = Arc<dyn ClipboardStorage>;
type HistoryNotifier = Arc<dyn Fn() + Send + Sync>;
type RuntimeClock = Arc<dyn Fn() -> i64 + Send + Sync>;

#[derive(Debug)]
pub(crate) enum ClipboardBackendEvent {
    Input(ClipboardInput),
    Skipped,
    Error(String),
}

#[derive(Debug)]
pub(crate) enum ClipboardRuntimeError {
    AppDataDirectory(String),
    Storage(StorageError),
    Watcher(String),
    EventReceiverUnavailable,
    WorkerStart(io::Error),
    WorkerPanicked,
    StatePoisoned,
}

impl fmt::Display for ClipboardRuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AppDataDirectory(error) => {
                write!(
                    formatter,
                    "unable to resolve app local data directory: {error}"
                )
            }
            Self::Storage(error) => {
                write!(
                    formatter,
                    "clipboard storage initialization failed: {error}"
                )
            }
            Self::Watcher(error) => write!(formatter, "clipboard watcher failed: {error}"),
            Self::EventReceiverUnavailable => {
                formatter.write_str("clipboard watcher event receiver is unavailable")
            }
            Self::WorkerStart(error) => {
                write!(formatter, "clipboard worker failed to start: {error}")
            }
            Self::WorkerPanicked => {
                formatter.write_str("clipboard worker panicked during shutdown")
            }
            Self::StatePoisoned => formatter.write_str("clipboard runtime state lock was poisoned"),
        }
    }
}

impl Error for ClipboardRuntimeError {}

impl From<StorageError> for ClipboardRuntimeError {
    fn from(value: StorageError) -> Self {
        Self::Storage(value)
    }
}

pub(crate) struct ClipboardRuntime {
    watcher: Option<PlatformClipboardWatcher>,
    worker: Option<JoinHandle<()>>,
    storage: Option<StorageHandle>,
}

impl ClipboardRuntime {
    fn start(app: &AppHandle) -> Result<Self, ClipboardRuntimeError> {
        let data_directory = app
            .path()
            .app_local_data_dir()
            .map_err(|error| ClipboardRuntimeError::AppDataDirectory(error.to_string()))?
            .join(CLIPBOARD_DIRECTORY);
        let storage: StorageHandle = Arc::new(SqliteClipboardStorage::open(
            data_directory,
            PrivacyPolicy::default(),
            RetentionPolicy::default(),
        )?);
        storage.enforce_retention(current_time_ms())?;

        let mut watcher = PlatformClipboardWatcher::start()
            .map_err(|error| ClipboardRuntimeError::Watcher(error.to_string()))?;
        let events = watcher
            .take_events()
            .ok_or(ClipboardRuntimeError::EventReceiverUnavailable)?;
        let history_app = app.clone();
        let notifier: HistoryNotifier = Arc::new(move || {
            let _ = history_app.emit(HISTORY_CHANGED_EVENT, ());
        });
        let receive = move || events.recv().ok().map(normalize_backend_event);
        let worker = match spawn_ingestion_worker(
            Arc::clone(&storage),
            receive,
            notifier,
            Arc::new(current_time_ms),
            RETENTION_WRITE_INTERVAL,
        ) {
            Ok(worker) => worker,
            Err(error) => {
                let _ = watcher.stop();
                return Err(ClipboardRuntimeError::WorkerStart(error));
            }
        };

        Ok(Self {
            watcher: Some(watcher),
            worker: Some(worker),
            storage: Some(storage),
        })
    }

    fn shutdown(&mut self) -> Result<(), ClipboardRuntimeError> {
        let mut first_error = None;
        if let Some(watcher) = self.watcher.take() {
            if let Err(error) = watcher.stop() {
                first_error = Some(ClipboardRuntimeError::Watcher(error.to_string()));
            }
        }
        if let Some(worker) = self.worker.take() {
            if worker.join().is_err() && first_error.is_none() {
                first_error = Some(ClipboardRuntimeError::WorkerPanicked);
            }
        }
        self.storage.take();
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

impl Drop for ClipboardRuntime {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

#[derive(Default)]
pub(crate) struct ClipboardRuntimeState {
    runtime: Mutex<Option<ClipboardRuntime>>,
}

impl ClipboardRuntimeState {
    pub(crate) fn initialize(&self, app: &AppHandle) -> Result<(), ClipboardRuntimeError> {
        let runtime = ClipboardRuntime::start(app)?;
        let mut slot = self
            .runtime
            .lock()
            .map_err(|_| ClipboardRuntimeError::StatePoisoned)?;
        *slot = Some(runtime);
        Ok(())
    }

    pub(crate) fn shutdown(&self) -> Result<(), ClipboardRuntimeError> {
        let runtime = self
            .runtime
            .lock()
            .map_err(|_| ClipboardRuntimeError::StatePoisoned)?
            .take();
        match runtime {
            Some(mut runtime) => runtime.shutdown(),
            None => Ok(()),
        }
    }

    pub(crate) fn history_service(
        &self,
    ) -> Result<Option<ClipboardHistoryService>, ClipboardRuntimeError> {
        let runtime = self
            .runtime
            .lock()
            .map_err(|_| ClipboardRuntimeError::StatePoisoned)?;
        Ok(runtime
            .as_ref()
            .and_then(|runtime| runtime.storage.as_ref())
            .map(Arc::clone)
            .map(ClipboardHistoryService::new))
    }
}

fn normalize_backend_event<E>(event: Result<Option<ClipboardInput>, E>) -> ClipboardBackendEvent
where
    E: fmt::Display,
{
    match event {
        Ok(Some(input)) => ClipboardBackendEvent::Input(input),
        Ok(None) => ClipboardBackendEvent::Skipped,
        Err(error) => ClipboardBackendEvent::Error(error.to_string()),
    }
}

fn spawn_ingestion_worker<F>(
    storage: StorageHandle,
    mut receive: F,
    notifier: HistoryNotifier,
    clock: RuntimeClock,
    retention_write_interval: usize,
) -> Result<JoinHandle<()>, io::Error>
where
    F: FnMut() -> Option<ClipboardBackendEvent> + Send + 'static,
{
    thread::Builder::new()
        .name("litesnap-clipboard-ingestion".into())
        .spawn(move || {
            let mut successful_writes = 0usize;
            while let Some(event) = receive() {
                match event {
                    ClipboardBackendEvent::Input(input) => match storage.record(input, clock()) {
                        Ok(RecordOutcome::Inserted(_) | RecordOutcome::Duplicate(_)) => {
                            successful_writes = successful_writes.saturating_add(1);
                            notifier();
                            if retention_write_interval > 0
                                && successful_writes % retention_write_interval == 0
                            {
                                if let Err(error) = storage.enforce_retention(clock()) {
                                    eprintln!("Clipboard retention failed: {error}");
                                }
                            }
                        }
                        Ok(RecordOutcome::Ignored(_)) => {}
                        Err(error) => {
                            eprintln!("Clipboard storage write failed: {error}");
                        }
                    },
                    ClipboardBackendEvent::Skipped => {}
                    ClipboardBackendEvent::Error(error) => {
                        eprintln!("Clipboard backend event failed: {error}");
                    }
                }
            }
        })
}

fn current_time_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}
#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::{
            Arc,
            atomic::{AtomicI64, AtomicU64, AtomicUsize, Ordering},
            mpsc::{self, Sender},
        },
        thread::JoinHandle,
        time::{SystemTime, UNIX_EPOCH},
    };

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
                "litesnap-runtime-{name}-{}-{timestamp}-{nonce}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("create runtime test directory");
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

    fn test_storage(directory: &TestDirectory) -> Arc<SqliteClipboardStorage> {
        Arc::new(
            SqliteClipboardStorage::open(
                directory.path(),
                PrivacyPolicy::default(),
                RetentionPolicy::default(),
            )
            .expect("open runtime test storage"),
        )
    }

    fn text(value: &str) -> ClipboardInput {
        ClipboardInput::Text {
            text: value.into(),
            source_app: Some("runtime-test".into()),
        }
    }

    fn spawn_test_worker(
        storage: Arc<SqliteClipboardStorage>,
    ) -> (
        Sender<ClipboardBackendEvent>,
        JoinHandle<()>,
        Arc<AtomicUsize>,
    ) {
        let (sender, receiver) = mpsc::channel();
        let notifications = Arc::new(AtomicUsize::new(0));
        let notifier_count = Arc::clone(&notifications);
        let notifier: HistoryNotifier = Arc::new(move || {
            notifier_count.fetch_add(1, Ordering::SeqCst);
        });
        let ticks = Arc::new(AtomicI64::new(0));
        let clock_ticks = Arc::clone(&ticks);
        let clock: RuntimeClock =
            Arc::new(move || clock_ticks.fetch_add(10, Ordering::SeqCst) + 10);
        let shared_storage: StorageHandle = storage;
        let worker = spawn_ingestion_worker(
            shared_storage,
            move || receiver.recv().ok(),
            notifier,
            clock,
            RETENTION_WRITE_INTERVAL,
        )
        .expect("spawn runtime test worker");
        (sender, worker, notifications)
    }

    fn stop_test_worker(sender: Sender<ClipboardBackendEvent>, worker: JoinHandle<()>) {
        drop(sender);
        worker.join().expect("join runtime test worker");
    }

    #[test]
    fn runtime_input_is_stored() {
        let directory = TestDirectory::new("input");
        let storage = test_storage(&directory);
        let (sender, worker, notifications) = spawn_test_worker(Arc::clone(&storage));

        sender
            .send(ClipboardBackendEvent::Input(text("A")))
            .expect("send input");
        stop_test_worker(sender, worker);

        let items = storage.list(10).expect("list stored input");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].text_content.as_deref(), Some("A"));
        assert_eq!(notifications.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn runtime_deduplicates_and_moves_latest_item_first() {
        let directory = TestDirectory::new("dedup");
        let storage = test_storage(&directory);
        let (sender, worker, notifications) = spawn_test_worker(Arc::clone(&storage));

        for value in ["A", "B", "A"] {
            sender
                .send(ClipboardBackendEvent::Input(text(value)))
                .expect("send input");
        }
        stop_test_worker(sender, worker);

        let items = storage.list(10).expect("list deduplicated inputs");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].text_content.as_deref(), Some("A"));
        assert_eq!(items[1].text_content.as_deref(), Some("B"));
        assert_eq!(notifications.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn runtime_image_is_stored_as_png() {
        let directory = TestDirectory::new("image");
        let storage = test_storage(&directory);
        let (sender, worker, _) = spawn_test_worker(Arc::clone(&storage));

        sender
            .send(ClipboardBackendEvent::Input(ClipboardInput::Image {
                width: 1,
                height: 1,
                rgba8: vec![10, 20, 30, 255],
                source_app: None,
            }))
            .expect("send image input");
        stop_test_worker(sender, worker);

        let items = storage.list(10).expect("list image input");
        let image_path = items[0].image_path.as_ref().expect("image path");
        assert!(image_path.exists());
        assert_eq!(
            image_path.extension().and_then(|value| value.to_str()),
            Some("png")
        );
    }

    #[test]
    fn runtime_backend_error_does_not_stop_next_input() {
        let directory = TestDirectory::new("error-recovery");
        let storage = test_storage(&directory);
        let (sender, worker, notifications) = spawn_test_worker(Arc::clone(&storage));

        sender
            .send(ClipboardBackendEvent::Error(
                "synthetic backend error".into(),
            ))
            .expect("send backend error");
        sender
            .send(ClipboardBackendEvent::Input(text("B")))
            .expect("send recovery input");
        stop_test_worker(sender, worker);

        let items = storage.list(10).expect("list recovery input");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].text_content.as_deref(), Some("B"));
        assert_eq!(notifications.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn runtime_skipped_event_is_not_stored() {
        let directory = TestDirectory::new("skipped");
        let storage = test_storage(&directory);
        let (sender, worker, notifications) = spawn_test_worker(Arc::clone(&storage));

        sender
            .send(ClipboardBackendEvent::Skipped)
            .expect("send skipped event");
        stop_test_worker(sender, worker);

        assert!(storage.list(10).expect("list skipped input").is_empty());
        assert_eq!(notifications.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn runtime_shutdown_joins_worker_and_releases_storage() {
        let directory = TestDirectory::new("shutdown");
        let storage = test_storage(&directory);
        let (sender, receiver) = mpsc::channel();
        let worker_storage: StorageHandle = storage.clone();
        let worker = spawn_ingestion_worker(
            worker_storage,
            move || receiver.recv().ok(),
            Arc::new(|| {}),
            Arc::new(|| 1),
            RETENTION_WRITE_INTERVAL,
        )
        .expect("spawn shutdown worker");
        let runtime_storage: StorageHandle = storage.clone();
        let mut runtime = ClipboardRuntime {
            watcher: None,
            worker: Some(worker),
            storage: Some(runtime_storage),
        };

        drop(sender);
        runtime.shutdown().expect("shutdown runtime");

        assert!(runtime.worker.is_none());
        assert!(runtime.storage.is_none());
        assert_eq!(Arc::strong_count(&storage), 1);
    }

    #[test]
    fn runtime_unavailable_has_no_history_service() {
        let state = ClipboardRuntimeState::default();
        assert!(state.history_service().unwrap().is_none());
    }
}
