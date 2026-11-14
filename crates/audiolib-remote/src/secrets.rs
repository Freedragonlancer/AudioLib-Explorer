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
use audiolib_core::server_url::ServerAddress;
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
    /// Address binding for the mirrored token. Older unbound files remain readable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_server_url: Option<String>,
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
    /// Canonical API address authorized for this token, including its path prefix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_url: Option<String>,
    /// Runtime quarantine retains the original record for repair but never exposes
    /// its token to callers. Reload derives this flag again from address bindings.
    #[doc(hidden)]
    #[serde(skip)]
    pub unavailable: bool,
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

// Binding comparison must preserve case-sensitive reverse-proxy prefixes. The
// general profile comparison also folds paths for legacy grouping purposes.
fn server_binding(value: &str) -> Result<String> {
    let mut address = ServerAddress::parse(value).map_err(|_| binding_error())?;
    address.host = match address.bare_host().parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V6(ip)) => format!("[{ip}]"),
        Ok(std::net::IpAddr::V4(ip)) => ip.to_string(),
        Err(_) => address.host.to_ascii_lowercase(),
    };
    address.port = Some(address.effective_port());
    Ok(address.api_url())
}

fn binding_error() -> RemoteError {
    RemoteError::Validation(
        "Stored server credentials do not match the configured address; re-enter the access token"
            .into(),
    )
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
            .filter(|secret| !secret.unavailable)
            .map(|secret| secret.token.as_str())
            .unwrap_or_default()
    }

    /// Save (or clear, when empty after trimming) a profile's token.
    pub fn set_token(&mut self, profile_id: &str, token: &str) {
        let token = token.trim();
        if token.is_empty() {
            self.profiles.remove(profile_id);
        } else {
            // Explicit replacement authorizes a new binding at the next save.
            let secret = self.profiles.entry(profile_id.to_owned()).or_default();
            secret.token = token.to_owned();
            secret.server_url = None;
            secret.unavailable = false;
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
                let binding = self.token_server_url.clone();
                self.set_token(&id, &token);
                if let Some(secret) = self.profiles.get_mut(&id) {
                    secret.server_url = binding;
                }
            }
            self.token = self.token_for(&id).to_owned();
            self.token_server_url = self
                .profiles
                .get(&id)
                .and_then(|secret| secret.server_url.clone());
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
            self.token_server_url = self
                .profiles
                .get(&id)
                .and_then(|secret| secret.server_url.clone());
        }
    }

    /// Bind credentials before persistence. Retained quarantined records require
    /// explicit repair, so a save never silently rebinds or drops their secrets.
    pub fn bind_profiles(&mut self, settings: &DataSourceSettings) -> Result<()> {
        self.prepare_profiles(settings)?;
        if self
            .profiles
            .iter()
            .any(|(id, secret)| secret.unavailable && settings.profile(id).is_some())
        {
            return Err(binding_error());
        }
        Ok(())
    }

    // Syntax errors still reject a malformed document. Valid address mismatches
    // are quarantined individually, preserving unrelated usable credentials and
    // the raw record needed to repair a partial two-file preferences save.
    fn prepare_profiles(&mut self, settings: &DataSourceSettings) -> Result<()> {
        for secret in self.profiles.values() {
            if let Some(bound) = &secret.server_url {
                server_binding(bound)?;
            }
        }
        if let Some(bound) = &self.token_server_url {
            server_binding(bound)?;
        }
        let identities = settings
            .profiles
            .iter()
            .map(|profile| Ok((profile.id.clone(), server_binding(&profile.server_url)?)))
            .collect::<Result<BTreeMap<_, _>>>()?;
        self.reconcile(settings);
        for (id, expected) in identities {
            if let Some(secret) = self.profiles.get_mut(&id) {
                if let Some(bound) = &secret.server_url {
                    secret.unavailable = server_binding(bound)? != expected;
                } else {
                    secret.server_url = Some(expected);
                    secret.unavailable = false;
                }
            }
        }
        self.reconcile(settings);
        Ok(())
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

/// Load raw persisted secrets; a missing file yields empty secrets. Network callers
/// must use [`load_profile_secrets`] to validate bindings and quarantine mismatches.
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

/// Load credentials for the committed settings. Valid address mismatches are
/// individually quarantined; legacy entries migrate in memory. Nothing is written.
pub fn load_profile_secrets(
    data_root: &Path,
    settings: &DataSourceSettings,
) -> Result<RemoteSecrets> {
    let mut secrets = load_secrets(data_root)?;
    secrets.prepare_profiles(settings)?;
    Ok(secrets)
}

/// Bind credentials to the settings being saved, then persist that same object.
/// Address bindings fail closed if settings persistence subsequently fails.
pub fn save_profile_secrets(
    data_root: &Path,
    settings: &DataSourceSettings,
    secrets: &mut RemoteSecrets,
) -> Result<()> {
    secrets.bind_profiles(settings)?;
    save_secrets(data_root, secrets)
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
        let mut settings = server_settings("http://server.example.lan:8790");
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
        save_profile_secrets(directory.path(), &settings, &mut secrets).unwrap();
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
        let mut settings = server_settings("http://server.example.lan:8790");
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

    fn move_profile(settings: &DataSourceSettings, url: &str) -> DataSourceSettings {
        let mut moved = settings.clone();
        moved.profile_mut(DEFAULT_PROFILE_ID).unwrap().server_url = url.into();
        moved.mirror_active();
        moved
    }

    #[test]
    fn failed_settings_commit_keeps_replacement_credentials_off_the_old_address() {
        let directory = tempfile::tempdir().unwrap();
        let mut committed = server_settings("https://old.example.test/Audio");
        let unrelated = committed
            .add_profile("Unrelated", "https://other.example.test")
            .unwrap();
        let settings_path = directory.path().join("settings.json");
        std::fs::write(&settings_path, serde_json::to_vec(&committed).unwrap()).unwrap();
        let mut credentials = RemoteSecrets::default();
        credentials.set_token(DEFAULT_PROFILE_ID, "owned-old-token");
        credentials.set_token(&unrelated, "owned-unrelated-token");
        save_profile_secrets(directory.path(), &committed, &mut credentials).unwrap();

        // Model the actual write order: new encrypted credentials succeed, but
        // the settings replacement fails, leaving the committed settings intact.
        let replacement = move_profile(&committed, "https://replacement.example.test/Audio");
        credentials.set_token(DEFAULT_PROFILE_ID, "owned-replacement-token");
        save_profile_secrets(directory.path(), &replacement, &mut credentials).unwrap();
        let before = std::fs::read(secrets_path(directory.path())).unwrap();
        let mut masked = load_profile_secrets(directory.path(), &committed).unwrap();
        assert!(masked.token_for(DEFAULT_PROFILE_ID).is_empty());
        assert!(masked.token.is_empty());
        assert_eq!(masked.token_for(&unrelated), "owned-unrelated-token");
        assert!(masked.profiles[DEFAULT_PROFILE_ID].unavailable);
        assert!(
            serde_json::to_value(&masked).unwrap()["profiles"][DEFAULT_PROFILE_ID]
                .get("unavailable")
                .is_none()
        );
        let restarted: DataSourceSettings =
            serde_json::from_slice(&std::fs::read(&settings_path).unwrap()).unwrap();
        assert!(
            load_profile_secrets(directory.path(), &restarted)
                .unwrap()
                .token_for(DEFAULT_PROFILE_ID)
                .is_empty()
        );
        assert!(save_profile_secrets(directory.path(), &committed, &mut masked).is_err());
        assert_eq!(
            std::fs::read(secrets_path(directory.path())).unwrap(),
            before
        );
        let preserved = load_secrets(directory.path()).unwrap();
        assert_eq!(preserved.token_for(&unrelated), "owned-unrelated-token");
        assert_eq!(
            preserved.token_for(DEFAULT_PROFILE_ID),
            "owned-replacement-token"
        );
        assert_eq!(
            load_profile_secrets(directory.path(), &replacement)
                .unwrap()
                .token_for(DEFAULT_PROFILE_ID),
            "owned-replacement-token"
        );
        masked.set_token(DEFAULT_PROFILE_ID, "owned-repaired-token");
        save_profile_secrets(directory.path(), &committed, &mut masked).unwrap();
        let repaired = load_profile_secrets(directory.path(), &committed).unwrap();
        assert_eq!(
            repaired.token_for(DEFAULT_PROFILE_ID),
            "owned-repaired-token"
        );
        assert_eq!(repaired.token_for(&unrelated), "owned-unrelated-token");
        assert!(!repaired.profiles[DEFAULT_PROFILE_ID].unavailable);
    }

    #[test]
    fn a_bound_mirrored_token_cannot_rehydrate_a_removed_old_profile() {
        let directory = tempfile::tempdir().unwrap();
        let mut committed = server_settings("https://old.example.test");
        let next = committed
            .add_profile("Replacement", "https://replacement.example.test")
            .unwrap();
        let mut credentials = RemoteSecrets::default();
        credentials.set_token(DEFAULT_PROFILE_ID, "owned-old-token");
        credentials.set_token(&next, "owned-next-token");
        save_profile_secrets(directory.path(), &committed, &mut credentials).unwrap();
        let mut replacement = committed.clone();
        replacement
            .set_active(&ActiveSource::Profile(next.clone()))
            .unwrap();
        replacement.remove_profile(DEFAULT_PROFILE_ID).unwrap();
        credentials.remove_profile(DEFAULT_PROFILE_ID);
        credentials.mirror(&replacement);
        save_profile_secrets(directory.path(), &replacement, &mut credentials).unwrap();

        // Old settings still select a profile absent from the new secrets file.
        // Its legacy fallback must not adopt the new active server's token.
        let before = std::fs::read(secrets_path(directory.path())).unwrap();
        let mut masked = load_profile_secrets(directory.path(), &committed).unwrap();
        assert!(masked.token_for(DEFAULT_PROFILE_ID).is_empty());
        assert!(masked.token.is_empty());
        assert!(masked.profiles[DEFAULT_PROFILE_ID].unavailable);
        assert_eq!(masked.token_for(&next), "owned-next-token");
        assert_eq!(
            std::fs::read(secrets_path(directory.path())).unwrap(),
            before
        );
        let raw = load_secrets(directory.path()).unwrap();
        assert!(!raw.profiles.contains_key(DEFAULT_PROFILE_ID));
        assert_eq!(raw.token, "owned-next-token");
        assert!(raw.token_server_url.is_some());
        assert_eq!(
            load_profile_secrets(directory.path(), &replacement)
                .unwrap()
                .token_for(&next),
            "owned-next-token"
        );
        masked.set_token(DEFAULT_PROFILE_ID, "owned-repaired-token");
        save_profile_secrets(directory.path(), &committed, &mut masked).unwrap();
        let repaired = load_profile_secrets(directory.path(), &committed).unwrap();
        assert_eq!(
            repaired.token_for(DEFAULT_PROFILE_ID),
            "owned-repaired-token"
        );
        assert_eq!(repaired.token_for(&next), "owned-next-token");
    }

    #[test]
    fn bindings_normalize_authority_and_suffix_without_folding_path_prefixes() {
        let directory = tempfile::tempdir().unwrap();
        let committed = server_settings("https://SERVER.example.test/Audio");
        let mut credentials = RemoteSecrets::default();
        credentials.set_token(DEFAULT_PROFILE_ID, "owned-token");
        save_profile_secrets(directory.path(), &committed, &mut credentials).unwrap();
        let equivalent = move_profile(&committed, "https://server.example.test:443/Audio/api/v1/");
        assert_eq!(
            load_profile_secrets(directory.path(), &equivalent)
                .unwrap()
                .token_for(DEFAULT_PROFILE_ID),
            "owned-token"
        );
        for different in [
            "https://server.example.test/audio",
            "https://server.example.test/Other",
            "https://server.example.test:444/Audio",
            "http://server.example.test:443/Audio",
        ] {
            let mut masked =
                load_profile_secrets(directory.path(), &move_profile(&committed, different))
                    .unwrap();
            assert!(masked.token_for(DEFAULT_PROFILE_ID).is_empty());
            assert!(
                save_profile_secrets(
                    directory.path(),
                    &move_profile(&committed, different),
                    &mut masked,
                )
                .is_err()
            );
        }
        assert_eq!(
            server_binding("https://[2001:0db8::1]/Audio").unwrap(),
            server_binding("https://[2001:db8:0:0:0:0:0:1]:443/Audio/api/v1").unwrap()
        );
        for invalid in [
            "not a URL",
            "https://user:password@example.test",
            "https://example.test/?key=invalid",
        ] {
            let mut invalid_binding = credentials.clone();
            invalid_binding
                .profiles
                .get_mut(DEFAULT_PROFILE_ID)
                .unwrap()
                .server_url = Some(invalid.into());
            save_secrets(directory.path(), &invalid_binding).unwrap();
            assert!(load_profile_secrets(directory.path(), &committed).is_err());
        }
    }

    #[test]
    fn endpoint_changes_require_explicit_token_replacement_before_rebinding() {
        let directory = tempfile::tempdir().unwrap();
        let committed = server_settings("https://old.example.test");
        let mut credentials = RemoteSecrets::default();
        credentials.set_token(DEFAULT_PROFILE_ID, "owned-old-token");
        save_profile_secrets(directory.path(), &committed, &mut credentials).unwrap();
        let before = std::fs::read(secrets_path(directory.path())).unwrap();
        let replacement = move_profile(&committed, "https://replacement.example.test");
        assert!(save_profile_secrets(directory.path(), &replacement, &mut credentials).is_err());
        assert_eq!(
            std::fs::read(secrets_path(directory.path())).unwrap(),
            before
        );
        credentials.set_token(DEFAULT_PROFILE_ID, "owned-new-token");
        save_profile_secrets(directory.path(), &replacement, &mut credentials).unwrap();
        assert_eq!(
            load_profile_secrets(directory.path(), &replacement)
                .unwrap()
                .token_for(DEFAULT_PROFILE_ID),
            "owned-new-token"
        );
        assert!(
            load_profile_secrets(directory.path(), &committed)
                .unwrap()
                .token_for(DEFAULT_PROFILE_ID)
                .is_empty()
        );
    }
}
