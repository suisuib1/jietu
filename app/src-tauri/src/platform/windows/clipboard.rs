use std::{
    error::Error,
    fmt,
    ptr::{null, null_mut},
    slice,
    sync::{
        OnceLock,
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use windows_sys::Win32::{
    Foundation::{
        CloseHandle, ERROR_CLASS_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, WPARAM,
    },
    System::{
        DataExchange::{
            AddClipboardFormatListener, CloseClipboard, GetClipboardData, GetClipboardOwner,
            GetClipboardSequenceNumber, IsClipboardFormatAvailable, OpenClipboard,
            RegisterClipboardFormatW, RemoveClipboardFormatListener,
        },
        LibraryLoader::GetModuleHandleW,
        Memory::{GlobalLock, GlobalSize, GlobalUnlock},
        Ole::{CF_DIB, CF_DIBV5, CF_HDROP, CF_UNICODETEXT},
        Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW},
    },
    UI::{
        Shell::DragQueryFileW,
        WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetForegroundWindow,
            GetMessageW, GetWindowThreadProcessId, HWND_MESSAGE, MSG, PostMessageW, RegisterClassW,
            TranslateMessage, WM_CLIPBOARDUPDATE, WM_CLOSE, WNDCLASSW,
        },
    },
};

use crate::core::clipboard::ClipboardInput;

const WINDOW_CLASS_NAME: [u16; 25] = [
    76, 105, 116, 101, 83, 110, 97, 112, 67, 108, 105, 112, 98, 111, 97, 114, 100, 87, 97, 116, 99,
    104, 101, 114, 0,
];
const CLIPBOARD_RETRY_DELAYS_MS: [u64; 5] = [10, 20, 40, 80, 100];
const MAX_PROCESS_PATH_U16: usize = 32_768;

pub(crate) type ClipboardBackendEvent = Result<Option<ClipboardInput>, ClipboardBackendError>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ClipboardBackendError {
    ListenerRegistration { operation: &'static str, code: u32 },
    ClipboardBusy { attempts: usize, code: u32 },
    ClipboardRead { format: &'static str, code: u32 },
    InvalidUtf16,
    InvalidHtml,
    InvalidImage,
    Win32 { operation: &'static str, code: u32 },
    WatcherShutdown { operation: &'static str, code: u32 },
}

impl fmt::Display for ClipboardBackendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ListenerRegistration { operation, code } => {
                write!(
                    formatter,
                    "clipboard listener registration failed in {operation}: {code}"
                )
            }
            Self::ClipboardBusy { attempts, code } => write!(
                formatter,
                "clipboard remained busy after {attempts} attempts: {code}"
            ),
            Self::ClipboardRead { format, code } => {
                write!(
                    formatter,
                    "failed to copy clipboard format {format}: {code}"
                )
            }
            Self::InvalidUtf16 => formatter.write_str("clipboard text contains invalid UTF-16"),
            Self::InvalidHtml => formatter.write_str("clipboard contains invalid CF_HTML"),
            Self::InvalidImage => formatter.write_str("clipboard image is invalid"),
            Self::Win32 { operation, code } => {
                write!(
                    formatter,
                    "Win32 clipboard operation {operation} failed: {code}"
                )
            }
            Self::WatcherShutdown { operation, code } => {
                write!(
                    formatter,
                    "clipboard watcher shutdown failed in {operation}: {code}"
                )
            }
        }
    }
}

impl Error for ClipboardBackendError {}

pub(crate) struct WindowsClipboardWatcher {
    message_window: isize,
    events: Option<Receiver<ClipboardBackendEvent>>,
    thread: Option<JoinHandle<Result<(), ClipboardBackendError>>>,
}

impl WindowsClipboardWatcher {
    pub(crate) fn start() -> Result<Self, ClipboardBackendError> {
        let (ready_sender, ready_receiver) = mpsc::channel();
        let (event_sender, events) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("litesnap-clipboard-listener".into())
            .spawn(move || listener_thread(ready_sender, event_sender))
            .map_err(|_| ClipboardBackendError::ListenerRegistration {
                operation: "spawn listener thread",
                code: 0,
            })?;

        match ready_receiver.recv() {
            Ok(Ok(message_window)) => Ok(Self {
                message_window,
                events: Some(events),
                thread: Some(thread),
            }),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(_) => match thread.join() {
                Ok(Err(error)) => Err(error),
                _ => Err(ClipboardBackendError::ListenerRegistration {
                    operation: "listener startup handshake",
                    code: 0,
                }),
            },
        }
    }

    pub(crate) fn take_events(&mut self) -> Option<Receiver<ClipboardBackendEvent>> {
        self.events.take()
    }

    pub(crate) fn message_window_handle(&self) -> isize {
        self.message_window
    }

    pub(crate) fn stop(mut self) -> Result<(), ClipboardBackendError> {
        self.shutdown()
    }

    fn shutdown(&mut self) -> Result<(), ClipboardBackendError> {
        let Some(thread) = self.thread.take() else {
            return Ok(());
        };
        let post_error =
            if unsafe { PostMessageW(self.message_window as HWND, WM_CLOSE, 0, 0) } == 0 {
                Some(last_error())
            } else {
                None
            };

        match thread.join() {
            Ok(Err(error)) => Err(error),
            Err(_) => Err(ClipboardBackendError::WatcherShutdown {
                operation: "join listener thread",
                code: 0,
            }),
            Ok(Ok(())) => match post_error {
                Some(code) => Err(ClipboardBackendError::WatcherShutdown {
                    operation: "PostMessageW(WM_CLOSE)",
                    code,
                }),
                None => Ok(()),
            },
        }
    }
}

impl Drop for WindowsClipboardWatcher {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

fn listener_thread(
    ready_sender: Sender<Result<isize, ClipboardBackendError>>,
    event_sender: Sender<ClipboardBackendEvent>,
) -> Result<(), ClipboardBackendError> {
    let window = match ListenerWindow::create() {
        Ok(window) => window,
        Err(error) => {
            let _ = ready_sender.send(Err(error.clone()));
            return Err(error);
        }
    };
    ready_sender.send(Ok(window.hwnd as isize)).map_err(|_| {
        ClipboardBackendError::WatcherShutdown {
            operation: "listener startup receiver",
            code: 0,
        }
    })?;

    let mut sequence = SequenceTracker::default();
    let mut message = MSG::default();
    loop {
        let result = unsafe { GetMessageW(&mut message, null_mut(), 0, 0) };
        if result == -1 {
            return Err(ClipboardBackendError::Win32 {
                operation: "GetMessageW",
                code: last_error(),
            });
        }
        if result == 0 || (message.hwnd == window.hwnd && message.message == WM_CLOSE) {
            break;
        }
        if message.hwnd == window.hwnd && message.message == WM_CLIPBOARDUPDATE {
            let current = unsafe { GetClipboardSequenceNumber() };
            if sequence.should_process(current) {
                let _ = event_sender.send(capture_clipboard_input(current));
            }
            continue;
        }
        unsafe {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    window.close()
}

struct ListenerWindow {
    hwnd: HWND,
    listener_registered: bool,
}

impl ListenerWindow {
    fn create() -> Result<Self, ClipboardBackendError> {
        let instance = window_class_instance()? as _;
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                WINDOW_CLASS_NAME.as_ptr(),
                WINDOW_CLASS_NAME.as_ptr(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                null_mut(),
                instance,
                null(),
            )
        };
        if hwnd.is_null() {
            return Err(ClipboardBackendError::ListenerRegistration {
                operation: "CreateWindowExW",
                code: last_error(),
            });
        }
        let mut window = Self {
            hwnd,
            listener_registered: false,
        };
        if unsafe { AddClipboardFormatListener(hwnd) } == 0 {
            return Err(ClipboardBackendError::ListenerRegistration {
                operation: "AddClipboardFormatListener",
                code: last_error(),
            });
        }
        window.listener_registered = true;
        Ok(window)
    }

    fn close(mut self) -> Result<(), ClipboardBackendError> {
        let mut first_error = None;
        if self.listener_registered {
            if unsafe { RemoveClipboardFormatListener(self.hwnd) } == 0 {
                first_error = Some(ClipboardBackendError::WatcherShutdown {
                    operation: "RemoveClipboardFormatListener",
                    code: last_error(),
                });
            } else {
                self.listener_registered = false;
            }
        }
        if unsafe { DestroyWindow(self.hwnd) } == 0 {
            first_error.get_or_insert_with(|| ClipboardBackendError::WatcherShutdown {
                operation: "DestroyWindow",
                code: last_error(),
            });
        } else {
            self.hwnd = null_mut();
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

impl Drop for ListenerWindow {
    fn drop(&mut self) {
        if self.hwnd.is_null() {
            return;
        }
        unsafe {
            if self.listener_registered {
                RemoveClipboardFormatListener(self.hwnd);
            }
            DestroyWindow(self.hwnd);
        }
    }
}

fn window_class_instance() -> Result<isize, ClipboardBackendError> {
    static REGISTRATION: OnceLock<Result<isize, u32>> = OnceLock::new();
    let registration = REGISTRATION.get_or_init(|| {
        let instance = unsafe { GetModuleHandleW(null()) };
        if instance.is_null() {
            return Err(last_error());
        }
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: WINDOW_CLASS_NAME.as_ptr(),
            ..WNDCLASSW::default()
        };
        let atom = unsafe { RegisterClassW(&class) };
        if atom == 0 {
            let code = last_error();
            if code != ERROR_CLASS_ALREADY_EXISTS {
                return Err(code);
            }
        }
        Ok(instance as isize)
    });
    match *registration {
        Ok(instance) => Ok(instance),
        Err(code) => Err(ClipboardBackendError::ListenerRegistration {
            operation: "RegisterClassW",
            code,
        }),
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

#[derive(Default)]
struct SequenceTracker {
    last_sequence_number: Option<u32>,
}

impl SequenceTracker {
    fn should_process(&mut self, sequence_number: u32) -> bool {
        if self.last_sequence_number == Some(sequence_number) {
            return false;
        }
        self.last_sequence_number = Some(sequence_number);
        true
    }
}

fn last_error() -> u32 {
    unsafe { GetLastError() }
}
#[derive(Clone, Copy)]
struct RegisteredFormats {
    html: u32,
    png: u32,
    exclude_from_monitor: u32,
    can_include_history: u32,
}

fn registered_formats() -> Result<RegisteredFormats, ClipboardBackendError> {
    static FORMATS: OnceLock<Result<RegisteredFormats, u32>> = OnceLock::new();
    match *FORMATS.get_or_init(|| {
        Ok(RegisteredFormats {
            html: register_clipboard_format("HTML Format")?,
            png: register_clipboard_format("PNG")?,
            exclude_from_monitor: register_clipboard_format(
                "ExcludeClipboardContentFromMonitorProcessing",
            )?,
            can_include_history: register_clipboard_format("CanIncludeInClipboardHistory")?,
        })
    }) {
        Ok(formats) => Ok(formats),
        Err(code) => Err(ClipboardBackendError::Win32 {
            operation: "RegisterClipboardFormatW",
            code,
        }),
    }
}

fn register_clipboard_format(name: &str) -> Result<u32, u32> {
    let name = wide_null(name);
    let format = unsafe { RegisterClipboardFormatW(name.as_ptr()) };
    if format == 0 {
        Err(last_error())
    } else {
        Ok(format)
    }
}

fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain([0]).collect()
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ClipboardPrivacyFlags {
    exclude_from_monitor_processing: bool,
    can_include_in_history: Option<bool>,
}

impl ClipboardPrivacyFlags {
    fn should_skip(self) -> bool {
        self.exclude_from_monitor_processing || self.can_include_in_history == Some(false)
    }
}

type RawFormat<T> = Option<Result<T, ClipboardBackendError>>;

struct RawClipboardSnapshot {
    sequence_number: u32,
    privacy_flags: ClipboardPrivacyFlags,
    source_window: isize,
    text: RawFormat<Vec<u8>>,
    html: RawFormat<Vec<u8>>,
    high_confidence_image: RawFormat<Vec<u8>>,
    fallback_image: RawFormat<Vec<u8>>,
    files: RawFormat<Vec<Vec<u16>>>,
}

fn capture_clipboard_input(
    sequence_number: u32,
) -> Result<Option<ClipboardInput>, ClipboardBackendError> {
    let formats = registered_formats()?;
    let source_window = source_window();
    let clipboard = ClipboardOpenGuard::open_with_retry()?;
    let privacy_flags = read_privacy_flags(formats)?;
    if privacy_flags.should_skip() {
        drop(clipboard);
        return Ok(None);
    }

    let raw = RawClipboardSnapshot {
        sequence_number,
        privacy_flags,
        source_window: source_window as isize,
        text: read_optional_bytes(CF_UNICODETEXT as u32, "CF_UNICODETEXT"),
        html: read_optional_bytes(formats.html, "HTML Format"),
        high_confidence_image: read_optional_bytes(formats.png, "PNG"),
        fallback_image: if unsafe { IsClipboardFormatAvailable(CF_DIBV5 as u32) } != 0 {
            Some(read_clipboard_bytes(CF_DIBV5 as u32, "CF_DIBV5"))
        } else {
            read_optional_bytes(CF_DIB as u32, "CF_DIB")
        },
        files: read_optional_files(),
    };
    drop(clipboard);

    let source_application = source_application(raw.source_window as HWND);
    select_clipboard_input(WindowsClipboardSnapshot::from_raw(raw, source_application))
}

fn source_window() -> HWND {
    let owner = unsafe { GetClipboardOwner() };
    if owner.is_null() {
        unsafe { GetForegroundWindow() }
    } else {
        owner
    }
}

struct ClipboardOpenGuard;

impl ClipboardOpenGuard {
    fn open_with_retry() -> Result<Self, ClipboardBackendError> {
        let mut code = 0;
        for attempt in 0..=CLIPBOARD_RETRY_DELAYS_MS.len() {
            if unsafe { OpenClipboard(null_mut()) } != 0 {
                return Ok(Self);
            }
            code = last_error();
            if let Some(delay) = CLIPBOARD_RETRY_DELAYS_MS.get(attempt) {
                thread::sleep(Duration::from_millis(*delay));
            }
        }
        Err(ClipboardBackendError::ClipboardBusy {
            attempts: CLIPBOARD_RETRY_DELAYS_MS.len() + 1,
            code,
        })
    }
}

impl Drop for ClipboardOpenGuard {
    fn drop(&mut self) {
        unsafe {
            CloseClipboard();
        }
    }
}

fn read_privacy_flags(
    formats: RegisteredFormats,
) -> Result<ClipboardPrivacyFlags, ClipboardBackendError> {
    let exclude_from_monitor_processing =
        unsafe { IsClipboardFormatAvailable(formats.exclude_from_monitor) } != 0;
    let can_include_in_history =
        if unsafe { IsClipboardFormatAvailable(formats.can_include_history) } != 0 {
            let bytes =
                read_clipboard_bytes(formats.can_include_history, "CanIncludeInClipboardHistory")?;
            let value = bytes
                .get(..4)
                .and_then(|bytes| <[u8; 4]>::try_from(bytes).ok())
                .map(u32::from_le_bytes)
                .ok_or(ClipboardBackendError::ClipboardRead {
                    format: "CanIncludeInClipboardHistory",
                    code: 0,
                })?;
            Some(value != 0)
        } else {
            None
        };
    Ok(ClipboardPrivacyFlags {
        exclude_from_monitor_processing,
        can_include_in_history,
    })
}

fn read_optional_bytes(format: u32, name: &'static str) -> RawFormat<Vec<u8>> {
    if unsafe { IsClipboardFormatAvailable(format) } == 0 {
        None
    } else {
        Some(read_clipboard_bytes(format, name))
    }
}

fn read_clipboard_bytes(format: u32, name: &'static str) -> Result<Vec<u8>, ClipboardBackendError> {
    let handle = unsafe { GetClipboardData(format) };
    if handle.is_null() {
        return Err(ClipboardBackendError::ClipboardRead {
            format: name,
            code: last_error(),
        });
    }
    let size = unsafe { GlobalSize(handle) };
    if size == 0 {
        return Ok(Vec::new());
    }
    let pointer = unsafe { GlobalLock(handle) };
    if pointer.is_null() {
        return Err(ClipboardBackendError::ClipboardRead {
            format: name,
            code: last_error(),
        });
    }
    let lock = GlobalMemoryLock { handle, pointer };
    let bytes = unsafe { slice::from_raw_parts(lock.pointer.cast::<u8>(), size) }.to_vec();
    Ok(bytes)
}

struct GlobalMemoryLock {
    handle: *mut core::ffi::c_void,
    pointer: *mut core::ffi::c_void,
}

impl Drop for GlobalMemoryLock {
    fn drop(&mut self) {
        unsafe {
            GlobalUnlock(self.handle);
        }
    }
}

fn read_optional_files() -> RawFormat<Vec<Vec<u16>>> {
    if unsafe { IsClipboardFormatAvailable(CF_HDROP as u32) } == 0 {
        None
    } else {
        Some(read_clipboard_files())
    }
}

fn read_clipboard_files() -> Result<Vec<Vec<u16>>, ClipboardBackendError> {
    let handle = unsafe { GetClipboardData(CF_HDROP as u32) };
    if handle.is_null() {
        return Err(ClipboardBackendError::ClipboardRead {
            format: "CF_HDROP",
            code: last_error(),
        });
    }
    let count = unsafe { DragQueryFileW(handle, u32::MAX, null_mut(), 0) };
    let mut files = Vec::with_capacity(count as usize);
    for index in 0..count {
        let length = unsafe { DragQueryFileW(handle, index, null_mut(), 0) };
        let mut buffer = vec![0u16; length as usize + 1];
        let copied =
            unsafe { DragQueryFileW(handle, index, buffer.as_mut_ptr(), buffer.len() as u32) };
        if copied == 0 && length != 0 {
            return Err(ClipboardBackendError::ClipboardRead {
                format: "CF_HDROP",
                code: last_error(),
            });
        }
        files.push(buffer[..copied as usize].to_vec());
    }
    Ok(files)
}

fn decode_file_paths(paths: Vec<Vec<u16>>) -> Result<Vec<String>, ClipboardBackendError> {
    paths
        .into_iter()
        .map(|path| String::from_utf16(&path).map_err(|_| ClipboardBackendError::InvalidUtf16))
        .collect()
}
fn source_application(window: HWND) -> Option<String> {
    executable_name_for_window(window).or_else(|| {
        let foreground = unsafe { GetForegroundWindow() };
        if foreground == window {
            None
        } else {
            executable_name_for_window(foreground)
        }
    })
}

fn executable_name_for_window(window: HWND) -> Option<String> {
    if window.is_null() {
        return None;
    }
    let mut process_id = 0;
    if unsafe { GetWindowThreadProcessId(window, &mut process_id) } == 0 || process_id == 0 {
        return None;
    }
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id) };
    if process.is_null() {
        return None;
    }
    let process = ProcessHandle(process);
    let mut path = vec![0u16; MAX_PROCESS_PATH_U16];
    let mut length = path.len() as u32;
    if unsafe { QueryFullProcessImageNameW(process.0, 0, path.as_mut_ptr(), &mut length) } == 0 {
        return None;
    }
    let path = String::from_utf16(&path[..length as usize]).ok()?;
    executable_name_from_path(&path)
}

fn executable_name_from_path(path: &str) -> Option<String> {
    path.rsplit(['\\', '/'])
        .find(|component| !component.is_empty())
        .map(str::to_owned)
}
struct ProcessHandle(*mut core::ffi::c_void);

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
#[derive(Debug, Default)]
struct WindowsClipboardSnapshot {
    sequence_number: u32,
    privacy_flags: ClipboardPrivacyFlags,
    source_application: Option<String>,
    text: RawFormat<String>,
    html: RawFormat<String>,
    high_confidence_image: RawFormat<CanonicalImage>,
    fallback_image: RawFormat<CanonicalImage>,
    files: RawFormat<Vec<String>>,
}

impl WindowsClipboardSnapshot {
    fn from_raw(raw: RawClipboardSnapshot, source_application: Option<String>) -> Self {
        Self {
            sequence_number: raw.sequence_number,
            privacy_flags: raw.privacy_flags,
            source_application,
            text: raw
                .text
                .map(|result| result.and_then(|bytes| decode_unicode_text(&bytes))),
            html: raw
                .html
                .map(|result| result.and_then(|bytes| parse_cf_html(&bytes))),
            high_confidence_image: raw
                .high_confidence_image
                .map(|result| result.and_then(|bytes| decode_png(&bytes))),
            fallback_image: raw
                .fallback_image
                .map(|result| result.and_then(|bytes| decode_dib(&bytes))),
            files: raw.files.map(|result| result.and_then(decode_file_paths)),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CanonicalImage {
    width: u32,
    height: u32,
    rgba8: Vec<u8>,
}

fn select_clipboard_input(
    snapshot: WindowsClipboardSnapshot,
) -> Result<Option<ClipboardInput>, ClipboardBackendError> {
    if snapshot.privacy_flags.should_skip() {
        return Ok(None);
    }
    let WindowsClipboardSnapshot {
        sequence_number: _,
        privacy_flags: _,
        source_application,
        text,
        html,
        high_confidence_image,
        fallback_image,
        files,
    } = snapshot;
    let mut first_error = None;

    if let Some(result) = files {
        match result {
            Ok(files) if !files.is_empty() => {
                return Ok(Some(ClipboardInput::Files {
                    files,
                    source_app: source_application,
                }));
            }
            Ok(_) => {}
            Err(error) => remember_first_error(&mut first_error, error),
        }
    }

    if let Some(result) = high_confidence_image {
        match result {
            Ok(image) => {
                return Ok(Some(image_input(image, source_application)));
            }
            Err(error) => remember_first_error(&mut first_error, error),
        }
    }

    if let Some(result) = html {
        match result {
            Ok(html) if !html.is_empty() => {
                let plain_text = text
                    .as_ref()
                    .and_then(|result| result.as_ref().ok())
                    .filter(|text| !text.is_empty())
                    .cloned();
                return Ok(Some(ClipboardInput::Html {
                    html,
                    text: plain_text,
                    source_app: source_application,
                }));
            }
            Ok(_) => {}
            Err(error) => remember_first_error(&mut first_error, error),
        }
    }

    if let Some(result) = fallback_image {
        match result {
            Ok(image) => {
                return Ok(Some(image_input(image, source_application)));
            }
            Err(error) => remember_first_error(&mut first_error, error),
        }
    }

    if let Some(result) = text {
        match result {
            Ok(text) if !text.is_empty() => {
                return Ok(Some(ClipboardInput::Text {
                    text,
                    source_app: source_application,
                }));
            }
            Ok(_) => {}
            Err(error) => remember_first_error(&mut first_error, error),
        }
    }

    match first_error {
        Some(error) => Err(error),
        None => Ok(None),
    }
}

fn remember_first_error(
    first_error: &mut Option<ClipboardBackendError>,
    error: ClipboardBackendError,
) {
    if first_error.is_none() {
        *first_error = Some(error);
    }
}

fn image_input(image: CanonicalImage, source_application: Option<String>) -> ClipboardInput {
    ClipboardInput::Image {
        width: image.width,
        height: image.height,
        rgba8: image.rgba8,
        source_app: source_application,
    }
}

fn decode_unicode_text(bytes: &[u8]) -> Result<String, ClipboardBackendError> {
    if !bytes.len().is_multiple_of(2) {
        return Err(ClipboardBackendError::InvalidUtf16);
    }
    let units = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .take_while(|unit| *unit != 0)
        .collect::<Vec<_>>();
    String::from_utf16(&units).map_err(|_| ClipboardBackendError::InvalidUtf16)
}

fn parse_cf_html(bytes: &[u8]) -> Result<String, ClipboardBackendError> {
    let fragment = cf_html_range(bytes, "StartFragment:", "EndFragment:");
    let html = cf_html_range(bytes, "StartHTML:", "EndHTML:");
    for range in [fragment, html].into_iter().flatten() {
        if range.start >= range.end || range.end > bytes.len() {
            continue;
        }
        let mut content = &bytes[range];
        while content.last() == Some(&0) {
            content = &content[..content.len() - 1];
        }
        return std::str::from_utf8(content)
            .map(str::to_owned)
            .map_err(|_| ClipboardBackendError::InvalidHtml);
    }
    Err(ClipboardBackendError::InvalidHtml)
}

fn cf_html_range(bytes: &[u8], start_key: &str, end_key: &str) -> Option<std::ops::Range<usize>> {
    let start = cf_html_offset(bytes, start_key)?;
    let end = cf_html_offset(bytes, end_key)?;
    Some(start..end)
}

fn cf_html_offset(bytes: &[u8], key: &str) -> Option<usize> {
    let header_length = bytes.len().min(4_096);
    let header = String::from_utf8_lossy(&bytes[..header_length]);
    header.lines().find_map(|line| {
        let value = line.strip_prefix(key)?.trim();
        if value == "-1" {
            None
        } else {
            value.parse().ok()
        }
    })
}

fn decode_png(bytes: &[u8]) -> Result<CanonicalImage, ClipboardBackendError> {
    let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
        .map_err(|_| ClipboardBackendError::InvalidImage)?
        .into_rgba8();
    let (width, height) = image.dimensions();
    validate_canonical_image(width, height, image.into_raw())
}

fn decode_dib(bytes: &[u8]) -> Result<CanonicalImage, ClipboardBackendError> {
    let header_size = read_u32_le(bytes, 0)? as usize;
    if header_size < 40 || header_size > bytes.len() {
        return Err(ClipboardBackendError::InvalidImage);
    }
    let bits_per_pixel = read_u16_le(bytes, 14)?;
    let compression = read_u32_le(bytes, 16)?;
    let colors_used = read_u32_le(bytes, 32)? as usize;
    let mask_bytes = if header_size == 40 {
        match compression {
            3 => 12,
            6 => 16,
            _ => 0,
        }
    } else {
        0
    };
    let palette_entries = if colors_used != 0 {
        colors_used
    } else if bits_per_pixel <= 8 {
        1usize
            .checked_shl(u32::from(bits_per_pixel))
            .ok_or(ClipboardBackendError::InvalidImage)?
    } else {
        0
    };
    let pixel_offset = header_size
        .checked_add(mask_bytes)
        .and_then(|offset| offset.checked_add(palette_entries.checked_mul(4)?))
        .ok_or(ClipboardBackendError::InvalidImage)?;
    if pixel_offset > bytes.len() {
        return Err(ClipboardBackendError::InvalidImage);
    }

    let file_size = u32::try_from(
        14usize
            .checked_add(bytes.len())
            .ok_or(ClipboardBackendError::InvalidImage)?,
    )
    .map_err(|_| ClipboardBackendError::InvalidImage)?;
    let file_pixel_offset = u32::try_from(
        14usize
            .checked_add(pixel_offset)
            .ok_or(ClipboardBackendError::InvalidImage)?,
    )
    .map_err(|_| ClipboardBackendError::InvalidImage)?;
    let mut bmp = Vec::with_capacity(file_size as usize);
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&file_size.to_le_bytes());
    bmp.extend_from_slice(&[0; 4]);
    bmp.extend_from_slice(&file_pixel_offset.to_le_bytes());
    bmp.extend_from_slice(bytes);

    let image = image::load_from_memory_with_format(&bmp, image::ImageFormat::Bmp)
        .map_err(|_| ClipboardBackendError::InvalidImage)?
        .into_rgba8();
    let (width, height) = image.dimensions();
    validate_canonical_image(width, height, image.into_raw())
}

fn validate_canonical_image(
    width: u32,
    height: u32,
    rgba8: Vec<u8>,
) -> Result<CanonicalImage, ClipboardBackendError> {
    let expected = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(4))
        .and_then(|bytes| usize::try_from(bytes).ok())
        .ok_or(ClipboardBackendError::InvalidImage)?;
    if width == 0 || height == 0 || rgba8.len() != expected {
        return Err(ClipboardBackendError::InvalidImage);
    }
    Ok(CanonicalImage {
        width,
        height,
        rgba8,
    })
}

fn read_u16_le(bytes: &[u8], offset: usize) -> Result<u16, ClipboardBackendError> {
    bytes
        .get(offset..offset + 2)
        .and_then(|bytes| <[u8; 2]>::try_from(bytes).ok())
        .map(u16::from_le_bytes)
        .ok_or(ClipboardBackendError::InvalidImage)
}

fn read_u32_le(bytes: &[u8], offset: usize) -> Result<u32, ClipboardBackendError> {
    bytes
        .get(offset..offset + 4)
        .and_then(|bytes| <[u8; 4]>::try_from(bytes).ok())
        .map(u32::from_le_bytes)
        .ok_or(ClipboardBackendError::InvalidImage)
}
#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::IsWindow;

    fn snapshot() -> WindowsClipboardSnapshot {
        WindowsClipboardSnapshot {
            sequence_number: 42,
            source_application: Some("source.exe".into()),
            ..WindowsClipboardSnapshot::default()
        }
    }

    fn image() -> CanonicalImage {
        CanonicalImage {
            width: 1,
            height: 1,
            rgba8: vec![1, 2, 3, 255],
        }
    }

    #[test]
    fn privacy_exclude_marker_skips_content() {
        let result = select_clipboard_input(WindowsClipboardSnapshot {
            privacy_flags: ClipboardPrivacyFlags {
                exclude_from_monitor_processing: true,
                can_include_in_history: Some(true),
            },
            text: Some(Ok("secret".into())),
            ..snapshot()
        })
        .unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn privacy_include_history_zero_skips_content() {
        let result = select_clipboard_input(WindowsClipboardSnapshot {
            privacy_flags: ClipboardPrivacyFlags {
                exclude_from_monitor_processing: false,
                can_include_in_history: Some(false),
            },
            text: Some(Ok("secret".into())),
            ..snapshot()
        })
        .unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn privacy_include_history_one_allows_content() {
        let result = select_clipboard_input(WindowsClipboardSnapshot {
            privacy_flags: ClipboardPrivacyFlags {
                exclude_from_monitor_processing: false,
                can_include_in_history: Some(true),
            },
            text: Some(Ok("allowed".into())),
            ..snapshot()
        })
        .unwrap();
        assert!(matches!(
            result,
            Some(ClipboardInput::Text { text, .. }) if text == "allowed"
        ));
    }

    #[test]
    fn files_take_priority_over_text_and_keep_order() {
        let files = decode_file_paths(vec![
            "C:\\two.txt".encode_utf16().collect(),
            "C:\\one.txt".encode_utf16().collect(),
        ])
        .unwrap();
        assert_eq!(
            decode_file_paths(vec![vec![0xd800]]).unwrap_err(),
            ClipboardBackendError::InvalidUtf16
        );
        let result = select_clipboard_input(WindowsClipboardSnapshot {
            files: Some(Ok(files)),
            text: Some(Ok("fallback".into())),
            ..snapshot()
        })
        .unwrap();
        assert!(matches!(
            result,
            Some(ClipboardInput::Files { files, .. })
                if files == ["C:\\two.txt", "C:\\one.txt"]
        ));
    }

    #[test]
    fn html_takes_priority_over_plain_text() {
        let result = select_clipboard_input(WindowsClipboardSnapshot {
            html: Some(Ok("<b>rich</b>".into())),
            text: Some(Ok("rich".into())),
            ..snapshot()
        })
        .unwrap();
        assert!(matches!(
            result,
            Some(ClipboardInput::Html { html, text, .. })
                if html == "<b>rich</b>" && text.as_deref() == Some("rich")
        ));
    }

    #[test]
    fn registered_png_takes_priority_over_text() {
        let result = select_clipboard_input(WindowsClipboardSnapshot {
            high_confidence_image: Some(Ok(image())),
            text: Some(Ok("fallback".into())),
            ..snapshot()
        })
        .unwrap();
        assert!(matches!(
            result,
            Some(ClipboardInput::Image {
                width: 1,
                height: 1,
                rgba8,
                ..
            }) if rgba8 == [1, 2, 3, 255]
        ));
    }

    #[test]
    fn html_takes_priority_over_dib_fallback_and_text() {
        let result = select_clipboard_input(WindowsClipboardSnapshot {
            html: Some(Ok("<i>semantic</i>".into())),
            fallback_image: Some(Ok(image())),
            text: Some(Ok("semantic".into())),
            ..snapshot()
        })
        .unwrap();
        assert!(matches!(result, Some(ClipboardInput::Html { .. })));
    }

    #[test]
    fn valid_cf_html_without_fragment_offsets_returns_html() {
        let html = "<html><body>complete</body></html>";
        let header_template = concat!(
            "Version:1.0\r\n",
            "StartHTML:0000000000\r\n",
            "EndHTML:0000000000\r\n",
            "StartFragment:-1\r\n",
            "EndFragment:-1\r\n"
        );
        let start_html = header_template.len();
        let header = format!(
            "Version:1.0\r\nStartHTML:{start_html:010}\r\nEndHTML:{:010}\r\nStartFragment:-1\r\nEndFragment:-1\r\n",
            start_html + html.len()
        );
        assert_eq!(header.len(), start_html);
        assert_eq!(
            parse_cf_html(format!("{header}{html}").as_bytes()).unwrap(),
            html
        );
    }

    #[test]
    fn cf_html_fragment_offsets_return_only_fragment() {
        let fragment = "<strong>fragment</strong>";
        let html =
            format!("<html><body><!--StartFragment-->{fragment}<!--EndFragment--></body></html>");
        let header_template = concat!(
            "Version:1.0\r\n",
            "StartHTML:0000000000\r\n",
            "EndHTML:0000000000\r\n",
            "StartFragment:0000000000\r\n",
            "EndFragment:0000000000\r\n"
        );
        let start_html = header_template.len();
        let start_fragment = start_html + html.find(fragment).unwrap();
        let end_fragment = start_fragment + fragment.len();
        let header = format!(
            "Version:1.0\r\nStartHTML:{start_html:010}\r\nEndHTML:{:010}\r\nStartFragment:{start_fragment:010}\r\nEndFragment:{end_fragment:010}\r\n",
            start_html + html.len()
        );
        assert_eq!(header.len(), start_html);
        assert_eq!(
            parse_cf_html(format!("{header}{html}").as_bytes()).unwrap(),
            fragment
        );
    }

    #[test]
    fn malformed_cf_html_safely_falls_back_to_text() {
        let result = select_clipboard_input(WindowsClipboardSnapshot {
            html: Some(parse_cf_html(b"StartHTML:nope\r\n<html>broken</html>")),
            text: Some(Ok("fallback".into())),
            ..snapshot()
        })
        .unwrap();
        assert!(matches!(
            result,
            Some(ClipboardInput::Text { text, .. }) if text == "fallback"
        ));
    }

    #[test]
    fn unicode_text_decodes_valid_and_empty_values() {
        let mut bytes = "hello"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        bytes.extend_from_slice(&[0, 0]);
        assert_eq!(decode_unicode_text(&bytes).unwrap(), "hello");
        assert_eq!(decode_unicode_text(&[0, 0]).unwrap(), "");
        assert_eq!(
            decode_unicode_text(&[0]).unwrap_err(),
            ClipboardBackendError::InvalidUtf16
        );
    }

    #[test]
    fn canonical_image_rejects_invalid_rgba_length() {
        assert!(validate_canonical_image(1, 1, vec![1, 2, 3, 4]).is_ok());
        assert_eq!(
            validate_canonical_image(2, 1, vec![0; 4]).unwrap_err(),
            ClipboardBackendError::InvalidImage
        );
    }

    #[test]
    fn dib_is_decoded_to_canonical_rgba8() {
        let mut dib = vec![0u8; 44];
        dib[0..4].copy_from_slice(&40u32.to_le_bytes());
        dib[4..8].copy_from_slice(&1i32.to_le_bytes());
        dib[8..12].copy_from_slice(&1i32.to_le_bytes());
        dib[12..14].copy_from_slice(&1u16.to_le_bytes());
        dib[14..16].copy_from_slice(&32u16.to_le_bytes());
        dib[20..24].copy_from_slice(&4u32.to_le_bytes());
        dib[40..44].copy_from_slice(&[3, 2, 1, 255]);
        assert_eq!(decode_dib(&dib).unwrap(), image());
    }

    #[test]
    fn sequence_tracker_ignores_same_sequence_and_accepts_new_sequence() {
        let mut tracker = SequenceTracker::default();
        assert!(tracker.should_process(7));
        assert!(!tracker.should_process(7));
        assert!(tracker.should_process(8));
    }

    #[test]
    fn source_application_keeps_only_executable_name() {
        assert_eq!(
            executable_name_from_path("C:\\Program Files\\Example\\Example.exe").as_deref(),
            Some("Example.exe")
        );
    }

    #[test]
    fn clipboard_busy_retry_is_bounded_to_250_milliseconds() {
        assert_eq!(CLIPBOARD_RETRY_DELAYS_MS.iter().sum::<u64>(), 250);
        assert_eq!(CLIPBOARD_RETRY_DELAYS_MS.len() + 1, 6);
    }
    #[test]
    fn watcher_start_stop_releases_message_window_without_touching_clipboard() {
        let watcher = WindowsClipboardWatcher::start().expect("start clipboard watcher");
        let hwnd = watcher.message_window_handle();
        assert_ne!(hwnd, 0);
        watcher.stop().expect("stop clipboard watcher");
        assert_eq!(unsafe { IsWindow(hwnd as HWND) }, 0);
    }
}
