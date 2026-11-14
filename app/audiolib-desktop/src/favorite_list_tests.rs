// app/audiolib-desktop/src/favorite_list_tests.rs
//! Row favorites persist immediately, and the list filter stays in the open folder.
use super::*;
use std::cell::Cell;
use std::rc::Rc;

fn tiny_wav() -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend(b"RIFF");
    bytes.extend(38u32.to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16u32.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(48_000u32.to_le_bytes());
    bytes.extend(96_000u32.to_le_bytes());
    bytes.extend(2u16.to_le_bytes());
    bytes.extend(16u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend(2u32.to_le_bytes());
    bytes.extend(0i16.to_le_bytes());
    bytes
}

#[test]
fn folder_favorite_filter_keeps_the_open_folder() {
    i_slint_backend_testing::init_no_event_loop();
    let ui = AppWindow::new().unwrap();
    ui.set_selected_folder_path("D:\\Sounds\\Aircraft".into());
    let filters = Rc::new(Cell::new(0));
    let libraries = Rc::new(Cell::new(0));
    let observed_filters = filters.clone();
    ui.on_filter_requested(move || observed_filters.set(observed_filters.get() + 1));
    let observed_libraries = libraries.clone();
    ui.on_all_library_requested(move || observed_libraries.set(observed_libraries.get() + 1));
    ui.invoke_toggle_folder_favorites();
    assert!(ui.get_favorites_only());
    assert_eq!(ui.get_selected_folder_path(), "D:\\Sounds\\Aircraft");
    assert_eq!(filters.get(), 1);
    assert_eq!(libraries.get(), 0);
    ui.invoke_toggle_folder_favorites();
    assert!(!ui.get_favorites_only());
    assert_eq!(ui.get_selected_folder_path(), "D:\\Sounds\\Aircraft");
    assert_eq!(filters.get(), 2);
    assert_eq!(libraries.get(), 0);
}

#[test]
fn row_star_requests_the_toggled_favorite_in_both_themes() {
    i_slint_backend_testing::init_no_event_loop();
    for modern in [false, true] {
        let ui = AppWindow::new().unwrap();
        ui.window().set_size(slint::LogicalSize::new(1440.0, 900.0));
        ui.global::<crate::Theme>().set_modern_style(modern);
        ui.set_rows(ModelRc::new(VecModel::from(vec![
            AudioRow {
                id: "7".into(),
                name: "door.wav".into(),
                favorite: false,
                ..Default::default()
            },
            AudioRow {
                id: "8".into(),
                name: "wind.wav".into(),
                favorite: true,
                ..Default::default()
            },
        ])));
        let events = Rc::new(std::cell::RefCell::new(Vec::new()));
        let observed = events.clone();
        ui.on_favorite_set_requested(move |id, favorite| {
            observed.borrow_mut().push((id.to_string(), favorite));
        });
        ui.show().unwrap();
        click_favorite(&ui, "Favorite: door.wav");
        click_favorite(&ui, "Unfavorite: wind.wav");
        let events = events.borrow().clone();
        assert!(
            events.contains(&("7".to_string(), true)),
            "modern={modern} events={events:?}"
        );
        assert!(
            events.contains(&("8".to_string(), false)),
            "modern={modern} events={events:?}"
        );
        ui.hide().unwrap();
    }
}

fn click_favorite(ui: &AppWindow, label: &str) {
    use i_slint_backend_testing::{AccessibleRole, ElementHandle};
    use slint::platform::PointerEventButton;
    let buttons = ElementHandle::find_by_accessible_label(ui, label)
        .filter(|element| element.accessible_role() == Some(AccessibleRole::Button))
        .filter(|element| element.size().width > 0.0 && element.size().height > 0.0)
        .collect::<Vec<_>>();
    assert!(!buttons.is_empty(), "missing favorite control {label}");
    for button in buttons {
        button.mock_single_click(PointerEventButton::Left);
    }
}

#[test]
fn favorite_toggle_persists_without_saving_tags_and_the_filter_is_folder_local() {
    i_slint_backend_testing::init_no_event_loop();
    let root = tempfile::tempdir().unwrap();
    let library = root.path().join("library");
    for (folder, file) in [
        ("doors", "door.wav"),
        ("doors", "creak.wav"),
        ("wind", "gust.wav"),
    ] {
        let directory = library.join(folder);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join(file), tiny_wav()).unwrap();
    }
    let config = AppConfig::load_in(root.path().join("preferences")).unwrap();
    let provider = Arc::new(SqliteProvider::open(&config.paths.database).unwrap());
    ScanService::new(provider.clone())
        .scan(
            &library,
            &ScanOptions::default(),
            &ScanCancellation::default(),
            |_| {},
        )
        .unwrap();
    let playback = PlaybackConfig::default();
    let engine = audiolib_audio::AudioEngine::new(playback.clone()).unwrap();
    let ui = AppWindow::new().unwrap();
    let controller = Controller::new(&ui, provider.clone(), engine.handle(), config, playback);
    let files = provider
        .search(&SearchRequest {
            scope: SearchScope::All,
            ..Default::default()
        })
        .unwrap()
        .results;
    let by_name = |name: &str| {
        files
            .iter()
            .find(|file| file.filename == name)
            .unwrap_or_else(|| panic!("missing {name}"))
            .clone()
    };
    let door = by_name("door.wav");
    let creak = by_name("creak.wav");
    let gust = by_name("gust.wav");
    let doors = door.folder_id.expect("door folder");
    let wind = gust.folder_id.expect("wind folder");
    assert_eq!(creak.folder_id, Some(doors));
    assert_ne!(doors, wind);
    ui.set_selected_id(door.id.to_string().into());
    ui.set_edit_tags("draft-tag".into());
    ui.set_edit_notes("draft-note".into());
    ui.set_metadata_dirty(true);
    controller.capture_metadata(&ui);
    controller.set_favorite(door.id, true);
    controller.set_favorite(gust.id, true);
    let sequence = controller.metadata.lock().unwrap().sequence;
    assert!(controller.wait_metadata(sequence, true).is_ok());
    let saved = provider.get_audio(door.id).unwrap();
    assert!(saved.favorite);
    assert!(saved.tags.is_empty(), "favorite must not submit tag drafts");
    assert!(
        saved.notes.is_empty(),
        "favorite must not submit note drafts"
    );
    assert!(provider.get_audio(gust.id).unwrap().favorite);
    assert!(!provider.get_audio(creak.id).unwrap().favorite);
    let favorites_in = |folder: i64| {
        let mut names = provider
            .search(&SearchRequest {
                folder_id: Some(folder),
                scope: SearchScope::All,
                filters: SearchFilters {
                    favorites_only: true,
                    ..Default::default()
                },
                ..Default::default()
            })
            .unwrap()
            .results
            .into_iter()
            .map(|file| file.filename.clone())
            .collect::<Vec<_>>();
        names.sort();
        names
    };
    assert_eq!(favorites_in(doors), vec!["door.wav".to_string()]);
    assert_eq!(favorites_in(wind), vec!["gust.wav".to_string()]);
    controller.set_favorite(door.id, false);
    let sequence = controller.metadata.lock().unwrap().sequence;
    assert!(controller.wait_metadata(sequence, true).is_ok());
    assert!(favorites_in(doors).is_empty());
    controller.shutdown();
}
