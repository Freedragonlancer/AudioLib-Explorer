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
            ui.window().take_snapshot().unwrap();
            if scale > 1.0 {
                ui.invoke_scroll_main_to_end();
                ui.window().take_snapshot().unwrap();
            }
            assert!(!ui.get_appearance_advanced_open());
            let weak = ui.as_weak();
            ui.on_recommended_theme_requested(move |index| {
                let ui = weak.upgrade().unwrap();
                crate::theme::apply_theme(&ui, &crate::theme::theme_id(index));
                appearance::apply(&ui, &AppearancePreferences::default());
            });
            use slint::Model;
            let original = ElementHandle::find_by_accessible_label(
                &ui,
                if chinese {
                    "原始默认"
                } else {
                    "Original default"
                },
            )
            .next()
            .expect("original default preset is reachable without advanced options");
            assert_eq!(original.accessible_enabled(), Some(true));
            original.invoke_accessible_default_action();
            assert_eq!(crate::theme::theme_id(ui.get_theme_index()), "default");
            assert_eq!(
                ui.global::<Theme>().get_bg(),
                Color::from_argb_encoded(crate::theme::DEFAULT_PALETTE.bg)
            );
            assert!(!ui.global::<Theme>().get_modern_style());
            assert!(!ui.get_appearance_advanced_open());
            ui.window().take_snapshot().unwrap();
            let expected_background = ui
                .get_recommended_themes()
                .iter()
                .find(|card| crate::theme::theme_id(card.index) == "ocean")
                .unwrap()
                .bg;
            let ocean = ElementHandle::find_by_accessible_label(
                &ui,
                if chinese { "深海蓝" } else { "Ocean" },
            )
            .next()
            .expect("theme card is keyboard and screen-reader reachable");
            if scale == 1.0 {
                use slint::platform::{PointerEventButton, WindowEvent};
                let category = ElementHandle::find_by_accessible_label(
                    &ui,
                    if chinese {
                        "通用与界面"
                    } else {
                        "General / interface"
                    },
                )
                .next()
                .unwrap();
                assert!(
                    ocean.absolute_position().x
                        >= category.absolute_position().x + category.size().width,
                    "Theme cards must not overlap the settings navigation"
                );
                let origin = ocean.absolute_position();
                let size = ocean.size();
                let position = slint::LogicalPosition::new(
                    origin.x + size.width / 2.0,
                    origin.y + size.height / 2.0,
                );
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
            } else {
                ocean.invoke_accessible_default_action();
            }
            assert_eq!(crate::theme::theme_id(ui.get_theme_index()), "ocean");
            assert_eq!(ui.global::<Theme>().get_bg(), expected_background);
            for bottom in [false, true] {
                if bottom {
                    let toggle = ElementHandle::find_by_accessible_label(
                        &ui,
                        if chinese {
                            "高级选项 · 字体、文字颜色与预设"
                        } else {
                            "Advanced · Fonts, text colors and presets"
                        },
                    )
                    .next()
                    .unwrap();
                    toggle.invoke_accessible_default_action();
                    assert!(ui.get_appearance_advanced_open());
                    // Conditional advanced groups participate in layout on the next frame.
                    ui.window().take_snapshot().unwrap();
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
            let label = if chinese {
                "保存为新预设"
            } else {
                "Save new preset"
            };
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
fn recommended_theme_previews_match_distinct_readable_workbench_palettes() {
    use i_slint_backend_testing::{TestingBackend, TestingBackendOptions};
    use slint::Model;
    slint::platform::set_platform(Box::new(TestingBackend::new(TestingBackendOptions {
        mock_time: true,
        threading: false,
        renderer_name: Some("software".into()),
    })))
    .unwrap();
    let ui = AppWindow::new().unwrap();
    ui.window().set_size(slint::LogicalSize::new(1280.0, 800.0));
    ui.global::<Theme>().set_motion(0.0);
    ui.set_chinese(true);
    ui.show().unwrap();
    crate::theme::apply_theme(&ui, "default-v2");
    let previews = ui.get_recommended_themes();
    let mut surfaces = std::collections::BTreeSet::new();
    for card in previews.iter() {
        crate::theme::apply_theme(&ui, &crate::theme::theme_id(card.index));
        let theme = ui.global::<Theme>();
        assert_eq!(theme.get_bg(), card.bg);
        assert_eq!(theme.get_panel(), card.panel);
        assert_eq!(theme.get_ink(), card.ink);
        assert_eq!(theme.get_accent(), card.accent);
        assert_eq!(theme.get_selected(), card.selected);
        assert!(surfaces.insert(theme.get_bg().as_argb_encoded()));
        assert!(contrast(theme.get_ink(), theme.get_panel()) >= 4.5);
        assert!(contrast(theme.get_dim(), theme.get_panel()) >= 4.5);
        assert!(contrast(theme.get_on_accent(), theme.get_accent()) >= 4.5);
        assert!(contrast(theme.get_ink(), theme.get_selected()) >= 4.5);
        assert!(contrast(theme.get_muted(), theme.get_raised()) >= 4.5);
        assert!(contrast(theme.get_dim(), theme.get_hover()) >= 4.5);
        assert!(contrast(theme.get_ink(), theme.get_checked()) >= 4.5);
        assert!(contrast(theme.get_strong_border(), theme.get_panel()) >= 3.0);
        if crate::theme::theme_id(card.index) != "default" {
            // Preserve the original palette exactly; enhanced status contrast belongs
            // to the four redesigned themes, not a silent change to that baseline.
            for status in [theme.get_mint(), theme.get_warning(), theme.get_danger()] {
                assert!(contrast(status, theme.get_hover()) >= 4.5);
            }
            assert_eq!(theme.get_workbench_backdrop_opacity(), 0.0);
            assert_eq!(theme.get_normal_canvas(), theme.get_trough());
        }
        assert!(contrast(theme.get_dim(), theme.get_normal_canvas()) >= 4.5);
        assert_eq!(
            theme.get_pure_black_workspace(),
            crate::theme::theme_id(card.index) == "obsidian"
        );
        if crate::theme::theme_id(card.index) == "obsidian" {
            for surface in [
                theme.get_bg(),
                theme.get_library_surface(),
                theme.get_table_surface(),
                theme.get_player(),
                theme.get_normal_canvas(),
            ] {
                assert_eq!(surface, Color::from_rgb_u8(0, 0, 0));
            }
            // Neutral chrome must not inherit the retired green surfaces or accents.
            for color in [
                theme.get_bg(),
                theme.get_panel(),
                theme.get_raised(),
                theme.get_hover(),
                theme.get_ink(),
                theme.get_dim(),
                theme.get_muted(),
                theme.get_accent(),
                theme.get_selected(),
                theme.get_checked(),
                theme.get_player(),
            ] {
                let rgb = color.to_argb_u8();
                assert_eq!(rgb.red, rgb.green);
                assert_eq!(rgb.green, rgb.blue);
            }
            // Old stored identifiers also render the replacement, not the fallback theme.
            crate::theme::apply_theme(&ui, "forest");
            assert_eq!(ui.get_theme_index(), card.index);
            assert_eq!(theme.get_bg(), card.bg);
        }
        let pixels = ui.window().take_snapshot().unwrap();
        if crate::theme::theme_id(card.index) == "obsidian" {
            let black_pixels = pixels
                .as_bytes()
                .chunks_exact(4)
                .filter(|p| *p == [0, 0, 0, 255])
                .count();
            let black_fraction = black_pixels as f64 / (pixels.width() * pixels.height()) as f64;
            assert!(
                black_fraction > 0.35,
                "OLED theme must paint large opaque black work areas, got {black_fraction:.2}"
            );
        }
        {
            let canvas = theme.get_normal_canvas().to_argb_u8();
            assert!(
                pixels
                    .as_bytes()
                    .chunks_exact(4)
                    .any(|p| { p[..3] == [canvas.red, canvas.green, canvas.blue] }),
                "empty waveform must use the dark theme canvas"
            );
            let spectral = theme.get_spectral_canvas().to_argb_u8();
            assert!(
                !pixels
                    .as_bytes()
                    .chunks_exact(4)
                    .any(|p| { p[..3] == [spectral.red, spectral.green, spectral.blue] }),
                "spectral data canvas must not fill an empty waveform"
            );
        }
        let panel = card.panel.to_argb_u8();
        assert!(
            pixels
                .as_bytes()
                .chunks_exact(4)
                .any(|p| p[..3] == [panel.red, panel.green, panel.blue]),
            "theme surface must be painted"
        );
        if let Some(directory) = std::env::var_os("AUDIOLIB_UI_SCALE_SNAPSHOT_DIR") {
            let directory = std::path::Path::new(&directory);
            std::fs::create_dir_all(directory).unwrap();
            image::save_buffer(
                directory.join(format!("theme-{}.png", card.index)),
                pixels.as_bytes(),
                pixels.width(),
                pixels.height(),
                image::ColorType::Rgba8,
            )
            .unwrap();
        }
    }
    assert_eq!(surfaces.len(), 5);
    ui.hide().unwrap();
}

fn contrast(a: Color, b: Color) -> f64 {
    let luminance = |color: Color| {
        let c = color.to_argb_u8();
        let linear = |v: u8| {
            let v = f64::from(v) / 255.0;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(c.red) + 0.7152 * linear(c.green) + 0.0722 * linear(c.blue)
    };
    let a = luminance(a);
    let b = luminance(b);
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
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
