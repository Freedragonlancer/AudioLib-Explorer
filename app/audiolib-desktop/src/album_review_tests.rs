// app/audiolib-desktop/src/album_review_tests.rs
//! Exercise album filters, current-query facets and stale publications through real workers.
use super::*;

#[path = "artwork_management_tests.rs"]
mod management;

#[test]
fn albums_filter_results_follow_current_query_and_restore_with_tasks() {
    isolated("album_review::child_album_navigation");
}

fn wait_albums(fixture: &Fixture, count: usize) {
    wait_for(&fixture.ui, move |ui| {
        ui.get_albums().row_count() == count && !ui.get_searching()
    });
}

#[test]
#[ignore = "isolated album navigation regression"]
fn child_album_navigation() {
    let fixture = fixture();
    let ui = &fixture.ui;
    let controller = &fixture.controller;
    // These statements modify only the isolated fixture. Album values are derived
    // from the same owned Metadata representation used by scanning and v7 backfill.
    let connection = rusqlite::Connection::open(fixture.provider.database_path()).unwrap();
    audiolib_db::register_library_functions(&connection).unwrap();
    connection.execute(
        "UPDATE audio_files SET metadata_json=json_set(audiolib_metadata_json(metadata_json),'$.wav_metadata.fields.library',CASE WHEN filename LIKE 'gore%' THEN 'Gore album' ELSE 'Wind album' END)",
        [],
    ).unwrap();
    drop(connection);
    search(&fixture, "", 4);
    wait_albums(&fixture, 2);
    let gore = ui
        .get_albums()
        .iter()
        .find(|row| row.name == "Gore album")
        .unwrap()
        .id;
    let wind = ui
        .get_albums()
        .iter()
        .find(|row| row.name == "Wind album")
        .unwrap()
        .id;
    ui.invoke_album_selected(gore.clone());
    wait_for(ui, |ui| {
        !ui.get_searching() && ui.get_rows().row_count() == 3
    });
    assert_eq!(
        controller.request.lock().unwrap().filters.album_id,
        Some(gore.parse::<i64>().unwrap())
    );
    // The current query is wind while the selected album is gore: results are empty,
    // but cross-filter facets expose the matching wind album immediately.
    search(&fixture, "wind", 0);
    wait_albums(&fixture, 1);
    assert_eq!(
        ui.get_albums().row_data(0).unwrap().name.as_str(),
        "Wind album"
    );
    ui.invoke_reset_filters();
    wait_for(ui, |ui| {
        !ui.get_searching() && ui.get_rows().row_count() == 1
    });
    assert!(
        controller
            .request
            .lock()
            .unwrap()
            .filters
            .album_id
            .is_none()
    );
    assert!(ui.get_album_filter().is_empty());
    ui.invoke_album_selected(wind.clone());
    wait_for(ui, |ui| !ui.get_searching());
    assert_eq!(
        controller.request.lock().unwrap().filters.album_id,
        Some(wind.parse::<i64>().unwrap())
    );
    ui.invoke_album_selected(wind);
    wait_for(ui, |ui| !ui.get_searching());
    assert!(
        controller
            .request
            .lock()
            .unwrap()
            .filters
            .album_id
            .is_none()
    );

    search(&fixture, "", 4);
    ui.invoke_album_selected(gore.clone());
    wait_for(ui, |ui| {
        !ui.get_searching() && ui.get_rows().row_count() == 3
    });
    let first = active_id(ui);
    ui.invoke_search_task_created();
    wait_for(ui, |ui| {
        ui.get_search_tasks().row_count() == 2 && !ui.get_task_restoring() && !ui.get_searching()
    });
    assert!(ui.get_album_filter().is_empty());
    ui.invoke_album_selected(
        ui.get_albums()
            .iter()
            .find(|row| row.name == "Wind album")
            .unwrap()
            .id,
    );
    wait_for(ui, |ui| {
        !ui.get_searching() && ui.get_rows().row_count() == 1
    });
    ui.invoke_search_task_selected(first.clone().into());
    wait_for(ui, move |ui| {
        active_id(ui) == first
            && !ui.get_task_restoring()
            && !ui.get_searching()
            && ui.get_rows().row_count() == 3
    });
    wait_albums(&fixture, 2);
    assert_eq!(ui.get_album_filter().as_str(), "Gore album");

    let file = controller.records.lock().unwrap()[0].clone();
    present_file(ui, &file);
    ui.set_album_cover_ready(true);
    present_file(ui, &file);
    assert!(
        ui.get_album_cover_ready(),
        "saving annotations must retain the current cover"
    );

    // Queue an old local publication, then switch library context before delivery.
    let stale = SearchJob {
        request: controller.request.lock().unwrap().clone(),
        generation: controller.search_generation.load(Ordering::Acquire),
        not_before: Instant::now(),
        cancel: controller.search_cancel.lock().unwrap().clone(),
        facet_revision: controller.facet_revision.load(Ordering::Acquire),
        provider: fixture.provider.clone(),
    };
    controller.publish_albums(&stale);
    controller.invalidate_library_reads();
    controller.reset_library_view(ui, true, false);
    controller.remote_mode.store(true, Ordering::Release);
    ui.set_remote_mode(true);
    ui.invoke_album_selected(gore);
    wait_for(ui, |_| true);
    assert_eq!(ui.get_albums().row_count(), 0);
    assert!(ui.get_album_filter().is_empty());
    assert!(
        controller
            .request
            .lock()
            .unwrap()
            .filters
            .album_id
            .is_none()
    );
    controller.remote_mode.store(false, Ordering::Release);
    ui.set_remote_mode(false);
    finish(&fixture);
}
