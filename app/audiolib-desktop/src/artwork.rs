// app/audiolib-desktop/src/artwork.rs
//! Bounded cover decoding and source-versioned thumbnail cache keys.
use image::{ImageReader, RgbaImage};
use std::{
    io::Cursor,
    path::{Path, PathBuf},
};

const MAX_ENCODED_BYTES: usize = 8 * 1024 * 1024;
const MAX_DIMENSION: u32 = 4096;
const MAX_DECODE_BYTES: u64 = 64 * 1024 * 1024;

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
}
