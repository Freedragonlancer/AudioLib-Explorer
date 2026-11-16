// app/audiolib-desktop/src/appearance_tests.rs
//! Text-role inheritance, theme reset and rendered settings use isolated synthetic state.
use crate::{AppWindow, Theme, appearance};
use audiolib_core::appearance::AppearancePreferences;
use slint::{Color, ComponentHandle};

#[test]
fn appearance_roles_inherit_and_reset_without_changing_theme_surfaces() {
    i_slint_backend_testing::init_no_event_loop();
    let ui = AppWindow::new().unwrap();
    let theme = ui.global::<Theme>();
    for id in audiolib_core::config::THEME_IDS {
        crate::theme::apply_theme(&ui, id);
        let surface = theme.get_panel();
        let accent = theme.get_accent();
        let original = theme.get_ink();
        let mut prefs = AppearancePreferences {
            text_colors: appearance::builtin_colors(2).unwrap(),
            font_family: "Segoe UI".into(),
            ..Default::default()
        };
        prefs
            .text_colors
            .insert("results.normal".into(), "#ABCDEF".into());
        appearance::apply(&ui, &prefs);
        assert_eq!(
            theme.invoke_text(2, 0, original),
            Color::from_rgb_u8(0xab, 0xcd, 0xef)
        );
        assert_eq!(
            theme.invoke_text(3, 0, original),
            Color::from_rgb_u8(0xe1, 0xe8, 0xf0)
        );
        assert_eq!(theme.get_panel(), surface);
        assert_eq!(theme.get_accent(), accent);
        // Switching workbench keeps text overrides and the selected font.
        crate::theme::apply_theme(&ui, "studio");
        assert_eq!(
            theme.invoke_text(2, 0, original),
            Color::from_rgb_u8(0xab, 0xcd, 0xef)
        );
        assert_eq!(theme.get_font_family(), "Segoe UI");
        appearance::apply(&ui, &AppearancePreferences::default());
        assert_eq!(theme.invoke_text(2, 0, theme.get_ink()), theme.get_ink());
    }
}

#[test]
fn appearance_editor_renders_both_languages_and_reaches_preset_controls() {
    use i_slint_backend_testing::{ElementHandle, TestingBackend, TestingBackendOptions};
    slint::platform::set_platform(Box::new(TestingBackend::new(TestingBackendOptions {
        mock_time: true,
        threading: false,
        renderer_name: Some("software".into()),
    })))
    .unwrap();
    for chinese in [false, true] {
        for scale in [1.0, 2.0] {
            let ui = AppWindow::new().unwrap();
            ui.window().set_size(slint::LogicalSize::new(1080.0, 700.0));
            crate::theme::apply_theme(&ui, "default-v2");
            ui.global::<Theme>().set_ui_scale(scale);
            ui.set_chinese(chinese);
            ui.set_system_fonts(appearance::strings([
                "System default font".into(),
                "Segoe UI".into(),
                "Microsoft YaHei UI".into(),
            ]));
            let prefs = AppearancePreferences {
                text_colors: appearance::builtin_colors(1).unwrap(),
                ..Default::default()
            };
            appearance::apply(&ui, &prefs);
            ui.set_settings_page(1);
            ui.set_settings_visible(true);
            ui.show().unwrap();
            for bottom in [false, true] {
                if bottom {
                    ui.invoke_scroll_main_to_end();
                }
                let pixels = ui.window().take_snapshot().unwrap();
                if let Some(directory) = std::env::var_os("AUDIOLIB_UI_SCALE_SNAPSHOT_DIR") {
                    let directory = std::path::Path::new(&directory);
                    std::fs::create_dir_all(directory).unwrap();
                    image::save_buffer(
                        directory.join(format!("appearance-{chinese}-{scale}-{bottom}.png")),
                        pixels.as_bytes(),
                        pixels.width(),
                        pixels.height(),
                        image::ColorType::Rgba8,
                    )
                    .unwrap();
                }
            }
            let label = if chinese { "另存为" } else { "Save new" };
            let save = ElementHandle::find_by_accessible_label(&ui, label)
                .next()
                .expect("preset action reachable");
            assert_eq!(save.accessible_enabled(), Some(true));
            let request = std::rc::Rc::new(std::cell::Cell::new(-1));
            let observed = request.clone();
            ui.on_appearance_preset_requested(move |action| observed.set(action));
            save.invoke_accessible_default_action();
            assert_eq!(request.get(), 1);
            assert!(ui.get_settings_last_bottom() <= ui.get_settings_panel_bottom());
            ui.hide().unwrap();
        }
    }
}

#[test]
#[cfg(windows)]
fn appearance_discovers_real_system_fonts() {
    let fonts = appearance::installed_fonts();
    assert!(
        !fonts.is_empty(),
        "Windows font discovery returned no fonts"
    );
    assert!(
        fonts
            .windows(2)
            .all(|pair| pair[0].to_lowercase() < pair[1].to_lowercase())
    );
    println!("Discovered {} installed font family names", fonts.len());
}
