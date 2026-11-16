// app/audiolib-desktop/src/window_keyboard.rs
//! Preserve activation-key repeat metadata lost by Slint 1.17.1's winit backend.
//!
//! Only real repeated Space/Enter presses are forwarded through Slint's public
//! repeated-key input. Other keys, releases and modifiers remain backend-owned.
//! Ctrl+Alt combinations also remain backend-owned: on Windows the public input
//! API cannot supply `text_without_modifiers`, which Slint needs to distinguish
//! Ctrl+Alt from AltGr. Revisit this narrow adapter when upgrading Slint.
use slint::winit_030::winit::{
    event::{ElementState, WindowEvent},
    keyboard::{Key, ModifiersState, NamedKey},
};

/// Per-window modifier tracking and native activation-key repeat forwarding.
#[derive(Default)]
pub(crate) struct NativeKeyRepeatAdapter {
    modifiers: ModifiersState,
}

impl NativeKeyRepeatAdapter {
    /// Return true only when the event was delivered to Slint here, so the
    /// caller can prevent a second delivery by the normal winit backend.
    pub(crate) fn handle_event(&mut self, window: &slint::Window, event: &WindowEvent) -> bool {
        match event {
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
                false
            }
            WindowEvent::Focused(false) => {
                self.modifiers = ModifiersState::empty();
                false
            }
            WindowEvent::KeyboardInput {
                event,
                is_synthetic,
                ..
            } => self.forward_repeat(
                window,
                &event.logical_key,
                event.state,
                event.repeat,
                *is_synthetic,
            ),
            _ => false,
        }
    }

    fn forward_repeat(
        &self,
        window: &slint::Window,
        key: &Key,
        state: ElementState,
        repeated: bool,
        synthetic: bool,
    ) -> bool {
        let Some(text) = repeat_text(key, state, repeated, synthetic, self.modifiers) else {
            return false;
        };
        window
            .try_dispatch_event(slint::platform::WindowEvent::KeyPressRepeated { text })
            .is_ok()
    }
}

fn repeat_text(
    key: &Key,
    state: ElementState,
    repeated: bool,
    synthetic: bool,
    modifiers: ModifiersState,
) -> Option<slint::SharedString> {
    if !repeated
        || synthetic
        || state != ElementState::Pressed
        || (modifiers.control_key() && modifiers.alt_key())
    {
        return None;
    }
    // Match Slint 1.17.1's to_slint_key mapping exactly for these two keys.
    // Named Enter maps to LF (Key::Return), regardless of winit event.text.
    match key {
        Key::Named(NamedKey::Space) => Some(" ".into()),
        Key::Named(NamedKey::Enter) => Some(slint::platform::Key::Return.into()),
        Key::Character(text) if text == " " || text == "\n" => Some(text.as_str().into()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppWindow, AudioRow};
    use slint::{ComponentHandle, ModelRc, VecModel, platform::WindowEvent as Input};
    use std::{cell::Cell, rc::Rc, time::Duration};

    #[test]
    fn repeat_mapping_is_limited_to_real_pressed_activation_keys() {
        let space = Key::Named(NamedKey::Space);
        let enter = Key::Named(NamedKey::Enter);
        for (key, expected) in [
            (space.clone(), " "),
            (enter, "\n"),
            (Key::Character(" ".into()), " "),
            (Key::Character("\n".into()), "\n"),
        ] {
            for modifiers in [
                ModifiersState::empty(),
                ModifiersState::SHIFT,
                ModifiersState::CONTROL,
                ModifiersState::ALT,
            ] {
                assert_eq!(
                    repeat_text(&key, ElementState::Pressed, true, false, modifiers).as_deref(),
                    Some(expected)
                );
            }
        }
        for (state, repeated, synthetic, modifiers) in [
            (ElementState::Released, true, false, ModifiersState::empty()),
            (ElementState::Pressed, false, false, ModifiersState::empty()),
            (ElementState::Pressed, true, true, ModifiersState::empty()),
            (
                ElementState::Pressed,
                true,
                false,
                ModifiersState::CONTROL | ModifiersState::ALT,
            ),
        ] {
            assert!(repeat_text(&space, state, repeated, synthetic, modifiers).is_none());
        }
        for key in [Key::Named(NamedKey::ArrowDown), Key::Character("a".into())] {
            assert!(
                repeat_text(
                    &key,
                    ElementState::Pressed,
                    true,
                    false,
                    ModifiersState::empty()
                )
                .is_none()
            );
        }
    }

    #[test]
    fn forwarded_repeats_keep_text_entry_and_do_not_rearm_result_shortcuts() {
        i_slint_backend_testing::init_no_event_loop();
        for theme in audiolib_core::config::THEME_IDS {
            let ui = AppWindow::new().unwrap();
            crate::theme::apply_theme(&ui, theme);
            ui.window().set_size(slint::LogicalSize::new(1440.0, 900.0));
            ui.set_rows(ModelRc::new(VecModel::from(vec![AudioRow {
                id: "1".into(),
                name: "Native repeat fixture.wav".into(),
                ..Default::default()
            }])));
            ui.set_selected_row(0);
            ui.set_audio_ready(true);
            ui.set_audition_play_on_single_click(true);
            let actions = Rc::new(Cell::new(0));
            let observed = actions.clone();
            ui.on_play_requested(move || observed.set(observed.get() + 1));
            let observed = actions.clone();
            ui.on_audition_requested(move |_| observed.set(observed.get() + 1));
            ui.show().unwrap();
            let mut adapter = NativeKeyRepeatAdapter::default();
            for key in [Key::Named(NamedKey::Space), Key::Named(NamedKey::Enter)] {
                crate::ui_tests::click_label(&ui, "Native repeat fixture.wav");
                i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(1));
                let text = repeat_text(
                    &key,
                    ElementState::Pressed,
                    true,
                    false,
                    ModifiersState::empty(),
                )
                .unwrap();
                let before = actions.get();
                ui.window()
                    .dispatch_event(Input::KeyPressed { text: text.clone() });
                assert_eq!(actions.get(), before + 1);
                crate::ui_tests::click_label(&ui, "Search keywords");
                i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(1));
                crate::ui_tests::click_label(&ui, "Native repeat fixture.wav");
                i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(1));
                assert!(adapter.forward_repeat(
                    ui.window(),
                    &key,
                    ElementState::Pressed,
                    true,
                    false
                ));
                assert_eq!(
                    actions.get(),
                    before + 1,
                    "{theme}: interrupted repeat stays inert"
                );
                ui.window()
                    .dispatch_event(Input::KeyReleased { text: text.clone() });
                ui.window()
                    .dispatch_event(Input::KeyPressed { text: text.clone() });
                assert_eq!(actions.get(), before + 2);
                ui.window().dispatch_event(Input::KeyReleased { text });
            }
            crate::ui_tests::click_label(&ui, "Search keywords");
            crate::ui_tests::send_keys(&ui, &[slint::platform::Key::Control.into(), "a".into()]);
            crate::ui_tests::send_keys(&ui, &["x".into()]);
            for _ in 0..3 {
                assert!(adapter.forward_repeat(
                    ui.window(),
                    &Key::Named(NamedKey::Space),
                    ElementState::Pressed,
                    true,
                    false,
                ));
            }
            assert_eq!(
                ui.get_query(),
                "x   ",
                "{theme}: text entry keeps native repeats"
            );
            // Modifier changes/releases are never consumed. Ctrl+Alt repeat
            // dispatch also stays with winit, preserving its Windows AltGr data.
            assert!(!adapter.handle_event(
                ui.window(),
                &WindowEvent::ModifiersChanged(
                    (ModifiersState::CONTROL | ModifiersState::ALT).into(),
                )
            ));
            assert!(!adapter.forward_repeat(
                ui.window(),
                &Key::Named(NamedKey::Space),
                ElementState::Pressed,
                true,
                false,
            ));
            assert_eq!(ui.get_query(), "x   ");
            assert!(!adapter.handle_event(ui.window(), &WindowEvent::Focused(false)));
            assert!(adapter.modifiers.is_empty());
            ui.hide().unwrap();
        }
    }
}
