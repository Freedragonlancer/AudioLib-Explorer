// crates/audiolib-core/src/appearance.rs
//! Portable, bounded typography and text-palette preferences. No font files or library data.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Stable module identifiers; their order is shared with the desktop palette editor.
pub const TEXT_MODULES: [&str; 8] = [
    "general",
    "navigation",
    "results",
    "metadata",
    "player",
    "dialogs",
    "ai",
    "status",
];
/// Text roles, including interaction and semantic states. Surfaces and spectral data are separate.
pub const TEXT_STATES: [&str; 15] = [
    "normal",
    "secondary",
    "muted",
    "disabled",
    "hover",
    "selected",
    "active",
    "success",
    "warning",
    "error",
    "on_accent",
    "played",
    "missing",
    "favorite",
    "pressed",
];
/// Maximum number of user presets retained in preferences.
pub const MAX_APPEARANCE_PRESETS: usize = 64;

/// A complete user preset, independent of the currently selected library.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppearancePreset {
    pub name: String,
    pub theme: String,
    /// Empty selects the platform's default UI font.
    pub font_family: String,
    pub ui_font_size: u32,
    pub list_font_size: u32,
    /// `module.state` → opaque `#RRGGBB`. Missing roles inherit general, then the theme.
    pub text_colors: BTreeMap<String, String>,
}

impl Default for AppearancePreset {
    fn default() -> Self {
        Self {
            name: String::new(),
            theme: crate::config::DEFAULT_THEME.into(),
            font_family: String::new(),
            ui_font_size: 13,
            list_font_size: 13,
            text_colors: BTreeMap::new(),
        }
    }
}

/// Current font and text overrides plus named user presets.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppearancePreferences {
    pub font_family: String,
    pub text_colors: BTreeMap<String, String>,
    pub presets: Vec<AppearancePreset>,
}

/// Accept only opaque RGB; reject incomplete edits without changing the active color.
pub fn normalize_text_color(value: &str) -> Option<String> {
    let value = value.trim();
    (value.len() == 7
        && value.starts_with('#')
        && value[1..].bytes().all(|c| c.is_ascii_hexdigit()))
    .then(|| value.to_ascii_uppercase())
}

/// Font names are portable strings, never paths or arbitrary unbounded data.
pub fn normalize_font_family(value: &str) -> String {
    value
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(128)
        .collect()
}

fn normalize_colors(colors: &mut BTreeMap<String, String>) {
    colors.retain(|key, value| {
        let valid_key = key.split_once('.').is_some_and(|(module, state)| {
            TEXT_MODULES.contains(&module) && TEXT_STATES.contains(&state)
        });
        if valid_key && let Some(color) = normalize_text_color(value) {
            *value = color;
            true
        } else {
            false
        }
    });
}

impl AppearancePreferences {
    /// Normalize independently of unrelated preferences so a damaged palette is recoverable.
    pub fn normalized(mut self) -> Self {
        self.font_family = normalize_font_family(&self.font_family);
        normalize_colors(&mut self.text_colors);
        let mut names = std::collections::BTreeSet::new();
        self.presets.retain_mut(|preset| {
            preset.name = preset
                .name
                .trim()
                .chars()
                .filter(|c| !c.is_control())
                .take(64)
                .collect();
            if preset.name.is_empty() || !names.insert(preset.name.to_lowercase()) {
                return false;
            }
            preset.theme = crate::config::normalize_theme(&preset.theme);
            preset.font_family = normalize_font_family(&preset.font_family);
            preset.ui_font_size = crate::config::clamp_font_size(i64::from(preset.ui_font_size));
            preset.list_font_size =
                crate::config::clamp_font_size(i64::from(preset.list_font_size));
            normalize_colors(&mut preset.text_colors);
            true
        });
        self.presets.truncate(MAX_APPEARANCE_PRESETS);
        self
    }
}

/// Tolerate invalid appearance fields without losing audio, paths or other preferences.
pub fn deserialize_appearance<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<AppearancePreferences, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    let font_family = value
        .get("font_family")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .into();
    let text_colors = value
        .get("text_colors")
        .and_then(|v| v.as_object())
        .map(|colors| {
            colors
                .iter()
                .filter_map(|(key, value)| value.as_str().map(|v| (key.clone(), v.into())))
                .collect()
        })
        .unwrap_or_default();
    let presets = value
        .get("presets")
        .and_then(|v| v.as_array())
        .map(|entries| {
            entries
                .iter()
                .filter_map(|v| serde_json::from_value(v.clone()).ok())
                .take(MAX_APPEARANCE_PRESETS)
                .collect()
        })
        .unwrap_or_default();
    Ok(AppearancePreferences {
        font_family,
        text_colors,
        presets,
    }
    .normalized())
}

/// Resolve an explicit language or a platform locale. Unsupported locales use English.
pub fn uses_chinese(language: &str, system_locale: Option<&str>) -> bool {
    match language {
        "cn" => true,
        "en" => false,
        _ => system_locale.is_some_and(|locale| {
            locale
                .split(['-', '_', '.'])
                .next()
                .is_some_and(|s| s.eq_ignore_ascii_case("zh"))
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_resolution_keeps_explicit_choices_and_handles_system_variants() {
        for locale in ["zh-CN", "zh_TW.UTF-8", "ZH-Hans", "zh-HK"] {
            assert!(uses_chinese("system", Some(locale)));
            assert!(!uses_chinese("en", Some(locale)));
        }
        for locale in [None, Some("en-US"), Some("ja-JP"), Some("zhang")] {
            assert!(!uses_chinese("system", locale));
            assert!(uses_chinese("cn", locale));
        }
    }

    #[test]
    fn colors_and_presets_are_bounded_and_invalid_fields_are_isolated() {
        assert_eq!(
            normalize_text_color(" #a0B1c2 ").as_deref(),
            Some("#A0B1C2")
        );
        for value in ["red", "#fff", "#00000000", "#你好", "#gggggg"] {
            assert!(normalize_text_color(value).is_none());
        }
        let mut prefs = AppearancePreferences::default();
        prefs
            .text_colors
            .insert("results.missing".into(), "#aabbcc".into());
        prefs
            .text_colors
            .insert("results.other".into(), "#123456".into());
        prefs.presets = vec![
            AppearancePreset {
                name: " Test ".into(),
                ui_font_size: 999,
                ..Default::default()
            },
            AppearancePreset {
                name: "test".into(),
                ..Default::default()
            },
        ];
        let prefs = prefs.normalized();
        assert_eq!(prefs.text_colors.len(), 1);
        assert_eq!(prefs.presets.len(), 1);
        assert_eq!(prefs.presets[0].ui_font_size, 24);
    }
}
