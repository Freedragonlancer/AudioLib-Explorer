// app/audiolib-desktop/src/appearance.rs
//! UI-thread projection of portable appearance preferences and platform language/font discovery.
use crate::{AppWindow, Theme};
use audiolib_core::appearance::{AppearancePreferences, TEXT_MODULES, TEXT_STATES};
use slint::{Color, ComponentHandle, ModelRc, SharedString, VecModel};

pub(crate) fn chinese(language: &str) -> bool {
    audiolib_core::appearance::uses_chinese(language, sys_locale::get_locale().as_deref())
}

pub(crate) fn language_index(language: &str) -> i32 {
    match language {
        "en" => 1,
        "cn" => 2,
        _ => 0,
    }
}

pub(crate) fn language_id(index: i32) -> &'static str {
    match index {
        1 => "en",
        2 => "cn",
        _ => "system",
    }
}

pub(crate) fn strings(values: impl IntoIterator<Item = String>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        values
            .into_iter()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    ))
}

pub(crate) fn color_key(module: i32, state: i32) -> Option<String> {
    Some(format!(
        "{}.{}",
        TEXT_MODULES.get(usize::try_from(module).ok()?)?,
        TEXT_STATES.get(usize::try_from(state).ok()?)?
    ))
}

/// Text overrides never modify the base theme, surfaces, artwork or spectral palette.
pub(crate) fn apply(ui: &AppWindow, prefs: &AppearancePreferences) {
    let colors = TEXT_MODULES
        .iter()
        .flat_map(|module| {
            TEXT_STATES.iter().map(move |state| {
                prefs
                    .text_colors
                    .get(&format!("{module}.{state}"))
                    .and_then(|hex| u32::from_str_radix(hex.trim_start_matches('#'), 16).ok())
                    .map_or(Color::default(), |rgb| {
                        Color::from_argb_encoded(0xff00_0000 | rgb)
                    })
            })
        })
        .collect::<Vec<_>>();
    ui.global::<Theme>()
        .set_text_colors(ModelRc::new(VecModel::from(colors)));
    ui.global::<Theme>()
        .set_font_family(prefs.font_family.clone().into());
    ui.set_font_family(prefs.font_family.clone().into());
    ui.set_appearance_preset_names(strings(prefs.presets.iter().map(|p| p.name.clone())));
    ui.set_appearance_customized(!prefs.text_colors.is_empty());
    publish_color(ui, prefs);
    crate::detail_presentation::relayout(ui);
}

pub(crate) fn publish_color(ui: &AppWindow, prefs: &AppearancePreferences) {
    let key = color_key(ui.get_text_module(), ui.get_text_state());
    let value = key.as_ref().and_then(|key| prefs.text_colors.get(key));
    ui.set_text_color_value(value.cloned().unwrap_or_default().into());
    ui.set_text_color_error(false);
}

/// Enumerate real installed font families off the event loop; keep locale aliases.
pub(crate) fn installed_fonts() -> Vec<String> {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    let mut families = db
        .faces()
        .flat_map(|face| face.families.iter().map(|(name, _)| name.clone()))
        .filter(|name| !name.starts_with('@'))
        .collect::<Vec<_>>();
    families.sort_by_key(|name| name.to_lowercase());
    families.dedup_by(|a, b| a.to_lowercase() == b.to_lowercase());
    families
}

/// Legacy text palette fixtures cover persisted overrides independently of full themes.
#[cfg(test)]
pub(crate) fn builtin_colors(index: i32) -> Option<std::collections::BTreeMap<String, String>> {
    let colors = match index {
        0 => return Some(Default::default()),
        1 => [
            "#E0E8E5", "#B4C4BF", "#98AAA4", "#788D85", "#D0EFE3", "#C2E6D7", "#A7D0BC", "#A9D6AD",
            "#DEC698", "#E6A6A3", "#20342B", "#9DBDAD", "#E0B793", "#E3CEA2", "#FFFFFF",
        ],
        2 => [
            "#E1E8F0", "#B6C5D5", "#99ABBE", "#7B8FA6", "#D8EDFF", "#C6E1FA", "#A7CDED", "#A4D4C4",
            "#E0C598", "#E5A5B2", "#1E2D3E", "#A1BAD4", "#DFBCA0", "#E1CFAB", "#FFFFFF",
        ],
        3 => [
            "#EEE6DB", "#CCBFB0", "#B4A593", "#938573", "#FFE7C4", "#F3D9B3", "#DEBE91", "#B7D0A4",
            "#E8C282", "#E8A59C", "#352B20", "#C8BA9E", "#E0AC93", "#EDD299", "#FFFFFF",
        ],
        _ => return None,
    };
    Some(
        TEXT_STATES
            .iter()
            .zip(colors)
            .map(|(state, color)| (format!("general.{state}"), color.into()))
            .collect(),
    )
}
