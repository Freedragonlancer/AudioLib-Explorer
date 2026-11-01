// app/audiolib-desktop/src/theme.rs
//! Appearance themes and font sizes. Built-in definitions live in core so settings
//! validation, bilingual names and presentation selection share one registry.
//! Font sizes are whole pixels; `Theme.font()` / `Theme.list-font()` combine them
//! with the interface scale.
use crate::AppWindow;
use audiolib_core::config::{
    BUILTIN_THEMES, DEFAULT_THEME, PlayerStyle, THEME_IDS, clamp_font_size, normalize_theme,
};
use slint::{Color, ComponentHandle, ModelRc, SharedString, VecModel};

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

/// The original soft charcoal palette; identical to the defaults in `ui/theme.slint`.
pub(crate) const DEFAULT_PALETTE: Palette = Palette {
    bg: 0xff18_1b20,
    panel: 0xff20_242a,
    raised: 0xff2a_3037,
    hover: 0xff35_3d45,
    border: 0xff35_3d45,
    strong_border: 0xff69_7581,
    ink: 0xffd9_dee3,
    dim: 0xffaa_b2bd,
    muted: 0xff95_a0ac,
    accent: 0xff8f_aea6,
    accent_hover: 0xffa8_c3bb,
    on_accent: 0xff20_2b28,
    selected: 0xff2b_3a38,
    checked: 0xff35_4744,
    alternate: 0xff22_282e,
    warning: 0xffc3_b48e,
    trough: 0xff15_181c,
    segment: 0xff3a_434c,
    danger: 0xffcf_9696,
    danger_surface: 0xff3a_2a2d,
    player: 0xff23_2a30,
    player_border: 0xff46_5059,
    tooltip: 0xff14_171c,
    overlay: 0xa800_0000,
};

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
    let id = normalize_theme(id);
    let index = theme_index(&id);
    ui.set_theme_index(index);
    let entry = &BUILTIN_THEMES[index as usize];
    // Both shipped themes deliberately share charcoal colors. Studio's new
    // audition presentation is opt-in; spectral frequency colors remain data.
    let palette = &DEFAULT_PALETTE;
    let theme = ui.global::<crate::Theme>();
    theme.set_modern_style(entry.player_style == PlayerStyle::Studio);
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
}
