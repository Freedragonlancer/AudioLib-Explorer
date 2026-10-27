// server/audiolib-server/src/artwork.rs
//! On-demand embedded cover for `GET /audio/{id}/artwork`.
//!
//! The picture is read from the audio file on disk (`audiolib_audio::probe_with_artwork`).
//! Image bytes are never written into the library SQLite database or into snapshots.
//! Resolution is the audio id's file path, so a schema 6 library works without the
//! schema 7 album column. An optional disk cache under the server data directory
//! (next to the library) keys entries by library, id, path and source file identity.
use crate::{
    ApiError, AppState,
    state::{OpenError, OpenProgress},
    stream,
};
use audiolib_core::Error;
use std::{
    fs, io,
    path::{Path, PathBuf},
    time::SystemTime,
};

/// Bumped when thumbnail bytes for the same source and `side` change.
const ARTWORK_ALGORITHM: u32 = 1;
/// Same encoded-byte cap as ID3 extraction and the desktop cover decoder.
const MAX_ENCODED_BYTES: usize = 8 * 1024 * 1024;
/// Same dimension and allocation caps as `app/audiolib-desktop/src/artwork.rs`.
const MAX_DIMENSION: u32 = 4096;
const MAX_DECODE_BYTES: u64 = 64 * 1024 * 1024;
/// Largest accepted `side` (thumbnail long-side bound), matching the desktop decoder.
pub(crate) const MAX_SIDE: u32 = 512;
const JPEG_QUALITY: u8 = 85;
/// Cover cache stays under this size; oldest files are removed past it.
const MAX_CACHE_BYTES: u64 = 256 * 1024 * 1024;

/// JPEG or PNG bytes ready to send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Cover {
    /// `image/jpeg` or `image/png`.
    pub mime: &'static str,
    /// Encoded image. The original container bytes when `side` is omitted.
    pub bytes: Vec<u8>,
}

/// Reply of `GET /audio/{id}/artwork`.
pub(crate) enum ArtworkReply {
    /// `If-None-Match` hit. The file was not decoded again.
    NotModified { etag: String },
    /// Image body.
    Body {
        etag: String,
        mime: &'static str,
        bytes: Vec<u8>,
    },
}

/// `side`: omitted means the embedded JPEG/PNG; otherwise 1..=[`MAX_SIDE`].
pub(crate) fn parse_side(value: Option<&str>) -> Result<Option<u32>, ApiError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_empty() || value.len() > 3 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ApiError::validation(format!(
            "side must be 1..={MAX_SIDE}, got {value:?}"
        )));
    }
    let side: u32 = value.parse().unwrap_or(0);
    if !(1..=MAX_SIDE).contains(&side) {
        return Err(ApiError::validation(format!(
            "side must be 1..={MAX_SIDE}, got {value}"
        )));
    }
    Ok(Some(side))
}

/// Strong entity tag for one source file and thumbnail side (`0` = original bytes).
pub(crate) fn artwork_etag(file_etag: &str, side: u32) -> String {
    let inner = file_etag.trim().trim_start_matches("W/").trim_matches('"');
    format!("\"aa{ARTWORK_ALGORITHM}-{inner}-{side:x}\"")
}

impl AppState {
    /// Blocking. Open audio `id` like a stream, then return its embedded cover.
    ///
    /// No cover, a missing file and a failed extract are [`Error::NotFound`] (HTTP 404).
    /// A root that is offline stays [`OpenError::RootOffline`].
    pub(crate) fn artwork(
        &self,
        id: i64,
        side: Option<u32>,
        if_none_match: Option<&str>,
        progress: &OpenProgress,
    ) -> Result<ArtworkReply, OpenError> {
        let served = self.open_audio_reporting(id, progress)?;
        drop(served.file);
        progress.clear();
        let side_key = side.unwrap_or(0);
        let etag = artwork_etag(&served.etag, side_key);
        if if_none_match.is_some_and(|value| stream::none_match(value, &etag)) {
            return Ok(ArtworkReply::NotModified { etag });
        }
        let cache_dir = artwork_cache_dir(self);
        let key = cache_key(self.library_id(), id, &served.path, &served.etag, side_key);
        if let Some(cover) = load_cache(&cache_dir, &key) {
            return Ok(ArtworkReply::Body {
                etag,
                mime: cover.mime,
                bytes: cover.bytes,
            });
        }
        let cover = match render_file(&served.path, side) {
            Some(cover) => cover,
            None => {
                tracing::debug!(id, "Embedded cover is absent or could not be decoded");
                return Err(Error::NotFound(format!("Audio {id} has no embedded cover")).into());
            }
        };
        store_cache(&cache_dir, &key, &cover);
        Ok(ArtworkReply::Body {
            etag,
            mime: cover.mime,
            bytes: cover.bytes,
        })
    }
}

/// Server data directory cache: `<library directory>/artwork`.
fn artwork_cache_dir(state: &AppState) -> PathBuf {
    state
        .provider()
        .database_path()
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .join("artwork")
}

fn cache_key(library_id: &str, id: i64, path: &Path, file_etag: &str, side: u32) -> String {
    let material = format!(
        "{ARTWORK_ALGORITHM}\0{library_id}\0{id}\0{}\0{file_etag}\0{side}",
        path.display()
    );
    format!("{:016x}", fnv64(material.as_bytes()))
}

fn fnv64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn render_file(path: &Path, side: Option<u32>) -> Option<Cover> {
    let (_, artwork) = audiolib_audio::probe_with_artwork(path).ok()?;
    let artwork = artwork?;
    render(artwork.bytes, side)
}

/// Validate magic, dimension and allocation bounds, then optionally scale to `side`.
fn render(bytes: Vec<u8>, side: Option<u32>) -> Option<Cover> {
    let mime = classify(&bytes)?;
    let decoded = decode_limited(&bytes)?;
    match side {
        None => Some(Cover { mime, bytes }),
        Some(side) => encode(&decoded.thumbnail(side, side), mime),
    }
}

fn classify(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() > MAX_ENCODED_BYTES || bytes.is_empty() {
        return None;
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else {
        None
    }
}

fn decode_limited(bytes: &[u8]) -> Option<image::DynamicImage> {
    let mut reader = image::ImageReader::new(io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    reader.limits(limits);
    reader.decode().ok()
}

fn encode(image: &image::DynamicImage, mime: &'static str) -> Option<Cover> {
    let mut bytes = Vec::new();
    if mime == "image/jpeg" {
        let mut encoder =
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, JPEG_QUALITY);
        encoder.encode_image(image).ok()?;
    } else {
        image
            .write_to(&mut io::Cursor::new(&mut bytes), image::ImageFormat::Png)
            .ok()?;
    }
    let mime = classify(&bytes)?;
    Some(Cover { mime, bytes })
}

fn load_cache(dir: &Path, key: &str) -> Option<Cover> {
    for (extension, mime) in [("jpg", "image/jpeg"), ("png", "image/png")] {
        let path = dir.join(format!("{key}.{extension}"));
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        if classify(&bytes) == Some(mime) {
            return Some(Cover { mime, bytes });
        }
    }
    None
}

fn store_cache(dir: &Path, key: &str, cover: &Cover) {
    if fs::create_dir_all(dir).is_err() {
        return;
    }
    let extension = if cover.mime == "image/jpeg" {
        "jpg"
    } else {
        "png"
    };
    let path = dir.join(format!("{key}.{extension}"));
    let pid = std::process::id();
    let temporary = dir.join(format!(".{key}.{pid}.{extension}.tmp"));
    if fs::write(&temporary, &cover.bytes).is_err() {
        let _ = fs::remove_file(&temporary);
        return;
    }
    if fs::rename(&temporary, &path).is_err() {
        let _ = fs::remove_file(&path);
        if fs::rename(&temporary, &path).is_err() {
            let _ = fs::remove_file(&temporary);
            return;
        }
    }
    prune(dir, MAX_CACHE_BYTES);
}

fn prune(dir: &Path, max_bytes: u64) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut files = Vec::new();
    let mut total = 0_u64;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !is_cache_file(name) {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        total = total.saturating_add(metadata.len());
        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        files.push((modified, entry.path(), metadata.len()));
    }
    if total <= max_bytes {
        return;
    }
    files.sort_by_key(|(modified, _, _)| *modified);
    for (_, path, len) in files {
        if total <= max_bytes {
            break;
        }
        if fs::remove_file(path).is_ok() {
            total = total.saturating_sub(len);
        }
    }
}

fn is_cache_file(name: &str) -> bool {
    let Some((key, extension)) = name.rsplit_once('.') else {
        return false;
    };
    key.len() == 16
        && key.bytes().all(|byte| byte.is_ascii_hexdigit())
        && (extension == "jpg" || extension == "png")
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::GenericImageView;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbaImage::from_fn(width, height, |x, _| {
            image::Rgba([u8::try_from(x % 251).unwrap_or(1), 20, 40, 255])
        });
        let mut output = Vec::new();
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut io::Cursor::new(&mut output), image::ImageFormat::Png)
            .unwrap();
        output
    }

    fn jpeg(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbImage::from_pixel(width, height, image::Rgb([12, 34, 56]));
        let mut output = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut output, 80)
            .encode_image(&image::DynamicImage::ImageRgb8(image))
            .unwrap();
        output
    }

    #[test]
    fn side_accepts_only_a_bounded_decimal() {
        assert_eq!(parse_side(None).unwrap(), None);
        assert_eq!(parse_side(Some("40")).unwrap(), Some(40));
        assert_eq!(parse_side(Some("512")).unwrap(), Some(512));
        assert_eq!(parse_side(Some("01")).unwrap(), Some(1));
        for value in ["0", "513", "1000", "", "+8", "8.0", "1e2", " 8"] {
            assert!(parse_side(Some(value)).is_err(), "{value}");
        }
    }

    #[test]
    fn original_jpeg_and_png_pass_through_and_thumbnails_stay_bounded() {
        let png_bytes = png(80, 40);
        let original = render(png_bytes.clone(), None).unwrap();
        assert_eq!(original.mime, "image/png");
        assert_eq!(original.bytes, png_bytes);
        let thumb = render(png_bytes, Some(20)).unwrap();
        assert_eq!(thumb.mime, "image/png");
        let decoded = decode_limited(&thumb.bytes).unwrap();
        assert_eq!(decoded.dimensions(), (20, 10));

        let jpeg_bytes = jpeg(30, 10);
        let jpeg_cover = render(jpeg_bytes, Some(8)).unwrap();
        assert_eq!(jpeg_cover.mime, "image/jpeg");
        assert!(jpeg_cover.bytes.starts_with(&[0xff, 0xd8, 0xff]));
        assert!(decode_limited(&jpeg_cover.bytes).unwrap().width() <= 8);
    }

    #[test]
    fn undecodable_and_oversized_images_are_absent() {
        assert!(render(b"\x89PNG\r\n\x1a\nnot really".to_vec(), None).is_none());
        assert!(render(b"GIF89a".to_vec(), None).is_none());
        let mut huge = b"\xff\xd8\xff".to_vec();
        huge.resize(MAX_ENCODED_BYTES + 1, 0);
        assert!(render(huge, None).is_none());
        assert!(render(png(MAX_DIMENSION + 1, 1), Some(8)).is_none());
    }

    #[test]
    fn etag_and_cache_key_follow_the_source_identity() {
        let etag = artwork_etag("\"abc\"", 0);
        assert_eq!(etag, "\"aa1-abc-0\"");
        assert_ne!(artwork_etag("\"abc\"", 40), etag);
        assert!(stream::none_match(&etag, &etag));
        let first = cache_key("lib", 7, Path::new("/a.wav"), "\"abc\"", 0);
        assert_ne!(
            first,
            cache_key("lib", 8, Path::new("/a.wav"), "\"abc\"", 0)
        );
        assert_ne!(
            first,
            cache_key("lib", 7, Path::new("/a.wav"), "\"abd\"", 0)
        );
        assert_eq!(first.len(), 16);
    }

    #[test]
    fn cache_round_trip_prunes_the_oldest_files() {
        let dir = tempfile::tempdir().unwrap();
        let cover = Cover {
            mime: "image/png",
            bytes: png(4, 4),
        };
        store_cache(dir.path(), &"a".repeat(16), &cover);
        let loaded = load_cache(dir.path(), &"a".repeat(16)).unwrap();
        assert_eq!(loaded, cover);
        let older = dir.path().join(format!("{}.png", "b".repeat(16)));
        fs::write(&older, &cover.bytes).unwrap();
        filetime::set_file_mtime(&older, filetime::FileTime::from_unix_time(1, 0)).unwrap();
        let limit = fs::metadata(dir.path().join(format!("{}.png", "a".repeat(16))))
            .unwrap()
            .len();
        prune(dir.path(), limit);
        assert!(dir.path().join(format!("{}.png", "a".repeat(16))).is_file());
        assert!(!older.exists());
    }
}
