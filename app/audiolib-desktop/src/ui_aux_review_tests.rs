// app/audiolib-desktop/src/ui_aux_review_tests.rs
//! Cross-theme toggle semantics and Studio transport gesture boundaries.
use crate::AppWindow;
use audiolib_core::config::{STUDIO_THEME, THEME_IDS};
use i_slint_backend_testing::ElementHandle;
use slint::{
    ComponentHandle,
    platform::{Key, PointerEventButton, WindowEvent},
};
use std::{cell::Cell, rc::Rc, time::Duration};

fn window(theme: &str, chinese: bool) -> AppWindow {
    let ui = AppWindow::new().unwrap();
    crate::theme::apply_theme(&ui, theme);
    ui.set_chinese(chinese);
    ui.global::<crate::Theme>().set_motion(0.0);
    ui.window()
        .set_size(slint::LogicalSize::new(1600.0, 1000.0));
    ui.set_audio_ready(true);
    ui.set_playback_state(2);
    ui.set_volume_db(-12.0);
    ui.show().unwrap();
    settle();
    ui
}

fn settle() {
    i_slint_backend_testing::mock_elapsed_time(Duration::ZERO);
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(10));
}

fn action(ui: &AppWindow, label: &str) -> ElementHandle {
    ElementHandle::find_by_element_type_name(ui, "AuditionPanel")
        .flat_map(|panel| {
            let label = label.to_owned();
            panel
                .query_descendants()
                .match_predicate(move |element| {
                    element.accessible_label().as_deref() == Some(label.as_str())
                })
                .find_all()
        })
        .next()
        .unwrap_or_else(|| {
            panic!(
                "Missing player action {label}: scale={}, font={}, chinese={}, expanded={}, viewport={}x{}, last=({}, {})",
                ui.global::<crate::Theme>().get_ui_scale(),
                ui.global::<crate::Theme>().get_ui_font_px(),
                ui.get_chinese(),
                ui.get_audition_expanded(),
                ui.get_main_viewport_width(),
                ui.get_main_viewport_height(),
                ui.get_main_last_right(),
                ui.get_main_last_bottom(),
            )
        })
}

#[test]
fn player_toggles_publish_their_current_state_in_every_theme_and_language() {
    i_slint_backend_testing::init_no_event_loop();
    for theme in THEME_IDS {
        for chinese in [false, true] {
            let ui = window(theme, chinese);
            for label in if chinese {
                ["循环整个声音", "单声道监听"]
            } else {
                ["Repeat sound", "Mono monitoring"]
            } {
                let control = action(&ui, label);
                assert_eq!(
                    control.accessible_checkable(),
                    Some(true),
                    "{theme}: {label}"
                );
                assert_eq!(control.accessible_checked(), Some(false));
                control.invoke_accessible_default_action();
                assert_eq!(control.accessible_checked(), Some(true));
                control.invoke_accessible_default_action();
                assert_eq!(control.accessible_checked(), Some(false));
            }
            if theme == STUDIO_THEME {
                let mute = action(&ui, if chinese { "静音" } else { "Mute" });
                assert_eq!(mute.accessible_checkable(), Some(true));
                assert_eq!(mute.accessible_checked(), Some(false));
                mute.invoke_accessible_default_action();
                let unmute = action(&ui, if chinese { "取消静音" } else { "Unmute" });
                assert_eq!(unmute.accessible_checked(), Some(true));
                unmute.invoke_accessible_default_action();
                assert_eq!(ui.get_volume_db(), -12.0);
            }
            ui.hide().unwrap();
        }
    }
}

fn pointer(ui: &AppWindow, position: slint::LogicalPosition, down: bool) {
    ui.window()
        .dispatch_event(WindowEvent::PointerMoved { position });
    ui.window().dispatch_event(if down {
        WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Left,
        }
    } else {
        WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Left,
        }
    });
}

#[test]
fn studio_transport_requires_a_fresh_enabled_pointer_gesture() {
    i_slint_backend_testing::init_no_event_loop();
    for chinese in [false, true] {
        let ui = window(STUDIO_THEME, chinese);
        let actions = Rc::new(Cell::new(0));
        let observed = actions.clone();
        ui.on_start_requested(move || observed.set(observed.get() + 1));
        let observed = actions.clone();
        ui.on_seek_requested(move |_| observed.set(observed.get() + 1));
        for label in if chinese {
            ["播放", "回到开头"]
        } else {
            ["Play", "Go to start"]
        } {
            let control = action(&ui, label);
            let origin = control.absolute_position();
            let size = control.size();
            let center = slint::LogicalPosition::new(
                origin.x + size.width / 2.0,
                origin.y + size.height / 2.0,
            );
            for enabled_at_press in [false, true] {
                ui.set_audio_ready(enabled_at_press);
                settle();
                pointer(&ui, center, true);
                ui.set_audio_ready(false);
                settle();
                ui.set_audio_ready(true);
                settle();
                let before = actions.get();
                pointer(&ui, center, false);
                assert_eq!(
                    actions.get(),
                    before,
                    "{label}: a disabled transition cancels the held gesture"
                );
                pointer(&ui, center, true);
                pointer(&ui, center, false);
                assert_eq!(
                    actions.get(),
                    before + 1,
                    "{label}: a fresh enabled click activates once"
                );
            }
        }
        ui.hide().unwrap();
    }
}

fn key(ui: &AppWindow, text: slint::SharedString, down: bool) {
    ui.window().dispatch_event(if down {
        WindowEvent::KeyPressed { text }
    } else {
        WindowEvent::KeyReleased { text }
    });
}

fn repeat_key(ui: &AppWindow, text: slint::SharedString) {
    ui.window()
        .dispatch_event(WindowEvent::KeyPressRepeated { text });
}

fn focus_from_pitch(ui: &AppWindow, chinese: bool, backtabs: usize) {
    let pitch = action(
        ui,
        if chinese {
            "变调半音，回车或离开时应用"
        } else {
            "Pitch semitones, applied on Enter or focus loss"
        },
    );
    let origin = pitch.absolute_position();
    let size = pitch.size();
    let center =
        slint::LogicalPosition::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0);
    pointer(ui, center, true);
    pointer(ui, center, false);
    // Repeat immediately precedes Pitch in every player presentation.
    for _ in 0..backtabs {
        key(ui, Key::Backtab.into(), true);
        key(ui, Key::Backtab.into(), false);
    }
    settle();
}

#[test]
fn studio_transport_keys_release_once_and_cancel_when_interrupted() {
    i_slint_backend_testing::init_no_event_loop();
    for chinese in [false, true] {
        let ui = window(STUDIO_THEME, chinese);
        let actions = Rc::new(Cell::new(0));
        let observed = actions.clone();
        ui.on_start_requested(move || observed.set(observed.get() + 1));
        let observed = actions.clone();
        ui.on_transform_requested(move |_, _, _, _| observed.set(observed.get() + 1));
        for backtabs in [1, 4] {
            for activation in [slint::SharedString::from(" "), Key::Return.into()] {
                focus_from_pitch(&ui, chinese, backtabs);
                actions.set(0);
                for _ in 0..3 {
                    key(&ui, activation.clone(), true);
                }
                assert_eq!(actions.get(), 0, "A held key waits for release");
                key(&ui, activation.clone(), false);
                assert_eq!(actions.get(), 1, "Repeated key-down activates only once");
                key(&ui, activation.clone(), false);
                assert_eq!(actions.get(), 1, "Unarmed release cannot activate");

                actions.set(0);
                key(&ui, activation.clone(), true);
                key(&ui, Key::Escape.into(), true);
                key(&ui, Key::Escape.into(), false);
                repeat_key(&ui, activation.clone());
                key(&ui, activation.clone(), false);
                assert_eq!(actions.get(), 0, "Escape cancels the held key");

                key(&ui, activation.clone(), true);
                focus_from_pitch(&ui, chinese, backtabs);
                actions.set(0);
                repeat_key(&ui, activation.clone());
                key(&ui, activation.clone(), false);
                assert_eq!(
                    actions.get(),
                    0,
                    "Losing then regaining focus cancels the held key"
                );

                if backtabs == 4 {
                    key(&ui, activation.clone(), true);
                    ui.set_audio_ready(false);
                    settle();
                    ui.set_audio_ready(true);
                    settle();
                    let before = actions.get();
                    repeat_key(&ui, activation.clone());
                    key(&ui, activation, false);
                    assert_eq!(actions.get(), before, "Disabling cancels the held Play key");
                }
            }
        }
        ui.hide().unwrap();
    }
}

#[test]
fn interrupted_player_keys_do_not_rearm_from_native_repeat_in_any_theme() {
    i_slint_backend_testing::init_no_event_loop();
    for theme in THEME_IDS {
        for chinese in [false, true] {
            let ui = window(theme, chinese);
            let actions = Rc::new(Cell::new(0));
            let observed = actions.clone();
            ui.on_transform_requested(move |_, _, _, _| observed.set(observed.get() + 1));
            for activation in [slint::SharedString::from(" "), Key::Return.into()] {
                focus_from_pitch(&ui, chinese, 1);
                actions.set(0);
                key(&ui, activation.clone(), true);
                key(&ui, Key::Escape.into(), true);
                key(&ui, Key::Escape.into(), false);
                repeat_key(&ui, activation.clone());
                key(&ui, activation.clone(), false);
                assert_eq!(
                    actions.get(),
                    0,
                    "{theme}: repeat cannot rearm after Escape"
                );

                key(&ui, activation.clone(), true);
                focus_from_pitch(&ui, chinese, 1);
                actions.set(0);
                repeat_key(&ui, activation.clone());
                key(&ui, activation.clone(), false);
                assert_eq!(
                    actions.get(),
                    0,
                    "{theme}: repeat cannot rearm after focus loss"
                );

                key(&ui, activation.clone(), true);
                repeat_key(&ui, activation.clone());
                key(&ui, activation, false);
                assert_eq!(
                    actions.get(),
                    1,
                    "{theme}: a fresh press still activates once"
                );
            }
            ui.hide().unwrap();
        }
    }
}

fn paint_layout(ui: &AppWindow) {
    // Theme application restores the old main scroll offsets in two deferred
    // layout turns. Finish that restoration before this test scrolls the dock.
    for _ in 0..16 {
        settle();
        ui.window().take_snapshot().unwrap();
        if !ui.get_task_restoring() {
            settle();
            ui.window().take_snapshot().unwrap();
            return;
        }
    }
    panic!("The Studio workbench restoration did not finish");
}

fn assert_in_window(ui: &AppWindow, element: &ElementHandle, context: &str) {
    let viewport = ui.window().size().to_logical(ui.window().scale_factor());
    let origin = element.absolute_position();
    let size = element.size();
    assert!(
        size.width > 0.0
            && size.height > 0.0
            && origin.x >= -0.5
            && origin.y >= -0.5
            && origin.x + size.width <= viewport.width + 0.5
            && origin.y + size.height <= viewport.height + 0.5,
        "{context}: {origin:?} + {size:?} outside {viewport:?}"
    );
}

#[test]
fn scaled_studio_dock_scrolls_to_its_rightmost_action_and_keyboard_play() {
    use i_slint_backend_testing::{TestingBackend, TestingBackendOptions};
    slint::platform::set_platform(Box::new(TestingBackend::new(TestingBackendOptions {
        mock_time: true,
        threading: false,
        renderer_name: Some("software".into()),
    })))
    .unwrap();
    for chinese in [false, true] {
        for scale in [1.5, 2.0] {
            for font in [13, 24] {
                let ui = window(STUDIO_THEME, chinese);
                ui.window().set_size(slint::LogicalSize::new(1080.0, 700.0));
                ui.global::<crate::Theme>().set_ui_scale(scale);
                crate::theme::apply_font_sizes(&ui, font, 13);
                let starts = Rc::new(Cell::new(0));
                let observed = starts.clone();
                ui.on_start_requested(move || observed.set(observed.get() + 1));
                paint_layout(&ui);
                for expanded in [false, true] {
                    ui.invoke_scroll_main_to_end();
                    paint_layout(&ui);
                    let label = if expanded {
                        if chinese {
                            "收起试听波形"
                        } else {
                            "Compact audition waveform"
                        }
                    } else if chinese {
                        "展开试听波形"
                    } else {
                        "Expand audition waveform"
                    };
                    let control = action(&ui, label);
                    let context = format!("{label}, scale={scale}, font={font}");
                    assert_in_window(&ui, &control, &context);
                    let origin = control.absolute_position();
                    let size = control.size();
                    let center = slint::LogicalPosition::new(
                        origin.x + size.width / 2.0,
                        origin.y + size.height / 2.0,
                    );
                    pointer(&ui, center, true);
                    pointer(&ui, center, false);
                    assert_eq!(ui.get_audition_expanded(), !expanded, "{context}");
                }

                // Starting at the far right, native Tab must reveal the left
                // transport owner before it accepts keyboard playback.
                ui.invoke_scroll_main_to_end();
                paint_layout(&ui);
                let label = if chinese { "播放" } else { "Play" };
                for _ in 0..160 {
                    key(&ui, Key::Tab.into(), true);
                    key(&ui, Key::Tab.into(), false);
                    settle();
                    let play = ElementHandle::find_by_element_type_name(&ui, "AuditionPanel")
                        .flat_map(|panel| {
                            panel
                                .query_descendants()
                                .match_predicate(move |element| {
                                    element.accessible_label().as_deref() == Some(label)
                                })
                                .find_all()
                        })
                        .next();
                    if let Some(play) = play {
                        let origin = play.absolute_position();
                        let size = play.size();
                        if origin.x >= 0.0
                            && origin.y >= 0.0
                            && origin.x + size.width <= 1080.5
                            && origin.y + size.height <= 700.5
                        {
                            key(&ui, " ".into(), true);
                            key(&ui, " ".into(), false);
                            if starts.get() > 0 {
                                assert_in_window(&ui, &play, "Focused Studio Play");
                                break;
                            }
                        }
                    }
                }
                assert_eq!(
                    starts.get(),
                    1,
                    "Keyboard Play at scale={scale}, font={font}, chinese={chinese}"
                );
                ui.hide().unwrap();
            }
        }
    }
}
