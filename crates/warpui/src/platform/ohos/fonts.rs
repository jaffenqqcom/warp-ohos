//! Font and text-layout access for HarmonyOS NEXT.
//!
//! The cosmic-text implementation under `windowing::winit::fonts` is not
//! winit-specific: it implements both [`platform::FontDB`] and
//! [`platform::TextLayoutSystem`], and only font *enumeration* is
//! platform-specific. OHOS has no fontconfig and no complete enumeration API,
//! so enumeration walks the system font directories and reads each font's
//! metadata; shaping, rasterization and per-character fallback selection are
//! shared with the other platforms and re-exported here rather than duplicated.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::os::raw::c_int;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use fontdb::Source;

use super::{fontcache, fontconfig};
use crate::fonts::{Properties, Style, Weight};
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

/// `OH_Drawing_SystemFontType::ALL`: every font type the graphics system knows.
const FONT_TYPE_ALL: c_int = 1 << 0;

/// UTF-16 string handed back by the ArkGraphics font manager, as declared in
/// `native_drawing/drawing_types.h`.
#[repr(C)]
struct OhDrawingString {
    str_data: *mut u8,
    str_len: u32,
}

#[link(name = "native_drawing")]
unsafe extern "C" {
    /// Returns one `OH_Drawing_String` per font file of `font_type`.
    ///
    /// The array belongs to the caller and the API exposes no matching release
    /// function, so the strings below are intentionally leaked; the scan that
    /// reads them runs at most once per process.
    fn OH_Drawing_GetFontPathsByType(
        font_type: c_int,
        path_count: *mut usize,
    ) -> *mut OhDrawingString;
}

/// A single font face that the text stack can load from disk.
pub(crate) struct ScannedFontFace {
    pub(crate) path: PathBuf,
    pub(crate) index: u32,
    pub(crate) is_monospace: bool,
    /// Distance from the normal weight; only used to order a family's faces.
    weight: u16,
    /// Whether the face is slanted; only used to order a family's faces.
    is_italic: bool,
    /// Codepoints the face covers, as ascending, coalesced, inclusive ranges.
    /// Used to trim fallback faces that contribute no new coverage.
    coverage: Box<[(u32, u32)]>,
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
        let mut database = fontdb::Database::new();
        let directories = SYSTEM_FONT_DIRS
            .iter()
            .map(PathBuf::from)
            .chain(reported_font_dirs());
        for directory in directories {
            if directory.is_dir() {
                database.load_fonts_dir(&directory);
            } else {
                log::debug!("ohos font scan: no fonts under {}", directory.display());
            }
        }

        let mut cache = fontcache::CoverageCache::load();
        let mut identities: HashMap<PathBuf, fontcache::FileIdentity> = HashMap::new();

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

            let identity = *identities
                .entry(path.clone())
                .or_insert_with(|| fontcache::file_identity(&path));

            let coverage = cache.coverage_for(&path, face.index, identity, || {
                face_coverage(&database, face.id)
            });

            grouped
                .entry(family_name.clone())
                .or_default()
                .push(ScannedFontFace {
                    path,
                    index: face.index,
                    is_monospace: face.monospaced,
                    weight: face.weight.0,
                    is_italic: !matches!(face.style, fontdb::Style::Normal),
                    coverage,
                });
        }
        cache.flush();

        let families = grouped
            .into_iter()
            .map(|(family_name, faces)| ScannedFontFamily { family_name, faces })
            .collect::<Vec<_>>();
        families
    })
}

/// Font directories the graphics system reports, excluding the ones already
/// scanned from [`SYSTEM_FONT_DIRS`].
///
/// User-installed fonts land outside the shipped directories, and where the
/// font installer puts them is its own business rather than a platform
/// constant, so the locations are asked for instead of guessed. Scanning the
/// reported directories rather than the reported files keeps the mmap-backed
/// load path and the ttc face expansion that `load_fonts_dir` provides.
fn reported_font_dirs() -> Vec<PathBuf> {
    let mut count = 0;
    // SAFETY: the callee writes the entry count through `count` and returns a
    // pointer to that many `OhDrawingString`s, or null on failure.
    let paths = unsafe { OH_Drawing_GetFontPathsByType(FONT_TYPE_ALL, &mut count) };
    if paths.is_null() {
        log::warn!("ohos font scan: font manager reported no font paths");
        return Vec::new();
    }

    let mut directories: Vec<PathBuf> = Vec::new();
    for index in 0..count {
        // SAFETY: the callee guarantees `count` initialized entries.
        let entry = unsafe { &*paths.add(index) };
        if entry.str_data.is_null() {
            continue;
        }
        // SAFETY: `str_data` holds `str_len` bytes of UTF-16 code units. The
        // units are read without a reference so that the alignment the callee
        // used does not matter.
        let units = (0..entry.str_len as usize / 2).map(|unit| unsafe {
            std::ptr::read_unaligned((entry.str_data as *const u16).add(unit))
        });
        let path = PathBuf::from(String::from_utf16_lossy(&units.collect::<Vec<_>>()));
        let Some(directory) = path.parent() else {
            continue;
        };
        if SYSTEM_FONT_DIRS
            .iter()
            .any(|scanned| directory == Path::new(scanned))
            || directories.iter().any(|seen| seen == directory)
        {
            continue;
        }
        directories.push(directory.to_path_buf());
    }
    log::info!(
        "ohos font scan: {} extra font directories from the font manager",
        directories.len()
    );
    directories
}

/// The faces to try when `excluded_family_name` has no glyph for a character.
///
/// Mirrors fontconfig's `FcFontSetSort(trim: true)`, which `platform::mac` and
/// the Linux back-end reach through fontconfig: candidate families are ordered
/// by closeness to the requested family/weight/slant, then faces whose Unicode
/// coverage is already provided by an earlier face are elided. HarmonyOS ships
/// no fontconfig, so the family order comes from its own configuration
/// ([`fontconfig`]) when available and from a monospace-first heuristic
/// otherwise. Symbol and emoji families stay last so they never answer for
/// characters the text families also cover.
pub(crate) fn fallback_font_faces(
    excluded_family_name: &str,
    properties: Properties,
) -> Vec<&'static ScannedFontFace> {
    let families = system_font_families();
    let ordered = ordered_fallback_faces(families, excluded_family_name, properties);
    trim_fallback_faces(families, excluded_family_name, ordered)
}

/// Orders every fallback face by family closeness, then by weight/slant.
fn ordered_fallback_faces<'a>(
    families: &'a [ScannedFontFamily],
    excluded_family_name: &str,
    properties: Properties,
) -> Vec<&'a ScannedFontFace> {
    let by_name: HashMap<String, &'a ScannedFontFamily> = families
        .iter()
        .map(|family| (family.family_name.to_lowercase(), family))
        .collect();
    let excluded = excluded_family_name.to_lowercase();

    let mut chosen: HashSet<String> = HashSet::new();
    let mut text_families: Vec<&'a ScannedFontFamily> = Vec::new();
    let mut symbol_families: Vec<&'a ScannedFontFamily> = Vec::new();

    if fontconfig::is_loaded() {
        let primary =
            fontconfig::generic_alias_of(excluded_family_name).unwrap_or(excluded_family_name);
        push_family(
            primary,
            &excluded,
            &by_name,
            &mut chosen,
            &mut text_families,
            &mut symbol_families,
        );
        for name in fontconfig::fallback_family_chain() {
            push_family(
                name,
                &excluded,
                &by_name,
                &mut chosen,
                &mut text_families,
                &mut symbol_families,
            );
        }
    } else {
        log::warn!(
            "ohos font fallback: no font configuration, using the monospace-first heuristic"
        );
        for family in families {
            if !is_symbol_family(&family.family_name)
                && family.faces.iter().any(|face| face.is_monospace)
            {
                push_family(
                    &family.family_name,
                    &excluded,
                    &by_name,
                    &mut chosen,
                    &mut text_families,
                    &mut symbol_families,
                );
            }
        }
    }

    // Whatever the configuration did not mention, as a last resort. Symbol and
    // emoji families are collected separately so they stay at the very end.
    for family in families {
        push_family(
            &family.family_name,
            &excluded,
            &by_name,
            &mut chosen,
            &mut text_families,
            &mut symbol_families,
        );
    }

    let want_italic = matches!(properties.style, Style::Italic);
    let want_weight = weight_value(properties.weight);

    let mut faces = Vec::new();
    for family in text_families.into_iter().chain(symbol_families) {
        let mut family_faces: Vec<&ScannedFontFace> = family.faces.iter().collect();
        family_faces.sort_by_key(|face| {
            (
                face.is_italic != want_italic,
                face.weight.abs_diff(want_weight),
            )
        });
        faces.extend(family_faces);
    }
    faces
}

/// Adds the family named `name`, unless it is the excluded one or already added.
fn push_family<'a>(
    name: &str,
    excluded: &str,
    by_name: &HashMap<String, &'a ScannedFontFamily>,
    chosen: &mut HashSet<String>,
    text_families: &mut Vec<&'a ScannedFontFamily>,
    symbol_families: &mut Vec<&'a ScannedFontFamily>,
) {
    let key = name.to_lowercase();
    if key.is_empty() || key == excluded || !chosen.insert(key.clone()) {
        return;
    }
    let Some(family) = by_name.get(&key) else {
        return;
    };
    if is_symbol_family(&family.family_name) {
        symbol_families.push(family);
    } else {
        text_families.push(family);
    }
}

/// Elides faces whose coverage is already provided by an earlier face.
fn trim_fallback_faces<'a>(
    families: &'a [ScannedFontFamily],
    excluded_family_name: &str,
    ordered: Vec<&'a ScannedFontFace>,
) -> Vec<&'a ScannedFontFace> {
    let excluded = excluded_family_name.to_lowercase();
    let mut covered: Box<[(u32, u32)]> = Vec::new().into_boxed_slice();
    if let Some(family) = families
        .iter()
        .find(|family| family.family_name.to_lowercase() == excluded)
    {
        for face in &family.faces {
            covered = coverage_union(&covered, &face.coverage);
        }
    }

    let mut kept = Vec::with_capacity(ordered.len());
    for face in ordered {
        // A face with no measurable coverage is kept rather than trimmed: the
        // coverage is unknown, not absent, and dropping it could leave a hole.
        if !face.coverage.is_empty() && coverage_is_subset(&face.coverage, &covered) {
            continue;
        }
        covered = coverage_union(&covered, &face.coverage);
        kept.push(face);
    }
    kept
}

/// Maps a font weight to its OpenType `usWeightClass` value.
fn weight_value(weight: Weight) -> u16 {
    match weight {
        Weight::Thin => 100,
        Weight::ExtraLight => 200,
        Weight::Light => 300,
        Weight::Normal => 400,
        Weight::Medium => 500,
        Weight::Semibold => 600,
        Weight::Bold => 700,
        Weight::ExtraBold => 800,
        Weight::Black => 900,
    }
}

/// The codepoints a face covers, parsed from its character map.
fn face_coverage(database: &fontdb::Database, id: fontdb::ID) -> Box<[(u32, u32)]> {
    let codepoints = database
        .with_face_data(id, |data, index| {
            let face = match owned_ttf_parser::Face::parse(data, index) {
                Ok(face) => face,
                Err(error) => {
                    log::warn!("ohos font scan: cannot parse a face for coverage: {error:?}");
                    return Vec::new();
                }
            };
            unicode_codepoints(&face)
        })
        .unwrap_or_default();
    coverage_ranges(codepoints)
}

/// Every codepoint the face maps through a Unicode character map.
fn unicode_codepoints(face: &owned_ttf_parser::Face) -> Vec<u32> {
    let Some(cmap) = face.tables().cmap else {
        return Vec::new();
    };

    // Formats 12 and 13 cover the full repertoire, so they already contain
    // everything the BMP subtables hold; reading one alone avoids enumerating
    // the same codepoints twice (the cost is dominated by CJK fonts).
    let mut codepoints = Vec::new();
    if let Some(subtable) = full_repertoire_subtable(&cmap) {
        subtable.codepoints(|codepoint| codepoints.push(codepoint));
        return codepoints;
    }
    for subtable in cmap.subtables {
        if subtable.is_unicode() {
            subtable.codepoints(|codepoint| codepoints.push(codepoint));
        }
    }
    codepoints
}

/// The first Unicode subtable whose format spans the full repertoire.
fn full_repertoire_subtable<'a>(
    cmap: &owned_ttf_parser::cmap::Table<'a>,
) -> Option<owned_ttf_parser::cmap::Subtable<'a>> {
    cmap.subtables.into_iter().find(|subtable| {
        subtable.is_unicode()
            && matches!(
                subtable.format,
                owned_ttf_parser::cmap::Format::SegmentedCoverage(_)
                    | owned_ttf_parser::cmap::Format::ManyToOneRangeMappings(_)
            )
    })
}

/// Coalesces a set of codepoints into ascending, inclusive ranges.
fn coverage_ranges(mut codepoints: Vec<u32>) -> Box<[(u32, u32)]> {
    codepoints.sort_unstable();
    codepoints.dedup();
    let mut ranges: Vec<(u32, u32)> = Vec::new();
    for codepoint in codepoints {
        match ranges.last_mut() {
            Some(last) if codepoint <= last.1.saturating_add(1) => {
                last.1 = last.1.max(codepoint);
            }
            _ => ranges.push((codepoint, codepoint)),
        }
    }
    ranges.into_boxed_slice()
}

/// Whether every range in `subset` is contained in `superset` (both ascending).
fn coverage_is_subset(subset: &[(u32, u32)], superset: &[(u32, u32)]) -> bool {
    let mut cursor = 0;
    for &(start, end) in subset {
        while cursor < superset.len() && superset[cursor].1 < start {
            cursor += 1;
        }
        let Some(&(outer_start, outer_end)) = superset.get(cursor) else {
            return false;
        };
        if start < outer_start || end > outer_end {
            return false;
        }
    }
    true
}

/// Merges two ascending range sets into one.
fn coverage_union(left: &[(u32, u32)], right: &[(u32, u32)]) -> Box<[(u32, u32)]> {
    let mut merged: Vec<(u32, u32)> = Vec::with_capacity(left.len() + right.len());
    let (mut left_index, mut right_index) = (0, 0);
    while left_index < left.len() || right_index < right.len() {
        let next = if right_index >= right.len()
            || (left_index < left.len() && left[left_index].0 <= right[right_index].0)
        {
            let range = left[left_index];
            left_index += 1;
            range
        } else {
            let range = right[right_index];
            right_index += 1;
            range
        };
        match merged.last_mut() {
            Some(last) if next.0 <= last.1.saturating_add(1) => last.1 = last.1.max(next.1),
            _ => merged.push(next),
        }
    }
    merged.into_boxed_slice()
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
