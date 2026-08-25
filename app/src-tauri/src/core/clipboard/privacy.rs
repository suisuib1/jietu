use std::collections::HashSet;

use super::ClipboardInput;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PrivacyRejection {
    ExcludedSource,
    EmptyContent,
    ContentTooLarge,
    InvalidImage,
    TooManyFiles,
    InvalidFileEntry,
}

#[derive(Clone, Debug)]
pub(crate) struct PrivacyPolicy {
    excluded_source_apps: HashSet<String>,
    pub(crate) max_text_bytes: usize,
    pub(crate) max_html_bytes: usize,
    pub(crate) max_image_bytes: usize,
    pub(crate) max_image_pixels: u64,
    pub(crate) max_files: usize,
    pub(crate) max_file_entry_bytes: usize,
}

impl Default for PrivacyPolicy {
    fn default() -> Self {
        Self {
            excluded_source_apps: HashSet::new(),
            max_text_bytes: 4 * 1024 * 1024,
            max_html_bytes: 8 * 1024 * 1024,
            max_image_bytes: 256 * 1024 * 1024,
            max_image_pixels: 64 * 1024 * 1024,
            max_files: 2_048,
            max_file_entry_bytes: 32 * 1024,
        }
    }
}

impl PrivacyPolicy {
    pub(crate) fn exclude_source_app(&mut self, source_app: impl AsRef<str>) {
        self.excluded_source_apps
            .insert(normalize_source_app(source_app.as_ref()));
    }

    pub(crate) fn validate(&self, input: &ClipboardInput) -> Result<(), PrivacyRejection> {
        if input
            .source_app()
            .map(normalize_source_app)
            .is_some_and(|app| self.excluded_source_apps.contains(&app))
        {
            return Err(PrivacyRejection::ExcludedSource);
        }

        match input {
            ClipboardInput::Text { text, .. } => validate_text(text, self.max_text_bytes),
            ClipboardInput::Html { html, text, .. } => {
                validate_text(html, self.max_html_bytes)?;
                if text
                    .as_ref()
                    .is_some_and(|text| text.len() > self.max_text_bytes)
                {
                    return Err(PrivacyRejection::ContentTooLarge);
                }
                Ok(())
            }
            ClipboardInput::Image {
                width,
                height,
                rgba8,
                ..
            } => self.validate_image(*width, *height, rgba8),
            ClipboardInput::Files { files, .. } => self.validate_files(files),
        }
    }

    fn validate_image(
        &self,
        width: u32,
        height: u32,
        rgba8: &[u8],
    ) -> Result<(), PrivacyRejection> {
        let pixels = u64::from(width)
            .checked_mul(u64::from(height))
            .ok_or(PrivacyRejection::InvalidImage)?;
        let expected_bytes = pixels
            .checked_mul(4)
            .and_then(|bytes| usize::try_from(bytes).ok())
            .ok_or(PrivacyRejection::InvalidImage)?;
        if width == 0 || height == 0 || rgba8.len() != expected_bytes {
            return Err(PrivacyRejection::InvalidImage);
        }
        if pixels > self.max_image_pixels || rgba8.len() > self.max_image_bytes {
            return Err(PrivacyRejection::ContentTooLarge);
        }
        Ok(())
    }

    fn validate_files(&self, files: &[String]) -> Result<(), PrivacyRejection> {
        if files.is_empty() {
            return Err(PrivacyRejection::EmptyContent);
        }
        if files.len() > self.max_files {
            return Err(PrivacyRejection::TooManyFiles);
        }
        if files.iter().any(|file| {
            file.trim().is_empty()
                || file.len() > self.max_file_entry_bytes
                || file.as_bytes().contains(&0)
        }) {
            return Err(PrivacyRejection::InvalidFileEntry);
        }
        Ok(())
    }
}

fn validate_text(value: &str, max_bytes: usize) -> Result<(), PrivacyRejection> {
    if value.is_empty() {
        return Err(PrivacyRejection::EmptyContent);
    }
    if value.len() > max_bytes {
        return Err(PrivacyRejection::ContentTooLarge);
    }
    Ok(())
}

fn normalize_source_app(value: &str) -> String {
    value.trim().to_lowercase()
}
