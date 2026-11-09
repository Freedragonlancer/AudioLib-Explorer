// app/audiolib-desktop/src/waveform_selection_ui_tests.rs
//! Waveform range gestures run against isolated Slint windows without audio or libraries.
use crate::{AppWindow, Theme};
use i_slint_backend_testing::ElementHandle;
use slint::{
    ComponentHandle, LogicalPosition,
    platform::{Key, PointerEventButton, WindowEvent},
};
use std::{cell::RefCell, rc::Rc, time::Duration};

fn window(modern: bool) -> AppWindow {
    let ui = AppWindow::new().unwrap();
    ui.global::<Theme>().set_modern_style(modern);
    ui.global::<Theme>().set_modern_player(modern);
    ui.window().set_size(slint::LogicalSize::new(1440.0, 900.0));
    ui.set_audio_ready(true);
    ui.set_waveform_ready(true);
    ui.set_playback_state(2);
    ui.set_play_duration(30.0);
    ui.set_view_start(0.0);
    ui.set_view_end(20.0);
    ui.set_wave_selection_identity("library-a:file-42:version-1".into());
    ui.show().unwrap();
    settle();
    ui
}

fn settle() {
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(1));
}

fn point(ui: &AppWindow, fraction: f32) -> LogicalPosition {
    let wave = ElementHandle::find_by_accessible_label(ui, "Waveform timeline")
        .next()
        .expect("Waveform timeline");
    let origin = wave.absolute_position();
    let size = wave.size();
    LogicalPosition::new(
        origin.x + size.width * fraction,
        origin.y + size.height * 0.45,
    )
}

fn moved(ui: &AppWindow, position: LogicalPosition) {
    ui.window()
        .dispatch_event(WindowEvent::PointerMoved { position });
    settle();
}

fn pressed(ui: &AppWindow, fraction: f32) {
    let position = point(ui, fraction);
    moved(ui, position);
    ui.window().dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
    settle();
}

fn released(ui: &AppWindow, fraction: f32) {
    let position = point(ui, fraction);
    ui.window().dispatch_event(WindowEvent::PointerReleased {
        position,
        button: PointerEventButton::Left,
    });
    settle();
}

fn select(ui: &AppWindow, start: f32, end: f32) {
    pressed(ui, start);
    moved(ui, point(ui, end));
    released(ui, end);
}

fn accept_ranges(ui: &AppWindow) -> Rc<RefCell<Vec<(f32, f32)>>> {
    let ranges = Rc::new(RefCell::new(Vec::new()));
    let observed = ranges.clone();
    let weak = ui.as_weak();
    ui.on_wave_selection_committed(move |start, end| {
        observed.borrow_mut().push((start, end));
        if let Some(ui) = weak.upgrade() {
            ui.set_wave_selection_start(start);
            ui.set_wave_selection_end(end);
            ui.set_wave_selection_active(true);
        }
    });
    ranges
}

fn near(actual: (f32, f32), expected: (f32, f32)) {
    assert!((actual.0 - expected.0).abs() < 0.025, "{actual:?}");
    assert!((actual.1 - expected.1).abs() < 0.025, "{actual:?}");
}

fn cursor(ui: &AppWindow, expected: &str) {
    assert_eq!(format!("{:?}", ui.get_wave_selection_cursor()), expected);
}

#[test]
fn both_players_create_reverse_ranges_resize_and_drag_exactly_once() {
    i_slint_backend_testing::init_no_event_loop();
    for modern in [false, true] {
        let ui = window(modern);
        let ranges = accept_ranges(&ui);
        ui.on_seek_play_requested(|_| panic!("Drawing a range must not seek"));
        let drags = Rc::new(RefCell::new(0));
        let observed = drags.clone();
        ui.on_wave_selection_drag_requested(move || *observed.borrow_mut() += 1);
        let ended = Rc::new(RefCell::new(0));
        let observed = ended.clone();
        ui.on_wave_selection_drag_ended(move || *observed.borrow_mut() += 1);

        moved(&ui, point(&ui, 0.7));
        cursor(&ui, "Crosshair");
        select(&ui, 0.7, 0.2);
        assert_eq!(ranges.borrow().len(), 1);
        near(ranges.borrow()[0], (4.0, 14.0));
        assert!(!ui.get_wave_selection_gesture());

        moved(&ui, point(&ui, 0.2));
        cursor(&ui, "EwResize");
        select(&ui, 0.2, 0.3);
        near(*ranges.borrow().last().unwrap(), (6.0, 14.0));
        assert_eq!(*drags.borrow(), 0, "The boundary must win over drag-out");

        // Fine boundary adjustments do not inherit the export drag threshold.
        let left = point(&ui, 0.3);
        pressed(&ui, 0.3);
        moved(&ui, LogicalPosition::new(left.x + 3.0, left.y));
        ui.window().dispatch_event(WindowEvent::PointerReleased {
            position: LogicalPosition::new(left.x + 3.0, left.y),
            button: PointerEventButton::Left,
        });
        settle();
        assert_eq!(ranges.borrow().len(), 3);
        assert!(ranges.borrow().last().unwrap().0 > 6.0);

        moved(&ui, point(&ui, 0.5));
        cursor(&ui, "Grab");
        pressed(&ui, 0.5);
        cursor(&ui, "Grabbing");
        let start = point(&ui, 0.5);
        moved(&ui, LogicalPosition::new(start.x + 3.0, start.y));
        assert_eq!(*drags.borrow(), 0);
        for offset in [8.0, 18.0, 28.0] {
            moved(&ui, LogicalPosition::new(start.x + offset, start.y));
        }
        assert_eq!(*drags.borrow(), 1);
        released(&ui, 0.55);
        assert_eq!(*ended.borrow(), 1);
        assert_eq!(ranges.borrow().len(), 3);
        ui.hide().unwrap();
    }
}

#[test]
fn shift_reselects_inside_existing_range_and_clicks_keep_seek_and_play() {
    i_slint_backend_testing::init_no_event_loop();
    for modern in [false, true] {
        let ui = window(modern);
        let ranges = accept_ranges(&ui);
        select(&ui, 0.2, 0.8);
        ui.on_wave_selection_drag_requested(|| unreachable!());
        // Shift creates a new range even when pressed inside the old selection.
        ui.window().dispatch_event(WindowEvent::KeyPressed {
            text: Key::Shift.into(),
        });
        moved(&ui, point(&ui, 0.4));
        cursor(&ui, "Crosshair");
        select(&ui, 0.4, 0.6);
        ui.window().dispatch_event(WindowEvent::KeyReleased {
            text: Key::Shift.into(),
        });
        near(*ranges.borrow().last().unwrap(), (8.0, 12.0));

        let seeks = Rc::new(RefCell::new(Vec::new()));
        let observed = seeks.clone();
        ui.on_seek_play_requested(move |time| observed.borrow_mut().push(time));
        for fraction in [0.1, 0.5] {
            pressed(&ui, fraction);
            released(&ui, fraction);
        }
        assert_eq!(seeks.borrow().len(), 2);
        assert!((seeks.borrow()[0] - 2.0).abs() < 0.025);
        assert!((seeks.borrow()[1] - 10.0).abs() < 0.025);
        assert_eq!(ranges.borrow().len(), 2);
        ui.hide().unwrap();
    }
}

#[test]
fn gesture_freezes_pending_viewport_and_identity_changes_cancel_without_committing() {
    i_slint_backend_testing::init_no_event_loop();
    for modern in [false, true] {
        let ui = window(modern);
        let ranges = accept_ranges(&ui);
        ui.set_wave_target_start(8.0);
        ui.set_wave_target_end(18.0);
        pressed(&ui, 0.2);
        assert!(ui.get_wave_selection_gesture());
        ui.set_wave_target_start(12.0);
        ui.set_wave_target_end(22.0);
        ui.set_playback_clock("seek-and-play-revision".into());
        moved(&ui, point(&ui, 0.7));
        released(&ui, 0.7);
        near(ranges.borrow()[0], (10.0, 15.0));

        pressed(&ui, 0.8);
        moved(&ui, point(&ui, 0.9));
        ui.set_wave_selection_identity("library-b:file-42:version-1".into());
        settle();
        assert!(!ui.get_wave_selection_gesture());
        released(&ui, 0.9);
        assert_eq!(
            ranges.borrow().len(),
            1,
            "The old gesture cannot select the new source"
        );
        ui.hide().unwrap();
    }
}

#[test]
fn escape_cancels_gesture_then_preparation_then_selection_and_clear_is_accessible() {
    i_slint_backend_testing::init_no_event_loop();
    for modern in [false, true] {
        let ui = window(modern);
        let ranges = accept_ranges(&ui);
        select(&ui, 0.2, 0.6);
        ui.set_help_visible(true);
        settle();
        ui.window().dispatch_event(WindowEvent::KeyPressed {
            text: Key::Escape.into(),
        });
        ui.window().dispatch_event(WindowEvent::KeyReleased {
            text: Key::Escape.into(),
        });
        settle();
        assert!(
            !ui.get_help_visible(),
            "A modal receives Escape before the selection"
        );
        assert!(ui.get_wave_selection_active());
        pressed(&ui, 0.8);
        moved(&ui, point(&ui, 0.9));
        let escape = || {
            ui.window().dispatch_event(WindowEvent::KeyPressed {
                text: Key::Escape.into(),
            });
            ui.window().dispatch_event(WindowEvent::KeyReleased {
                text: Key::Escape.into(),
            });
            settle();
        };
        escape();
        released(&ui, 0.9);
        assert_eq!(ranges.borrow().len(), 1);
        assert!(ui.get_wave_selection_active());

        let cancels = Rc::new(RefCell::new(0));
        let observed = cancels.clone();
        let weak = ui.as_weak();
        ui.on_wave_selection_cancel_requested(move || {
            *observed.borrow_mut() += 1;
            weak.unwrap().set_wave_selection_preparing(false);
        });
        ui.set_wave_selection_preparing(true);
        moved(&ui, point(&ui, 0.4));
        cursor(&ui, "Progress");
        escape();
        assert_eq!(*cancels.borrow(), 1);
        assert!(ui.get_wave_selection_active());
        let clears = Rc::new(RefCell::new(0));
        let observed = clears.clone();
        let weak = ui.as_weak();
        ui.on_wave_selection_cleared(move || {
            *observed.borrow_mut() += 1;
            weak.unwrap().set_wave_selection_active(false);
        });
        escape();
        assert_eq!(*clears.borrow(), 1);
        assert!(!ui.get_wave_selection_active());
        ui.set_wave_selection_active(true);
        ElementHandle::find_by_accessible_label(&ui, "Clear waveform selection")
            .next()
            .unwrap()
            .invoke_accessible_default_action();
        assert_eq!(*clears.borrow(), 2);
        ui.hide().unwrap();
    }
}

#[test]
fn loaded_waveform_selection_survives_transport_reload_and_an_unrelated_missing_inspector() {
    i_slint_backend_testing::init_no_event_loop();
    for modern in [false, true] {
        let ui = window(modern);
        let ranges = accept_ranges(&ui);
        pressed(&ui, 0.2);
        ui.set_audio_ready(false);
        ui.set_playback_state(1);
        ui.set_selected_missing(true);
        moved(&ui, point(&ui, 0.6));
        assert!(ui.get_wave_selection_gesture());
        released(&ui, 0.6);
        assert_eq!(ranges.borrow().len(), 1);
        near(ranges.borrow()[0], (4.0, 12.0));
        ui.hide().unwrap();
    }
}

#[test]
fn source_view_selection_and_wheel_work_with_empty_or_shorter_transport() {
    i_slint_backend_testing::init_no_event_loop();
    for modern in [false, true] {
        for expanded in [false, true] {
            for transport_duration in [0.0, 1.0] {
                let ui = window(modern);
                ui.set_audition_expanded(expanded);
                ui.set_audition_preferred_height(if expanded { 260.0 } else { 88.0 });
                if !expanded {
                    ui.window().set_size(slint::LogicalSize::new(1080.0, 700.0));
                }
                ui.set_play_duration(transport_duration);
                ui.set_audio_ready(transport_duration > 0.0);
                ui.set_playback_state(if transport_duration == 0.0 { 0 } else { 2 });
                ui.set_view_start(0.0);
                ui.set_view_end(6.0);
                settle();
                let zooms = Rc::new(RefCell::new(Vec::new()));
                let observed = zooms.clone();
                ui.on_waveform_wheel_zoom_requested(move |factor, anchor| {
                    observed.borrow_mut().push((factor, anchor));
                });
                let position = point(&ui, 0.5);
                moved(&ui, position);
                ui.window().dispatch_event(WindowEvent::PointerScrolled {
                    position,
                    delta_x: 0.0,
                    delta_y: 120.0,
                });
                settle();
                assert_eq!(zooms.borrow().len(), 1);
                near(zooms.borrow()[0], (2.0, 0.5));
                let ranges = accept_ranges(&ui);
                select(&ui, 0.25, 0.75);
                assert_eq!(ranges.borrow().len(), 1);
                near(ranges.borrow()[0], (1.5, 4.5));
                assert!(ranges.borrow()[0].1 > transport_duration);
                ui.hide().unwrap();
            }
        }
    }
}

#[test]
fn source_gesture_freezes_its_upper_bound_across_transport_and_view_changes() {
    i_slint_backend_testing::init_no_event_loop();
    for modern in [false, true] {
        let ui = window(modern);
        ui.set_play_duration(1.0);
        ui.set_view_start(0.0);
        ui.set_view_end(6.0);
        settle();
        let ranges = accept_ranges(&ui);
        pressed(&ui, 0.2);
        ui.set_play_duration(0.0);
        ui.set_view_start(6.0);
        ui.set_view_end(12.0);
        ui.set_wave_target_start(12.0);
        ui.set_wave_target_end(24.0);
        moved(&ui, point(&ui, 0.8));
        ui.set_play_duration(0.5);
        ui.set_view_start(0.0);
        ui.set_view_end(30.0);
        released(&ui, 0.8);
        assert_eq!(ranges.borrow().len(), 1);
        near(ranges.borrow()[0], (1.2, 4.8));
        ui.hide().unwrap();
    }
}

#[test]
fn legacy_seek_freezes_its_transport_duration_limit() {
    i_slint_backend_testing::init_no_event_loop();
    for modern in [false, true] {
        let ui = window(modern);
        ui.set_waveform_ready(false);
        ui.set_wave_selection_identity("".into());
        ui.set_play_duration(1.0);
        ui.set_view_start(0.0);
        ui.set_view_end(6.0);
        settle();
        let seeks = Rc::new(RefCell::new(Vec::new()));
        let observed = seeks.clone();
        ui.on_seek_play_requested(move |time| observed.borrow_mut().push(time));
        ui.on_wave_selection_committed(|_, _| {
            panic!("A legacy scrub cannot create a source range")
        });
        pressed(&ui, 0.2);
        ui.set_play_duration(0.0);
        moved(&ui, point(&ui, 0.8));
        released(&ui, 0.8);
        assert_eq!(seeks.borrow().len(), 1);
        assert!((seeks.borrow()[0] - 1.0).abs() < 0.001);
        ui.hide().unwrap();
    }
}

#[test]
fn narrow_selection_has_a_drag_grip_inside_the_compact_waveform() {
    i_slint_backend_testing::init_no_event_loop();
    for modern in [false, true] {
        let ui = window(modern);
        ui.window().set_size(slint::LogicalSize::new(1080.0, 700.0));
        ui.set_wave_selection_start(8.0);
        ui.set_wave_selection_end(8.001);
        ui.set_wave_selection_active(true);
        settle();
        let grip = ElementHandle::find_by_accessible_label(&ui, "Drag selection as WAV")
            .next()
            .expect("Narrow ranges retain a separate drag grip");
        let wave = ElementHandle::find_by_accessible_label(&ui, "Waveform timeline")
            .next()
            .unwrap();
        let origin = grip.absolute_position();
        let size = grip.size();
        assert!(origin.x >= wave.absolute_position().x);
        assert!(origin.y >= wave.absolute_position().y);
        assert!(origin.y + size.height <= wave.absolute_position().y + wave.size().height);
        let ruler = ElementHandle::find_by_accessible_label(&ui, "Waveform time ruler")
            .next()
            .unwrap();
        assert!(origin.y + size.height <= ruler.absolute_position().y);
        let start = LogicalPosition::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0);
        let requests = Rc::new(RefCell::new(0));
        let observed = requests.clone();
        ui.on_wave_selection_drag_requested(move || *observed.borrow_mut() += 1);
        let endings = Rc::new(RefCell::new(0));
        let observed = endings.clone();
        ui.on_wave_selection_drag_ended(move || *observed.borrow_mut() += 1);
        ui.on_wave_selection_committed(|_, _| panic!("The export grip must not resize"));
        ui.on_seek_play_requested(|_| panic!("The export grip must not seek"));
        moved(&ui, start);
        cursor(&ui, "Grab");
        ui.window().dispatch_event(WindowEvent::PointerPressed {
            position: start,
            button: PointerEventButton::Left,
        });
        settle();
        assert!(ui.get_wave_selection_gesture());
        cursor(&ui, "Grabbing");
        moved(&ui, LogicalPosition::new(start.x + 3.0, start.y));
        assert_eq!(*requests.borrow(), 0);
        for offset in [8.0, 18.0] {
            moved(&ui, LogicalPosition::new(start.x + offset, start.y));
        }
        assert_eq!(*requests.borrow(), 1);
        assert_eq!(
            *endings.borrow(),
            0,
            "Keep drag intent while the button is held"
        );
        ui.window().dispatch_event(WindowEvent::PointerReleased {
            position: LogicalPosition::new(start.x + 18.0, start.y),
            button: PointerEventButton::Left,
        });
        settle();
        assert_eq!(*endings.borrow(), 1);
        assert!(!ui.get_wave_selection_gesture());
        ui.hide().unwrap();
    }
}

#[test]
fn predicted_playhead_does_not_advance_past_selection_end() {
    i_slint_backend_testing::init_no_event_loop();
    for modern in [false, true] {
        let ui = window(modern);
        ui.set_wave_selection_start(4.0);
        ui.set_wave_selection_end(8.0);
        ui.set_wave_selection_active(true);
        ui.set_play_position(7.9);
        ui.set_confirmed_playback_rate(4.0);
        ui.set_playback_smooth(true);
        ui.set_playing(true);
        ui.set_playback_clock("selected-range-play".into());
        let played_fraction = || {
            // Changed handlers establish the anchor at an event-loop boundary;
            // reading geometry alone does not flush the pending clock revision.
            i_slint_backend_testing::mock_elapsed_time(Duration::ZERO);
            let wave = ElementHandle::find_by_accessible_label(&ui, "Waveform timeline")
                .next()
                .unwrap();
            let played = ElementHandle::find_by_accessible_label(&ui, "Played waveform region")
                .next()
                .unwrap();
            played.size().width / wave.size().width
        };
        let initial = played_fraction();
        assert!((initial - 0.395).abs() < 0.0001, "{modern}: {initial}");
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(16));
        let advancing = played_fraction();
        assert!(advancing > initial, "{modern}: prediction did not advance");
        assert!((advancing - 0.3982).abs() < 0.0001, "{modern}: {advancing}");
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(16));
        let clipped = played_fraction();
        assert!((clipped - 0.4).abs() < 0.0001, "{modern}: {clipped}");
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(100));
        assert!((played_fraction() - 0.4).abs() < 0.0001);
        // The same prediction exceeds the selected end once its clamp is removed.
        ui.set_wave_selection_active(false);
        let unrestricted = played_fraction();
        assert!(
            (unrestricted - 0.415).abs() < 0.0001,
            "{modern}: {unrestricted}"
        );
        ui.hide().unwrap();
    }
}

#[test]
fn export_folder_choice_is_reachable_in_both_languages() {
    i_slint_backend_testing::init_no_event_loop();
    for chinese in [false, true] {
        let ui = window(false);
        ui.set_chinese(chinese);
        ui.set_slice_export_directory("C:/Audio Slices".into());
        ui.invoke_open_global_settings(0);
        settle();
        ui.invoke_scroll_main_to_end();
        settle();
        let choices = Rc::new(RefCell::new(0));
        let observed = choices.clone();
        ui.on_slice_export_directory_requested(move || *observed.borrow_mut() += 1);
        ElementHandle::find_by_accessible_label(
            &ui,
            if chinese {
                "选择保存目录…"
            } else {
                "Choose export folder…"
            },
        )
        .next()
        .expect("Export folder chooser")
        .invoke_accessible_default_action();
        assert_eq!(*choices.borrow(), 1);
        ui.hide().unwrap();
    }
}
