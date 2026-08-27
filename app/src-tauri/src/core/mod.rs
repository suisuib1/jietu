pub(crate) mod capture;
#[allow(dead_code, unused_imports)]
pub(crate) mod clipboard;
pub(crate) mod image;
mod pin;
pub(crate) mod scroll;

pub(crate) use pin::{ImagePinPayload, PinManager};
