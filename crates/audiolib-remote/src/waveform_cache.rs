// crates/audiolib-remote/src/waveform_cache.rs
//! Server list Waveform cache management (doc/api/README.md §9.5): generation jobs
//! (`POST /waveforms/jobs`, `POST /waveforms/jobs/current/pause|resume|stop`), the legacy
//! `POST /waveforms/pregeneration/pause|resume` aliases (§9.2), the `list_waveform_rate`
//! setting (`PATCH /settings`) and the mapping of failures to the core
//! [`WaveformCacheError`] used through `DataProvider`.
//!
//! Mutations are gated by the server's `manage_mode` (`403 MANAGE_FORBIDDEN`); a second
//! job is `409 BUSY`, control without a job `409 JOB_STATE`. Every call blocks: callers
//! run them on worker threads, never on the UI thread or in audio callbacks.
use crate::client::RemoteClient;
use crate::error::{RemoteError, Result};
use crate::manage::manage_error_text;
use audiolib_core::waveform_cache::{
    WaveformCacheError, WaveformCacheErrorKind, WaveformCacheSettings, WaveformCacheSettingsPatch,
    WaveformCacheStatus, WaveformJobControl, WaveformJobMode,
};

impl RemoteClient {
    /// `POST /waveforms/jobs` (§9.5.3). `rebuild` is refused locally unless
    /// `confirm_rebuild` (the user confirmed twice); then `"confirm": "rebuild"` is sent.
    /// Replies `202` with the full status.
    pub fn start_list_waveform_job(
        &self,
        mode: WaveformJobMode,
        confirm_rebuild: bool,
    ) -> Result<WaveformCacheStatus> {
        let body = match mode {
            WaveformJobMode::Rebuild if !confirm_rebuild => {
                return Err(RemoteError::Validation(
                    "a rebuild must be confirmed before it is sent".into(),
                ));
            }
            WaveformJobMode::Rebuild => {
                serde_json::json!({ "mode": mode.as_str(), "confirm": "rebuild" })
            }
            _ => serde_json::json!({ "mode": mode.as_str() }),
        };
        self.manage_json("POST", "/waveforms/jobs", Some(&body))
    }

    /// `POST /waveforms/jobs/current/{pause|resume|stop}` (§9.5.3, no body).
    pub fn control_list_waveform_job(
        &self,
        control: WaveformJobControl,
    ) -> Result<WaveformCacheStatus> {
        self.manage_json(
            "POST",
            &format!("/waveforms/jobs/current/{}", control.as_str()),
            None,
        )
    }

    /// Legacy `POST /waveforms/pregeneration/{pause|resume}` (§9.2) for servers without
    /// `list_waveform_jobs`. `Stop` has no legacy endpoint and is refused locally.
    pub fn control_list_waveform_pregeneration(&self, control: WaveformJobControl) -> Result<()> {
        if control == WaveformJobControl::Stop {
            return Err(RemoteError::Validation(
                "pre-generation can only be paused or resumed".into(),
            ));
        }
        self.manage_call(
            "POST",
            &format!("/waveforms/pregeneration/{}", control.as_str()),
            None,
        )
        .map(drop)
    }

    /// `PATCH /settings` with `pregenerate_list_waveforms` and/or `list_waveform_rate`
    /// (`null` removes the rate override). Validated before sending.
    pub fn update_list_waveform_settings(
        &self,
        patch: &WaveformCacheSettingsPatch,
    ) -> Result<WaveformCacheSettings> {
        patch
            .validate()
            .map_err(|error| RemoteError::Validation(error.message))?;
        let mut body = serde_json::Map::new();
        if let Some(enabled) = patch.pregenerate_list_waveforms {
            body.insert("pregenerate_list_waveforms".into(), enabled.into());
        }
        if let Some(rate) = patch.list_waveform_rate {
            body.insert(
                "list_waveform_rate".into(),
                rate.map_or(serde_json::Value::Null, Into::into),
            );
        }
        self.manage_json("PATCH", "/settings", Some(&serde_json::Value::Object(body)))
    }
}

/// A remote failure as the core waveform cache error, with management wording.
pub fn waveform_cache_error(error: &RemoteError, chinese: bool) -> WaveformCacheError {
    let kind = match error {
        RemoteError::Forbidden(_) => WaveformCacheErrorKind::Forbidden,
        RemoteError::Busy { .. } => WaveformCacheErrorKind::Busy,
        RemoteError::JobState(_) => WaveformCacheErrorKind::JobState,
        RemoteError::Validation(_) => WaveformCacheErrorKind::Validation,
        RemoteError::Unsupported(_) | RemoteError::NotFound(_) => {
            WaveformCacheErrorKind::Unsupported
        }
        RemoteError::Unreachable(_)
        | RemoteError::Timeout(_)
        | RemoteError::Starting { .. }
        | RemoteError::Tls { .. } => WaveformCacheErrorKind::Unavailable,
        _ => WaveformCacheErrorKind::Other,
    };
    let message = match (error, chinese) {
        (RemoteError::Busy { message, .. }, true) => {
            format!("已有列表波形任务在进行中（409），已刷新显示当前任务。（{message}）")
        }
        (RemoteError::Busy { message, .. }, false) => format!(
            "A list Waveform job is already in progress (409); showing the current job. ({message})"
        ),
        (RemoteError::JobState(message), true) => {
            format!("当前没有进行中的列表波形任务（409），已刷新状态。（{message}）")
        }
        (RemoteError::JobState(message), false) => format!(
            "There is no list Waveform job in progress (409); the status was refreshed. ({message})"
        ),
        (other, chinese) => manage_error_text(other, chinese),
    };
    WaveformCacheError::new(kind, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_server::{TestRequest, TestResponse, TestServer};
    use audiolib_core::waveform_cache::WaveformRunState;
    use std::sync::{Arc, Mutex};

    const LIBRARY: &str =
        r#"{"library_id":"lib-wc","identity_bound_writes":true,"schema_version":5,"revision":1}"#;
    const STATUS: &str = r#"{"enabled":true,"state":"running","total":10,"cached":4,"remaining":6,
        "pending":6,"run_state":"running","current_rate":2.5,"eta_secs":3,"last_error":null,
        "job":{"id":"wfj-1","mode":"missing","mode_effective":"missing","state":"running",
        "started_at":1700000000,"done":4,"failed":0,"total":10}}"#;

    type Seen = Arc<Mutex<Vec<(String, String, String)>>>;

    fn server(
        reply: impl Fn(&TestRequest) -> TestResponse + Send + Sync + 'static,
    ) -> (TestServer, Seen) {
        let seen: Seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        let server = TestServer::start(move |request| {
            if request.method == "GET" && request.path == "/api/v1/library" {
                return TestResponse::json(200, LIBRARY);
            }
            log.lock().unwrap().push((
                request.method.clone(),
                request.path.clone(),
                String::from_utf8_lossy(&request.body).into_owned(),
            ));
            reply(request)
        });
        (server, seen)
    }

    fn client(server: &TestServer) -> RemoteClient {
        RemoteClient::new(&server.url, "")
            .unwrap()
            .with_library_id("lib-wc")
            .unwrap()
    }

    fn error(status: u16, code: &str) -> TestResponse {
        TestResponse::json(
            status,
            &format!(r#"{{"error_code":"{code}","message":"refused","job_id":"wfj-1"}}"#),
        )
    }

    #[test]
    fn jobs_send_modes_and_confirm_only_for_rebuild() {
        let (server, seen) = server(|_| TestResponse::json(202, STATUS));
        let client = client(&server);
        let status = client
            .start_list_waveform_job(WaveformJobMode::Missing, false)
            .unwrap();
        assert_eq!(status.effective_run_state(), WaveformRunState::Running);
        assert_eq!(status.eta_secs, Some(3));
        assert_eq!(status.job.unwrap().id, "wfj-1");
        assert!(matches!(
            client.start_list_waveform_job(WaveformJobMode::Rebuild, false),
            Err(RemoteError::Validation(_))
        ));
        client
            .start_list_waveform_job(WaveformJobMode::Rebuild, true)
            .unwrap();
        client
            .start_list_waveform_job(WaveformJobMode::Failed, false)
            .unwrap();
        let seen = seen.lock().unwrap();
        let bodies: Vec<serde_json::Value> = seen
            .iter()
            .map(|(method, path, body)| {
                assert_eq!(
                    (method.as_str(), path.as_str()),
                    ("POST", "/api/v1/waveforms/jobs")
                );
                serde_json::from_str(body).unwrap()
            })
            .collect();
        assert_eq!(
            bodies,
            vec![
                serde_json::json!({"mode":"missing"}),
                serde_json::json!({"mode":"rebuild","confirm":"rebuild"}),
                serde_json::json!({"mode":"failed"}),
            ],
            "an unconfirmed rebuild is never sent"
        );
    }

    #[test]
    fn controls_use_current_job_and_legacy_endpoints() {
        let (server, seen) = server(|request| {
            if request.path.contains("/pregeneration/") {
                TestResponse::json(200, r#"{"ok":true}"#)
            } else {
                TestResponse::json(200, STATUS)
            }
        });
        let client = client(&server);
        for control in [
            WaveformJobControl::Pause,
            WaveformJobControl::Resume,
            WaveformJobControl::Stop,
        ] {
            client.control_list_waveform_job(control).unwrap();
        }
        client
            .control_list_waveform_pregeneration(WaveformJobControl::Pause)
            .unwrap();
        client
            .control_list_waveform_pregeneration(WaveformJobControl::Resume)
            .unwrap();
        assert!(
            client
                .control_list_waveform_pregeneration(WaveformJobControl::Stop)
                .is_err()
        );
        let paths: Vec<String> = seen
            .lock()
            .unwrap()
            .iter()
            .map(|(method, path, body)| {
                assert_eq!(method, "POST");
                assert!(body.is_empty(), "controls carry no body");
                path.clone()
            })
            .collect();
        assert_eq!(
            paths,
            [
                "/api/v1/waveforms/jobs/current/pause",
                "/api/v1/waveforms/jobs/current/resume",
                "/api/v1/waveforms/jobs/current/stop",
                "/api/v1/waveforms/pregeneration/pause",
                "/api/v1/waveforms/pregeneration/resume",
            ]
        );
    }

    #[test]
    fn errors_classify_forbidden_busy_and_job_state() {
        let (server, _) = server(|request| match request.path.as_str() {
            "/api/v1/waveforms/jobs" => error(409, "BUSY"),
            "/api/v1/waveforms/jobs/current/stop" => error(409, "JOB_STATE"),
            _ => error(403, "MANAGE_FORBIDDEN"),
        });
        let client = client(&server);
        let busy = client
            .start_list_waveform_job(WaveformJobMode::Missing, false)
            .unwrap_err();
        assert!(matches!(busy, RemoteError::Busy { .. }), "{busy:?}");
        assert_eq!(
            waveform_cache_error(&busy, true).kind,
            WaveformCacheErrorKind::Busy
        );
        let state = client
            .control_list_waveform_job(WaveformJobControl::Stop)
            .unwrap_err();
        assert!(matches!(state, RemoteError::JobState(_)), "{state:?}");
        assert_eq!(
            waveform_cache_error(&state, false).kind,
            WaveformCacheErrorKind::JobState
        );
        let forbidden = client
            .control_list_waveform_job(WaveformJobControl::Pause)
            .unwrap_err();
        assert_eq!(
            waveform_cache_error(&forbidden, true).kind,
            WaveformCacheErrorKind::Forbidden
        );
        let forbidden = client
            .update_list_waveform_settings(&WaveformCacheSettingsPatch {
                list_waveform_rate: Some(Some(20)),
                ..Default::default()
            })
            .unwrap_err();
        assert!(matches!(forbidden, RemoteError::Forbidden(_)));
    }

    #[test]
    fn rate_patch_sends_value_or_null_and_rejects_out_of_range() {
        let (server, seen) = server(|_| {
            TestResponse::json(
                200,
                r#"{"pregenerate_list_waveforms":true,"pregenerate_list_waveforms_source":"admin",
                "list_waveform_rate":20,"list_waveform_rate_source":"admin","can_change":true}"#,
            )
        });
        let client = client(&server);
        let settings = client
            .update_list_waveform_settings(&WaveformCacheSettingsPatch {
                list_waveform_rate: Some(Some(20)),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(settings.list_waveform_rate, Some(20));
        assert_eq!(settings.list_waveform_rate_source.as_deref(), Some("admin"));
        client
            .update_list_waveform_settings(&WaveformCacheSettingsPatch {
                pregenerate_list_waveforms: Some(false),
                list_waveform_rate: Some(None),
            })
            .unwrap();
        assert!(matches!(
            client.update_list_waveform_settings(&WaveformCacheSettingsPatch {
                list_waveform_rate: Some(Some(1001)),
                ..Default::default()
            }),
            Err(RemoteError::Validation(_))
        ));
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2, "an out-of-range rate is not sent");
        let bodies: Vec<serde_json::Value> = seen
            .iter()
            .map(|(method, path, body)| {
                assert_eq!(
                    (method.as_str(), path.as_str()),
                    ("PATCH", "/api/v1/settings")
                );
                serde_json::from_str(body).unwrap()
            })
            .collect();
        assert_eq!(bodies[0], serde_json::json!({"list_waveform_rate":20}));
        assert_eq!(
            bodies[1],
            serde_json::json!({"pregenerate_list_waveforms":false,"list_waveform_rate":null})
        );
    }
}
