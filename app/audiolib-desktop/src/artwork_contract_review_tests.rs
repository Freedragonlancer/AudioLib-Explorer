// app/audiolib-desktop/src/artwork_contract_review_tests.rs
//! Persisted custom covers follow real missing-record selection and relink flows.
use super::*;

#[test]
fn missing_selection_reloads_its_custom_cover_after_selecting_another_record() {
    isolated("album_review::management::contracts::child_missing_custom_cover_selection");
}

#[test]
#[ignore = "isolated missing-record artwork regression"]
fn child_missing_custom_cover_selection() {
    let fixture = fixture();
    let file = selected_artwork_file(&fixture);
    let (custom, _) = override_paths(&fixture.controller, &file);
    manual_cover(&custom);
    let original_bytes = std::fs::read(&file.file_path).unwrap();
    let held = fixture.root.join("held.wav");
    std::fs::rename(&file.file_path, &held).unwrap();
    ScanService::new(fixture.provider.clone())
        .scan(
            &fixture.root.join("sounds"),
            &ScanOptions {
                incremental: false,
                ..Default::default()
            },
            &ScanCancellation::default(),
            |_| {},
        )
        .unwrap();
    assert!(fixture.provider.get_audio(file.id).unwrap().missing);
    let other = fixture
        .provider
        .search(&SearchRequest::default())
        .unwrap()
        .results
        .into_iter()
        .find(|other| other.id != file.id)
        .unwrap();

    fixture.controller.select_id(file.id, false);
    let expected = file.id.to_string();
    wait_for(&fixture.ui, move |ui| {
        ui.get_selected_id().as_str() == expected
            && ui.get_selected_missing()
            && ui.get_album_cover_ready()
    });
    assert_eq!(fixture.ui.get_album_cover().size().width, 3);
    fixture.controller.select_id(other.id, false);
    let expected = other.id.to_string();
    wait_for(&fixture.ui, move |ui| {
        ui.get_selected_id().as_str() == expected && !ui.get_album_cover_ready()
    });
    fixture.controller.select_id(file.id, false);
    let expected = file.id.to_string();
    wait_for(&fixture.ui, move |ui| {
        ui.get_selected_id().as_str() == expected
            && ui.get_selected_missing()
            && ui.get_album_cover_ready()
    });
    assert_eq!(fixture.ui.get_album_cover().size().width, 3);
    assert_eq!(std::fs::read(&held).unwrap(), original_bytes);
    assert!(custom.is_file());
    finish(&fixture);
}

#[test]
fn relink_refresh_reloads_the_retained_records_custom_cover() {
    isolated("album_review::management::contracts::child_relink_custom_cover");
}

#[test]
#[ignore = "isolated retained-selection artwork regression"]
fn child_relink_custom_cover() {
    let fixture = fixture();
    let file = fixture
        .provider
        .search(&SearchRequest::default())
        .unwrap()
        .results
        .remove(0);
    assert!(audiolib_core::album(&file.attributes).is_empty());
    fixture.controller.config.lock().unwrap().allowed_scan_roots =
        vec![fixture.root.join("sounds")];
    fixture.controller.select_id(file.id, false);
    let expected = file.id.to_string();
    wait_for(&fixture.ui, move |ui| {
        ui.get_selected_id().as_str() == expected
    });
    let (custom, _) = override_paths(&fixture.controller, &file);
    manual_cover(&custom);
    fixture.controller.queue_artwork(&file);
    wait_for(&fixture.ui, |ui| ui.get_album_cover_ready());
    assert_eq!(fixture.ui.get_album_cover().size().width, 3);

    let original_bytes = std::fs::read(&file.file_path).unwrap();
    let target = fixture.root.join("sounds").join("relinked.wav");
    std::fs::write(&target, &original_bytes).unwrap();
    let expected_path = audiolib_db::display_path(&target.canonicalize().unwrap());
    fixture
        .controller
        .run_library_operation(LibraryOperation::Relink(file.id, target.clone()));
    wait_for(&fixture.ui, move |ui| {
        !ui.get_library_busy()
            && ui.get_selected_path().as_str() == expected_path
            && ui.get_album_cover_ready()
    });
    assert_eq!(fixture.ui.get_album_cover().size().width, 3);
    let linked = fixture.provider.get_audio(file.id).unwrap();
    assert_eq!(linked.id, file.id);
    assert_eq!(override_paths(&fixture.controller, &linked).0, custom);
    assert_eq!(std::fs::read(&file.file_path).unwrap(), original_bytes);
    assert_eq!(std::fs::read(&target).unwrap(), original_bytes);
    assert!(custom.is_file());
    finish(&fixture);
}

#[test]
fn album_representative_survives_search_refresh_and_respects_manual_choices() {
    isolated("album_review::management::contracts::child_album_representative_refresh");
}

#[test]
#[ignore = "isolated generated album representative regression"]
fn child_album_representative_refresh() {
    let fixture = fixture();
    let controller = &fixture.controller;
    let ui = &fixture.ui;
    controller.config.lock().unwrap().allowed_scan_roots = vec![fixture.root.join("sounds")];
    let connection = rusqlite::Connection::open(fixture.provider.database_path()).unwrap();
    audiolib_db::register_library_functions(&connection).unwrap();
    connection
        .execute_batch(
            "UPDATE audio_files SET album='Shared artwork',
             metadata_json=json_set(metadata_json,'$.wav_metadata.fields.library','Shared artwork');",
        )
        .unwrap();
    drop(connection);
    let files = fixture
        .provider
        .search(&SearchRequest::default())
        .unwrap()
        .results;
    let selected = files
        .iter()
        .find(|file| file.filename == "wind.wav")
        .unwrap();
    let first = &files[0];
    assert_ne!(first.id, selected.id);
    embedded_cover(Path::new(&selected.file_path));
    search(&fixture, "", 4);
    wait_for(ui, |ui| ui.get_albums().row_count() == 1);
    controller.select_id(selected.id, false);
    wait_for(ui, |ui| {
        ui.get_album_cover_ready()
            && ui
                .get_albums()
                .row_data(0)
                .is_some_and(|row| row.cover.size().width > 0)
    });

    // The representative is outside the refreshed query. Its source can also
    // disappear: sidebar work must reuse generated pixels without opening a WAV.
    let held = fixture.root.join("held-representative.wav");
    std::fs::rename(&selected.file_path, &held).unwrap();
    // Cancelling a picker or another album's work changes the artwork epoch,
    // but must retain this still-valid representative and its existing PNG.
    controller.cancel_artwork();
    controller.select_id(first.id, false);
    let expected = first.id.to_string();
    wait_for(ui, move |ui| {
        ui.get_selected_id().as_str() == expected && !ui.get_album_cover_ready()
    });
    ui.set_albums(ModelRc::new(VecModel::<AlbumRow>::default()));
    search(&fixture, "gore", 3);
    wait_for(ui, |ui| {
        ui.get_albums()
            .row_data(0)
            .is_some_and(|row| row.cover.size().width > 0)
    });
    assert_eq!(
        ui.get_albums().row_data(0).unwrap().name.as_str(),
        "Shared artwork"
    );

    let generated = controller.cached_artwork_path(selected).unwrap();
    {
        let _cache = controller.artwork_cache_lock.lock().unwrap();
        std::fs::remove_file(&generated).unwrap();
    }
    ui.set_albums(ModelRc::new(VecModel::<AlbumRow>::default()));
    search(&fixture, "gore", 3);
    wait_for(ui, |ui| {
        ui.get_albums()
            .row_data(0)
            .is_some_and(|row| row.cover.size().width == 0)
    });

    let (custom, _) = override_paths(controller, first);
    manual_cover(&custom);
    ui.set_albums(ModelRc::new(VecModel::<AlbumRow>::default()));
    search(&fixture, "gore", 3);
    wait_for(ui, |ui| {
        ui.get_albums()
            .row_data(0)
            .is_some_and(|row| row.cover.size().width == 3)
    });
    ui.invoke_artwork_action_requested(3);
    wait_for(ui, |ui| {
        !ui.get_artwork_busy()
            && ui
                .get_albums()
                .row_data(0)
                .is_some_and(|row| row.cover.size().width == 0)
    });
    assert!(!custom.exists());
    assert!(held.is_file());
    finish(&fixture);
}
