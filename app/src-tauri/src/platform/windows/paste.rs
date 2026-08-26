use std::{
    borrow::Cow,
    mem, ptr, thread,
    time::{Duration, Instant},
};

use arboard::{Clipboard, ImageData};
use windows_sys::Win32::{
    Foundation::{GlobalFree, HANDLE, HWND},
    System::{
        DataExchange::{
            CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW,
            SetClipboardData,
        },
        Memory::{GMEM_MOVEABLE, GMEM_ZEROINIT, GlobalAlloc, GlobalLock, GlobalUnlock},
        Ole::{CF_HDROP, CF_UNICODETEXT},
    },
    UI::{
        Input::KeyboardAndMouse::{
            INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VK_CONTROL,
        },
        Shell::DROPFILES,
        WindowsAndMessaging::{GetForegroundWindow, IsWindow, SetForegroundWindow},
    },
};

use crate::core::clipboard::ClipboardRestorePayload;

#[derive(Debug)]
pub(crate) enum PasteError {
    Clipboard,
    InvalidPayload,
    Input,
}

pub(crate) fn current_foreground() -> isize {
    unsafe { GetForegroundWindow() as isize }
}
pub(crate) fn valid_target(hwnd: isize, history_hwnd: Option<isize>) -> bool {
    hwnd != 0 && unsafe { IsWindow(hwnd as HWND) != 0 } && history_hwnd != Some(hwnd)
}

pub(crate) fn write_clipboard(payload: &ClipboardRestorePayload) -> Result<(), PasteError> {
    match payload {
        ClipboardRestorePayload::Image {
            width,
            height,
            rgba8,
        } => {
            let mut clipboard = Clipboard::new().map_err(|_| PasteError::Clipboard)?;
            clipboard
                .set_image(ImageData {
                    width: *width as usize,
                    height: *height as usize,
                    bytes: Cow::Borrowed(rgba8),
                })
                .map_err(|_| PasteError::Clipboard)
        }
        ClipboardRestorePayload::Text(text) => write_native(text, None, None),
        ClipboardRestorePayload::Html { html, text } => write_native(text, Some(html), None),
        ClipboardRestorePayload::Files(files) => write_native("", None, Some(files)),
    }
}

fn write_native(
    text: &str,
    html: Option<&str>,
    files: Option<&[String]>,
) -> Result<(), PasteError> {
    unsafe {
        if OpenClipboard(ptr::null_mut()) == 0 {
            return Err(PasteError::Clipboard);
        }
        let result = (|| {
            if EmptyClipboard() == 0 {
                return Err(PasteError::Clipboard);
            }
            if let Some(files) = files {
                set_files(files)?;
            } else {
                set_unicode_text(text)?;
                if let Some(html) = html {
                    set_html(html)?;
                }
            }
            Ok(())
        })();
        CloseClipboard();
        result
    }
}

unsafe fn alloc_bytes(bytes: &[u8]) -> Result<HANDLE, PasteError> {
    let handle = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, bytes.len());
    if handle.is_null() {
        return Err(PasteError::Clipboard);
    }
    let ptr = GlobalLock(handle) as *mut u8;
    if ptr.is_null() {
        GlobalFree(handle);
        return Err(PasteError::Clipboard);
    }
    ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
    GlobalUnlock(handle);
    Ok(handle)
}

unsafe fn set_unicode_text(text: &str) -> Result<(), PasteError> {
    let mut bytes = Vec::with_capacity((text.encode_utf16().count() + 1) * 2);
    for unit in text.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    bytes.extend_from_slice(&[0, 0]);
    let handle = alloc_bytes(&bytes)?;
    if SetClipboardData(CF_UNICODETEXT as u32, handle).is_null() {
        GlobalFree(handle);
        return Err(PasteError::Clipboard);
    }
    Ok(())
}

unsafe fn set_html(html: &str) -> Result<(), PasteError> {
    let bytes = build_cf_html(html);
    let handle = alloc_bytes(&bytes)?;
    let name: Vec<u16> = "HTML Format"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let format = RegisterClipboardFormatW(name.as_ptr());
    if format == 0 || SetClipboardData(format, handle).is_null() {
        GlobalFree(handle);
        return Err(PasteError::Clipboard);
    }
    Ok(())
}

fn build_cf_html(html: &str) -> Vec<u8> {
    let fragment = format!("<!--StartFragment-->{html}<!--EndFragment-->");
    let body = fragment.as_bytes();
    let header_template = "Version:0.9\r\nStartHTML:0000000000\r\nEndHTML:0000000000\r\nStartFragment:0000000000\r\nEndFragment:0000000000\r\n";
    let header_len = header_template.len();
    let start_html = header_len;
    let end_html = start_html + body.len();
    let start_fragment = start_html + "<!--StartFragment-->".len();
    let end_fragment = end_html - "<!--EndFragment-->".len();
    let header = format!(
        "Version:0.9\r\nStartHTML:{start_html:010}\r\nEndHTML:{end_html:010}\r\nStartFragment:{start_fragment:010}\r\nEndFragment:{end_fragment:010}\r\n"
    );
    let mut bytes = header.into_bytes();
    bytes.extend_from_slice(body);
    bytes.push(0);
    bytes
}

unsafe fn set_files(files: &[String]) -> Result<(), PasteError> {
    if files.is_empty() {
        return Err(PasteError::InvalidPayload);
    }
    let mut names = Vec::<u16>::new();
    for file in files {
        names.extend(file.encode_utf16());
        names.push(0);
    }
    names.push(0);
    let header = mem::size_of::<DROPFILES>();
    let mut bytes = vec![0u8; header + names.len() * 2];
    let drop = bytes.as_mut_ptr() as *mut DROPFILES;
    (*drop).pFiles = header as u32;
    (*drop).fWide = 1;
    ptr::copy_nonoverlapping(
        names.as_ptr() as *const u8,
        bytes.as_mut_ptr().add(header),
        names.len() * 2,
    );
    let handle = alloc_bytes(&bytes)?;
    if SetClipboardData(CF_HDROP as u32, handle).is_null() {
        GlobalFree(handle);
        return Err(PasteError::Clipboard);
    }
    Ok(())
}

pub(crate) fn restore_foreground(hwnd: isize, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    if unsafe { SetForegroundWindow(hwnd as HWND) == 0 } {
        return false;
    }
    while Instant::now() < deadline {
        if current_foreground() == hwnd {
            return true;
        }
        thread::sleep(Duration::from_millis(15));
    }
    current_foreground() == hwnd
}

pub(crate) fn send_ctrl_v() -> Result<(), PasteError> {
    let inputs = [
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VK_CONTROL,
                    ..Default::default()
                },
            },
        },
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: b'V' as u16,
                    ..Default::default()
                },
            },
        },
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: b'V' as u16,
                    dwFlags: KEYEVENTF_KEYUP,
                    ..Default::default()
                },
            },
        },
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VK_CONTROL,
                    dwFlags: KEYEVENTF_KEYUP,
                    ..Default::default()
                },
            },
        },
    ];
    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            mem::size_of::<INPUT>() as i32,
        )
    };
    if sent == inputs.len() as u32 {
        Ok(())
    } else {
        Err(PasteError::Input)
    }
}

#[cfg(test)]
mod tests {
    use super::build_cf_html;
    #[test]
    fn html_offsets_use_utf8_bytes() {
        let html = "<p>你好</p>";
        let bytes = build_cf_html(html);
        let source = String::from_utf8(bytes[..bytes.len() - 1].to_vec()).unwrap();
        let start: usize = source[source.find("StartFragment:").unwrap() + 14..][..10]
            .parse()
            .unwrap();
        let end: usize = source[source.find("EndFragment:").unwrap() + 12..][..10]
            .parse()
            .unwrap();
        assert_eq!(&source.as_bytes()[start..end], html.as_bytes());
    }
}
