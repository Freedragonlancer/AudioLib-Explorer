// app/audiolib-desktop/src/pane_review_tests.rs
//! Real input checks for pane divider keyboard, cancellation and modal boundaries.
use crate::AppWindow;
use i_slint_backend_testing::ElementHandle;
use slint::{ComponentHandle, platform::PointerEventButton, platform::WindowEvent};
use std::time::Duration;

#[test]
fn pane_keyboard_arrows_resize_in_visual_direction_and_obey_limits() {
    i_slint_backend_testing::init_no_event_loop();
    for chinese in [false, true] {
        let ui = window(chinese);
        click_divider(&ui, false);
        key(&ui, slint::platform::Key::RightArrow);
        near(ui.get_sidebar_preferred_width(), 214.0);
        key(&ui, slint::platform::Key::LeftArrow);
        near(ui.get_sidebar_preferred_width(), 198.0);
        for _ in 0..30 {
            key(&ui, slint::platform::Key::RightArrow);
        }
        near(ui.get_sidebar_preferred_width(), 480.0);
        for _ in 0..30 {
            key(&ui, slint::platform::Key::LeftArrow);
        }
        near(ui.get_sidebar_preferred_width(), 170.0);

        click_divider(&ui, true);
        key(&ui, slint::platform::Key::LeftArrow);
        near(ui.get_inspector_preferred_width(), 336.0);
        key(&ui, slint::platform::Key::RightArrow);
        near(ui.get_inspector_preferred_width(), 320.0);
        for _ in 0..30 {
            key(&ui, slint::platform::Key::LeftArrow);
        }
        near(ui.get_inspector_preferred_width(), 600.0);
        for _ in 0..30 {
            key(&ui, slint::platform::Key::RightArrow);
        }
        near(ui.get_inspector_preferred_width(), 240.0);
        ui.hide().unwrap();
    }
}

#[test]
fn pane_keyboard_and_pointer_requests_are_blocked_by_every_modal_guard() {
    i_slint_backend_testing::init_no_event_loop();
    for chinese in [false, true] {
        for guard in 0..10 {
            for right in [false, true] {
                let ui = window(chinese);
                let divider = divider(&ui, right);
                let position = midpoint(&divider);
                click_divider(&ui, right);
                let preferred = preferred_width(&ui, right);
                set_guard(&ui, guard, true);
                settle();
                assert_eq!(divider.accessible_enabled(), Some(false), "guard {guard}");
                key(&ui, slint::platform::Key::LeftArrow);
                key(&ui, slint::platform::Key::RightArrow);
                pointer(&ui, position, PointerEventButton::Left, true);
                move_pointer(&ui, position, 48.0);
                pointer(
                    &ui,
                    slint::LogicalPosition::new(position.x + 48.0, position.y),
                    PointerEventButton::Left,
                    false,
                );
                near(preferred_width(&ui, right), preferred);
                set_guard(&ui, guard, false);
                ui.hide().unwrap();
            }
        }
    }
}

#[test]
fn disabling_a_drag_cancels_it_and_a_new_drag_still_works() {
    i_slint_backend_testing::init_no_event_loop();
    for right in [false, true] {
        let ui = window(false);
        let position = midpoint(&divider(&ui, right));
        let preferred = preferred_width(&ui, right);
        pointer(&ui, position, PointerEventButton::Left, true);
        ui.set_library_busy(true);
        move_pointer(&ui, position, 48.0);
        near(preferred_width(&ui, right), preferred);
        ui.set_library_busy(false);
        move_pointer(&ui, position, 64.0);
        near(preferred_width(&ui, right), preferred);
        pointer(
            &ui,
            slint::LogicalPosition::new(position.x + 64.0, position.y),
            PointerEventButton::Left,
            false,
        );

        let fresh = midpoint(&divider(&ui, right));
        pointer(&ui, fresh, PointerEventButton::Left, true);
        move_pointer(&ui, fresh, 16.0);
        pointer(
            &ui,
            slint::LogicalPosition::new(fresh.x + 16.0, fresh.y),
            PointerEventButton::Left,
            false,
        );
        near(
            preferred_width(&ui, right),
            preferred + if right { -16.0 } else { 16.0 },
        );
        ui.hide().unwrap();
    }
}

#[test]
fn non_left_pointer_buttons_and_pointer_exit_never_leave_an_active_drag() {
    i_slint_backend_testing::init_no_event_loop();
    for right in [false, true] {
        let ui = window(false);
        let preferred = preferred_width(&ui, right);
        for button in [
            PointerEventButton::Right,
            PointerEventButton::Middle,
            PointerEventButton::Other,
        ] {
            let position = midpoint(&divider(&ui, right));
            pointer(&ui, position, button, true);
            move_pointer(&ui, position, 48.0);
            pointer(
                &ui,
                slint::LogicalPosition::new(position.x + 48.0, position.y),
                button,
                false,
            );
            near(preferred_width(&ui, right), preferred);
        }
        let position = midpoint(&divider(&ui, right));
        pointer(&ui, position, PointerEventButton::Left, true);
        ui.window().dispatch_event(WindowEvent::PointerExited);
        move_pointer(&ui, position, 48.0);
        near(preferred_width(&ui, right), preferred);
        pointer(&ui, position, PointerEventButton::Left, false);
        ui.hide().unwrap();
    }
}

fn window(chinese: bool) -> AppWindow {
    let ui = AppWindow::new().unwrap();
    ui.set_chinese(chinese);
    ui.window().set_size(slint::LogicalSize::new(2200.0, 900.0));
    ui.show().unwrap();
    settle();
    ui
}

fn divider(ui: &AppWindow, right: bool) -> ElementHandle {
    let label = match (ui.get_chinese(), right) {
        (false, false) => "Resize library sidebar",
        (false, true) => "Resize sound details",
        (true, false) => "调整素材库侧栏宽度",
        (true, true) => "调整声音详情宽度",
    };
    ElementHandle::find_by_accessible_label(ui, label)
        .next()
        .expect(label)
}

fn preferred_width(ui: &AppWindow, right: bool) -> f32 {
    if right {
        ui.get_inspector_preferred_width()
    } else {
        ui.get_sidebar_preferred_width()
    }
}

fn midpoint(element: &ElementHandle) -> slint::LogicalPosition {
    let origin = element.absolute_position();
    let size = element.size();
    slint::LogicalPosition::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0)
}

fn click_divider(ui: &AppWindow, right: bool) {
    let position = midpoint(&divider(ui, right));
    pointer(ui, position, PointerEventButton::Left, true);
    pointer(ui, position, PointerEventButton::Left, false);
}

fn pointer(
    ui: &AppWindow,
    position: slint::LogicalPosition,
    button: PointerEventButton,
    pressed: bool,
) {
    if pressed {
        ui.window()
            .dispatch_event(WindowEvent::PointerMoved { position });
        ui.window()
            .dispatch_event(WindowEvent::PointerPressed { position, button });
    } else {
        ui.window()
            .dispatch_event(WindowEvent::PointerReleased { position, button });
    }
    settle();
}

fn move_pointer(ui: &AppWindow, start: slint::LogicalPosition, delta: f32) {
    ui.window().dispatch_event(WindowEvent::PointerMoved {
        position: slint::LogicalPosition::new(start.x + delta, start.y),
    });
    settle();
}

fn key(ui: &AppWindow, key: slint::platform::Key) {
    crate::ui_tests::send_keys(ui, &[key.into()]);
    settle();
}

fn set_guard(ui: &AppWindow, guard: usize, visible: bool) {
    match guard {
        0 => ui.set_library_busy(visible),
        1 => ui.set_closing(visible),
        2 => ui.set_library_visible(visible),
        3 => ui.set_confirmation_visible(visible),
        4 => ui.set_batch_visible(visible),
        5 => ui.set_import_visible(visible),
        6 => ui.set_settings_visible(visible),
        7 => ui.set_peaks_visible(visible),
        8 => ui.set_about_visible(visible),
        9 => ui.set_help_visible(visible),
        _ => unreachable!(),
    }
}

fn settle() {
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(1));
}

fn near(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() <= 1.0,
        "Expected {expected}px, got {actual}px"
    );
}
