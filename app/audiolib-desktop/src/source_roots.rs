// app/audiolib-desktop/src/source_roots.rs
//! Remote root editing and the server directory picker. Requests retain the displayed
//! server identity, directory replies use a navigation sequence, and mutations reuse
//! the library operation gate so requested Metadata writes drain before server changes.
use super::{Controller, LibraryOperation, PAGE_LIBRARY, cn_or, post};
use crate::{AppWindow, RemoteBreadcrumbRow, RemoteFolderRow};
use audiolib_core::DataProvider;
use audiolib_remote::{CacheState, RemoteClient, RemoteDirectory, RemoteError, RemoteRoot};
use slint::{ModelRc, VecModel};
use std::sync::{Arc, Mutex, atomic::Ordering};

#[cfg(test)]
#[path = "source_roots_tests.rs"]
mod controller_tests;

#[derive(Clone)]
enum RootAction {
    Add { name: String, path: String },
    Update { name: String, path: String },
    Remove { name: String, path: String },
}

/// Captured server identity and operation, executed only after the GUI write drain.
#[derive(Clone)]
pub(in crate::controller) struct RootOperation {
    client: RemoteClient,
    snapshot: CacheState,
    action: RootAction,
    generation: u64,
    request: u64,
    error: Arc<Mutex<Option<RemoteError>>>,
}

impl Controller {
    pub(super) fn bind_source_roots(self: &Arc<Self>, ui: &AppWindow) {
        let c = self.clone();
        ui.on_sm_root_add_requested(move || c.open_root_form(false));
        let c = self.clone();
        ui.on_sm_root_edit_requested(move || c.open_root_form(true));
        let c = self.clone();
        ui.on_sm_root_save_requested(move || c.save_root_form());
        let c = self.clone();
        ui.on_sm_root_delete_requested(move || {
            let Some(ui) = c.ui.upgrade() else { return };
            if let Some(root) = c.selected_root(&ui) {
                c.submit_root(RootAction::Remove {
                    name: root.name,
                    path: root.server_path,
                });
            }
        });
        let c = self.clone();
        ui.on_sm_fs_open_requested(move || {
            let Some(ui) = c.ui.upgrade() else { return };
            if !ui.get_sm_root_controls() || ui.get_sm_manage_busy() {
                return;
            }
            ui.set_sm_fs_visible(true);
            c.browse_server_folder(ui.get_sm_root_path().to_string());
        });
        let c = self.clone();
        ui.on_sm_fs_navigate(move |path| c.browse_server_folder(path.to_string()));
        let c = self.clone();
        ui.on_sm_fs_closed(move || {
            c.remote.lock().unwrap().manager.browser_sequence += 1;
            if let Some(ui) = c.ui.upgrade() {
                ui.set_sm_fs_visible(false);
                ui.set_sm_fs_busy(false);
            }
        });
    }

    fn selected_root(&self, ui: &AppWindow) -> Option<RemoteRoot> {
        let index = usize::try_from(ui.get_sm_root_selected()).ok()?;
        self.remote
            .lock()
            .unwrap()
            .manager
            .roots
            .get(index)
            .cloned()
    }

    fn open_root_form(self: &Arc<Self>, editing: bool) {
        let Some(ui) = self.ui.upgrade() else { return };
        if !ui.get_sm_root_controls()
            || ui.get_sm_manage_busy()
            || ui.get_sm_scan_active()
            || ui.get_ds_syncing()
            || !self.available()
        {
            return;
        }
        let root = editing.then(|| self.selected_root(&ui)).flatten();
        if editing && root.is_none() {
            return;
        }
        self.remote.lock().unwrap().manager.root_edit_target =
            root.as_ref().map(|r| r.name.clone());
        ui.set_sm_root_editing(editing);
        ui.set_sm_root_name(
            root.as_ref()
                .map(|r| r.name.clone())
                .unwrap_or_default()
                .into(),
        );
        ui.set_sm_root_path(root.map(|r| r.server_path).unwrap_or_default().into());
        ui.set_sm_root_message("".into());
        ui.set_sm_confirm(0);
        ui.set_sm_root_form(true);
    }

    fn save_root_form(self: &Arc<Self>) {
        let Some(ui) = self.ui.upgrade() else { return };
        let name = ui.get_sm_root_name().trim().to_owned();
        let path = ui.get_sm_root_path().to_string();
        let action = match self.remote.lock().unwrap().manager.root_edit_target.clone() {
            Some(name) => RootAction::Update { name, path },
            None => RootAction::Add { name, path },
        };
        self.submit_root(action);
    }

    fn submit_root(self: &Arc<Self>, action: RootAction) {
        let Some(ui) = self.ui.upgrade() else { return };
        if !ui.get_sm_root_controls()
            || ui.get_sm_manage_busy()
            || ui.get_sm_scan_active()
            || ui.get_ds_syncing()
            || !self.available()
        {
            return;
        }
        let (client, snapshot) = match self.active_manage_client() {
            Ok((_, client, snapshot)) => (client, snapshot),
            Err(error) => {
                ui.set_sm_root_message(error.into());
                return;
            }
        };
        let (generation, request) = self.begin_manage_request(&ui);
        ui.set_sm_root_message("".into());
        ui.set_sm_confirm(0);
        self.run_library_operation(LibraryOperation::ServerRoot(Box::new(RootOperation {
            client,
            snapshot,
            action,
            generation,
            request,
            error: Arc::new(Mutex::new(None)),
        })));
        // A draft enqueue failure prevents the operation from acquiring its gate.
        if !self.library_busy.load(Ordering::Acquire) {
            self.finish_manage_request(&ui, request);
            ui.set_sm_root_message(ui.get_management_report());
        }
    }

    pub(in crate::controller) fn execute_server_root(
        &self,
        operation: &RootOperation,
    ) -> Result<String, String> {
        let chinese = self.chinese.load(Ordering::Acquire);
        self.require_remote_write_identity()?;
        if !self.remote_snapshot_current(&operation.snapshot) {
            return Err(cn_or(
                chinese,
                "服务器素材库已变化，请刷新后重试。",
                "The server library changed; refresh and retry.",
            ));
        }
        let result = match &operation.action {
            RootAction::Add { name, path } => operation.client.add_root(name, path).map(|root| {
                if chinese { format!("已添加根目录「{}」。选择该目录并开始扫描即可入库。", root.name) }
                else { format!("Added root \"{}\". Select it and start a scan to index its sounds.", root.name) }
            }),
            RootAction::Update { name, path } => operation.client.update_root_path(name, path).map(|root| {
                if chinese { format!("已更新「{}」的资源路径：{}。文件 ID、Metadata 和标注已保留。", root.name, root.server_path) }
                else { format!("Updated the resource path of \"{}\": {}. File IDs, Metadata and annotations were retained.", root.name, root.server_path) }
            }),
            RootAction::Remove { name, path } => operation.client.remove_root(name).map(|report| {
                // The cached snapshot remains immutable while the fresh snapshot loads.
                let provider = self.provider();
                let mut pending = vec![None];
                while let Some(parent) = pending.pop() {
                    for folder in provider.folders(parent).unwrap_or_default() {
                        if same_server_path(&folder.path, path) {
                            self.hide_removed(&[], &self.removed_folder_subtree(folder.id), report.removed);
                            pending.clear();
                            break;
                        }
                        pending.push(Some(folder.id));
                    }
                }
                if chinese { format!("已删除根目录「{}」及 {} 条库记录。服务器上的目录和 WAV 文件已保留。", report.name, report.removed) }
                else { format!("Removed root \"{}\" and {} library records. Server folders and WAV files were retained.", report.name, report.removed) }
            }),
        };
        result.map_err(|error| {
            let message = Self::manage_text(&error, chinese);
            *operation.error.lock().unwrap() = Some(error);
            message
        })
    }

    pub(in crate::controller) fn server_root_finished(
        self: &Arc<Self>,
        ui: &AppWindow,
        operation: &RootOperation,
        succeeded: bool,
    ) {
        let current = self.finish_manage_request(ui, operation.request)
            && self.library_current(operation.generation);
        let error = operation.error.lock().unwrap().take();
        if let Some(error) = &error {
            self.note_remote_identity_error(ui, &operation.snapshot, error);
        }
        if succeeded {
            self.invalidate_server_ids(None);
            if current {
                ui.set_sm_root_form(false);
                ui.set_sm_manage_error(false);
                ui.set_sm_manage_message(ui.get_management_report());
                self.show_page(PAGE_LIBRARY);
            }
            // Root configuration can change without changing the audio revision. A
            // forced snapshot refresh updates both path mappings and cached rows.
            if !self.close_after_operation.load(Ordering::Acquire) {
                self.start_sync(true, false);
            }
        } else if current {
            ui.set_sm_root_message(ui.get_management_report());
            ui.set_sm_manage_error(true);
            ui.set_sm_manage_message(ui.get_management_report());
        }
        if error.as_ref().is_some_and(root_needs_refresh) {
            // A lost response may follow a committed server mutation. Never resend
            // it automatically; reconcile the snapshot before the next user action.
            self.start_sync(true, false);
        }
    }

    fn browse_server_folder(self: &Arc<Self>, path: String) {
        let Some(ui) = self.ui.upgrade() else { return };
        if !ui.get_sm_fs_visible() || !ui.get_sm_root_controls() || !self.available() {
            return;
        }
        let chinese = self.chinese.load(Ordering::Acquire);
        let (client, snapshot) = match self.active_manage_client() {
            Ok((_, client, snapshot)) => (client, snapshot),
            Err(error) => {
                ui.set_sm_fs_error(error.into());
                return;
            }
        };
        let (generation, sequence) = {
            let mut remote = self.remote.lock().unwrap();
            remote.manager.browser_sequence += 1;
            (
                remote.manager.library_generation,
                remote.manager.browser_sequence,
            )
        };
        ui.set_sm_fs_busy(true);
        ui.set_sm_fs_can_select(false);
        ui.set_sm_fs_error("".into());
        let c = self.clone();
        std::thread::spawn(move || {
            let result =
                client.browse_directory((!path.is_empty()).then_some(path.as_str()), false);
            let c2 = c.clone();
            post(c.ui.clone(), move |ui| {
                let current = {
                    let remote = c2.remote.lock().unwrap();
                    remote.manager.library_generation == generation
                        && remote.manager.browser_sequence == sequence
                };
                if !current || !ui.get_sm_fs_visible() || !c2.remote_snapshot_current(&snapshot) {
                    return;
                }
                ui.set_sm_fs_busy(false);
                match result {
                    Ok(directory) => publish_directory(&ui, directory, chinese),
                    Err(error) => ui.set_sm_fs_error(Self::manage_text(&error, chinese).into()),
                }
            });
        });
    }
}

// POSIX directory names retain their case and may contain literal backslashes.
// Only Windows drive/UNC paths use case-insensitive separator normalization.
fn same_server_path(left: &str, right: &str) -> bool {
    fn windows_path(path: &str) -> bool {
        let bytes = path.as_bytes();
        path.starts_with("\\\\")
            || (bytes.len() >= 3
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && matches!(bytes[2], b'/' | b'\\'))
    }
    if windows_path(left) && windows_path(right) {
        left.replace('\\', "/")
            .eq_ignore_ascii_case(&right.replace('\\', "/"))
    } else {
        left == right
    }
}

// Root changes span SQLite and persistent server settings. A server storage error
// can follow the SQLite commit, unlike an ordinary annotation PATCH failure.
fn root_needs_refresh(error: &RemoteError) -> bool {
    error.write_may_have_committed()
        || matches!(
            error,
            RemoteError::Server {
                status: 500..=599,
                ..
            } | RemoteError::InsufficientStorage(_)
                | RemoteError::Io(_)
        )
}

fn publish_directory(ui: &AppWindow, directory: RemoteDirectory, chinese: bool) {
    let path = directory.path.unwrap_or_default();
    let mut rows = directory
        .places
        .into_iter()
        .map(|place| RemoteFolderRow {
            name: place.name.into(),
            path: place.path.into(),
        })
        .collect::<Vec<_>>();
    rows.extend(directory.entries.into_iter().map(|entry| RemoteFolderRow {
        name: entry.name.into(),
        path: entry.path.into(),
    }));
    ui.set_sm_fs_rows(ModelRc::new(VecModel::from(rows)));
    ui.set_sm_fs_breadcrumbs(ModelRc::new(VecModel::from(remote_breadcrumbs(&path))));
    ui.set_sm_fs_can_select(!path.is_empty());
    ui.set_sm_fs_path(path.into());
    ui.set_sm_fs_parent(directory.parent.unwrap_or_default().into());
    ui.set_sm_fs_error(
        if directory.truncated || directory.counts_truncated {
            cn_or(
                chinese,
                "此目录内容较多，列表已截断；可输入完整路径进入子目录。",
                "Large directory: listing truncated. Enter a full path to open a subfolder.",
            )
        } else {
            String::new()
        }
        .into(),
    );
}

/// Build paths using the server's separators, never the client's Windows Path rules.
fn remote_breadcrumbs(path: &str) -> Vec<RemoteBreadcrumbRow> {
    let mut result = Vec::new();
    let separator = if path.starts_with('\\') || (!path.starts_with('/') && path.contains('\\')) {
        '\\'
    } else {
        '/'
    };
    let prefix = if path.starts_with("\\\\") || path.starts_with("//") {
        2
    } else if path.starts_with('/') {
        1
    } else {
        0
    };
    if prefix == 1 {
        result.push(RemoteBreadcrumbRow {
            name: "/".into(),
            path: "/".into(),
        });
    }
    let mut end = prefix;
    for part in path[prefix..].split(separator).filter(|s| !s.is_empty()) {
        end = path[end..]
            .find(part)
            .map(|offset| end + offset + part.len())
            .unwrap_or(path.len());
        let target = if part.ends_with(':') {
            format!("{}{}", &path[..end], separator)
        } else {
            path[..end].to_owned()
        };
        result.push(RemoteBreadcrumbRow {
            name: part.into(),
            path: target.into(),
        });
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removed_root_overlay_respects_server_path_case() {
        assert!(same_server_path("/srv/Sounds", "/srv/Sounds"));
        assert!(!same_server_path("/srv/Sounds", "/srv/sounds"));
        assert!(!same_server_path("/srv/a\\b", "/srv/a/b"));
        assert!(same_server_path(r"D:\Sounds", "d:/sounds"));
        assert!(same_server_path(
            r"\\NAS\Share\Sounds",
            r"\\nas\share\sounds"
        ));
    }

    #[test]
    fn root_storage_errors_reconcile_without_retrying_the_mutation() {
        assert!(root_needs_refresh(&RemoteError::Server {
            status: 500,
            code: "INTERNAL".into(),
            message: "settings persistence failed".into(),
        }));
        assert!(root_needs_refresh(&RemoteError::InsufficientStorage(
            "disk full".into()
        )));
        assert!(root_needs_refresh(&RemoteError::Timeout(
            "lost response".into()
        )));
        assert!(!root_needs_refresh(&RemoteError::Validation(
            "bad path".into()
        )));
        assert!(!root_needs_refresh(&RemoteError::Forbidden(
            "manage off".into()
        )));
    }

    #[test]
    fn breadcrumbs_use_server_paths_on_a_windows_client() {
        let rows = remote_breadcrumbs("/Volumes/ssd8t/音效库");
        assert_eq!(
            rows.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(),
            ["/", "/Volumes", "/Volumes/ssd8t", "/Volumes/ssd8t/音效库"]
        );
        let rows = remote_breadcrumbs(r"D:\Sounds\Foley");
        assert_eq!(rows[0].path.as_str(), "D:\\");
        assert_eq!(rows[2].path.as_str(), r"D:\Sounds\Foley");
        let rows = remote_breadcrumbs(r"/srv/a\b");
        assert_eq!(rows[2].name.as_str(), r"a\b");
        assert_eq!(rows[2].path.as_str(), r"/srv/a\b");
        let rows = remote_breadcrumbs(r"\\nas\share\Sounds");
        assert_eq!(rows[1].path.as_str(), r"\\nas\share");
        assert!(remote_breadcrumbs("").is_empty());
    }
}
