// app/audiolib-desktop/src/ai_settings.rs
//! Skill editor drafts and search preferences; durable changes use the AI save barrier.
use super::*;
use audiolib_ai::customization::{Customization, Skill};

#[path = "ai_skill_files.rs"]
mod skill_files;

pub(super) enum SettingsJob {
    Save(Customization, bool),
    Import(PathBuf),
    Export(Skill, PathBuf),
    Browse(bool, usize),
    OpenFolder(Skill),
}

impl Controller {
    fn skill_from_ui(&self, ui: &AppWindow) -> Skill {
        Skill {
            id: ui.get_ai_skill_id().into(),
            name: ui.get_ai_skill_name().into(),
            documents: [
                ui.get_ai_skill_body().into(),
                ui.get_ai_skill_prompt().into(),
                ui.get_ai_skill_techniques().into(),
            ],
            enabled: [
                ui.get_ai_skill_body_enabled(),
                ui.get_ai_skill_prompt_enabled(),
                ui.get_ai_skill_techniques_enabled(),
            ],
        }
    }

    fn remember_skill_draft(&self, ui: &AppWindow) {
        if !ui.get_ai_skill_id().is_empty() && ui.get_ai_skill_dirty() {
            self.ai
                .skill_drafts
                .lock()
                .unwrap()
                .insert(ui.get_ai_skill_id().into(), self.skill_from_ui(ui));
        }
    }

    fn set_skill_editor(&self, ui: &AppWindow, skill: Skill, dirty: bool) {
        ui.set_ai_skill_id(skill.id.into());
        ui.set_ai_skill_name(skill.name.into());
        let [body, prompt, techniques] = skill.documents;
        ui.set_ai_skill_body(body.into());
        ui.set_ai_skill_prompt(prompt.into());
        ui.set_ai_skill_techniques(techniques.into());
        ui.set_ai_skill_body_enabled(skill.enabled[0]);
        ui.set_ai_skill_prompt_enabled(skill.enabled[1]);
        ui.set_ai_skill_techniques_enabled(skill.enabled[2]);
        ui.set_ai_skill_dirty(dirty);
        self.preview_skill(ui);
    }

    fn preview_skill(&self, ui: &AppWindow) {
        let guidance = if ui.get_ai_skill_id().is_empty() {
            Ok(ui.get_ai_instruction().to_string())
        } else {
            self.skill_from_ui(ui).guidance()
        };
        match guidance {
            Ok(text) => {
                ui.set_ai_skill_validation(self.localized(
                    &format!(
                        "合并后 {} / 16000 字符 · 保存并启用后生效",
                        text.chars().count()
                    ),
                    &format!(
                        "{} / 16000 characters combined · save and activate to use",
                        text.chars().count()
                    ),
                ));
                ui.set_ai_skill_preview(text.into());
            }
            Err(_) => {
                ui.set_ai_skill_validation(self.localized(
                    "请至少启用一个非空文件，合计不超过 16000 字符",
                    "Enable at least one nonempty file; combined limit: 16000 characters",
                ));
                ui.set_ai_skill_preview("".into());
            }
        }
    }

    pub(super) fn publish_ai_customization(&self, ui: &AppWindow, restore: bool) {
        let settings = self
            .ai
            .store
            .lock()
            .unwrap()
            .as_ref()
            .map(|s| (s.customization().clone(), s.has_customization()));
        let Some((settings, saved)) = settings else {
            return;
        };
        let mut names = vec![self.localized("内置 / 兼容预设", "Built-in / legacy preset")];
        names.extend(settings.skills.iter().map(|s| s.name.clone().into()));
        // Unsaved copies remain selectable across page, skill and language changes.
        let mut drafts: Vec<_> = self
            .ai
            .skill_drafts
            .lock()
            .unwrap()
            .values()
            .filter(|d| !settings.skills.iter().any(|s| s.id == d.id))
            .cloned()
            .collect();
        drafts.sort_by(|a, b| a.id.cmp(&b.id));
        names.extend(drafts.iter().map(|s| format!("{} *", s.name).into()));
        let ids: Vec<_> = std::iter::once(String::new())
            .chain(settings.skills.iter().map(|s| s.id.clone()))
            .chain(drafts.iter().map(|s| s.id.clone()))
            .collect();
        let selection = if restore {
            settings.active_skill.clone().unwrap_or_default()
        } else {
            ui.get_ai_skill_id().to_string()
        };
        let index = ids.iter().position(|id| *id == selection).unwrap_or(0);
        *self.ai.skill_ids.lock().unwrap() = ids;
        ui.set_ai_skills(ModelRc::new(VecModel::from(names)));
        ui.set_ai_skill_index(index as i32);
        ui.set_ai_custom_skill_active(settings.active().is_some());
        ui.set_ai_active_skill_name(
            settings
                .active()
                .map(|s| s.name.clone().into())
                .unwrap_or_else(|| self.localized("内置 / 兼容预设", "Built-in / legacy preset")),
        );
        if restore {
            if saved {
                ui.set_ai_layers(settings.layers);
                ui.set_ai_auto_search(settings.auto_search);
                ui.set_ai_auto_search_count(settings.auto_search_count as i32);
            }
            self.select_skill(ui, index as i32);
        } else {
            self.preview_skill(ui);
        }
    }

    fn select_skill(&self, ui: &AppWindow, index: i32) {
        let Some(id) = usize::try_from(index)
            .ok()
            .and_then(|i| self.ai.skill_ids.lock().unwrap().get(i).cloned())
        else {
            return;
        };
        if id.is_empty() {
            let mut built_in = Skill::from_legacy(&ui.get_ai_instruction());
            built_in.id.clear();
            self.set_skill_editor(ui, built_in, false);
        } else {
            let draft = self.ai.skill_drafts.lock().unwrap().get(&id).cloned();
            let dirty = draft.is_some();
            let skill = draft.or_else(|| {
                self.ai.store.lock().unwrap().as_ref().and_then(|s| {
                    s.customization()
                        .skills
                        .iter()
                        .find(|s| s.id == id)
                        .cloned()
                })
            });
            if let Some(skill) = skill {
                self.set_skill_editor(ui, skill, dirty);
            }
        }
    }

    pub(super) fn ai_settings_action(self: &Arc<Self>, ui: &AppWindow, kind: i32, index: i32) {
        if kind == 40 {
            self.preview_skill(ui);
            return;
        }
        if kind == 37 {
            self.ai_job(
                ui,
                Job::Settings(SettingsJob::Browse(
                    ui.get_chinese(),
                    crate::dialogs::owner_handle(ui.window()),
                )),
            );
            return;
        }
        if kind == 43 {
            self.ai_job(
                ui,
                Job::Settings(SettingsJob::OpenFolder(self.skill_from_ui(ui))),
            );
            return;
        }
        let Some(mut settings) = self
            .ai
            .store
            .lock()
            .unwrap()
            .as_ref()
            .map(|s| s.customization().clone())
        else {
            return;
        };
        match kind {
            30 => {
                self.remember_skill_draft(ui);
                self.select_skill(ui, index);
            }
            31 => {
                if self.ai.skill_ids.lock().unwrap().len() > 16 {
                    self.ai_failure(
                        ui,
                        self.localized(
                            "最多 16 个 Skill，请先删除不需要的配置",
                            "Up to 16 skills; remove an unused skill first",
                        ),
                    );
                    return;
                }
                self.remember_skill_draft(ui);
                let mut skill = Skill::from_legacy(&ui.get_ai_instruction());
                if !ui.get_ai_skill_id().is_empty() {
                    let current = self.skill_from_ui(ui);
                    skill.name = current.name;
                    skill.documents = current.documents;
                    skill.enabled = current.enabled;
                }
                skill.name = format!(
                    "{} {}",
                    skill.name.chars().take(65).collect::<String>(),
                    self.localized("副本", "copy")
                );
                self.ai
                    .skill_drafts
                    .lock()
                    .unwrap()
                    .insert(skill.id.clone(), skill.clone());
                self.set_skill_editor(ui, skill, true);
                self.publish_ai_customization(ui, false);
            }
            32 | 33 => {
                if ui.get_ai_skill_id().is_empty() {
                    if kind != 33 {
                        return;
                    }
                    settings.active_skill = None;
                } else {
                    let skill = self.skill_from_ui(ui);
                    if skill.validate().is_err() {
                        self.ai_failure(ui, self.localized("请检查 Skill 名称（1–80 字）、文件大小和合并提示词长度", "Check the skill name (1–80 characters), file sizes and combined prompt length"));
                        return;
                    }
                    if kind == 33 {
                        settings.active_skill = Some(skill.id.clone());
                    }
                    if let Some(saved) = settings.skills.iter_mut().find(|s| s.id == skill.id) {
                        *saved = skill;
                    } else {
                        settings.skills.push(skill);
                    }
                }
                self.ai_job(ui, Job::Settings(SettingsJob::Save(settings, true)));
            }
            34 => {
                let id = ui.get_ai_skill_id().to_string();
                if id.is_empty() {
                    return;
                }
                if settings.skills.iter().any(|s| s.id == id) {
                    settings.skills.retain(|s| s.id != id);
                    if settings.active_skill.as_deref() == Some(&id) {
                        settings.active_skill = None;
                    }
                    self.ai_job(ui, Job::Settings(SettingsJob::Save(settings, true)));
                } else {
                    self.ai.skill_drafts.lock().unwrap().remove(&id);
                    self.ai
                        .settings_save_errors
                        .lock()
                        .unwrap()
                        .remove(&SettingsSaveTarget::Skill(id.clone()));
                    ui.set_ai_skill_id("".into());
                    self.publish_ai_customization(ui, false);
                    self.select_skill(ui, 0);
                }
            }
            35 => {
                self.ai
                    .settings_save_errors
                    .lock()
                    .unwrap()
                    .remove(&SettingsSaveTarget::Skill(ui.get_ai_skill_id().to_string()));
                self.ai
                    .skill_drafts
                    .lock()
                    .unwrap()
                    .remove(ui.get_ai_skill_id().as_str());
                self.publish_ai_customization(ui, false);
                self.select_skill(ui, ui.get_ai_skill_index());
            }
            36 => {
                let built_in = Skill::from_legacy(DEFAULT_PROMPT);
                let mut skill = self.skill_from_ui(ui);
                if let Ok(file) = usize::try_from(ui.get_ai_skill_file())
                    && file < 3
                    && !skill.id.is_empty()
                {
                    skill.documents[file] = built_in.documents[file].clone();
                    self.set_skill_editor(ui, skill, true);
                }
            }
            39 => {
                settings.layers = ui.get_ai_layers();
                settings.auto_search = ui.get_ai_auto_search();
                settings.auto_search_count = ui.get_ai_auto_search_count().max(1) as usize;
                self.ai_job(ui, Job::Settings(SettingsJob::Save(settings, false)));
            }
            41 | 42 => {
                let path = PathBuf::from(ui.get_ai_skill_folder().as_str());
                if !path.is_absolute() {
                    self.ai_failure(
                        ui,
                        self.localized(
                            "请选择或填写完整文件夹路径",
                            "Choose or enter an absolute folder path",
                        ),
                    );
                    return;
                }
                self.ai_job(
                    ui,
                    Job::Settings(if kind == 41 {
                        SettingsJob::Import(path)
                    } else {
                        let mut skill = self.skill_from_ui(ui);
                        if skill.id.is_empty() {
                            skill = Skill::from_legacy(&ui.get_ai_instruction());
                        }
                        SettingsJob::Export(skill, path)
                    }),
                );
            }
            _ => {}
        }
    }

    pub(super) fn run_ai_settings(&self, job: SettingsJob) -> audiolib_ai::Result<Reply> {
        match job {
            SettingsJob::Browse(chinese, owner) => Ok(Reply::SkillFolder(
                crate::dialogs::choose_folder(chinese, owner).map_err(|_| AiError::Storage)?,
            )),
            SettingsJob::Export(skill, path) => Ok(Reply::SkillExported(skill.export(&path)?)),
            SettingsJob::OpenFolder(skill) => {
                let root = self.config.lock().unwrap().data_root();
                Ok(Reply::SkillLocalFolder(skill_files::prepare(&root, skill)?))
            }
            SettingsJob::Save(settings, editor_saved) => {
                let mut guard = self.ai.store.lock().unwrap();
                persist_settings(
                    guard.as_mut().ok_or(AiError::Storage)?,
                    settings,
                    None,
                    editor_saved,
                )
            }
            SettingsJob::Import(path) => {
                let skill = match Skill::import(&path) {
                    Ok(skill) => skill,
                    Err(error) => return Ok(Reply::SettingsRejected(error)),
                };
                let id = skill.id.clone();
                let mut guard = self.ai.store.lock().unwrap();
                let store = guard.as_mut().ok_or(AiError::Storage)?;
                let mut settings = store.customization().clone();
                settings.skills.push(skill);
                persist_settings(store, settings, Some(id), true)
            }
        }
    }

    pub(super) fn finish_ai_settings(
        &self,
        ui: &AppWindow,
        imported: Option<String>,
        editor_saved: bool,
    ) {
        if editor_saved && imported.is_none() {
            self.ai
                .skill_drafts
                .lock()
                .unwrap()
                .remove(ui.get_ai_skill_id().as_str());
        }
        if let Some(id) = imported {
            self.remember_skill_draft(ui);
            ui.set_ai_skill_id(id.into());
        }
        self.publish_ai_customization(ui, false);
        if editor_saved {
            self.select_skill(ui, ui.get_ai_skill_index());
        }
        ui.set_ai_status(self.localized(
            "AI 设置已保存；搜索使用已启用的 Skill",
            "AI settings saved; generation uses the active skill",
        ));
    }
}

fn persist_settings(
    store: &mut ProfileStore,
    settings: Customization,
    imported: Option<String>,
    editor_saved: bool,
) -> audiolib_ai::Result<Reply> {
    match store.save_customization(settings) {
        Ok(()) => Ok(Reply::SettingsSaved(imported, editor_saved)),
        Err(AiError::Invalid) => Ok(Reply::SettingsRejected(AiError::Invalid)),
        Err(error) => Err(error),
    }
}
