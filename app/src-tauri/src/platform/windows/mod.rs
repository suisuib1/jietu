#[cfg(target_os = "windows")]
pub(crate) mod capture;
#[cfg(target_os = "windows")]
#[allow(dead_code)]
pub(crate) mod clipboard;
pub(crate) mod paste;
