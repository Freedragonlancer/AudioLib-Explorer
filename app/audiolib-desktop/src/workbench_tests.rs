// app/audiolib-desktop/src/workbench_tests.rs
//! Real pointer input verifies the compact workbench's new navigation and action owners.
use crate::{AppWindow, AudioRow};
use i_slint_backend_testing::{AccessibleRole, ElementHandle};
use slint::{
    ComponentHandle, ModelRc, VecModel,
    platform::{PointerEventButton, WindowEvent},
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

fn settle() {
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(200));
}

fn element(ui: &AppWindow, id: &str) -> ElementHandle {
    ElementHandle::find_by_element_id(ui, &format!("AppWindow::{id}"))
        .next()
        .unwrap_or_else(|| panic!("Missing {id}"))
}

fn button(ui: &AppWindow, label: &str) -> ElementHandle {
    ElementHandle::find_by_accessible_label(ui, label)
        .find(|element| element.accessible_role() == Some(AccessibleRole::Button))
        .unwrap_or_else(|| panic!("Missing button {label}"))
}

fn midpoint(element: &ElementHandle) -> slint::LogicalPosition {
    let origin = element.absolute_position();
    let size = element.size();
    slint::LogicalPosition::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0)
}

fn click(ui: &AppWindow, element: &ElementHandle) {
    let position = midpoint(element);
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
    settle();
}

#[test]
fn collapsing_panes_releases_space_and_the_audition_dock_can_expand_and_compact() {
    i_slint_backend_testing::init_no_event_loop();
    let ui = AppWindow::new().unwrap();
    ui.window()
        .set_size(slint::LogicalSize::new(1920.0, 1080.0));
    ui.show().unwrap();
    settle();
    let initial = element(&ui, "results-panel").size().width;
    click(&ui, &button(&ui, "Show / hide library sidebar"));
    assert!(!ui.get_sidebar_visible());
    let no_left = element(&ui, "results-panel").size().width;
    assert!(no_left > initial + 160.0);
    click(&ui, &button(&ui, "Hide sound details"));
    assert!(!ui.get_inspector_visible());
    assert!(element(&ui, "results-panel").size().width > no_left + 200.0);
    let compact = element(&ui, "audition").size().height;
    let list_height = element(&ui, "results-panel").size().height;
    click(&ui, &button(&ui, "Expand audition waveform"));
    assert!(ui.get_audition_expanded());
    assert!(element(&ui, "audition").size().height > compact + 80.0);
    assert!(element(&ui, "results-panel").size().height < list_height - 80.0);
    click(&ui, &button(&ui, "Compact audition waveform"));
    assert!(!ui.get_audition_expanded());
    assert!((element(&ui, "audition").size().height - compact).abs() < 1.0);
    ui.hide().unwrap();
}

#[test]
fn row_transport_tracks_the_playing_source_while_another_record_is_selected() {
    let ui = software_window();
    ui.set_rows(ModelRc::new(VecModel::from(vec![
        AudioRow {
            id: "101".into(),
            name: "A.wav".into(),
            file_path: "C:/Fixture/A.wav".into(),
            ..Default::default()
        },
        AudioRow {
            id: "202".into(),
            name: "B.wav".into(),
            file_path: "C:/Fixture/B.wav".into(),
            ..Default::default()
        },
    ])));
    ui.set_playback_path("C:/Fixture/A.wav".into());
    ui.set_playing(true);
    ui.set_playback_state(3);
    ui.set_selected_row(1);
    let pauses = Rc::new(Cell::new(0));
    let observed = pauses.clone();
    ui.on_pause_requested(move || observed.set(observed.get() + 1));
    let plays = Rc::new(RefCell::new(Vec::new()));
    let observed = plays.clone();
    ui.on_row_clicked(move |index, toggle, range, play| {
        observed.borrow_mut().push((index, toggle, range, play))
    });
    let favorites = Rc::new(RefCell::new(Vec::new()));
    let observed = favorites.clone();
    ui.on_file_action_requested(move |id, action| {
        observed.borrow_mut().push((id.to_string(), action))
    });
    ui.show().unwrap();
    settle();
    assert!(
        ElementHandle::find_by_accessible_label(&ui, "Audition: B.wav")
            .next()
            .is_none()
    );
    assert!(
        ElementHandle::find_by_accessible_label(&ui, "Pause: B.wav")
            .next()
            .is_none()
    );
    let indicator_pixels = |label: &str| {
        ui.window().dispatch_event(WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(0.0, 0.0),
        });
        settle();
        let item = button(&ui, label);
        let origin = item.absolute_position();
        let size = item.size();
        let frame = ui.window().take_snapshot().unwrap();
        let mut pixels = Vec::new();
        for y in origin.y as u32..(origin.y + size.height) as u32 {
            for x in origin.x as u32..(origin.x + size.width) as u32 {
                let offset = ((y * frame.width() + x) * 4) as usize;
                pixels.extend_from_slice(&frame.as_bytes()[offset..offset + 4]);
            }
        }
        assert!(
            pixels
                .chunks_exact(4)
                .any(|pixel| pixel[1] > 100 && pixel[1] > pixel[0].saturating_add(35))
        );
        pixels
    };
    let playing_pixels = indicator_pixels("Pause: A.wav");
    click(&ui, &button(&ui, "Pause: A.wav"));
    assert_eq!(pauses.get(), 1);
    assert!(plays.borrow().is_empty());
    ui.set_playing(false);
    ui.set_playback_state(4);
    settle();
    let paused = button(&ui, "Audition: A.wav");
    assert!(paused.accessible_description().unwrap().contains("Paused"));
    let paused_pixels = indicator_pixels("Audition: A.wav");
    assert_ne!(
        playing_pixels, paused_pixels,
        "Pausing changes the current file's visible status glyph"
    );
    assert!(
        ElementHandle::find_by_accessible_label(&ui, "Audition: B.wav")
            .next()
            .is_none()
    );
    click(&ui, &paused);
    assert_eq!(&*plays.borrow(), &[(0, false, false, true)]);
    click(&ui, &button(&ui, "Favorite: B.wav"));
    assert_eq!(&*favorites.borrow(), &[("202".to_owned(), 5)]);
    assert_eq!(ui.get_selected_row(), 1);
    ui.set_playback_state(6);
    settle();
    assert!(
        ElementHandle::find_by_accessible_label(&ui, "Audition: A.wav")
            .next()
            .is_none()
    );
    ui.hide().unwrap();
}

#[test]
fn detail_drag_exports_the_record_captured_at_press_once_even_if_selection_changes() {
    i_slint_backend_testing::init_no_event_loop();
    let ui = AppWindow::new().unwrap();
    ui.window()
        .set_size(slint::LogicalSize::new(1920.0, 1080.0));
    ui.set_has_selection(true);
    ui.set_selected_id("101".into());
    let requests = Rc::new(RefCell::new(Vec::new()));
    let observed = requests.clone();
    ui.on_drag_out_requested(move |id| observed.borrow_mut().push(id.to_string()));
    ui.show().unwrap();
    settle();
    let position = midpoint(&button(&ui, "Drag out WAV file"));
    ui.window()
        .dispatch_event(WindowEvent::PointerMoved { position });
    ui.window().dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
    ui.set_selected_id("202".into());
    for delta in [12.0, 24.0] {
        ui.window().dispatch_event(WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(position.x + delta, position.y),
        });
    }
    ui.window().dispatch_event(WindowEvent::PointerReleased {
        position,
        button: PointerEventButton::Left,
    });
    assert_eq!(&*requests.borrow(), &["101"]);
    ui.hide().unwrap();
}

fn software_window() -> AppWindow {
    use i_slint_backend_testing::{TestingBackend, TestingBackendOptions};
    slint::platform::set_platform(Box::new(TestingBackend::new(TestingBackendOptions {
        mock_time: true,
        threading: false,
        renderer_name: Some("software".into()),
    })))
    .unwrap();
    let ui = AppWindow::new().unwrap();
    ui.global::<crate::Theme>().set_motion(0.0);
    ui.window()
        .set_size(slint::LogicalSize::new(1920.0, 1080.0));
    ui
}

#[test]
fn cached_waveform_colour_stops_at_the_confirmed_playhead_and_moves_with_it() {
    use audiolib_audio::{WaveformData, WaveformPeak};
    let ui = software_window();
    let data = WaveformData {
        sample_rate: 48_000,
        channels: 2,
        frames: 48_000,
        samples_per_pixel: 375,
        peaks: vec![
            vec![
                WaveformPeak {
                    min: -0.7,
                    max: 0.7,
                    ..Default::default()
                };
                128
            ];
            2
        ],
    };
    let mask = crate::waveform_view::render_waveform_mask(&data, 128, 64, Default::default());
    ui.set_waveform_mask(slint::Image::from_rgba8(slint::SharedPixelBuffer::<
        slint::Rgba8Pixel,
    >::clone_from_slice(
        &mask, 128, 64
    )));
    ui.set_audio_ready(true);
    ui.set_waveform_ready(true);
    ui.set_view_end(10.0);
    ui.set_play_duration(10.0);
    ui.set_playback_state(4);
    ui.show().unwrap();
    settle();
    let timeline = ElementHandle::find_by_accessible_label(&ui, "Waveform timeline")
        .next()
        .unwrap();
    let origin = timeline.absolute_position();
    let size = timeline.size();
    let pixel = |frame: &slint::SharedPixelBuffer<slint::Rgba8Pixel>, fraction: f32| {
        let x = (origin.x + size.width * fraction) as u32;
        let y = (origin.y + size.height * 0.25) as u32;
        let offset = ((y * frame.width() + x) * 4) as usize;
        frame.as_bytes()[offset..offset + 4].to_vec()
    };
    let before = ui.window().take_snapshot().unwrap();
    ui.set_play_position(5.0);
    settle();
    let half = ui.window().take_snapshot().unwrap();
    let played = pixel(&half, 0.25);
    assert!(
        played[1] > played[0].saturating_add(30),
        "Played peak should be teal: {played:?}"
    );
    let near_head = pixel(&half, 0.485);
    assert!(
        near_head[0] > near_head[2].saturating_add(20),
        "Played peaks near the head should become apricot: {near_head:?}"
    );
    assert_eq!(
        pixel(&half, 0.75),
        pixel(&before, 0.75),
        "Unplayed peaks retain their neutral colour"
    );
    ui.set_play_position(8.0);
    settle();
    let advanced = ui.window().take_snapshot().unwrap();
    assert_ne!(
        pixel(&advanced, 0.75),
        pixel(&half, 0.75),
        "The colour follows confirmed source time"
    );
    if let Some(directory) = std::env::var_os("AUDIOLIB_UI_SCALE_SNAPSHOT_DIR") {
        let directory = std::path::Path::new(&directory);
        std::fs::create_dir_all(directory).unwrap();
        for (name, frame) in [
            ("progress-half.png", half),
            ("progress-advanced.png", advanced),
        ] {
            image::save_buffer(
                directory.join(name),
                frame.as_bytes(),
                frame.width(),
                frame.height(),
                image::ColorType::Rgba8,
            )
            .unwrap();
        }
    }
    ui.hide().unwrap();
}

#[test]
fn large_result_scrolling_keeps_selection_and_details_consistent() {
    let ui = software_window();
    ui.set_rows(ModelRc::new(VecModel::from(
        (0..10_000)
            .map(|index| AudioRow {
                id: index.to_string().into(),
                file_path: format!("C:/OwnedUiFixture/Recording {index:05}.wav").into(),
                name: format!("Synthetic field recording {index:05}.wav").into(),
                duration: "00:01.176".into(),
                format: "48.0k / 24b".into(),
                channels: "2".into(),
                tags: "fixture, foley".into(),
                ..Default::default()
            })
            .collect::<Vec<_>>(),
    )));
    ui.set_has_selection(true);
    ui.show().unwrap();
    settle();
    let list = element(&ui, "result-list");
    let origin = list.absolute_position();
    let pointer = slint::LogicalPosition::new(origin.x + 160.0, origin.y + 80.0);
    let mut timings = Vec::new();
    for step in 0..60 {
        let started = std::time::Instant::now();
        let index = step * 100;
        ui.set_selected_row(index);
        ui.set_selected_id(index.to_string().into());
        ui.set_selected_name(format!("Synthetic field recording {index:05}.wav").into());
        crate::detail_presentation::present(
            &ui,
            &audiolib_core::AudioFile {
                duration: 1.176,
                sample_rate: 48_000,
                bit_depth: 24,
                channels: 2,
                tags: "fixture, foley".into(),
                ..Default::default()
            },
        );
        ui.window().dispatch_event(WindowEvent::PointerScrolled {
            position: pointer,
            delta_x: 0.0,
            delta_y: -400.0,
        });
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(16));
        let frame = ui.window().take_snapshot().unwrap();
        assert_eq!(frame.width(), 1920);
        timings.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    assert_eq!(ui.get_selected_id(), "5900");
    assert!(
        ui.get_task_result_scroll_y() < -1000.0,
        "Real scroll input traverses the large result model"
    );
    timings.sort_by(f64::total_cmp);
    let report = serde_json::json!({
        "scope": "UI presentation only; synthetic owned DTOs; no database or audio-device measurement",
        "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "renderer": "Slint software/testing backend",
        "logical_size": [1920,1080],
        "records": 10000,
        "scroll_and_selection_frames": timings.len(),
        "frame_ms_p50": timings[timings.len()/2],
        "frame_ms_p95": timings[(timings.len()*95).div_ceil(100)-1],
        "frame_ms_max": timings.last(),
    });
    if let Some(directory) = std::env::var_os("AUDIOLIB_UI_SCALE_SNAPSHOT_DIR") {
        let directory = std::path::Path::new(&directory);
        std::fs::create_dir_all(directory).unwrap();
        std::fs::write(
            directory.join("scroll-performance.json"),
            serde_json::to_vec_pretty(&report).unwrap(),
        )
        .unwrap();
    }
    eprintln!("{report}");
    ui.hide().unwrap();
}
