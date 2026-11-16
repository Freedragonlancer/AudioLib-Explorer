// app/audiolib-desktop/src/controller/appearance.rs
//! Ordered appearance/language preferences use the existing background writer and shutdown drain.
use super::*;
use audiolib_core::appearance::{
    AppearancePreset, MAX_APPEARANCE_PRESETS, normalize_font_family, normalize_text_color,
};

impl Controller {
    fn discover_system_fonts(self: &Arc<Self>) {
        if self.system_fonts_started.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Some(ui) = self.ui.upgrade() {
            self.publish_system_fonts(&ui);
            ui.set_system_fonts_loading(true);
        }
        let c = self.clone();
        std::thread::spawn(move || {
            let fonts = crate::appearance::installed_fonts();
            *c.system_fonts.lock().unwrap() = fonts;
            let weak = c.ui.clone();
            let _ = weak.upgrade_in_event_loop(move |ui| {
                c.publish_system_fonts(&ui);
                ui.set_system_fonts_loading(false);
            });
        });
    }

    fn publish_system_fonts(&self, ui: &AppWindow) {
        let filter = ui.get_font_filter().to_lowercase();
        let current = ui.get_font_family().to_string();
        let mut fonts = vec![
            self.localized("系统默认字体", "System default font")
                .to_string(),
        ];
        fonts.extend(
            self.system_fonts
                .lock()
                .unwrap()
                .iter()
                .filter(|name| name.to_lowercase().contains(&filter))
                .cloned(),
        );
        if !current.is_empty() && !fonts[1..].contains(&current) {
            fonts.insert(1, current.clone());
        }
        let index = if current.is_empty() {
            0
        } else {
            fonts.iter().position(|name| *name == current).unwrap_or(0)
        };
        ui.set_system_fonts(crate::appearance::strings(fonts));
        ui.set_system_font_index(index as i32);
    }

    fn queue_appearance_save(&self, ui: &AppWindow) {
        crate::appearance::apply(ui, &self.appearance.lock().unwrap());
        self.publish_system_fonts(ui);
        *self.single_click_play_pending.lock().unwrap() = true;
        self.single_click_play_wake.notify_one();
    }

    pub(super) fn bind_appearance(self: &Arc<Self>, ui: &AppWindow) {
        let c = self.clone();
        ui.on_system_fonts_requested(move || c.discover_system_fonts());
        let c = self.clone();
        ui.on_font_filter_requested(move |_| {
            if let Some(ui) = c.ui.upgrade() {
                c.publish_system_fonts(&ui);
            }
        });
        let c = self.clone();
        ui.on_font_family_requested(move |name| {
            if !c.available() {
                return;
            }
            if let Some(ui) = c.ui.upgrade() {
                c.appearance.lock().unwrap().font_family = normalize_font_family(&name);
                c.queue_appearance_save(&ui);
            }
        });
        let c = self.clone();
        ui.on_text_color_selected(move || {
            if let Some(ui) = c.ui.upgrade() {
                crate::appearance::publish_color(&ui, &c.appearance.lock().unwrap());
            }
        });
        let c = self.clone();
        ui.on_text_color_requested(move |value| {
            if !c.available() {
                return;
            }
            let Some(ui) = c.ui.upgrade() else {
                return;
            };
            let Some(key) = crate::appearance::color_key(ui.get_text_module(), ui.get_text_state())
            else {
                return;
            };
            let value = value.trim();
            let color = normalize_text_color(value);
            if !value.is_empty() && color.is_none() {
                ui.set_text_color_error(true);
                return;
            }
            {
                let mut prefs = c.appearance.lock().unwrap();
                if let Some(color) = color {
                    prefs.text_colors.insert(key, color);
                } else {
                    prefs.text_colors.remove(&key);
                }
            }
            c.queue_appearance_save(&ui);
        });
        let c = self.clone();
        ui.on_text_colors_reset(move || c.select_text_palette(0));
        let c = self.clone();
        ui.on_text_palette_requested(move |index| c.select_text_palette(index));
        let c = self.clone();
        ui.on_appearance_preset_requested(move |action| c.edit_appearance_preset(action));
        let c = self.clone();
        ui.on_language_requested(move |chinese| c.select_language(if chinese { 2 } else { 1 }));
        let c = self.clone();
        ui.on_language_mode_requested(move |index| c.select_language(index));
    }

    fn select_text_palette(&self, index: i32) {
        if !self.available() {
            return;
        }
        if let Some(ui) = self.ui.upgrade()
            && let Some(colors) = crate::appearance::builtin_colors(index)
        {
            self.appearance.lock().unwrap().text_colors = colors;
            self.queue_appearance_save(&ui);
        }
    }

    fn edit_appearance_preset(&self, action: i32) {
        if !self.available() {
            return;
        }
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let selected = usize::try_from(ui.get_appearance_preset_index()).ok();
        let name = ui.get_appearance_preset_name().trim().to_owned();
        let mut prefs = self.appearance.lock().unwrap();
        if action != 1 && !selected.is_some_and(|index| index < prefs.presets.len()) {
            return;
        }
        if [1, 3].contains(&action)
            && (name.is_empty()
                || name.chars().count() > 64
                || name.chars().any(char::is_control)
                || prefs.presets.iter().enumerate().any(|(index, preset)| {
                    (action == 1 || Some(index) != selected)
                        && preset.name.to_lowercase() == name.to_lowercase()
                }))
        {
            ui.set_appearance_message(self.localized(
                "请输入不重复的预设名称（1–64 字）。",
                "Use a unique preset name (1–64 characters).",
            ));
            return;
        }
        if action == 1 && prefs.presets.len() >= MAX_APPEARANCE_PRESETS {
            ui.set_appearance_message(self.localized(
                "最多保存 64 个预设，请先删除一个。",
                "Up to 64 presets; delete one before saving another.",
            ));
            return;
        }
        let current = AppearancePreset {
            name: name.clone(),
            theme: crate::theme::theme_id(ui.get_theme_index()),
            font_family: prefs.font_family.clone(),
            ui_font_size: ui.get_ui_font_px() as u32,
            list_font_size: ui.get_list_font_px() as u32,
            text_colors: prefs.text_colors.clone(),
        };
        let mut load = None;
        match action {
            0 => {
                let preset = prefs.presets[selected.unwrap()].clone();
                prefs.font_family.clone_from(&preset.font_family);
                prefs.text_colors.clone_from(&preset.text_colors);
                load = Some(preset);
            }
            1 => {
                prefs.presets.push(current);
                ui.set_appearance_preset_index((prefs.presets.len() - 1) as i32);
            }
            2 => {
                let index = selected.unwrap();
                let name = prefs.presets[index].name.clone();
                prefs.presets[index] = AppearancePreset { name, ..current };
            }
            3 => prefs.presets[selected.unwrap()].name = name,
            4 => {
                prefs.presets.remove(selected.unwrap());
                ui.set_appearance_preset_index(-1);
                ui.set_appearance_preset_name("".into());
            }
            _ => return,
        }
        drop(prefs);
        if let Some(preset) = load {
            ui.invoke_theme_requested(crate::theme::theme_index(&preset.theme));
            ui.invoke_ui_font_requested(preset.ui_font_size as i32);
            ui.invoke_list_font_requested(preset.list_font_size as i32);
        }
        self.queue_appearance_save(&ui);
        let (cn, en) = match action {
            0 => ("已加载预设。", "Preset loaded."),
            1 => ("新预设已加入自动保存。", "New preset queued for saving."),
            2 => (
                "预设已更新，将自动保存。",
                "Preset updated; changes save automatically.",
            ),
            3 => (
                "预设已重命名，将自动保存。",
                "Preset renamed; changes save automatically.",
            ),
            _ => (
                "预设已删除，将自动保存。",
                "Preset deleted; changes save automatically.",
            ),
        };
        ui.set_appearance_message(self.localized(cn, en));
    }

    fn select_language(self: &Arc<Self>, index: i32) {
        if !self.available() {
            return;
        }
        let language = crate::appearance::language_id(index);
        let chinese = crate::appearance::chinese(language);
        *self.language.lock().unwrap() = language.into();
        self.chinese.store(chinese, Ordering::Release);
        *self.single_click_play_pending.lock().unwrap() = true;
        self.single_click_play_wake.notify_one();
        let c = self.clone();
        std::thread::spawn(move || c.reload_tree());
        if let Some(ui) = self.ui.upgrade() {
            ui.set_chinese(chinese);
            ui.set_language_index(crate::appearance::language_index(language));
            self.publish_system_fonts(&ui);
            self.publish_search_tasks(&ui);
            self.publish_ai_profiles(&ui);
            self.request_search(&ui, true, false);
            self.refresh_library_info();
            self.publish_data_source(&ui);
            self.refresh_generated_text(&ui);
            self.refresh_generated_source_text(&ui);
            if ui.get_library_visible() {
                self.load_history(0);
            }
        }
    }
}
