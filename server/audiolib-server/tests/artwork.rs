// server/audiolib-server/tests/artwork.rs
//! `GET /audio/{id}/artwork`: capability `album_artwork`, a JPEG/PNG body, and 404.
mod support;

use audiolib_db::{ScanCancellation, ScanOptions, ScanService, SqliteProvider};
use audiolib_server::{AppState, BearerToken, ErrorBody, ServerConfig, router};
use axum::{
    body::Body,
    http::{Request, StatusCode, header},
    response::Response,
};
use http_body_util::BodyExt;
use image::GenericImageView;
use rusqlite::{Connection, OpenFlags};
use std::{fs, io, path::Path, sync::Arc};
use tower::ServiceExt;

const TOKEN: &str = "artwork-test-token-0123456789abcdef";

fn real_path(path: &Path) -> std::path::PathBuf {
    let canonical = path.canonicalize().unwrap();
    let text = canonical.to_string_lossy().into_owned();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC") => rest.into(),
        _ => canonical,
    }
}

fn wav(path: &Path) {
    let data = [0_u8; 8];
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&48_000_u32.to_le_bytes());
    bytes.extend_from_slice(&96_000_u32.to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&data);
    fs::write(path, bytes).unwrap();
}

fn synchsafe(size: usize) -> [u8; 4] {
    [
        ((size >> 21) & 127) as u8,
        ((size >> 14) & 127) as u8,
        ((size >> 7) & 127) as u8,
        (size & 127) as u8,
    ]
}

fn embed_png(path: &Path, image: &[u8]) {
    let mut apic = b"\0image/png\0\x03\0".to_vec();
    apic.extend_from_slice(image);
    let mut frame = b"APIC".to_vec();
    frame.extend_from_slice(&(apic.len() as u32).to_be_bytes());
    frame.extend_from_slice(&[0, 0]);
    frame.extend_from_slice(&apic);
    let mut tag = b"ID3\x03\x00\x00".to_vec();
    tag.extend_from_slice(&synchsafe(frame.len()));
    tag.extend_from_slice(&frame);
    let mut bytes = fs::read(path).unwrap();
    let mut chunk = b"ID3 ".to_vec();
    chunk.extend_from_slice(&(tag.len() as u32).to_le_bytes());
    chunk.extend_from_slice(&tag);
    if !tag.len().is_multiple_of(2) {
        chunk.push(0);
    }
    bytes.extend(chunk);
    let size = (bytes.len() - 8) as u32;
    bytes[4..8].copy_from_slice(&size.to_le_bytes());
    fs::write(path, bytes).unwrap();
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let image = image::RgbaImage::from_fn(width, height, |x, y| {
        image::Rgba([
            u8::try_from((x * 3 + y) % 200 + 20).unwrap_or(20),
            80,
            160,
            255,
        ])
    });
    let mut output = Vec::new();
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut io::Cursor::new(&mut output), image::ImageFormat::Png)
        .unwrap();
    output
}

struct Fixture {
    _dir: tempfile::TempDir,
    db: std::path::PathBuf,
    state: AppState,
    cover_id: i64,
    plain_id: i64,
    image: Vec<u8>,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let base = real_path(dir.path());
    let sounds = base.join("sounds");
    fs::create_dir_all(&sounds).unwrap();
    let image = png(32, 16);
    let cover = sounds.join("cover.wav");
    let plain = sounds.join("plain.wav");
    wav(&cover);
    embed_png(&cover, &image);
    wav(&plain);
    let db = dir.path().join("library").join("library.db");
    let writer = Arc::new(SqliteProvider::open(&db).unwrap());
    let progress = ScanService::new(writer.clone())
        .scan(
            &sounds,
            &ScanOptions {
                incremental: false,
                ..Default::default()
            },
            &ScanCancellation::default(),
            |_| {},
        )
        .unwrap();
    assert_eq!(progress.added, 2, "{progress:?}");
    drop(writer);
    let config = ServerConfig::from_toml_str(
        &format!(
            "bind = \"127.0.0.1:0\"\nauth = true\nlibrary_db = 'library/library.db'\n\
             [[roots]]\nname = \"SFX\"\nserver_path = '{}'\n",
            sounds.display()
        ),
        dir.path(),
    )
    .unwrap();
    let state = AppState::open(&config, BearerToken::new(TOKEN).unwrap()).unwrap();
    let (cover_id, plain_id) = {
        let audio_id = |name: &str| -> i64 {
            Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap()
                .query_row(
                    "SELECT id FROM audio_files WHERE filename=?1",
                    [name],
                    |row| row.get(0),
                )
                .unwrap()
        };
        (audio_id("cover.wav"), audio_id("plain.wav"))
    };
    Fixture {
        _dir: dir,
        db,
        state,
        cover_id,
        plain_id,
        image,
    }
}

async fn send(state: &AppState, path: &str) -> Response {
    router(state.clone())
        .oneshot(
            support::modern(Request::get(path))
                .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn body(response: Response) -> Vec<u8> {
    response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec()
}

fn contains(path: &Path, needle: &[u8]) -> bool {
    fs::read(path)
        .ok()
        .is_some_and(|bytes| bytes.windows(needle.len()).any(|window| window == needle))
}

#[tokio::test]
async fn capabilities_advertise_album_artwork_and_cover_round_trips() {
    let f = fixture();
    let response = send(&f.state, "/api/v1/capabilities").await;
    assert_eq!(response.status(), StatusCode::OK);
    let caps: serde_json::Value = serde_json::from_slice(&body(response).await).unwrap();
    let features = caps["compatibility"]["features"]
        .as_object()
        .expect("features");
    assert!(
        features
            .get("album_artwork")
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
            >= 1,
        "{features:?}"
    );
    assert!(features.get("artwork").is_none(), "{features:?}");

    let path = format!("/api/v1/audio/{}/artwork", f.cover_id);
    let response = send(&f.state, &path).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "image/png"
    );
    assert_eq!(
        response.headers().get(header::CACHE_CONTROL).unwrap(),
        "private, no-cache"
    );
    let etag = response
        .headers()
        .get(header::ETAG)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    assert!(etag.starts_with("\"aa1-") && etag.ends_with('"'), "{etag}");
    let bytes = body(response).await;
    assert_eq!(bytes, f.image);
    assert!(
        f.db.parent()
            .unwrap()
            .join("artwork")
            .read_dir()
            .unwrap()
            .next()
            .is_some(),
        "cover cache is stored beside the library"
    );
    assert!(
        !contains(&f.db, &f.image),
        "cover bytes must stay out of SQLite"
    );
    let wal = f.db.with_extension("db-wal");
    if wal.exists() {
        assert!(!contains(&wal, &f.image));
    }

    let not_modified = router(f.state.clone())
        .oneshot(
            support::modern(Request::get(&path))
                .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                .header(header::IF_NONE_MATCH, &etag)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(not_modified.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(
        not_modified
            .headers()
            .get(header::ETAG)
            .unwrap()
            .to_str()
            .unwrap(),
        etag
    );
    assert!(body(not_modified).await.is_empty());

    let head = router(f.state.clone())
        .oneshot(
            support::modern(Request::builder().method("HEAD").uri(&path))
                .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(head.status(), StatusCode::OK);
    assert_eq!(
        head.headers().get(header::ETAG).unwrap().to_str().unwrap(),
        etag
    );
    assert!(body(head).await.is_empty());

    let thumb = send(&f.state, &format!("{path}?side=8")).await;
    assert_eq!(thumb.status(), StatusCode::OK);
    assert_eq!(
        thumb.headers().get(header::CONTENT_TYPE).unwrap(),
        "image/png"
    );
    let thumb_etag = thumb
        .headers()
        .get(header::ETAG)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    assert_ne!(thumb_etag, etag);
    let thumb_bytes = body(thumb).await;
    let decoded = image::ImageReader::new(io::Cursor::new(thumb_bytes))
        .with_guessed_format()
        .unwrap()
        .decode()
        .unwrap();
    assert_eq!(decoded.dimensions(), (8, 4));

    let missing_cover = send(&f.state, &format!("/api/v1/audio/{}/artwork", f.plain_id)).await;
    assert_eq!(missing_cover.status(), StatusCode::NOT_FOUND);
    let error: ErrorBody = serde_json::from_slice(&body(missing_cover).await).unwrap();
    assert_eq!(error.error_code, "NOT_FOUND");

    let unknown = send(&f.state, "/api/v1/audio/999999/artwork").await;
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);

    let bad_side = send(&f.state, &format!("{path}?side=0")).await;
    assert_eq!(bad_side.status(), StatusCode::BAD_REQUEST);
    let error: ErrorBody = serde_json::from_slice(&body(bad_side).await).unwrap();
    assert_eq!(error.error_code, "VALIDATION");

    let cover_path = Connection::open_with_flags(&f.db, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap()
        .query_row(
            "SELECT file_path FROM audio_files WHERE id=?1",
            [f.cover_id],
            |row| row.get::<_, String>(0),
        )
        .unwrap();
    fs::write(&cover_path, b"not a wav").unwrap();
    let broken = send(&f.state, &path).await;
    assert_eq!(
        broken.status(),
        StatusCode::NOT_FOUND,
        "extract failure is 404"
    );

    fs::remove_file(&cover_path).unwrap();
    let gone = send(&f.state, &path).await;
    assert_eq!(gone.status(), StatusCode::NOT_FOUND);
}
