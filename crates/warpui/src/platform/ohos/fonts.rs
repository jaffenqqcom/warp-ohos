//! Font and text-layout access for HarmonyOS NEXT.
//!
//! The cosmic-text implementation under `windowing::winit::fonts` is not
//! winit-specific: it implements both [`platform::FontDB`] and
//! [`platform::TextLayoutSystem`], and only font *enumeration* is
//! platform-specific. OHOS has no fontconfig and no complete enumeration API,
//! so enumeration walks the system font directories and reads each font's
//! metadata; shaping, rasterization and per-character fallback selection are
//! shared with the other platforms and re-exported here rather than duplicated.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use fontdb::Source;

pub use crate::windowing::winit::fonts::FontDB;

/// Directories that hold the fonts shipped with the system.
///
/// `/system/fonts` carries the whole shipped set; the others exist on some
/// devices only, so they are scanned opportunistically.
const SYSTEM_FONT_DIRS: &[&str] = &[
    "/system/fonts",
    "/system/font",
    "/vendor/fonts",
    "/system_ext/fonts",
    "/data/fonts",
];

/// A single font face that the text stack can load from disk.
pub(crate) struct ScannedFontFace {
    pub(crate) path: PathBuf,
    pub(crate) index: u32,
    pub(crate) is_monospace: bool,
}

/// A font family and every face it contains.
pub(crate) struct ScannedFontFamily {
    pub(crate) family_name: String,
    pub(crate) faces: Vec<ScannedFontFace>,
}

/// Every font family installed on the device, ordered by family name.
///
/// The scan parses the fonts once and is memoized: callers run on both the
/// layout path and the settings path, and re-reading the system font
/// directories per call would stall them.
pub(crate) fn system_font_families() -> &'static [ScannedFontFamily] {
    static FAMILIES: OnceLock<Vec<ScannedFontFamily>> = OnceLock::new();

    FAMILIES.get_or_init(|| {
        log::info!("ohos font scan: starting, directories={SYSTEM_FONT_DIRS:?}");
        let mut database = fontdb::Database::new();
        for directory in SYSTEM_FONT_DIRS {
            let path = Path::new(directory);
            if path.is_dir() {
                log::info!("ohos font scan: loading fonts under {directory}");
                database.load_fonts_dir(path);
            } else {
                log::debug!("ohos font scan: no fonts under {directory}");
            }
        }

        let mut grouped: BTreeMap<String, Vec<ScannedFontFace>> = BTreeMap::new();
        for face in database.faces() {
            let Some(path) = face_path(&face.source) else {
                log::warn!("ohos font scan: face {} is not backed by a file", face.id);
                continue;
            };
            let Some((family_name, _)) = face.families.first() else {
                log::warn!(
                    "ohos font scan: {} has no family name, skipping",
                    path.display()
                );
                continue;
            };

            grouped
                .entry(family_name.clone())
                .or_default()
                .push(ScannedFontFace {
                    path,
                    index: face.index,
                    is_monospace: face.monospaced,
                });
        }

        let families = grouped
            .into_iter()
            .map(|(family_name, faces)| ScannedFontFamily { family_name, faces })
            .collect::<Vec<_>>();
        log::info!(
            "ohos font scan: finished, {} families, {} faces",
            families.len(),
            families.iter().map(|family| family.faces.len()).sum::<usize>(),
        );
        families
    })
}

/// The faces to try when `excluded_family_name` has no glyph for a character.
///
/// Symbol and emoji families go last: they carry glyphs for characters that
/// text families also cover, and going first would render those characters as
/// emoji.
pub(crate) fn fallback_font_faces(excluded_family_name: &str) -> Vec<&'static ScannedFontFace> {
    let mut text_faces = Vec::new();
    let mut symbol_faces = Vec::new();
    for family in system_font_families() {
        if family.family_name == excluded_family_name {
            continue;
        }
        if is_symbol_family(&family.family_name) {
            symbol_faces.extend(family.faces.iter());
        } else {
            text_faces.extend(family.faces.iter());
        }
    }
    text_faces.extend(symbol_faces);
    text_faces
}

fn is_symbol_family(family_name: &str) -> bool {
    let lowercase = family_name.to_ascii_lowercase();
    lowercase.contains("emoji") || lowercase.contains("symbol")
}

fn face_path(source: &Source) -> Option<PathBuf> {
    match source {
        Source::File(path) => Some(path.clone()),
        Source::SharedFile(path, _) => Some(path.clone()),
        Source::Binary(_) => None,
    }
}
