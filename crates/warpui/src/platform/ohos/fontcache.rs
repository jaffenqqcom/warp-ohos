//! On-disk cache of per-face Unicode coverage.
//!
//! Scanning every face's character map costs a few hundred milliseconds, so the
//! result is persisted and reused while the font files are unchanged. Entries
//! are keyed by `(path, index)` and validated against the file's modification
//! time and size, so installed, removed or upgraded fonts are picked up on the
//! next start; anything not seen this run is dropped when the cache is
//! rewritten.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// Identifies the cache file and guards against reading an unrelated file.
const MAGIC: &[u8; 4] = b"WFC1";
/// Bumped when the entry layout changes so stale caches are discarded.
const FORMAT_VERSION: u32 = 1;
/// Cache location relative to the application's data directory (`HOME`).
const CACHE_RELATIVE_PATH: &str = ".cache/warp/font-coverage.bin";
/// Bytes an entry occupies before its path and coverage ranges.
const MINIMUM_ENTRY_BYTES: usize = 4 + 4 + 8 + 4 + 8 + 4;
/// Bytes a single coverage range occupies.
const RANGE_BYTES: usize = 8;

/// The identity of a font file: modification time and size.
pub(crate) type FileIdentity = (u64, u32, u64);

/// A face's coverage together with the file identity it was computed from.
struct CachedFace {
    identity: FileIdentity,
    coverage: Box<[(u32, u32)]>,
}

/// A coverage cache backed by a file below the application's data directory.
pub(crate) struct CoverageCache {
    file: Option<PathBuf>,
    loaded: HashMap<(PathBuf, u32), CachedFace>,
    fresh: HashMap<(PathBuf, u32), CachedFace>,
    computed: usize,
}

impl CoverageCache {
    /// Loads the cache, ignoring an unreadable or malformed one.
    pub(crate) fn load() -> Self {
        let file = cache_file();
        let loaded = match &file {
            Some(path) => match fs::read(path) {
                Ok(bytes) => match decode(&bytes) {
                    Ok(entries) => entries,
                    Err(error) => {
                        log::warn!("ohos font cache: discarding {}: {error}", path.display());
                        HashMap::new()
                    }
                },
                Err(error) => {
                    log::debug!("ohos font cache: no cache at {}: {error}", path.display());
                    HashMap::new()
                }
            },
            None => HashMap::new(),
        };
        Self {
            file,
            loaded,
            fresh: HashMap::new(),
            computed: 0,
        }
    }

    /// Returns the coverage of a face, computing it only on a cache miss.
    pub(crate) fn coverage_for(
        &mut self,
        path: &Path,
        index: u32,
        identity: FileIdentity,
        compute: impl FnOnce() -> Box<[(u32, u32)]>,
    ) -> Box<[(u32, u32)]> {
        let key = (path.to_path_buf(), index);
        if let Some(cached) = self.loaded.get(&key)
            && cached.identity == identity
        {
            let coverage = cached.coverage.clone();
            self.fresh.insert(
                key,
                CachedFace {
                    identity,
                    coverage: coverage.clone(),
                },
            );
            return coverage;
        }

        self.computed += 1;
        let coverage = compute();
        self.fresh.insert(
            key,
            CachedFace {
                identity,
                coverage: coverage.clone(),
            },
        );
        coverage
    }

    /// Persists the faces seen this run, dropping the ones that disappeared.
    pub(crate) fn flush(&self) {
        let Some(file) = &self.file else {
            log::warn!("ohos font cache: no writable location, skipping persistence");
            return;
        };
        // Nothing was recomputed and no face appeared or disappeared: the file
        // on disk is already current.
        if self.computed == 0 && self.fresh.len() == self.loaded.len() {
            return;
        }

        if let Some(parent) = file.parent()
            && let Err(error) = fs::create_dir_all(parent)
        {
            log::warn!(
                "ohos font cache: cannot create {}: {error}",
                parent.display()
            );
            return;
        }

        let encoded = encode(&self.fresh);
        if let Err(error) = fs::File::create(file).and_then(|mut file| file.write_all(&encoded)) {
            log::warn!("ohos font cache: cannot write {}: {error}", file.display());
        }
    }
}

/// The cache file location, derived from the application's data directory.
fn cache_file() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    if home.is_empty() {
        return None;
    }
    Some(PathBuf::from(home).join(CACHE_RELATIVE_PATH))
}

/// The identity of the file backing a face.
pub(crate) fn file_identity(path: &Path) -> FileIdentity {
    match fs::metadata(path) {
        Ok(metadata) => {
            let modified = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|duration| (duration.as_secs(), duration.subsec_nanos()))
                .unwrap_or((0, 0));
            (modified.0, modified.1, metadata.len())
        }
        Err(error) => {
            log::warn!("ohos font cache: cannot stat {}: {error}", path.display());
            (0, 0, u64::MAX)
        }
    }
}

fn encode(entries: &HashMap<(PathBuf, u32), CachedFace>) -> Vec<u8> {
    let mut buffer = Vec::new();
    buffer.extend_from_slice(MAGIC);
    push_u32(&mut buffer, FORMAT_VERSION);
    push_u32(&mut buffer, entries.len() as u32);
    for ((path, index), face) in entries {
        let bytes = path.to_string_lossy();
        let bytes = bytes.as_bytes();
        push_u32(&mut buffer, bytes.len() as u32);
        buffer.extend_from_slice(bytes);
        push_u32(&mut buffer, *index);
        push_u64(&mut buffer, face.identity.0);
        push_u32(&mut buffer, face.identity.1);
        push_u64(&mut buffer, face.identity.2);
        push_u32(&mut buffer, face.coverage.len() as u32);
        for &(start, end) in face.coverage.iter() {
            push_u32(&mut buffer, start);
            push_u32(&mut buffer, end);
        }
    }
    buffer
}

fn decode(bytes: &[u8]) -> io::Result<HashMap<(PathBuf, u32), CachedFace>> {
    let mut cursor = Cursor::new(bytes);
    if cursor.take(4)? != MAGIC {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "bad magic"));
    }
    if cursor.u32()? != FORMAT_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "stale format version",
        ));
    }

    // The count comes from the file, so it is only trusted as far as the file
    // is long enough to hold that many minimal entries; otherwise a corrupted
    // length field could reserve host memory without bound.
    let count = cursor.u32()? as usize;
    if count.saturating_mul(MINIMUM_ENTRY_BYTES) > cursor.remaining() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "entry count exceeds file size",
        ));
    }

    let mut entries = HashMap::with_capacity(count);
    for _ in 0..count {
        let path_len = cursor.u32()? as usize;
        let path = String::from_utf8(cursor.take(path_len)?.to_vec())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "path is not UTF-8"))?;
        let index = cursor.u32()?;
        let modified_secs = cursor.u64()?;
        let modified_nanos = cursor.u32()?;
        let size = cursor.u64()?;
        let range_count = cursor.u32()? as usize;
        if range_count.saturating_mul(RANGE_BYTES) > cursor.remaining() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "range count exceeds file size",
            ));
        }
        let mut coverage = Vec::with_capacity(range_count);
        for _ in 0..range_count {
            let start = cursor.u32()?;
            let end = cursor.u32()?;
            coverage.push((start, end));
        }
        entries.insert(
            (PathBuf::from(path), index),
            CachedFace {
                identity: (modified_secs, modified_nanos, size),
                coverage: coverage.into_boxed_slice(),
            },
        );
    }
    Ok(entries)
}

fn push_u32(buffer: &mut Vec<u8>, value: u32) {
    buffer.extend_from_slice(&value.to_le_bytes());
}

fn push_u64(buffer: &mut Vec<u8>, value: u64) {
    buffer.extend_from_slice(&value.to_le_bytes());
}

/// A minimal little-endian reader for the cache format.
struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn remaining(&self) -> usize {
        self.bytes.len() - self.offset
    }

    fn take(&mut self, length: usize) -> io::Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(length)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "truncated"))?;
        let slice = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(slice)
    }

    fn u32(&mut self) -> io::Result<u32> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn u64(&mut self) -> io::Result<u64> {
        let bytes = self.take(8)?;
        Ok(u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }
}
