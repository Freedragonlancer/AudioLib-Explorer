// crates/audiolib-core/src/waveform_cache.rs
//! Owned contracts for the server's list Waveform cache management (doc/api/README.md
//! §9.2 and §9.5): status (`GET /waveforms/status`), generation jobs
//! (`POST /waveforms/jobs`, `…/jobs/current/pause|resume|stop`), the related
//! `GET`/`PATCH /settings` fields and the `list_waveform_jobs` capability.
//!
//! These values travel through [`crate::DataProvider`]; only server-mode providers
//! implement the operations, every other provider answers
//! [`WaveformCacheErrorKind::Unsupported`]. All fields default so that older and newer
//! servers deserialize (unknown fields are ignored, missing ones take their default).
use serde::{Deserialize, Serialize};

/// Normalized run state of the server's list Waveform generation (§9.5.2 `run_state`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WaveformRunState {
    #[default]
    Idle,
    Running,
    Paused,
    Error,
    /// A value this client does not know (a later server).
    #[serde(other)]
    Unknown,
}

/// Generation job mode (§9.5.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WaveformJobMode {
    /// Records without a cached list Waveform or with a stale cache key.
    Missing,
    /// Clear the library's list cache and regenerate everything (destructive; the server
    /// requires `"confirm": "rebuild"`).
    Rebuild,
    /// Records whose decode failed.
    Failed,
}

impl WaveformJobMode {
    /// Wire name (`missing`, `rebuild`, `failed`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Rebuild => "rebuild",
            Self::Failed => "failed",
        }
    }
}

/// Control of the current job (§9.5.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaveformJobControl {
    Pause,
    Resume,
    Stop,
}

impl WaveformJobControl {
    /// Wire name used in the endpoint path.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pause => "pause",
            Self::Resume => "resume",
            Self::Stop => "stop",
        }
    }
}

/// The most recent background error (§9.5.2 `last_error`); never contains a path.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WaveformCacheLastError {
    pub at: Option<i64>,
    pub error_code: String,
    pub message: String,
    pub id: Option<i64>,
}

/// The record a job is processing (§9.5.2 `job.current`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WaveformJobCurrent {
    pub id: i64,
    pub started_at: Option<i64>,
}

/// The current or most recent generation job (§9.5.2 `job`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WaveformCacheJob {
    /// Opaque id; a different id means a new job.
    pub id: String,
    /// Requested mode as sent (string so unknown later modes still deserialize).
    pub mode: String,
    /// Mode actually executed (`failed` falls back to `missing` after a restart).
    pub mode_effective: String,
    /// `running`, `yielding`, `paused`, `completed`, `cancelled` or `error`.
    pub state: String,
    pub started_at: Option<i64>,
    pub updated_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub total: u64,
    pub done: u64,
    pub generated: u64,
    pub skipped: u64,
    pub failed: u64,
    pub skipped_offline: u64,
    pub resumed: bool,
    pub current: Option<WaveformJobCurrent>,
}

impl WaveformCacheJob {
    /// Running, yielding or paused: a new job would be refused with `409 BUSY`.
    pub fn in_progress(&self) -> bool {
        matches!(self.state.as_str(), "running" | "yielding" | "paused")
    }

    /// Whether the server ran a different mode than requested (restart fallback).
    pub fn fell_back(&self) -> bool {
        !self.mode_effective.is_empty() && self.mode_effective != self.mode
    }
}

/// `GET /waveforms/status` (§9.2 plus the §9.5.2 additions; all fields optional on the
/// wire).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WaveformCacheStatus {
    pub enabled: bool,
    /// `off`, `idle`, `running`, `yielding` or `paused` (§9.2).
    pub state: String,
    /// `scan` or `audio` while yielding.
    pub yield_reason: Option<String>,
    pub total: u64,
    pub cached: u64,
    pub remaining: Option<u64>,
    pub queued_on_demand: u64,
    pub failed: u64,
    pub skipped_offline: u64,
    pub rate: u32,
    pub cache_bytes: u64,
    pub cache_limit_bytes: u64,
    pub over_budget: bool,
    pub pass_started_at: Option<i64>,
    pub pass_finished_at: Option<i64>,
    // §9.5.2 (absent on older servers).
    pub pending: Option<u64>,
    pub run_state: Option<WaveformRunState>,
    pub current_rate: Option<f64>,
    /// `None` while the server is still estimating (shown as 「估算中」).
    pub eta_secs: Option<u64>,
    pub last_error: Option<WaveformCacheLastError>,
    pub job: Option<WaveformCacheJob>,
}

impl WaveformCacheStatus {
    /// `run_state`, or one derived from the §9.2 `state` for older servers.
    pub fn effective_run_state(&self) -> WaveformRunState {
        self.run_state.unwrap_or(match self.state.as_str() {
            "running" | "yielding" => WaveformRunState::Running,
            "paused" => WaveformRunState::Paused,
            _ => WaveformRunState::Idle,
        })
    }

    /// Records still to generate: `pending`, else `remaining`.
    pub fn effective_pending(&self) -> Option<u64> {
        self.pending.or(self.remaining)
    }

    /// Progress in 0.0..=1.0: of the active job when there is one, else `cached / total`.
    pub fn progress(&self) -> f32 {
        let (done, total) = match &self.job {
            Some(job) if job.in_progress() => (job.done, job.total),
            _ => (self.cached, self.total),
        };
        if total == 0 {
            0.0
        } else {
            (done.min(total) as f64 / total as f64) as f32
        }
    }

    /// Whether a job is running, yielding or paused.
    pub fn job_in_progress(&self) -> bool {
        self.job.as_ref().is_some_and(WaveformCacheJob::in_progress)
    }
}

/// The waveform fields of `GET`/`PATCH /settings` (§9.2, §9.5.4).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WaveformCacheSettings {
    pub pregenerate_list_waveforms: bool,
    /// `default`, `config` or `admin`.
    pub pregenerate_list_waveforms_source: String,
    /// Effective generation rate (files/second); absent on older servers.
    pub list_waveform_rate: Option<u32>,
    /// `default`, `config` or `admin`.
    pub list_waveform_rate_source: Option<String>,
    /// The server would accept a `PATCH /settings` from this client (a hint).
    pub can_change: bool,
}

/// Lowest and highest accepted `list_waveform_rate` (§9.5.4).
pub const LIST_WAVEFORM_RATE_RANGE: std::ops::RangeInclusive<u32> = 1..=1000;

/// A `PATCH /settings` change; `None` fields are not sent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WaveformCacheSettingsPatch {
    pub pregenerate_list_waveforms: Option<bool>,
    /// `Some(None)` removes the override (back to the server configuration).
    pub list_waveform_rate: Option<Option<u32>>,
}

impl WaveformCacheSettingsPatch {
    /// Check the rate range before anything is sent.
    pub fn validate(&self) -> Result<(), WaveformCacheError> {
        if let Some(Some(rate)) = self.list_waveform_rate
            && !LIST_WAVEFORM_RATE_RANGE.contains(&rate)
        {
            return Err(WaveformCacheError::new(
                WaveformCacheErrorKind::Validation,
                format!("list_waveform_rate must be 1..=1000, got {rate}"),
            ));
        }
        if self.pregenerate_list_waveforms.is_none() && self.list_waveform_rate.is_none() {
            return Err(WaveformCacheError::new(
                WaveformCacheErrorKind::Validation,
                "empty settings change",
            ));
        }
        Ok(())
    }
}

/// `GET /capabilities` `list_waveform_jobs` (§9.5.7); absent = legacy server.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WaveformJobsCapability {
    pub version: u32,
    pub modes: Vec<String>,
    pub rate_setting: bool,
}

impl WaveformJobsCapability {
    /// Whether the server lists `mode` (the client shows only listed modes).
    pub fn offers(&self, mode: WaveformJobMode) -> bool {
        self.version >= 1 && self.modes.iter().any(|offered| offered == mode.as_str())
    }
}

/// What the waveform cache section may offer for the active server.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WaveformCacheCapabilities {
    /// `None` on servers without the §9.5 job endpoints.
    pub jobs: Option<WaveformJobsCapability>,
    /// The server's `can_manage` hint (§6.9).
    pub can_manage: bool,
}

/// Classification the settings page acts on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaveformCacheErrorKind {
    /// `403 MANAGE_FORBIDDEN`: disable the controls.
    Forbidden,
    /// `409 BUSY`: a job already exists (refresh and show it).
    Busy,
    /// `409 JOB_STATE`: no job or it has finished (refresh).
    JobState,
    /// `400 VALIDATION`.
    Validation,
    /// The provider or server does not offer this.
    Unsupported,
    /// Unreachable, timeout, starting (`503`): retry later.
    Unavailable,
    Other,
}

/// A failed waveform cache operation with localized text.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct WaveformCacheError {
    pub kind: WaveformCacheErrorKind,
    pub message: String,
    /// The server's `400 VALIDATION` `sub_code` (§9.5.3: `CONFIRM_REQUIRED`,
    /// `UNSUPPORTED_MODE`) when it sent one; `message` already explains it.
    pub sub_code: Option<String>,
}

impl WaveformCacheError {
    pub fn new(kind: WaveformCacheErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            sub_code: None,
        }
    }

    /// The default answer of providers without server waveform caches.
    pub fn unsupported() -> Self {
        Self::new(
            WaveformCacheErrorKind::Unsupported,
            "This data provider has no server list Waveform cache",
        )
    }
}

/// Result of a waveform cache operation.
pub type WaveformCacheResult<T> = std::result::Result<T, WaveformCacheError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_status_derives_run_state_and_pending() {
        let status: WaveformCacheStatus = serde_json::from_str(
            r#"{"enabled":true,"state":"yielding","total":10,"cached":4,"remaining":6}"#,
        )
        .unwrap();
        assert_eq!(status.effective_run_state(), WaveformRunState::Running);
        assert_eq!(status.effective_pending(), Some(6));
        assert!((status.progress() - 0.4).abs() < 1e-6);
        assert!(status.job.is_none() && status.eta_secs.is_none());
    }

    #[test]
    fn extended_status_reads_job_and_unknown_values() {
        let status: WaveformCacheStatus = serde_json::from_str(
            r#"{"state":"running","total":100,"cached":10,"pending":50,"run_state":"later",
            "current_rate":9.6,"eta_secs":null,"last_error":{"at":1,"error_code":"AUDIO","message":"bad","id":null},
            "job":{"id":"wfj-1","mode":"failed","mode_effective":"missing","state":"paused","started_at":5,
            "done":50,"failed":1,"total":100,"future":1}}"#,
        )
        .unwrap();
        assert_eq!(status.effective_run_state(), WaveformRunState::Unknown);
        let job = status.job.as_ref().unwrap();
        assert!(job.in_progress() && job.fell_back());
        assert!((status.progress() - 0.5).abs() < 1e-6);
        assert_eq!(status.last_error.unwrap().error_code, "AUDIO");
    }

    #[test]
    fn rate_patch_is_range_checked() {
        let patch = |rate| WaveformCacheSettingsPatch {
            list_waveform_rate: Some(rate),
            ..Default::default()
        };
        assert!(patch(Some(1)).validate().is_ok());
        assert!(patch(Some(1000)).validate().is_ok());
        assert!(patch(None).validate().is_ok());
        assert_eq!(
            patch(Some(0)).validate().unwrap_err().kind,
            WaveformCacheErrorKind::Validation
        );
        assert!(patch(Some(1001)).validate().is_err());
        assert!(WaveformCacheSettingsPatch::default().validate().is_err());
    }

    #[test]
    fn capability_lists_modes() {
        let capability: WaveformJobsCapability = serde_json::from_str(
            r#"{"version":1,"modes":["missing","failed"],"rate_setting":true}"#,
        )
        .unwrap();
        assert!(capability.offers(WaveformJobMode::Missing));
        assert!(!capability.offers(WaveformJobMode::Rebuild));
    }
}
