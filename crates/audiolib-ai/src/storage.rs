// crates/audiolib-ai/src/storage.rs
//! Atomic AI profiles, encrypted Windows credentials and informational usage history.
use crate::{
    AiError, Result,
    config::{ApiKey, Profile, PromptSettings, Protocol, validate_instruction},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
};

/// Persisted app-local usage, independent from a provider's account-wide allowance.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Usage {
    pub month: String,
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    version: u32,
    profiles: Vec<Profile>,
    selected: Option<String>,
    protected_keys: BTreeMap<String, Vec<u8>>,
    usage: BTreeMap<String, Usage>,
    #[serde(default)]
    prompt: PromptSettings,
}

/// One serialized owner per desktop instance, protected by its existing data-dir lease.
/// A corrupt/future document is never replaced with an empty fallback.
pub struct ProfileStore {
    path: PathBuf,
    document: Document,
    session_keys: BTreeMap<String, ApiKey>,
}

impl ProfileStore {
    /// Open only the supplied owned data directory, never a library or legacy config.
    pub fn open(root: &Path) -> Result<Self> {
        let path = root.join("ai/profiles.json");
        let document = match std::fs::read(&path) {
            Ok(bytes) if bytes.len() <= 1024 * 1024 => {
                let d: Document = serde_json::from_slice(&bytes).map_err(|_| AiError::Storage)?;
                if d.version != 1
                    || validate_instruction(&d.prompt.custom).is_err()
                    || d.profiles.len() > 16
                    || d.profiles.iter().any(|p| p.validate().is_err())
                {
                    return Err(AiError::Storage);
                }
                let ids: std::collections::HashSet<_> = d.profiles.iter().map(|p| &p.id).collect();
                if ids.len() != d.profiles.len()
                    || d.selected.as_ref().is_some_and(|s| !ids.contains(s))
                {
                    return Err(AiError::Storage);
                }
                d
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Document {
                version: 1,
                ..Default::default()
            },
            _ => return Err(AiError::Storage),
        };
        Ok(Self {
            path,
            document,
            session_keys: BTreeMap::new(),
        })
    }
    /// Saved profiles, without credentials.
    pub fn profiles(&self) -> &[Profile] {
        &self.document.profiles
    }
    /// Profile chosen at the last successful save.
    pub fn selected(&self) -> Option<&Profile> {
        self.document
            .profiles
            .iter()
            .find(|p| Some(&p.id) == self.document.selected.as_ref())
    }
    /// Suggest a unique vendor/sequence name without renaming any existing profile.
    pub fn next_profile_name(&self, protocol: Protocol, exclude_id: &str) -> String {
        (1..=self.document.profiles.len() + 1)
            .map(|number| format!("{} {number}", protocol.display_name()))
            .find(|name| {
                !self
                    .document
                    .profiles
                    .iter()
                    .any(|p| p.id != exclude_id && p.name.eq_ignore_ascii_case(name))
            })
            .expect("a finite profile list always leaves a free sequence name")
    }
    /// Whether credential storage is populated; never decrypts or returns secret bytes.
    /// This is a presence indicator, not proof of successful provider authentication.
    pub fn has_key(&self, profile: &Profile) -> bool {
        self.session_keys
            .get(&profile.credential_ref)
            .is_some_and(|key| !key.is_empty())
            || self
                .document
                .protected_keys
                .contains_key(&profile.credential_ref)
    }
    /// Numeric app-local usage. Unknown provider balance stays unknown in the UI.
    pub fn usage(&self, id: &str) -> Usage {
        self.document.usage.get(id).cloned().unwrap_or_default()
    }

    /// Saved prompt settings, including the retained custom preset.
    pub fn prompt(&self) -> &PromptSettings {
        &self.document.prompt
    }

    /// Atomically save the selected prompt; never overwrite the custom text when
    /// returning to the built-in default. Failure retains the previous settings.
    pub fn save_prompt(&mut self, use_custom: bool, text: &str) -> Result<()> {
        validate_instruction(text)?;
        let mut next = self.copy_document();
        next.prompt.use_custom = use_custom;
        if use_custom {
            next.prompt.custom = text.into();
        }
        self.persist_document(&next)?;
        self.document = next;
        Ok(())
    }
    /// Resolve a key without exposing it through Debug or profile serialization.
    pub fn key(&self, p: &Profile) -> Result<ApiKey> {
        if let Some(key) = self.session_keys.get(&p.credential_ref) {
            return Ok(key.clone());
        }
        let Some(bytes) = self.document.protected_keys.get(&p.credential_ref) else {
            return Ok(ApiKey::default());
        };
        if !audiolib_windows::user_secret_protection_available() {
            return Err(AiError::SessionOnly);
        }
        let plain = audiolib_windows::unprotect_user_secret(bytes).map_err(|_| AiError::Storage)?;
        ApiKey::new(String::from_utf8(plain).map_err(|_| AiError::Storage)?)
    }
    /// Resolve an unsaved discovery draft without storing it. A stored key is only
    /// reused for the same saved profile, protocol and endpoint; supplied keys win.
    pub fn discovery_key(&self, draft: &Profile, supplied: Option<ApiKey>) -> Result<ApiKey> {
        draft.validate_discovery_request()?;
        if let Some(key) = supplied {
            return Ok(key);
        }
        let Some(saved) = self.document.profiles.iter().find(|p| p.id == draft.id) else {
            return Ok(ApiKey::default());
        };
        if saved.base_url != draft.base_url || saved.protocol != draft.protocol {
            return Err(AiError::Invalid);
        }
        self.key(saved)
    }
    fn persist_document(&self, document: &Document) -> Result<()> {
        let parent = self.path.parent().ok_or(AiError::Storage)?;
        std::fs::create_dir_all(parent).map_err(|_| AiError::Storage)?;
        let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|_| AiError::Storage)?;
        file.write_all(&serde_json::to_vec_pretty(document).map_err(|_| AiError::Storage)?)
            .map_err(|_| AiError::Storage)?;
        file.as_file().sync_all().map_err(|_| AiError::Storage)?;
        file.persist(&self.path).map_err(|_| AiError::Storage)?;
        Ok(())
    }
    fn copy_document(&self) -> Document {
        Document {
            version: 1,
            profiles: self.document.profiles.clone(),
            selected: self.document.selected.clone(),
            protected_keys: self.document.protected_keys.clone(),
            usage: self.document.usage.clone(),
            prompt: self.document.prompt.clone(),
        }
    }
    /// Atomically store config and protected credentials together. `None` keeps a key;
    /// a supplied empty key removes it. Non-Windows keys stay in this session only.
    pub fn save(&mut self, mut profile: Profile, key: Option<ApiKey>) -> Result<()> {
        profile.validate()?;
        let mut next = self.copy_document();
        let old = next.profiles.iter().find(|p| p.id == profile.id).cloned();
        if old.is_none() && next.profiles.len() >= 16 {
            return Err(AiError::Invalid);
        }
        if let Some(old) = &old {
            if key.is_none()
                && (old.base_url != profile.base_url || old.protocol != profile.protocol)
            {
                return Err(AiError::Invalid);
            }
            profile.credential_ref = old.credential_ref.clone();
        }
        if let Some(key) = &key {
            profile.credential_ref = uuid::Uuid::new_v4().to_string();
            if !key.is_empty() && audiolib_windows::user_secret_protection_available() {
                next.protected_keys.insert(
                    profile.credential_ref.clone(),
                    audiolib_windows::protect_user_secret(key.expose().as_bytes())
                        .map_err(|_| AiError::Storage)?,
                );
            }
        }
        next.profiles.retain(|p| p.id != profile.id);
        next.profiles.push(profile.clone());
        next.selected = Some(profile.id.clone());
        next.protected_keys
            .retain(|id, _| next.profiles.iter().any(|p| &p.credential_ref == id));
        self.persist_document(&next)?;
        self.document = next;
        self.session_keys.retain(|id, _| {
            self.document
                .profiles
                .iter()
                .any(|p| &p.credential_ref == id)
        });
        if let Some(key) = key {
            self.session_keys.insert(profile.credential_ref, key);
        }
        Ok(())
    }
    /// Remove only this app's profile/key, not a provider subscription.
    pub fn remove(&mut self, id: &str) -> Result<()> {
        let mut next = self.copy_document();
        next.profiles.retain(|p| p.id != id);
        next.protected_keys
            .retain(|key, _| next.profiles.iter().any(|p| &p.credential_ref == key));
        next.usage.remove(id);
        if next.selected.as_deref() == Some(id) {
            next.selected = next.profiles.first().map(|p| p.id.clone());
        }
        self.persist_document(&next)?;
        self.document = next;
        self.session_keys.retain(|key, _| {
            self.document
                .profiles
                .iter()
                .any(|p| &p.credential_ref == key)
        });
        Ok(())
    }
    /// Record an attempt before sending, without imposing a local request allowance.
    /// Failures remain counted because
    /// a timed-out provider may have processed and billed the request.
    pub fn record_request(&mut self, profile: &Profile, month: &str) -> Result<()> {
        if month.len() != 7 || !month.chars().all(|c| c.is_ascii_digit() || c == '-') {
            return Err(AiError::Invalid);
        }
        let mut next = self.copy_document();
        let usage = next.usage.entry(profile.id.clone()).or_default();
        if usage.month != month {
            *usage = Usage {
                month: month.into(),
                ..Default::default()
            };
        }
        usage.requests = usage.requests.saturating_add(1);
        self.persist_document(&next)?;
        self.document = next;
        Ok(())
    }
    /// Account only provider-reported token usage, with saturating arithmetic.
    pub fn record_tokens(&mut self, id: &str, input: u64, output: u64) -> Result<()> {
        let mut next = self.copy_document();
        let usage = next.usage.entry(id.into()).or_default();
        usage.input_tokens = usage.input_tokens.saturating_add(input);
        usage.output_tokens = usage.output_tokens.saturating_add(output);
        self.persist_document(&next)?;
        self.document = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn magpie_lan_profiles_persist_without_loosening_credential_binding() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = ProfileStore::open(dir.path()).unwrap();
        store
            .save(
                Profile {
                    protocol: Protocol::Magpie,
                    base_url: "http://192.168.31.3:3425/v1".into(),
                    model: "group/sound-design".into(),
                    ..Default::default()
                },
                Some(ApiKey::new("owned-gateway-key".into()).unwrap()),
            )
            .unwrap();
        let saved = store.selected().unwrap().clone();
        let before = std::fs::read(&store.path).unwrap();
        let reopened = ProfileStore::open(dir.path()).unwrap();
        assert_eq!(reopened.selected().unwrap(), &saved);
        let mut draft = saved.clone();
        draft.protocol = Protocol::ChatCompletions;
        assert!(store.save(draft, None).is_err());
        let mut draft = saved.clone();
        draft.base_url = "http://192.168.31.4:3425/v1".into();
        assert!(draft.validate_discovery_request().is_ok());
        assert!(store.discovery_key(&draft, None).is_err());
        assert!(store.save(draft, None).is_err());
        assert_eq!(store.selected().unwrap(), &saved);
        assert_eq!(std::fs::read(&store.path).unwrap(), before);
    }

    use crate::config::Protocol;
    #[test]
    fn profile_names_and_hidden_key_presence_survive_reopening() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = ProfileStore::open(dir.path()).unwrap();
        let mut first = Profile {
            protocol: Protocol::OpenCodeGo,
            model: "first-model".into(),
            ..Default::default()
        };
        first.name = store.next_profile_name(first.protocol, "");
        store
            .save(
                first.clone(),
                Some(ApiKey::new("hidden-fixture-key".into()).unwrap()),
            )
            .unwrap();
        let saved = store.selected().unwrap().clone();
        assert!(store.has_key(&saved));
        assert_eq!(
            store.next_profile_name(first.protocol, &first.id),
            first.name
        );
        let second = Profile {
            name: store.next_profile_name(first.protocol, ""),
            model: "second-model".into(),
            protocol: first.protocol,
            ..Default::default()
        };
        assert_ne!(second.name, first.name);
        store.save(second.clone(), None).unwrap();
        assert!(!store.has_key(store.selected().unwrap()));
        let reopened = ProfileStore::open(dir.path()).unwrap();
        assert_eq!(reopened.selected().unwrap().name, second.name);
        assert_eq!(reopened.profiles()[0].name, first.name);
        #[cfg(windows)]
        assert!(reopened.has_key(&saved));
        assert!(
            !std::fs::read_to_string(&store.path)
                .unwrap()
                .contains("hidden-fixture-key")
        );
    }
    #[test]
    fn discovery_drafts_keep_keys_bound_to_the_saved_endpoint_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = ProfileStore::open(dir.path()).unwrap();
        let mut draft = Profile {
            model: "saved-model".into(),
            ..Default::default()
        };
        store
            .save(
                draft.clone(),
                Some(ApiKey::new("saved-key".into()).unwrap()),
            )
            .unwrap();
        let before = std::fs::read(&store.path).unwrap();
        draft.model.clear();
        assert_eq!(
            store.discovery_key(&draft, None).unwrap().expose(),
            "saved-key"
        );
        draft.base_url = "https://another.example/v1".into();
        assert!(store.discovery_key(&draft, None).is_err());
        assert_eq!(
            store
                .discovery_key(&draft, Some(ApiKey::new("new-key".into()).unwrap()))
                .unwrap()
                .expose(),
            "new-key"
        );
        assert!(
            store
                .discovery_key(&draft, Some(ApiKey::default()))
                .unwrap()
                .is_empty()
        );
        draft.base_url = Protocol::default().default_url().into();
        draft.protocol = Protocol::DeepSeek;
        assert!(store.discovery_key(&draft, None).is_err());
        draft.id = "unsaved-profile".into();
        assert!(store.discovery_key(&draft, None).unwrap().is_empty());
        assert_eq!(std::fs::read(&store.path).unwrap(), before);
        assert_eq!(store.profiles().len(), 1);
    }
    #[test]
    fn custom_prompt_survives_default_selection_profile_edits_and_restart() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = ProfileStore::open(dir.path()).unwrap();
        assert_eq!(store.prompt().effective(), crate::config::DEFAULT_PROMPT);
        store
            .save_prompt(true, "优先寻找适合分层制作的原始素材")
            .unwrap();
        store
            .save(
                Profile {
                    model: "mock".into(),
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        store
            .save_prompt(false, crate::config::DEFAULT_PROMPT)
            .unwrap();
        let mut reopened = ProfileStore::open(dir.path()).unwrap();
        assert_eq!(reopened.prompt().effective(), crate::config::DEFAULT_PROMPT);
        assert_eq!(reopened.prompt().custom, "优先寻找适合分层制作的原始素材");
        assert!(reopened.save_prompt(true, &"词".repeat(2001)).is_err());
        assert!(!reopened.prompt().use_custom);
    }
    #[test]
    fn retired_limits_do_not_reject_attempts_and_future_configuration_is_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = ProfileStore::open(dir.path()).unwrap();
        let p = Profile {
            model: "test-model".into(),
            max_requests_per_month: 1,
            ..Default::default()
        };
        store
            .save(p.clone(), Some(ApiKey::new("PRIVATE-KEY".into()).unwrap()))
            .unwrap();
        let bytes = std::fs::read_to_string(&store.path).unwrap();
        assert!(!bytes.contains("PRIVATE-KEY"));
        assert!(!store.key(store.selected().unwrap()).unwrap().is_empty());
        store.record_request(&p, "2026-11").unwrap();
        store.record_request(&p, "2026-11").unwrap();
        store.record_tokens(&p.id, 25, 13).unwrap();
        assert_eq!(store.usage(&p.id).requests, 2);
        let mut edited = store.selected().unwrap().clone();
        edited.request_mode = crate::config::RequestMode::Server;
        edited.timeout_seconds = 600;
        store.save(edited, None).unwrap();
        let reopened = ProfileStore::open(dir.path()).unwrap();
        assert_eq!(reopened.usage(&p.id).requests, 2);
        assert_eq!(reopened.usage(&p.id).input_tokens, 25);
        assert_eq!(reopened.usage(&p.id).output_tokens, 13);
        assert_eq!(reopened.selected().unwrap().timeout_seconds, 600);
        #[cfg(windows)]
        assert_eq!(
            reopened.key(reopened.selected().unwrap()).unwrap().expose(),
            "PRIVATE-KEY"
        );
        store.record_request(&p, "2026-12").unwrap();
        let future = bytes.replace("\"version\": 1", "\"version\": 999");
        std::fs::write(&store.path, &future).unwrap();
        assert!(ProfileStore::open(dir.path()).is_err());
        assert_eq!(std::fs::read_to_string(&store.path).unwrap(), future);
    }
    #[test]
    fn editing_endpoint_cannot_reuse_a_credential_implicitly() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = ProfileStore::open(dir.path()).unwrap();
        let mut p = Profile {
            model: "model".into(),
            ..Default::default()
        };
        s.save(p.clone(), Some(ApiKey::new("key".into()).unwrap()))
            .unwrap();
        p.base_url = "https://different.example/v1".into();
        assert!(s.save(p, None).is_err());
    }
}
