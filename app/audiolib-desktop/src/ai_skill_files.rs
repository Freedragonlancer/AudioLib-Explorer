// app/audiolib-desktop/src/ai_skill_files.rs
//! On-demand local viewing copies; the saved AI configuration remains authoritative.
use audiolib_ai::{AiError, Result, customization::Skill};
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
};

macro_rules! reference {
    ($path:literal) => {
        (
            $path,
            include_str!(concat!(
                "../../../.agents/skills/audio-keyword-search/",
                $path
            )),
        )
    };
}

const REFERENCES: &[(&str, &str)] = &[
    reference!("references/output-contract.md"),
    reference!("references/sources.md"),
    reference!("data/manifest.json"),
    reference!("data/keywords.jsonl"),
    reference!("data/ucs.jsonl"),
    reference!("data/recipes.jsonl"),
    reference!("scripts/lookup.ps1"),
    reference!("agents/openai.yaml"),
];

/// Prepare the displayed documents and their bundled references off the UI thread.
/// New revisions get separate folders; existing external edits are never overwritten.
pub(super) fn prepare(root: &Path, mut skill: Skill) -> Result<PathBuf> {
    if skill.id.is_empty() {
        // The built-in editor has no saved identity; keep its viewing copy repeatable.
        skill.id = "00000000-0000-0000-0000-000000000000".into();
    }
    skill.validate()?;
    let source = serde_json::to_vec(&(skill.clone(), REFERENCES)).map_err(|_| AiError::Storage)?;
    let mut key = DefaultHasher::new();
    source.hash(&mut key);
    // This is a disposable cache key, not a persistent ID or integrity guarantee.
    // Full source bytes below also guard against collisions or changed cache markers.
    let parent = root.join("ai/skill-files");
    ensure_unlinked(&parent)?;
    std::fs::create_dir_all(&parent).map_err(|_| AiError::Storage)?;
    let folder = parent.join(format!("skill-{:016x}", key.finish()));
    ensure_unlinked(&folder)?;
    if folder.try_exists().map_err(|_| AiError::Storage)? {
        let marker = folder.join(".audiolib-source.json");
        ensure_unlinked(&marker)?;
        if std::fs::metadata(&marker)
            .map_err(|_| AiError::Storage)?
            .len()
            != source.len() as u64
            || std::fs::read(&marker).map_err(|_| AiError::Storage)? != source
        {
            return Err(AiError::Storage);
        }
        // Preserve externally edited files, but never open redirected bundle paths.
        for name in audiolib_ai::customization::FILE_NAMES
            .into_iter()
            .chain(REFERENCES.iter().map(|(name, _)| *name))
            .chain(["skill.json"])
        {
            let path = folder.join(name);
            ensure_unlinked(&path)?;
            if !path.is_file() {
                return Err(AiError::Storage);
            }
        }
        return Ok(folder);
    }
    let staging = tempfile::tempdir_in(&parent).map_err(|_| AiError::Storage)?;
    let prepared = skill.export(staging.path())?;
    for (name, bytes) in REFERENCES {
        let path = prepared.join(name);
        std::fs::create_dir_all(path.parent().ok_or(AiError::Storage)?)
            .map_err(|_| AiError::Storage)?;
        std::fs::write(path, bytes).map_err(|_| AiError::Storage)?;
    }
    std::fs::write(prepared.join(".audiolib-source.json"), source).map_err(|_| AiError::Storage)?;
    std::fs::rename(prepared, &folder).map_err(|_| AiError::Storage)?;
    Ok(folder)
}

fn ensure_unlinked(path: &Path) -> Result<()> {
    for ancestor in path.ancestors() {
        match std::fs::symlink_metadata(ancestor) {
            Ok(metadata) => {
                #[cfg(windows)]
                let linked = {
                    use std::os::windows::fs::MetadataExt;
                    metadata.file_attributes() & 0x400 != 0
                };
                #[cfg(not(windows))]
                let linked = metadata.file_type().is_symlink();
                if linked {
                    return Err(AiError::Storage);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(AiError::Storage),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_skill_folder_contains_references_and_preserves_external_edits() {
        let root = tempfile::tempdir().unwrap();
        let mut skill = Skill::from_legacy("original role");
        skill.id.clear();
        let folder = prepare(root.path(), skill.clone()).unwrap();
        for (name, content) in REFERENCES {
            assert_eq!(
                std::fs::read_to_string(folder.join(name)).unwrap(),
                *content
            );
        }
        assert_eq!(
            std::fs::read_to_string(folder.join("SKILL.md")).unwrap(),
            skill.documents[0]
        );
        assert_eq!(
            std::fs::read_to_string(folder.join("references/search-techniques.md")).unwrap(),
            skill.documents[2]
        );
        let prompt = folder.join("prompts/search.md");
        std::fs::write(&prompt, "external edit").unwrap();
        assert_eq!(prepare(root.path(), skill.clone()).unwrap(), folder);
        assert_eq!(
            Skill::import(&folder).unwrap().documents[1],
            "external edit"
        );
        skill.documents[1] = "new editor revision".into();
        let changed = prepare(root.path(), skill).unwrap();
        assert_ne!(changed, folder);
        assert_eq!(std::fs::read_to_string(prompt).unwrap(), "external edit");
        assert_eq!(
            Skill::import(&changed).unwrap().documents[1],
            "new editor revision"
        );
        assert!(!root.path().join("ai/customization.json").exists());
        assert!(!root.path().join("ai/profiles.json").exists());
    }

    #[test]
    fn local_skill_folder_rejects_unrecognized_existing_folder_without_replacing_it() {
        let root = tempfile::tempdir().unwrap();
        let skill = Skill::from_legacy("role");
        let folder = prepare(root.path(), skill.clone()).unwrap();
        let marker = folder.join(".audiolib-source.json");
        std::fs::write(&marker, "unrecognized").unwrap();
        assert!(prepare(root.path(), skill).is_err());
        assert_eq!(std::fs::read_to_string(marker).unwrap(), "unrecognized");
    }
}
