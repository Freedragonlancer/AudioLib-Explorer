// app/audiolib-desktop/src/search_tasks.rs
//! Client-local search workspaces. Only the event loop captures or restores a view;
//! blocking identity checks and ordered durable writes run on workers.
use super::*;
use audiolib_core::search_workspace::{
    SavedSearchTask, SearchAudioRef, SearchFolderRef, SearchTask, SearchTaskDefinition,
    SearchTaskView, SearchWorkspace, SearchWorkspaceTarget,
};
use std::collections::BTreeMap;

#[cfg(test)]
#[path = "search_task_tests.rs"]
mod tests;

#[derive(Clone)]
struct TaskCache {
    provider: Weak<SqliteProvider>,
    revision: u64,
    tree: TreeState,
    records: Vec<AudioFile>,
    summary: String,
    page: String,
    previous: bool,
    next: bool,
}

/// Mutable workspace state is never held across provider or filesystem calls.
pub(super) struct TaskWorkspaceState {
    pub(super) workspace: SearchWorkspace,
    target: Option<SearchWorkspaceTarget>,
    caches: HashMap<String, TaskCache>,
    page_revision: Option<u64>,
    folders_revision: Option<u64>,
    page_complete: bool,
    epoch: u64,
    loading: bool,
    restoring: bool,
    pending_view: Option<SearchTaskView>,
    unavailable_folder: Option<SearchFolderRef>,
    protected: bool,
}

impl Default for TaskWorkspaceState {
    fn default() -> Self {
        Self {
            workspace: SearchWorkspace::for_scope(""),
            target: None,
            caches: HashMap::new(),
            page_revision: None,
            folders_revision: None,
            page_complete: false,
            epoch: 0,
            loading: false,
            restoring: false,
            pending_view: None,
            unavailable_folder: None,
            protected: false,
        }
    }
}

struct SaveJob {
    provider: Arc<SqliteProvider>,
    target: SearchWorkspaceTarget,
    workspace: SearchWorkspace,
}

#[derive(Default)]
pub(super) struct WorkspaceWrites {
    jobs: VecDeque<SaveJob>,
    in_flight: bool,
    errors: Vec<(SearchWorkspaceTarget, String)>,
}

fn unique_id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    format!(
        "{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

fn audio_ref(file: &AudioFile) -> SearchAudioRef {
    SearchAudioRef {
        id: file.id,
        file_path: file.file_path.clone(),
    }
}

fn folder_ref(folder: &Folder) -> SearchFolderRef {
    SearchFolderRef {
        id: folder.id,
        path: folder.path.clone(),
    }
}

fn fresh_task(number: usize, order_by: SortOrder, descending: bool, chinese: bool) -> SearchTask {
    SearchTask {
        id: unique_id(),
        title: if chinese {
            format!("搜索任务 {number}")
        } else {
            format!("Search {number}")
        },
        definition: SearchTaskDefinition {
            order_by,
            descending,
            ..Default::default()
        },
        view: SearchTaskView::default(),
        saved_search_id: None,
    }
}

impl Controller {
    pub(super) fn begin_task_search(&self, reset: bool) {
        let mut state = self.task_workspace.lock().unwrap();
        state.page_revision = None;
        state.folders_revision = None;
        state.page_complete = false;
        if reset {
            state.pending_view = None;
        }
    }

    pub(super) fn clear_task_folder_error(&self) {
        self.task_workspace.lock().unwrap().unavailable_folder = None;
    }

    pub(super) fn task_folder_unavailable(&self, ui: &AppWindow) -> bool {
        let folder_id = self.request.lock().unwrap().folder_id;
        let mut state = self.task_workspace.lock().unwrap();
        if state
            .unavailable_folder
            .as_ref()
            .is_some_and(|folder| Some(folder.id) == folder_id)
        {
            ui.set_selected_folder_path("".into());
            ui.set_status(self.localized(
                "任务目录已失效，请重新选择目录",
                "Task folder is unavailable; choose a folder to resume this search",
            ));
            true
        } else {
            state.unavailable_folder = None;
            false
        }
    }
    pub(super) fn freeze_search_workspace(&self) {
        let mut state = self.task_workspace.lock().unwrap();
        state.loading = true;
        state.restoring = false;
        state.epoch += 1;
        drop(state);
        if let Some(ui) = self.ui.upgrade() {
            ui.set_search_workspace_loading(false);
        }
    }

    pub(super) fn thaw_search_workspace(self: &Arc<Self>) {
        let id = {
            let mut state = self.task_workspace.lock().unwrap();
            state.loading = false;
            state.workspace.session.active_task_id.clone()
        };
        if let Some(id) = id {
            self.restore_search_task(&id);
        }
    }

    pub(super) fn bind_search_tasks(self: &Arc<Self>, ui: &AppWindow) {
        let c = self.clone();
        ui.on_search_task_created(move || c.create_search_task());
        let c = self.clone();
        ui.on_search_task_selected(move |id| c.switch_search_task(&id));
        let c = self.clone();
        ui.on_search_task_closed(move |id| c.close_search_task(&id));
        let c = self.clone();
        ui.on_search_task_favorite_toggled(move |id| c.toggle_search_task_favorite(&id));
        let c = self.clone();
        ui.on_search_task_renamed(move |id, name| c.rename_search_task(&id, &name, false));
        let c = self.clone();
        ui.on_saved_search_opened(move |id| c.open_saved_search(&id));
        let c = self.clone();
        ui.on_saved_search_renamed(move |id, name| c.rename_search_task(&id, &name, true));
        let c = self.clone();
        ui.on_saved_search_updated(move |id| c.update_saved_search(&id));
        let c = self.clone();
        ui.on_saved_search_deleted(move |id| c.delete_saved_search(&id));
        let c = self.clone();
        ui.on_max_search_tasks_requested(move |limit| c.set_search_task_limit(limit));
        ui.set_max_search_tasks(self.max_search_tasks.load(Ordering::Acquire) as i32);
    }

    pub(super) fn start_workspace_worker(self: &Arc<Self>) {
        let c = self.clone();
        std::thread::Builder::new()
            .name("audiolib-search-workspace".into())
            .spawn(move || {
                loop {
                    let mut writes = c.workspace_writes.lock().unwrap();
                    while writes.jobs.is_empty() && !c.shutdown.load(Ordering::Acquire) {
                        writes = c.workspace_wake.wait(writes).unwrap();
                    }
                    let Some(job) = writes.jobs.pop_front() else {
                        return;
                    };
                    writes.in_flight = true;
                    drop(writes);
                    let result = job
                        .provider
                        .save_search_workspace(&job.target, &job.workspace)
                        .map_err(|error| error.to_string());
                    let mut writes = c.workspace_writes.lock().unwrap();
                    writes.in_flight = false;
                    // A failed write is retained as an error until a later explicit save
                    // succeeds; the latest state remains in the event-loop workspace.
                    writes.errors.retain(|(target, _)| target != &job.target);
                    let error = result.err();
                    if let Some(error) = &error {
                        writes.errors.push((job.target, error.clone()));
                    }
                    c.workspace_wake.notify_all();
                    drop(writes);
                    if let Some(error) = error {
                        c.status(format!("Search workspace could not be saved: {error}"));
                        post(c.ui.clone(), move |ui| {
                            ui.set_search_workspace_error(
                                format!("Search workspace could not be saved: {error}").into(),
                            )
                        });
                    } else {
                        post(c.ui.clone(), move |ui| {
                            ui.set_search_workspace_error("".into())
                        });
                    }
                }
            })
            .expect("spawn workspace worker");
    }

    pub(super) fn wait_workspace_writes(&self) -> Result<(), String> {
        let mut writes = self.workspace_writes.lock().unwrap();
        while !writes.jobs.is_empty() || writes.in_flight {
            writes = self.workspace_wake.wait(writes).unwrap();
        }
        writes
            .errors
            .first()
            .map_or(Ok(()), |(_, error)| Err(error.clone()))
    }

    fn queue_workspace_save(&self) {
        let job = {
            let state = self.task_workspace.lock().unwrap();
            if state.loading || state.protected {
                return;
            }
            state.target.as_ref().map(|target| SaveJob {
                provider: self.provider(),
                target: target.clone(),
                workspace: state.workspace.clone(),
            })
        };
        if let Some(job) = job {
            let mut writes = self.workspace_writes.lock().unwrap();
            // Coalesce only an unstarted snapshot for this scope. A pending old
            // library can never be retargeted when another source becomes active.
            writes.jobs.retain(|queued| queued.target != job.target);
            writes.jobs.push_back(job);
            self.workspace_wake.notify_one();
        }
    }

    pub(super) fn init_search_workspace(self: &Arc<Self>) {
        let Some(ui) = self.ui.upgrade() else { return };
        let epoch = {
            let mut state = self.task_workspace.lock().unwrap();
            state.epoch += 1;
            state.loading = true;
            state.caches.clear();
            state.page_revision = None;
            state.epoch
        };
        ui.set_search_workspace_loading(true);
        ui.set_search_workspace_error("".into());
        let provider = self.provider();
        let data_root = self.config.lock().unwrap().data_root();
        let server_scope = if self.remote_read_only() {
            self.remote
                .lock()
                .unwrap()
                .state
                .as_ref()
                .map(|state| format!("server-{}", state.library_id))
        } else {
            None
        };
        let is_remote = self.remote_read_only();
        let c = self.clone();
        std::thread::spawn(move || {
            let result = (|| -> audiolib_core::Result<_> {
                c.wait_workspace_writes()
                    .map_err(audiolib_core::Error::Validation)?;
                let scope = if is_remote {
                    server_scope.ok_or_else(|| {
                        audiolib_core::Error::Validation(
                            "Sync a server library before restoring search tasks".into(),
                        )
                    })?
                } else {
                    audiolib_db::local_play_history::local_library_id(provider.database_path())?
                };
                let target = SearchWorkspaceTarget::new(data_root, scope);
                let workspace = provider.load_search_workspace(&target);
                Ok((target, workspace))
            })();
            let c2 = c.clone();
            post(c.ui.clone(), move |ui| {
                if c2.shutdown.load(Ordering::Acquire)
                    || c2.task_workspace.lock().unwrap().epoch != epoch
                {
                    return;
                }
                let mut state = c2.task_workspace.lock().unwrap();
                state.loading = false;
                state.protected = false;
                match result {
                    Ok((target, Ok(workspace))) => {
                        state.target = Some(target);
                        state.workspace = workspace;
                    }
                    Ok((target, Err(error))) => {
                        state.workspace = SearchWorkspace::for_scope(target.library_scope.clone());
                        state.target = Some(target);
                        state.protected = true;
                        ui.set_search_workspace_error(
                            format!("Search workspace preserved; changes cannot be saved: {error}")
                                .into(),
                        );
                        ui.set_status(
                            format!(
                                "Search workspace preserved and cannot be overwritten: {error}"
                            )
                            .into(),
                        );
                    }
                    Err(error) => {
                        state.target = None;
                        state.workspace = SearchWorkspace::for_scope("");
                        ui.set_status(error.to_string().into());
                    }
                }
                let fresh_workspace = state.workspace.session.tasks.is_empty();
                if fresh_workspace {
                    let preferences = c2.audio_table.lock().unwrap();
                    let task = fresh_task(
                        1,
                        preferences.order_by,
                        preferences.descending,
                        c2.chinese.load(Ordering::Acquire),
                    );
                    state.workspace.session.active_task_id = Some(task.id.clone());
                    state.workspace.session.tasks.push(task);
                }
                let count = state.workspace.session.tasks.len() as u32;
                if c2.max_search_tasks.load(Ordering::Acquire) < count {
                    c2.max_search_tasks.store(count, Ordering::Release);
                    ui.set_max_search_tasks(count as i32);
                    *c2.single_click_play_pending.lock().unwrap() = true;
                    c2.single_click_play_wake.notify_one();
                }
                let id = state
                    .workspace
                    .session
                    .active_task_id
                    .clone()
                    .filter(|id| {
                        state
                            .workspace
                            .session
                            .tasks
                            .iter()
                            .any(|task| &task.id == id)
                    })
                    .unwrap_or_else(|| state.workspace.session.tasks[0].id.clone());
                state.workspace.session.active_task_id = Some(id.clone());
                drop(state);
                ui.set_search_workspace_loading(false);
                if fresh_workspace {
                    // First-use workspaces adopt the already running initial view.
                    // Do not cancel a selection or query requested while startup
                    // completed merely to restore an identical blank task.
                    c2.persist_search_task(&ui);
                } else {
                    c2.restore_search_task(&id);
                }
            });
        });
    }

    /// Capture raw controls as well as the last valid request. Invalid drafts are
    /// session state, never a replacement for the saved executable definition.
    pub(super) fn capture_search_task(&self, ui: &AppWindow) {
        let previous = {
            let state = self.task_workspace.lock().unwrap();
            if state.loading || state.restoring {
                return;
            }
            state
                .workspace
                .session
                .tasks
                .iter()
                .find(|task| Some(&task.id) == state.workspace.session.active_task_id.as_ref())
                .cloned()
        };
        let request = self.request.lock().unwrap().clone();
        let tree = self.tree.lock().unwrap().clone();
        let records = self.records.lock().unwrap().clone();
        let selected = self.selected.lock().unwrap().as_ref().map(audio_ref);
        let checked = self.multi_selection.lock().unwrap().clone();
        let anchor = *self.selection_anchor.lock().unwrap();
        let mut raw = BTreeMap::new();
        for (key, value) in [
            ("query", ui.get_query()),
            ("tag_filter", ui.get_tag_filter()),
            ("tag_draft", ui.get_tag_draft()),
            ("color_filter", ui.get_color_filter()),
            ("duration_min", ui.get_duration_min()),
            ("duration_max", ui.get_duration_max()),
        ] {
            raw.insert(key.to_owned(), value.to_string());
        }
        raw.insert("favorites_only".into(), ui.get_favorites_only().to_string());
        raw.insert("tagged_only".into(), ui.get_tagged_only().to_string());
        let row_height = ui.get_task_result_row_height().max(1.0);
        let scroll_y = ui.get_task_result_scroll_y();
        let view = SearchTaskView {
            offset: request.offset,
            raw_inputs: raw,
            expanded_folders: tree
                .expanded
                .iter()
                .filter_map(|id| tree.known.get(id).map(folder_ref))
                .collect(),
            folder_scroll_y: ui.get_task_folder_scroll_y(),
            list_scroll_y: scroll_y,
            list_scroll_x: ui.get_task_result_scroll_x(),
            list_top_row: (-scroll_y / row_height).max(0.0).floor() as usize,
            list_row_offset: (-scroll_y).max(0.0) % row_height / row_height,
            selected_audio: selected,
            checked_audio: {
                let mut refs = previous
                    .as_ref()
                    .map_or_else(Vec::new, |task| task.view.checked_audio.clone());
                refs.retain(|reference| checked.contains(&reference.id));
                for file in records.iter().filter(|file| checked.contains(&file.id)) {
                    refs.retain(|reference| reference.id != file.id);
                    refs.push(audio_ref(file));
                }
                refs
            },
            selection_anchor: records
                .iter()
                .find(|file| Some(file.id) == anchor)
                .map(audio_ref)
                .or_else(|| {
                    previous
                        .as_ref()
                        .and_then(|task| task.view.selection_anchor.clone())
                        .filter(|reference| Some(reference.id) == anchor)
                }),
            sample_filter: ui.get_sample_filter(),
            bit_filter: ui.get_bit_filter(),
            channel_filter: ui.get_channel_filter(),
            visibility_index: ui.get_visibility_index(),
            sidebar_page: ui.get_library_sidebar_page(),
            favorites_category: ui.get_favorites_category(),
            filters_visible: ui.get_filters_visible(),
            filter_scroll_y: ui.get_task_filter_scroll_y(),
            inspector_scroll_y: ui.get_task_inspector_scroll_y(),
        };
        let definition = SearchTaskDefinition {
            query: request.query,
            folder: tree
                .selected
                .and_then(|id| tree.known.get(&id).map(folder_ref))
                .or_else(|| {
                    previous
                        .as_ref()
                        .and_then(|task| task.definition.folder.clone())
                        .filter(|folder| request.folder_id == Some(folder.id))
                }),
            include_subfolders: request.include_subfolders,
            filters: request.filters,
            order_by: table_sort_order(ui.get_sort_index()),
            descending: ui.get_sort_descending(),
        };
        let mut state = self.task_workspace.lock().unwrap();
        if state.loading {
            return;
        }
        let Some(id) = state.workspace.session.active_task_id.clone() else {
            return;
        };
        if let Some(task) = state
            .workspace
            .session
            .tasks
            .iter_mut()
            .find(|task| task.id == id)
        {
            task.definition = definition;
            task.view = view;
        }
        if let Some(revision) = state.page_revision.filter(|revision| {
            state.page_complete && state.folders_revision == Some(*revision) && !ui.get_searching()
        }) {
            state.caches.insert(
                id,
                TaskCache {
                    provider: Arc::downgrade(&self.provider()),
                    revision,
                    tree,
                    records,
                    summary: ui.get_result_summary().to_string(),
                    page: ui.get_page_label().to_string(),
                    previous: ui.get_can_previous(),
                    next: ui.get_can_next(),
                },
            );
        } else {
            state.caches.remove(&id);
        }
    }

    pub(super) fn persist_search_task(&self, ui: &AppWindow) {
        self.capture_search_task(ui);
        self.publish_search_tasks(ui);
        self.queue_workspace_save();
    }

    pub(super) fn publish_search_tasks(&self, ui: &AppWindow) {
        let state = self.task_workspace.lock().unwrap();
        let rows = state
            .workspace
            .session
            .tasks
            .iter()
            .map(|task| {
                let saved = task.saved_search_id.as_ref().and_then(|id| {
                    state
                        .workspace
                        .favorites
                        .iter()
                        .find(|saved| &saved.id == id)
                });
                crate::SearchTaskRow {
                    id: task.id.as_str().into(),
                    name: task.title.as_str().into(),
                    favorite: saved.is_some(),
                    modified: saved.is_some_and(|saved| {
                        saved.definition != task.definition || !task_inputs_valid(task)
                    }),
                    active: state.workspace.session.active_task_id.as_ref() == Some(&task.id),
                }
            })
            .collect::<Vec<_>>();
        ui.set_search_tasks(ModelRc::new(VecModel::from(rows)));
        ui.set_saved_search_tasks(ModelRc::new(VecModel::from(
            state
                .workspace
                .favorites
                .iter()
                .map(|saved| crate::SavedSearchTaskRow {
                    id: saved.id.as_str().into(),
                    name: saved.title.as_str().into(),
                    summary: saved.definition.query.as_str().into(),
                })
                .collect::<Vec<_>>(),
        )));
    }

    pub(super) fn set_search_task_limit(&self, limit: i32) {
        let Some(ui) = self.ui.upgrade() else { return };
        let limit = limit.clamp(1, 6) as u32;
        let count = self
            .task_workspace
            .lock()
            .unwrap()
            .workspace
            .session
            .tasks
            .len();
        if limit < count as u32 {
            ui.set_max_search_tasks(self.max_search_tasks.load(Ordering::Acquire) as i32);
            ui.set_status(self.localized(
                "请先关闭多余搜索任务，再降低上限",
                "Close extra search tasks before reducing the limit",
            ));
            return;
        }
        self.max_search_tasks.store(limit, Ordering::Release);
        ui.set_max_search_tasks(limit as i32);
        *self.single_click_play_pending.lock().unwrap() = true;
        self.single_click_play_wake.notify_one();
    }

    pub(super) fn create_search_task(self: &Arc<Self>) {
        if !self.available() {
            return;
        }
        let Some(ui) = self.ui.upgrade() else { return };
        if self.task_workspace.lock().unwrap().loading {
            return;
        }
        if let Err(error) = self.enqueue_current_metadata(&ui) {
            ui.set_status(error.into());
            return;
        }
        self.capture_search_task(&ui);
        let mut state = self.task_workspace.lock().unwrap();
        if state.workspace.session.tasks.len()
            >= self.max_search_tasks.load(Ordering::Acquire) as usize
        {
            ui.set_status(self.localized(
                "搜索任务已达上限，请先关闭一个任务",
                "Search task limit reached; close a task first",
            ));
            return;
        }
        let prefs = self.audio_table.lock().unwrap();
        let task = fresh_task(
            state.workspace.session.tasks.len() + 1,
            prefs.order_by,
            prefs.descending,
            self.chinese.load(Ordering::Acquire),
        );
        let id = task.id.clone();
        state.workspace.session.tasks.push(task);
        state.workspace.session.active_task_id = Some(id.clone());
        drop(prefs);
        drop(state);
        self.restore_search_task(&id);
    }

    pub(super) fn switch_search_task(self: &Arc<Self>, id: &str) {
        if !self.available() {
            return;
        }
        let Some(ui) = self.ui.upgrade() else { return };
        {
            let state = self.task_workspace.lock().unwrap();
            if state.loading
                || state.workspace.session.active_task_id.as_deref() == Some(id)
                || !state
                    .workspace
                    .session
                    .tasks
                    .iter()
                    .any(|task| task.id == id)
            {
                return;
            }
        }
        if let Err(error) = self.enqueue_current_metadata(&ui) {
            ui.set_status(error.into());
            return;
        }
        self.capture_search_task(&ui);
        self.task_workspace
            .lock()
            .unwrap()
            .workspace
            .session
            .active_task_id = Some(id.to_owned());
        self.restore_search_task(id);
    }

    pub(super) fn close_search_task(self: &Arc<Self>, id: &str) {
        if !self.available() {
            return;
        }
        let Some(ui) = self.ui.upgrade() else { return };
        if self.task_workspace.lock().unwrap().loading {
            return;
        }
        if let Err(error) = self.enqueue_current_metadata(&ui) {
            ui.set_status(error.into());
            return;
        }
        self.capture_search_task(&ui);
        let mut state = self.task_workspace.lock().unwrap();
        let Some(index) = state
            .workspace
            .session
            .tasks
            .iter()
            .position(|task| task.id == id)
        else {
            return;
        };
        let active = state.workspace.session.active_task_id.as_deref() == Some(id);
        state.workspace.session.tasks.remove(index);
        state.caches.remove(id);
        if state.workspace.session.tasks.is_empty() {
            state.workspace.session.tasks.push(fresh_task(
                1,
                SortOrder::Relevance,
                false,
                self.chinese.load(Ordering::Acquire),
            ));
        }
        let next = state.workspace.session.tasks
            [index.min(state.workspace.session.tasks.len() - 1)]
        .id
        .clone();
        if active {
            state.workspace.session.active_task_id = Some(next.clone());
        }
        drop(state);
        if active {
            self.restore_search_task(&next);
        } else {
            self.publish_search_tasks(&ui);
            self.queue_workspace_save();
        }
    }

    fn rename_search_task(&self, id: &str, name: &str, saved: bool) {
        if !self.available() {
            return;
        }
        let Some(ui) = self.ui.upgrade() else { return };
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 100 || name.chars().any(char::is_control) {
            ui.set_status(self.localized(
                "任务名称须为 1–100 个字符",
                "Task names must contain 1–100 characters",
            ));
            return;
        }
        let mut state = self.task_workspace.lock().unwrap();
        if saved {
            if let Some(task) = state
                .workspace
                .favorites
                .iter_mut()
                .find(|task| task.id == id)
            {
                task.title = name.to_owned();
            }
        } else if let Some(task) = state
            .workspace
            .session
            .tasks
            .iter_mut()
            .find(|task| task.id == id)
        {
            task.title = name.to_owned();
        }
        drop(state);
        self.publish_search_tasks(&ui);
        self.queue_workspace_save();
    }

    pub(super) fn toggle_search_task_favorite(self: &Arc<Self>, id: &str) {
        if !self.available() {
            return;
        }
        let Some(ui) = self.ui.upgrade() else { return };
        self.capture_search_task(&ui);
        let mut state = self.task_workspace.lock().unwrap();
        let Some(index) = state
            .workspace
            .session
            .tasks
            .iter()
            .position(|task| task.id == id)
        else {
            return;
        };
        let task = state.workspace.session.tasks[index].clone();
        if let Some(saved_id) = task.saved_search_id {
            drop(state);
            self.delete_saved_search(&saved_id);
            return;
        }
        if !task_inputs_valid(&task) {
            ui.set_status(self.localized(
                "请先修正检索条件，再收藏任务",
                "Correct the search conditions before saving a task",
            ));
            return;
        }
        let saved_id = unique_id();
        state.workspace.favorites.push(SavedSearchTask {
            id: saved_id.clone(),
            title: task.title,
            definition: task.definition,
        });
        state.workspace.session.tasks[index].saved_search_id = Some(saved_id);
        drop(state);
        self.publish_search_tasks(&ui);
        self.queue_workspace_save();
    }

    fn delete_saved_search(&self, id: &str) {
        if !self.available() {
            return;
        }
        let mut state = self.task_workspace.lock().unwrap();
        state.workspace.favorites.retain(|saved| saved.id != id);
        for task in &mut state.workspace.session.tasks {
            if task.saved_search_id.as_deref() == Some(id) {
                task.saved_search_id = None;
            }
        }
        drop(state);
        if let Some(ui) = self.ui.upgrade() {
            self.publish_search_tasks(&ui);
        }
        self.queue_workspace_save();
    }

    pub(super) fn update_saved_search(&self, id: &str) {
        if !self.available() {
            return;
        }
        let Some(ui) = self.ui.upgrade() else { return };
        self.capture_search_task(&ui);
        let mut state = self.task_workspace.lock().unwrap();
        let task = state
            .workspace
            .session
            .tasks
            .iter()
            .find(|task| Some(&task.id) == state.workspace.session.active_task_id.as_ref())
            .cloned();
        let Some(task) = task.filter(task_inputs_valid) else {
            ui.set_status(
                self.localized("请先修正检索条件", "Correct the search conditions first"),
            );
            return;
        };
        if let Some(saved) = state
            .workspace
            .favorites
            .iter_mut()
            .find(|saved| saved.id == id)
        {
            saved.definition = task.definition;
        }
        drop(state);
        self.publish_search_tasks(&ui);
        self.queue_workspace_save();
    }

    pub(super) fn open_saved_search(self: &Arc<Self>, id: &str) {
        if !self.available() {
            return;
        }
        let Some(ui) = self.ui.upgrade() else { return };
        if let Err(error) = self.enqueue_current_metadata(&ui) {
            ui.set_status(error.into());
            return;
        }
        self.capture_search_task(&ui);
        let mut state = self.task_workspace.lock().unwrap();
        if state.loading {
            return;
        }
        let Some(saved) = state
            .workspace
            .favorites
            .iter()
            .find(|saved| saved.id == id)
            .cloned()
        else {
            return;
        };
        if let Some(existing) = state
            .workspace
            .session
            .tasks
            .iter()
            .find(|task| task.definition == saved.definition && task_inputs_valid(task))
        {
            let existing = existing.id.clone();
            drop(state);
            self.switch_search_task(&existing);
            return;
        }
        if state.workspace.session.tasks.len()
            >= self.max_search_tasks.load(Ordering::Acquire) as usize
        {
            ui.set_status(self.localized(
                "搜索任务已达上限，请先关闭一个任务",
                "Search task limit reached; close a task first",
            ));
            return;
        }
        let task = SearchTask {
            id: unique_id(),
            title: saved.title,
            definition: saved.definition,
            saved_search_id: Some(saved.id),
            view: SearchTaskView::default(),
        };
        let id = task.id.clone();
        state.workspace.session.tasks.push(task);
        state.workspace.session.active_task_id = Some(id.clone());
        drop(state);
        self.restore_search_task(&id);
    }

    fn restore_search_task(self: &Arc<Self>, id: &str) {
        let Some(ui) = self.ui.upgrade() else { return };
        self.search_cancel
            .lock()
            .unwrap()
            .store(true, Ordering::Release);
        self.search_generation.fetch_add(1, Ordering::AcqRel);
        self.tree_generation.fetch_add(1, Ordering::AcqRel);
        self.selection_generation.fetch_add(1, Ordering::AcqRel);
        self.invalidate_waveform();
        self.list_waveforms.invalidate();
        *self.search_job.lock().unwrap() = None;
        *self.facet_job.lock().unwrap() = None;
        *self.search_publication.lock().unwrap() = None;
        // Do not use select_id(false): its existing selection path also performs Load.
        *self.latest_selection.lock().unwrap() = None;
        *self.selection_job.lock().unwrap() = None;
        *self.selection_audio_job.lock().unwrap() = None;
        let (task, cache, epoch) = {
            let mut state = self.task_workspace.lock().unwrap();
            let Some(task) = state
                .workspace
                .session
                .tasks
                .iter()
                .find(|task| task.id == id)
                .cloned()
            else {
                return;
            };
            state.epoch += 1;
            state.page_revision = None;
            state.restoring = true;
            (task, state.caches.get(id).cloned(), state.epoch)
        };
        ui.set_task_restoring(true);
        ui.set_search_workspace_loading(true);
        ui.set_searching(false);
        apply_task_controls(&ui, &task);
        self.records.lock().unwrap().clear();
        *self.selected.lock().unwrap() = None;
        ui.set_rows(ModelRc::new(VecModel::<AudioRow>::default()));
        ui.set_selected_id("".into());
        ui.set_has_selection(false);
        ui.set_selected_folder_path("".into());
        self.publish_search_tasks(&ui);
        let index = self
            .task_workspace
            .lock()
            .unwrap()
            .workspace
            .session
            .tasks
            .iter()
            .position(|task| task.id == id);
        if let Some(index) = index {
            ui.invoke_reveal_search_task(index as i32);
        }
        let provider = self.provider();
        let c = self.clone();
        std::thread::spawn(move || {
            let result = resolve_task(&provider, task, cache);
            let c2 = c.clone();
            post(c.ui.clone(), move |ui| {
                if c2.shutdown.load(Ordering::Acquire)
                    || c2.closing.load(Ordering::Acquire)
                    || c2.task_workspace.lock().unwrap().epoch != epoch
                {
                    return;
                }
                ui.set_search_workspace_loading(false);
                c2.task_workspace.lock().unwrap().restoring = false;
                match result {
                    Ok((task, cache, selected, folder_missing, folders)) => {
                        c2.apply_search_task(&ui, task, cache, selected, folder_missing, folders);
                    }
                    Err(error) => {
                        ui.set_task_restoring(false);
                        ui.set_status(error.into());
                    }
                }
            });
        });
    }

    fn apply_search_task(
        self: &Arc<Self>,
        ui: &AppWindow,
        task: SearchTask,
        cache: Option<TaskCache>,
        selected: Option<AudioFile>,
        folder_missing: bool,
        folders: Vec<Folder>,
    ) {
        apply_task_controls(ui, &task);
        *self.request.lock().unwrap() = task.definition.to_request(PAGE_SIZE, task.view.offset);
        let stats = self.tree.lock().unwrap().stats.clone();
        *self.tree.lock().unwrap() = cache.as_ref().map_or_else(
            || TreeState {
                stats,
                ..Default::default()
            },
            |cache| cache.tree.clone(),
        );
        {
            let mut tree = self.tree.lock().unwrap();
            tree.selected = task
                .definition
                .folder
                .as_ref()
                .filter(|_| !folder_missing)
                .map(|folder| folder.id);
            if cache.is_none() && !folders.is_empty() {
                tree.known = Arc::new(
                    folders
                        .iter()
                        .map(|folder| (folder.id, folder.clone()))
                        .collect(),
                );
                tree.matches = Arc::new(folders);
            }
            tree.expanded = task
                .view
                .expanded_folders
                .iter()
                .map(|folder| folder.id)
                .collect();
        }
        *self.multi_selection.lock().unwrap() =
            task.view.checked_audio.iter().map(|file| file.id).collect();
        *self.selection_anchor.lock().unwrap() =
            task.view.selection_anchor.as_ref().map(|file| file.id);
        *self.selected.lock().unwrap() = selected.clone();
        if let Some(file) = selected {
            let (file, dirty) = self.metadata.lock().unwrap().for_display(&file, 0);
            *self.selected.lock().unwrap() = Some(file.clone());
            present_file(ui, &file);
            ui.set_metadata_dirty(dirty);
            // The loaded sound and its play session remain global. A later explicit
            // Play of this selection must load it before auditioning.
            self.mark_audio_reload(file.id);
        } else {
            ui.set_selected_id("".into());
            ui.set_has_selection(false);
            ui.set_selected_name("".into());
            ui.set_selected_path("".into());
            ui.set_metadata_dirty(false);
        }
        let records = cache
            .as_ref()
            .map_or_else(Vec::new, |cache| cache.records.clone());
        let checked = self.multi_selection.lock().unwrap().clone();
        let selected_row = records
            .iter()
            .position(|file| file.id.to_string() == ui.get_selected_id().as_str());
        let rows = records
            .iter()
            .map(|file| {
                let mut row = audio_row(file);
                row.checked = checked.contains(&file.id);
                row
            })
            .collect();
        *self.records.lock().unwrap() = records;
        publish_audio_rows(ui, self.decorate_list_waveforms(rows));
        ui.set_selected_row(selected_row.map_or(-1, |index| index as i32));
        ui.set_checked_count(checked.len() as i32);
        ui.set_selected_folder_path(
            task.definition
                .folder
                .as_ref()
                .map_or("", |folder| folder.path.as_str())
                .into(),
        );
        let (visible, expanded) = {
            let tree = self.tree.lock().unwrap();
            (tree.visible.clone(), tree.expanded.clone())
        };
        ui.set_folders(ModelRc::new(FolderTreeModel { visible, expanded }));
        ui.set_crumbs(ModelRc::new(VecModel::<CrumbRow>::default()));
        ui.set_result_summary(
            cache
                .as_ref()
                .map_or("", |cache| cache.summary.as_str())
                .into(),
        );
        ui.set_page_label(
            cache
                .as_ref()
                .map_or("1", |cache| cache.page.as_str())
                .into(),
        );
        ui.set_can_previous(cache.as_ref().is_some_and(|cache| cache.previous));
        ui.set_can_next(cache.as_ref().is_some_and(|cache| cache.next));
        {
            let mut state = self.task_workspace.lock().unwrap();
            state.page_revision = cache.as_ref().map(|cache| cache.revision);
            state.folders_revision = state.page_revision;
            state.page_complete = cache.is_some();
            state.unavailable_folder = task.definition.folder.clone().filter(|_| folder_missing);
            if let Some(stored) = state
                .workspace
                .session
                .tasks
                .iter_mut()
                .find(|stored| stored.id == task.id)
            {
                *stored = task.clone();
            }
        }
        if folder_missing {
            ui.set_selected_folder_path("".into());
            self.reload_tree();
            ui.set_status(self.localized(
                "任务目录已失效，请重新选择目录；检索范围未扩大",
                "Task folder is unavailable; choose a folder to resume this search",
            ));
        } else if cache.is_none() {
            self.task_workspace.lock().unwrap().pending_view = Some(task.view.clone());
            self.request_navigation_search(ui, true, false);
        } else {
            self.reload_tree();
        }
        self.restore_task_scroll(ui, &task.view);
        self.publish_search_tasks(ui);
        self.queue_workspace_save();
    }

    fn restore_task_scroll(&self, ui: &AppWindow, view: &SearchTaskView) {
        let y = -(view.list_top_row as f32 + view.list_row_offset)
            * ui.get_task_result_row_height().max(1.0);
        ui.invoke_restore_search_task_viewport(
            y,
            view.list_scroll_x,
            view.folder_scroll_y,
            view.filter_scroll_y,
            view.inspector_scroll_y,
        );
    }

    pub(super) fn search_task_page_ready(&self, ui: &AppWindow, revision: Option<u64>) {
        self.task_workspace.lock().unwrap().page_revision = revision;
        self.persist_search_task(ui);
    }

    pub(super) fn search_task_folders_ready(&self, revision: Option<u64>) {
        self.task_workspace.lock().unwrap().folders_revision = revision;
    }

    pub(super) fn search_task_directory_ready(&self, ui: &AppWindow) {
        self.task_workspace.lock().unwrap().page_complete = true;
        let view = self.task_workspace.lock().unwrap().pending_view.take();
        if let Some(view) = view {
            self.restore_task_scroll(ui, &view);
        }
        self.persist_search_task(ui);
    }
}

fn task_inputs_valid(task: &SearchTask) -> bool {
    let mut request = task.definition.to_request(PAGE_SIZE, task.view.offset);
    let raw = &task.view.raw_inputs;
    if let Some(query) = raw.get("query") {
        request.query = query.clone();
    }
    for (key, destination) in [
        ("duration_min", &mut request.filters.duration_min),
        ("duration_max", &mut request.filters.duration_max),
    ] {
        if let Some(value) = raw.get(key) {
            *destination = if value.trim().is_empty() {
                None
            } else {
                let Ok(value) = value.trim().parse::<f64>() else {
                    return false;
                };
                Some(value)
            };
        }
    }
    if let Some(color) = raw.get("color_filter") {
        request.filters.color = (!color.trim().is_empty()).then(|| color.trim().to_owned());
    }
    if let Some(tags) = raw.get("tag_filter") {
        audiolib_core::tag_filter::apply_tag_terms(
            &mut request.filters,
            &audiolib_core::tag_filter::parse_tag_terms(tags),
        );
    }
    request.validate().is_ok()
        && !request.needs_more_search_text()
        && request.query == task.definition.query
        && request.filters == task.definition.filters
}

fn apply_task_controls(ui: &AppWindow, task: &SearchTask) {
    let definition = &task.definition;
    let view = &task.view;
    let raw = &view.raw_inputs;
    ui.set_query(
        raw.get("query")
            .unwrap_or(&definition.query)
            .as_str()
            .into(),
    );
    ui.set_duration_min(
        raw.get("duration_min")
            .cloned()
            .unwrap_or_else(|| {
                definition
                    .filters
                    .duration_min
                    .map_or_else(String::new, |value| value.to_string())
            })
            .into(),
    );
    ui.set_duration_max(
        raw.get("duration_max")
            .cloned()
            .unwrap_or_else(|| {
                definition
                    .filters
                    .duration_max
                    .map_or_else(String::new, |value| value.to_string())
            })
            .into(),
    );
    ui.set_color_filter(
        raw.get("color_filter")
            .cloned()
            .unwrap_or_else(|| definition.filters.color.clone().unwrap_or_default())
            .into(),
    );
    let tags = audiolib_core::tag_filter::format_tag_terms(
        &audiolib_core::tag_filter::tag_terms_of(&definition.filters),
    );
    ui.set_tag_filter(raw.get("tag_filter").unwrap_or(&tags).as_str().into());
    ui.set_tag_draft(raw.get("tag_draft").map_or("", String::as_str).into());
    let index = |values: &[u32], value: Option<u32>| {
        values
            .iter()
            .position(|candidate| Some(*candidate) == value)
            .map_or(0, |index| index as i32)
    };
    let has_raw = !raw.is_empty();
    ui.set_sample_filter(if has_raw {
        view.sample_filter
    } else {
        index(
            &[0, 44100, 48000, 88200, 96000, 192000],
            definition.filters.sample_rate,
        )
    });
    ui.set_bit_filter(if has_raw {
        view.bit_filter
    } else {
        index(
            &[0, 16, 24, 32],
            definition.filters.bit_depth.map(u32::from),
        )
    });
    ui.set_channel_filter(if has_raw {
        view.channel_filter
    } else {
        index(
            &[0, 1, 2, 4, 6, 8],
            definition.filters.channels.map(u32::from),
        )
    });
    ui.set_visibility_index(if has_raw {
        view.visibility_index
    } else {
        match definition.filters.visibility {
            FileVisibility::Present => 0,
            FileVisibility::Missing => 1,
            FileVisibility::All => 2,
        }
    });
    ui.set_favorites_only(
        raw.get("favorites_only")
            .map_or(definition.filters.favorites_only, |value| value == "true"),
    );
    ui.set_tagged_only(
        raw.get("tagged_only")
            .map_or(definition.filters.tagged_only, |value| value == "true"),
    );
    ui.set_sort_index(sort_index(definition.order_by));
    ui.set_sort_descending(definition.descending);
    ui.set_library_sidebar_page(view.sidebar_page);
    ui.set_favorites_category(view.favorites_category);
    ui.set_filters_visible(view.filters_visible);
    crate::tag_chips::relayout(ui);
    crate::tag_chips::update_color_swatch(ui);
}

type ResolvedTask = (
    SearchTask,
    Option<TaskCache>,
    Option<AudioFile>,
    bool,
    Vec<Folder>,
);

fn resolve_task(
    provider: &Arc<SqliteProvider>,
    mut task: SearchTask,
    cache: Option<TaskCache>,
) -> Result<ResolvedTask, String> {
    let resolve_audio = |reference: &SearchAudioRef| -> Result<Option<AudioFile>, String> {
        match provider.get_audio(reference.id) {
            Ok(file) if same_path(Path::new(&file.file_path), Path::new(&reference.file_path)) => {
                return Ok(Some(file));
            }
            Ok(_) | Err(audiolib_core::Error::NotFound(_)) => {}
            Err(error) => return Err(error.to_string()),
        }
        provider
            .find_audio_by_path(Path::new(&reference.file_path))
            .map_err(|error| error.to_string())
    };
    let mut folders = None;
    let mut resolve_folder =
        |reference: &SearchFolderRef| -> Result<Option<SearchFolderRef>, String> {
            match provider.get_folder(reference.id) {
                Ok(folder) if same_path(Path::new(&folder.path), Path::new(&reference.path)) => {
                    return Ok(Some(folder_ref(&folder)));
                }
                Ok(_) | Err(audiolib_core::Error::NotFound(_)) => {}
                Err(error) => return Err(error.to_string()),
            }
            // Resolve against indexed paths, never this machine's filesystem (remote
            // paths need not be mounted here). Materialize the tree only on ID drift.
            if folders.is_none() {
                let mut all = Vec::new();
                let mut parents = vec![None];
                let mut seen = HashSet::new();
                while let Some(parent) = parents.pop() {
                    for folder in provider
                        .folders(parent)
                        .map_err(|error| error.to_string())?
                    {
                        if seen.insert(folder.id) {
                            parents.push(Some(folder.id));
                            all.push(folder);
                        }
                    }
                }
                folders = Some(all);
            }
            Ok(folders
                .as_ref()
                .and_then(|all| {
                    all.iter().find(|folder| {
                        same_path(Path::new(&folder.path), Path::new(&reference.path))
                    })
                })
                .map(folder_ref))
        };
    let folder_missing = if let Some(folder) = task.definition.folder.clone() {
        if let Some(resolved) = resolve_folder(&folder)? {
            task.definition.folder = Some(resolved);
            false
        } else {
            true
        }
    } else {
        false
    };
    task.view.expanded_folders = task
        .view
        .expanded_folders
        .iter()
        .map(&mut resolve_folder)
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect();
    let selected = task
        .view
        .selected_audio
        .as_ref()
        .map(resolve_audio)
        .transpose()?
        .flatten();
    task.view.selected_audio = selected.as_ref().map(audio_ref);
    task.view.checked_audio = task
        .view
        .checked_audio
        .iter()
        .map(|reference| resolve_audio(reference).map(|file| file.as_ref().map(audio_ref)))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect();
    task.view.selection_anchor = task
        .view
        .selection_anchor
        .as_ref()
        .map(|reference| resolve_audio(reference).map(|file| file.as_ref().map(audio_ref)))
        .transpose()?
        .flatten();
    let revision = provider
        .search_revision()
        .map_err(|error| error.to_string())?;
    let cache = cache.filter(|cache| {
        !folder_missing
            && cache.provider.ptr_eq(&Arc::downgrade(provider))
            && Some(cache.revision) == revision
    });
    Ok((
        task,
        cache,
        selected,
        folder_missing,
        folders.unwrap_or_default(),
    ))
}
