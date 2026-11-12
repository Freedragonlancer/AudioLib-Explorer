// app/audiolib-desktop/src/pagination_preferences_tests.rs
//! Paging policy compatibility and real provider navigation, using owned fixtures.
use super::super::{
    apply_task_controls, reconcile_paging_view, reset_paging_view, task_inputs_valid,
};
use super::scope_paging::{reference_pages, seed_folder_pages};
use super::*;

#[test]
fn legacy_policy_hints_do_not_turn_filters_into_default_editor_drafts() {
    isolated("pagination_preferences::child_legacy_policy_filters");
}

#[test]
#[ignore = "isolated legacy task/filter and favorite compatibility"]
fn child_legacy_policy_filters() {
    let fixture = fixture();
    let controller = &fixture.controller;
    let ui = &fixture.ui;
    let mut task = controller
        .task_workspace
        .lock()
        .unwrap()
        .workspace
        .session
        .tasks[0]
        .clone();
    task.definition.query = "wind".into();
    task.definition.filters.sample_rate = Some(48_000);
    task.definition.filters.channels = Some(2);
    task.definition.filters.bit_depth = Some(24);
    task.definition.filters.visibility = FileVisibility::Missing;
    task.definition.filters.favorites_only = true;
    task.view.raw_inputs.clear();
    let file = controller.records.lock().unwrap()[0].clone();
    let reference = super::super::audio_ref(&file);
    task.view.selected_audio = Some(reference.clone());
    task.view.checked_audio = vec![reference.clone()];
    task.view.selection_anchor = Some(reference);
    task.view.offset = 800;
    let definition = task.definition.clone();
    assert!(!reconcile_paging_view(
        &mut task.view,
        &AudioTablePreferences::default()
    ));
    assert_eq!(
        task.view.offset, 800,
        "an unchanged legacy policy keeps its view"
    );
    let preferences = AudioTablePreferences {
        pagination_enabled: true,
        page_size: 1000,
        ..Default::default()
    };
    reset_paging_view(&mut task.view, &preferences);
    assert!(task.view.raw_inputs.is_empty());
    apply_task_controls(ui, &task);
    assert_eq!(
        (
            ui.get_sample_filter(),
            ui.get_channel_filter(),
            ui.get_bit_filter(),
            ui.get_visibility_index()
        ),
        (2, 2, 2, 1)
    );
    assert!(ui.get_favorites_only());
    assert!(task_inputs_valid(&task));
    assert_eq!(task.definition, definition);
    task.saved_search_id = Some("paging-favorite".into());
    {
        let mut state = controller.task_workspace.lock().unwrap();
        state.workspace.session.tasks[0] = task.clone();
        state
            .workspace
            .favorites
            .push(audiolib_core::search_workspace::SavedSearchTask {
                id: "paging-favorite".into(),
                title: task.title.clone(),
                title_is_custom: task.title_is_custom,
                definition: definition.clone(),
            });
    }
    controller.publish_search_tasks(ui);
    let row = ui.get_search_tasks().row_data(0).unwrap();
    assert!(row.favorite && !row.modified);
    let bytes = serde_json::to_vec(&task).unwrap();
    let mut reloaded: audiolib_core::search_workspace::SearchTask =
        serde_json::from_slice(&bytes).unwrap();
    assert!(!reconcile_paging_view(&mut reloaded.view, &preferences));
    reloaded.view.offset = 999;
    assert!(reconcile_paging_view(
        &mut reloaded.view,
        &AudioTablePreferences::default()
    ));
    assert_eq!(reloaded.view.offset, 0);
    assert_eq!(reloaded.definition, definition);
    // The core regression projects through the legacy DTO. Here exercise the
    // control restore after a legacy writer has omitted the unknown hint.
    let mut legacy = serde_json::to_value(&task).unwrap();
    legacy["view"]
        .as_object_mut()
        .unwrap()
        .remove("result_paging_policy");
    legacy["view"]["offset"] = serde_json::json!(1000);
    let mut after_legacy_save: audiolib_core::search_workspace::SearchTask =
        serde_json::from_value(legacy).unwrap();
    assert!(after_legacy_save.view.result_paging_policy.is_none());
    assert!(after_legacy_save.view.raw_inputs.is_empty());
    assert!(reconcile_paging_view(
        &mut after_legacy_save.view,
        &preferences
    ));
    assert_eq!(after_legacy_save.view.offset, 0);
    assert_eq!(after_legacy_save.definition, definition);
    assert_eq!(after_legacy_save.saved_search_id, task.saved_search_id);
    assert_eq!(
        after_legacy_save.view.selected_audio,
        task.view.selected_audio
    );
    assert_eq!(
        after_legacy_save.view.checked_audio,
        task.view.checked_audio
    );
    assert_eq!(
        after_legacy_save.view.selection_anchor,
        task.view.selection_anchor
    );
    apply_task_controls(ui, &after_legacy_save);
    assert_eq!(
        (
            ui.get_sample_filter(),
            ui.get_channel_filter(),
            ui.get_bit_filter(),
            ui.get_visibility_index()
        ),
        (2, 2, 2, 1)
    );
    finish(&fixture);
}

#[test]
fn configurable_pages_keep_real_path_cursors_and_hidden_span_navigation() {
    isolated("pagination_preferences::child_real_provider_paging");
}

#[test]
#[ignore = "isolated SQLite Path/All paging with bounded queries and removals"]
fn child_real_provider_paging() {
    let fixture = fixture();
    seed_folder_pages(&fixture, "wind", 10_000, 2_200);
    // Keep the path below the half-library broadness boundary.
    seed_folder_pages(&fixture, "gore", 20_000, 2_300);
    for scope in [SearchScope::Path, SearchScope::All] {
        let reference = SearchRequest {
            query: "wind".into(),
            scope,
            limit: PAGE_SIZE,
            filters: SearchFilters {
                visibility: FileVisibility::All,
                ..Default::default()
            },
            ..Default::default()
        };
        let all = reference_pages(&fixture.provider, reference.clone());
        assert_eq!(all.len(), 2_201);
        for size in [1, 50, 432, 997, 1000] {
            for with_holes in [false, true] {
                let hidden = all
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| {
                        with_holes
                            && (*index < 37
                                || (250..350).contains(index)
                                || (1050..1100).contains(index))
                    })
                    .map(|(_, file)| file.id)
                    .collect::<HashSet<_>>();
                let expected = all
                    .iter()
                    .filter(|file| !hidden.contains(&file.id))
                    .map(|file| file.id)
                    .collect::<Vec<_>>();
                let mut request = reference.clone();
                request.limit = size;
                let search = |request: &SearchRequest| {
                    assert!((1..=PAGE_SIZE).contains(&request.limit));
                    fixture.provider.search(request)
                };
                let hide = |page: &mut audiolib_core::SearchPage| {
                    page.results.retain(|file| !hidden.contains(&file.id));
                    page.returned_count = page.results.len();
                };
                let cancel = AtomicBool::new(false);
                let first =
                    result_window::load_forward(&mut request, &cancel, search, hide).unwrap();
                assert_eq!(
                    first
                        .page
                        .results
                        .iter()
                        .map(|file| file.id)
                        .collect::<Vec<_>>(),
                    expected[..size]
                );
                let mut navigation = result_page_navigation::ResultPageNavigation::default();
                assert!(navigation.install_window(&request, &first.coverage));
                navigation.step(&mut request, 1, size);
                navigation.prepare_query(&mut request, false);
                let admitted_cursor = request.cursor.clone();
                let second =
                    result_window::load_forward(&mut request, &cancel, search, hide).unwrap();
                assert_eq!(
                    second
                        .page
                        .results
                        .iter()
                        .map(|file| file.id)
                        .collect::<Vec<_>>(),
                    expected[size..size * 2]
                );
                if !with_holes {
                    assert_eq!(
                        request.cursor, admitted_cursor,
                        "the aggregated page retains its bounded first cursor"
                    );
                }
                assert!(navigation.install_window(&request, &second.coverage));
                let second_request = request.clone();
                navigation.step(&mut request, 1, size);
                navigation.failed(&mut request);
                assert_eq!(
                    (request.offset, request.limit, &request.cursor),
                    (
                        second_request.offset,
                        second_request.limit,
                        &second_request.cursor
                    )
                );
                navigation.step(&mut request, -1, size);
                navigation.prepare_query(&mut request, false);
                let previous =
                    result_window::load_forward(&mut request, &cancel, search, hide).unwrap();
                assert_eq!(
                    previous
                        .page
                        .results
                        .iter()
                        .map(|file| file.id)
                        .collect::<Vec<_>>(),
                    expected[..size]
                );
                let mut cold_request = second_request;
                let before = cold_request.offset;
                let cold =
                    result_window::load_backward(&mut cold_request, before, &cancel, search, hide)
                        .unwrap();
                assert_eq!(
                    cold.page
                        .results
                        .iter()
                        .map(|file| file.id)
                        .collect::<Vec<_>>(),
                    expected[..size]
                );
            }
        }
    }
    finish(&fixture);
}

#[test]
fn settings_paging_resets_ranges_before_failed_search_and_retains_metadata_and_audio() {
    isolated("pagination_preferences::child_settings_policy_transition");
}

#[test]
#[ignore = "isolated real settings callback, stale ranges and source/editor ownership"]
fn child_settings_policy_transition() {
    let fixture = fixture();
    let controller = &fixture.controller;
    let ui = &fixture.ui;
    let file = controller.records.lock().unwrap().last().unwrap().clone();
    *controller.selected.lock().unwrap() = Some(file.clone());
    present_file(ui, &file);
    let loaded = controller
        .audio
        .send_and_wait(
            AudioCommand::Load(PathBuf::from(&file.file_path)),
            Duration::from_secs(4),
        )
        .unwrap();
    ui.set_edit_notes("draft retained across paging".into());
    ui.set_metadata_dirty(true);
    controller.capture_metadata(ui);
    let revision = controller.metadata.lock().unwrap().drafts[&file.id].revision;
    controller.multi_selection.lock().unwrap().insert(file.id);
    *controller.selection_anchor.lock().unwrap() = Some(file.id);
    controller.capture_search_task(ui);
    {
        let mut state = controller.task_workspace.lock().unwrap();
        let mut inactive = state.workspace.session.tasks[0].clone();
        inactive.id = "inactive-paging-task".into();
        inactive.view.offset = 1200;
        inactive.view.list_top_row = 250;
        inactive.view.list_scroll_y = -5500.0;
        state.workspace.session.tasks.push(inactive);
    }
    {
        let mut request = controller.request.lock().unwrap();
        request.offset = 800;
        request.limit = 600;
        request.cursor = Some("obsolete-range".into());
    }
    ui.invoke_open_global_settings(2);
    assert!(ui.get_background_blocked());
    let cancel = controller.search_cancel.lock().unwrap().clone();
    let generation = controller.search_generation.load(Ordering::Acquire);
    let selection_generation = controller.selection_generation.load(Ordering::Acquire);
    ui.set_duration_min("unfinished".into());
    ui.invoke_paging_policy_requested(true, 50);
    assert_eq!(
        (ui.get_pagination_enabled(), ui.get_result_page_size()),
        (true, 50)
    );
    assert!(cancel.load(Ordering::Acquire));
    assert!(controller.search_generation.load(Ordering::Acquire) > generation);
    let request = controller.request.lock().unwrap().clone();
    assert_eq!(
        (request.offset, request.limit, request.cursor),
        (0, 50, None)
    );
    assert!(controller.window_coverage.lock().unwrap().is_none());
    {
        let state = controller.task_workspace.lock().unwrap();
        assert!(state.caches.is_empty());
        assert!(!state.page_complete);
        assert!(
            state
                .workspace
                .session
                .tasks
                .iter()
                .all(|task| task.view.offset == 0 && task.view.list_top_row == 0)
        );
    }
    assert_eq!(ui.get_selected_id().as_str(), file.id.to_string());
    assert_eq!(
        controller.selected.lock().unwrap().as_ref().unwrap().id,
        file.id
    );
    assert_eq!(
        controller.metadata.lock().unwrap().drafts[&file.id].revision,
        revision
    );
    assert_eq!(ui.get_edit_notes(), "draft retained across paging");
    assert!(ui.get_metadata_dirty());
    assert!(
        controller
            .multi_selection
            .lock()
            .unwrap()
            .contains(&file.id)
    );
    assert_eq!(
        controller.selection_generation.load(Ordering::Acquire),
        selection_generation
    );
    assert_eq!(controller.audio.status().generation, loaded.generation);
    assert_eq!(controller.audio.status().file_path, loaded.file_path);
    ui.set_duration_min("".into());
    ui.invoke_paging_policy_requested(true, 1);
    let ready = controller.clone();
    wait_for(ui, move |ui| {
        !ui.get_searching() && ui.get_rows().row_count() == 1 && fully_published(&ready)
    });
    assert!(ui.get_settings_visible());
    assert!(ui.get_settings_visible_page_focused());
    assert!(!ui.get_settings_hidden_page_focused());
    assert_eq!(
        controller.metadata.lock().unwrap().drafts[&file.id].revision,
        revision
    );
    assert_eq!(controller.audio.status().generation, loaded.generation);
    assert!(!controller.extend_result_window(ui, 1));
    let accepted = controller.audio_table.lock().unwrap().clone();
    for blocked in [0, 1, 2, 3, 4, 5, 6] {
        match blocked {
            0 => controller.library_busy.store(true, Ordering::Release),
            1 => controller.source_changing.store(true, Ordering::Release),
            2 => controller.closing.store(true, Ordering::Release),
            3 => controller.task_workspace.lock().unwrap().loading = true,
            4 => controller.task_workspace.lock().unwrap().restoring = true,
            5 => controller.task_workspace.lock().unwrap().protected = true,
            _ => controller.search_v2_ux.lock().unwrap().declined = true,
        }
        ui.set_pagination_enabled(false);
        ui.set_result_page_size(997);
        ui.invoke_paging_policy_requested(false, 997);
        assert_eq!(*controller.audio_table.lock().unwrap(), accepted);
        assert_eq!(
            table_preferences(ui),
            accepted,
            "close/layout capture cannot save a rejected bound value"
        );
        controller.library_busy.store(false, Ordering::Release);
        controller.source_changing.store(false, Ordering::Release);
        controller.closing.store(false, Ordering::Release);
        controller.search_v2_ux.lock().unwrap().declined = false;
        let mut state = controller.task_workspace.lock().unwrap();
        state.loading = false;
        state.restoring = false;
        state.protected = false;
    }
    ui.invoke_paging_policy_requested(false, 1);
    let ready = controller.clone();
    wait_for(ui, move |ui| {
        !ui.get_searching() && ui.get_rows().row_count() == 4 && fully_published(&ready)
    });
    assert_eq!(controller.request.lock().unwrap().limit, PAGE_SIZE);
    let generation = controller.search_generation.load(Ordering::Acquire);
    ui.invoke_paging_policy_requested(false, 50);
    assert_eq!(
        controller.search_generation.load(Ordering::Acquire),
        generation,
        "an inactive stored page size does not change continuous browsing"
    );
    assert_eq!(controller.result_page_size(), PAGE_SIZE);
    assert_eq!(controller.result_window_limit(), PAGE_SIZE * 3);
    finish(&fixture);
}
