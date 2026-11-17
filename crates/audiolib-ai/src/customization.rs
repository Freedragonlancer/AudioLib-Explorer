// crates/audiolib-ai/src/customization.rs
//! User-authored skill documents. Text only: no scripts, links or tools are executed.
use crate::{AiError, Result, config::validate_guidance};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::Path,
};

/// Fixed bundle paths; imported content cannot select arbitrary files.
pub const FILE_NAMES: [&str; 3] = [
    "SKILL.md",
    "prompts/search.md",
    "references/search-techniques.md",
];
/// Embedded authoring references; the legacy search path does not send these by default.
pub const BUILTIN_SKILL: &str =
    include_str!("../../../.agents/skills/audio-keyword-search/SKILL.md");
/// Sound-design guidance supplied with a new custom skill.
pub const BUILTIN_TECHNIQUES: &str =
    include_str!("../../../.agents/skills/audio-keyword-search/references/search-techniques.md");
const MAX_DOCUMENT_BYTES: u64 = 64 * 1024;
const MAX_STORE_BYTES: u64 = 2 * 1024 * 1024;

/// A named, portable set of Markdown documents, without credentials or endpoints.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Skill {
    pub id: String,
    pub name: String,
    pub documents: [String; 3],
    pub enabled: [bool; 3],
}

impl Skill {
    /// Seed an editable copy with the current legacy prompt, preserving its wording.
    pub fn from_legacy(prompt: &str) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: "audio-keyword-search".into(),
            documents: [
                BUILTIN_SKILL.into(),
                prompt.into(),
                BUILTIN_TECHNIQUES.into(),
            ],
            enabled: [true; 3],
        }
    }

    /// Compose the exact optional guidance passed before the fixed response contract.
    pub fn guidance(&self) -> Result<String> {
        if !self
            .documents
            .iter()
            .zip(self.enabled)
            .any(|(text, enabled)| enabled && !text.trim().is_empty())
        {
            return Err(AiError::Invalid);
        }
        let text = self
            .documents
            .iter()
            .zip(self.enabled)
            .zip(FILE_NAMES)
            .filter(|((_, enabled), _)| *enabled)
            .map(|((text, _), name)| format!("## {name}\n{text}"))
            .collect::<Vec<_>>()
            .join("\n\n");
        validate_guidance(&text)?;
        Ok(text)
    }

    /// Validate every document, including disabled files, before replacing saved data.
    pub fn validate(&self) -> Result<()> {
        if uuid::Uuid::parse_str(&self.id).is_err()
            || self.name.trim().is_empty()
            || self.name.chars().count() > 80
            || self.name.chars().any(char::is_control)
            || self
                .documents
                .iter()
                .any(|s| s.len() as u64 > MAX_DOCUMENT_BYTES || s.contains('\0'))
        {
            return Err(AiError::Invalid);
        }
        self.guidance()?;
        Ok(())
    }

    /// Import only the three named UTF-8 files. Missing, linked or oversized files fail.
    /// A fresh identity prevents an imported bundle from overwriting another skill.
    pub fn import(directory: &Path) -> Result<Self> {
        if is_link(&std::fs::symlink_metadata(directory).map_err(|_| AiError::Storage)?) {
            return Err(AiError::Storage);
        }
        let mut skill = Self::from_legacy(crate::config::DEFAULT_PROMPT);
        let metadata = directory.join("skill.json");
        if metadata.try_exists().map_err(|_| AiError::Storage)? {
            let imported: Self = serde_json::from_slice(&read_bounded(&metadata, MAX_STORE_BYTES)?)
                .map_err(|_| AiError::Invalid)?;
            skill.name = imported.name;
            skill.enabled = imported.enabled;
        }
        for (slot, name) in skill.documents.iter_mut().zip(FILE_NAMES) {
            let path = directory.join(name);
            if path
                .ancestors()
                .take_while(|p| *p != directory)
                .any(|p| std::fs::symlink_metadata(p).map_or(true, |m| is_link(&m)))
            {
                return Err(AiError::Storage);
            }
            *slot = String::from_utf8(read_bounded(&path, MAX_DOCUMENT_BYTES)?)
                .map_err(|_| AiError::Invalid)?;
        }
        skill.validate()?;
        Ok(skill)
    }

    /// Export an independent bundle into a new child folder, never overwrite files.
    pub fn export(&self, parent: &Path) -> Result<std::path::PathBuf> {
        self.validate()?;
        let directory = parent.join(format!("audiolib-skill-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).map_err(|_| AiError::Storage)?;
        for (text, name) in self.documents.iter().zip(FILE_NAMES) {
            let path = directory.join(name);
            std::fs::create_dir_all(path.parent().ok_or(AiError::Storage)?)
                .map_err(|_| AiError::Storage)?;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .map_err(|_| AiError::Storage)?;
            file.write_all(text.as_bytes())
                .map_err(|_| AiError::Storage)?;
            file.sync_all().map_err(|_| AiError::Storage)?;
        }
        // Enabled flags travel with the bundle, but provider credentials never do.
        atomic_write(
            &directory.join("skill.json"),
            &serde_json::to_vec_pretty(self).map_err(|_| AiError::Storage)?,
        )?;
        Ok(directory)
    }
}

/// Separate settings document so old provider files remain readable by older clients.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Customization {
    pub version: u32,
    pub skills: Vec<Skill>,
    pub active_skill: Option<String>,
    pub layers: bool,
    pub auto_search: bool,
    pub auto_search_count: usize,
}

impl Default for Customization {
    fn default() -> Self {
        Self {
            version: 1,
            skills: vec![],
            active_skill: None,
            layers: true,
            auto_search: false,
            auto_search_count: 3,
        }
    }
}

impl Customization {
    /// The saved active skill; None preserves the original prompt preset behavior.
    pub fn active(&self) -> Option<&Skill> {
        self.skills
            .iter()
            .find(|s| Some(&s.id) == self.active_skill.as_ref())
    }
    /// Validate the entire transaction before persisting any edits.
    pub fn validate(&self) -> Result<()> {
        if self.version != 1
            || self.skills.len() > 16
            || !(1..=3).contains(&self.auto_search_count)
            || (self.active_skill.is_some() && self.active().is_none())
        {
            return Err(AiError::Invalid);
        }
        let mut ids = std::collections::HashSet::new();
        for skill in &self.skills {
            skill.validate()?;
            if !ids.insert(&skill.id) {
                return Err(AiError::Invalid);
            }
        }
        Ok(())
    }
    pub(crate) fn load(root: &Path) -> Result<Self> {
        let path = root.join("ai/customization.json");
        if !path.try_exists().map_err(|_| AiError::Storage)? {
            return Ok(Self::default());
        }
        let value: Self = serde_json::from_slice(&read_bounded(&path, MAX_STORE_BYTES)?)
            .map_err(|_| AiError::Storage)?;
        value.validate().map_err(|_| AiError::Storage)?;
        Ok(value)
    }
    pub(crate) fn save(&self, root: &Path) -> Result<()> {
        self.validate()?;
        let bytes = serde_json::to_vec_pretty(self).map_err(|_| AiError::Storage)?;
        if bytes.len() as u64 > MAX_STORE_BYTES {
            return Err(AiError::Invalid);
        }
        atomic_write(&root.join("ai/customization.json"), &bytes)
    }
}

fn is_link(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn read_bounded(path: &Path, maximum: u64) -> Result<Vec<u8>> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| AiError::Storage)?;
    if !metadata.is_file() || is_link(&metadata) || metadata.len() > maximum {
        return Err(AiError::Storage);
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|_| AiError::Storage)?
        .take(maximum + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| AiError::Storage)?;
    if bytes.len() as u64 > maximum {
        return Err(AiError::Storage);
    }
    Ok(bytes)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or(AiError::Storage)?;
    std::fs::create_dir_all(parent).map_err(|_| AiError::Storage)?;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|_| AiError::Storage)?;
    file.write_all(bytes).map_err(|_| AiError::Storage)?;
    file.as_file().sync_all().map_err(|_| AiError::Storage)?;
    file.persist(path).map_err(|_| AiError::Storage)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{ApiKey, Profile},
        storage::ProfileStore,
    };

    #[test]
    fn legacy_profiles_and_prompt_survive_custom_skill_activation_and_removal() {
        let root = tempfile::tempdir().unwrap();
        let mut store = ProfileStore::open(root.path()).unwrap();
        assert!(!store.has_customization());
        let profile = Profile {
            model: "owned-model".into(),
            ..Default::default()
        };
        store
            .save(profile, Some(ApiKey::new("owned-key".into()).unwrap()))
            .unwrap();
        store.save_prompt(true, "保留旧提示词").unwrap();
        let legacy = std::fs::read(root.path().join("ai/profiles.json")).unwrap();
        let mut next = store.customization().clone();
        assert!(next.active().is_none());
        assert_eq!(store.generation_guidance("旧草稿").unwrap(), "旧草稿");
        let mut skill = Skill::from_legacy(store.prompt().effective());
        skill.documents[1] = "优先机械材质".into();
        skill.enabled[2] = false;
        next.active_skill = Some(skill.id.clone());
        next.skills.push(skill.clone());
        next.auto_search_count = 1;
        store.save_customization(next.clone()).unwrap();
        let mut reopened = ProfileStore::open(root.path()).unwrap();
        assert!(reopened.has_customization());
        assert_eq!(reopened.customization(), &next);
        assert_eq!(
            reopened.generation_guidance("ignored draft").unwrap(),
            skill.guidance().unwrap()
        );
        assert!(
            !skill
                .guidance()
                .unwrap()
                .contains("## references/search-techniques.md")
        );
        if audiolib_windows::user_secret_protection_available() {
            assert_eq!(
                reopened.key(reopened.selected().unwrap()).unwrap().expose(),
                "owned-key"
            );
        }
        next.active_skill = None;
        next.skills.clear();
        reopened.save_customization(next).unwrap();
        assert_eq!(reopened.prompt().effective(), "保留旧提示词");
        assert_eq!(
            std::fs::read(root.path().join("ai/profiles.json")).unwrap(),
            legacy
        );
    }

    #[test]
    fn invalid_edits_and_failed_writes_preserve_last_saved_skill() {
        let root = tempfile::tempdir().unwrap();
        let mut store = ProfileStore::open(root.path()).unwrap();
        let skill = Skill::from_legacy("role");
        let valid = Customization {
            active_skill: Some(skill.id.clone()),
            skills: vec![skill],
            ..Default::default()
        };
        store.save_customization(valid.clone()).unwrap();
        let path = root.path().join("ai/customization.json");
        let before = std::fs::read(&path).unwrap();
        for case in 0..6 {
            let mut invalid = valid.clone();
            match case {
                0 => invalid.skills[0].documents[1] = "声".repeat(16001),
                1 => invalid.skills[0].enabled = [false; 3],
                2 => invalid.skills[0].name = "".into(),
                3 => invalid.active_skill = Some("unknown".into()),
                4 => invalid.skills.push(invalid.skills[0].clone()),
                _ => invalid.auto_search_count = 4,
            }
            assert!(store.save_customization(invalid).is_err());
            assert_eq!(store.customization(), &valid);
            assert_eq!(std::fs::read(&path).unwrap(), before);
        }
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(store.save_customization(Customization::default()).is_err());
        assert_eq!(store.customization(), &valid);
    }

    #[test]
    fn portable_markdown_round_trips_with_new_identity_and_external_edits() {
        let root = tempfile::tempdir().unwrap();
        let mut skill = Skill::from_legacy("my role");
        skill.name = "机械声音".into();
        skill.enabled[2] = false;
        let folder = skill.export(root.path()).unwrap();
        std::fs::write(folder.join(FILE_NAMES[1]), "externally edited role").unwrap();
        let imported = Skill::import(&folder).unwrap();
        assert_ne!(imported.id, skill.id);
        assert_eq!(imported.name, skill.name);
        assert_eq!(imported.enabled, skill.enabled);
        assert_eq!(imported.documents[1], "externally edited role");
        assert_ne!(skill.export(root.path()).unwrap(), folder);
        std::fs::write(
            folder.join(FILE_NAMES[0]),
            vec![b'x'; MAX_DOCUMENT_BYTES as usize + 1],
        )
        .unwrap();
        assert!(Skill::import(&folder).is_err());
    }

    #[test]
    fn corrupt_customization_is_not_silently_replaced_and_guidance_has_a_separate_bound() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("ai/customization.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = br#"{"version":999,"skills":[]}"#;
        std::fs::write(&path, original).unwrap();
        assert!(ProfileStore::open(root.path()).is_err());
        assert_eq!(std::fs::read(path).unwrap(), original);
        assert!(crate::config::validate_instruction(&"声".repeat(2001)).is_err());
        assert!(validate_guidance(&"声".repeat(16000)).is_ok());
        assert!(validate_guidance(&"声".repeat(16001)).is_err());
        let mut skill = Skill::from_legacy("role");
        skill.documents = [" ".into(), "".into(), "\n".into()];
        assert!(skill.guidance().is_err());
    }
}
