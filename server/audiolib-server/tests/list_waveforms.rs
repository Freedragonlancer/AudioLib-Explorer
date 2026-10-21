// server/audiolib-server/tests/list_waveforms.rs
//! List thumbnail waveforms (doc/api/README.md §9.1–§9.2): `POST /waveforms`,
//! `GET /waveforms/status`, pre-generation, `GET/PATCH /settings` and the
//! `/capabilities` fields, against temporary libraries of generated WAVs.
mod support;

use audiolib_db::{ScanCancellation, ScanOptions, ScanService, SqliteProvider};
use audiolib_server::{
    AppState, BearerToken, ConnectionInfo, ErrorBody, Listener, RootMonitor, RootPolicy, RootProbe,
    RootState, ServerConfig, Service,
    list_waveform::{BatchResponse, ItemStatus, ListWaveformStatus},
    router,
};
use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
    response::Response,
};
use http_body_util::BodyExt;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tower::ServiceExt;

const TOKEN: &str = "test-token-0123456789abcdef";
const LAN: &str = "10.0.0.20:50000";
const PUBLIC: &str = "203.0.113.9:50000";

/// 16-bit mono 48 kHz WAV whose samples follow `sample(i)`.
fn wav(path: &Path, frames: usize, sample: impl Fn(usize) -> i16) {
    let data: Vec<u8> = (0..frames).flat_map(|i| sample(i).to_le_bytes()).collect();
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
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn real_path(path: &Path) -> PathBuf {
    let canonical = path.canonicalize().unwrap();
    let text = canonical.to_string_lossy().into_owned();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC") => rest.into(),
        _ => canonical,
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    sounds: PathBuf,
    db: PathBuf,
    config: ServerConfig,
    state: AppState,
}

impl Fixture {
    fn id(&self, name: &str) -> i64 {
        rusqlite::Connection::open(&self.db)
            .unwrap()
            .query_row(
                "SELECT id FROM audio_files WHERE filename = ?1",
                [name],
                |row| row.get(0),
            )
            .unwrap()
    }
}

/// `tone.wav` (1000 frames, a ramp to +0.5 and a dip to -0.25), `silence.wav` (64
/// frames), `empty.wav` (0 frames); external auth mode, root `SFX`.
fn fixture(extra: &str) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let base = real_path(dir.path());
    let sounds = base.join("sounds");
    wav(&sounds.join("tone.wav"), 1000, |i| {
        if i == 10 {
            16384
        } else if i == 500 {
            -8192
        } else {
            0
        }
    });
    wav(&sounds.join("silence.wav"), 64, |_| 0);
    wav(&sounds.join("empty.wav"), 0, |_| 0);
    let db = dir.path().join("library").join("library.db");
    let writer = Arc::new(SqliteProvider::open(&db).unwrap());
    let progress = ScanService::new(writer)
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
    assert_eq!(progress.added, 3, "{progress:?}");
    let config = ServerConfig::from_toml_str(
        &format!(
            "bind = \"127.0.0.1:0\"\nlibrary_db = 'library/library.db'\nsnapshot_dir = 'snapshots'\n{extra}\
             [[roots]]\nname = \"SFX\"\nserver_path = '{}'\n",
            sounds.display()
        ),
        dir.path(),
    )
    .unwrap();
    let state = AppState::open(&config, BearerToken::new(TOKEN).unwrap()).unwrap();
    Fixture {
        _dir: dir,
        sounds,
        db,
        config,
        state,
    }
}

fn peer(address: &str) -> ConnectionInfo {
    ConnectionInfo {
        peer: address.parse().unwrap(),
        listener: Listener::Http,
    }
}

async fn call(
    service: impl Into<Service>,
    method: Method,
    path: &str,
    headers: &[(&str, &str)],
    address: &str,
    body: Option<&str>,
) -> Response {
    let mut request = support::modern(
        Request::builder()
            .method(method)
            .uri(format!("/api/v1{path}")),
    );
    if body.is_some()
        && !headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("content-type"))
    {
        request = request.header("content-type", "application/json");
    }
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let mut request = request
        .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
        .unwrap();
    request.extensions_mut().insert(peer(address));
    router(service.into()).oneshot(request).await.unwrap()
}

async fn bytes(response: Response) -> Vec<u8> {
    response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec()
}

async fn ok<T: serde::de::DeserializeOwned>(response: Response) -> T {
    let status = response.status();
    let body = bytes(response).await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    serde_json::from_slice(&body)
        .unwrap_or_else(|error| panic!("{error}: {}", String::from_utf8_lossy(&body)))
}

async fn error(response: Response, status: StatusCode, code: &str) {
    assert_eq!(response.status(), status);
    let body: ErrorBody = serde_json::from_slice(&bytes(response).await).unwrap();
    assert_eq!(body.error_code, code, "{}", body.message);
}

async fn batch(state: &AppState, body: &str) -> BatchResponse {
    ok(call(
        state.clone(),
        Method::POST,
        "/waveforms",
        &[],
        LAN,
        Some(body),
    )
    .await)
    .await
}

/// Request `ids` until no item is `pending` (the workers computed them).
async fn settled(state: &AppState, body: &str) -> BatchResponse {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let reply = batch(state, body).await;
        if reply
            .items
            .iter()
            .all(|item| item.status != ItemStatus::Pending)
        {
            return reply;
        }
        assert!(Instant::now() < deadline, "still pending: {reply:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn decode_base64(text: &str) -> Vec<u8> {
    let value = |c: u8| match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => panic!("not base64: {c}"),
    };
    let mut out = Vec::new();
    for chunk in text.as_bytes().chunks(4) {
        let pad = chunk.iter().filter(|c| **c == b'=').count();
        let bits = chunk
            .iter()
            .map(|c| if *c == b'=' { 0 } else { u32::from(value(*c)) })
            .fold(0_u32, |acc, v| (acc << 6) | v);
        let bytes = [(bits >> 16) as u8, (bits >> 8) as u8, bits as u8];
        out.extend_from_slice(&bytes[..3 - pad]);
    }
    out
}

fn values(peaks: &str) -> Vec<i16> {
    decode_base64(peaks)
        .chunks_exact(2)
        .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
        .collect()
}

#[tokio::test]
async fn batches_answer_pending_then_ok_not_modified_and_missing() {
    let f = fixture("");
    let tone = f.id("tone.wav");
    let silence = f.id("silence.wav");
    let empty = f.id("empty.wav");
    let body = format!(r#"{{"ids":[{tone},{silence},{empty},999999,{tone}]}}"#);

    let first = batch(&f.state, &body).await;
    assert_eq!(first.library_id, f.state.library_id());
    assert_eq!(first.buckets, 128);
    let ids: Vec<i64> = first.items.iter().map(|item| item.id).collect();
    assert_eq!(
        ids,
        vec![tone, silence, empty, 999999],
        "unique, request order"
    );
    assert_eq!(first.items[3].status, ItemStatus::Missing);
    if first.items[..3]
        .iter()
        .any(|item| item.status == ItemStatus::Pending)
    {
        let retry = first.retry_after.expect("retry_after with pending items");
        assert!((1..=5).contains(&retry), "{retry}");
    }

    let reply = settled(&f.state, &body).await;
    assert_eq!(reply.retry_after, None);
    let item = &reply.items[0];
    assert_eq!(item.status, ItemStatus::Ok, "{item:?}");
    // 1000 frames: samples_per_pixel 8, 125 columns.
    assert_eq!(item.n, Some(125));
    let peaks = item.peaks.as_deref().unwrap();
    assert_eq!(peaks.len(), (125 * 4_usize).div_ceil(3) * 4);
    let decoded = values(peaks);
    assert_eq!(decoded.len(), 250);
    // Column 1 holds frame 10 (+0.5), column 62 frame 500 (-0.25).
    assert_eq!(decoded[2..4], [0, 16384]);
    assert_eq!(decoded[124..126], [-8192, 0]);
    let etag = item.etag.clone().unwrap();
    assert!(
        etag.starts_with("W/\"wf4-k2-") && etag.ends_with("-l1\""),
        "{etag}"
    );
    assert_eq!(reply.items[1].n, Some(64));
    assert_eq!(reply.items[2].status, ItemStatus::Ok);
    assert_eq!(reply.items[2].n, Some(0));
    assert_eq!(reply.items[2].peaks.as_deref(), Some(""));

    // The list etag shares its hash with the detail waveform (buckets=128, colors=0).
    let detail = call(
        f.state.clone(),
        Method::GET,
        &format!("/audio/{tone}/waveform?buckets=128&colors=0"),
        &[],
        LAN,
        None,
    )
    .await;
    let detail_etag = detail.headers()[header::ETAG].to_str().unwrap().to_string();
    assert_eq!(format!("{}-l1\"", detail_etag.trim_end_matches('"')), etag);

    // `known` turns matching items into not_modified (no peaks).
    let known = format!(
        r#"{{"ids":[{tone},{silence}],"buckets":128,"known":{{"{tone}":{},"{silence}":"W/\"stale\""}}}}"#,
        serde_json::to_string(&etag).unwrap()
    );
    let reply = batch(&f.state, &known).await;
    assert_eq!(reply.items[0].status, ItemStatus::NotModified);
    assert_eq!(reply.items[0].etag.as_deref(), Some(etag.as_str()));
    assert!(reply.items[0].peaks.is_none());
    assert_eq!(reply.items[1].status, ItemStatus::Ok);

    // A record marked missing is `missing` without touching the disk.
    rusqlite::Connection::open(&f.db)
        .unwrap()
        .execute("UPDATE audio_files SET missing=1 WHERE id=?1", [silence])
        .unwrap();
    let reply = batch(&f.state, &format!(r#"{{"ids":[{silence}]}}"#)).await;
    assert_eq!(reply.items[0].status, ItemStatus::Missing);

    // Raw response: no-store, Vary, zstd on request.
    let response = call(
        f.state.clone(),
        Method::POST,
        "/waveforms",
        &[("accept-encoding", "zstd")],
        LAN,
        Some(&format!(r#"{{"ids":[{tone}]}}"#)),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(response.headers()[header::VARY], "accept-encoding");
    assert_eq!(response.headers()[header::CONTENT_ENCODING], "zstd");
    assert!(!response.headers().contains_key(header::ETAG));
    let packed = bytes(response).await;
    let unpacked: BatchResponse =
        serde_json::from_slice(&zstd::decode_all(&packed[..]).unwrap()).unwrap();
    assert_eq!(unpacked.items[0].status, ItemStatus::Ok);
}

#[tokio::test]
async fn invalid_batches_are_rejected() {
    let f = fixture("");
    let too_many = format!(
        r#"{{"ids":[{}]}}"#,
        (1..=201)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(",")
    );
    for body in [
        r#"{"ids":[1],"buckets":64}"#,
        r#"{"ids":[1],"buckets":1024}"#,
        too_many.as_str(),
        r#"{"ids":[]}"#,
        r#"{"ids":[0]}"#,
        r#"{"ids":[1],"known":{"one":"x"}}"#,
        r#"{"ids":[1],"known":{"1":1}}"#,
        "not json",
    ] {
        error(
            call(
                f.state.clone(),
                Method::POST,
                "/waveforms",
                &[],
                LAN,
                Some(body),
            )
            .await,
            StatusCode::BAD_REQUEST,
            "VALIDATION",
        )
        .await;
    }
    let ids200 = format!(
        r#"{{"ids":[{}]}}"#,
        (1..=200)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(",")
    );
    let reply = batch(&f.state, &ids200).await;
    assert_eq!(reply.items.len(), 200);
    error(
        call(
            f.state.clone(),
            Method::POST,
            "/waveforms",
            &[("content-type", "text/plain")],
            LAN,
            Some(r#"{"ids":[1]}"#),
        )
        .await,
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "JSON_CONTENT_TYPE_REQUIRED",
    )
    .await;
    // External requests still need the token (external auth mode).
    error(
        call(
            f.state.clone(),
            Method::POST,
            "/waveforms",
            &[],
            PUBLIC,
            Some(r#"{"ids":[1]}"#),
        )
        .await,
        StatusCode::UNAUTHORIZED,
        "UNAUTHORIZED",
    )
    .await;
}

#[tokio::test]
async fn modifying_a_file_changes_its_etag_and_recomputes() {
    let f = fixture("");
    let tone = f.id("tone.wav");
    let body = format!(r#"{{"ids":[{tone}]}}"#);
    let before = settled(&f.state, &body).await.items.remove(0);
    let old_etag = before.etag.unwrap();
    // Rewrite in place with different content (different size and mtime).
    std::thread::sleep(Duration::from_millis(20));
    wav(&f.sounds.join("tone.wav"), 2000, |i| {
        if i == 0 { -32768 } else { 0 }
    });
    let stale = batch(
        &f.state,
        &format!(
            r#"{{"ids":[{tone}],"known":{{"{tone}":{}}}}}"#,
            serde_json::to_string(&old_etag).unwrap()
        ),
    )
    .await;
    assert_eq!(stale.items[0].status, ItemStatus::Pending, "{stale:?}");
    let after = settled(&f.state, &body).await.items.remove(0);
    assert_eq!(after.status, ItemStatus::Ok);
    let new_etag = after.etag.unwrap();
    assert_ne!(new_etag, old_etag);
    assert_eq!(after.n, Some(125), "2000 frames: samples_per_pixel 16");
    assert_eq!(values(after.peaks.as_deref().unwrap())[0], -32767);
}

#[tokio::test]
async fn offline_roots_answer_root_offline_without_disk_access() {
    let f = fixture("");
    let online = Arc::new(AtomicBool::new(false));
    let probe: RootProbe = {
        let online = online.clone();
        Arc::new(move |path: &Path| {
            if online.load(Ordering::SeqCst) {
                Ok(path.to_path_buf())
            } else {
                Err("Root directory not found (volume not mounted?)".into())
            }
        })
    };
    let monitor = RootMonitor::start_with_probe(
        &f.config.roots,
        RootPolicy {
            interval: Duration::from_secs(3600),
            waking_after: Duration::from_millis(100),
            offline_after: Duration::from_secs(60),
        },
        probe,
    );
    let state =
        AppState::open_with_monitor(&f.config, BearerToken::new(TOKEN).unwrap(), monitor).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while state.root_statuses()[0].state != RootState::Offline {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let tone = f.id("tone.wav");
    let reply = batch(&state, &format!(r#"{{"ids":[{tone}]}}"#)).await;
    assert_eq!(reply.items[0].status, ItemStatus::RootOffline);
    assert_eq!(reply.items[0].root.as_deref(), Some("SFX"));
    assert_eq!(reply.retry_after, None);
}

async fn status(state: &AppState) -> ListWaveformStatus {
    ok(call(
        state.clone(),
        Method::GET,
        "/waveforms/status",
        &[],
        LAN,
        None,
    )
    .await)
    .await
}

#[tokio::test]
async fn pregeneration_fills_the_cache_and_writes_atomic_shards() {
    let f = fixture("pregenerate_list_waveforms = true\nlist_waveform_rate = 1000\n");
    let deadline = Instant::now() + Duration::from_secs(30);
    let done = loop {
        let current = status(&f.state).await;
        assert!(current.enabled);
        if current.pass_finished_at.is_some() {
            break current;
        }
        assert!(Instant::now() < deadline, "{current:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(done.state, "idle", "{done:?}");
    assert_eq!(done.total, 3);
    assert_eq!(done.cached, 3);
    assert_eq!(done.remaining, Some(0));
    assert_eq!(done.rate, 1000);
    assert_eq!(done.cache_limit_bytes, 640 * 1024 * 1024);
    assert!(!done.over_budget);
    assert!(done.pass_started_at.is_some());
    // Everything is answered from the cache now.
    let ids = [f.id("tone.wav"), f.id("silence.wav"), f.id("empty.wav")];
    let reply = batch(
        &f.state,
        &format!(r#"{{"ids":[{},{},{}]}}"#, ids[0], ids[1], ids[2]),
    )
    .await;
    assert!(reply.items.iter().all(|item| item.status == ItemStatus::Ok));

    // Shards live under waveforms/list/<library hash>/ and are replaced atomically:
    // no temporary file is left behind.
    f.state.list_waveforms().cache().flush_all();
    let dir = f.state.list_waveforms().cache().dir().to_path_buf();
    assert!(dir.starts_with(f.state.waveform_cache_dir().join("list")));
    let names: Vec<String> = fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["shard-0.bin".to_string()]);
    let after = status(&f.state).await;
    assert!(after.cache_bytes > 0);
    // The detail waveform cache trim never sees list shards.
    assert!(
        fs::read_dir(f.state.waveform_cache_dir())
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".bin"))
    );

    // Pausing is reported and does not touch the setting.
    let paused: ListWaveformStatus = ok(call(
        f.state.clone(),
        Method::POST,
        "/waveforms/pregeneration/pause",
        &[],
        LAN,
        None,
    )
    .await)
    .await;
    assert_eq!(paused.state, "paused");
    assert!(paused.enabled);
    let resumed: ListWaveformStatus = ok(call(
        f.state.clone(),
        Method::POST,
        "/waveforms/pregeneration/resume",
        &[],
        LAN,
        None,
    )
    .await)
    .await;
    assert_eq!(resumed.state, "idle");

    // Removing a record prunes its entry in the background.
    let state = f.state.clone();
    let removed = ids[1];
    let response = call(
        state.clone(),
        Method::POST,
        "/audio/remove",
        &[("x-audiolib-library-id", f.state.library_id())],
        LAN,
        Some(&format!(r#"{{"ids":[{removed}]}}"#)),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let current = status(&state).await;
        if current.cached == 2 {
            assert_eq!(current.total, 2);
            break;
        }
        assert!(Instant::now() < deadline, "{current:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn status_is_off_without_pregeneration() {
    let f = fixture("");
    let current = status(&f.state).await;
    assert!(!current.enabled);
    assert_eq!(current.state, "off");
    assert_eq!(current.remaining, None);
    assert_eq!(current.yield_reason, None);
    assert_eq!(current.total, 3);
    assert_eq!(current.pass_started_at, None);
}

#[tokio::test]
async fn settings_follow_config_admin_overrides_and_manage_mode() {
    let f = fixture("manage_mode = \"lan\"\n");
    let bearer = format!("Bearer {TOKEN}");
    let external: &[(&str, &str)] = &[("authorization", &bearer)];
    let settings = |value: &serde_json::Value| {
        (
            value["pregenerate_list_waveforms"].as_bool().unwrap(),
            value["pregenerate_list_waveforms_source"]
                .as_str()
                .unwrap()
                .to_string(),
            value["can_change"].as_bool().unwrap(),
        )
    };
    let get: serde_json::Value =
        ok(call(f.state.clone(), Method::GET, "/settings", &[], LAN, None).await).await;
    assert_eq!(settings(&get), (false, "default".into(), true));
    let get: serde_json::Value = ok(call(
        f.state.clone(),
        Method::GET,
        "/settings",
        external,
        PUBLIC,
        None,
    )
    .await)
    .await;
    assert_eq!(settings(&get), (false, "default".into(), false));

    // manage_mode = lan: external PATCH is forbidden, nothing changes.
    error(
        call(
            f.state.clone(),
            Method::PATCH,
            "/settings",
            external,
            PUBLIC,
            Some(r#"{"pregenerate_list_waveforms":true}"#),
        )
        .await,
        StatusCode::FORBIDDEN,
        "MANAGE_FORBIDDEN",
    )
    .await;
    for path in [
        "/waveforms/pregeneration/pause",
        "/waveforms/pregeneration/resume",
    ] {
        error(
            call(f.state.clone(), Method::POST, path, external, PUBLIC, None).await,
            StatusCode::FORBIDDEN,
            "MANAGE_FORBIDDEN",
        )
        .await;
    }
    // Unauthenticated external requests get 401 first.
    error(
        call(
            f.state.clone(),
            Method::PATCH,
            "/settings",
            &[],
            PUBLIC,
            Some(r#"{"pregenerate_list_waveforms":true}"#),
        )
        .await,
        StatusCode::UNAUTHORIZED,
        "UNAUTHORIZED",
    )
    .await;
    for bad in [
        r#"{"pregenerate_list_waveforms":"yes"}"#,
        r#"{"other":true}"#,
        "[]",
    ] {
        error(
            call(
                f.state.clone(),
                Method::PATCH,
                "/settings",
                &[],
                LAN,
                Some(bad),
            )
            .await,
            StatusCode::BAD_REQUEST,
            "VALIDATION",
        )
        .await;
    }
    assert!(!f.state.security().pregenerate_list_waveforms());

    // LAN PATCH writes admin-settings.json and takes effect at once.
    let patched: serde_json::Value = ok(call(
        f.state.clone(),
        Method::PATCH,
        "/settings",
        &[],
        LAN,
        Some(r#"{"pregenerate_list_waveforms":true}"#),
    )
    .await)
    .await;
    assert_eq!(settings(&patched), (true, "admin".into(), true));
    let saved: serde_json::Value = serde_json::from_slice(
        &fs::read(f.config.admin_dir().join("admin-settings.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(saved["pregenerate_list_waveforms"], true);
    let capabilities: serde_json::Value = ok(call(
        f.state.clone(),
        Method::GET,
        "/capabilities",
        &[],
        LAN,
        None,
    )
    .await)
    .await;
    assert_eq!(capabilities["pregenerate_list_waveforms"], true);
    assert!(status(&f.state).await.enabled);

    // `null` removes the override.
    let cleared: serde_json::Value = ok(call(
        f.state.clone(),
        Method::PATCH,
        "/settings",
        &[],
        LAN,
        Some(r#"{"pregenerate_list_waveforms":null}"#),
    )
    .await)
    .await;
    assert_eq!(settings(&cleared), (false, "default".into(), true));

    // The admin page accepts it too ("" removes the override).
    let admin = |body: &'static str| {
        let mut request = Request::builder()
            .method(Method::PUT)
            .uri("/admin/api/settings")
            .header("host", "127.0.0.1")
            .header("x-audiolib-admin", "1")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        request.extensions_mut().insert(peer("127.0.0.1:1"));
        router(f.state.clone()).oneshot(request)
    };
    let response = admin(r#"{"pregenerate_list_waveforms":true}"#)
        .await
        .unwrap();
    let status_body: serde_json::Value = ok(response).await;
    assert_eq!(status_body["pregenerate_list_waveforms"], true);
    assert_eq!(status_body["pregenerate_list_waveforms_source"], "admin");
    let response = admin(r#"{"pregenerate_list_waveforms":""}"#).await.unwrap();
    let status_body: serde_json::Value = ok(response).await;
    assert_eq!(status_body["pregenerate_list_waveforms"], false);
    assert_eq!(status_body["pregenerate_list_waveforms_source"], "default");
    let response = admin(r#"{"pregenerate_list_waveforms":3}"#).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn config_default_and_capabilities_fields() {
    let f = fixture("pregenerate_list_waveforms = true\n");
    let get: serde_json::Value =
        ok(call(f.state.clone(), Method::GET, "/settings", &[], LAN, None).await).await;
    assert_eq!(get["pregenerate_list_waveforms"], true);
    assert_eq!(get["pregenerate_list_waveforms_source"], "config");
    let capabilities: serde_json::Value = ok(call(
        f.state.clone(),
        Method::GET,
        "/capabilities",
        &[],
        LAN,
        None,
    )
    .await)
    .await;
    assert_eq!(
        capabilities["list_waveforms"],
        serde_json::json!({
            "version": 1,
            "buckets": [128],
            "max_ids": 200,
            "encoding": "i16le-minmax-base64",
            "encodings": ["i16le-minmax-base64", "i8-minmax-base64"],
            "bundle": true
        })
    );
    assert_eq!(capabilities["pregenerate_list_waveforms"], true);
    // /settings answers while the library is still opening.
    let starting = Service::starting(BearerToken::new(TOKEN).unwrap());
    let bearer = format!("Bearer {TOKEN}");
    let response = call(
        starting,
        Method::GET,
        "/settings",
        &[("authorization", &bearer)],
        LAN,
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[test]
fn list_waveform_config_is_validated() {
    let base = "library_db = 'lib.db'\n";
    let config = ServerConfig::from_toml_str(base, Path::new(".")).unwrap();
    assert_eq!(config.pregenerate_list_waveforms, None);
    assert_eq!(config.list_waveform_cache_mb, 640);
    assert_eq!(config.list_waveform_workers, 1);
    assert_eq!(config.list_waveform_rate, 10);
    assert_eq!(config.effective_list_waveform_workers(), 1);
    let many = ServerConfig::from_toml_str(
        &format!("{base}list_waveform_workers = 64\nmax_waveforms = 4\n"),
        Path::new("."),
    )
    .unwrap();
    assert_eq!(many.effective_list_waveform_workers(), 3);
    let single = ServerConfig::from_toml_str(
        &format!("{base}list_waveform_workers = 8\nmax_waveforms = 1\n"),
        Path::new("."),
    )
    .unwrap();
    assert_eq!(single.effective_list_waveform_workers(), 1);
    for bad in [
        "list_waveform_cache_mb = 15",
        "list_waveform_workers = 0",
        "list_waveform_workers = 65",
        "list_waveform_rate = 0",
        "list_waveform_rate = 1001",
        "pregenerate_list_waveforms = \"yes\"",
    ] {
        assert!(
            ServerConfig::from_toml_str(&format!("{base}{bad}\n"), Path::new(".")).is_err(),
            "{bad}"
        );
    }
}

#[tokio::test]
async fn scans_queue_new_records_for_pregeneration() {
    let f = fixture("pregenerate_list_waveforms = true\nlist_waveform_rate = 1000\n");
    let wait_cached = |expected: u64| {
        let state = f.state.clone();
        async move {
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                let current = status(&state).await;
                if current.cached == expected && current.state == "idle" {
                    return current;
                }
                assert!(Instant::now() < deadline, "{current:?}");
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    };
    let first = wait_cached(3).await;
    wav(&f.sounds.join("new.wav"), 300, |i| (i as i16) * 10);
    let library_id = f.state.library_id().to_string();
    let started = call(
        f.state.clone(),
        Method::POST,
        "/scans",
        &[("x-audiolib-library-id", &library_id)],
        LAN,
        Some(r#"{"root":"SFX"}"#),
    )
    .await;
    assert_eq!(started.status(), StatusCode::OK);
    let deadline = Instant::now() + Duration::from_secs(30);
    while f.state.scans().current().is_some() {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let after = wait_cached(4).await;
    assert_eq!(after.total, 4);
    assert_eq!(after.pass_started_at, first.pass_started_at, "same pass");
    let id = f.id("new.wav");
    let reply = batch(&f.state, &format!(r#"{{"ids":[{id}]}}"#)).await;
    assert_eq!(reply.items[0].status, ItemStatus::Ok);
    assert_eq!(reply.items[0].n, Some(100));
}

#[tokio::test]
async fn on_demand_queue_yields_during_the_prebuffer_window_only() {
    let f = fixture("");
    let tone = f.id("tone.wav");
    let body = format!(r#"{{"ids":[{tone}]}}"#);
    // A stream/preview just started: nothing is decoded for 3 seconds.
    f.state.list_waveforms().note_playback_start();
    let started = Instant::now();
    assert_eq!(
        batch(&f.state, &body).await.items[0].status,
        ItemStatus::Pending
    );
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(
        batch(&f.state, &body).await.items[0].status,
        ItemStatus::Pending
    );
    // Afterwards the queue resumes (no stream is actually active here).
    let reply = settled(&f.state, &body).await;
    assert_eq!(reply.items[0].status, ItemStatus::Ok);
    assert!(started.elapsed() >= Duration::from_secs(3));
}

#[tokio::test]
async fn slow_metadata_reads_continue_in_the_background_after_the_budget() {
    let f = fixture("");
    let tone = f.id("tone.wav");
    let silence = f.id("silence.wav");
    let body = format!(r#"{{"ids":[{tone},{silence}]}}"#);
    let list = f.state.list_waveforms().clone();
    // Every metadata read outlasts the 2 s budget.
    f.state.set_stat_delay(Duration::from_millis(3000));

    let started = Instant::now();
    let first = batch(&f.state, &body).await;
    let took = started.elapsed();
    assert!(
        took < Duration::from_millis(2900),
        "answered within the budget: {took:?}"
    );
    assert!(
        first
            .items
            .iter()
            .all(|item| item.status == ItemStatus::Pending),
        "{first:?}"
    );
    // No read finished in the budget: no progress to estimate from.
    assert_eq!(first.retry_after, Some(5));
    assert_eq!(
        list.outstanding_stats(),
        2,
        "reads continue after the reply"
    );

    // A repeat while they run starts no new reads and answers at once.
    let started = Instant::now();
    let again = batch(&f.state, &body).await;
    assert!(
        started.elapsed() < Duration::from_millis(1000),
        "{:?}",
        started.elapsed()
    );
    assert!(
        again
            .items
            .iter()
            .all(|item| item.status == ItemStatus::Pending),
        "{again:?}"
    );
    assert!(list.outstanding_stats() <= 2);

    f.state.set_stat_delay(Duration::ZERO);
    let deadline = Instant::now() + Duration::from_secs(10);
    while list.outstanding_stats() > 0 {
        assert!(Instant::now() < deadline, "background reads never finished");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let reply = settled(&f.state, &body).await;
    assert!(
        reply.items.iter().all(|item| item.status == ItemStatus::Ok),
        "{reply:?}"
    );
}

#[test]
fn stat_pause_on_playback_is_off_by_default_and_a_bool() {
    let base = "library_db = 'lib.db'\n";
    let config = ServerConfig::from_toml_str(base, Path::new(".")).unwrap();
    assert!(!config.list_waveform_pause_stats_on_playback);
    let on = ServerConfig::from_toml_str(
        &format!("{base}list_waveform_pause_stats_on_playback = true\n"),
        Path::new("."),
    )
    .unwrap();
    assert!(on.list_waveform_pause_stats_on_playback);
    assert!(
        ServerConfig::from_toml_str(
            &format!("{base}list_waveform_pause_stats_on_playback = \"yes\"\n"),
            Path::new("."),
        )
        .is_err()
    );
}

#[tokio::test]
async fn stat_pause_on_playback_answers_pending_during_the_prebuffer_window() {
    for pause in [false, true] {
        let f = fixture(if pause {
            "list_waveform_pause_stats_on_playback = true\n"
        } else {
            ""
        });
        let tone = f.id("tone.wav");
        let silence = f.id("silence.wav");
        let body = format!(r#"{{"ids":[{tone},{silence}]}}"#);
        let warm = settled(&f.state, &body).await;
        assert!(warm.items.iter().all(|item| item.status == ItemStatus::Ok));

        // A stream/preview starts (HEAD would not call this).
        f.state.list_waveforms().note_playback_start();
        let reply = batch(&f.state, &body).await;
        if !pause {
            assert!(
                reply.items.iter().all(|item| item.status == ItemStatus::Ok),
                "off: metadata reads go on during playback: {reply:?}"
            );
            assert_eq!(reply.retry_after, None);
            continue;
        }
        assert!(
            reply
                .items
                .iter()
                .all(|item| item.status == ItemStatus::Pending),
            "on: no metadata reads in the prebuffer window: {reply:?}"
        );
        let retry = reply.retry_after.expect("retry_after while paused");
        assert!((1..=3).contains(&retry), "covers the window: {retry}");
        assert_eq!(f.state.list_waveforms().outstanding_stats(), 0);

        tokio::time::sleep(Duration::from_millis(3100)).await;
        let after = batch(&f.state, &body).await;
        assert!(
            after.items.iter().all(|item| item.status == ItemStatus::Ok),
            "after the window: {after:?}"
        );
    }
}

#[tokio::test]
async fn i8_encoding_etags_lengths_waveforms_db_and_bundle() {
    let f = fixture("");
    let tone = f.id("tone.wav");
    let silence = f.id("silence.wav");
    let empty = f.id("empty.wav");
    let ids = format!("[{tone},{silence},{empty}]");
    let i16 = settled(&f.state, &format!(r#"{{"ids":{ids}}}"#)).await;
    assert_eq!(i16.encoding, "i16le-minmax-base64");
    let i8 = settled(
        &f.state,
        &format!(r#"{{"ids":{ids},"encoding":"i8-minmax-base64"}}"#),
    )
    .await;
    assert_eq!(i8.encoding, "i8-minmax-base64");
    for (a, b) in i16.items.iter().zip(&i8.items) {
        assert_eq!(b.status, ItemStatus::Ok, "{b:?}");
        let (ea, eb) = (a.etag.clone().unwrap(), b.etag.clone().unwrap());
        assert!(ea.ends_with("-l1\""), "{ea}");
        assert_eq!(eb, format!("{}i8\"", ea.trim_end_matches('"')));
        let n = b.n.unwrap();
        assert_eq!(a.n, b.n);
        let peaks = b.peaks.as_deref().unwrap();
        assert_eq!(peaks.len(), audiolib_server::wfdb::base64_len(n));
        let raw = decode_base64(peaks);
        assert_eq!(raw.len(), 2 * n);
        let expected: Vec<u8> = values(a.peaks.as_deref().unwrap())
            .iter()
            .map(|v| audiolib_server::wfdb::i16_to_i8(*v) as u8)
            .collect();
        assert_eq!(raw, expected);
        assert!(raw.iter().all(|b| *b != 0x80), "-128 emitted");
        for pair in raw.chunks_exact(2) {
            assert!((pair[0] as i8) <= (pair[1] as i8));
        }
    }
    assert_eq!(i8.items[2].n, Some(0));
    assert_eq!(i8.items[2].peaks.as_deref(), Some(""));
    assert_eq!(i8.items[0].n, Some(125));
    // tone.wav: +16384 → 64 (half away from zero would matter only at exact ties).
    let tone_bytes = decode_base64(i8.items[0].peaks.as_deref().unwrap());
    assert_eq!(tone_bytes.iter().map(|b| *b as i8).max(), Some(64));

    // known with the i16 tag while asking for i8: ok with the -l1i8 tag.
    let l1 = i16.items[0].etag.clone().unwrap();
    let l1i8 = i8.items[0].etag.clone().unwrap();
    let known = |etag: &str| {
        format!(
            r#"{{"ids":[{tone}],"encoding":"i8-minmax-base64","known":{{"{tone}":{}}}}}"#,
            serde_json::to_string(etag).unwrap()
        )
    };
    let reply = batch(&f.state, &known(&l1)).await;
    assert_eq!(reply.items[0].status, ItemStatus::Ok);
    assert_eq!(reply.items[0].etag.as_deref(), Some(l1i8.as_str()));
    let reply = batch(&f.state, &known(&l1i8)).await;
    assert_eq!(reply.items[0].status, ItemStatus::NotModified);
    // The i8 tag as known for an i16 request is not a match either.
    let reply = batch(
        &f.state,
        &format!(
            r#"{{"ids":[{tone}],"known":{{"{tone}":{}}}}}"#,
            serde_json::to_string(&l1i8).unwrap()
        ),
    )
    .await;
    assert_eq!(reply.items[0].status, ItemStatus::Ok);

    // Unknown encodings: 400 VALIDATION / UNSUPPORTED_ENCODING; a non-string
    // encoding is a plain VALIDATION without sub_code.
    for (body, sub_code) in [
        (
            format!(r#"{{"ids":[{tone}],"encoding":"u8"}}"#),
            Some("UNSUPPORTED_ENCODING"),
        ),
        (
            format!(r#"{{"ids":[{tone}],"encoding":"../x"}}"#),
            Some("UNSUPPORTED_ENCODING"),
        ),
        (format!(r#"{{"ids":[{tone}],"encoding":5}}"#), None),
    ] {
        let response = call(
            f.state.clone(),
            Method::POST,
            "/waveforms",
            &[],
            LAN,
            Some(&body),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{body}");
        let reply: ErrorBody = serde_json::from_slice(&bytes(response).await).unwrap();
        assert_eq!(reply.error_code, "VALIDATION", "{body}");
        assert_eq!(reply.sub_code.as_deref(), sub_code, "{body}");
    }

    // waveforms.db holds one 256-byte row per record with the -l1i8 tag.
    let wfdb = f.state.list_waveforms().wfdb().unwrap();
    wfdb.flush();
    assert_eq!(wfdb.count(), 3);
    let row = wfdb.get(tone).unwrap();
    assert_eq!(row.etag, l1i8);
    assert_eq!(row.blob.len(), 256);
    assert_eq!(row.n, 125);
    assert!(row.src_size > 0 && row.src_mtime > 0);

    // Bundle: validated SQLite snapshot with meta; bad encodings rejected.
    let response = call(
        f.state.clone(),
        Method::GET,
        "/waveforms/bundle?encoding=i16le-minmax-base64",
        &[],
        LAN,
        None,
    )
    .await;
    error(response, StatusCode::BAD_REQUEST, "VALIDATION").await;
    for query in [
        "encoding=../x",
        "encoding=%2e%2e%2fwaveforms.db",
        "encoding=",
    ] {
        let response = call(
            f.state.clone(),
            Method::GET,
            &format!("/waveforms/bundle?{query}"),
            &[],
            LAN,
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{query}");
        let reply: ErrorBody = serde_json::from_slice(&bytes(response).await).unwrap();
        assert_eq!(
            reply.sub_code.as_deref(),
            Some("UNSUPPORTED_ENCODING"),
            "{query}"
        );
    }
    // Without a token an external client gets 401.
    let response = call(
        f.state.clone(),
        Method::GET,
        "/waveforms/bundle?encoding=i8-minmax-base64",
        &[],
        "203.0.113.9:50000",
        None,
    )
    .await;
    error(response, StatusCode::UNAUTHORIZED, "UNAUTHORIZED").await;
    // A second bundle while one is in progress: 503 BUSY + Retry-After.
    {
        let _held = audiolib_server::wfdb::BUNDLE_SLOT.try_acquire().unwrap();
        let response = call(
            f.state.clone(),
            Method::GET,
            "/waveforms/bundle?encoding=i8-minmax-base64",
            &[],
            LAN,
            None,
        )
        .await;
        assert_eq!(response.headers()["retry-after"], "5");
        error(response, StatusCode::SERVICE_UNAVAILABLE, "BUSY").await;
    }
    let response = call(
        f.state.clone(),
        Method::GET,
        "/waveforms/bundle?encoding=i8-minmax-base64",
        &[],
        LAN,
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["content-type"],
        "application/vnd.sqlite3"
    );
    assert_eq!(response.headers()["x-library-id"], f.state.library_id());
    assert!(response.headers().contains_key("x-library-revision"));
    let bundle_etag = response.headers()["etag"].to_str().unwrap().to_string();
    assert!(bundle_etag.starts_with("\"wfb1-"), "{bundle_etag}");
    let body = bytes(response).await;
    let out = tempfile::tempdir().unwrap();
    let path = out.path().join("b.db");
    std::fs::write(&path, &body).unwrap();
    audiolib_server::wfdb::validate_bundle(&path).unwrap();
    assert_eq!(
        audiolib_server::wfdb::file_etag(&path).unwrap(),
        bundle_etag
    );
    let conn = rusqlite::Connection::open(&path).unwrap();
    let meta = |key: &str| -> String {
        conn.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .unwrap()
    };
    assert_eq!(meta("library_id"), f.state.library_id());
    assert_eq!(meta("encoding"), "i8-minmax-base64");
    assert!(meta("generated_at").parse::<u64>().unwrap() > 0);
    meta("revision").parse::<u64>().unwrap();
    // The temporary bundle directory is gone once the body was sent.
    let leftovers = std::fs::read_dir(wfdb.path().parent().unwrap())
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("audiolib-wfbundle-")
        })
        .count();
    assert_eq!(leftovers, 0);
}

/// `GET /waveforms/bundle` (status, has `Retry-After`, body), retrying while another
/// test holds the process-wide bundle slot (503 `BUSY`).
async fn bundle_response(state: &AppState) -> (StatusCode, bool, Vec<u8>) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let response = call(
            state.clone(),
            Method::GET,
            "/waveforms/bundle?encoding=i8-minmax-base64",
            &[],
            LAN,
            None,
        )
        .await;
        let status = response.status();
        let retry = response.headers().contains_key("retry-after");
        let body = bytes(response).await;
        let busy = status == StatusCode::SERVICE_UNAVAILABLE
            && serde_json::from_slice::<ErrorBody>(&body).is_ok_and(|b| b.error_code == "BUSY");
        if !busy || Instant::now() > deadline {
            return (status, retry, body);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn first_bundle_creates_a_missing_cache_directory() {
    // §9.5.8: the first bundle request used to fail with 500 IO (ENOENT) when the
    // library's list cache directory did not exist yet.
    let f = fixture("");
    let dir = f
        .state
        .list_waveforms()
        .wfdb()
        .unwrap()
        .path()
        .parent()
        .unwrap()
        .to_path_buf();
    // Nothing was generated yet: the directory does not exist (or only holds an
    // empty waveforms.db); make sure it is gone.
    let _ = fs::remove_dir_all(&dir);
    assert!(!dir.exists());
    let (status, _, body) = bundle_response(&f.state).await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    assert!(dir.is_dir());
}

#[tokio::test]
async fn bundle_is_unavailable_when_the_cache_directory_cannot_be_created() {
    let f = fixture("");
    let dir = f
        .state
        .list_waveforms()
        .wfdb()
        .unwrap()
        .path()
        .parent()
        .unwrap()
        .to_path_buf();
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.parent().unwrap()).unwrap();
    // A file where the directory should be: create_dir_all fails.
    fs::write(&dir, b"not a directory").unwrap();
    let (status, retry, body) = bundle_response(&f.state).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(retry);
    let body: ErrorBody = serde_json::from_slice(&body).unwrap();
    assert_eq!(body.error_code, "UNAVAILABLE");
    assert!(!body.message.contains('/'), "{}", body.message);
}

#[tokio::test]
async fn list_waveform_rate_setting_follows_config_admin_and_manage_mode() {
    let f = fixture("manage_mode = \"lan\"\nlist_waveform_rate = 25\n");
    let bearer = format!("Bearer {TOKEN}");
    let external: &[(&str, &str)] = &[("authorization", &bearer)];
    let rate = |v: &serde_json::Value| {
        (
            v["list_waveform_rate"].as_u64().unwrap(),
            v["list_waveform_rate_source"].as_str().unwrap().to_string(),
        )
    };
    let get: serde_json::Value =
        ok(call(f.state.clone(), Method::GET, "/settings", &[], LAN, None).await).await;
    assert_eq!(rate(&get), (25, "config".into()));
    assert_eq!(status(&f.state).await.rate, 25);

    // External PATCH is forbidden under manage_mode = lan.
    error(
        call(
            f.state.clone(),
            Method::PATCH,
            "/settings",
            external,
            PUBLIC,
            Some(r#"{"list_waveform_rate":50}"#),
        )
        .await,
        StatusCode::FORBIDDEN,
        "MANAGE_FORBIDDEN",
    )
    .await;
    for bad in [
        r#"{"list_waveform_rate":0}"#,
        r#"{"list_waveform_rate":1001}"#,
        r#"{"list_waveform_rate":-1}"#,
        r#"{"list_waveform_rate":2.5}"#,
        r#"{"list_waveform_rate":"10"}"#,
    ] {
        error(
            call(
                f.state.clone(),
                Method::PATCH,
                "/settings",
                &[],
                LAN,
                Some(bad),
            )
            .await,
            StatusCode::BAD_REQUEST,
            "VALIDATION",
        )
        .await;
    }
    let patched: serde_json::Value = ok(call(
        f.state.clone(),
        Method::PATCH,
        "/settings",
        &[],
        LAN,
        Some(r#"{"list_waveform_rate":1000}"#),
    )
    .await)
    .await;
    assert_eq!(rate(&patched), (1000, "admin".into()));
    // pregenerate_list_waveforms is untouched by a rate-only PATCH.
    assert_eq!(patched["pregenerate_list_waveforms"], false);
    assert_eq!(status(&f.state).await.rate, 1000);
    // null removes the override: back to config.toml.
    let reset: serde_json::Value = ok(call(
        f.state.clone(),
        Method::PATCH,
        "/settings",
        &[],
        LAN,
        Some(r#"{"list_waveform_rate":null}"#),
    )
    .await)
    .await;
    assert_eq!(rate(&reset), (25, "config".into()));
    // Persisted in admin-settings.json across a reload.
    let _: serde_json::Value = ok(call(
        f.state.clone(),
        Method::PATCH,
        "/settings",
        &[],
        LAN,
        Some(r#"{"list_waveform_rate":7}"#),
    )
    .await)
    .await;
    let reloaded = audiolib_server::Security::from_config(&f.config, None);
    assert_eq!(
        reloaded.list_waveform_rate_with_source(),
        (7, audiolib_server::auth::Source::Admin)
    );
}

#[tokio::test]
async fn list_waveform_rate_defaults_to_ten() {
    let f = fixture("");
    let get: serde_json::Value =
        ok(call(f.state.clone(), Method::GET, "/settings", &[], LAN, None).await).await;
    assert_eq!(get["list_waveform_rate"], 10);
    assert_eq!(get["list_waveform_rate_source"], "default");
}
