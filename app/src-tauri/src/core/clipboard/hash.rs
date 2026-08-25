use sha2::{Digest, Sha256};

use super::ClipboardInput;

const HASH_DOMAIN: &[u8] = b"litesnap-clipboard-v1\0";

pub(crate) fn content_hash(input: &ClipboardInput) -> String {
    let mut digest = Sha256::new();
    digest.update(HASH_DOMAIN);
    match input {
        ClipboardInput::Text { text, .. } => {
            digest.update([1]);
            update_bytes(&mut digest, text.as_bytes());
        }
        ClipboardInput::Html { html, text, .. } => {
            digest.update([2]);
            update_bytes(&mut digest, html.as_bytes());
            update_optional_bytes(&mut digest, text.as_deref().map(str::as_bytes));
        }
        ClipboardInput::Image {
            width,
            height,
            rgba8,
            ..
        } => {
            digest.update([3]);
            digest.update(width.to_le_bytes());
            digest.update(height.to_le_bytes());
            update_bytes(&mut digest, rgba8);
        }
        ClipboardInput::Files { files, .. } => {
            digest.update([4]);
            digest.update((files.len() as u64).to_le_bytes());
            for file in files {
                update_bytes(&mut digest, file.as_bytes());
            }
        }
    }
    format!("{:x}", digest.finalize())
}

fn update_optional_bytes(digest: &mut Sha256, value: Option<&[u8]>) {
    match value {
        Some(value) => {
            digest.update([1]);
            update_bytes(digest, value);
        }
        None => digest.update([0]),
    }
}

fn update_bytes(digest: &mut Sha256, value: &[u8]) {
    digest.update((value.len() as u64).to_le_bytes());
    digest.update(value);
}
