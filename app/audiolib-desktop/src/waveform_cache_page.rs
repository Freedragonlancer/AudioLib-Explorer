// app/audiolib-desktop/src/waveform_cache_page.rs
//! The 「波形缓存」 (Waveform cache) section of the 服务器音效库 page (doc/api/README.md
//! §9.5): run state, progress, rate, ETA (「估算中」 while the server has none), the last
//! error, the auto-generate switch, generation jobs (补齐缺失 / 重试失败 / 全部重建 with a
//! two-step confirmation), pause / resume / stop and the rate setting.
//!
//! Shown only for the active, open server source. Every request goes through the server
//! `DataProvider` on a worker thread; results return to the Slint event loop through
//! `post` and are dropped when the page changed meanwhile. Without the server's
//! `list_waveform_jobs` capability only the legacy switch, status and pre-generation
//! pause/resume are offered. `403 MANAGE_FORBIDDEN` disables the controls until the
//! page reloads; `409 BUSY` / `JOB_STATE` refresh the status and show why.
use super::{Controller, cn_or, post};
use crate::AppWindow;
use audiolib_core::DataProvider;
use audiolib_core::config::ActiveSource;
use audiolib_core::waveform_cache::{
    LIST_WAVEFORM_RATE_RANGE, WaveformCacheCapabilities, WaveformCacheError,
    WaveformCacheErrorKind, WaveformCacheResult, WaveformCacheSettings, WaveformCacheSettingsPatch,
    WaveformCacheStatus, WaveformJobControl, WaveformJobMode, WaveformRunState,
};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

/// Status poll interval while the section is open (only then; never from audio code).
pub(super) const WAVEFORM_CACHE_POLL: Duration = Duration::from_secs(2);

/// What the section last learned about the active server (kept for the polls).
#[derive(Clone, Debug, Default)]
pub(super) struct WaveformCacheState {
    pub(super) capabilities: Option<WaveformCacheCapabilities>,
    pub(super) settings: Option<WaveformCacheSettings>,
    /// A `403 MANAGE_FORBIDDEN` was answered since the page loaded.
    pub(super) forbidden: bool,
    /// The library page generation this state belongs to.
    pub(super) generation: Option<u64>,
}

impl WaveformCacheState {
    /// The state after a reading for page `generation`. A reload of the same page (e.g.
    /// after a scan finished) keeps the 403 state; the flag tells whether the page is new,
    /// so the rebuild confirmation and the rate draft are reset only then.
    pub(super) fn reloaded(
        &self,
        generation: u64,
        capabilities: Option<WaveformCacheCapabilities>,
        settings: Option<WaveformCacheSettings>,
    ) -> (Self, bool) {
        let same = self.generation == Some(generation);
        (
            Self {
                capabilities,
                settings,
                forbidden: same && self.forbidden,
                generation: Some(generation),
            },
            !same,
        )
    }

    fn jobs(&self) -> bool {
        self.capabilities
            .as_ref()
            .is_some_and(|capabilities| capabilities.jobs.is_some())
    }

    fn can_manage(&self) -> bool {
        !self.forbidden
            && self
                .capabilities
                .as_ref()
                .is_some_and(|capabilities| capabilities.can_manage)
    }
}

/// One user request from the section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WaveformCacheAction {
    /// Start a job. The flag is `confirm_rebuild`: `true` only from the UI's second
    /// 全部重建 confirmation, so the provider's and client's rebuild guards still apply
    /// to every other path.
    Start(WaveformJobMode, bool),
    Control(WaveformJobControl),
    /// `None` restores the server's configured rate.
    Rate(Option<u32>),
    Pregenerate(bool),
}

/// Everything the section displays, computed off the Slint types (unit-tested).
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct WaveformCacheView {
    pub(super) jobs: bool,
    pub(super) can_manage: bool,
    pub(super) rate_setting: bool,
    pub(super) offers_missing: bool,
    pub(super) offers_failed: bool,
    pub(super) offers_rebuild: bool,
    pub(super) job_active: bool,
    pub(super) paused: bool,
    pub(super) has_failed: bool,
    pub(super) run_state: String,
    pub(super) progress: f32,
    pub(super) progress_text: String,
    pub(super) rate_text: String,
    pub(super) job_text: String,
    pub(super) last_error: String,
}

fn duration_text(seconds: u64, chinese: bool) -> String {
    let (hours, minutes, secs) = (seconds / 3600, seconds % 3600 / 60, seconds % 60);
    match (hours, minutes, chinese) {
        (0, 0, true) => format!("{secs} 秒"),
        (0, 0, false) => format!("{secs} s"),
        (0, _, true) => format!("{minutes} 分 {secs} 秒"),
        (0, _, false) => format!("{minutes} min {secs} s"),
        (_, _, true) => format!("{hours} 小时 {minutes} 分"),
        (_, _, false) => format!("{hours} h {minutes} min"),
    }
}

fn mode_text(mode: &str, chinese: bool) -> String {
    match (mode, chinese) {
        ("missing", true) => "补齐缺失".into(),
        ("missing", false) => "fill missing".into(),
        ("rebuild", true) => "全部重建".into(),
        ("rebuild", false) => "rebuild all".into(),
        ("failed", true) => "重试失败".into(),
        ("failed", false) => "retry failed".into(),
        (other, _) => other.into(),
    }
}

fn job_state_text(state: &str, chinese: bool) -> String {
    match (state, chinese) {
        ("running", true) => "进行中".into(),
        ("yielding", true) => "让路中".into(),
        ("paused", true) => "已暂停".into(),
        ("completed", true) => "已完成".into(),
        ("cancelled", true) => "已停止".into(),
        ("error", true) => "出错".into(),
        (other, _) => other.into(),
    }
}

/// The section's display values for `status`.
pub(super) fn waveform_cache_view(
    state: &WaveformCacheState,
    status: &WaveformCacheStatus,
    chinese: bool,
) -> WaveformCacheView {
    let jobs = state
        .capabilities
        .as_ref()
        .and_then(|capabilities| capabilities.jobs.as_ref());
    let run = status.effective_run_state();
    let yielding = status.state == "yielding";
    let run_state = match (run, chinese) {
        (WaveformRunState::Idle, true) => "空闲".to_owned(),
        (WaveformRunState::Idle, false) => "idle".to_owned(),
        (WaveformRunState::Running, true) if yielding => match status.yield_reason.as_deref() {
            Some("audio") => "运行中（为试听让路）".to_owned(),
            Some("scan") => "运行中（为扫描让路）".to_owned(),
            _ => "运行中（让路）".to_owned(),
        },
        (WaveformRunState::Running, false) if yielding => match status.yield_reason.as_deref() {
            Some(reason) => format!("running (yielding: {reason})"),
            None => "running (yielding)".to_owned(),
        },
        (WaveformRunState::Running, true) => "运行中".to_owned(),
        (WaveformRunState::Running, false) => "running".to_owned(),
        (WaveformRunState::Paused, true) => "已暂停".to_owned(),
        (WaveformRunState::Paused, false) => "paused".to_owned(),
        (WaveformRunState::Error, true) => "出错".to_owned(),
        (WaveformRunState::Error, false) => "error".to_owned(),
        (WaveformRunState::Unknown, true) => "未知".to_owned(),
        (WaveformRunState::Unknown, false) => "unknown".to_owned(),
    };
    let mut progress = vec![if chinese {
        format!("已缓存 {} / {}", status.cached, status.total)
    } else {
        format!("cached {} / {}", status.cached, status.total)
    }];
    if let Some(pending) = status.effective_pending() {
        progress.push(if chinese {
            format!("待生成 {pending}")
        } else {
            format!("{pending} pending")
        });
    }
    if status.failed > 0 {
        progress.push(if chinese {
            format!("失败 {}", status.failed)
        } else {
            format!("{} failed", status.failed)
        });
    }
    if status.skipped_offline > 0 {
        progress.push(if chinese {
            format!("根离线跳过 {}", status.skipped_offline)
        } else {
            format!("{} skipped (root offline)", status.skipped_offline)
        });
    }
    let rate_text = if run == WaveformRunState::Running || status.current_rate.is_some() {
        let rate = status.current_rate.unwrap_or(0.0);
        let eta = match (status.eta_secs, chinese) {
            (Some(seconds), _) => duration_text(seconds, chinese),
            (None, true) => "估算中".to_owned(),
            (None, false) => "estimating".to_owned(),
        };
        let limit = state
            .settings
            .as_ref()
            .and_then(|settings| settings.list_waveform_rate)
            .unwrap_or(status.rate);
        if chinese {
            format!("速度 {rate:.1} 个/秒（限速 {limit}）· 预计剩余 {eta}")
        } else {
            format!("rate {rate:.1} files/s (limit {limit}) · ETA {eta}")
        }
    } else {
        String::new()
    };
    let job_text = status.job.as_ref().map_or_else(String::new, |job| {
        let mut text = if chinese {
            format!(
                "任务（{}）：{} · {} / {} · 失败 {}",
                mode_text(&job.mode, true),
                job_state_text(&job.state, true),
                job.done,
                job.total,
                job.failed
            )
        } else {
            format!(
                "Job ({}): {} · {} / {} · {} failed",
                mode_text(&job.mode, false),
                job_state_text(&job.state, false),
                job.done,
                job.total,
                job.failed
            )
        };
        if job.skipped_offline > 0 {
            text.push_str(&if chinese {
                format!(" · 根离线跳过 {}", job.skipped_offline)
            } else {
                format!(" · {} skipped (root offline)", job.skipped_offline)
            });
        }
        if job.fell_back() {
            text.push_str(&if chinese {
                format!("（已按{}执行）", mode_text(&job.mode_effective, true))
            } else {
                format!(" (ran as {})", mode_text(&job.mode_effective, false))
            });
        }
        text
    });
    let last_error = status
        .last_error
        .as_ref()
        .map_or_else(String::new, |error| {
            let record = error.id.map(|id| format!(" #{id}")).unwrap_or_default();
            if chinese {
                format!("最近错误{record}：{} {}", error.error_code, error.message)
            } else {
                format!("Last error{record}: {} {}", error.error_code, error.message)
            }
        });
    let failed = status
        .failed
        .max(status.job.as_ref().map_or(0, |job| job.failed));
    WaveformCacheView {
        jobs: jobs.is_some(),
        can_manage: state.can_manage(),
        rate_setting: jobs.is_some_and(|jobs| jobs.rate_setting),
        offers_missing: jobs.is_some_and(|jobs| jobs.offers(WaveformJobMode::Missing)),
        offers_failed: jobs.is_some_and(|jobs| jobs.offers(WaveformJobMode::Failed)),
        offers_rebuild: jobs.is_some_and(|jobs| jobs.offers(WaveformJobMode::Rebuild)),
        job_active: status.job_in_progress(),
        paused: run == WaveformRunState::Paused,
        has_failed: failed > 0,
        run_state,
        progress: status.progress(),
        progress_text: progress.join(" · "),
        rate_text,
        job_text,
        last_error,
    }
}

/// The rate field's initial text: the effective rate unless it is the built-in default.
pub(super) fn rate_draft(settings: Option<&WaveformCacheSettings>) -> String {
    settings
        .filter(|settings| settings.list_waveform_rate_source.as_deref() != Some("default"))
        .and_then(|settings| settings.list_waveform_rate)
        .map(|rate| rate.to_string())
        .unwrap_or_default()
}

/// Parse the rate field: empty restores the default, otherwise 1..=1000.
pub(super) fn parse_rate(text: &str, chinese: bool) -> Result<Option<u32>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    match text.parse::<u32>() {
        Ok(rate) if LIST_WAVEFORM_RATE_RANGE.contains(&rate) => Ok(Some(rate)),
        _ => Err(cn_or(
            chinese,
            "限速须为 1–1000 的整数（留空 = 恢复默认）。",
            "The rate must be a whole number 1–1000 (empty = default).",
        )),
    }
}

fn apply_view(ui: &AppWindow, view: &WaveformCacheView) {
    ui.set_sm_wc_jobs(view.jobs);
    ui.set_sm_wc_can_manage(view.can_manage);
    ui.set_sm_wc_rate_setting(view.rate_setting);
    ui.set_sm_wc_offers_missing(view.offers_missing);
    ui.set_sm_wc_offers_failed(view.offers_failed);
    ui.set_sm_wc_offers_rebuild(view.offers_rebuild);
    ui.set_sm_wc_job_active(view.job_active);
    ui.set_sm_wc_paused(view.paused);
    ui.set_sm_wc_has_failed(view.has_failed);
    ui.set_sm_wc_run_state(view.run_state.as_str().into());
    ui.set_sm_wc_progress(view.progress);
    ui.set_sm_wc_progress_text(view.progress_text.as_str().into());
    ui.set_sm_wc_rate_text(view.rate_text.as_str().into());
    ui.set_sm_wc_job_text(view.job_text.as_str().into());
    ui.set_sm_wc_last_error(view.last_error.as_str().into());
}

/// The first reading of the section (all through the provider).
pub(super) struct WaveformCacheFetch {
    capabilities: WaveformCacheResult<WaveformCacheCapabilities>,
    settings: WaveformCacheResult<WaveformCacheSettings>,
    status: WaveformCacheResult<WaveformCacheStatus>,
}

impl WaveformCacheFetch {
    pub(super) fn fetch(provider: &dyn DataProvider) -> Self {
        Self {
            capabilities: provider.waveform_cache_capabilities(),
            settings: provider.waveform_cache_settings(),
            status: provider.waveform_cache_status(),
        }
    }
}

impl Controller {
    /// The active server's provider when `profile_id` is the active, open server source;
    /// `None` hides the section. Called on worker threads.
    pub(super) fn waveform_cache_provider(
        &self,
        profile_id: &str,
    ) -> Option<Arc<dyn DataProvider>> {
        let active = self.config.lock().unwrap().data_source.active_source()
            == ActiveSource::Profile(profile_id.to_owned());
        if !active || !self.remote_read_only() {
            return None;
        }
        self.remote_list_waveform_provider()
            .map(|provider| provider as Arc<dyn DataProvider>)
    }

    /// Publish the first reading (UI thread); remembers capability and settings.
    pub(super) fn publish_waveform_cache_fetch(
        &self,
        ui: &AppWindow,
        fetch: WaveformCacheFetch,
        generation: u64,
        chinese: bool,
    ) {
        let (state, new_page) = {
            let mut remote = self.remote.lock().unwrap();
            let (state, new_page) = remote.manager.wave_cache.reloaded(
                generation,
                fetch.capabilities.as_ref().ok().cloned(),
                fetch.settings.as_ref().ok().cloned(),
            );
            remote.manager.wave_cache = state.clone();
            (state, new_page)
        };
        ui.set_sm_wc_visible(fetch.capabilities.is_ok());
        if new_page {
            ui.set_sm_wc_confirm(0);
            ui.set_sm_wc_rate_draft(rate_draft(state.settings.as_ref()).into());
        }
        if let Some(settings) = &state.settings {
            ui.set_sm_wave_pregenerate(settings.pregenerate_list_waveforms);
            ui.set_sm_wave_can_change(settings.can_change && state.can_manage());
        }
        let mut message = Vec::new();
        for error in [
            fetch.capabilities.as_ref().err(),
            fetch.settings.as_ref().err(),
        ]
        .into_iter()
        .flatten()
        {
            message.push(error.message.clone());
        }
        self.publish_waveform_cache_status(ui, &state, &fetch.status, chinese);
        if !message.is_empty() {
            ui.set_sm_wave_message(message.join("\n").into());
        }
    }

    /// Show one status reading (UI thread).
    pub(super) fn publish_waveform_cache_status(
        &self,
        ui: &AppWindow,
        state: &WaveformCacheState,
        status: &WaveformCacheResult<WaveformCacheStatus>,
        chinese: bool,
    ) {
        match status {
            Ok(status) => {
                apply_view(ui, &waveform_cache_view(state, status, chinese));
                ui.set_sm_wave_error(false);
                ui.set_sm_wave_status(super::wave_status_text(status, chinese).into());
            }
            Err(error) => {
                apply_view(
                    ui,
                    &WaveformCacheView {
                        jobs: state.jobs(),
                        can_manage: state.can_manage(),
                        ..WaveformCacheView::default()
                    },
                );
                ui.set_sm_wave_error(true);
                ui.set_sm_wave_status(
                    if chinese {
                        format!("读取波形缓存状态失败：{}", error.message)
                    } else {
                        format!(
                            "Could not read the Waveform cache status: {}",
                            error.message
                        )
                    }
                    .into(),
                );
            }
        }
    }

    /// Poll `GET /waveforms/status` through `provider` every [`WAVEFORM_CACHE_POLL`] while
    /// this page generation stays open (its own thread; stops on any page change).
    pub(super) fn poll_waveform_cache(
        self: &Arc<Self>,
        generation: u64,
        provider: Arc<dyn DataProvider>,
        chinese: bool,
    ) {
        let poll = {
            let mut remote = self.remote.lock().unwrap();
            remote.manager.wave_poll += 1;
            remote.manager.wave_poll
        };
        let c = self.clone();
        let current = move |c: &Self| {
            c.library_current(generation)
                && c.remote.lock().unwrap().manager.wave_poll == poll
                && !c.shutdown.load(Ordering::Acquire)
        };
        let spawned = std::thread::Builder::new()
            .name("audiolib-waveform-cache".into())
            .spawn(move || {
                loop {
                    let deadline = std::time::Instant::now() + WAVEFORM_CACHE_POLL;
                    while std::time::Instant::now() < deadline {
                        std::thread::sleep(Duration::from_millis(100));
                        if !current(&c) {
                            return;
                        }
                    }
                    let status = provider.waveform_cache_status();
                    let c2 = c.clone();
                    post(c.ui.clone(), move |ui| {
                        if c2.library_current(generation) {
                            let state = c2.remote.lock().unwrap().manager.wave_cache.clone();
                            c2.publish_waveform_cache_status(&ui, &state, &status, chinese);
                        }
                    });
                }
            });
        if let Err(error) = spawned {
            tracing::warn!(%error, "could not start the Waveform cache status poll");
        }
    }

    /// Run one section request on a worker thread, then refresh the status.
    pub(super) fn run_waveform_cache_action(self: &Arc<Self>, action: WaveformCacheAction) {
        let chinese = self.chinese.load(Ordering::Acquire);
        let generation = self.remote.lock().unwrap().manager.library_generation;
        let Some(profile) = self.selected_profile() else {
            return;
        };
        let state = self.remote.lock().unwrap().manager.wave_cache.clone();
        if !state.can_manage() {
            post(self.ui.clone(), move |ui| {
                ui.set_sm_wave_message(
                    cn_or(
                        chinese,
                        "服务器不允许本机管理波形缓存，只能查看。",
                        "The server does not allow this PC to manage the Waveform cache; view only.",
                    )
                    .into(),
                );
            });
            return;
        }
        post(self.ui.clone(), |ui| {
            ui.set_sm_wave_busy(true);
            ui.set_sm_wave_message("".into());
        });
        let c = self.clone();
        std::thread::spawn(move || {
            let Some(provider) = c.waveform_cache_provider(&profile.id) else {
                post(c.ui.clone(), move |ui| {
                    ui.set_sm_wave_busy(false);
                    ui.set_sm_wave_message(
                        cn_or(
                            chinese,
                            "只有当前已打开的服务器数据源可以管理波形缓存。",
                            "Only the active, open server source can manage its Waveform cache.",
                        )
                        .into(),
                    );
                });
                return;
            };
            let mut settings = None;
            let outcome: WaveformCacheResult<Option<WaveformCacheStatus>> = match action {
                WaveformCacheAction::Start(mode, confirm_rebuild) => provider
                    .start_waveform_cache_job(mode, confirm_rebuild)
                    .map(Some),
                WaveformCacheAction::Control(control) if state.jobs() => {
                    provider.control_waveform_cache_job(control).map(Some)
                }
                WaveformCacheAction::Control(control) => provider
                    .control_waveform_pregeneration(control)
                    .map(|()| None),
                WaveformCacheAction::Rate(rate) => provider
                    .update_waveform_cache_settings(&WaveformCacheSettingsPatch {
                        list_waveform_rate: Some(rate),
                        ..Default::default()
                    })
                    .map(|changed| {
                        settings = Some(changed);
                        None
                    }),
                WaveformCacheAction::Pregenerate(enabled) => provider
                    .update_waveform_cache_settings(&WaveformCacheSettingsPatch {
                        pregenerate_list_waveforms: Some(enabled),
                        ..Default::default()
                    })
                    .map(|changed| {
                        settings = Some(changed);
                        None
                    }),
            };
            let (status, error) = match outcome {
                Ok(Some(status)) => (Ok(status), None),
                // Settings, legacy controls, BUSY and JOB_STATE: show the current status.
                Ok(None) => (provider.waveform_cache_status(), None),
                Err(error) => (provider.waveform_cache_status(), Some(error)),
            };
            let c2 = c.clone();
            post(c.ui.clone(), move |ui| {
                if !c2.library_current(generation) {
                    return;
                }
                ui.set_sm_wave_busy(false);
                let state = {
                    let mut remote = c2.remote.lock().unwrap();
                    let state = &mut remote.manager.wave_cache;
                    if let Some(settings) = settings {
                        state.settings = Some(settings);
                    }
                    if error
                        .as_ref()
                        .is_some_and(|error| error.kind == WaveformCacheErrorKind::Forbidden)
                    {
                        state.forbidden = true;
                    }
                    state.clone()
                };
                if let Some(settings) = &state.settings {
                    ui.set_sm_wave_pregenerate(settings.pregenerate_list_waveforms);
                    ui.set_sm_wave_can_change(settings.can_change && state.can_manage());
                }
                if let Some(error) = &error {
                    if let WaveformCacheAction::Pregenerate(enabled) = action {
                        ui.set_sm_wave_pregenerate(!enabled);
                    }
                    ui.set_sm_wave_message(action_error_text(error, chinese).into());
                }
                c2.publish_waveform_cache_status(&ui, &state, &status, chinese);
            });
        });
    }
}

fn action_error_text(error: &WaveformCacheError, chinese: bool) -> String {
    match error.kind {
        WaveformCacheErrorKind::Busy | WaveformCacheErrorKind::JobState => error.message.clone(),
        _ if chinese => format!("波形缓存操作失败：{}", error.message),
        _ => format!("The Waveform cache request failed: {}", error.message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use audiolib_core::waveform_cache::{
        WaveformCacheJob, WaveformCacheLastError, WaveformJobsCapability,
    };

    fn state(jobs: bool, can_manage: bool) -> WaveformCacheState {
        WaveformCacheState {
            capabilities: Some(WaveformCacheCapabilities {
                jobs: jobs.then(|| WaveformJobsCapability {
                    version: 1,
                    modes: vec!["missing".into(), "failed".into(), "rebuild".into()],
                    rate_setting: true,
                }),
                can_manage,
            }),
            settings: Some(WaveformCacheSettings {
                list_waveform_rate: Some(20),
                ..Default::default()
            }),
            forbidden: false,
            generation: Some(1),
        }
    }

    fn running() -> WaveformCacheStatus {
        WaveformCacheStatus {
            state: "running".into(),
            total: 100,
            cached: 40,
            pending: Some(60),
            run_state: Some(WaveformRunState::Running),
            current_rate: Some(9.64),
            eta_secs: None,
            failed: 2,
            job: Some(WaveformCacheJob {
                id: "wfj-1".into(),
                mode: "failed".into(),
                mode_effective: "missing".into(),
                state: "running".into(),
                total: 100,
                done: 50,
                failed: 2,
                skipped_offline: 3,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn eta_shows_estimating_until_the_server_has_one() {
        let view = waveform_cache_view(&state(true, true), &running(), true);
        assert_eq!(view.run_state, "运行中");
        assert_eq!(view.rate_text, "速度 9.6 个/秒（限速 20）· 预计剩余 估算中");
        assert_eq!(view.progress_text, "已缓存 40 / 100 · 待生成 60 · 失败 2");
        assert!((view.progress - 0.5).abs() < 1e-6, "job progress");
        assert!(view.job_active && view.has_failed && !view.paused);
        assert!(
            view.job_text.contains("（已按补齐缺失执行）"),
            "{}",
            view.job_text
        );
        assert!(view.job_text.contains("根离线跳过 3"), "{}", view.job_text);
        let mut known = running();
        known.eta_secs = Some(3725);
        let view = waveform_cache_view(&state(true, true), &known, false);
        assert!(
            view.rate_text.ends_with("ETA 1 h 2 min"),
            "{}",
            view.rate_text
        );
    }

    #[test]
    fn legacy_servers_offer_only_switch_status_and_pause() {
        let status = WaveformCacheStatus {
            state: "paused".into(),
            total: 10,
            cached: 5,
            remaining: Some(5),
            ..Default::default()
        };
        let view = waveform_cache_view(&state(false, true), &status, false);
        assert!(!view.jobs && !view.rate_setting);
        assert!(!view.offers_missing && !view.offers_failed && !view.offers_rebuild);
        assert!(view.paused && view.can_manage);
        assert_eq!(view.run_state, "paused");
        assert_eq!(view.rate_text, "");
    }

    #[test]
    fn forbidden_or_no_manage_disables_controls() {
        let mut forbidden = state(true, true);
        forbidden.forbidden = true;
        assert!(!waveform_cache_view(&forbidden, &running(), true).can_manage);
        assert!(!waveform_cache_view(&state(true, false), &running(), true).can_manage);
    }

    #[test]
    fn last_error_and_idle_state_are_shown() {
        let status = WaveformCacheStatus {
            state: "idle".into(),
            run_state: Some(WaveformRunState::Error),
            last_error: Some(WaveformCacheLastError {
                at: Some(1),
                error_code: "AUDIO".into(),
                message: "decode failed".into(),
                id: Some(106),
            }),
            ..Default::default()
        };
        let view = waveform_cache_view(&state(true, true), &status, true);
        assert_eq!(view.run_state, "出错");
        assert_eq!(view.last_error, "最近错误 #106：AUDIO decode failed");
        assert_eq!(view.progress, 0.0);
    }

    #[test]
    fn reload_keeps_forbidden_and_drafts_only_on_the_same_page() {
        let mut loaded = state(true, true);
        loaded.forbidden = true;
        let (same, new_page) = loaded.reloaded(1, loaded.capabilities.clone(), None);
        assert!(same.forbidden && !new_page);
        let (next, new_page) = loaded.reloaded(2, loaded.capabilities.clone(), None);
        assert!(!next.forbidden && new_page);
        assert_eq!(next.generation, Some(2));
        let (first, new_page) = WaveformCacheState::default().reloaded(1, None, None);
        assert!(!first.forbidden && new_page);
    }

    #[test]
    fn rate_draft_hides_the_default() {
        let mut settings = WaveformCacheSettings {
            list_waveform_rate: Some(10),
            list_waveform_rate_source: Some("default".into()),
            ..Default::default()
        };
        assert_eq!(rate_draft(Some(&settings)), "");
        settings.list_waveform_rate_source = Some("config".into());
        assert_eq!(rate_draft(Some(&settings)), "10");
        assert_eq!(rate_draft(None), "");
    }

    #[test]
    fn rate_field_accepts_empty_or_range() {
        assert_eq!(parse_rate(" ", true), Ok(None));
        assert_eq!(parse_rate("1", true), Ok(Some(1)));
        assert_eq!(parse_rate("1000", true), Ok(Some(1000)));
        assert!(parse_rate("0", true).is_err());
        assert!(parse_rate("1001", false).is_err());
        assert!(parse_rate("abc", false).is_err());
    }
}
