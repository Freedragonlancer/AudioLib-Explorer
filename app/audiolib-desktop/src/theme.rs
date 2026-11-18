// app/audiolib-desktop/src/theme.rs
//! Appearance themes and font sizes. Built-in definitions live in core so settings
//! validation, bilingual names and presentation selection share one registry.
//! Font sizes are whole pixels; `Theme.font()` / `Theme.list-font()` combine them
//! with the interface scale.
use crate::AppWindow;
use audiolib_core::config::{
    BUILTIN_THEMES, DEFAULT_THEME, DEFAULT_V2_THEME, PlayerStyle, THEME_IDS, WorkbenchStyle,
    clamp_font_size, normalize_theme,
};
use slint::{Color, ComponentHandle, Image, ModelRc, SharedString, VecModel};

/// Color tokens of one theme, as `0xAARRGGBB`.
pub(crate) struct Palette {
    pub bg: u32,
    pub panel: u32,
    pub raised: u32,
    pub hover: u32,
    pub border: u32,
    pub strong_border: u32,
    pub ink: u32,
    pub dim: u32,
    pub muted: u32,
    pub accent: u32,
    pub accent_hover: u32,
    pub mint: u32,
    pub apricot: u32,
    pub coral: u32,
    pub on_accent: u32,
    pub selected: u32,
    pub checked: u32,
    pub alternate: u32,
    pub warning: u32,
    pub trough: u32,
    pub segment: u32,
    pub danger: u32,
    pub danger_surface: u32,
    pub player: u32,
    pub player_border: u32,
    pub tooltip: u32,
    pub overlay: u32,
}

/// Classic graphite surfaces and amber search accents; identical to the defaults in `ui/theme.slint`.
pub(crate) const DEFAULT_PALETTE: Palette = Palette {
    bg: 0xff19_1a1b,
    panel: 0xff25_2627,
    raised: 0xff30_3132,
    hover: 0xff3c_3d3e,
    border: 0xff39_3a3b,
    strong_border: 0xff85_8582,
    ink: 0xfff2_f0eb,
    dim: 0xffc4_c2bc,
    muted: 0xffa9_a9a4,
    accent: 0xffed_b85d,
    accent_hover: 0xffff_d28a,
    mint: 0xff96_d1ab,
    apricot: 0xffe6_c68d,
    coral: 0xffec_9d90,
    on_accent: 0xff25_1e12,
    selected: 0xff41_4343,
    checked: 0xff35_3737,
    alternate: 0xff29_2a2b,
    warning: 0xffff_ce86,
    trough: 0xff1a_1b1c,
    segment: 0xff45_4746,
    danger: 0xfff3_a49c,
    danger_surface: 0xff43_2b29,
    player: 0xff21_2223,
    player_border: 0xff44_4544,
    tooltip: 0xff18_191a,
    overlay: 0xa800_0000,
};

/// The PR #185 workbench palette, retained as the opt-in Studio theme.
pub(crate) const STUDIO_PALETTE: Palette = Palette {
    bg: 0xff20_2226,
    panel: 0xff24_272b,
    raised: 0xff39_3c43,
    hover: 0xff34_383e,
    border: 0xff38_3c43,
    strong_border: 0xff62_6873,
    ink: 0xffec_ebe7,
    dim: 0xffad_b0b7,
    muted: 0xff85_8b94,
    accent: 0xff2b_bfbd,
    accent_hover: 0xff66_d2c9,
    mint: 0xff79_d5bc,
    apricot: 0xffef_c4a5,
    coral: 0xffea_8b75,
    on_accent: 0xff10_2a2a,
    selected: 0xff29_3e40,
    checked: 0xff30_3c3e,
    alternate: 0xff27_2a2f,
    warning: 0xffd6_ba98,
    trough: 0xff1e_2024,
    segment: 0xff3b_4148,
    danger: 0xffdf_9e98,
    danger_surface: 0xff40_3033,
    player: 0xff29_2c31,
    player_border: 0xff41_454c,
    tooltip: 0xff19_1b1f,
    overlay: 0xa800_0000,
};

/// Cool graphite layers and mist-silver actions, with the original player layout.
pub(crate) const DEFAULT_V2_PALETTE: Palette = Palette {
    bg: 0xff20_2327,
    panel: 0xff29_2d31,
    raised: 0xff34_393e,
    hover: 0xff41_474d,
    border: 0xff45_4e56,
    strong_border: 0xff81_8b94,
    ink: 0xffe5_e7e9,
    dim: 0xffbe_c5cc,
    muted: 0xffab_b5bf,
    accent: 0xffb2_bdc5,
    accent_hover: 0xffd0_d8de,
    mint: 0xffb2_c8bc,
    apricot: 0xffd6_c39f,
    coral: 0xffd8_acaa,
    on_accent: 0xff22_2a30,
    selected: 0xff3d_4851,
    checked: 0xff48_5660,
    alternate: 0xff2c_3136,
    warning: 0xffd6_c39f,
    trough: 0xff1b_1e22,
    segment: 0xff47_525c,
    danger: 0xffd8_acaa,
    danger_surface: 0xff3c_2d30,
    player: 0xff24_282d,
    player_border: 0xff52_5e68,
    tooltip: 0xff18_1b1f,
    overlay: 0xa800_0000,
};

/// OLED-oriented black workspace with restrained charcoal elevation and silver actions.
/// Only semantic status colors use hue; no backdrop lifts the pure-black surfaces.
const OBSIDIAN_PALETTE: Palette = Palette {
    bg: 0xff00_0000,
    panel: 0xff0c_0c0c,
    raised: 0xff18_1818,
    hover: 0xff24_2424,
    border: 0xff28_2828,
    strong_border: 0xff6e_6e6e,
    ink: 0xffd0_d0d0,
    dim: 0xffa8_a8a8,
    muted: 0xff96_9696,
    accent: 0xffb8_b8b8,
    accent_hover: 0xffd0_d0d0,
    on_accent: 0xff12_1212,
    selected: 0xff28_2828,
    checked: 0xff36_3636,
    alternate: 0xff08_0808,
    mint: 0xffa9_b7ad,
    apricot: 0xffc6_b48f,
    warning: 0xffc6_b48f,
    coral: 0xffd4_9a9a,
    danger: 0xffd4_9a9a,
    danger_surface: 0xff26_1a1a,
    trough: 0xff00_0000,
    segment: 0xff33_3333,
    player: 0xff00_0000,
    player_border: 0xff30_3030,
    tooltip: 0xff12_1212,
    overlay: 0xa800_0000,
};
const OCEAN_PALETTE: Palette = Palette {
    bg: 0xff17_202b,
    panel: 0xff20_2b38,
    raised: 0xff2b_3949,
    hover: 0xff37_495c,
    border: 0xff3e_5063,
    strong_border: 0xff7f_93a7,
    ink: 0xffe0_e6ed,
    dim: 0xffb6_c3d1,
    muted: 0xffa3_b2c2,
    accent: 0xff9f_bcd5,
    accent_hover: 0xffc2_d5e5,
    mint: 0xffb2_c8bc,
    apricot: 0xffd6_c39f,
    coral: 0xffd8_acaa,
    on_accent: 0xff1d_2b39,
    selected: 0xff34_485b,
    checked: 0xff40_586d,
    alternate: 0xff24_303e,
    warning: 0xffd6_c39f,
    trough: 0xff13_1b25,
    segment: 0xff40_546a,
    danger: 0xffd8_acaa,
    danger_surface: 0xff3c_2c37,
    player: 0xff1c_2632,
    player_border: 0xff50_677d,
    tooltip: 0xff10_1822,
    overlay: 0xa800_0000,
};
const AMBER_PALETTE: Palette = Palette {
    bg: 0xff24_211e,
    panel: 0xff2d_2925,
    raised: 0xff3a_342e,
    hover: 0xff49_4036,
    border: 0xff51_473c,
    strong_border: 0xff9b_8b78,
    ink: 0xffe9_e3d9,
    dim: 0xffc5_baaa,
    muted: 0xffb4_a897,
    accent: 0xffcc_b994,
    accent_hover: 0xffe4_d5b9,
    mint: 0xffb2_c8bc,
    apricot: 0xffd6_c39f,
    coral: 0xffd8_acaa,
    on_accent: 0xff30_281e,
    selected: 0xff4b_4031,
    checked: 0xff5b_4c39,
    alternate: 0xff30_2c27,
    warning: 0xffd6_c39f,
    trough: 0xff1d_1a17,
    segment: 0xff53_483c,
    danger: 0xffd8_acaa,
    danger_surface: 0xff40_2d2b,
    player: 0xff27_231f,
    player_border: 0xff63_5545,
    tooltip: 0xff19_1613,
    overlay: 0xa800_0000,
};

fn palette_for(id: &str) -> &'static Palette {
    match id {
        DEFAULT_V2_THEME => &DEFAULT_V2_PALETTE,
        "studio" => &STUDIO_PALETTE,
        "obsidian" => &OBSIDIAN_PALETTE,
        "ocean" => &OCEAN_PALETTE,
        "amber" => &AMBER_PALETTE,
        _ => &DEFAULT_PALETTE,
    }
}

fn recommended_themes() -> ModelRc<crate::ThemePreview> {
    let cards = [
        (
            DEFAULT_V2_THEME,
            "Graphite",
            "石墨",
            "Cool graphite, mist silver",
            "冷灰层次，雾银点缀",
        ),
        (
            "obsidian",
            "Obsidian",
            "护眼黑曜",
            "OLED black, soft white",
            "OLED 纯黑，柔白文字",
        ),
        (
            "ocean",
            "Ocean",
            "深海蓝",
            "Slate blue, quiet focus",
            "低饱和蓝灰，清晰沉静",
        ),
        (
            "amber",
            "Warm amber",
            "暖砂琥珀",
            "Warm grays, soft sand",
            "暖灰底色，淡砂金点缀",
        ),
        (
            DEFAULT_THEME,
            "Classic",
            "经典默认",
            "Graphite and amber",
            "石墨灰底色，琥珀色搜索与焦点",
        ),
    ]
    .into_iter()
    .map(|(id, name, name_cn, description, description_cn)| {
        let p = palette_for(id);
        crate::ThemePreview {
            index: theme_index(id),
            name: name.into(),
            name_cn: name_cn.into(),
            description: description.into(),
            description_cn: description_cn.into(),
            bg: Color::from_argb_encoded(p.bg),
            panel: Color::from_argb_encoded(p.panel),
            ink: Color::from_argb_encoded(p.ink),
            accent: Color::from_argb_encoded(p.accent),
            selected: Color::from_argb_encoded(p.selected),
        }
    })
    .collect::<Vec<_>>();
    ModelRc::new(VecModel::from(cards))
}

/// Appearance combo index for a theme identifier; unknown themes select the default.
pub(crate) fn theme_index(id: &str) -> i32 {
    let id = normalize_theme(id);
    THEME_IDS
        .iter()
        .position(|candidate| *candidate == id)
        .unwrap_or(0) as i32
}

/// Theme identifier for an Appearance combo index; unknown indices use the default.
pub(crate) fn theme_id(index: i32) -> String {
    usize::try_from(index)
        .ok()
        .and_then(|position| THEME_IDS.get(position))
        .map_or_else(|| DEFAULT_THEME.to_owned(), |id| (*id).to_owned())
}

/// List and apply a complete theme (palette and presentation) on the UI thread.
pub(crate) fn apply_theme(ui: &AppWindow, id: &str) {
    let names = |chinese: bool| {
        let names = BUILTIN_THEMES
            .iter()
            .map(|entry| SharedString::from(if chinese { entry.name_cn } else { entry.name }))
            .collect::<Vec<_>>();
        ModelRc::new(VecModel::from(names))
    };
    ui.set_theme_names(names(false));
    ui.set_theme_names_cn(names(true));
    ui.set_recommended_themes(recommended_themes());
    let id = normalize_theme(id);
    let index = theme_index(&id);
    ui.set_theme_index(index);
    let entry = &BUILTIN_THEMES[index as usize];
    // Spectral frequency colors remain independent of the workbench palette.
    let palette = palette_for(entry.id);
    let theme = ui.global::<crate::Theme>();
    let modern_style = entry.workbench_style == WorkbenchStyle::Studio;
    let modern_player = entry.player_style == PlayerStyle::Studio;
    let tinted_surfaces = matches!(entry.id, DEFAULT_V2_THEME | "obsidian" | "ocean" | "amber");
    let presentation_changed =
        theme.get_modern_style() != modern_style || theme.get_modern_player() != modern_player;
    if presentation_changed {
        ui.invoke_prepare_workbench_switch();
        // Clear transient events before Slint creates the new presenter and binds
        // its change trackers. Clearing on a deferred style-changed handler is late.
        ui.invoke_clear_audition_input_feedback();
    }
    // Solid-color themes never display the legacy SVG backdrops. Avoid a large
    // synchronous rasterization on their first selection, especially during saves.
    if modern_style && !tinted_surfaces {
        install_studio_surfaces(&theme);
    }
    theme.set_modern_style(modern_style);
    theme.set_modern_player(modern_player);
    theme.set_tinted_surfaces(tinted_surfaces);
    theme.set_pure_black_workspace(entry.id == "obsidian");
    let color = Color::from_argb_encoded;
    theme.set_bg(color(palette.bg));
    theme.set_panel(color(palette.panel));
    theme.set_raised(color(palette.raised));
    theme.set_hover(color(palette.hover));
    theme.set_border(color(palette.border));
    theme.set_strong_border(color(palette.strong_border));
    theme.set_ink(color(palette.ink));
    theme.set_dim(color(palette.dim));
    theme.set_muted(color(palette.muted));
    theme.set_accent(color(palette.accent));
    theme.set_accent_hover(color(palette.accent_hover));
    theme.set_mint(color(palette.mint));
    theme.set_apricot(color(palette.apricot));
    theme.set_coral(color(palette.coral));
    theme.set_on_accent(color(palette.on_accent));
    theme.set_selected(color(palette.selected));
    theme.set_checked(color(palette.checked));
    theme.set_alternate(color(palette.alternate));
    theme.set_warning(color(palette.warning));
    theme.set_trough(color(palette.trough));
    theme.set_segment(color(palette.segment));
    theme.set_danger(color(palette.danger));
    theme.set_danger_surface(color(palette.danger_surface));
    theme.set_player(color(palette.player));
    theme.set_player_border(color(palette.player_border));
    theme.set_tooltip(color(palette.tooltip));
    theme.set_overlay(color(palette.overlay));
    if presentation_changed {
        ui.invoke_restore_workbench_switch();
    }
}

/// Studio surface artwork in the order of the `Theme.studio-*` images.
const STUDIO_SURFACES: [&str; 7] = [
    include_str!("../ui/surfaces/toolbar.svg"),
    include_str!("../ui/surfaces/rail.svg"),
    include_str!("../ui/surfaces/library.svg"),
    include_str!("../ui/surfaces/inspector.svg"),
    include_str!("../ui/surfaces/player.svg"),
    include_str!("../ui/surfaces/transport.svg"),
    include_str!("../ui/surfaces/tab-indicator.svg"),
];

thread_local! {
    /// Rasterized once per UI thread; images share their pixels when cloned.
    static STUDIO_SURFACE_IMAGES: Vec<Image> =
        STUDIO_SURFACES.iter().map(|svg| rasterize_surface(svg)).collect();
}

/// Give the Studio backdrops bitmaps. The software renderer has no cache for
/// rasterized SVG: an SVG backdrop is rasterized at its full drawn size on every
/// repaint that touches it, which cost a whole frame budget per playback frame.
/// Bitmaps are only sampled inside the repainted region.
fn install_studio_surfaces(theme: &crate::Theme) {
    STUDIO_SURFACE_IMAGES.with(|images| {
        let [
            toolbar,
            rail,
            library,
            inspector,
            player,
            transport,
            tab_indicator,
        ] = images.as_slice()
        else {
            return;
        };
        theme.set_studio_toolbar(toolbar.clone());
        theme.set_studio_rail(rail.clone());
        theme.set_studio_library(library.clone());
        theme.set_studio_inspector(inspector.clone());
        theme.set_studio_player(player.clone());
        theme.set_studio_transport(transport.clone());
        theme.set_studio_tab_indicator(tab_indicator.clone());
    });
}

/// Rasterize one surface at twice its design size, so typical HiDPI windows
/// scale it down or barely up. The surfaces are smooth gradients that stretch
/// (`preserveAspectRatio="none"`), so a fixed raster keeps their look.
fn rasterize_surface(svg: &str) -> Image {
    let doubled = double_svg_size(svg);
    Image::load_from_svg_data(doubled.as_bytes())
        .ok()
        .and_then(|image| image.to_rgba8())
        .map(Image::from_rgba8)
        .unwrap_or_default()
}

/// Double the root element's `width` and `height`; the viewBox keeps the art.
fn double_svg_size(svg: &str) -> String {
    let Some(root_end) = svg.find('>') else {
        return svg.to_owned();
    };
    let (root, rest) = svg.split_at(root_end);
    let mut root = root.to_owned();
    for attribute in [" width=\"", " height=\""] {
        let Some(start) = root.find(attribute).map(|at| at + attribute.len()) else {
            continue;
        };
        let Some(length) = root[start..].find('"') else {
            continue;
        };
        if let Ok(value) = root[start..start + length].parse::<f64>() {
            root.replace_range(start..start + length, &(value * 2.0).to_string());
        }
    }
    root + rest
}

/// Show and apply the interface and list font sizes (clamped to the supported range).
pub(crate) fn apply_font_sizes(ui: &AppWindow, ui_font: u32, list_font: u32) {
    let ui_font = clamp_font_size(i64::from(ui_font)) as i32;
    let list_font = clamp_font_size(i64::from(list_font)) as i32;
    ui.set_ui_font_px(ui_font);
    ui.set_list_font_px(list_font);
    let theme = ui.global::<crate::Theme>();
    theme.set_ui_font_px(ui_font);
    theme.set_list_font_px(list_font);
    crate::detail_presentation::relayout(ui);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaled_checkboxes_reserve_their_painted_bounds_and_keep_native_input() {
        use i_slint_backend_testing::{AccessibleRole, ElementHandle};
        use slint::platform::{PointerEventButton, WindowEvent};

        i_slint_backend_testing::init_no_event_loop();
        for id in THEME_IDS {
            for chinese in [false, true] {
                for scale in [1.0, 1.5, 2.0] {
                    let ui = AppWindow::new().unwrap();
                    ui.window()
                        .set_size(slint::LogicalSize::new(2400.0, 1600.0));
                    apply_theme(&ui, id);
                    apply_font_sizes(&ui, 10, 13);
                    ui.global::<crate::Theme>().set_ui_scale(scale);
                    ui.set_chinese(chinese);
                    ui.invoke_open_global_settings(3);
                    ui.show().unwrap();
                    let label = if chinese {
                        "有新版本时自动同步"
                    } else {
                        "Sync automatically when updated"
                    };
                    let checkbox = ElementHandle::find_by_accessible_label(&ui, label)
                        .find(|element| element.accessible_role() == Some(AccessibleRole::Checkbox))
                        .expect("native checkbox remains the input and accessibility owner");
                    let origin = checkbox.absolute_position();
                    let size = checkbox.size();
                    assert!(
                        size.height >= 18.0 * scale,
                        "{id}, {chinese}, x{scale}: the hit area must contain the painted indicator"
                    );
                    // A click just inside the painted indicator's lower edge must
                    // remain inside the native hit target at every supported scale.
                    let position = slint::LogicalPosition::new(
                        origin.x + 9.0 * scale,
                        origin.y + size.height / 2.0 + 9.0 * scale - 1.0,
                    );
                    let before = ui.get_ds_auto_sync();
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
                    assert_eq!(ui.get_ds_auto_sync(), !before);
                    assert!(ui.get_shared_sync_dirty());
                    ui.invoke_focus_settings_first();
                    crate::ui_tests::send_keys(&ui, &[slint::platform::Key::Tab.into()]);
                    crate::ui_tests::send_keys(&ui, &[" ".into()]);
                    assert_eq!(ui.get_ds_auto_sync(), before);
                    checkbox.invoke_accessible_default_action();
                    assert_eq!(ui.get_ds_auto_sync(), !before);
                    ui.set_shared_sync_busy(true);
                    checkbox.invoke_accessible_default_action();
                    assert_eq!(ui.get_ds_auto_sync(), !before, "disabled input is inert");
                    ui.hide().unwrap();
                }
            }
        }
    }

    #[test]
    fn scaled_slider_thumb_edges_do_not_jump_volume_before_dragging() {
        use i_slint_backend_testing::{AccessibleRole, ElementHandle};
        use slint::platform::{PointerEventButton, WindowEvent};

        i_slint_backend_testing::init_no_event_loop();
        // Studio uses its own volume control; these themes share ThemedSlider.
        for id in [DEFAULT_THEME, DEFAULT_V2_THEME] {
            for chinese in [false, true] {
                for scale in [1.0, 1.5, 2.0] {
                    let ui = AppWindow::new().unwrap();
                    ui.window()
                        .set_size(slint::LogicalSize::new(2400.0, 1600.0));
                    apply_theme(&ui, id);
                    ui.global::<crate::Theme>().set_ui_scale(scale);
                    ui.set_chinese(chinese);
                    ui.show().unwrap();
                    let label = if chinese { "音量" } else { "Volume" };
                    let volume = ElementHandle::find_by_element_type_name(&ui, "ThemedSlider")
                        .find(|widget| {
                            widget
                                .query_descendants()
                                .match_accessible_role(AccessibleRole::Slider)
                                .match_predicate(move |element| {
                                    element.accessible_label().as_deref() == Some(label)
                                })
                                .find_first()
                                .is_some()
                        })
                        .expect("the themed volume slider");
                    let thumb = volume
                        .query_descendants()
                        .match_id("ThemedSlider::thumb")
                        .find_first()
                        .expect("painted volume thumb");
                    for initial in [-60.0, -30.0, 0.0] {
                        for (x_fraction, y_fraction) in
                            [(0.1, 0.5), (0.9, 0.5), (0.5, 0.1), (0.5, 0.9)]
                        {
                            ui.set_volume_db(initial);
                            let origin = thumb.absolute_position();
                            let size = thumb.size();
                            let position = slint::LogicalPosition::new(
                                origin.x + size.width * x_fraction,
                                origin.y + size.height * y_fraction,
                            );
                            ui.window()
                                .dispatch_event(WindowEvent::PointerMoved { position });
                            ui.window().dispatch_event(WindowEvent::PointerPressed {
                                position,
                                button: PointerEventButton::Left,
                            });
                            assert!(
                                (ui.get_volume_db() - initial).abs() < 0.001,
                                "{id}, {chinese}, x{scale}, {initial}dB: pressing the painted thumb must not jump the value"
                            );
                            let direction = if initial == 0.0 { -1.0 } else { 1.0 };
                            let dragged = slint::LogicalPosition::new(
                                position.x + direction * 5.0,
                                position.y,
                            );
                            ui.window()
                                .dispatch_event(WindowEvent::PointerMoved { position: dragged });
                            assert!(
                                (ui.get_volume_db() - initial) * direction > 0.0,
                                "the native slider must still drag from every painted thumb edge"
                            );
                            ui.window().dispatch_event(WindowEvent::PointerReleased {
                                position: dragged,
                                button: PointerEventButton::Left,
                            });
                        }
                    }
                    ui.hide().unwrap();
                }
            }
        }
    }

    #[test]
    fn studio_surfaces_rasterize_at_twice_their_design_size() {
        assert_eq!(
            double_svg_size(
                r#"<svg xmlns="x" width="56" height="668" viewBox="0 0 100 1000"><path/></svg>"#
            ),
            r#"<svg xmlns="x" width="112" height="1336" viewBox="0 0 100 1000"><path/></svg>"#
        );
        STUDIO_SURFACE_IMAGES.with(|images| {
            assert_eq!(images.len(), STUDIO_SURFACES.len());
            let sizes: Vec<_> = images
                .iter()
                .map(|image| (image.size().width, image.size().height))
                .collect();
            assert_eq!(
                sizes,
                [
                    (3832, 112),
                    (112, 1336),
                    (416, 1336),
                    (592, 1336),
                    (3832, 256),
                    (200, 200),
                    (448, 6)
                ]
            );
            for image in images {
                assert!(image.to_rgba8().is_some(), "a surface is a bitmap");
            }
        });
    }
}
