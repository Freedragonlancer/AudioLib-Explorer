// app/audiolib-desktop/src/search_v2_ux.rs
//! #209 client UX: the §7.1 local schema 9 (path index) upgrade dialog and the §4
//! vendor table import dialog (server mode, manage rights only). All library work goes
//! through `DataProvider`; this module only drives the UI and worker threads.
use super::{Controller, post};
use crate::AppWindow;
use audiolib_core::DataProvider;
use audiolib_core::local_upgrade::{self as lu, LocalUpgradeOutcome, LocalUpgradePlan};
use audiolib_core::vendor_import::{self as vi, VendorLimits};
use slint::ComponentHandle;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Dialog states shared with `app-window.slint` (`lu-state`).
pub(super) const LU_HIDDEN: i32 = 0;
pub(super) const LU_CONFIRM: i32 = 1;
pub(super) const LU_PROGRESS: i32 = 2;
pub(super) const LU_CANCEL_CONFIRM: i32 = 3;
pub(super) const LU_FAILED: i32 = 4;
pub(super) const LU_NO_SPACE: i32 = 5;

/// `vi-state`: 0 hidden, 1 choose a file, 2 working, 3 preview ready, 4 committed.
const VI_HIDDEN: i32 = 0;
const VI_CHOOSE: i32 = 1;
const VI_BUSY: i32 = 2;
const VI_PREVIEW: i32 = 3;
const VI_COMMITTED: i32 = 4;

struct PendingImport {
    import_id: String,
    /// One key per preview: a retried commit replays the same result (§4.5).
    key: String,
}

#[derive(Default)]
struct UxState {
    /// Library already checked this session, so a declined or cancelled upgrade is not
    /// asked again until the next open.
    offered: Option<PathBuf>,
    cancel: Option<Arc<AtomicBool>>,
    upgrading: bool,
    vendor: Option<PendingImport>,
}

fn ux() -> &'static Mutex<UxState> {
    static STATE: OnceLock<Mutex<UxState>> = OnceLock::new();
    STATE.get_or_init(Default::default)
}

fn idempotency_key(import_id: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("audiolib-{import_id}-{nanos:x}")
}

pub(super) fn limits_text(limits: VendorLimits, chinese: bool) -> String {
    let size = lu::size_text(limits.max_file_bytes);
    let line = lu::size_text(limits.max_row_text_bytes);
    if chinese {
        format!(
            "支持 CSV / XLS / XLSX（按文件头识别）。上限：单文件 {size}、{} 行、每行 {line}；超限整份拒绝。",
            limits.max_rows
        )
    } else {
        format!(
            "CSV / XLS / XLSX (detected from the file header). Limits: {size} per file, {} rows, {line} per row; an oversized file is rejected whole.",
            limits.max_rows
        )
    }
}

impl Controller {
    pub(super) fn bind_search_v2_ux(self: &Arc<Self>, ui: &AppWindow) {
        let c = self.clone();
        ui.on_lu_accept(move || c.start_local_upgrade(false));
        let c = self.clone();
        ui.on_lu_decline(move || {
            if let Some(ui) = c.ui.upgrade() {
                ui.set_lu_state(LU_HIDDEN);
                ui.invoke_open_library(0);
            }
        });
        let c = self.clone();
        ui.on_lu_cancel_requested(move || {
            if let Some(ui) = c.ui.upgrade()
                && ui.get_lu_state() == LU_PROGRESS
            {
                let chinese = c.chinese.load(Ordering::Acquire);
                ui.set_lu_question(lu::cancel_confirm_text(chinese).into());
                ui.set_lu_state(LU_CANCEL_CONFIRM);
            }
        });
        let c = self.clone();
        ui.on_lu_cancel_answer(move |cancel| {
            let Some(ui) = c.ui.upgrade() else { return };
            if cancel && let Some(flag) = ux().lock().unwrap().cancel.as_ref() {
                flag.store(true, Ordering::Release);
            }
            if ui.get_lu_state() == LU_CANCEL_CONFIRM {
                ui.set_lu_state(LU_PROGRESS);
            }
        });
        let c = self.clone();
        ui.on_lu_close(move || {
            if let Some(ui) = c.ui.upgrade() {
                ui.set_lu_state(LU_HIDDEN);
            }
        });

        let c = self.clone();
        ui.on_vendor_import_requested(move || c.open_vendor_import());
        let c = self.clone();
        ui.on_vi_pick(move || c.pick_vendor_table());
        let c = self.clone();
        ui.on_vi_commit(move || c.commit_vendor_import());
        let c = self.clone();
        ui.on_vi_rollback(move || c.rollback_vendor_import());
        let c = self.clone();
        ui.on_vi_close(move || {
            if let Some(ui) = c.ui.upgrade()
                && ui.get_vi_state() != VI_BUSY
            {
                ui.set_vi_state(VI_HIDDEN);
            }
        });
    }

    /// Ask once per opened local library whether the path index (schema 9) is missing.
    /// The plan is read on a worker; nothing runs while another operation holds the
    /// library.
    pub(super) fn maybe_offer_local_upgrade(self: &Arc<Self>, ui: &AppWindow) {
        if self.remote_read_only() || !self.available() || ui.get_lu_state() != LU_HIDDEN {
            return;
        }
        let provider = self.provider();
        let path = provider.database_path().to_path_buf();
        {
            let mut state = ux().lock().unwrap();
            if state.upgrading || state.offered.as_ref() == Some(&path) {
                return;
            }
            state.offered = Some(path);
        }
        let c = self.clone();
        std::thread::spawn(move || {
            let plan = (provider.as_ref() as &dyn DataProvider).local_upgrade_plan();
            let c2 = c.clone();
            post(c.ui.clone(), move |ui| match plan {
                Ok(Some(plan)) => c2.show_local_upgrade(&ui, plan),
                Ok(None) => c2.refresh_path_search_availability(&ui),
                Err(error) => tracing::warn!(%error, "Read local upgrade plan"),
            });
        });
    }

    fn show_local_upgrade(self: &Arc<Self>, ui: &AppWindow, plan: LocalUpgradePlan) {
        if self.remote_read_only() {
            return;
        }
        let chinese = self.chinese.load(Ordering::Acquire);
        ui.set_lu_title(lu::confirm_title(chinese).into());
        let (accept, decline) = lu::confirm_buttons(chinese);
        ui.set_lu_accept_label(accept.into());
        ui.set_lu_decline_label(decline.into());
        ui.set_lu_copy_label(lu::copy_diagnostics_label(chinese).into());
        if plan.insufficient_space() {
            ui.set_lu_body(lu::insufficient_space_text(&plan, chinese).into());
            ui.set_lu_state(LU_NO_SPACE);
        } else if plan.resuming {
            // §6: a resume neither asks again nor backs up again.
            ui.set_lu_body(lu::resume_text(chinese).into());
            self.start_local_upgrade(true);
        } else {
            ui.set_lu_body(lu::confirm_body(&plan, chinese).into());
            ui.set_lu_state(LU_CONFIRM);
        }
    }

    fn start_local_upgrade(self: &Arc<Self>, resuming: bool) {
        let Some(ui) = self.ui.upgrade() else { return };
        if self.remote_read_only()
            || !self.available()
            || self.library_busy.swap(true, Ordering::AcqRel)
        {
            return;
        }
        let chinese = self.chinese.load(Ordering::Acquire);
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let mut state = ux().lock().unwrap();
            state.cancel = Some(cancel.clone());
            state.upgrading = true;
        }
        ui.set_lu_progress(0.0);
        ui.set_lu_progress_text(if resuming {
            lu::resume_text(chinese).into()
        } else {
            "".into()
        });
        ui.set_lu_state(LU_PROGRESS);
        let provider = self.provider();
        let c = self.clone();
        std::thread::spawn(move || {
            let weak = c.ui.clone();
            let mut last: Option<Instant> = None;
            let mut report = |progress: lu::LocalUpgradeProgress| {
                let finished = progress.completed >= progress.total;
                if !finished && last.is_some_and(|at| at.elapsed() < Duration::from_millis(100)) {
                    return;
                }
                last = Some(Instant::now());
                let text = lu::progress_text(&progress, chinese);
                let fraction = f32::from(progress.percent()) / 100.0;
                post(weak.clone(), move |ui| {
                    ui.set_lu_progress_text(text.into());
                    ui.set_lu_progress(fraction);
                });
            };
            let result =
                (provider.as_ref() as &dyn DataProvider).run_local_upgrade(&cancel, &mut report);
            {
                let mut state = ux().lock().unwrap();
                state.cancel = None;
                state.upgrading = false;
            }
            c.library_busy.store(false, Ordering::Release);
            let c2 = c.clone();
            post(c.ui.clone(), move |ui| c2.finish_local_upgrade(&ui, result));
        });
    }

    fn finish_local_upgrade(
        self: &Arc<Self>,
        ui: &AppWindow,
        result: audiolib_core::Result<LocalUpgradeOutcome>,
    ) {
        let chinese = self.chinese.load(Ordering::Acquire);
        match result {
            Ok(LocalUpgradeOutcome::Done { .. }) => {
                ui.set_lu_state(LU_HIDDEN);
                ui.set_status(lu::done_text(chinese).into());
                self.refresh_path_search_availability(ui);
            }
            Ok(LocalUpgradeOutcome::Cancelled) => {
                ui.set_lu_state(LU_HIDDEN);
                ui.set_status(lu::cancelled_text(chinese).into());
            }
            Err(error) => {
                let backup = self
                    .provider()
                    .database_path()
                    .parent()
                    .and_then(latest_backup);
                ui.set_lu_body(lu::failed_text(backup.as_deref(), chinese).into());
                ui.set_lu_diagnostics(
                    format!(
                        "AudioLib Explorer {}\nschema 9 path index upgrade failed\n{error}",
                        env!("CARGO_PKG_VERSION")
                    )
                    .into(),
                );
                ui.set_lu_state(LU_FAILED);
            }
        }
    }

    fn vendor_limits(&self) -> Option<VendorLimits> {
        if !self.remote_read_only() {
            return None;
        }
        self.current_vendor_import()
            .map(|info| VendorLimits::from(&info))
    }

    fn open_vendor_import(self: &Arc<Self>) {
        let Some(ui) = self.ui.upgrade() else { return };
        let chinese = self.chinese.load(Ordering::Acquire);
        let remote = self.remote_read_only();
        match self.vendor_limits() {
            Some(limits) => {
                ui.set_vi_available(true);
                ui.set_vi_text(limits_text(limits, chinese).into());
            }
            None => {
                ui.set_vi_available(false);
                ui.set_vi_text(vi::unavailable_text(remote, chinese).into());
            }
        }
        ux().lock().unwrap().vendor = None;
        ui.set_vi_state(VI_CHOOSE);
    }

    fn pick_vendor_table(self: &Arc<Self>) {
        let Some(ui) = self.ui.upgrade() else { return };
        let Some(limits) = self.vendor_limits() else {
            return;
        };
        let chinese = self.chinese.load(Ordering::Acquire);
        let owner = crate::dialogs::owner_handle(ui.window());
        let path = match crate::dialogs::choose(
            false,
            crate::dialogs::FileKind::VendorTable,
            chinese,
            owner,
        ) {
            Ok(Some(path)) => path,
            Ok(None) => return,
            Err(error) => {
                ui.set_vi_text(error.into());
                return;
            }
        };
        let provider = match self.remote_provider() {
            Ok(provider) => provider,
            Err(error) => {
                ui.set_vi_text(error.into());
                return;
            }
        };
        ui.set_vi_state(VI_BUSY);
        ui.set_vi_text(
            if chinese {
                "正在检查并上传预览…"
            } else {
                "Checking and uploading for preview…"
            }
            .into(),
        );
        let c = self.clone();
        std::thread::spawn(move || {
            let outcome = match vi::precheck(&path, limits) {
                Err(error) => Err(error.to_string()),
                Ok(Err(reason)) => Err(reason.text(chinese)),
                Ok(Ok(_format)) => (provider.as_ref() as &dyn DataProvider)
                    .vendor_import_preview(&path)
                    .map_err(|error| error.to_string()),
            };
            post(c.ui.clone(), move |ui| match outcome {
                Ok(preview) => {
                    ux().lock().unwrap().vendor = Some(PendingImport {
                        key: idempotency_key(&preview.import_id),
                        import_id: preview.import_id.clone(),
                    });
                    ui.set_vi_text(vi::preview_summary(&preview, chinese).into());
                    ui.set_vi_state(VI_PREVIEW);
                }
                Err(text) => {
                    ui.set_vi_text(text.into());
                    ui.set_vi_state(VI_CHOOSE);
                }
            });
        });
    }

    fn commit_vendor_import(self: &Arc<Self>) {
        self.vendor_write(true);
    }

    fn rollback_vendor_import(self: &Arc<Self>) {
        self.vendor_write(false);
    }

    fn vendor_write(self: &Arc<Self>, commit: bool) {
        let Some(ui) = self.ui.upgrade() else { return };
        let chinese = self.chinese.load(Ordering::Acquire);
        let Some((import_id, key)) = ux()
            .lock()
            .unwrap()
            .vendor
            .as_ref()
            .map(|pending| (pending.import_id.clone(), pending.key.clone()))
        else {
            return;
        };
        let provider = match self.remote_provider() {
            Ok(provider) => provider,
            Err(error) => {
                ui.set_vi_text(error.into());
                return;
            }
        };
        let previous = ui.get_vi_state();
        ui.set_vi_state(VI_BUSY);
        let c = self.clone();
        std::thread::spawn(move || {
            let provider = provider.as_ref() as &dyn DataProvider;
            let result = if commit {
                provider.vendor_import_commit(&import_id, &key)
            } else {
                provider.vendor_import_rollback(&import_id)
            };
            let c2 = c.clone();
            post(c.ui.clone(), move |ui| match result {
                Ok(reply) => {
                    if commit {
                        ui.set_vi_text(vi::committed_text(&reply, chinese).into());
                        ui.set_vi_state(VI_COMMITTED);
                    } else {
                        ux().lock().unwrap().vendor = None;
                        ui.set_vi_text(vi::rolled_back_text(&reply, chinese).into());
                        ui.set_vi_state(VI_CHOOSE);
                    }
                    // §7.6: the server revision moved; offer the refresh at once.
                    ui.set_remote_notice(super::remote_mode::refresh_notice_text(chinese).into());
                    ui.set_remote_notice_kind(super::remote_mode::NOTICE_UPDATE);
                    ui.set_remote_notice_refresh(true);
                    c2.wake_revision_poller();
                }
                Err(error) => {
                    ui.set_vi_text(error.to_string().into());
                    ui.set_vi_state(previous);
                }
            });
        });
    }
}

/// Newest `*.pre-schema9-*.db` next to the library, for the failure text.
fn latest_backup(folder: &std::path::Path) -> Option<PathBuf> {
    std::fs::read_dir(folder)
        .ok()?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains(".pre-schema9-") && name.ends_with(".db"))
        })
        .max()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_text_names_all_three_limits() {
        let text = limits_text(VendorLimits::default(), true);
        assert!(text.contains("50.0 MB"), "{text}");
        assert!(text.contains("200000"), "{text}");
        assert!(text.contains("256.0 KB"), "{text}");
    }

    #[test]
    fn idempotency_key_is_stable_per_call_and_names_the_batch() {
        let key = idempotency_key("abc");
        assert!(key.starts_with("audiolib-abc-"));
    }

    #[test]
    fn latest_backup_picks_the_newest_pre_schema9_copy() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["lib.pre-schema9-100.db", "lib.pre-schema9-200.db", "lib.db"] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        let found = latest_backup(dir.path()).unwrap();
        assert!(found.ends_with("lib.pre-schema9-200.db"));
    }
}
