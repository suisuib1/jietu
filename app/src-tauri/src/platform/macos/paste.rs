//! macOS QuickPaste backend.
//!
//! This module owns only AppKit/CoreGraphics integration. Clipboard history,
//! hashing, suppression and usage accounting remain in the shared runtime.

use std::ffi::c_void;
use std::time::{Duration, Instant};

use objc2::{rc::autoreleasepool, runtime::ProtocolObject};
use objc2_app_kit::{
    NSPasteboard, NSPasteboardType, NSPasteboardTypeHTML, NSPasteboardTypePNG,
    NSPasteboardTypeString, NSPasteboardWriting, NSWorkspace,
};
use objc2_foundation::{NSArray, NSData, NSString, NSURL};

use crate::core::clipboard::ClipboardRestorePayload;

#[derive(Clone, Debug)]
pub(crate) struct MacPasteTarget {
    pub(crate) pid: i32,
    pub(crate) bundle_identifier: Option<String>,
}

#[derive(Debug)]
pub(crate) enum MacPasteError {
    ClipboardWrite,
    TargetUnavailable,
    ActivationFailed,
    EventCreation,
}

pub(crate) fn capture_frontmost() -> Option<MacPasteTarget> {
    autoreleasepool(|_| {
        let app = NSWorkspace::sharedWorkspace().frontmostApplication()?;
        let pid = app.processIdentifier();
        if pid <= 0 || pid == std::process::id() as i32 {
            return None;
        }
        let bundle_identifier = app.bundleIdentifier().map(|value| value.to_string());
        Some(MacPasteTarget {
            pid,
            bundle_identifier,
        })
    })
}

pub(crate) fn write_clipboard(payload: &ClipboardRestorePayload) -> Result<(), MacPasteError> {
    autoreleasepool(|_| {
        let pasteboard = NSPasteboard::generalPasteboard();
        if pasteboard.clearContents() == 0 {
            return Err(MacPasteError::ClipboardWrite);
        }

        let ok = unsafe {
            match payload {
                ClipboardRestorePayload::Text(text) => {
                    let value = NSString::from_str(text);
                    pasteboard.setString_forType(&value, &NSPasteboardTypeString)
                }
                ClipboardRestorePayload::Html { html, text } => {
                    let html_value = NSString::from_str(html);
                    let text_value = NSString::from_str(text);
                    pasteboard.setString_forType(&html_value, &NSPasteboardTypeHTML)
                        && pasteboard.setString_forType(&text_value, &NSPasteboardTypeString)
                }
                ClipboardRestorePayload::Image {
                    width,
                    height,
                    rgba8,
                } => {
                    let Some(image) = image::RgbaImage::from_raw(*width, *height, rgba8.clone())
                    else {
                        return Err(MacPasteError::ClipboardWrite);
                    };
                    let mut encoded = std::io::Cursor::new(Vec::new());
                    image::DynamicImage::ImageRgba8(image)
                        .write_to(&mut encoded, image::ImageFormat::Png)
                        .map_err(|_| MacPasteError::ClipboardWrite)?;
                    let data = NSData::with_bytes(encoded.get_ref());
                    pasteboard.setData_forType(Some(&data), &NSPasteboardTypePNG)
                }
                ClipboardRestorePayload::Files(files) => {
                    if files.is_empty() {
                        false
                    } else {
                        let objects = files
                            .iter()
                            .map(|path| {
                                let path_value = NSString::from_str(path);
                                ProtocolObject::from_retained(NSURL::fileURLWithPath(&path_value))
                            })
                            .collect::<Vec<_>>();
                        let objects = NSArray::from_retained_slice(&objects);
                        pasteboard.writeObjects(&objects)
                    }
                }
            }
        };

        if ok {
            Ok(())
        } else {
            Err(MacPasteError::ClipboardWrite)
        }
    })
}

pub(crate) fn activate_target(target: &MacPasteTarget) -> bool {
    autoreleasepool(|_| {
        let Some(app) =
            objc2_app_kit::NSRunningApplication::runningApplicationWithProcessIdentifier(
                target.pid,
            )
        else {
            return false;
        };
        if app.processIdentifier() == std::process::id() as i32 {
            return false;
        }
        if let Some(expected) = &target.bundle_identifier {
            let actual = app.bundleIdentifier().map(|id| id.to_string());
            if !super::logic::bundle_matches(Some(expected.as_str()), actual.as_deref()) {
                return false;
            }
        }
        if !app.activateWithOptions(
            objc2_app_kit::NSApplicationActivationOptions::ActivateIgnoringOtherApps,
        ) {
            return false;
        }
        let deadline = Instant::now() + Duration::from_millis(500);
        while Instant::now() < deadline {
            if app.isActive() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        app.isActive()
    })
}

pub(crate) fn send_command_v() -> Result<(), MacPasteError> {
    use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| MacPasteError::EventCreation)?;
    let mut command_down = CGEvent::new_keyboard_event(source.clone(), 55, true)
        .map_err(|_| MacPasteError::EventCreation)?;
    let mut v_down = CGEvent::new_keyboard_event(source.clone(), 9, true)
        .map_err(|_| MacPasteError::EventCreation)?;
    let mut v_up = CGEvent::new_keyboard_event(source.clone(), 9, false)
        .map_err(|_| MacPasteError::EventCreation)?;
    let mut command_up =
        CGEvent::new_keyboard_event(source, 55, false).map_err(|_| MacPasteError::EventCreation)?;
    let flags = CGEventFlags::CGEventFlagCommand;
    command_down.set_flags(flags);
    v_down.set_flags(flags);
    v_up.set_flags(flags);
    command_up.set_flags(CGEventFlags::empty());
    command_down.post(CGEventTapLocation::HID);
    v_down.post(CGEventTapLocation::HID);
    v_up.post(CGEventTapLocation::HID);
    command_up.post(CGEventTapLocation::HID);
    Ok(())
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;
    static kAXTrustedCheckOptionPrompt: *const c_void;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFDictionaryCreate(
        allocator: *const c_void,
        keys: *const *const c_void,
        values: *const *const c_void,
        count: isize,
        key_callbacks: *const c_void,
        value_callbacks: *const c_void,
    ) -> *const c_void;
    static kCFBooleanTrue: *const c_void;
    fn CFRelease(value: *const c_void);
}

pub(crate) fn accessibility_trusted(prompt: bool) -> bool {
    unsafe {
        if !prompt {
            return AXIsProcessTrustedWithOptions(std::ptr::null());
        }
        let key = kAXTrustedCheckOptionPrompt;
        let value = kCFBooleanTrue;
        let options = CFDictionaryCreate(
            std::ptr::null(),
            &key,
            &value,
            1,
            std::ptr::null(),
            std::ptr::null(),
        );
        let trusted = AXIsProcessTrustedWithOptions(options);
        if !options.is_null() {
            CFRelease(options);
        }
        trusted
    }
}
