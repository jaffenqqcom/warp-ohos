//! Clipboard support for HarmonyOS NEXT, backed by the ability crate's system
//! pasteboard wrapper.
//!
//! The wrapper talks to `libpasteboard`/`libudmf` directly and carries plain
//! text, HTML and encoded images. Nothing is cached between operations, because
//! the underlying native handle is not `Send` while the `Clipboard`
//! implementation must be.

use std::path::Path;

use anyhow::Result;
use openharmony_ability::{
    read_content, write_content, ClipboardContent as PasteboardContent, ClipboardImage,
};
use warpui_core::clipboard::{Clipboard as ClipboardTrait, ClipboardContent, ImageData};

/// How many lines of pasted text are inspected when deciding whether the
/// clipboard holds file paths.
const MAX_LINES_SCANNED_FOR_PATHS: usize = 1024;

pub struct Clipboard;

impl Clipboard {
    pub fn new() -> Result<Self> {
        log::info!("ohos::clipboard::Clipboard::new: pasteboard-backed clipboard ready");
        Ok(Self)
    }

    /// Parses clipboard text for absolute file paths, following the same
    /// all-or-nothing rule as the Linux back-end: every scanned line must be an
    /// absolute path, otherwise the content is treated as ordinary text.
    fn parse_file_paths_from_text(text: &str) -> Option<Vec<String>> {
        let mut file_paths = Vec::new();
        for line in text.lines().take(MAX_LINES_SCANNED_FOR_PATHS) {
            let candidate = line.trim();
            if candidate.is_empty() {
                continue;
            }
            let path = Path::new(candidate);
            if !path.is_absolute() {
                return None;
            }
            file_paths.push(candidate.to_string());
        }

        if file_paths.is_empty() {
            return None;
        }
        Some(file_paths)
    }
}

impl ClipboardTrait for Clipboard {
    fn write(&mut self, contents: ClipboardContent) {
        let has_text = !contents.plain_text.is_empty();
        let html = contents.html.filter(|html| !html.is_empty());
        let images = contents.images.unwrap_or_default();

        if !has_text && html.is_none() && images.is_empty() {
            log::debug!("ohos::clipboard::write: nothing to write, skipping pasteboard update");
            return;
        }

        let pasteboard = PasteboardContent {
            plain_text: contents.plain_text,
            html,
            images: images
                .into_iter()
                .map(|image| ClipboardImage {
                    data: image.data,
                    mime_type: image.mime_type,
                })
                .collect(),
        };

        if write_content(&pasteboard) {
            log::info!("ohos::clipboard::write: the pasteboard accepted the content");
        } else {
            log::error!("ohos::clipboard::write: the pasteboard write failed");
        }
    }

    fn read(&mut self) -> ClipboardContent {
        let content = read_content();
        if content.plain_text.is_empty() && content.html.is_none() && content.images.is_empty() {
            log::debug!("ohos::clipboard::read: the pasteboard carries nothing");
            return ClipboardContent::default();
        }

        let paths = Self::parse_file_paths_from_text(&content.plain_text);
        // The pasteboard reports no file name of its own; the HTML flavour is
        // the only place it can come from, as on the macOS back-end.
        let filename = content
            .html
            .as_deref()
            .and_then(warpui_core::clipboard_utils::extract_filename_from_html);

        log::info!(
            "ohos::clipboard::read: {} text byte(s), paths={}, html={}, image(s)={}",
            content.plain_text.len(),
            paths.as_ref().map_or(0, Vec::len),
            content.html.is_some(),
            content.images.len()
        );

        let images = (!content.images.is_empty()).then(|| {
            content
                .images
                .into_iter()
                .map(|image| ImageData {
                    data: image.data,
                    mime_type: image.mime_type,
                    filename: filename.clone(),
                })
                .collect()
        });

        ClipboardContent {
            plain_text: content.plain_text,
            paths,
            html: content.html,
            images,
        }
    }
}
