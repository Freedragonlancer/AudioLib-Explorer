// server/audiolib-server/src/waveform.rs
//! `GET /audio/{id}/waveform`: peak summaries computed with `audiolib_audio` and kept in
//! a server-side cache (default `<library_db directory>/waveforms`).
//!
//! Cache entries and the ETag are keyed by library identity, record id, canonical
//! source path, the source stream ETag, cache/algorithm versions and the
//! request parameters, so a cache hit only needs the file's metadata, never its audio
//! data. Compressed copies (`Accept-Encoding: zstd`) are cached next to the JSON. The
//! cache is trimmed to `waveform_cache_mb`, least recently used first.
use audiolib_audio::{WAVEFORM_CACHE_VERSION, WaveformData, WaveformRequest};
use audiolib_core::{Error, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::SystemTime,
};

/// Default and allowed number of peak columns.
pub const DEFAULT_BUCKETS: u32 = 1024;
/// Smallest accepted `buckets`.
pub const MIN_BUCKETS: u32 = 64;
/// Largest accepted `buckets`.
pub const MAX_BUCKETS: u32 = 8192;
/// zstd level for compressed waveform replies.
const ZSTD_LEVEL: i32 = 3;
/// The cache is trimmed after this many new entries.
const TRIM_EVERY: u64 = 32;
/// Server cache identity version; v2 separates libraries and distinct source files.
pub(crate) const SERVER_CACHE_VERSION: u32 = 2;

/// Validated `GET /audio/{id}/waveform` parameters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WaveformParams {
    /// Peak columns (`buckets`, 64..=8192, default 1024).
    pub buckets: u32,
    /// Include spectral colors (`colors`, default true).
    pub colors: bool,
    /// First frame (`start_frame`, default 0).
    pub start_frame: Option<u64>,
    /// End frame, exclusive (`end_frame`, default the end of the file).
    pub end_frame: Option<u64>,
}

impl WaveformParams {
    /// Validate raw query values; errors are `Validation` (400).
    pub fn parse(
        buckets: Option<&str>,
        colors: Option<&str>,
        start_frame: Option<&str>,
        end_frame: Option<&str>,
    ) -> Result<Self> {
        let number = |name: &str, value: &str| {
            value.parse::<u64>().map_err(|_| {
                Error::Validation(format!(
                    "{name} must be a non-negative integer, got {value:?}"
                ))
            })
        };
        let buckets = match buckets {
            None => DEFAULT_BUCKETS,
            Some(value) => {
                let parsed = number("buckets", value)?;
                if !(u64::from(MIN_BUCKETS)..=u64::from(MAX_BUCKETS)).contains(&parsed) {
                    return Err(Error::Validation(format!(
                        "buckets must be {MIN_BUCKETS}..={MAX_BUCKETS}, got {parsed}"
                    )));
                }
                parsed as u32
            }
        };
        let colors = match colors {
            None => true,
            Some(value) if value == "1" || value.eq_ignore_ascii_case("true") => true,
            Some(value) if value == "0" || value.eq_ignore_ascii_case("false") => false,
            Some(value) => {
                return Err(Error::Validation(format!(
                    "colors must be 0, 1, true or false, got {value:?}"
                )));
            }
        };
        let start_frame = start_frame.map(|v| number("start_frame", v)).transpose()?;
        let end_frame = end_frame.map(|v| number("end_frame", v)).transpose()?;
        if let (Some(start), Some(end)) = (start_frame, end_frame)
            && end <= start
        {
            return Err(Error::Validation(format!(
                "end_frame ({end}) must be greater than start_frame ({start})"
            )));
        }
        Ok(Self {
            buckets,
            colors,
            start_frame,
            end_frame,
        })
    }
}

/// Reply body: `audiolib_audio::WaveformData` plus the resolved frame range.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WaveformResponse {
    /// Peaks exactly as `audiolib_audio::waveform` returns them; `frames` is the number of
    /// frames covered (`end_frame - start_frame`).
    #[serde(flatten)]
    pub waveform: WaveformData,
    /// First frame covered.
    pub start_frame: u64,
    /// End of the covered range (exclusive).
    pub end_frame: u64,
    /// Frames in the whole file.
    pub total_frames: u64,
}

/// Cache identity of one request against one version of a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WaveformKey {
    hash: u64,
}

impl WaveformKey {
    /// Key scoped to a library, record and canonical source, then its file version
    /// and rendering parameters. Equal length/mtime values never merge two sources.
    pub fn new(
        library_id: &str,
        id: i64,
        source: &Path,
        file_etag: &str,
        params: &WaveformParams,
    ) -> Self {
        #[cfg(windows)]
        let source: Vec<u8> = {
            use std::os::windows::ffi::OsStrExt;
            source
                .as_os_str()
                .encode_wide()
                .flat_map(u16::to_le_bytes)
                .collect()
        };
        #[cfg(not(windows))]
        let source: Vec<u8> = source.as_os_str().as_encoded_bytes().to_vec();
        // JSON length-delimits strings and bytes so source names cannot alias tuple
        // boundaries. This tuple consists only of infallibly serializable primitives.
        let text = serde_json::to_vec(&(
            SERVER_CACHE_VERSION,
            WAVEFORM_CACHE_VERSION,
            library_id,
            id,
            source,
            file_etag,
            params.buckets,
            params.colors,
            params.start_frame,
            params.end_frame,
        ))
        .expect("Waveform identity primitives are serializable");
        Self {
            hash: fnv1a64(&text),
        }
    }

    /// The 64-bit key hash (shared by the list waveform entity tags, §9.1).
    pub fn hash(&self) -> u64 {
        self.hash
    }

    /// Entity tag; distinct per encoding.
    pub fn etag(&self, zstd: bool) -> String {
        let suffix = if zstd { "-zstd" } else { "" };
        format!(
            "W/\"wf{WAVEFORM_CACHE_VERSION}-k{SERVER_CACHE_VERSION}-{:016x}{suffix}\"",
            self.hash
        )
    }

    fn file_name(&self, zstd: bool) -> String {
        format!("{:016x}.json{}", self.hash, if zstd { ".zst" } else { "" })
    }
}

/// Stable 64-bit FNV-1a (std's `DefaultHasher` may change between Rust releases).
pub(crate) fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The on-disk waveform cache.
pub struct WaveformCache {
    dir: PathBuf,
    max_bytes: u64,
    writes: AtomicU64,
    /// Bytes measured by the last trim plus bytes written since ([`UNKNOWN_BYTES`]
    /// before the first trim). An estimate: overwrites are counted again.
    estimated: AtomicU64,
}

/// [`WaveformCache::estimated`] before any trim measured the directory.
const UNKNOWN_BYTES: u64 = u64::MAX;

impl WaveformCache {
    /// Cache in `dir` (created on first write) limited to `max_mb` MiB.
    pub fn new(dir: PathBuf, max_mb: u64) -> Self {
        Self {
            dir,
            max_bytes: max_mb.saturating_mul(1024 * 1024),
            writes: AtomicU64::new(0),
            estimated: AtomicU64::new(UNKNOWN_BYTES),
        }
    }

    /// Limit in bytes (`waveform_cache_mb`).
    pub fn limit_bytes(&self) -> u64 {
        self.max_bytes
    }

    /// Estimated cache size (see [`WaveformCache::estimated`]); never touches the disk.
    pub fn estimated_bytes(&self) -> Option<u64> {
        Some(self.estimated.load(Ordering::Relaxed)).filter(|bytes| *bytes != UNKNOWN_BYTES)
    }

    /// Cache directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Blocking. Cached bytes of `key` in one encoding (marking the entry recently used).
    pub fn get(&self, key: &WaveformKey, zstd: bool) -> Option<Vec<u8>> {
        let path = self.dir.join(key.file_name(zstd));
        let bytes = fs::read(&path).ok()?;
        if let Ok(file) = fs::File::options().write(true).open(&path) {
            let _ = file.set_modified(SystemTime::now());
        }
        Some(bytes)
    }

    /// Blocking. Store `bytes` for `key` atomically; trims the cache now and then.
    pub fn put(&self, key: &WaveformKey, zstd: bool, bytes: &[u8]) -> Result<()> {
        fs::create_dir_all(&self.dir)?;
        let mut stage = tempfile::Builder::new()
            .prefix(".waveform-")
            .tempfile_in(&self.dir)?;
        stage.write_all(bytes)?;
        stage
            .persist(self.dir.join(key.file_name(zstd)))
            .map_err(|error| Error::Io(error.error))?;
        let len = bytes.len() as u64;
        let _ = self
            .estimated
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |known| {
                (known != UNKNOWN_BYTES).then(|| known.saturating_add(len).min(UNKNOWN_BYTES - 1))
            });
        if self.writes.fetch_add(1, Ordering::Relaxed) % TRIM_EVERY == TRIM_EVERY - 1 {
            self.trim();
        }
        Ok(())
    }

    /// Blocking. Remove least recently used entries until the cache is within 90% of its
    /// limit. Only `*.json` / `*.json.zst` files are considered.
    pub fn trim(&self) {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return;
        };
        let mut files: Vec<(SystemTime, u64, PathBuf)> = entries
            .flatten()
            .filter(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                name.ends_with(".json") || name.ends_with(".json.zst")
            })
            .filter_map(|entry| {
                let metadata = entry.metadata().ok()?;
                metadata.is_file().then(|| {
                    (
                        metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                        metadata.len(),
                        entry.path(),
                    )
                })
            })
            .collect();
        let mut total: u64 = files.iter().map(|(_, len, _)| len).sum();
        if total <= self.max_bytes {
            self.estimated
                .store(total.min(UNKNOWN_BYTES - 1), Ordering::Relaxed);
            return;
        }
        let target = self.max_bytes / 10 * 9;
        files.sort();
        let mut removed = 0_usize;
        for (_, len, path) in files {
            if total <= target {
                break;
            }
            if fs::remove_file(&path).is_ok() {
                total = total.saturating_sub(len);
                removed += 1;
            }
        }
        self.estimated
            .store(total.min(UNKNOWN_BYTES - 1), Ordering::Relaxed);
        tracing::info!(removed, bytes = total, "Trimmed the waveform cache");
    }
}

/// Blocking. Compute the reply for `path` (no cache involved); honours `cancel`.
pub fn compute(path: &Path, params: &WaveformParams, cancel: &AtomicBool) -> Result<Vec<u8>> {
    let metadata = audiolib_audio::probe(path)?;
    let total = metadata.frames;
    let start = params.start_frame.unwrap_or(0);
    let end = params.end_frame.unwrap_or(total).min(total);
    if start >= end {
        return Err(Error::Validation(format!(
            "The frame range {start}..{end} is empty for a file of {total} frames"
        )));
    }
    let samples_per_pixel = u32::try_from((end - start).div_ceil(u64::from(params.buckets)))
        .unwrap_or(u32::MAX)
        .max(1);
    let waveform = audiolib_audio::waveform_with_cancel(
        path,
        WaveformRequest {
            samples_per_pixel,
            start_frame: start,
            end_frame: Some(end),
            include_colors: params.colors,
        },
        cancel,
    )?;
    serde_json::to_vec(&WaveformResponse {
        waveform,
        start_frame: start,
        end_frame: end,
        total_frames: total,
    })
    .map_err(|error| Error::Audio(format!("Cannot encode waveform: {error}")))
}

/// zstd-compress a reply body.
pub fn compress(bytes: &[u8]) -> Result<Vec<u8>> {
    Ok(zstd::encode_all(bytes, ZSTD_LEVEL)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parameters_are_validated_with_defaults() {
        let defaults = WaveformParams::parse(None, None, None, None).unwrap();
        assert_eq!(
            defaults,
            WaveformParams {
                buckets: 1024,
                colors: true,
                start_frame: None,
                end_frame: None
            }
        );
        let custom = WaveformParams::parse(Some("64"), Some("0"), Some("10"), Some("20")).unwrap();
        assert_eq!((custom.buckets, custom.colors), (64, false));
        assert!(WaveformParams::parse(Some("8192"), Some("TRUE"), None, None).is_ok());
        for (buckets, colors, start, end) in [
            (Some("63"), None, None, None),
            (Some("8193"), None, None, None),
            (Some("-1"), None, None, None),
            (Some("x"), None, None, None),
            (None, Some("2"), None, None),
            (None, None, Some("5"), Some("5")),
            (None, None, Some("a"), None),
        ] {
            assert!(
                matches!(
                    WaveformParams::parse(buckets, colors, start, end),
                    Err(Error::Validation(_))
                ),
                "{buckets:?} {colors:?} {start:?} {end:?}"
            );
        }
    }

    #[test]
    fn keys_cover_file_version_and_every_parameter() {
        let base = WaveformParams::parse(None, None, None, None).unwrap();
        let key = WaveformKey::new("lib", 1, Path::new("a.wav"), "\"a\"", &base);
        let variants = [
            WaveformKey::new("other-library", 1, Path::new("a.wav"), "\"a\"", &base),
            WaveformKey::new("lib", 2, Path::new("a.wav"), "\"a\"", &base),
            WaveformKey::new("lib", 1, Path::new("b.wav"), "\"a\"", &base),
            WaveformKey::new("lib", 1, Path::new("a.wav"), "\"b\"", &base),
            WaveformKey::new(
                "lib",
                1,
                Path::new("a.wav"),
                "\"a\"",
                &WaveformParams {
                    buckets: 512,
                    ..base
                },
            ),
            WaveformKey::new(
                "lib",
                1,
                Path::new("a.wav"),
                "\"a\"",
                &WaveformParams {
                    colors: false,
                    ..base
                },
            ),
            WaveformKey::new(
                "lib",
                1,
                Path::new("a.wav"),
                "\"a\"",
                &WaveformParams {
                    start_frame: Some(0),
                    ..base
                },
            ),
            WaveformKey::new(
                "lib",
                1,
                Path::new("a.wav"),
                "\"a\"",
                &WaveformParams {
                    end_frame: Some(9),
                    ..base
                },
            ),
        ];
        for other in &variants {
            assert_ne!(other.etag(false), key.etag(false));
        }
        assert_eq!(
            key,
            WaveformKey::new("lib", 1, Path::new("a.wav"), "\"a\"", &base)
        );
        assert_ne!(key.etag(true), key.etag(false));
        assert!(key.etag(false).starts_with("W/\"wf4-k2-"));
    }

    #[test]
    fn the_cache_trims_least_recently_used_entries() {
        let dir = tempfile::tempdir().unwrap();
        let cache = WaveformCache::new(dir.path().join("w"), 16);
        let big = vec![b'x'; 6 * 1024 * 1024];
        let keys: Vec<_> = (0..3)
            .map(|i| {
                WaveformKey::new(
                    "lib",
                    1,
                    Path::new("a.wav"),
                    &format!("\"{i}\""),
                    &WaveformParams::parse(None, None, None, None).unwrap(),
                )
            })
            .collect();
        for key in &keys {
            cache.put(key, false, &big).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        // Use the oldest entry so the middle one is the least recently used.
        assert!(cache.get(&keys[0], false).is_some());
        cache.trim();
        assert!(cache.get(&keys[0], false).is_some());
        assert!(cache.get(&keys[1], false).is_none());
        assert!(cache.get(&keys[2], false).is_some());
    }
}
