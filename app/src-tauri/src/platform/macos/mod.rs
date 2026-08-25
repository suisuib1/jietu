pub(crate) mod capture;
#[cfg(any(target_os = "macos", test))]
pub(crate) mod clipboard;
pub(crate) mod permission;
