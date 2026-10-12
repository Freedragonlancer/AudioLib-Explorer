// crates/audiolib-remote/src/secrets.rs
//! API token and WebDAV password storage, separate from settings.json.
//!
//! Stored at `<data dir>/remote/secrets.bin`. On Windows the JSON payload is encrypted
//! for the current user with DPAPI; elsewhere it is written with owner-only access.
//! Values are never logged: `Debug` is redacted.
//!
//! Tokens are kept per server profile (`profiles`, keyed by profile id). The top-level
//! `token` stays as the token of the profile the old single-server settings fields
//! describe ([`DataSourceSettings::legacy_profile_id`]), so older clients keep working;
//! see [`RemoteSecrets::reconcile`] and [`RemoteSecrets::mirror`].
use crate::error::{RemoteError, Result};
use crate::util::write_atomic;
use audiolib_core::config::DataSourceSettings;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const DPAPI_MAGIC: &[u8] = b"ALXS-DPAPI-1\n";
const PLAIN_MAGIC: &[u8] = b"ALXS-PLAIN-1\n";

/// Secrets for server mode.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteSecrets {
    /// Token of the legacy (active or most recently used) profile; mirrored.
    #[serde(default)]
    pub token: String,
    /// Global, like the other WebDAV settings.
    #[serde(default)]
    pub webdav_password: String,
    /// Per-profile secrets keyed by profile id. Older clients ignore this field (and
    /// drop it when they save; [`RemoteSecrets::reconcile`] then migrates again).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub profiles: BTreeMap<String, ProfileSecret>,
}

/// Secrets of one server profile.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileSecret {
    #[serde(default)]
    pub token: String,
}

impl std::fmt::Debug for ProfileSecret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProfileSecret")
            .field("token", &redacted(&self.token))
            .finish()
    }
}

fn redacted(value: &str) -> &'static str {
    if value.is_empty() { "" } else { "<redacted>" }
}

impl RemoteSecrets {
    /// Nothing stored (saving removes the file).
    pub fn is_empty(&self) -> bool {
        self.token.is_empty()
            && self.webdav_password.is_empty()
            && self.profiles.values().all(|secret| secret.token.is_empty())
    }

    /// Token saved for a profile (empty when none).
    pub fn token_for(&self, profile_id: &str) -> &str {
        self.profiles
            .get(profile_id)
            .map(|secret| secret.token.as_str())
            .unwrap_or_default()
    }

    /// Save (or clear, when empty after trimming) a profile's token.
    pub fn set_token(&mut self, profile_id: &str, token: &str) {
        let token = token.trim();
        if token.is_empty() {
            self.profiles.remove(profile_id);
        } else {
            self.profiles
                .entry(profile_id.to_owned())
                .or_default()
                .token = token.to_owned();
        }
    }

    /// Forget a removed profile's secrets.
    pub fn remove_profile(&mut self, profile_id: &str) {
        self.profiles.remove(profile_id);
    }

    /// Load-time migration, after [`DataSourceSettings::migrate_profiles`]: without
    /// per-profile entries (first run of this version, or the file was rewritten by an
    /// older client) the top-level token belongs to the legacy profile. A legacy profile
    /// without an entry (an older client added its URL) also takes the top-level token.
    /// Otherwise the per-profile entries are authoritative and the top-level token is
    /// re-mirrored. Returns whether anything changed (not saved here).
    pub fn reconcile(&mut self, settings: &DataSourceSettings) -> bool {
        let before = self.clone();
        if let Some(id) = settings.legacy_profile_id() {
            if !self.profiles.contains_key(&id) {
                let token = self.token.clone();
                self.set_token(&id, &token);
            }
            self.token = self.token_for(&id).to_owned();
        }
        *self != before
    }

    /// Before saving: mirror the legacy profile's token to the top-level `token` and drop
    /// entries of profiles that no longer exist. Without a legacy profile the top-level
    /// token is left as it is.
    pub fn mirror(&mut self, settings: &DataSourceSettings) {
        self.profiles
            .retain(|id, secret| settings.profile(id).is_some() && !secret.token.is_empty());
        if let Some(id) = settings.legacy_profile_id() {
            self.token = self.token_for(&id).to_owned();
        }
    }
}

impl std::fmt::Debug for RemoteSecrets {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RemoteSecrets")
            .field("token", &redacted(&self.token))
            .field("webdav_password", &redacted(&self.webdav_password))
            .field("profiles", &self.profiles)
            .finish()
    }
}

/// Location of the protected secrets file for a user data root.
pub fn secrets_path(data_root: &Path) -> PathBuf {
    data_root.join("remote").join("secrets.bin")
}

/// Load secrets; a missing file yields empty secrets.
pub fn load_secrets(data_root: &Path) -> Result<RemoteSecrets> {
    let path = secrets_path(data_root);
    let data = match std::fs::read(&path) {
        Ok(data) => data,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RemoteSecrets::default());
        }
        Err(error) => return Err(error.into()),
    };
    let json = if let Some(protected) = data.strip_prefix(DPAPI_MAGIC) {
        audiolib_windows::unprotect_user_secret(protected).map_err(|_| {
            RemoteError::Validation(
                "Stored server credentials cannot be decrypted for this Windows user; enter them again"
                    .into(),
            )
        })?
    } else if let Some(plain) = data.strip_prefix(PLAIN_MAGIC) {
        plain.to_vec()
    } else {
        return Err(RemoteError::Validation(
            "Stored server credentials have an unknown format; enter them again".into(),
        ));
    };
    serde_json::from_slice(&json)
        .map_err(|_| RemoteError::Validation("Stored server credentials are corrupt".into()))
}

/// Load secrets and fold them into the profile layout of `settings`
/// ([`RemoteSecrets::reconcile`]); nothing is written.
pub fn load_profile_secrets(
    data_root: &Path,
    settings: &DataSourceSettings,
) -> Result<RemoteSecrets> {
    let mut secrets = load_secrets(data_root)?;
    secrets.reconcile(settings);
    Ok(secrets)
}

/// Persist secrets atomically (DPAPI on Windows). Empty secrets remove the file.
pub fn save_secrets(data_root: &Path, secrets: &RemoteSecrets) -> Result<()> {
    let path = secrets_path(data_root);
    if secrets.is_empty() {
        match std::fs::remove_file(&path) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        }
    }
    let json =
        serde_json::to_vec(secrets).map_err(|error| RemoteError::Protocol(error.to_string()))?;
    let mut data = Vec::new();
    if audiolib_windows::user_secret_protection_available() {
        data.extend_from_slice(DPAPI_MAGIC);
        data.extend(
            audiolib_windows::protect_user_secret(&json).map_err(|error| {
                RemoteError::Io(std::io::Error::other(format!("DPAPI: {error}")))
            })?,
        );
    } else {
        data.extend_from_slice(PLAIN_MAGIC);
        data.extend(json);
    }
    write_atomic(&path, &data)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn secrets_round_trip_protected_and_redacted() {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(
            load_secrets(directory.path()).unwrap(),
            RemoteSecrets::default()
        );
        let secrets = RemoteSecrets {
            token: "tok-密钥-123".into(),
            webdav_password: "dav-pass".into(),
            ..RemoteSecrets::default()
        };
        save_secrets(directory.path(), &secrets).unwrap();
        let raw = std::fs::read(secrets_path(directory.path())).unwrap();
        if audiolib_windows::user_secret_protection_available() {
            assert!(!raw.windows(8).any(|window| window == b"dav-pass"));
        }
        assert_eq!(load_secrets(directory.path()).unwrap(), secrets);
        let debug = format!("{secrets:?}");
        assert!(!debug.contains("tok-") && !debug.contains("dav-pass"));
        save_secrets(directory.path(), &RemoteSecrets::default()).unwrap();
        assert!(!secrets_path(directory.path()).exists());
        std::fs::create_dir_all(directory.path().join("remote")).unwrap();
        std::fs::write(secrets_path(directory.path()), b"garbage").unwrap();
        assert!(load_secrets(directory.path()).is_err());
    }
}

#[cfg(test)]
mod profile_tests {
    use super::*;
    use audiolib_core::config::{ActiveSource, DEFAULT_PROFILE_ID, DataSourceMode};

    fn server_settings(url: &str) -> DataSourceSettings {
        let mut settings = DataSourceSettings {
            mode: DataSourceMode::Server,
            server_url: url.into(),
            ..DataSourceSettings::default()
        };
        settings.migrate_profiles();
        settings
    }

    #[test]
    fn the_single_token_migrates_to_the_first_profile_and_stays_mirrored() {
        let directory = tempfile::tempdir().unwrap();
        // A secrets file of the previous release: one token, no profiles.
        std::fs::create_dir_all(directory.path().join("remote")).unwrap();
        let mut old = b"ALXS-PLAIN-1\n".to_vec();
        old.extend_from_slice(br#"{"token":"lan-token","webdav_password":"dav"}"#);
        std::fs::write(secrets_path(directory.path()), old).unwrap();
        let mut settings = server_settings("http://192.168.1.10:8790");
        let mut secrets = load_profile_secrets(directory.path(), &settings).unwrap();
        assert_eq!(secrets.token_for(DEFAULT_PROFILE_ID), "lan-token");
        assert_eq!(secrets.token, "lan-token");
        assert_eq!(secrets.webdav_password, "dav");
        // A second profile with its own token; switching mirrors the top-level token.
        let ext = settings
            .upsert_external_profile(DEFAULT_PROFILE_ID, "https://audiolib.example.com", None)
            .unwrap();
        secrets.set_token(&ext, " ext-token ");
        settings
            .set_active(&ActiveSource::Profile(ext.clone()))
            .unwrap();
        secrets.mirror(&settings);
        assert_eq!(secrets.token, "ext-token");
        save_secrets(directory.path(), &secrets).unwrap();
        let loaded = load_profile_secrets(directory.path(), &settings).unwrap();
        assert_eq!(loaded, secrets);
        // An older client reads only the top-level token: the active profile's.
        #[derive(serde::Deserialize)]
        struct OldSecrets {
            token: String,
        }
        let raw = std::fs::read(secrets_path(directory.path())).unwrap();
        let json = if let Some(plain) = raw.strip_prefix(PLAIN_MAGIC) {
            plain.to_vec()
        } else {
            audiolib_windows::unprotect_user_secret(raw.strip_prefix(DPAPI_MAGIC).unwrap()).unwrap()
        };
        let old: OldSecrets = serde_json::from_slice(&json).unwrap();
        assert_eq!(old.token, "ext-token");
        // Local mode keeps the most recently used server's token on top.
        settings.set_active(&ActiveSource::Local).unwrap();
        secrets.mirror(&settings);
        assert_eq!(secrets.token, "ext-token");
        // Removing a profile drops its entry; an emptied token removes the entry.
        settings.remove_profile(&ext).unwrap();
        secrets.mirror(&settings);
        assert!(!secrets.profiles.contains_key(&ext));
        secrets.set_token(DEFAULT_PROFILE_ID, "  ");
        assert_eq!(secrets.token_for(DEFAULT_PROFILE_ID), "");
        // The redacted Debug output never shows a token.
        let debug = format!("{loaded:?}");
        assert!(
            !debug.contains("ext-token") && !debug.contains("lan-token"),
            "{debug}"
        );
        assert!(debug.contains(&ext));
    }

    #[test]
    fn an_older_client_rewrite_is_migrated_again() {
        let mut settings = server_settings("http://192.168.1.10:8790");
        let other = settings
            .add_profile("vps", "https://vps.example.net")
            .unwrap();
        let mut secrets = RemoteSecrets::default();
        secrets.set_token(DEFAULT_PROFILE_ID, "old");
        secrets.set_token(&other, "vps-token");
        secrets.mirror(&settings);
        // The older client saved a new token and dropped `profiles`.
        let mut rewritten = RemoteSecrets {
            token: "new".into(),
            ..RemoteSecrets::default()
        };
        assert!(rewritten.reconcile(&settings));
        assert_eq!(rewritten.token_for(DEFAULT_PROFILE_ID), "new");
        // With entries present they are authoritative and the top level follows them.
        let mut stale = secrets.clone();
        stale.token = "something else".into();
        assert!(stale.reconcile(&settings));
        assert_eq!(stale.token, "old");
        assert!(!stale.reconcile(&settings));
        // An older client added a URL (new legacy profile without an entry).
        let mut edited = settings.clone();
        edited.server_url = "http://10.0.0.2:8790".into();
        edited.migrate_profiles();
        let mut carried = secrets.clone();
        carried.token = "typed-in-old-client".into();
        carried.reconcile(&edited);
        let id = edited.active_profile().unwrap().id.clone();
        assert_eq!(carried.token_for(&id), "typed-in-old-client");
        assert_eq!(carried.token_for(&other), "vps-token");
    }

    #[test]
    fn profile_tokens_alone_keep_the_file() {
        let directory = tempfile::tempdir().unwrap();
        let mut secrets = RemoteSecrets::default();
        secrets.set_token("p-1", "t");
        assert!(!secrets.is_empty());
        save_secrets(directory.path(), &secrets).unwrap();
        assert!(secrets_path(directory.path()).exists());
        assert_eq!(load_secrets(directory.path()).unwrap(), secrets);
        secrets.set_token("p-1", "");
        save_secrets(directory.path(), &secrets).unwrap();
        assert!(!secrets_path(directory.path()).exists());
    }
}
