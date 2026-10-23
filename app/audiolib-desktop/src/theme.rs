// app/audiolib-desktop/src/theme.rs
//! Appearance themes and font sizes. A theme is a palette of Slint `Theme` color
//! tokens set from Rust; only the default theme (the original look) ships for now.
//! Font sizes are whole pixels; `Theme.font()` / `Theme.list-font()` combine them
//! with the interface scale.
use crate::AppWindow;
use audiolib_core::config::{DEFAULT_THEME, THEME_IDS, clamp_font_size, normalize_theme};
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
    player: 0xff23_2a30,
    player_border: 0xff46_5059,
    tooltip: 0xff14_171c,
    overlay: 0xa800_0000,
};

/// Palette and English/Chinese display names for a known theme identifier.
fn theme_entry(id: &str) -> (&'static Palette, &'static str, &'static str) {
    match id {
        DEFAULT_THEME => (&DEFAULT_PALETTE, "Default", "默认"),
        _ => (&DEFAULT_PALETTE, "Default", "默认"),
    }
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

/// List the themes, select `id` and apply its palette to the Slint `Theme` global.
pub(crate) fn apply_theme(ui: &AppWindow, id: &str) {
    let names = |chinese: bool| {
        let names = THEME_IDS
            .iter()
            .map(|id| {
                let (_, english, chinese_name) = theme_entry(id);
                SharedString::from(if chinese { chinese_name } else { english })
            })
            .collect::<Vec<_>>();
        ModelRc::new(VecModel::from(names))
    };
    ui.set_theme_names(names(false));
    ui.set_theme_names_cn(names(true));
    let id = normalize_theme(id);
    ui.set_theme_index(theme_index(&id));
    let (palette, _, _) = theme_entry(&id);
    let theme = ui.global::<crate::Theme>();
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
