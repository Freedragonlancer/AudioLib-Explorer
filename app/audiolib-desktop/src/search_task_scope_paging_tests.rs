// app/audiolib-desktop/src/search_task_scope_paging_tests.rs
//! Actual controller restoration and provider-backed continuation regressions.
use super::*;
use audiolib_core::{SearchScope, search_workspace::SearchTaskDefinition};

fn select_task(fixture: &Fixture, id: &str, count: usize) {
    fixture.ui.invoke_search_task_selected(id.into());
    let id = id.to_owned();
    let controller = fixture.controller.clone();
    wait_for(&fixture.ui, move |ui| {
        active_id(ui) == id
            && !ui.get_task_restoring()
            && !ui.get_searching()
            && ui.get_rows().row_count() == count
            && controller.task_workspace.lock().unwrap().page_complete
    });
}

#[test]
fn scopes_survive_task_switches_favorites_and_workspace_storage() {
    isolated("scope_paging::child_scopes_survive_task_switches");
}

#[test]
#[ignore = "isolated real controller task-scope restoration"]
fn child_scopes_survive_task_switches() {
    let fixture = fixture();
    let ui = &fixture.ui;
    let controller = &fixture.controller;
    ui.set_search_scope_index(2);
    ui.invoke_search_scope_changed();
    search(&fixture, "wind", 1);
    let path_task = active_id(ui);
    ui.invoke_search_task_favorite_toggled(path_task.clone().into());
    ui.invoke_search_task_created();
    wait_for(ui, |ui| {
        ui.get_search_tasks().row_count() == 2 && !ui.get_task_restoring() && !ui.get_searching()
    });
    ui.set_search_scope_index(3);
    ui.invoke_search_scope_changed();
    search(&fixture, "gore", 3);
    let filename_task = active_id(ui);
    controller.capture_search_task(ui);
    controller.task_workspace.lock().unwrap().caches.clear();
    select_task(&fixture, &path_task, 1);
    assert_eq!(ui.get_search_scope_index(), 2);
    assert_eq!(controller.request.lock().unwrap().scope, SearchScope::Path);
    assert_eq!(ui.get_query(), "wind");
    select_task(&fixture, &filename_task, 3);
    assert_eq!(ui.get_search_scope_index(), 3);
    assert_eq!(
        controller.request.lock().unwrap().scope,
        SearchScope::Filename
    );
    select_task(&fixture, &path_task, 1);
    assert!(
        !ui.get_search_tasks()
            .iter()
            .find(|task| task.active)
            .unwrap()
            .modified
    );

    controller.persist_search_task(ui);
    controller.wait_workspace_writes().unwrap();
    let target = controller
        .task_workspace
        .lock()
        .unwrap()
        .target
        .clone()
        .unwrap();
    let restored = fixture.provider.load_search_workspace(&target).unwrap();
    assert_eq!(
        restored
            .session
            .tasks
            .iter()
            .find(|task| task.id == path_task)
            .unwrap()
            .definition
            .scope,
        SearchScope::Path
    );
    assert_eq!(
        restored
            .session
            .tasks
            .iter()
            .find(|task| task.id == filename_task)
            .unwrap()
            .definition
            .scope,
        SearchScope::Filename
    );
    assert_eq!(restored.favorites[0].definition.scope, SearchScope::Path);

    // A workspace from before scope persistence always means legacy main fields,
    // regardless of the most recently selected application-level preference.
    select_task(&fixture, &filename_task, 3);
    {
        let mut state = controller.task_workspace.lock().unwrap();
        let task = state
            .workspace
            .session
            .tasks
            .iter_mut()
            .find(|task| task.id == path_task)
            .unwrap();
        task.definition =
            serde_json::from_str::<SearchTaskDefinition>(r#"{"query":"wind"}"#).unwrap();
        state.caches.clear();
    }
    select_task(&fixture, &path_task, 1);
    assert_eq!(ui.get_search_scope_index(), 1);
    assert_eq!(controller.request.lock().unwrap().scope, SearchScope::Main);
    finish(&fixture);
}

fn seed_path_pages(fixture: &Fixture) {
    let conn = rusqlite::Connection::open(fixture.provider.database_path()).unwrap();
    audiolib_db::register_library_functions(&conn).unwrap();
    let wind = fixture
        .provider
        .find_folder_by_path(&fixture.root.join("sounds/wind"))
        .unwrap()
        .unwrap();
    let gore = fixture
        .provider
        .find_folder_by_path(&fixture.root.join("sounds/gore"))
        .unwrap()
        .unwrap();
    let tx = conn.unchecked_transaction().unwrap();
    for (folder, first, count) in [(&wind, 1_000, 250), (&gore, 2_000, 254)] {
        for id in first..first + count {
            let filename = format!("unrecorded_{id}.wav");
            let path = format!("{}/{}", folder.path, filename);
            tx.execute(
                "INSERT INTO audio_files(id,file_path,filename,folder_id,missing) VALUES(?1,?2,?3,?4,1)",
                rusqlite::params![id, path, filename, folder.id],
            ).unwrap();
        }
    }
    tx.commit().unwrap();
    fixture.provider.rebuild_directory_search().unwrap();
}

#[test]
fn lower_bound_path_pages_use_continuations_and_observed_counts() {
    isolated("scope_paging::child_lower_bound_path_pages");
}

#[test]
#[ignore = "isolated actual SQLite path pages and native UI navigation"]
fn child_lower_bound_path_pages() {
    let fixture = fixture();
    seed_path_pages(&fixture);
    let ui = &fixture.ui;
    let controller = &fixture.controller;
    ui.set_visibility_index(2);
    ui.set_search_scope_index(2);
    search(&fixture, "wind", PAGE_SIZE);
    assert!(ui.get_result_summary().contains("≥201"));
    assert_eq!(ui.get_page_label(), "1/?");
    assert!(ui.get_can_next());
    let first = controller
        .records
        .lock()
        .unwrap()
        .iter()
        .map(|file| file.id)
        .collect::<HashSet<_>>();
    controller.request_page(ui, 1);
    let ready = controller.clone();
    wait_for(ui, move |ui| {
        !ui.get_searching() && ui.get_rows().row_count() == 51 && fully_published(&ready)
    });
    assert_eq!(controller.request.lock().unwrap().offset, PAGE_SIZE);
    assert!(controller.request.lock().unwrap().cursor.is_some());
    // A cursor page reports its observed suffix, not an exact global total.
    assert_eq!(ui.get_page_label(), "2/?");
    assert!(ui.get_result_summary().contains("≥51 sounds"));
    assert!(!ui.get_can_next());
    let second = controller
        .records
        .lock()
        .unwrap()
        .iter()
        .map(|file| file.id)
        .collect::<HashSet<_>>();
    assert!(first.is_disjoint(&second));
    assert_eq!(first.len() + second.len(), 251);
    controller.request_page(ui, -1);
    let ready = controller.clone();
    wait_for(ui, move |ui| {
        !ui.get_searching() && ui.get_rows().row_count() == PAGE_SIZE && fully_published(&ready)
    });
    assert_eq!(controller.request.lock().unwrap().offset, 0);
    assert!(controller.request.lock().unwrap().cursor.is_none());
    assert_eq!(
        controller
            .records
            .lock()
            .unwrap()
            .iter()
            .map(|file| file.id)
            .collect::<HashSet<_>>(),
        first
    );

    // Without a cursor, traversing beyond the end observes the exact global
    // total. Restoration clamps to and reloads the actual final page.
    controller.request.lock().unwrap().offset = 1_000;
    controller.request_navigation_search(ui, true, false);
    let ready = controller.clone();
    wait_for(ui, move |ui| {
        !ui.get_searching() && ui.get_rows().row_count() == 51 && fully_published(&ready)
    });
    assert_eq!(controller.request.lock().unwrap().offset, PAGE_SIZE);
    assert!(controller.request.lock().unwrap().cursor.is_none());
    assert_eq!(ui.get_page_label(), "2/2");
    assert!(!ui.get_result_summary().contains('≥'));
    assert!(ui.get_result_summary().contains("251 sounds"));
    assert!(ui.get_can_previous());
    assert!(!ui.get_can_next());
    finish(&fixture);
}
