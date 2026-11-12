// app/audiolib-desktop/src/ai_search.rs
//! AI search presentation. All network, key protection and durable writes run off the UI.
use super::*;
use audiolib_ai::{
    AiError,
    config::{ApiKey, DEFAULT_PROMPT, Profile, Protocol, validate_instruction},
    provider,
    reference::ReferenceIndex,
    storage::ProfileStore,
};
use audiolib_core::ai_search::{AiQueryPlan, validate_ai_query};

const MAX_AI_RECOMMENDATIONS: usize = 5;
const MAX_AI_AUTO_TASKS: usize = 3;

#[derive(Clone, PartialEq)]
struct Context {
    revision: u64,
    library: u64,
    source: u64,
    search: u64,
    task: Option<String>,
}

// Session-only editor state. Never serialized or logged; secrets stay masked in the UI.
#[derive(Clone)]
struct FormDraft {
    profile: Profile,
    key: String,
    clear_key: bool,
    dirty: bool,
    models: Vec<String>,
}

// Exact editor values catch unsent drafts as well as admitted query changes.
// Kept on the event loop only; no query or Metadata is logged or persisted here.
#[derive(PartialEq)]
struct SearchControls {
    text: [slint::SharedString; 8],
    choices: [i32; 6],
    flags: [bool; 3],
}

impl SearchControls {
    fn capture(ui: &AppWindow) -> Self {
        Self {
            text: [
                ui.get_query(),
                ui.get_tag_filter(),
                ui.get_tag_draft(),
                ui.get_color_filter(),
                ui.get_duration_min(),
                ui.get_duration_max(),
                ui.get_album_filter(),
                ui.get_album_filter_id(),
            ],
            choices: [
                ui.get_search_scope_index(),
                ui.get_sample_filter(),
                ui.get_bit_filter(),
                ui.get_channel_filter(),
                ui.get_visibility_index(),
                ui.get_sort_index(),
            ],
            flags: [
                ui.get_sort_descending(),
                ui.get_favorites_only(),
                ui.get_tagged_only(),
            ],
        }
    }
}

#[derive(PartialEq)]
struct BatchControls {
    search: SearchControls,
    candidate: slint::SharedString,
    selected: slint::SharedString,
    checked: i32,
    scroll: [f32; 2],
}

impl BatchControls {
    fn capture(ui: &AppWindow) -> Self {
        Self {
            search: SearchControls::capture(ui),
            candidate: ui.get_ai_query(),
            selected: ui.get_selected_id(),
            checked: ui.get_checked_count(),
            scroll: [ui.get_task_result_scroll_x(), ui.get_task_result_scroll_y()],
        }
    }
}

struct VerifiedSearch {
    provider: std::sync::Weak<SqliteProvider>,
    request: SearchRequest,
    scope_choice: i32,
    search: u64,
    status: slint::SharedString,
}

struct TaskBatch {
    ids: Vec<String>,
    index: usize,
    context: Context,
    request: SearchRequest,
    controls: BatchControls,
    started: Instant,
    status: slint::SharedString,
    scroll_settled: bool,
}

struct PendingApply {
    kind: i32,
    index: i32,
    started: Instant,
    context: Context,
    token: u64,
    controls: SearchControls,
    query: slint::SharedString,
}

#[derive(Default)]
pub(super) struct Runtime {
    store: Mutex<Option<ProfileStore>>,
    busy: AtomicBool,
    revision: AtomicU64,
    cancel: Mutex<Arc<AtomicBool>>,
    context: Mutex<Option<Context>>,
    saves: Mutex<bool>,
    save_wake: Condvar,
    save_error: Mutex<Option<String>>,
    draft_protocol: std::sync::atomic::AtomicI32,
    drafts: Mutex<std::collections::HashMap<String, FormDraft>>,
    batch_revision: AtomicU64,
    verified_search: Mutex<Option<VerifiedSearch>>,
}
impl Runtime {
    pub(super) fn cancel(&self) {
        self.revision.fetch_add(1, Ordering::AcqRel);
        self.cancel.lock().unwrap().store(true, Ordering::Release);
        *self.context.lock().unwrap() = None;
    }
    pub(super) fn wait_saves(&self) -> Result<(), String> {
        let mut pending = self.saves.lock().unwrap();
        while *pending {
            pending = self.save_wake.wait(pending).unwrap();
        }
        // Close has set Controller::closing before this barrier. In-flight usage
        // writes drain here; later network completions recheck closing under this lock.
        let _store = self.store.lock().unwrap();
        self.save_error.lock().unwrap().clone().map_or(Ok(()), Err)
    }
}

enum Job {
    Load,
    Local(String, bool),
    Generate(Profile, String, bool, String),
    Models(Profile, Option<ApiKey>),
    Probe(Profile, Option<ApiKey>),
    Save(Profile, Option<ApiKey>),
    Remove(String),
    SavePrompt(bool, String),
}
enum Reply {
    Loaded,
    Plan(AiQueryPlan, String),
    Models(Vec<String>),
    Probed,
    Saved,
    Removed,
    PromptSaved,
    SearchFailed,
}

impl Controller {
    fn ai_context(&self) -> Context {
        Context {
            revision: self.ai.revision.load(Ordering::Acquire),
            library: self.library_generation.load(Ordering::Acquire),
            source: self.source_generation.load(Ordering::Acquire),
            search: self.search_generation.load(Ordering::Acquire),
            task: self
                .task_workspace
                .lock()
                .unwrap()
                .workspace
                .session
                .active_task_id
                .clone(),
        }
    }

    pub(super) fn bind_ai_search(self: &Arc<Self>, ui: &AppWindow) {
        let c = self.clone();
        ui.on_ai_action(move |kind, index| {
            let Some(ui) = c.ui.upgrade() else { return };
            if c.closing.load(Ordering::Acquire) || c.shutdown.load(Ordering::Acquire) {
                return;
            }
            if c.ai.busy.load(Ordering::Acquire) && !matches!(kind, 1 | 3) {
                if kind == 0 {
                    ui.set_ai_visible(true);
                }
                return;
            }
            match kind {
                0 => {
                    if ui.get_ai_prompt().trim().is_empty() {
                        ui.set_ai_prompt(ui.get_query());
                    }
                    ui.set_ai_visible(true);
                    if c.ai.store.lock().unwrap().is_none() {
                        c.ai_job(&ui, Job::Load);
                    } else {
                        c.publish_ai_profiles(&ui);
                    }
                }
                1 | 3 => {
                    let retained = (kind == 1)
                        .then(|| c.ai.context.lock().unwrap().clone())
                        .flatten();
                    c.cancel_ai_task_batch(&ui);
                    c.ai.cancel();
                    ui.set_ai_generating(false);
                    if let Some(mut context) = retained {
                        // Closing the drawer cancels pending work, but completed
                        // recommendations remain usable from the quick-search row.
                        context.revision = c.ai.revision.load(Ordering::Acquire);
                        *c.ai.context.lock().unwrap() = Some(context);
                    } else {
                        c.clear_ai_suggestions(&ui);
                    }
                    if kind == 1 {
                        ui.set_ai_visible(false);
                    }
                    ui.set_ai_status(if c.ai.busy.load(Ordering::Acquire) {
                        c.localized(
                            "已取消结果；正在结束当前请求",
                            "Result cancelled; waiting for the current request to finish",
                        )
                    } else {
                        "".into()
                    });
                }
                2 => {
                    if !c.available() {
                        return;
                    }
                    if c.ai_search_probe_request(&ui).is_none() {
                        ui.set_ai_error(true);
                        ui.set_ai_status(c.localized(
                            "请先完成当前目录和筛选条件的搜索，再生成推荐",
                            "Finish a valid search for the current directory and filters before generating suggestions",
                        ));
                        return;
                    }
                    ui.set_ai_settings(false);
                    let intent = ui.get_ai_prompt().trim().to_owned();
                    let layers = ui.get_ai_layers();
                    let job = if ui.get_ai_provider_index() == 0 {
                        Job::Local(intent, layers)
                    } else {
                        let store = c.ai.store.lock().unwrap();
                        let Some(p) = store.as_ref().and_then(|s| {
                            s.profiles().get((ui.get_ai_provider_index() - 1) as usize)
                        }) else {
                            ui.set_ai_status(
                                c.localized("请先保存供应商配置", "Save a provider profile first"),
                            );
                            return;
                        };
                        Job::Generate(
                            p.clone(),
                            intent,
                            layers,
                            ui.get_ai_instruction().to_string(),
                        )
                    };
                    c.ai_job(&ui, job);
                }
                4 => {
                    c.remember_ai_draft(&ui);
                    c.cancel_ai_task_batch(&ui);
                    c.ai.cancel();
                    c.clear_ai_suggestions(&ui);
                    ui.set_ai_provider_index(index);
                    c.select_ai_profile(&ui, index);
                }
                5 => {
                    if let Some(row) = ui.get_ai_rows().row_data(index.max(0) as usize) {
                        ui.set_ai_selected(index);
                        ui.set_ai_query(row.text);
                        ui.set_ai_detail(row.detail);
                    }
                }
                6 | 7 | 16 | 20 | 21 => {
                    if !c.available() || ui.get_ai_batch_blocked() {
                        return;
                    }
                    if ui.get_task_restoring() || ui.get_search_workspace_loading() {
                        c.cancel_ai_task_batch(&ui);
                        c.defer_ai_apply(&ui, PendingApply {
                            kind, index, started: Instant::now(), context: c.ai_context(),
                            token: c.ai.batch_revision.load(Ordering::Acquire),
                            controls: SearchControls::capture(&ui), query: ui.get_ai_query(),
                        });
                        return;
                    }
                    if kind == 20 {
                        c.search_ai_recommendations(&ui, MAX_AI_RECOMMENDATIONS);
                        return;
                    }
                    if !c.admit_ai_suggestions(&ui) {
                        return;
                    }
                    c.cancel_ai_task_batch(&ui);
                    let query = if kind == 16 {
                        let Some(row) = usize::try_from(index)
                            .ok()
                            .filter(|i| *i < MAX_AI_RECOMMENDATIONS)
                            .and_then(|i| ui.get_ai_quick_rows().row_data(i))
                        else {
                            return;
                        };
                        ui.set_ai_selected(index);
                        ui.set_ai_query(row.text.clone());
                        ui.set_ai_detail(row.detail);
                        row.text.to_string()
                    } else if kind == 21 {
                        let Some(row) = usize::try_from(index)
                            .ok()
                            .and_then(|i| ui.get_ai_rows().row_data(i))
                        else {
                            return;
                        };
                        ui.set_ai_selected(index);
                        ui.set_ai_query(row.text.clone());
                        ui.set_ai_detail(row.detail);
                        row.text.to_string()
                    } else {
                        ui.get_ai_query().trim().to_owned()
                    };
                    if validate_ai_query(&query).is_err() {
                        ui.set_ai_status(c.localized(
                            "请输入不含布尔运算符的关键词，最多 500 字符",
                            "Use plain keywords without Boolean operators, up to 500 characters",
                        ));
                        return;
                    }
                    if kind != 7 {
                        ui.set_query(query.into());
                        c.request_search(&ui, true, true);
                        *c.ai.context.lock().unwrap() = Some(c.ai_context());
                    } else {
                        c.open_ai_query(&ui, &query);
                    }
                }
                8 => match draft(&ui, true) {
                    Ok(p) => {
                        let key = if ui.get_ai_clear_key() {
                            Some(ApiKey::default())
                        } else if ui.get_ai_key().is_empty() {
                            None
                        } else {
                            match ApiKey::new(ui.get_ai_key().to_string()) {
                                Ok(k) => Some(k),
                                Err(e) => {
                                    c.ai_error(&ui, e);
                                    return;
                                }
                            }
                        };
                        c.ai_job(&ui, Job::Save(p, key));
                    }
                    Err(e) => c.ai_error(&ui, e),
                },
                9 => {
                    c.remember_ai_draft(&ui);
                    c.ai.drafts.lock().unwrap().remove("");
                    c.cancel_ai_task_batch(&ui);
                    c.ai.cancel();
                    c.clear_ai_suggestions(&ui);
                    ui.set_ai_provider_index(0);
                    c.select_ai_profile(&ui, 0);
                    ui.set_ai_status("".into());
                }
                10 => {
                    if !ui.get_ai_profile_id().is_empty() {
                        c.ai_job(&ui, Job::Remove(ui.get_ai_profile_id().to_string()));
                    }
                }
                11 | 17 => {
                    // Discover directly from the form; the worker resolves a saved key
                    // only when its endpoint and protocol still match this draft.
                    let input = draft(&ui, kind == 17).and_then(|profile| {
                        let key = if ui.get_ai_clear_key() {
                            Some(ApiKey::default())
                        } else if ui.get_ai_key().is_empty() {
                            None
                        } else {
                            Some(ApiKey::new(ui.get_ai_key().to_string())?)
                        };
                        Ok((profile, key))
                    });
                    match input {
                        Ok((profile, key)) => c.ai_job(
                            &ui,
                            if kind == 17 {
                                Job::Probe(profile, key)
                            } else {
                                Job::Models(profile, key)
                            },
                        ),
                        Err(e) => c.ai_error(&ui, e),
                    }
                }
                12 => {
                    if let Ok(protocol) = Protocol::from_index(index) {
                        // Selecting the same dropdown item is not a provider change.
                        if c.ai.draft_protocol.swap(index, Ordering::AcqRel) == index {
                            return;
                        }
                        ui.set_ai_endpoint(protocol.default_url().into());
                        ui.set_ai_model(if protocol == Protocol::Jev {
                            "jev-latest".into()
                        } else {
                            "".into()
                        });
                        ui.set_ai_key("".into());
                        ui.set_ai_go_authorized(false);
                        ui.set_ai_subscription(protocol == Protocol::OpenCodeGo);
                        ui.set_ai_models(ModelRc::default());
                        ui.set_ai_dirty(true);
                        c.refresh_ai_name_hint(&ui);
                    }
                }
                18 => {
                    c.ai.drafts
                        .lock()
                        .unwrap()
                        .remove(ui.get_ai_profile_id().as_str());
                    c.select_ai_profile(&ui, ui.get_ai_provider_index());
                    ui.set_ai_status(c.localized(
                        "已恢复保存的配置；未保存的新配置已重置",
                        "Saved profile restored; unsaved new profile reset",
                    ));
                }
                14 => {
                    c.cancel_ai_task_batch(&ui);
                    c.ai.cancel();
                    c.clear_ai_suggestions(&ui);
                    let custom =
                        c.ai.store
                            .lock()
                            .unwrap()
                            .as_ref()
                            .map(|s| s.prompt().custom.clone())
                            .unwrap_or_else(|| DEFAULT_PROMPT.into());
                    ui.set_ai_instruction(if index == 0 {
                        DEFAULT_PROMPT.into()
                    } else {
                        custom.into()
                    });
                    ui.set_ai_status(c.localized(
                        "提示词已选择；保存后重启保留",
                        "Prompt selected; save to retain across restarts",
                    ));
                }
                15 => {
                    if validate_instruction(&ui.get_ai_instruction()).is_err() {
                        ui.set_ai_status(c.localized(
                            "提示词需为 1–2000 字符",
                            "Prompt must contain 1–2000 characters",
                        ));
                        return;
                    }
                    c.ai_job(
                        &ui,
                        Job::SavePrompt(
                            ui.get_ai_preset_index() == 1,
                            ui.get_ai_instruction().to_string(),
                        ),
                    );
                }
                _ => {}
            }
        });
    }

    pub(super) fn cancel_ai_task_batch(&self, ui: &AppWindow) {
        self.ai.batch_revision.fetch_add(1, Ordering::AcqRel);
        ui.set_ai_batch_searching(false);
        ui.set_ai_batch_completed(0);
        ui.set_ai_batch_total(0);
    }

    fn admit_ai_suggestions(&self, ui: &AppWindow) -> bool {
        if !self.available() || ui.get_ai_batch_blocked() {
            return false;
        }
        if !self.ai_suggestions_match_identity() {
            self.clear_ai_suggestions(ui);
            ui.set_ai_status(self.localized(
                "搜索上下文已变化，请重新生成建议",
                "Search context changed; generate suggestions again",
            ));
            return false;
        }
        // A completed plan belongs to its sound description and library. Its
        // original hit probes do not prevent applying it to another folder.
        // The chosen recommendation replaces only the ordinary query draft.
        // Keep validating all directory/filter drafts against the admitted
        // request so applying it cannot silently broaden the search conditions.
        self.capture_search_task(ui);
        if self.ai_captured_condition_request().is_none() {
            ui.set_ai_status(self.localized(
                "请先完成当前目录和筛选条件的有效搜索",
                "Finish a valid search for the current directory and filters first",
            ));
            return false;
        }
        true
    }

    fn ai_suggestions_match_identity(&self) -> bool {
        let current = self.ai_context();
        self.ai
            .context
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|context| {
                context.revision == current.revision
                    && context.library == current.library
                    && context.source == current.source
            })
            && self
                .ai
                .verified_search
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|verified| {
                    verified
                        .provider
                        .upgrade()
                        .is_some_and(|provider| Arc::ptr_eq(&provider, &self.provider()))
                })
    }

    // Zero hits prove an original recommendation stale only under the original
    // verification conditions. A narrower folder/filter is not library-wide
    // evidence against the sound keyword itself.
    fn ai_uses_verified_conditions(&self, ui: &AppWindow, request: &SearchRequest) -> bool {
        self.ai
            .verified_search
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|verified| {
                verified.scope_choice == ui.get_search_scope_index()
                    && same_verified_scope(&verified.request, request, verified.scope_choice == 0)
            })
    }

    // Both modes share admission, atomic task allocation and ordinary cache
    // warming. Manual All uses every tag; opt-in automation stays limited to 3.
    fn search_ai_recommendations(self: &Arc<Self>, ui: &AppWindow, limit: usize) {
        if !self.admit_ai_suggestions(ui) {
            return;
        }
        self.cancel_ai_task_batch(ui);
        let queries = ui
            .get_ai_quick_rows()
            .iter()
            .take(limit.min(MAX_AI_RECOMMENDATIONS))
            .map(|row| row.text.to_string())
            .collect::<Vec<_>>();
        let executed_scope = self.request.lock().unwrap().scope;
        let ids = self.open_ai_queries(ui, &queries);
        self.start_ai_task_batch(ui, ids, executed_scope);
    }

    // An AI tag can be clicked during the short identity-restore phase. Cancel
    // automatic advancement now, but apply the user's choice only after identity
    // validation; a newer gesture, editor change or source boundary supersedes it.
    fn defer_ai_apply(self: &Arc<Self>, ui: &AppWindow, pending: PendingApply) {
        let controller = self.clone();
        let weak = ui.as_weak();
        slint::Timer::single_shot(Duration::from_millis(25), move || {
            let Some(ui) = weak.upgrade() else { return };
            let current = controller.ai_context();
            if !controller.available()
                || ui.get_ai_batch_blocked()
                || current.revision != pending.context.revision
                || current.library != pending.context.library
                || current.source != pending.context.source
                || current.task != pending.context.task
                || controller.ai.batch_revision.load(Ordering::Acquire) != pending.token
                || SearchControls::capture(&ui) != pending.controls
                || ui.get_ai_query() != pending.query
            {
                return;
            }
            if ui.get_task_restoring() || ui.get_search_workspace_loading() {
                if pending.started.elapsed() < Duration::from_secs(5) {
                    controller.defer_ai_apply(&ui, pending);
                }
                return;
            }
            ui.invoke_ai_action(pending.kind, pending.index);
        });
    }

    /// Existing request admission calls this after capturing editor values.
    /// Folder/filter navigation retains the plan; only its sound/library identity
    /// can invalidate it. Original-context zero-hit executions may prune a tag.
    pub(super) fn invalidate_ai_suggestions_for_conditions(&self, ui: &AppWindow) {
        if ui.get_task_restoring()
            || ui.get_search_workspace_loading()
            || self.ai.verified_search.lock().unwrap().is_none()
        {
            return;
        }
        if !self.ai_suggestions_match_identity() {
            self.cancel_ai_task_batch(ui);
            self.clear_ai_suggestions(ui);
            ui.set_ai_status(self.localized(
                "搜索上下文已变化，请重新生成建议",
                "Search context changed; generate suggestions again",
            ));
            return;
        }
        if let Some(request) = self.ai_captured_condition_request() {
            self.prune_ai_suggestion_without_hits(ui, &request);
        }
    }

    // A later complete execution is stronger evidence than the original probe.
    // Use only the exact admitted tag query and its published ordinary cache;
    // drafts, failed pages and custom searches cannot prune recommendations.
    fn prune_ai_suggestion_without_hits(&self, ui: &AppWindow, request: &SearchRequest) {
        if ui.get_ai_batch_searching()
            || ui.get_searching()
            || !self.ai_uses_verified_conditions(ui, request)
            || ui.get_query() != request.query
            || request.offset != 0
            || request.cursor.is_some()
            || request.page.is_some_and(|page| page > 1)
        {
            return;
        }
        let generation = self.search_generation.load(Ordering::Acquire);
        if !self
            .ai
            .verified_search
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|verified| generation > verified.search)
            || !self
                .search_publication
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|publication| {
                    publication.generation == generation
                        && publication.page == SearchPagePhase::Published
                        && publication.model_published
                })
        {
            return;
        }
        let Some(task) = self.ai_context().task else {
            return;
        };
        if self.ai_task_batch_ready(&task) != Some(true)
            || self.ai_task_batch_has_results(&task) != Some(false)
        {
            return;
        }
        let key = ai_query_key(&request.query);
        let mut rows = ui.get_ai_quick_rows().iter().collect::<Vec<_>>();
        let count = rows.len();
        rows.retain(|row| ai_query_key(&row.text) != key);
        if rows.len() == count {
            return;
        }
        let edit = ui.get_ai_query();
        if let Some(index) = rows
            .iter()
            .position(|row| row.text.split_whitespace().count() == 1)
        {
            let single = rows.remove(index);
            rows.insert(0, single);
            ui.set_ai_quick_rows(ModelRc::new(VecModel::from(rows.clone())));
            ui.set_ai_rows(ModelRc::new(VecModel::from(rows.clone())));
            if let Some(index) = rows
                .iter()
                .position(|row| ai_query_key(&row.text) == ai_query_key(&edit))
            {
                ui.set_ai_selected(index as i32);
                ui.set_ai_detail(rows[index].detail.clone());
            } else if ai_query_key(&edit) == key {
                ui.set_ai_selected(0);
                ui.set_ai_query(rows[0].text.clone());
                ui.set_ai_detail(rows[0].detail.clone());
            } else {
                ui.set_ai_selected(-1);
                ui.set_ai_detail("".into());
            }
        } else {
            // Do not retain a pair-only plan after its sole verified broad
            // single disappears. Keep an unrelated custom editor draft intact.
            self.clear_ai_suggestions(ui);
            if ai_query_key(&edit) != key {
                ui.set_ai_query(edit);
            }
        }
        ui.set_ai_status(self.localized(
            "资料库结果已变化；无结果推荐已移除，可重新生成关键词",
            "Library results changed; the empty recommendation was removed. Regenerate keywords if needed",
        ));
    }

    // All probes use the already-admitted executable request. Only the candidate
    // text and first-row window change; directory IDs and filters never broaden.
    fn verify_ai_plan(
        &self,
        provider: &Arc<SqliteProvider>,
        base: &SearchRequest,
        context: &Context,
        cancel: &Arc<AtomicBool>,
        plan: &mut AiQueryPlan,
    ) -> audiolib_core::Result<()> {
        let cancelled = || {
            cancel.load(Ordering::Acquire)
                || !self.available()
                || self.ai_context() != *context
                || !Arc::ptr_eq(provider, &self.provider())
        };
        if cancelled() {
            return Err(audiolib_core::Error::Cancelled);
        }
        let revision = provider
            .search_revision()?
            .ok_or(audiolib_core::Error::Cancelled)?;
        let has_results = |text: &str| -> audiolib_core::Result<bool> {
            if cancelled() {
                return Err(audiolib_core::Error::Cancelled);
            }
            validate_ai_query(text)?;
            let mut request = base.clone();
            request.query = text.to_owned();
            request.limit = 1;
            request.offset = 0;
            request.cursor = None;
            request.page = None;
            if request.needs_more_search_text() {
                return Ok(false);
            }
            Ok(!provider
                .search_cancellable(&request, cancel.clone())?
                .results
                .is_empty())
        };
        let mut seen = std::collections::HashSet::new();
        let mut verified = Vec::new();
        let ranked_queries = std::mem::take(&mut plan.queries);
        for mut query in ranked_queries.iter().cloned() {
            if cancelled() {
                return Err(audiolib_core::Error::Cancelled);
            }
            let mut matched = None;
            for text in short_ai_query_variants(&query.text) {
                if cancelled() {
                    return Err(audiolib_core::Error::Cancelled);
                }
                if !seen.insert(ai_query_key(&text)) {
                    continue;
                }
                if text.split_whitespace().count() == 1 && is_ai_query_modifier(&text) {
                    continue;
                }
                if has_results(&text)? {
                    matched = Some(text);
                    break;
                }
            }
            if let Some(text) = matched {
                // One concise, verified variant represents this source scheme;
                // its reason, layer and provenance remain unchanged.
                query.text = text;
                verified.push(query);
            }
        }
        // Provider/local order is the ranking. Prefer its strongest verified
        // single word, preserving the relative order of the remaining schemes.
        if let Some(index) = verified
            .iter()
            .position(|query| query.text.split_whitespace().count() == 1)
        {
            let single = verified.remove(index);
            verified.insert(0, single);
        } else {
            // A ranked scheme's core word may match even when its AND pair
            // does not. Probe original candidates without broadening the scope,
            // retaining the strongest matching source's reason and provenance.
            let mut single = None;
            for query in &ranked_queries {
                if let Some(anchor) = query
                    .text
                    .split_whitespace()
                    .find(|word| !is_ai_query_modifier(word))
                    && has_results(anchor)?
                {
                    let mut anchored = query.clone();
                    anchored.text = anchor.to_owned();
                    single = Some(anchored);
                    break;
                }
            }
            if let Some(single) = single {
                verified.insert(0, single);
            } else {
                verified.clear();
            }
        }
        let mut final_seen = std::collections::HashSet::new();
        verified.retain(|query| final_seen.insert(ai_query_key(&query.text)));
        verified.truncate(MAX_AI_RECOMMENDATIONS);
        if cancelled() || provider.search_revision()? != Some(revision) {
            return Err(audiolib_core::Error::Cancelled);
        }
        plan.queries = verified;
        Ok(())
    }

    fn start_ai_task_batch(
        self: &Arc<Self>,
        ui: &AppWindow,
        ids: Vec<String>,
        executed_scope: SearchScope,
    ) {
        if ids.is_empty() {
            return;
        }
        let token = self.ai.batch_revision.fetch_add(1, Ordering::AcqRel) + 1;
        let batch = TaskBatch {
            ids,
            index: 0,
            context: self.ai_context(),
            request: self.ai_batch_expected_request(executed_scope),
            controls: BatchControls {
                scroll: [0.0, 0.0],
                ..BatchControls::capture(ui)
            },
            started: Instant::now(),
            status: self
                .ai
                .verified_search
                .lock()
                .unwrap()
                .as_ref()
                .map_or_else(|| ui.get_ai_status(), |verified| verified.status.clone()),
            scroll_settled: false,
        };
        ui.set_ai_batch_searching(true);
        ui.set_ai_batch_completed(0);
        ui.set_ai_batch_total(batch.ids.len() as i32);
        ui.set_ai_status(batch.status.clone());
        self.schedule_ai_task_batch(ui, token, batch);
    }

    fn ai_batch_expected_request(&self, executed_scope: SearchScope) -> SearchRequest {
        let mut request = self.request.lock().unwrap().clone();
        if executed_scope == SearchScope::Main && request.scope == SearchScope::All {
            request.scope = SearchScope::Main;
        }
        request
    }

    // Reuse the normal restore/search/capture path serially. It owns bounded
    // result windows, provider revisions, directory facets and Metadata draining.
    fn schedule_ai_task_batch(self: &Arc<Self>, ui: &AppWindow, token: u64, batch: TaskBatch) {
        let weak = ui.as_weak();
        let controller = self.clone();
        slint::Timer::single_shot(Duration::from_millis(25), move || {
            let Some(ui) = weak.upgrade() else { return };
            controller.advance_ai_task_batch(&ui, token, batch);
        });
    }

    fn advance_ai_task_batch(self: &Arc<Self>, ui: &AppWindow, token: u64, mut batch: TaskBatch) {
        if self.ai.batch_revision.load(Ordering::Acquire) != token {
            return;
        }
        let context = self.ai_context();
        let request = self.request.lock().unwrap().clone();
        if !self.available()
            || ui.get_ai_batch_blocked()
            || context.revision != batch.context.revision
            || context.library != batch.context.library
            || context.source != batch.context.source
            || context.task.as_ref() != batch.ids.get(batch.index)
            || !same_batch_controls(
                &BatchControls::capture(ui),
                &batch.controls,
                ui.get_task_restoring() || !batch.scroll_settled,
            )
            || !same_batch_request(
                &batch.request,
                &request,
                batch.controls.search.choices[0] == 0,
            )
        {
            // A user action or source boundary owns the UI now. Never restore a
            // previous tab or overwrite its new search just to finish this queue.
            self.cancel_ai_task_batch(ui);
            return;
        }
        if batch.started.elapsed() >= Duration::from_secs(60)
            || self
                .search_publication
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|publication| publication.page == SearchPagePhase::Failed)
        {
            self.cancel_ai_task_batch(ui);
            ui.set_ai_status(self.localized(
                "批量搜索已停止；已创建的任务保留，可逐个重试",
                "Batch search stopped; created tasks are retained and can be retried individually",
            ));
            return;
        }
        if !ui.get_searching() && !ui.get_task_restoring() {
            self.capture_search_task(ui);
        }
        let ready = self.ai_task_batch_ready(&batch.ids[batch.index]);
        if ready.is_none() {
            self.cancel_ai_task_batch(ui);
            return;
        }
        if ready != Some(true) || ui.get_searching() || ui.get_task_restoring() {
            self.schedule_ai_task_batch(ui, token, batch);
            return;
        }
        if self.ai_uses_verified_conditions(ui, &request)
            && self.ai_task_batch_has_results(&batch.ids[batch.index]) != Some(true)
        {
            // A matching record can disappear after preflight. Do not retain a
            // verified-hit tag or an empty automatic tab under that old claim.
            self.cancel_ai_task_batch(ui);
            self.close_search_task(&batch.ids[batch.index]);
            self.clear_ai_suggestions(ui);
            let retained = self
                .task_workspace
                .lock()
                .unwrap()
                .workspace
                .session
                .tasks
                .iter()
                .any(|task| task.id == batch.ids[batch.index]);
            ui.set_ai_status(if retained {
                self.localized("资料库结果已变化；推荐已清除，待保存完成后可关闭无结果任务", "Library results changed; recommendations cleared. Close the empty task after pending saves finish")
            } else {
                self.localized("资料库结果已变化；无结果任务已移除，请重新生成推荐", "Library results changed; the empty task was removed. Regenerate suggestions")
            });
            return;
        }
        if !batch.scroll_settled {
            // Viewport change handlers run after the timer that clears the UI's
            // restore flag. Give that pass one turn before treating scroll as input.
            batch.scroll_settled = true;
            self.schedule_ai_task_batch(ui, token, batch);
            return;
        }
        let completed = batch.index + 1;
        ui.set_ai_batch_completed(completed as i32);
        if completed == batch.ids.len() {
            // Every tab now owns a valid normal result cache. The user's initial
            // first query is selected again without rerunning completed searches.
            self.cancel_ai_task_batch(ui);
            self.switch_ai_batch_task(&batch.ids[0]);
            ui.set_ai_status(batch.status);
            return;
        }
        batch.index = completed;
        batch.scroll_settled = false;
        self.switch_ai_batch_task(&batch.ids[batch.index]);
        batch.request = self.ai_batch_expected_request(batch.request.scope);
        batch.controls = BatchControls {
            scroll: [0.0, 0.0],
            ..BatchControls::capture(ui)
        };
        self.schedule_ai_task_batch(ui, token, batch);
    }

    fn clear_ai_suggestions(&self, ui: &AppWindow) {
        self.cancel_ai_task_batch(ui);
        ui.set_ai_generating(false);
        self.ai.verified_search.lock().unwrap().take();
        ui.set_ai_rows(ModelRc::default());
        ui.set_ai_quick_rows(ModelRc::default());
        ui.set_ai_selected(-1);
        ui.set_ai_query("".into());
        ui.set_ai_detail("".into());
    }

    fn set_ai_draft(&self, ui: &AppWindow, p: &Profile) {
        self.ai
            .draft_protocol
            .store(p.protocol.index(), Ordering::Release);
        ui.set_ai_profile_id(p.id.clone().into());
        ui.set_ai_profile_name(p.name.clone().into());
        ui.set_ai_protocol(p.protocol.index());
        ui.set_ai_endpoint(p.base_url.clone().into());
        ui.set_ai_model(p.model.clone().into());
        ui.set_ai_key("".into());
        ui.set_ai_clear_key(false);
        ui.set_ai_subscription(p.subscription);
        ui.set_ai_go_authorized(p.go_use_authorized);
        ui.set_ai_timeout(p.timeout_seconds as i32);
        ui.set_ai_request_cap(p.max_requests_per_month as i32);
        ui.set_ai_models(ModelRc::default());
        ui.set_ai_dirty(false);
    }

    fn remember_ai_draft(&self, ui: &AppWindow) {
        // Keep at most one draft per saved profile plus the new-profile form.
        let profile = Profile {
            id: ui.get_ai_profile_id().to_string(),
            name: ui.get_ai_profile_name().to_string(),
            protocol: Protocol::from_index(ui.get_ai_protocol()).unwrap_or_default(),
            base_url: ui.get_ai_endpoint().to_string(),
            model: ui.get_ai_model().to_string(),
            timeout_seconds: ui.get_ai_timeout().max(0) as u64,
            max_requests_per_month: ui.get_ai_request_cap().max(0) as u64,
            subscription: ui.get_ai_subscription(),
            go_use_authorized: ui.get_ai_go_authorized(),
            ..Default::default()
        };
        self.ai.drafts.lock().unwrap().insert(
            profile.id.clone(),
            FormDraft {
                profile,
                key: ui.get_ai_key().to_string(),
                clear_key: ui.get_ai_clear_key(),
                dirty: ui.get_ai_dirty(),
                models: ui.get_ai_models().iter().map(|s| s.to_string()).collect(),
            },
        );
    }

    fn refresh_ai_name_hint(&self, ui: &AppWindow) {
        let protocol = Protocol::from_index(ui.get_ai_protocol()).unwrap_or_default();
        let name = self.ai.store.lock().unwrap().as_ref().map_or_else(
            || format!("{} 1", protocol.display_name()),
            |s| s.next_profile_name(protocol, ui.get_ai_profile_id().as_str()),
        );
        ui.set_ai_auto_name(name.into());
    }

    fn select_ai_profile(&self, ui: &AppWindow, index: i32) {
        let store = self.ai.store.lock().unwrap();
        let saved = if let Some(s) = store.as_ref()
            && let Some(p) = index
                .checked_sub(1)
                .and_then(|i| usize::try_from(i).ok())
                .and_then(|i| s.profiles().get(i))
        {
            self.publish_ai_usage(ui, s, p);
            ui.set_ai_saved_key(s.has_key(p));
            ui.set_ai_saved_endpoint(p.base_url.clone().into());
            ui.set_ai_saved_protocol(p.protocol.index());
            p.clone()
        } else {
            ui.set_ai_usage("".into());
            ui.set_ai_saved_key(false);
            ui.set_ai_saved_endpoint("".into());
            ui.set_ai_saved_protocol(-1);
            Profile {
                id: String::new(),
                name: String::new(),
                ..Default::default()
            }
        };
        drop(store);
        let cached = self.ai.drafts.lock().unwrap().get(&saved.id).cloned();
        if let Some(cached) = cached {
            self.set_ai_draft(ui, &cached.profile);
            ui.set_ai_key(cached.key.into());
            ui.set_ai_clear_key(cached.clear_key);
            ui.set_ai_dirty(cached.dirty);
            ui.set_ai_models(ModelRc::new(VecModel::from(
                cached
                    .models
                    .into_iter()
                    .map(Into::into)
                    .collect::<Vec<slint::SharedString>>(),
            )));
        } else {
            self.set_ai_draft(ui, &saved);
        }
        self.refresh_ai_name_hint(ui);
    }
    fn publish_ai_usage(&self, ui: &AppWindow, store: &ProfileStore, p: &Profile) {
        let u = store.usage(&p.id);
        ui.set_ai_usage(
            format!(
                "{} · {} / {} {} · tokens {} + {}\n{}",
                u.month,
                u.requests,
                p.max_requests_per_month,
                if self.chinese.load(Ordering::Acquire) {
                    "本应用请求（失败尝试也计入）"
                } else {
                    "app requests (failed attempts included)"
                },
                u.input_tokens,
                u.output_tokens,
                if self.chinese.load(Ordering::Acquire) {
                    "供应商余额：未知；测试使用当前表单，获取模型后保存"
                } else {
                    "Provider balance: unknown; test the current form, then save"
                }
            )
            .into(),
        );
    }
    fn publish_ai_profiles(&self, ui: &AppWindow) {
        let store = self.ai.store.lock().unwrap();
        let mut names = vec![self.localized("本地词表（离线）", "Local reference (offline)")];
        if let Some(s) = store.as_ref() {
            names.extend(s.profiles().iter().map(|p| p.name.clone().into()));
            if let Some(p) = s
                .profiles()
                .iter()
                .find(|p| p.id == ui.get_ai_profile_id().as_str())
            {
                self.publish_ai_usage(ui, s, p);
            }
        }
        ui.set_ai_providers(ModelRc::new(VecModel::from(names)));
        drop(store);
        self.refresh_ai_name_hint(ui);
    }
    fn ai_error(&self, ui: &AppWindow, error: AiError) {
        ui.set_ai_error(true);
        let message=match error {
            AiError::Invalid=>self.localized("配置或输入无效；检查模型 ID、URL、长度，更换地址须重新输入密钥", "Invalid input/config: check model, URL, limits; re-enter key after changing endpoint"),
            AiError::Uncertain=>self.localized("Jev 判断不够确定；请细化描述或切换本地词表", "Jev decision uncertain; refine the description or use local reference"),
            AiError::Response=>self.localized("未找到合适词条，或供应商结果未通过校验；请细化描述", "No matching reference or invalid provider result; refine the description"),
            AiError::Storage=>self.localized("AI 配置保存/读取失败；原配置已保留", "AI configuration could not be read/saved; original retained"),
            AiError::Transport=>self.localized("连接失败或超时；检查地址、网络和密钥", "Connection failed or timed out; check endpoint, network and key"),
            AiError::Http(401)=>self.localized("认证失败（401）；请检查 API Key", "Authentication failed (401); check the API key"),
            AiError::Http(403)=>self.localized("访问被拒绝（403）；请检查账户或订阅权限", "Access denied (403); check account or subscription permissions"),
            AiError::Http(404)=>self.localized("接口不存在（404）；请检查 Base URL，也可手动填写模型 ID", "Endpoint not found (404); check Base URL or enter a model ID manually"),
            AiError::Http(429)=>self.localized("供应商限流或额度不足（429）；请稍后重试或检查账户", "Provider rate/quota limit (429); retry later or check the account"),
            AiError::Budget=>self.localized("已达到本应用的每月请求上限", "This app's monthly request cap has been reached"),
            other=>other.to_string().into(),
        };
        ui.set_ai_status(message);
    }

    fn ai_job(self: &Arc<Self>, ui: &AppWindow, job: Job) {
        if self.ai.busy.swap(true, Ordering::AcqRel) {
            return;
        }
        self.cancel_ai_task_batch(ui);
        self.ai.cancel();
        let cancel = Arc::new(AtomicBool::new(false));
        *self.ai.cancel.lock().unwrap() = cancel.clone();
        let context = self.ai_context();
        let probe_provider = self.provider();
        let probe_request = self.request.lock().unwrap().clone();
        let probe_controls = SearchControls::capture(ui);
        let saving = matches!(job, Job::Save(..) | Job::Remove(..) | Job::SavePrompt(..));
        // One submission owns its automatic-search choice. Profile operations
        // and model probes never create tasks, even if the option is enabled.
        let generating = matches!(&job, Job::Local(..) | Job::Generate(..));
        let auto_search = generating && ui.get_ai_auto_search();
        if saving {
            *self.ai.saves.lock().unwrap() = true;
            *self.ai.save_error.lock().unwrap() = None;
        }
        ui.set_ai_busy(true);
        ui.set_ai_error(false);
        ui.set_ai_status(if generating {
            self.localized(
                "正在生成并验证关键词…",
                "Generating and verifying keywords…",
            )
        } else {
            self.localized("正在处理…", "Working…")
        });
        let discovery = matches!(&job, Job::Models(..));
        if discovery {
            ui.set_ai_models(ModelRc::default());
        }
        self.clear_ai_suggestions(ui);
        ui.set_ai_generating(generating);
        let root = self.config.lock().unwrap().data_root();
        let c = self.clone();
        std::thread::spawn(move || {
            let result = (|| -> audiolib_ai::Result<Reply> {
                {
                    let mut store = c.ai.store.lock().unwrap();
                    if store.is_none() && !matches!(job, Job::Local(..)) {
                        *store = Some(ProfileStore::open(&root)?);
                    }
                }
                let reply = match job {
                    Job::Load => Ok(Reply::Loaded),
                    Job::Local(intent, layers) => Ok(Reply::Plan(
                        ReferenceIndex::embedded().local_plan(&intent, layers)?,
                        String::new(),
                    )),
                    Job::Save(profile, key) => {
                        c.ai.store
                            .lock()
                            .unwrap()
                            .as_mut()
                            .unwrap()
                            .save(profile, key)?;
                        Ok(Reply::Saved)
                    }
                    Job::Remove(id) => {
                        c.ai.store.lock().unwrap().as_mut().unwrap().remove(&id)?;
                        Ok(Reply::Removed)
                    }
                    Job::SavePrompt(custom, text) => {
                        c.ai.store
                            .lock()
                            .unwrap()
                            .as_mut()
                            .unwrap()
                            .save_prompt(custom, &text)?;
                        Ok(Reply::PromptSaved)
                    }
                    Job::Models(profile, supplied) => {
                        let key =
                            c.ai.store
                                .lock()
                                .unwrap()
                                .as_ref()
                                .unwrap()
                                .discovery_key(&profile, supplied)?;
                        Ok(Reply::Models(provider::models(&profile, &key, &cancel)?))
                    }
                    Job::Probe(profile, supplied) => {
                        profile.validate_request()?;
                        let (key, tracked) = {
                            let mut store = c.ai.store.lock().unwrap();
                            let store = store.as_mut().unwrap();
                            let key = store.discovery_key(&profile, supplied)?;
                            if cancel.load(Ordering::Acquire) || c.closing.load(Ordering::Acquire) {
                                return Err(AiError::Cancelled);
                            }
                            let saved = store
                                .profiles()
                                .iter()
                                .find(|p| p.id == profile.id)
                                .cloned();
                            if let Some(saved) = &saved {
                                store.reserve(
                                    saved,
                                    &chrono::Utc::now().format("%Y-%m").to_string(),
                                )?;
                            }
                            (key, saved.is_some())
                        };
                        // A real structured round trip, not the often-public model list.
                        // Uses a fixed example, never the user's library or current prompt.
                        let output = provider::generate(
                            &profile,
                            &key,
                            "wind",
                            false,
                            DEFAULT_PROMPT,
                            &format!("audiolib-{}-probe", std::process::id()),
                            &cancel,
                        )?;
                        if tracked {
                            let mut store = c.ai.store.lock().unwrap();
                            if !c.closing.load(Ordering::Acquire) && !cancel.load(Ordering::Acquire)
                            {
                                store.as_mut().unwrap().record_tokens(
                                    &profile.id,
                                    output.input_tokens,
                                    output.output_tokens,
                                )?;
                            }
                        }
                        Ok(Reply::Probed)
                    }
                    Job::Generate(profile, intent, layers, guidance) => {
                        profile.validate_request()?;
                        audiolib_core::ai_search::validate_intent(&intent)
                            .map_err(|_| AiError::Invalid)?;
                        validate_instruction(&guidance)?;
                        // Candidate absence is not a paid decision request.
                        if profile.protocol == Protocol::Jev {
                            audiolib_ai::jev::request(&profile.model, &intent, &guidance)?;
                        }
                        let key = {
                            let mut store = c.ai.store.lock().unwrap();
                            let s = store.as_mut().unwrap();
                            if cancel.load(Ordering::Acquire)
                                || c.closing.load(Ordering::Acquire)
                                || c.shutdown.load(Ordering::Acquire)
                            {
                                return Err(AiError::Cancelled);
                            }
                            let key = s.key(&profile)?;
                            s.reserve(&profile, &chrono::Utc::now().format("%Y-%m").to_string())?;
                            key
                        };
                        let output = provider::generate(
                            &profile,
                            &key,
                            &intent,
                            layers,
                            &guidance,
                            &format!("audiolib-{}-{}", std::process::id(), context.source),
                            &cancel,
                        )?;
                        let accounting = {
                            let mut store = c.ai.store.lock().unwrap();
                            if cancel.load(Ordering::Acquire)
                                || c.closing.load(Ordering::Acquire)
                                || c.shutdown.load(Ordering::Acquire)
                            {
                                return Err(AiError::Cancelled);
                            }
                            store.as_mut().unwrap().record_tokens(
                                &profile.id,
                                output.input_tokens,
                                output.output_tokens,
                            )
                        };
                        let mut detail = output
                            .decision
                            .map(|d| {
                                format!(
                                    "Jev Choice={} p={:.2} · Score={:.2}/2 · Noul={:.2}",
                                    d.recipe, d.confidence, d.fit, d.clarification
                                )
                            })
                            .unwrap_or_default();
                        if accounting.is_err() {
                            detail.push_str(" · Token accounting could not be saved");
                        }
                        Ok(Reply::Plan(output.plan, detail))
                    }
                }?;
                if let Reply::Plan(mut plan, evidence) = reply {
                    match c.verify_ai_plan(
                        &probe_provider,
                        &probe_request,
                        &context,
                        &cancel,
                        &mut plan,
                    ) {
                        Ok(()) => Ok(Reply::Plan(plan, evidence)),
                        Err(audiolib_core::Error::Cancelled) => Err(AiError::Cancelled),
                        Err(_) => Ok(Reply::SearchFailed),
                    }
                } else {
                    Ok(reply)
                }
            })();
            if saving {
                *c.ai.save_error.lock().unwrap() = result.as_ref().err().map(|e| e.to_string());
                *c.ai.saves.lock().unwrap() = false;
                c.ai.save_wake.notify_all();
            }
            let weak = c.ui.clone();
            let rejected = c.clone();
            if !try_post(weak, move |ui| {
                c.ai.busy.store(false, Ordering::Release);
                ui.set_ai_busy(false);
                ui.set_ai_generating(false);
                if c.closing.load(Ordering::Acquire) || c.shutdown.load(Ordering::Acquire) {
                    return;
                }
                c.publish_ai_profiles(&ui);
                if (cancel.load(Ordering::Acquire)
                    || c.ai_context() != context
                    || !Arc::ptr_eq(&probe_provider, &c.provider())
                    || (matches!(&result, Ok(Reply::Plan(..) | Reply::SearchFailed))
                        && SearchControls::capture(&ui) != probe_controls))
                    && !matches!(
                        &result,
                        Ok(Reply::Loaded | Reply::Saved | Reply::Removed | Reply::PromptSaved)
                    )
                {
                    ui.set_ai_status(c.localized(
                        "请求已取消或搜索上下文已改变",
                        "Request cancelled or search context changed",
                    ));
                    return;
                }
                match result {
                    Ok(Reply::Plan(plan, evidence)) => {
                        let mut seen = std::collections::HashSet::new();
                        let rows = plan
                            .queries
                            .into_iter()
                            .filter(|q| seen.insert(ai_query_key(&q.text)))
                            .map(|q| crate::AiSuggestion {
                                text: q.text.into(),
                                detail: format!(
                                    "{} · {}\nUCS: {} · ref: {}",
                                    q.layer,
                                    q.reason,
                                    q.cat_ids.join(", "),
                                    q.source_ids.join(", ")
                                )
                                .into(),
                            })
                            .collect::<Vec<_>>();
                        if rows.is_empty() {
                            c.clear_ai_suggestions(&ui);
                            ui.set_ai_status(c.localized(
                                "当前目录和筛选条件下没有匹配的推荐词；未创建搜索任务",
                                "No recommended queries match the current directory and filters; no tasks were created",
                            ));
                            return;
                        }
                        let count = rows.len();
                        let first = rows.first().cloned();
                        ui.set_ai_quick_rows(ModelRc::new(VecModel::from(rows.iter().take(MAX_AI_RECOMMENDATIONS).cloned().collect::<Vec<_>>())));
                        ui.set_ai_rows(ModelRc::new(VecModel::from(rows)));
                        if let Some(row) = first {
                            ui.set_ai_selected(0);
                            ui.set_ai_query(row.text);
                            ui.set_ai_detail(row.detail);
                        }
                        let verified_search = context.search;
                        *c.ai.context.lock().unwrap() = Some(context);
                        let status: slint::SharedString = format!("{} {} {}", evidence, plan.cautions.join(" · "), if count < MAX_AI_RECOMMENDATIONS {
                            c.localized(&format!("当前条件下仅有 {count} 个有结果的推荐方案"), &format!("Only {count} recommended queries have results under the current conditions"))
                        } else { "".into() }).trim().into();
                        *c.ai.verified_search.lock().unwrap() = Some(VerifiedSearch {
                            provider: Arc::downgrade(&probe_provider),
                            request: probe_request,
                            scope_choice: probe_controls.choices[0],
                            search: verified_search,
                            status: status.clone(),
                        });
                        ui.set_ai_status(status);
                        if auto_search {
                            c.search_ai_recommendations(&ui, MAX_AI_AUTO_TASKS);
                        }
                    }
                    Ok(Reply::Models(models)) => {
                        let empty = models.is_empty();
                        if ui.get_ai_model().trim().is_empty()
                            && let Some(model) = models.first()
                        {
                            ui.set_ai_model(model.clone().into());
                            ui.set_ai_dirty(true);
                        }
                        ui.set_ai_models(ModelRc::new(VecModel::from(
                            models
                                .into_iter()
                                .map(Into::into)
                                .collect::<Vec<slint::SharedString>>(),
                        )));
                        ui.set_ai_status(if empty {
                            c.localized("接口可达，但未返回可用模型；可手动填写模型 ID", "Endpoint reachable, but no usable models returned; enter a model ID manually")
                        } else {
                            c.localized("已获取模型列表；请选择模型并测试调用。列表不代表密钥或订阅有效。", "Models retrieved; select a model and test it. A catalogue does not verify the key or subscription.")
                        });
                    }
                    Ok(Reply::Probed) => ui.set_ai_status(c.localized(
                        "模型测试成功：认证、生成和结构化解析均通过；可保存配置",
                        "Model test passed: authentication, generation and structured parsing; ready to save",
                    )),
                    Ok(Reply::Saved) => {
                        c.ai.drafts.lock().unwrap().remove(ui.get_ai_profile_id().as_str());
                        let index = c
                            .ai
                            .store
                            .lock()
                            .unwrap()
                            .as_ref()
                            .and_then(|s| {
                                s.selected()
                                    .and_then(|p| s.profiles().iter().position(|x| x.id == p.id))
                            })
                            .map_or(0, |i| i as i32 + 1);
                        ui.set_ai_provider_index(index);
                        c.select_ai_profile(&ui, index);
                        ui.set_ai_status(format!("{}：{}", c.localized("配置已保存", "Profile saved"), ui.get_ai_profile_name()).into());
                    }
                    Ok(Reply::Removed) => {
                        c.ai.drafts.lock().unwrap().remove(ui.get_ai_profile_id().as_str());
                        ui.set_ai_provider_index(0);
                        c.select_ai_profile(&ui, 0);
                        ui.set_ai_status(c.localized("配置已删除", "Profile removed"));
                    }
                    Ok(Reply::Loaded) => {
                        let index = if let Some(store) = c.ai.store.lock().unwrap().as_ref() {
                            ui.set_ai_preset_index(i32::from(store.prompt().use_custom));
                            ui.set_ai_instruction(store.prompt().effective().into());
                            store.selected().and_then(|p| store.profiles().iter().position(|x| x.id == p.id)).map_or(0, |i| i as i32 + 1)
                        } else { 0 };
                        ui.set_ai_provider_index(index);
                        c.select_ai_profile(&ui, index);
                        ui.set_ai_status(if index == 0 { c.localized(
                            "本地词表已就绪，无需 API Key",
                            "Local reference ready; no API key needed",
                        ) } else { c.localized("已恢复保存的供应商配置", "Saved provider profile restored") });
                    }
                    Ok(Reply::PromptSaved) => {
                        ui.set_ai_status(c.localized("提示词已保存", "Prompt saved"));
                    }
                    Ok(Reply::SearchFailed) => {
                        c.clear_ai_suggestions(&ui);
                        ui.set_ai_error(true);
                        ui.set_ai_status(c.localized(
                            "资料库查询验证失败；请稍后重新生成推荐，未创建搜索任务",
                            "Library query verification failed; regenerate suggestions later. No tasks were created",
                        ));
                    }
                    Err(AiError::Response) if discovery => ui.set_ai_status(c.localized(
                        "模型列表格式无法识别；检查接口或手动填写模型 ID",
                        "Unrecognized model list; check the endpoint or enter a model ID manually",
                    )),
                    Err(e) => c.ai_error(&ui, e),
                }
            }) {
                rejected.ai.busy.store(false, Ordering::Release);
            }
        });
    }
}

// Long legacy/model queries get at most three core-word alternatives, not
// every pairwise combination. Ordinary manually entered searches are untouched.
fn short_ai_query_variants(text: &str) -> Vec<String> {
    let words = text.split_whitespace().collect::<Vec<_>>();
    if words.len() <= 2 {
        return if words.is_empty() {
            Vec::new()
        } else {
            vec![words.join(" ")]
        };
    }
    let mut seen = std::collections::HashSet::new();
    let core = words
        .into_iter()
        .filter(|word| {
            let normalized = word.to_ascii_lowercase();
            !is_ai_query_modifier(word) && seen.insert(normalized)
        })
        .collect::<Vec<_>>();
    let Some(anchor) = core.first() else {
        return Vec::new();
    };
    let mut variants = Vec::with_capacity(3);
    if let Some(second) = core.get(1) {
        variants.push(format!("{anchor} {second}"));
        if core.last() != Some(second) {
            variants.push(format!("{anchor} {}", core.last().unwrap()));
        }
    }
    variants.push((*anchor).to_owned());
    variants
}

fn is_ai_query_modifier(word: &str) -> bool {
    matches!(
        word.to_ascii_lowercase().as_str(),
        "very"
            | "really"
            | "extremely"
            | "heavy"
            | "heavily"
            | "strong"
            | "strongly"
            | "intense"
            | "loud"
            | "quiet"
            | "soft"
            | "faint"
            | "subtle"
            | "gentle"
            | "distant"
            | "near"
            | "nearby"
            | "close"
            | "far"
            | "remote"
            | "large"
            | "big"
            | "small"
            | "huge"
            | "massive"
            | "tiny"
            | "constant"
            | "steady"
            | "continuous"
            | "sustained"
            | "scifi"
            | "sci-fi"
            | "low"
            | "high"
            | "sub"
            | "deep"
    )
}

fn ai_query_key(text: &str) -> String {
    let mut words = text
        .split_whitespace()
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>();
    words.sort_unstable();
    words.dedup();
    words.join(" ")
}

fn same_batch_request(left: &SearchRequest, right: &SearchRequest, main_to_all: bool) -> bool {
    left.query == right.query
        && same_verified_scope(left, right, main_to_all)
        && left.offset == right.offset
        && left.cursor == right.cursor
        && left.page == right.page
}

fn same_verified_scope(left: &SearchRequest, right: &SearchRequest, main_to_all: bool) -> bool {
    (left.scope == right.scope
        || (main_to_all && left.scope == SearchScope::Main && right.scope == SearchScope::All))
        && left.folder_id == right.folder_id
        && left.include_subfolders == right.include_subfolders
        && left.filters == right.filters
        && left.order_by == right.order_by
        && left.descending == right.descending
}

fn same_batch_controls(left: &BatchControls, right: &BatchControls, restoring: bool) -> bool {
    left.search == right.search
        && left.candidate == right.candidate
        && left.selected == right.selected
        && left.checked == right.checked
        && (restoring || left.scroll == right.scroll)
}

fn draft(ui: &AppWindow, model_required: bool) -> audiolib_ai::Result<Profile> {
    let mut p = Profile {
        name: if ui.get_ai_profile_name().trim().is_empty() {
            ui.get_ai_auto_name().to_string()
        } else {
            ui.get_ai_profile_name().trim().into()
        },
        protocol: Protocol::from_index(ui.get_ai_protocol())?,
        base_url: ui.get_ai_endpoint().trim().trim_end_matches('/').into(),
        model: ui.get_ai_model().trim().into(),
        timeout_seconds: ui.get_ai_timeout().max(0) as u64,
        max_requests_per_month: ui.get_ai_request_cap().max(0) as u64,
        subscription: ui.get_ai_subscription(),
        go_use_authorized: ui.get_ai_go_authorized(),
        ..Default::default()
    };
    if !ui.get_ai_profile_id().is_empty() {
        p.id = ui.get_ai_profile_id().into();
    }
    if model_required {
        p.validate()?;
    } else {
        p.validate_discovery_request()?;
    }
    Ok(p)
}
