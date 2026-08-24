#[cfg(target_os = "macos")]
use std::process::Command;

#[cfg(target_os = "macos")]
pub(crate) fn screen_permission_granted() -> bool {
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
    }
    unsafe { CGPreflightScreenCaptureAccess() }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn screen_permission_granted() -> bool {
    true
}

#[cfg(target_os = "macos")]
pub(crate) fn request_screen_permission() -> bool {
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGRequestScreenCaptureAccess() -> bool;
    }
    unsafe { CGRequestScreenCaptureAccess() }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn request_screen_permission() -> bool {
    true
}

pub(crate) fn open_screen_settings() {
    #[cfg(target_os = "macos")]
    let _ = Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")
        .spawn();
}
