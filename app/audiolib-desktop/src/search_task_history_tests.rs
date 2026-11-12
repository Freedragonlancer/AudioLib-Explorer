// app/audiolib-desktop/src/search_task_history_tests.rs
//! Provider-backed regressions for task-local search history navigation.
use super::*;
use i_slint_backend_testing::ElementHandle;
use slint::platform::{PointerEventButton, WindowEvent};

fn settled(fixture: &Fixture, query: &str, count: usize) {
    let query = query.to_owned();
    let controller = fixture.controller.clone();
    wait_for(&fixture.ui, move |ui| {
        ui.get_query().as_str() == query
            && !ui.get_search_workspace_loading()
            && !ui.get_task_restoring()
            && !ui.get_searching()
            && ui.get_rows().row_count() == count
            && fully_published(&controller)
    });
}

fn step(fixture: &Fixture, delta: i32, query: &str, count: usize) {
    fixture.ui.invoke_search_history_requested(delta);
    settled(fixture, query, count);
}

#[test]
fn history_restores_conditions_and_directory_independently_for_each_task() {
    isolated("history_navigation::child_history_conditions_and_task_isolation");
}

#[test]
#[ignore = "isolated provider-backed search-history controller regression"]
fn child_history_conditions_and_task_isolation() {
    let fixture = fixture();
    let ui = &fixture.ui;
    let controller = &fixture.controller;
    let first = active_id(ui);
    let initial_scope = ui.get_search_scope_index();
    let audio_generation = fixture.engine.handle().status().generation;
    assert!(!ui.get_search_history_can_back());
    assert!(!ui.get_search_history_can_forward());

    ui.set_search_scope_index(3);
    ui.set_tag_filter("gore".into());
    ui.set_sample_filter(2);
    ui.set_sort_index(2);
    ui.set_sort_descending(true);
    search(&fixture, "gore", 3);
    choose_gore(&fixture);
    let folder = controller.request.lock().unwrap().folder_id;
    assert!(folder.is_some());
    search(&fixture, "two", 1);

    step(&fixture, -1, "gore", 3);
    assert_eq!(controller.request.lock().unwrap().folder_id, folder);
    assert_eq!(ui.get_search_scope_index(), 3);
    assert_eq!(ui.get_tag_filter(), "gore");
    assert_eq!(ui.get_sample_filter(), 2);
    assert_eq!(ui.get_sort_index(), 2);
    assert!(ui.get_sort_descending());
    step(&fixture, -1, "gore", 3);
    assert_eq!(controller.request.lock().unwrap().folder_id, None);
    step(&fixture, -1, "", 4);
    assert_eq!(ui.get_search_scope_index(), initial_scope);
    assert_eq!(ui.get_tag_filter(), "");
    assert_eq!(ui.get_sample_filter(), 0);
    assert!(!ui.get_search_history_can_back());
    assert!(ui.get_search_history_can_forward());

    step(&fixture, 1, "gore", 3);
    step(&fixture, 1, "gore", 3);
    assert_eq!(controller.request.lock().unwrap().folder_id, folder);
    ui.invoke_search_task_created();
    settled(&fixture, "", 4);
    let second = active_id(ui);
    assert_ne!(first, second);
    assert!(!ui.get_search_history_can_back());
    assert!(!ui.get_search_history_can_forward());
    search(&fixture, "wind", 1);
    step(&fixture, -1, "", 4);
    assert!(!ui.get_search_history_can_back());
    assert!(ui.get_search_history_can_forward());

    ui.invoke_search_task_selected(first.clone().into());
    settled(&fixture, "gore", 3);
    assert_eq!(active_id(ui), first);
    assert_eq!(controller.request.lock().unwrap().folder_id, folder);
    assert!(ui.get_search_history_can_forward());
    step(&fixture, 1, "two", 1);
    assert!(!ui.get_search_history_can_forward());
    assert_eq!(
        fixture.engine.handle().status().generation,
        audio_generation
    );
    finish(&fixture);
}

#[test]
fn live_edits_merge_and_invalid_input_or_refresh_does_not_add_history() {
    isolated("history_navigation::child_history_live_edits_and_branching");
}

#[test]
#[ignore = "isolated live search-history and forward-branch controller regression"]
fn child_history_live_edits_and_branching() {
    let fixture = fixture();
    let ui = &fixture.ui;
    let controller = &fixture.controller;
    // Before a first result is published, Back restores the initial successful
    // state rather than skipping it or waiting for the obsolete request.
    ui.set_query("wind".into());
    ui.invoke_search_requested(true);
    assert!(ui.get_search_history_can_back());
    step(&fixture, -1, "", 4);
    assert!(!ui.get_search_history_can_back());
    search(&fixture, "wind", 1);
    // Queued edits in one typing burst publish only the final conditions.
    for query in ["go", "gor", "gore"] {
        ui.set_query(query.into());
        ui.invoke_search_requested(false);
    }
    settled(&fixture, "gore", 3);
    ui.invoke_refresh_requested();
    settled(&fixture, "gore", 3);
    controller.request_navigation_search(ui, true, false);
    settled(&fixture, "gore", 3);

    // A rejected one-letter draft never becomes a successful search entry.
    ui.set_query("g".into());
    ui.invoke_search_requested(true);
    assert!(!ui.get_searching());
    assert_eq!(controller.request.lock().unwrap().query, "gore");
    ui.set_query("gore".into());
    ui.invoke_search_requested(true);
    settled(&fixture, "gore", 3);
    step(&fixture, -1, "wind", 1);
    step(&fixture, 1, "gore", 3);

    step(&fixture, -1, "wind", 1);
    search(&fixture, "unmatched", 0);
    assert!(!ui.get_search_history_can_forward());
    step(&fixture, -1, "wind", 1);
    step(&fixture, 1, "unmatched", 0);
    assert!(!ui.get_search_history_can_forward());
    finish(&fixture);
}

#[test]
fn history_restores_the_sidebar_with_its_executed_favorites_filter() {
    isolated("history_navigation::child_history_favorites_sidebar");
}

#[test]
#[ignore = "isolated favorites-sidebar search-history controller regression"]
fn child_history_favorites_sidebar() {
    let fixture = fixture();
    let ui = &fixture.ui;
    let controller = &fixture.controller;
    search(&fixture, "gore", 3);

    for (index, theme) in [
        audiolib_core::config::DEFAULT_THEME,
        audiolib_core::config::STUDIO_THEME,
    ]
    .into_iter()
    .enumerate()
    {
        crate::theme::apply_theme(ui, theme);
        wait_for(ui, |_| true);
        assert_eq!(ui.get_library_sidebar_page(), 0);
        assert!(!ui.get_favorites_only());
        controller.persist_search_task(ui);
        if index == 0 {
            // The first navigation also exercises a history that must be
            // seeded from the valid task before the favorites controls change.
            controller.task_workspace.lock().unwrap().histories.clear();
        }
        let favorites_label = if theme == audiolib_core::config::STUDIO_THEME {
            "My favorites"
        } else {
            "Favorites page"
        };
        let favorites = ElementHandle::find_by_accessible_label(ui, favorites_label)
            .next()
            .expect("favorites navigation is visible");
        let origin = favorites.absolute_position();
        let size = favorites.size();
        let position =
            slint::LogicalPosition::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0);
        // This provider-backed fixture uses system time. Dispatch a real
        // pointer click without advancing the testing backend's mock clock.
        ui.window()
            .dispatch_event(WindowEvent::PointerMoved { position });
        ui.window().dispatch_event(WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Left,
        });
        ui.window().dispatch_event(WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Left,
        });
        // The fixture has no favorite records. Empty successful searches still
        // belong to history and must restore their own sidebar page.
        settled(&fixture, "gore", 0);
        assert!(ui.get_favorites_only());
        assert_eq!(ui.get_library_sidebar_page(), 1);
        assert_eq!(ui.get_favorites_category(), 0);
        assert!(ui.get_search_history_can_back());

        step(&fixture, -1, "gore", 3);
        assert!(!ui.get_favorites_only());
        assert!(!controller.request.lock().unwrap().filters.favorites_only);
        assert_eq!(ui.get_library_sidebar_page(), 0);
        assert!(ui.get_search_history_can_forward());

        step(&fixture, 1, "gore", 0);
        assert!(ui.get_favorites_only());
        assert_eq!(ui.get_library_sidebar_page(), 1);
        assert_eq!(ui.get_favorites_category(), 0);
        step(&fixture, -1, "gore", 3);
        assert_eq!(ui.get_library_sidebar_page(), 0);
    }

    // Restore a cold task saved between changing the favorites controls and
    // admitting their search. The raw draft survives for ordinary restoration,
    // while history seeds from the previous executable all-sounds condition.
    controller.persist_search_task(ui);
    let id = active_id(ui);
    {
        let mut state = controller.task_workspace.lock().unwrap();
        state.histories.remove(&id);
        state.caches.remove(&id);
        let task = state
            .workspace
            .session
            .tasks
            .iter_mut()
            .find(|task| task.id == id)
            .unwrap();
        task.view.sidebar_page = 1;
        task.view.favorites_category = 0;
        task.view
            .raw_inputs
            .insert("favorites_only".into(), "true".into());
    }
    controller.restore_search_task(&id);
    {
        let state = controller.task_workspace.lock().unwrap();
        let task = state
            .workspace
            .session
            .tasks
            .iter()
            .find(|task| task.id == id)
            .unwrap();
        assert_eq!(task.view.sidebar_page, 1);
        assert_eq!(task.view.raw_inputs["favorites_only"], "true");
        let entry = state.histories[&id].current().unwrap();
        assert_eq!(entry.view.sidebar_page, 0);
        assert!(entry.view.raw_inputs.is_empty());
    }
    settled(&fixture, "gore", 0);
    assert!(ui.get_favorites_only());
    step(&fixture, -1, "gore", 3);
    assert!(!ui.get_favorites_only());
    assert_eq!(ui.get_library_sidebar_page(), 0);
    finish(&fixture);
}
