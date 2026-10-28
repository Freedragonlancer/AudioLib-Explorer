// server/audiolib-server/src/artwork.rs
//! Embedded covers for legacy WAV reads and shared album representations.
//!
//! The picture is read from the authorized audio file handle on disk.
//! Image bytes are never written into the library SQLite database or into snapshots.
//! Resolution is the audio id's file path, so a schema 6 library works without the
//! schema 7 album column. An optional disk cache under the server data directory
//! (next to the library) keys entries by library, id, path and source file identity.
use crate::{
    ApiError, AppState,
    state::{OpenError, OpenProgress},
    stream,
};
use audiolib_core::{DataProvider, Error, MAX_ALBUM_MEMBERS};
use std::{
    collections::VecDeque,
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::SystemTime,
};

/// Bumped when thumbnail bytes for the same source and `side` change.
const ARTWORK_ALGORITHM: u32 = 2;
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
const MAX_ALBUM_CHOICES: usize = 128;

#[derive(Clone, Copy)]
enum AlbumDirectory<'a> {
    Global,
    Folder(i64),
    Unassigned(&'a str),
}

/// Representation and the actual canonical directory observed by the resolver.
pub(crate) struct AlbumArtworkReply {
    /// Encoded pixels or a conditional representation hit.
    pub reply: ArtworkReply,
    /// Canonical source parent; absent only for the original global endpoint.
    pub directory_identity: Option<String>,
}

/// ASCII-safe wire encoding of an already canonical source directory identity.
pub(crate) fn directory_identity_hex(directory: &str) -> String {
    use std::fmt::Write as _;
    let mut hex = String::with_capacity(directory.len() * 2);
    for byte in directory.bytes() {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// A resolved album source is shared across member selections and image sizes.
struct AlbumChoice {
    key: String,
    audio_id: i64,
}

fn album_choices() -> &'static Mutex<VecDeque<AlbumChoice>> {
    static CHOICES: OnceLock<Mutex<VecDeque<AlbumChoice>>> = OnceLock::new();
    CHOICES.get_or_init(|| Mutex::new(VecDeque::new()))
}

fn album_reply(
    reply: ArtworkReply,
    library_id: &str,
    namespace: &str,
    album_id: i64,
    directory: AlbumDirectory<'_>,
    directory_identity: Option<&str>,
) -> ArtworkReply {
    let library = fnv64(format!("{library_id}\0{namespace}").as_bytes());
    let context = match directory {
        AlbumDirectory::Global => String::new(),
        AlbumDirectory::Folder(id) => format!(
            "folder-{id}-directory-{:016x}-",
            fnv64(directory_identity.unwrap_or_default().as_bytes())
        ),
        AlbumDirectory::Unassigned(identity) => {
            format!("directory-{:016x}-", fnv64(identity.as_bytes()))
        }
    };
    match reply {
        ArtworkReply::NotModified { etag } => ArtworkReply::NotModified {
            etag: format!(
                "\"album1-{library:x}-{album_id}-{context}{}\"",
                etag.trim_matches('"')
            ),
        },
        ArtworkReply::Body { etag, mime, bytes } => ArtworkReply::Body {
            etag: format!(
                "\"album1-{library:x}-{album_id}-{context}{}\"",
                etag.trim_matches('"')
            ),
            mime,
            bytes,
        },
    }
}

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
    /// Resolve one deterministic cover from the whole album, independent of the
    /// selected member or search filters. Only owned candidates and authorized
    /// source handles are used; no picture is stored in SQLite.
    pub(crate) fn album_artwork(
        &self,
        album_id: i64,
        side: Option<u32>,
        if_none_match: Option<&str>,
        cancel: &Arc<AtomicBool>,
        progress: &OpenProgress,
    ) -> Result<ArtworkReply, OpenError> {
        self.album_artwork_context(
            album_id,
            AlbumDirectory::Global,
            side,
            if_none_match,
            cancel,
            progress,
        )
        .map(|resolved| resolved.reply)
    }

    /// Resolve only direct members of an existing folder, before the candidate
    /// limit. An uncovered folder never borrows another folder's album image.
    pub(crate) fn album_artwork_in_folder(
        &self,
        album_id: i64,
        folder_id: i64,
        side: Option<u32>,
        if_none_match: Option<&str>,
        cancel: &Arc<AtomicBool>,
        progress: &OpenProgress,
    ) -> Result<AlbumArtworkReply, OpenError> {
        self.album_artwork_context(
            album_id,
            AlbumDirectory::Folder(folder_id),
            side,
            if_none_match,
            cancel,
            progress,
        )
    }

    /// Resolve only NULL-folder members with this canonical source parent identity.
    /// Directory identities are compared as strings and never accessed as OS paths.
    pub(crate) fn album_artwork_in_unassigned_directory(
        &self,
        album_id: i64,
        directory: &str,
        side: Option<u32>,
        if_none_match: Option<&str>,
        cancel: &Arc<AtomicBool>,
        progress: &OpenProgress,
    ) -> Result<AlbumArtworkReply, OpenError> {
        self.album_artwork_context(
            album_id,
            AlbumDirectory::Unassigned(directory),
            side,
            if_none_match,
            cancel,
            progress,
        )
    }

    fn album_artwork_context(
        &self,
        album_id: i64,
        directory: AlbumDirectory<'_>,
        side: Option<u32>,
        if_none_match: Option<&str>,
        cancel: &Arc<AtomicBool>,
        progress: &OpenProgress,
    ) -> Result<AlbumArtworkReply, OpenError> {
        self.provider().get_album(album_id)?;
        let (members, directory_identity) = match directory {
            AlbumDirectory::Folder(folder_id) => {
                let folder = self.provider().get_folder(folder_id)?;
                let identity = audiolib_db::normalize_source_path_identity(&folder.path);
                if identity.len() > 4096 {
                    return Err(Error::Validation(
                        "Canonical directory exceeds 4096 UTF-8 bytes".into(),
                    )
                    .into());
                }
                (
                    self.provider().album_members_in_folder(
                        album_id,
                        folder_id,
                        MAX_ALBUM_MEMBERS,
                        cancel.clone(),
                    )?,
                    Some(identity),
                )
            }
            AlbumDirectory::Unassigned(identity) => (
                self.provider().album_members_in_unassigned_directory(
                    album_id,
                    identity,
                    MAX_ALBUM_MEMBERS,
                    cancel.clone(),
                )?,
                Some(identity.to_owned()),
            ),
            AlbumDirectory::Global => (
                self.provider()
                    .album_members(album_id, MAX_ALBUM_MEMBERS, cancel.clone())?,
                None,
            ),
        };
        let namespace = self.provider().album_namespace();
        let mut material = format!(
            "{}\0{namespace}\0{}\0{album_id}\0{}",
            self.library_id(),
            self.provider().database_path().display(),
            self.revision()?.revision
        );
        use std::fmt::Write as _;
        match directory {
            AlbumDirectory::Global => {}
            AlbumDirectory::Folder(id) => {
                let _ = write!(material, "\0folder:{id}");
            }
            AlbumDirectory::Unassigned(identity) => {
                let _ = write!(material, "\0directory:{}:{identity}", identity.len());
            }
        }
        if let Some(identity) = &directory_identity {
            let _ = write!(material, "\0actual-directory:{}:{identity}", identity.len());
        }
        for member in &members {
            let _ = write!(
                material,
                "\0{}:{}:{}",
                member.id, member.file_size, member.file_path
            );
        }
        let key = format!("{:016x}", fnv64(material.as_bytes()));
        let cached_id = {
            let choices = album_choices()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            choices
                .iter()
                .find(|choice| choice.key == key)
                .map(|choice| choice.audio_id)
        };
        let mut candidates: Vec<i64> = members.iter().map(|member| member.id).collect();
        if let Some(cached_id) = cached_id
            && let Some(index) = candidates.iter().position(|id| *id == cached_id)
        {
            candidates.remove(index);
            candidates.insert(0, cached_id);
        }
        for audio_id in candidates {
            if cancel.load(Ordering::Acquire) {
                return Err(Error::Cancelled.into());
            }
            match self.artwork(audio_id, side, None, progress) {
                Ok(reply) => {
                    let current = self.provider().get_audio(audio_id)?;
                    let same_directory = match directory {
                        AlbumDirectory::Global => true,
                        AlbumDirectory::Folder(id) => current.folder_id == Some(id),
                        AlbumDirectory::Unassigned(identity) => {
                            current.folder_id.is_none()
                                && audiolib_db::source_parent_path_identity(&current.file_path)
                                    == identity
                        }
                    };
                    if current.album_id != Some(album_id) || !same_directory {
                        continue;
                    }
                    if let AlbumDirectory::Folder(id) = directory {
                        let current_folder = self.provider().get_folder(id)?;
                        if Some(audiolib_db::normalize_source_path_identity(
                            &current_folder.path,
                        )) != directory_identity
                        {
                            return Err(Error::Validation(
                                "Artwork directory changed during resolution; retry".into(),
                            )
                            .into());
                        }
                    }
                    let mut choices = album_choices()
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    choices.retain(|choice| choice.key != key);
                    choices.push_back(AlbumChoice { key, audio_id });
                    while choices.len() > MAX_ALBUM_CHOICES {
                        choices.pop_front();
                    }
                    drop(choices);
                    let reply = album_reply(
                        reply,
                        self.library_id(),
                        &namespace,
                        album_id,
                        directory,
                        directory_identity.as_deref(),
                    );
                    let etag = match &reply {
                        ArtworkReply::NotModified { etag } | ArtworkReply::Body { etag, .. } => {
                            etag
                        }
                    };
                    let reply =
                        if if_none_match.is_some_and(|value| stream::none_match(value, etag)) {
                            ArtworkReply::NotModified { etag: etag.clone() }
                        } else {
                            reply
                        };
                    return Ok(AlbumArtworkReply {
                        reply,
                        directory_identity,
                    });
                }
                Err(OpenError::Library(Error::NotFound(_))) => {}
                Err(error) => return Err(error),
            }
        }
        Err(Error::NotFound(format!("Album {album_id} has no embedded cover")).into())
    }

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
        let mut served = self.open_audio_reporting(id, progress)?;
        progress.clear();
        let side_key = side.unwrap_or(0);
        let etag = artwork_etag(&served.etag, side_key);
        let cache_dir = artwork_cache_dir(self);
        let key = cache_key(self.library_id(), id, &served.path, &served.etag, side_key);
        if let Some(cover) = load_cache(&cache_dir, &key) {
            if if_none_match.is_some_and(|value| stream::none_match(value, &etag)) {
                return Ok(ArtworkReply::NotModified { etag });
            }
            return Ok(ArtworkReply::Body {
                etag,
                mime: cover.mime,
                bytes: cover.bytes,
            });
        }
        let cover = match render_file(&mut served.file, side) {
            Some(cover) => cover,
            None => {
                tracing::debug!(id, "Embedded cover is absent or could not be decoded");
                return Err(Error::NotFound(format!("Audio {id} has no embedded cover")).into());
            }
        };
        if stream::file_etag(&served.file.metadata().map_err(Error::from)?) != served.etag {
            return Err(Error::NotFound("Audio changed during cover extraction".into()).into());
        }
        store_cache(&cache_dir, &key, &cover);
        // A source WAV existing does not imply that its optional cover exists.
        // Evaluate conditional headers only after resolving that representation,
        // so `If-None-Match: *` on an uncovered WAV still answers 404.
        if if_none_match.is_some_and(|value| stream::none_match(value, &etag)) {
            return Ok(ArtworkReply::NotModified { etag });
        }
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

fn render_file(file: &mut fs::File, side: Option<u32>) -> Option<Cover> {
    let (_, artwork) = audiolib_audio::probe_file_with_artwork(file).ok()?;
    let artwork = artwork?;
    render(artwork.bytes, side)
}

/// Validate magic, dimension and allocation bounds, then optionally scale to `side`.
fn render(bytes: Vec<u8>, side: Option<u32>) -> Option<Cover> {
    let mime = classify(&bytes)?;
    let decoded = decode_limited(&bytes)?;
    match side {
        None => Some(Cover { mime, bytes }),
        Some(side) if decoded.width() <= side && decoded.height() <= side => {
            Some(Cover { mime, bytes })
        }
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
        let Ok(file) = fs::File::open(&path) else {
            continue;
        };
        let mut bytes = Vec::new();
        // A damaged cache is disposable. Bound the read itself rather than allocating
        // the entire file before checking its size or trusting a raced metadata length.
        if file
            .take(MAX_ENCODED_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .is_ok()
            && classify(&bytes) == Some(mime)
            && decode_limited(&bytes).is_some()
        {
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
    // Different requests for the same cover must never share a partially written
    // temporary file or delete another request's published cache entry on a race.
    let Ok(mut temporary) = tempfile::Builder::new()
        .prefix(".artwork-")
        .suffix(".tmp")
        .tempfile_in(dir)
    else {
        return;
    };
    if temporary.write_all(&cover.bytes).is_err() || temporary.persist(&path).is_err() {
        return;
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
        let small_png = render(png_bytes.clone(), Some(512)).unwrap();
        assert_eq!(small_png.bytes, png_bytes);
        assert_eq!(
            decode_limited(&small_png.bytes).unwrap().dimensions(),
            (80, 40)
        );
        let thumb = render(png_bytes, Some(20)).unwrap();
        assert_eq!(thumb.mime, "image/png");
        let decoded = decode_limited(&thumb.bytes).unwrap();
        assert_eq!(decoded.dimensions(), (20, 10));

        let jpeg_bytes = jpeg(30, 10);
        let small_jpeg = render(jpeg_bytes.clone(), Some(512)).unwrap();
        assert_eq!(small_jpeg.bytes, jpeg_bytes);
        assert_eq!(
            decode_limited(&small_jpeg.bytes).unwrap().dimensions(),
            (30, 10)
        );
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
        assert_eq!(etag, "\"aa2-abc-0\"");
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

    #[test]
    fn cache_rejects_corrupt_or_oversized_entries_before_using_them() {
        let dir = tempfile::tempdir().unwrap();
        let key = "c".repeat(16);
        let path = dir.path().join(format!("{key}.png"));
        fs::write(&path, b"\x89PNG\r\n\x1a\ntruncated").unwrap();
        assert!(load_cache(dir.path(), &key).is_none());
        let file = fs::File::create(&path).unwrap();
        file.set_len(MAX_ENCODED_BYTES as u64 + 1024).unwrap();
        assert!(load_cache(dir.path(), &key).is_none());
        fs::write(&path, png(MAX_DIMENSION + 1, 1)).unwrap();
        assert!(load_cache(dir.path(), &key).is_none());
    }

    #[test]
    fn concurrent_cache_writes_publish_a_complete_image() {
        let dir = tempfile::tempdir().unwrap();
        let key = "d".repeat(16);
        let cover = Cover {
            mime: "image/png",
            bytes: png(24, 12),
        };
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let (dir, key, cover) = (dir.path(), &key, &cover);
                scope.spawn(move || store_cache(dir, key, cover));
            }
        });
        assert_eq!(load_cache(dir.path(), &key).unwrap(), cover);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
