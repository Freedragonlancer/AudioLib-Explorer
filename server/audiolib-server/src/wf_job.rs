// server/audiolib-server/src/wf_job.rs
//! List waveform cache jobs (doc/api/README.md §9.5): the job record, its persisted
//! state file `job-state.json` next to the list cache, the 60 s rate window and the
//! wire shapes of `GET /waveforms/status` additions and `/capabilities`.
//!
//! Scheduling lives in [`crate::list_waveform`]; this module is plain data and file I/O.
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// File name of the persisted job state in `list/<library hash>/`.
pub const STATE_FILE: &str = "job-state.json";
/// Format version of [`StateFile`].
pub const STATE_VERSION: u32 = 1;
/// Progress is saved at most this often while a job runs …
pub const SAVE_EVERY: Duration = Duration::from_secs(5);
/// … or after this many records.
pub const SAVE_RECORDS: u64 = 1000;
/// Sliding window of `current_rate` / `eta_secs`.
pub const RATE_WINDOW: Duration = Duration::from_secs(60);
/// Every accepted `mode`, as advertised in `/capabilities`.
pub const MODES: [&str; 3] = ["missing", "rebuild", "failed"];

/// What a job processes (§9.5.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobMode {
    /// Records without a cache entry or with a stale one.
    Missing,
    /// Clear the library's list cache, then regenerate everything.
    Rebuild,
    /// Retry records whose decoding failed (in this process).
    Failed,
}

impl JobMode {
    /// Parse a wire name.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "missing" => Some(Self::Missing),
            "rebuild" => Some(Self::Rebuild),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

/// Persisted job state (`yielding` is never stored: it is derived for the status).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobState {
    /// Taking records (or waiting for the yield conditions).
    Running,
    /// Manually paused; stays paused across restarts.
    Paused,
    /// Every record was checked.
    Completed,
    /// Stopped with `POST /waveforms/jobs/current/stop`.
    Cancelled,
    /// Could not continue; see `last_error`.
    Error,
}

impl JobState {
    /// Running or paused (blocks a new job).
    pub fn active(self) -> bool {
        matches!(self, Self::Running | Self::Paused)
    }
}

/// The most recent background error (§9.5.2 `last_error`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastError {
    /// Unix seconds.
    pub at: i64,
    /// Error code, e.g. `AUDIO`, `IO` or `DATABASE`.
    pub error_code: String,
    /// Path-free description.
    pub message: String,
    /// Record concerned, if any.
    pub id: Option<i64>,
}

/// One job as persisted in [`StateFile`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobRecord {
    /// Opaque id; changes with every new job.
    pub id: String,
    /// Requested mode.
    pub mode: JobMode,
    /// Mode actually executed (`missing` when `failed` fell back).
    pub mode_effective: JobMode,
    /// Current state.
    pub state: JobState,
    /// Same as `state == paused` (kept for readability of the file).
    pub paused: bool,
    /// Unix seconds.
    pub started_at: i64,
    /// Unix seconds of the last change.
    pub updated_at: i64,
    /// Unix seconds; `null` while active.
    pub finished_at: Option<i64>,
    /// Largest record id fully processed (resume from `cursor + 1`).
    pub cursor: i64,
    /// Largest record id when the job started (upper bound of the walk).
    pub max_id: i64,
    /// Records to check.
    pub total: u64,
    /// Records checked (`generated + skipped + failed + skipped_offline`).
    pub done: u64,
    /// Records decoded and written.
    pub generated: u64,
    /// Records already up to date (only a metadata read).
    pub skipped: u64,
    /// Records that failed to decode.
    pub failed: u64,
    /// Records skipped because their root is offline.
    pub skipped_offline: u64,
    /// `rebuild` only: the cache was cleared.
    #[serde(default)]
    pub rebuild_cleared: bool,
}

/// `job-state.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateFile {
    /// [`STATE_VERSION`].
    pub version: u32,
    /// Library the state belongs to.
    pub library_id: String,
    /// Current or most recent job.
    pub job: Option<JobRecord>,
    /// Most recent background error.
    pub last_error: Option<LastError>,
}

/// Outcome of [`load`].
#[derive(Debug, PartialEq, Eq)]
pub enum Loaded {
    /// No state file.
    Absent,
    /// A valid file of this library.
    State(Box<StateFile>),
    /// Unreadable, unknown version or another library: renamed to `.bad`.
    Rejected,
}

/// Blocking. Read `dir/job-state.json`; a corrupt file, an unknown version or another
/// library's file is renamed to `job-state.json.bad` (logged) and ignored.
pub fn load(dir: &Path, library_id: &str) -> Loaded {
    let path = dir.join(STATE_FILE);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Loaded::Absent,
        Err(error) => {
            tracing::warn!(%error, "Cannot read the list waveform job state");
            return Loaded::Absent;
        }
    };
    let parsed: Option<StateFile> = serde_json::from_slice(&bytes).ok();
    match parsed {
        Some(file) if file.version == STATE_VERSION && file.library_id == library_id => {
            Loaded::State(Box::new(file))
        }
        other => {
            let reason = match other {
                None => "corrupt",
                Some(file) if file.version != STATE_VERSION => "unknown version",
                Some(_) => "another library",
            };
            tracing::warn!(
                reason,
                "Ignoring the list waveform job state; renamed to {STATE_FILE}.bad"
            );
            let _ = fs::rename(&path, bad_path(dir));
            Loaded::Rejected
        }
    }
}

/// `dir/job-state.json.bad`.
pub fn bad_path(dir: &Path) -> PathBuf {
    dir.join(format!("{STATE_FILE}.bad"))
}

/// Blocking. Atomically replace `dir/job-state.json` (temporary file + rename; on Unix
/// the directory is synced after the rename).
pub fn save(dir: &Path, file: &StateFile) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    let bytes = serde_json::to_vec_pretty(file).map_err(std::io::Error::other)?;
    let mut stage = tempfile::Builder::new()
        .prefix(".job-state-")
        .tempfile_in(dir)?;
    stage.write_all(&bytes)?;
    stage.as_file().sync_all()?;
    stage.persist(dir.join(STATE_FILE)).map_err(|e| e.error)?;
    // Make the rename itself durable.
    #[cfg(unix)]
    fs::File::open(dir)?.sync_all()?;
    Ok(())
}

/// New opaque job id: `wfj-<unix seconds>-<4 hex>`.
pub fn new_job_id(now: i64) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let mix = (nanos ^ (std::process::id() << 7)) & 0xffff;
    format!("wfj-{now}-{mix:04x}")
}

/// Records processed in the last [`RATE_WINDOW`] (§9.5.2 `current_rate`).
#[derive(Debug, Default)]
pub struct RateWindow {
    samples: VecDeque<Instant>,
    /// When processing (re)started; the window is shorter until 60 s have passed.
    since: Option<Instant>,
}

impl RateWindow {
    /// Processing starts or resumes now (the window restarts after a pause).
    pub fn restart(&mut self, now: Instant) {
        self.samples.clear();
        self.since = Some(now);
    }

    /// Stop measuring (paused, finished).
    pub fn stop(&mut self) {
        self.samples.clear();
        self.since = None;
    }

    /// One record was processed at `now`.
    pub fn record(&mut self, now: Instant) {
        self.since.get_or_insert(now);
        self.samples.push_back(now);
        self.trim(now);
    }

    fn trim(&mut self, now: Instant) {
        while self
            .samples
            .front()
            .is_some_and(|at| now.saturating_duration_since(*at) > RATE_WINDOW)
        {
            self.samples.pop_front();
        }
    }

    /// Records per second over the window, `None` without any sample yet.
    pub fn rate(&mut self, now: Instant) -> Option<f64> {
        self.trim(now);
        let since = self.since?;
        if self.samples.is_empty() {
            return if now.saturating_duration_since(since) >= RATE_WINDOW {
                Some(0.0)
            } else {
                None
            };
        }
        let span = now
            .saturating_duration_since(since)
            .min(RATE_WINDOW)
            .as_secs_f64()
            .max(1.0);
        Some(self.samples.len() as f64 / span)
    }
}

/// `eta_secs`: `pending / rate`, rounded up; `None` without a usable rate.
pub fn eta_secs(pending: Option<u64>, rate: Option<f64>) -> Option<u64> {
    let pending = pending?;
    let rate = rate.filter(|r| *r > 0.0 && r.is_finite())?;
    Some((pending as f64 / rate).ceil() as u64)
}

/// Round to one decimal.
pub fn one_decimal(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

/// `job.current` of the status.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CurrentItem {
    /// Record id.
    pub id: i64,
    /// Unix seconds.
    pub started_at: i64,
}

/// `job` of `GET /waveforms/status` (§9.5.2).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobView {
    /// Opaque id.
    pub id: String,
    /// Requested mode.
    pub mode: JobMode,
    /// Executed mode.
    pub mode_effective: JobMode,
    /// `running`, `yielding`, `paused`, `completed`, `cancelled` or `error`.
    pub state: String,
    /// Unix seconds.
    pub started_at: i64,
    /// Unix seconds.
    pub updated_at: i64,
    /// Unix seconds, `null` while active.
    pub finished_at: Option<i64>,
    /// Records to check.
    pub total: u64,
    /// Records checked.
    pub done: u64,
    /// Records decoded and written.
    pub generated: u64,
    /// Records already up to date.
    pub skipped: u64,
    /// Records that failed to decode.
    pub failed: u64,
    /// Records skipped because their root is offline.
    pub skipped_offline: u64,
    /// The job was resumed after a restart.
    pub resumed: bool,
    /// Record being processed, if any.
    pub current: Option<CurrentItem>,
}

/// `list_waveform_jobs` of `GET /capabilities` (§9.5.7).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ListWaveformJobsCapability {
    /// Version of the job API.
    pub version: u32,
    /// Accepted modes.
    pub modes: Vec<String>,
    /// `PATCH /settings` accepts `list_waveform_rate`.
    pub rate_setting: bool,
}

impl Default for ListWaveformJobsCapability {
    fn default() -> Self {
        Self {
            version: 1,
            modes: MODES.iter().map(|m| (*m).to_string()).collect(),
            rate_setting: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> JobRecord {
        JobRecord {
            id: "wfj-1-0001".into(),
            mode: JobMode::Missing,
            mode_effective: JobMode::Missing,
            state: JobState::Running,
            paused: false,
            started_at: 1,
            updated_at: 2,
            finished_at: None,
            cursor: 41,
            max_id: 99,
            total: 10,
            done: 4,
            generated: 1,
            skipped: 3,
            failed: 0,
            skipped_offline: 0,
            rebuild_cleared: false,
        }
    }

    #[test]
    fn state_file_round_trips_and_rejects_foreign_or_corrupt_files() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(dir.path(), "lib"), Loaded::Absent);
        let file = StateFile {
            version: STATE_VERSION,
            library_id: "lib".into(),
            job: Some(record()),
            last_error: Some(LastError {
                at: 3,
                error_code: "AUDIO".into(),
                message: "bad".into(),
                id: Some(7),
            }),
        };
        save(dir.path(), &file).unwrap();
        assert_eq!(
            load(dir.path(), "lib"),
            Loaded::State(Box::new(file.clone()))
        );
        // No temporary files are left behind.
        let names: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec![STATE_FILE.to_string()]);
        // Another library: renamed to .bad.
        assert_eq!(load(dir.path(), "other"), Loaded::Rejected);
        assert!(bad_path(dir.path()).exists());
        assert_eq!(load(dir.path(), "lib"), Loaded::Absent);
        // Unknown version and corrupt JSON.
        save(
            dir.path(),
            &StateFile {
                version: 9,
                ..file.clone()
            },
        )
        .unwrap();
        assert_eq!(load(dir.path(), "lib"), Loaded::Rejected);
        fs::write(dir.path().join(STATE_FILE), b"{not json").unwrap();
        assert_eq!(load(dir.path(), "lib"), Loaded::Rejected);
        assert!(!dir.path().join(STATE_FILE).exists());
    }

    #[test]
    fn state_file_uses_the_documented_field_names() {
        let value = serde_json::to_value(StateFile {
            version: 1,
            library_id: "lib".into(),
            job: Some(record()),
            last_error: None,
        })
        .unwrap();
        let job = value["job"].as_object().unwrap();
        for key in [
            "id",
            "mode",
            "mode_effective",
            "state",
            "paused",
            "started_at",
            "updated_at",
            "finished_at",
            "cursor",
            "max_id",
            "total",
            "done",
            "generated",
            "skipped",
            "failed",
            "skipped_offline",
            "rebuild_cleared",
        ] {
            assert!(job.contains_key(key), "{key}");
        }
        assert_eq!(job["mode"], "missing");
        assert_eq!(job["state"], "running");
    }

    #[test]
    fn modes_parse_and_states_classify() {
        assert_eq!(JobMode::parse("missing"), Some(JobMode::Missing));
        assert_eq!(JobMode::parse("rebuild"), Some(JobMode::Rebuild));
        assert_eq!(JobMode::parse("failed"), Some(JobMode::Failed));
        assert_eq!(JobMode::parse("all"), None);
        assert!(JobState::Running.active() && JobState::Paused.active());
        assert!(!JobState::Completed.active());
        assert!(!JobState::Cancelled.active());
        assert!(!JobState::Error.active());
        let a = new_job_id(5);
        assert!(a.starts_with("wfj-5-"), "{a}");
    }

    #[test]
    fn rate_window_covers_sixty_seconds_and_eta_follows() {
        let start = Instant::now();
        let mut window = RateWindow::default();
        assert_eq!(window.rate(start), None);
        window.restart(start);
        assert_eq!(window.rate(start + Duration::from_secs(1)), None);
        for i in 0..20 {
            window.record(start + Duration::from_millis(500 * i));
        }
        // 20 records in the first 10 s: 2/s.
        let rate = window.rate(start + Duration::from_secs(10)).unwrap();
        assert!((rate - 2.0).abs() < 1e-9, "{rate}");
        assert_eq!(eta_secs(Some(100), Some(rate)), Some(50));
        assert_eq!(eta_secs(Some(3), Some(2.0)), Some(2));
        // After 120 s without records the window is empty: rate 0, no ETA.
        let later = start + Duration::from_secs(120);
        assert_eq!(window.rate(later), Some(0.0));
        assert_eq!(eta_secs(Some(100), Some(0.0)), None);
        assert_eq!(eta_secs(None, Some(1.0)), None);
        // A full window divides by 60 s.
        for i in 0..120 {
            window.record(later + Duration::from_millis(500 * i));
        }
        let rate = window.rate(later + Duration::from_secs(60)).unwrap();
        assert!((rate - 2.0).abs() < 0.05, "{rate}");
        window.stop();
        assert_eq!(window.rate(later + Duration::from_secs(61)), None);
        assert_eq!(one_decimal(9.649), 9.6);
    }
}
