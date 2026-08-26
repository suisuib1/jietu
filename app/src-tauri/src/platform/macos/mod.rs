pub(crate) mod capture;
#[cfg(any(target_os = "macos", test))]
pub(crate) mod clipboard;
#[cfg(any(target_os = "macos", test))]
pub(crate) mod logic;
#[cfg(target_os = "macos")]
pub(crate) mod paste;
pub(crate) mod permission;
