// server/audiolib-server/tests/waveform_jobs.rs
//! List waveform cache management (doc/api/README.md §9.5): `POST /waveforms/jobs`,
//! `jobs/current/pause|resume|stop`, the old pre-generation aliases, the extended
//! `GET /waveforms/status`, `job-state.json` persistence and `/capabilities`, against
//! temporary libraries of generated WAVs (in-process, no listening server).
use audiolib_db::{ScanCancellation, ScanOptions, ScanService, SqliteProvider};
use audiolib_server::{
    AppState, BearerToken, ConnectionInfo, ErrorBody, Listener, ServerConfig, Service,
    list_waveform::ListWaveformStatus, router, wf_job,
};
use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
    response::Response,
};
use http_body_util::BodyExt;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
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
    // One list waveform worker: these tests pace a job file by file (`list_waveform_rate`)
    // and read its cursor and progress between files.
    let config = ServerConfig::from_toml_str(
        &format!(
            "bind = \"127.0.0.1:0\"\nlibrary_db = 'library/library.db'\nsnapshot_dir = 'snapshots'\nlist_waveform_workers = 1\n{extra}\
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
    let mut request = Request::builder()
        .method(method)
        .uri(format!("/api/v1{path}"));
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

async fn err(response: Response, status: StatusCode, code: &str) -> ErrorBody {
    assert_eq!(response.status(), status);
    let body: ErrorBody = serde_json::from_slice(&bytes(response).await).unwrap();
    assert_eq!(body.error_code, code, "{}", body.message);
    body
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

async fn status_json(state: &AppState) -> serde_json::Value {
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

async fn post(state: &AppState, path: &str, body: Option<&str>) -> Response {
    call(state.clone(), Method::POST, path, &[], LAN, body).await
}

async fn start(state: &AppState, body: &str) -> ListWaveformStatus {
    let response = post(state, "/waveforms/jobs", Some(body)).await;
    let code = response.status();
    let bytes = bytes(response).await;
    assert_eq!(
        code,
        StatusCode::ACCEPTED,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    serde_json::from_slice(&bytes).unwrap()
}

async fn control(state: &AppState, action: &str) -> ListWaveformStatus {
    ok(post(state, &format!("/waveforms/jobs/current/{action}"), None).await).await
}

/// Poll the status until the job has ended (or `check` holds).
async fn wait_for(
    state: &AppState,
    what: &str,
    check: impl Fn(&ListWaveformStatus) -> bool,
) -> ListWaveformStatus {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let current = status(state).await;
        if check(&current) {
            return current;
        }
        assert!(Instant::now() < deadline, "{what}: {current:?}");
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
}

async fn finished(state: &AppState) -> ListWaveformStatus {
    wait_for(state, "job end", |s| {
        s.job.as_ref().is_some_and(|job| job.finished_at.is_some())
    })
    .await
}

fn state_dir(state: &AppState) -> PathBuf {
    state.list_waveforms().cache().dir().to_path_buf()
}

/// Poll `job-state.json` until `check` holds.
async fn state_file_where(
    state: &AppState,
    check: impl Fn(&serde_json::Value) -> bool,
) -> serde_json::Value {
    let path = state_dir(state).join("job-state.json");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(bytes) = fs::read(&path) {
            let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            if check(&value) {
                return value;
            }
            assert!(Instant::now() < deadline, "{value}");
        }
        assert!(Instant::now() < deadline, "no state file");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn reopen(f: &Fixture) -> AppState {
    f.state.list_waveforms().shutdown();
    AppState::open(&f.config, BearerToken::new(TOKEN).unwrap()).unwrap()
}

#[tokio::test]
async fn capabilities_advertise_jobs_and_status_keeps_old_fields() {
    let f = fixture("");
    let caps: serde_json::Value = ok(call(
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
        caps["list_waveform_jobs"],
        serde_json::json!({"version": 1, "modes": ["missing", "rebuild", "failed"], "rate_setting": true})
    );
    let value = status_json(&f.state).await;
    for key in [
        "enabled",
        "state",
        "yield_reason",
        "total",
        "cached",
        "remaining",
        "queued_on_demand",
        "failed",
        "skipped_offline",
        "rate",
        "cache_bytes",
        "cache_limit_bytes",
        "over_budget",
        "pass_started_at",
        "pass_finished_at",
        "pending",
        "run_state",
        "current_rate",
        "eta_secs",
        "last_error",
        "job",
    ] {
        assert!(value.get(key).is_some(), "{key}: {value}");
    }
    assert_eq!(value["state"], "off");
    assert_eq!(value["run_state"], "idle");
    assert_eq!(value["job"], serde_json::Value::Null);
    assert_eq!(value["eta_secs"], serde_json::Value::Null);
    assert_eq!(value["current_rate"], 0.0);
}

#[tokio::test]
async fn missing_job_fills_the_cache_then_skips_up_to_date_entries() {
    let f = fixture("list_waveform_rate = 1000\n");
    let started = start(&f.state, r#"{"mode":"missing"}"#).await;
    let job = started.job.clone().unwrap();
    assert_eq!(job.mode, wf_job::JobMode::Missing);
    assert_eq!(job.mode_effective, wf_job::JobMode::Missing);
    assert!(job.id.starts_with("wfj-"), "{}", job.id);
    assert_eq!(job.total, 3);
    assert!(!job.resumed);
    let done = finished(&f.state).await;
    let job = done.job.unwrap();
    assert_eq!(job.state, "completed");
    assert_eq!(
        (job.total, job.done, job.generated, job.skipped, job.failed),
        (3, 3, 3, 0, 0)
    );
    assert!(job.current.is_none());
    assert_eq!(done.cached, 3);
    assert_eq!(done.run_state, "idle");
    assert_eq!(done.eta_secs, None);
    // The state file holds the finished job (written right after the last record).
    let file = state_file_where(&f.state, |file| file["job"]["state"] == "completed").await;
    assert_eq!(file["version"], 1);
    assert_eq!(file["library_id"], f.state.library_id());
    assert_eq!(file["job"]["state"], "completed");
    assert_eq!(file["job"]["done"], 3);

    // A second job gets a new id and only reads metadata.
    let again = start(&f.state, r#"{"mode":"missing","confirm":"ignored"}"#).await;
    assert_ne!(again.job.as_ref().unwrap().id, job.id);
    let job2 = finished(&f.state).await.job.unwrap();
    assert_eq!((job2.done, job2.generated, job2.skipped), (3, 0, 3));
}

#[tokio::test]
async fn one_job_at_a_time_and_pause_resume_stop() {
    let f = fixture("list_waveform_rate = 1\n");
    // Nothing to control yet.
    for action in ["pause", "resume", "stop"] {
        err(
            post(&f.state, &format!("/waveforms/jobs/current/{action}"), None).await,
            StatusCode::CONFLICT,
            "JOB_STATE",
        )
        .await;
    }
    // Keep a real stream response body unconsumed: its owned stream-slot permit
    // makes cache workers yield until we explicitly drop it. This pins the job's
    // running window across pause/resume/stop even if the test is descheduled for
    // longer than the entire three-record job would otherwise take.
    let playback = call(
        f.state.clone(),
        Method::GET,
        &format!("/audio/{}/stream", f.id("tone.wav")),
        &[],
        LAN,
        None,
    )
    .await;
    assert_eq!(playback.status(), StatusCode::OK);
    assert!(f.state.playback_active());
    let started = start(&f.state, r#"{"mode":"missing"}"#).await;
    assert_eq!(started.state, "yielding");
    assert_eq!(started.run_state, "running");
    assert_eq!(started.job.as_ref().unwrap().done, 0);
    let id = started.job.unwrap().id;
    let paused = control(&f.state, "pause").await;
    assert_eq!(paused.run_state, "paused");
    assert_eq!(paused.state, "paused");
    assert_eq!(paused.job.as_ref().unwrap().state, "paused");
    assert_eq!(paused.current_rate, 0.0);
    assert_eq!(paused.eta_secs, None);
    // Idempotent.
    assert_eq!(control(&f.state, "pause").await.run_state, "paused");
    // A second job is refused with the current job's id, paused or not.
    let busy = err(
        post(
            &f.state,
            "/waveforms/jobs",
            Some(r#"{"mode":"rebuild","confirm":"rebuild"}"#),
        )
        .await,
        StatusCode::CONFLICT,
        "BUSY",
    )
    .await;
    assert_eq!(busy.job_id.as_deref(), Some(id.as_str()));
    // The bundle stays usable during a job.
    let response = call(
        f.state.clone(),
        Method::GET,
        "/waveforms/bundle?encoding=i8-minmax-base64",
        &[],
        LAN,
        None,
    )
    .await;
    assert!(
        response.status() == StatusCode::OK || response.status() == StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        response.status()
    );
    if response.status() == StatusCode::SERVICE_UNAVAILABLE {
        // Only the process-wide single-bundle slot may refuse it (another test).
        err(response, StatusCode::SERVICE_UNAVAILABLE, "BUSY").await;
    } else {
        let _ = bytes(response).await;
    }
    // Nothing is processed while paused.
    let before = status(&f.state).await.job.unwrap().done;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(status(&f.state).await.job.unwrap().done, before);
    let resumed = control(&f.state, "resume").await;
    assert_ne!(resumed.run_state, "paused");
    assert_eq!(control(&f.state, "resume").await.job.unwrap().id, id);
    let stopped = control(&f.state, "stop").await;
    let job = stopped.job.unwrap();
    assert_eq!(job.state, "cancelled");
    assert!(job.finished_at.is_some());
    err(
        post(&f.state, "/waveforms/jobs/current/stop", None).await,
        StatusCode::CONFLICT,
        "JOB_STATE",
    )
    .await;
    drop(playback);
    assert!(!f.state.playback_active(), "stream permit must be released");
}

#[tokio::test]
async fn stop_keeps_generated_entries_and_cancels() {
    // 1 file per second: the stop arrives long before the third file.
    let f = fixture("list_waveform_rate = 1\n");
    start(&f.state, r#"{"mode":"missing"}"#).await;
    wait_for(&f.state, "first file", |s| {
        s.job.as_ref().unwrap().done >= 1
    })
    .await;
    let stopped = control(&f.state, "stop").await;
    let job = stopped.job.unwrap();
    assert_eq!(job.state, "cancelled");
    let after = wait_for(&f.state, "in-flight drained", |s| {
        s.job.as_ref().unwrap().current.is_none()
    })
    .await;
    assert!(after.job.as_ref().unwrap().done < 3, "{after:?}");
    assert_eq!(after.cached, after.job.as_ref().unwrap().generated);
    assert_eq!(after.run_state, "idle");
    // A new job may start after a stopped one.
    start(&f.state, r#"{"mode":"missing"}"#).await;
}

#[tokio::test]
async fn running_job_reports_rate_eta_and_current_item() {
    let f = fixture("list_waveform_rate = 2\n");
    let started = start(&f.state, r#"{"mode":"missing"}"#).await;
    assert_eq!(started.run_state, "running");
    // The first file may already be done when the start response is built.
    assert_eq!(
        started.pending,
        Some(3 - started.job.as_ref().unwrap().done),
        "{started:?}"
    );
    let s = wait_for(&f.state, "rate measured", |s| {
        s.job.as_ref().unwrap().done >= 1 && s.job.as_ref().unwrap().finished_at.is_none()
    })
    .await;
    assert!(s.current_rate > 0.0, "{s:?}");
    assert_eq!(s.pending, Some(3 - s.job.as_ref().unwrap().done));
    assert!(s.eta_secs.is_some(), "{s:?}");
    assert_eq!(s.pass_started_at, Some(s.job.as_ref().unwrap().started_at));
    finished(&f.state).await;
}

#[tokio::test]
async fn job_requests_are_validated() {
    let f = fixture("");
    for (body, sub_code) in [
        ("{}", None),
        (r#"{"mode":5}"#, None),
        (r#"{"mode":"all"}"#, Some("UNSUPPORTED_MODE")),
        (r#"{"mode":"missing","other":1}"#, None),
        ("[]", None),
        (r#"{"mode":"rebuild"}"#, Some("CONFIRM_REQUIRED")),
        (
            r#"{"mode":"rebuild","confirm":"yes"}"#,
            Some("CONFIRM_REQUIRED"),
        ),
        (
            r#"{"mode":"rebuild","confirm":true}"#,
            Some("CONFIRM_REQUIRED"),
        ),
    ] {
        let reply = err(
            post(&f.state, "/waveforms/jobs", Some(body)).await,
            StatusCode::BAD_REQUEST,
            "VALIDATION",
        )
        .await;
        assert_eq!(reply.sub_code.as_deref(), sub_code, "{body}");
    }
    err(
        call(
            f.state.clone(),
            Method::POST,
            "/waveforms/jobs",
            &[("content-type", "text/plain")],
            LAN,
            Some(r#"{"mode":"missing"}"#),
        )
        .await,
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "JSON_CONTENT_TYPE_REQUIRED",
    )
    .await;
    assert!(status(&f.state).await.job.is_none());
}

#[tokio::test]
async fn mutating_job_routes_follow_manage_mode() {
    let f = fixture("manage_mode = \"lan\"\n");
    let bearer = format!("Bearer {TOKEN}");
    let external: &[(&str, &str)] = &[("authorization", &bearer)];
    for (path, body) in [
        ("/waveforms/jobs", Some(r#"{"mode":"missing"}"#)),
        ("/waveforms/jobs/current/pause", None),
        ("/waveforms/jobs/current/resume", None),
        ("/waveforms/jobs/current/stop", None),
    ] {
        err(
            call(f.state.clone(), Method::POST, path, external, PUBLIC, body).await,
            StatusCode::FORBIDDEN,
            "MANAGE_FORBIDDEN",
        )
        .await;
        // Authentication comes first.
        err(
            call(f.state.clone(), Method::POST, path, &[], PUBLIC, body).await,
            StatusCode::UNAUTHORIZED,
            "UNAUTHORIZED",
        )
        .await;
    }
    // Status is not gated.
    let s: ListWaveformStatus = ok(call(
        f.state.clone(),
        Method::GET,
        "/waveforms/status",
        external,
        PUBLIC,
        None,
    )
    .await)
    .await;
    assert!(s.job.is_none());
    let f = fixture("manage_mode = \"off\"\n");
    err(
        post(&f.state, "/waveforms/jobs", Some(r#"{"mode":"missing"}"#)).await,
        StatusCode::FORBIDDEN,
        "MANAGE_FORBIDDEN",
    )
    .await;
}

#[tokio::test]
async fn pregeneration_aliases_act_on_the_job() {
    let f = fixture("list_waveform_rate = 1\n");
    start(&f.state, r#"{"mode":"missing"}"#).await;
    let paused: ListWaveformStatus =
        ok(post(&f.state, "/waveforms/pregeneration/pause", None).await).await;
    assert_eq!(paused.job.as_ref().unwrap().state, "paused");
    assert_eq!(paused.run_state, "paused");
    let resumed: ListWaveformStatus =
        ok(post(&f.state, "/waveforms/pregeneration/resume", None).await).await;
    assert_ne!(resumed.job.as_ref().unwrap().state, "paused");
    control(&f.state, "stop").await;
    // Without a job they still pause pre-generation (old behaviour), never an error.
    let _: ListWaveformStatus =
        ok(post(&f.state, "/waveforms/pregeneration/pause", None).await).await;
    let _: ListWaveformStatus =
        ok(post(&f.state, "/waveforms/pregeneration/resume", None).await).await;
}

#[tokio::test]
async fn rebuild_clears_and_regenerates_everything() {
    let f = fixture("list_waveform_rate = 1000\n");
    start(&f.state, r#"{"mode":"missing"}"#).await;
    finished(&f.state).await;
    assert_eq!(status(&f.state).await.cached, 3);
    let started = start(&f.state, r#"{"mode":"rebuild","confirm":"rebuild"}"#).await;
    assert_eq!(started.job.as_ref().unwrap().mode, wf_job::JobMode::Rebuild);
    let done = finished(&f.state).await;
    let job = done.job.unwrap();
    assert_eq!(job.state, "completed");
    // Everything was decoded again (nothing skipped as up to date).
    assert_eq!((job.done, job.generated, job.skipped), (3, 3, 0));
    assert_eq!(done.cached, 3);
    let file = state_file_where(&f.state, |file| file["job"]["state"] == "completed").await;
    assert_eq!(file["job"]["rebuild_cleared"], true);
}

#[tokio::test]
async fn failed_mode_retries_failures_and_falls_back_to_missing() {
    let f = fixture("list_waveform_rate = 1000\n");
    // Without known failures: fills missing entries instead.
    let started = start(&f.state, r#"{"mode":"failed"}"#).await;
    let job = started.job.unwrap();
    assert_eq!(job.mode, wf_job::JobMode::Failed);
    assert_eq!(job.mode_effective, wf_job::JobMode::Missing);
    assert_eq!(finished(&f.state).await.job.unwrap().generated, 3);
    // Corrupt one file: a missing job records a failure.
    fs::write(f.sounds.join("tone.wav"), b"RIFF\0\0\0\0WAVEjunkjunkjunk").unwrap();
    start(&f.state, r#"{"mode":"missing"}"#).await;
    let s = finished(&f.state).await;
    assert_eq!(s.job.as_ref().unwrap().failed, 1, "{s:?}");
    assert_eq!(s.failed, 1);
    let last = s.last_error.clone().unwrap();
    assert_eq!(last.id, Some(f.id("tone.wav")));
    assert!(
        !last.message.contains(&*f.sounds.to_string_lossy()),
        "{}",
        last.message
    );
    // A failed job retries exactly the failed record.
    let started = start(&f.state, r#"{"mode":"failed"}"#).await;
    let job = started.job.unwrap();
    assert_eq!(job.mode_effective, wf_job::JobMode::Failed);
    assert_eq!(job.total, 1);
    let job = finished(&f.state).await.job.unwrap();
    assert_eq!((job.done, job.failed), (1, 1));
    // Repaired: retried successfully.
    wav(&f.sounds.join("tone.wav"), 1000, |i| {
        if i == 10 { 16384 } else { 0 }
    });
    start(&f.state, r#"{"mode":"failed"}"#).await;
    let s = finished(&f.state).await;
    assert_eq!(s.job.as_ref().unwrap().generated, 1, "{s:?}");
    assert_eq!(s.failed, 0);
}

#[tokio::test]
async fn paused_job_survives_a_restart_and_resumes() {
    let f = fixture("list_waveform_rate = 1\n");
    let started = start(&f.state, r#"{"mode":"missing"}"#).await;
    let id = started.job.unwrap().id;
    control(&f.state, "pause").await;
    let state = reopen(&f);
    let s = status(&state).await;
    assert_eq!(s.run_state, "paused", "{s:?}");
    let job = s.job.clone().unwrap();
    assert_eq!(job.id, id);
    assert_eq!(job.state, "paused");
    assert!(job.resumed);
    assert_eq!(s.eta_secs, None);
    // Still paused: nothing happens.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(status(&state).await.job.unwrap().state, "paused");
    control(&state, "resume").await;
    let job = finished(&state).await.job.unwrap();
    assert_eq!(job.state, "completed");
    assert_eq!(job.id, id);
    assert_eq!(job.done, 3);
}

#[tokio::test]
async fn running_job_resumes_from_its_cursor_after_a_restart() {
    let f = fixture("list_waveform_rate = 1\n");
    start(&f.state, r#"{"mode":"missing"}"#).await;
    wait_for(&f.state, "first file", |s| {
        s.job.as_ref().unwrap().done >= 1
    })
    .await;
    let state = reopen(&f);
    let s = status(&state).await;
    let job = s.job.clone().unwrap();
    assert!(job.resumed, "{s:?}");
    assert!(job.done >= 1, "{s:?}");
    assert_ne!(job.state, "paused");
    let file: serde_json::Value =
        serde_json::from_slice(&fs::read(state_dir(&state).join("job-state.json")).unwrap())
            .unwrap();
    assert!(file["job"]["cursor"].as_i64().unwrap() >= 1, "{file}");
    let job = finished(&state).await.job.unwrap();
    assert_eq!(job.state, "completed");
    // Every record is cached; already done ones were not decoded twice.
    assert_eq!(status(&state).await.cached, 3);
    assert!(job.generated <= 3);
}

#[tokio::test]
async fn failed_job_restored_after_a_restart_falls_back_to_missing() {
    let f = fixture("list_waveform_rate = 1000\n");
    let dir = state_dir(&f.state);
    f.state.list_waveforms().shutdown();
    let file = wf_job::StateFile {
        version: 1,
        library_id: f.state.library_id().to_string(),
        job: Some(wf_job::JobRecord {
            id: "wfj-1-abcd".into(),
            mode: wf_job::JobMode::Failed,
            mode_effective: wf_job::JobMode::Failed,
            state: wf_job::JobState::Running,
            paused: false,
            started_at: 1,
            updated_at: 1,
            finished_at: None,
            cursor: 0,
            max_id: 0,
            total: 5,
            done: 2,
            generated: 0,
            skipped: 0,
            failed: 2,
            skipped_offline: 0,
            rebuild_cleared: false,
        }),
        last_error: None,
    };
    wf_job::save(&dir, &file).unwrap();
    let state = AppState::open(&f.config, BearerToken::new(TOKEN).unwrap()).unwrap();
    let job = finished(&state).await.job.unwrap();
    assert_eq!(job.id, "wfj-1-abcd");
    assert_eq!(job.mode, wf_job::JobMode::Failed);
    assert_eq!(job.mode_effective, wf_job::JobMode::Missing);
    assert!(job.resumed);
    assert_eq!((job.total, job.done, job.generated), (3, 3, 3));
}

#[tokio::test]
async fn corrupt_or_foreign_state_files_are_set_aside() {
    let f = fixture("");
    let dir = state_dir(&f.state);
    f.state.list_waveforms().shutdown();
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("job-state.json"), b"{broken").unwrap();
    let state = AppState::open(&f.config, BearerToken::new(TOKEN).unwrap()).unwrap();
    assert!(status(&state).await.job.is_none());
    assert!(dir.join("job-state.json.bad").exists());
    assert!(!dir.join("job-state.json").exists());
}

#[tokio::test]
async fn paused_mid_record_stays_paused_after_a_restart() {
    // 1 file per second: a record is being processed when the job is paused.
    let f = fixture("list_waveform_rate = 1\n");
    start(&f.state, r#"{"mode":"missing"}"#).await;
    wait_for(&f.state, "a record in flight", |s| {
        let job = s.job.as_ref().unwrap();
        job.done >= 1 && job.current.is_some()
    })
    .await;
    control(&f.state, "pause").await;
    // The record in flight completes after the pause; its progress save must not
    // overwrite the paused state.
    wait_for(&f.state, "in-flight drained", |s| {
        s.job.as_ref().unwrap().current.is_none()
    })
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let file = state_file_where(&f.state, |_| true).await;
    assert_eq!(file["job"]["state"], "paused", "{file}");
    let state = reopen(&f);
    let s = status(&state).await;
    assert_eq!(s.run_state, "paused", "{s:?}");
    let done = s.job.as_ref().unwrap().done;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let s = status(&state).await;
    assert_eq!(s.job.as_ref().unwrap().state, "paused", "{s:?}");
    assert_eq!(s.job.unwrap().done, done);
}

#[tokio::test]
async fn stopped_job_does_not_resume_after_a_restart() {
    let f = fixture("list_waveform_rate = 1\n");
    start(&f.state, r#"{"mode":"missing"}"#).await;
    wait_for(&f.state, "a record in flight", |s| {
        let job = s.job.as_ref().unwrap();
        job.done >= 1 && job.current.is_some()
    })
    .await;
    control(&f.state, "stop").await;
    wait_for(&f.state, "in-flight drained", |s| {
        s.job.as_ref().unwrap().current.is_none()
    })
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let file = state_file_where(&f.state, |_| true).await;
    assert_eq!(file["job"]["state"], "cancelled", "{file}");
    let state = reopen(&f);
    let s = status(&state).await;
    let job = s.job.clone().unwrap();
    assert_eq!(job.state, "cancelled", "{s:?}");
    assert!(!job.resumed);
    assert_eq!(s.run_state, "idle");
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(status(&state).await.job.unwrap().done, job.done);
}

#[tokio::test]
async fn unusable_waveforms_db_hides_the_job_capability() {
    let f = fixture("");
    let db = state_dir(&f.state).join("waveforms.db");
    let _ = fs::remove_file(&db);
    // A directory in its place: the database cannot be opened.
    fs::create_dir_all(db.join("blocker")).unwrap();
    let state = reopen(&f);
    let caps: serde_json::Value =
        ok(call(state.clone(), Method::GET, "/capabilities", &[], LAN, None).await).await;
    assert!(caps.get("list_waveform_jobs").is_none(), "{caps}");
    err(
        post(&state, "/waveforms/jobs", Some(r#"{"mode":"missing"}"#)).await,
        StatusCode::SERVICE_UNAVAILABLE,
        "UNAVAILABLE",
    )
    .await;
}
