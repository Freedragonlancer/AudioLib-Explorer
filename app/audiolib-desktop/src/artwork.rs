// app/audiolib-desktop/src/artwork.rs
//! Bounded cover decoding and source-versioned thumbnail cache keys.
use audiolib_remote::{ArtworkReply, RemoteClient};
use image::{ImageReader, RgbaImage};
use std::{
    io::Cursor,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

const MAX_ENCODED_BYTES: usize = 8 * 1024 * 1024;
const MAX_DIMENSION: u32 = 4096;
const MAX_DECODE_BYTES: u64 = 64 * 1024 * 1024;
/// Selection cover requested from `GET /audio/{id}/artwork?side=`.
pub(crate) const SELECTION_COVER_SIDE: u32 = 512;

/// Decode with strict dimensions and an allocation budget, then retain only display pixels.
pub(crate) fn decode_thumbnail(bytes: &[u8], side: u32) -> Option<RgbaImage> {
    if bytes.len() > MAX_ENCODED_BYTES || side == 0 || side > 512 {
        return None;
    }
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    reader.limits(limits);
    Some(reader.decode().ok()?.thumbnail(side, side).into_rgba8())
}

/// Key artwork by library, real source and source version so rescans cannot reuse stale art.
pub(crate) fn thumbnail_path(
    cache: &Path,
    library: &Path,
    source: &Path,
    album: &str,
) -> Option<PathBuf> {
    let real = source.canonicalize().ok()?;
    let metadata = real.metadata().ok()?;
    let modified = metadata
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    let key = format!(
        "{}\0{}\0{}\0{}\0{album}",
        library.display(),
        real.display(),
        metadata.len(),
        modified.as_nanos()
    );
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in key.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    Some(cache.join("artwork").join(format!("{hash:016x}.png")))
}

/// A cached server cover for one library revision, or a remembered miss.
pub(crate) enum RemoteArtworkCached {
    Image(Vec<u8>),
    Absent,
}

/// Fetch the selected record's cover from `GET /audio/{id}/artwork` and the remote
/// disk cache. A missing capability is decided by the caller; `404` and transport
/// failures return no image and do not report a status.
pub(crate) fn remote_selection_cover(
    client: &RemoteClient,
    cache: &Path,
    library_id: &str,
    audio_id: i64,
    revision: u64,
    cancel: &AtomicBool,
) -> Option<RgbaImage> {
    match load_remote_artwork(cache, library_id, audio_id, revision) {
        Some(RemoteArtworkCached::Absent) => return None,
        Some(RemoteArtworkCached::Image(bytes)) => {
            return decode_thumbnail(&bytes, SELECTION_COVER_SIDE);
        }
        None => {}
    }
    if cancel.load(Ordering::Acquire) {
        return None;
    }
    match client.album_artwork(audio_id, Some(SELECTION_COVER_SIDE), None, cancel) {
        Ok(ArtworkReply::Data { bytes, etag, .. }) => {
            let image = decode_thumbnail(&bytes, SELECTION_COVER_SIDE)?;
            if let Some(png) = encode_png(&image) {
                store_remote_artwork(
                    cache,
                    library_id,
                    audio_id,
                    revision,
                    etag.as_deref().unwrap_or(""),
                    &png,
                );
            }
            Some(image)
        }
        Ok(ArtworkReply::Missing | ArtworkReply::NotModified { .. }) => {
            store_remote_artwork_absent(cache, library_id, audio_id, revision);
            None
        }
        Err(audiolib_remote::RemoteError::Cancelled) => None,
        Err(error) => {
            tracing::debug!(%error, audio_id, "album artwork request failed");
            None
        }
    }
}

pub(crate) fn load_remote_artwork(
    cache: &Path,
    library_id: &str,
    audio_id: i64,
    revision: u64,
) -> Option<RemoteArtworkCached> {
    let text = std::fs::read_to_string(remote_index_path(cache, library_id, audio_id, revision)).ok()?;
    let mut lines = text.lines();
    match lines.next()? {
        "absent" => Some(RemoteArtworkCached::Absent),
        "image" => {
            let etag = lines.next().unwrap_or("");
            let bytes = std::fs::read(remote_bytes_path(cache, library_id, audio_id, revision, etag)).ok()?;
            if bytes.is_empty() || bytes.len() > MAX_ENCODED_BYTES {
                return None;
            }
            Some(RemoteArtworkCached::Image(bytes))
        }
        _ => None,
    }
}

pub(crate) fn store_remote_artwork(
    cache: &Path,
    library_id: &str,
    audio_id: i64,
    revision: u64,
    etag: &str,
    png: &[u8],
) {
    let etag = etag_line(etag);
    let path = remote_bytes_path(cache, library_id, audio_id, revision, &etag);
    if !write_atomic(&path, png) {
        return;
    }
    let index = format!("image\n{etag}\n");
    write_atomic(
        &remote_index_path(cache, library_id, audio_id, revision),
        index.as_bytes(),
    );
}

pub(crate) fn store_remote_artwork_absent(
    cache: &Path,
    library_id: &str,
    audio_id: i64,
    revision: u64,
) {
    write_atomic(
        &remote_index_path(cache, library_id, audio_id, revision),
        b"absent\n",
    );
}

fn etag_line(etag: &str) -> String {
    if etag.contains(['\r', '\n']) || etag.len() > 256 {
        String::new()
    } else {
        etag.to_owned()
    }
}

fn remote_bytes_path(
    cache: &Path,
    library_id: &str,
    audio_id: i64,
    revision: u64,
    etag: &str,
) -> PathBuf {
    let key = format!("{library_id}\0{audio_id}\0{revision}\0{etag}");
    cache
        .join("artwork")
        .join("remote")
        .join(format!("{:016x}.png", fnv64(key.as_bytes())))
}

fn remote_index_path(cache: &Path, library_id: &str, audio_id: i64, revision: u64) -> PathBuf {
    let key = format!("{library_id}\0{audio_id}\0{revision}");
    cache
        .join("artwork")
        .join("remote")
        .join(format!("{:016x}.idx", fnv64(key.as_bytes())))
}

fn fnv64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn encode_png(image: &RgbaImage) -> Option<Vec<u8>> {
    let mut png = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image.clone())
        .write_to(&mut png, image::ImageFormat::Png)
        .ok()?;
    Some(png.into_inner())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return false;
    }
    let temporary = path.with_extension(format!("part-{}", std::process::id()));
    if std::fs::write(&temporary, bytes).is_err() {
        return false;
    }
    if std::fs::rename(&temporary, path).is_err() {
        let _ = std::fs::remove_file(path);
        if std::fs::rename(&temporary, path).is_err() {
            let _ = std::fs::remove_file(&temporary);
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbaImage::new(width, height);
        let mut output = Cursor::new(Vec::new());
        image
            .write_to(&mut output, image::ImageFormat::Png)
            .unwrap();
        output.into_inner()
    }

    #[test]
    fn thumbnails_bound_decoded_size_and_reject_oversized_images() {
        let decoded = decode_thumbnail(&encoded(800, 400), 40).unwrap();
        assert_eq!(decoded.dimensions(), (40, 20));
        assert!(decode_thumbnail(&encoded(MAX_DIMENSION + 1, 1), 40).is_none());
        assert!(decode_thumbnail(&vec![0; MAX_ENCODED_BYTES + 1], 40).is_none());
        assert!(decode_thumbnail(b"broken image", 40).is_none());
    }

    #[test]
    fn thumbnail_cache_distinguishes_libraries_sources_and_source_versions() {
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("first.wav");
        let second = root.path().join("second.wav");
        std::fs::write(&first, b"one").unwrap();
        std::fs::write(&second, b"two").unwrap();
        let key = |library: &str, source: &Path| {
            thumbnail_path(root.path(), Path::new(library), source, "Same album").unwrap()
        };
        let original = key("first.db", &first);
        assert_ne!(original, key("second.db", &first));
        assert_ne!(original, key("first.db", &second));
        std::fs::write(&first, b"replacement cover").unwrap();
        assert_ne!(original, key("first.db", &first));
    }

    #[test]
    fn remote_cache_keys_use_library_id_revision_and_etag_not_a_local_path() {
        let root = tempfile::tempdir().unwrap();
        let png = encoded(20, 10);
        store_remote_artwork(root.path(), "library-a", 7, 3, "\"e1\"", &png);
        match load_remote_artwork(root.path(), "library-a", 7, 3).unwrap() {
            RemoteArtworkCached::Image(bytes) => assert_eq!(bytes, png),
            RemoteArtworkCached::Absent => panic!("stored cover was marked absent"),
        }
        assert!(load_remote_artwork(root.path(), "library-b", 7, 3).is_none());
        assert!(load_remote_artwork(root.path(), "library-a", 8, 3).is_none());
        assert!(load_remote_artwork(root.path(), "library-a", 7, 4).is_none());
        let other = encoded(8, 8);
        store_remote_artwork(root.path(), "library-a", 7, 3, "\"e2\"", &other);
        match load_remote_artwork(root.path(), "library-a", 7, 3).unwrap() {
            RemoteArtworkCached::Image(bytes) => assert_eq!(bytes, other),
            RemoteArtworkCached::Absent => panic!("replacement cover was marked absent"),
        }
        assert_ne!(
            remote_bytes_path(root.path(), "library-a", 7, 3, "\"e1\""),
            remote_bytes_path(root.path(), "library-a", 7, 3, "\"e2\"")
        );
        store_remote_artwork_absent(root.path(), "library-a", 7, 3);
        assert!(matches!(
            load_remote_artwork(root.path(), "library-a", 7, 3),
            Some(RemoteArtworkCached::Absent)
        ));
        // No source file exists: the remote key does not canonicalize a WAV path.
        assert!(thumbnail_path(root.path(), Path::new("library-a"), Path::new("missing.wav"), "Album").is_none());
    }

    #[test]
    fn remote_selection_cover_decodes_artwork_and_blanks_on_404() {
        use std::io::{BufRead, Write};
        use std::sync::atomic::AtomicUsize;
        use std::sync::Arc;

        let png = encoded(80, 40);
        let hits = Arc::new(AtomicUsize::new(0));
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let png_body = png.clone();
        let counted = hits.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
                let mut request = String::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    request.push_str(&line);
                }
                let path = request
                    .lines()
                    .next()
                    .unwrap_or("")
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("");
                assert!(
                    !path.contains("/stream") && !path.contains("/preview"),
                    "{path}"
                );
                counted.fetch_add(1, Ordering::SeqCst);
                let (status, content_type, body, etag) =
                    if path.ends_with("/audio/3/artwork?side=512") {
                        (
                            200,
                            "image/png",
                            png_body.as_slice(),
                            "ETag: \"cover-3\"\r\n",
                        )
                    } else if path.ends_with("/audio/4/artwork?side=512") {
                        (
                            404,
                            "application/json",
                            br#"{"error_code":"NOT_FOUND","message":"No artwork"}"#.as_slice(),
                            "",
                        )
                    } else {
                        (
                            404,
                            "application/json",
                            br#"{"error_code":"NOT_FOUND","message":"No such endpoint"}"#.as_slice(),
                            "",
                        )
                    };
                let head = format!(
                    "HTTP/1.1 {status} Test\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n{etag}Connection: close\r\n\r\n",
                    body.len()
                );
                let mut stream = stream;
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(body);
            }
        });
        let client = RemoteClient::new(&format!("http://{address}"), "").unwrap();
        let cache = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(false);
        let image =
            remote_selection_cover(&client, cache.path(), "lib", 3, 9, &cancel).expect("cover");
        // `thumbnail` does not enlarge, so an 80×40 cover stays 80×40 inside a 512 side.
        assert_eq!(image.dimensions(), (80, 40));
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        let cached = remote_selection_cover(&client, cache.path(), "lib", 3, 9, &cancel)
            .expect("cached cover");
        assert_eq!(cached.dimensions(), (80, 40));
        assert_eq!(
            hits.load(Ordering::SeqCst),
            1,
            "a cached cover must not be fetched again"
        );
        assert!(remote_selection_cover(&client, cache.path(), "lib", 4, 9, &cancel).is_none());
        assert_eq!(hits.load(Ordering::SeqCst), 2);
        assert!(remote_selection_cover(&client, cache.path(), "lib", 4, 9, &cancel).is_none());
        assert_eq!(
            hits.load(Ordering::SeqCst),
            2,
            "a 404 is remembered until the revision changes"
        );
    }
}
