// crates/audiolib-core/src/profiles.rs
//! Saved server connection profiles (multi-server data sources) and their migration from
//! the single-server settings of earlier releases.
//!
//! `settings.json` keeps the old single-server fields (`mode`, `server_url`) and mirrors
//! the active profile into them, so an older client still opens the same server. The new
//! fields are `data_source.active` (`"local"` or a profile id) and
//! `data_source.profiles`. Rules (see `doc/Rust_Remote_Mode.md`):
//!
//! * [`DataSourceSettings::migrate_profiles`] (on every load and save, idempotent): a file
//!   without profiles but with a server URL gets one profile (id [`DEFAULT_PROFILE_ID`]);
//!   when the old fields disagree with `active` (an older client changed them) the old
//!   fields win: an existing profile with that URL becomes active, otherwise a new profile
//!   is added. Nothing is ever deleted by migration.
//! * [`DataSourceSettings::mirror_active`] (after changing `active` or a profile): writes
//!   the active profile back into `mode` / `server_url`. In local mode `server_url` keeps
//!   the URL of the most recently used server (what an older client would switch to).
//!
//! Profiles carry no secrets: tokens live in the protected secrets file, keyed by profile
//! id (`audiolib-remote`).
use crate::config::{DataSourceMode, DataSourceSettings, WebDavSettings};
use crate::server_url::ServerAddress;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::sync::atomic::{AtomicU32, Ordering};

/// `data_source.active` value selecting the local library.
pub const ACTIVE_LOCAL: &str = "local";
/// Id of the profile migrated from a single-server settings file.
pub const DEFAULT_PROFILE_ID: &str = "default";
/// Longest accepted profile name (characters).
pub const MAX_PROFILE_NAME_CHARS: usize = 64;
/// Longest accepted profile id (ASCII letters, digits, `-`, `_`).
pub const MAX_PROFILE_ID_CHARS: usize = 64;
/// Most profiles one settings file may hold.
pub const MAX_PROFILES: usize = 64;
/// Longest stored test summary (characters); longer text is cut.
pub const MAX_TEST_SUMMARY_CHARS: usize = 200;

/// Network role of a profile's address: the same server is often saved twice, once with
/// its LAN address (no token needed by default) and once with its external address.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProfileKind {
    #[default]
    Lan,
    External,
}

impl ProfileKind {
    /// Best guess from the address: private, loopback and link-local IPs, `localhost`,
    /// single-label names and `.local` / `.lan` / `.home.arpa` hosts are LAN; anything
    /// else (public IPs, ordinary domains) is external. Unparsable input counts as LAN.
    pub fn guess(server_url: &str) -> Self {
        let Ok(address) = ServerAddress::parse(server_url) else {
            return Self::Lan;
        };
        let host = address.bare_host().to_ascii_lowercase();
        let lan = match host.parse::<IpAddr>() {
            Ok(IpAddr::V4(ip)) => {
                ip.is_private() || ip.is_loopback() || ip.is_link_local() || ip.is_unspecified()
            }
            Ok(IpAddr::V6(ip)) => {
                let first = ip.segments()[0];
                ip.is_loopback()
                    || ip.is_unspecified()
                    || (first & 0xfe00) == 0xfc00 // unique local fc00::/7
                    || (first & 0xffc0) == 0xfe80 // link local fe80::/10
                    || ip
                        .to_ipv4_mapped()
                        .is_some_and(|v4| v4.is_private() || v4.is_loopback())
            }
            Err(_) => {
                host == "localhost"
                    || !host.contains('.')
                    || [".local", ".lan", ".home.arpa", ".localdomain"]
                        .iter()
                        .any(|suffix| host.ends_with(suffix))
            }
        };
        if lan { Self::Lan } else { Self::External }
    }
}

/// Short result of the latest connection test of a profile (no paths, no secrets).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TestSummary {
    /// Unix seconds of the test.
    #[serde(default)]
    pub at: u64,
    #[serde(default)]
    pub ok: bool,
    /// One-line verdict such as `ok · 0.3.6 · r1287` or `需要令牌（401）`.
    #[serde(default)]
    pub summary: String,
}

/// One saved server connection. Secrets are stored separately, keyed by `id`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServerProfile {
    /// Stable identity (never reused for another server; survives renames and URL edits).
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// Base URL as stored in `server_url` (`scheme://host[:port][/prefix]`).
    #[serde(default, alias = "url")]
    pub server_url: String,
    #[serde(default)]
    pub kind: ProfileKind,
    /// Profiles of one server (its LAN and external addresses) share a group; empty means
    /// the profile is its own group (see [`ServerProfile::group_key`]).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub group: String,
    /// Server library identity seen by the latest test or sync. Profiles with the same
    /// identity share one snapshot cache (the cache is keyed by library id, not URL).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub library_id: Option<String>,
    /// Optional per-profile WebDAV preferences, same shape as `data_source.webdav`.
    /// `None` (the only form the desktop writes today) uses the global settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webdav: Option<WebDavSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_test: Option<TestSummary>,
    /// Audition through the server's transcoded `/preview` (16-bit PCM, FLAC on slow
    /// links) instead of the original `/stream`: the user's choice in the data source
    /// dialog, authoritative for this profile. `None` (settings written before this
    /// field) is filled once by [`DataSourceSettings::migrate_profiles`]; until then
    /// [`default_transcode`] applies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcode: Option<bool>,
}

/// Default of [`ServerProfile::transcode`] for an address: on for `https://` and
/// external (public) hosts, off for plain `http://` LAN addresses (private, loopback and
/// link-local IPs, `.local` names, … see [`ProfileKind::guess`]), which do not need it.
pub fn default_transcode(server_url: &str) -> bool {
    let https =
        ServerAddress::parse(server_url).is_ok_and(|address| address.scheme.as_str() == "https");
    https || ProfileKind::guess(server_url) == ProfileKind::External
}

impl ServerProfile {
    /// Whether auditions through this profile use the transcoded `/preview`.
    pub fn transcode_enabled(&self) -> bool {
        self.transcode
            .unwrap_or_else(|| default_transcode(&self.server_url))
    }

    /// The group this profile belongs to (its own id when `group` is empty).
    pub fn group_key(&self) -> &str {
        if self.group.is_empty() {
            &self.id
        } else {
            &self.group
        }
    }
}

/// Which data source is active.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActiveSource {
    Local,
    Profile(String),
}

/// What [`DataSourceSettings::migrate_profiles`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileMigration {
    /// Already consistent.
    Unchanged,
    /// First migration of a single-server settings file into a profile.
    Created,
    /// The old fields had been changed (older client) and the profiles followed them.
    Reconciled,
}

/// Normalised comparison key of a server URL (`None` when it does not parse).
fn url_key(url: &str) -> Option<String> {
    ServerAddress::parse(url.trim())
        .ok()
        .map(|address| address.api_url().to_ascii_lowercase())
}

/// True when both URLs address the same API (scheme, host, port and prefix).
pub fn same_server_url(a: &str, b: &str) -> bool {
    match (url_key(a), url_key(b)) {
        (Some(a), Some(b)) => a == b,
        _ => a.trim() == b.trim(),
    }
}

/// Default display name for a server URL: its host (and non-default port).
pub fn default_profile_name(server_url: &str) -> String {
    match ServerAddress::parse(server_url) {
        Ok(address) => match address.port {
            Some(port) if port != address.scheme.default_port() => {
                format!("{}:{port}", address.host)
            }
            _ => address.host,
        },
        Err(_) => server_url
            .trim()
            .chars()
            .take(MAX_PROFILE_NAME_CHARS)
            .collect(),
    }
}

fn clip(text: &str, limit: usize) -> String {
    let line = text.replace(['\r', '\n', '\t'], " ");
    let line = line.trim();
    if line.chars().count() <= limit {
        line.to_owned()
    } else {
        let mut cut: String = line.chars().take(limit.saturating_sub(1)).collect();
        cut.push('…');
        cut
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_PROFILE_ID_CHARS
        && id != ACTIVE_LOCAL
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

static ID_COUNTER: AtomicU32 = AtomicU32::new(0);

impl DataSourceSettings {
    /// The active data source (`active` naming no existing profile counts as local).
    pub fn active_source(&self) -> ActiveSource {
        match self.active_profile() {
            Some(profile) => ActiveSource::Profile(profile.id.clone()),
            None => ActiveSource::Local,
        }
    }

    /// Whether server-mode audition uses the transcoded `/preview` (the active profile's
    /// [`ServerProfile::transcode`]; settings without profiles use the address default,
    /// off when an older build set `audition_original`).
    pub fn audition_transcode(&self) -> bool {
        match self.active_profile() {
            Some(profile) => profile.transcode_enabled(),
            None => !self.audition_original && default_transcode(&self.server_url),
        }
    }

    /// The active server profile, if a server is active.
    pub fn active_profile(&self) -> Option<&ServerProfile> {
        let id = self.active.as_deref().filter(|id| *id != ACTIVE_LOCAL)?;
        self.profile(id)
    }

    pub fn profile(&self, id: &str) -> Option<&ServerProfile> {
        self.profiles.iter().find(|profile| profile.id == id)
    }

    pub fn profile_mut(&mut self, id: &str) -> Option<&mut ServerProfile> {
        self.profiles.iter_mut().find(|profile| profile.id == id)
    }

    /// The profile saved with this URL (the active one first when several match).
    pub fn profile_for_url(&self, server_url: &str) -> Option<&ServerProfile> {
        self.active_profile()
            .filter(|profile| same_server_url(&profile.server_url, server_url))
            .or_else(|| {
                self.profiles
                    .iter()
                    .find(|profile| same_server_url(&profile.server_url, server_url))
            })
    }

    /// The profile the old single-server fields describe: the active profile in server
    /// mode, else the profile whose URL is in `server_url` (most recently used server).
    /// Its token is mirrored to the old top-level token in the secrets file.
    pub fn legacy_profile_id(&self) -> Option<String> {
        if let Some(profile) = self.active_profile() {
            return Some(profile.id.clone());
        }
        let url = self.server_url.trim();
        if url.is_empty() {
            return None;
        }
        self.profile_for_url(url).map(|profile| profile.id.clone())
    }

    /// A new unique profile id (`p-<hex>`).
    pub fn new_profile_id(&self) -> String {
        loop {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or_default();
            let count = ID_COUNTER.fetch_add(1, Ordering::Relaxed);
            let mixed = (nanos as u64) ^ ((count as u64) << 40) ^ u64::from(std::process::id());
            let id = format!("p-{:010x}", mixed & 0xff_ffff_ffff);
            if self.profile(&id).is_none() {
                return id;
            }
        }
    }

    fn push_profile(&mut self, id: String, name: &str, server_url: &str) -> String {
        let name = clip(name, MAX_PROFILE_NAME_CHARS);
        self.profiles.push(ServerProfile {
            name: if name.is_empty() {
                default_profile_name(server_url)
            } else {
                name
            },
            server_url: server_url.trim().to_owned(),
            kind: ProfileKind::guess(server_url),
            group: id.clone(),
            id: id.clone(),
            ..ServerProfile::default()
        });
        id
    }

    /// Bring `active` / `profiles` in line with the old single-server fields (which win
    /// when they disagree, so changes made by an older client are kept). Idempotent;
    /// never removes a profile. Invalid URLs never become profiles.
    pub fn migrate_profiles(&mut self) -> ProfileMigration {
        let before = self.clone();
        let first = self.profiles.is_empty() && self.active.is_none();
        let url = self.server_url.trim().to_owned();
        let valid = !url.is_empty() && ServerAddress::parse(&url).is_ok();
        match self.mode {
            DataSourceMode::Server if valid => {
                let id = match self.profile_for_url(&url) {
                    Some(profile) => profile.id.clone(),
                    None => {
                        let id = if self.profile(DEFAULT_PROFILE_ID).is_none() {
                            DEFAULT_PROFILE_ID.to_owned()
                        } else {
                            self.new_profile_id()
                        };
                        self.push_profile(id, "", &url)
                    }
                };
                self.active = Some(id);
            }
            // Server mode with an invalid URL cannot be saved (validation fails); leave
            // the fields alone so the user can still correct them.
            DataSourceMode::Server => {}
            DataSourceMode::Local => {
                if valid && self.profile_for_url(&url).is_none() {
                    let id = if self.profile(DEFAULT_PROFILE_ID).is_none() {
                        DEFAULT_PROFILE_ID.to_owned()
                    } else {
                        self.new_profile_id()
                    };
                    self.push_profile(id, "", &url);
                }
                if !self.profiles.is_empty() || self.active.is_some() {
                    self.active = Some(ACTIVE_LOCAL.to_owned());
                }
            }
        }
        // Profiles saved before the per-profile transcode setting: the address default,
        // except that the old global "original quality" (`audition_original`) turns it off.
        let original = self.audition_original;
        for profile in &mut self.profiles {
            if profile.transcode.is_none() {
                profile.transcode = Some(!original && default_transcode(&profile.server_url));
            }
        }
        self.mirror_audition_original();
        if *self == before {
            ProfileMigration::Unchanged
        } else if first && !self.profiles.is_empty() {
            ProfileMigration::Created
        } else {
            ProfileMigration::Reconciled
        }
    }

    /// Write the active profile back into the old single-server fields: server →
    /// `mode = server` + its URL; local → `mode = local` (the URL of the most recently
    /// used server stays in `server_url` for older clients).
    pub fn mirror_active(&mut self) {
        match self
            .active_profile()
            .map(|profile| profile.server_url.clone())
        {
            Some(url) => {
                self.mode = DataSourceMode::Server;
                self.server_url = url;
                self.mirror_audition_original();
            }
            // `active` unset: settings never migrated (nothing to mirror).
            None if self.active.is_none() => {}
            None => {
                self.active = Some(ACTIVE_LOCAL.to_owned());
                self.mode = DataSourceMode::Local;
            }
        }
    }

    /// Older builds read the global `audition_original` ("never transcode"): keep it equal
    /// to the active profile's choice. Never read back once profiles carry `transcode`.
    fn mirror_audition_original(&mut self) {
        if let Some(transcode) = self.active_profile().map(ServerProfile::transcode_enabled) {
            self.audition_original = !transcode;
        }
    }

    /// Make `source` active and mirror it into the old fields.
    pub fn set_active(&mut self, source: &ActiveSource) -> Result<()> {
        match source {
            ActiveSource::Local => self.active = Some(ACTIVE_LOCAL.to_owned()),
            ActiveSource::Profile(id) => {
                if self.profile(id).is_none() {
                    return Err(Error::NotFound(format!("Server profile {id}")));
                }
                self.active = Some(id.clone());
            }
        }
        self.mirror_active();
        Ok(())
    }

    /// Add a profile for a server URL (normalised to its base URL); returns its id.
    /// `name` empty uses the host. The new profile is its own group.
    pub fn add_profile(&mut self, name: &str, server_url: &str) -> Result<String> {
        let address = ServerAddress::parse(server_url.trim())
            .map_err(|error| Error::Validation(error.to_string()))?;
        if self.profiles.len() >= MAX_PROFILES {
            return Err(Error::Validation(format!(
                "At most {MAX_PROFILES} server profiles can be saved"
            )));
        }
        let id = self.new_profile_id();
        let url = address.base_url();
        let id = self.push_profile(id, name, &url);
        if let Some(profile) = self.profile_mut(&id) {
            profile.transcode = Some(default_transcode(&url));
        }
        Ok(id)
    }

    /// Remove a profile (an active one switches the settings to local mode). Returns the
    /// removed profile; its token must be removed from the secrets file by the caller.
    pub fn remove_profile(&mut self, id: &str) -> Result<ServerProfile> {
        let index = self
            .profiles
            .iter()
            .position(|profile| profile.id == id)
            .ok_or_else(|| Error::NotFound(format!("Server profile {id}")))?;
        let was_active = self.active.as_deref() == Some(id);
        let removed = self.profiles.remove(index);
        if was_active {
            self.active = Some(ACTIVE_LOCAL.to_owned());
            self.mirror_active();
        }
        Ok(removed)
    }

    /// "Use the external address" (§6.2 `public_url` from `/health`): create, or update,
    /// the external sibling of profile `from` instead of overwriting its (LAN) address.
    ///
    /// The sibling shares `from`'s group and library id. An existing external profile of
    /// that group gets the new URL; a profile already saved with that URL joins the group;
    /// otherwise a new profile named `name` (or `<from name> (external)`) is added.
    /// Returns the sibling's id; `active` is not changed.
    pub fn upsert_external_profile(
        &mut self,
        from: &str,
        public_url: &str,
        name: Option<&str>,
    ) -> Result<String> {
        let address = ServerAddress::parse(public_url.trim())
            .map_err(|error| Error::Validation(error.to_string()))?;
        let url = address.base_url();
        let source = self
            .profile(from)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("Server profile {from}")))?;
        if same_server_url(&source.server_url, &url) {
            return Err(Error::Validation(
                "The external address is the profile's own address".into(),
            ));
        }
        let group = source.group_key().to_owned();
        if let Some(profile) = self.profile_mut(from) {
            profile.group = group.clone();
        }
        let existing = self
            .profiles
            .iter()
            .find(|profile| profile.id != from && same_server_url(&profile.server_url, &url))
            .or_else(|| {
                self.profiles.iter().find(|profile| {
                    profile.id != from
                        && profile.kind == ProfileKind::External
                        && profile.group_key() == group
                })
            })
            .map(|profile| profile.id.clone());
        let id = match existing {
            Some(id) => id,
            None => {
                if self.profiles.len() >= MAX_PROFILES {
                    return Err(Error::Validation(format!(
                        "At most {MAX_PROFILES} server profiles can be saved"
                    )));
                }
                let id = self.new_profile_id();
                let fallback = format!("{} (external)", source.name);
                let id = self.push_profile(id, name.unwrap_or(&fallback), &url);
                // An external address is for the internet: transcode by default.
                if let Some(profile) = self.profile_mut(&id) {
                    profile.transcode = Some(true);
                }
                id
            }
        };
        let profile = self.profile_mut(&id).expect("the sibling profile exists");
        profile.server_url = url;
        profile.kind = ProfileKind::External;
        profile.group = group;
        if profile.library_id.is_none() {
            profile.library_id = source.library_id.clone();
        }
        if self.active.as_deref() == Some(id.as_str()) {
            self.mirror_active();
        }
        Ok(id)
    }

    /// Store a profile's latest test verdict (one line, clipped) and library identity.
    pub fn record_test(
        &mut self,
        id: &str,
        ok: bool,
        summary: &str,
        at: u64,
        library_id: Option<&str>,
    ) -> Result<()> {
        let profile = self
            .profile_mut(id)
            .ok_or_else(|| Error::NotFound(format!("Server profile {id}")))?;
        profile.last_test = Some(TestSummary {
            at,
            ok,
            summary: clip(summary, MAX_TEST_SUMMARY_CHARS),
        });
        if let Some(library_id) = library_id.map(str::trim).filter(|id| !id.is_empty()) {
            profile.library_id = Some(library_id.to_owned());
        }
        Ok(())
    }

    /// Validate the profile list (called by [`DataSourceSettings::validate`]).
    pub(crate) fn validate_profiles(&self) -> Result<()> {
        if self.profiles.len() > MAX_PROFILES {
            return Err(Error::Validation(format!(
                "At most {MAX_PROFILES} server profiles can be saved"
            )));
        }
        let mut seen = std::collections::BTreeSet::new();
        for profile in &self.profiles {
            if !valid_id(&profile.id) {
                return Err(Error::Validation(format!(
                    "Server profile id {:?} is invalid",
                    profile.id
                )));
            }
            if !seen.insert(profile.id.as_str()) {
                return Err(Error::Validation(format!(
                    "Server profile id {} is used twice",
                    profile.id
                )));
            }
            if profile.name.chars().count() > MAX_PROFILE_NAME_CHARS
                || profile.name.chars().any(char::is_control)
            {
                return Err(Error::Validation(format!(
                    "Server profile name of {} is invalid (at most {MAX_PROFILE_NAME_CHARS} characters, one line)",
                    profile.id
                )));
            }
            ServerAddress::parse(&profile.server_url).map_err(|error| {
                Error::Validation(format!("Server profile {}: {error}", profile.id))
            })?;
            if let Some(webdav) = &profile.webdav {
                crate::config::validate_webdav(webdav)?;
            }
            if profile
                .last_test
                .as_ref()
                .is_some_and(|test| test.summary.chars().count() > MAX_TEST_SUMMARY_CHARS * 2)
            {
                return Err(Error::Validation(format!(
                    "Server profile {}: test summary is too long",
                    profile.id
                )));
            }
        }
        if let Some(active) = self.active.as_deref()
            && active != ACTIVE_LOCAL
            && self.profile(active).is_none()
        {
            return Err(Error::Validation(format!(
                "Active data source {active} is not a saved server profile"
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;

    const LAN: &str = "http://192.168.1.10:8790";
    const EXT: &str = "https://audiolib.example.com:18086";

    fn server(url: &str) -> DataSourceSettings {
        DataSourceSettings {
            mode: DataSourceMode::Server,
            server_url: url.into(),
            ..DataSourceSettings::default()
        }
    }

    /// The fields an older client knows; it ignores everything else.
    #[derive(serde::Deserialize)]
    struct OldClientView {
        mode: DataSourceMode,
        server_url: String,
    }

    #[test]
    fn single_server_settings_migrate_to_one_profile_with_a_backup() {
        let directory = tempfile::tempdir().unwrap();
        let original = r#"{"language":"cn","data_source":{"mode":"server","server_url":"http://192.168.1.10:8790","webdav":{"username":"sound"},"poll_interval_secs":120,"auto_sync":true}}"#;
        std::fs::write(directory.path().join("settings.json"), original).unwrap();
        let config = AppConfig::load_in(directory.path().to_owned()).unwrap();
        let source = &config.data_source;
        assert_eq!(source.active.as_deref(), Some(DEFAULT_PROFILE_ID));
        assert_eq!(source.profiles.len(), 1);
        let profile = &source.profiles[0];
        assert_eq!(
            (profile.id.as_str(), profile.server_url.as_str()),
            (DEFAULT_PROFILE_ID, LAN)
        );
        assert_eq!(profile.name, "192.168.1.10:8790");
        assert_eq!(profile.kind, ProfileKind::Lan);
        assert_eq!(profile.group_key(), DEFAULT_PROFILE_ID);
        // The old fields are untouched and the global settings stay global.
        assert_eq!(
            (source.mode, source.server_url.as_str()),
            (DataSourceMode::Server, LAN)
        );
        assert_eq!(source.webdav.username, "sound");
        assert_eq!((source.poll_interval_secs, source.auto_sync), (120, true));
        assert_eq!(
            source.active_source(),
            ActiveSource::Profile(DEFAULT_PROFILE_ID.into())
        );
        source.validate().unwrap();
        // One-time backup of the pre-migration file.
        let backup = directory.path().join("settings.json.pre-profiles");
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), original);
        config.save().unwrap();
        let saved = std::fs::read_to_string(directory.path().join("settings.json")).unwrap();
        assert!(saved.contains("\"profiles\"") && saved.contains("\"active\": \"default\""));
        let old: serde_json::Value = serde_json::from_str(&saved).unwrap();
        let old: OldClientView = serde_json::from_value(old["data_source"].clone()).unwrap();
        assert_eq!(
            (old.mode, old.server_url.as_str()),
            (DataSourceMode::Server, LAN)
        );
        let reloaded = AppConfig::load_in(directory.path().to_owned()).unwrap();
        assert_eq!(reloaded.data_source, config.data_source);
        // Loading again neither migrates nor rewrites the backup.
        std::fs::write(&backup, "kept").unwrap();
        let mut again = reloaded.data_source.clone();
        assert_eq!(again.migrate_profiles(), ProfileMigration::Unchanged);
        drop(AppConfig::load_in(directory.path().to_owned()).unwrap());
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), "kept");
    }

    #[test]
    fn local_mode_keeps_its_last_server_as_an_inactive_profile() {
        let mut settings = DataSourceSettings {
            server_url: EXT.into(),
            ..DataSourceSettings::default()
        };
        assert_eq!(settings.migrate_profiles(), ProfileMigration::Created);
        assert_eq!(settings.active.as_deref(), Some(ACTIVE_LOCAL));
        assert_eq!(settings.active_source(), ActiveSource::Local);
        assert_eq!(settings.profiles[0].kind, ProfileKind::External);
        assert_eq!(
            settings.legacy_profile_id().as_deref(),
            Some(DEFAULT_PROFILE_ID)
        );
        assert_eq!(settings.mode, DataSourceMode::Local);
        // Pristine settings and invalid local-mode input never create profiles.
        let mut pristine = DataSourceSettings::default();
        assert_eq!(pristine.migrate_profiles(), ProfileMigration::Unchanged);
        assert_eq!(pristine, DataSourceSettings::default());
        let mut typo = DataSourceSettings {
            server_url: "htp:/oops".into(),
            ..DataSourceSettings::default()
        };
        assert_eq!(typo.migrate_profiles(), ProfileMigration::Unchanged);
        assert!(typo.profiles.is_empty() && typo.validate().is_ok());
        // Server mode with an invalid URL is left for validation to report.
        let mut broken = server("ftp://host");
        assert_eq!(broken.migrate_profiles(), ProfileMigration::Unchanged);
        assert!(broken.validate().is_err());
    }

    #[test]
    fn edits_by_an_older_client_win_and_never_drop_profiles() {
        let mut settings = server(LAN);
        settings.migrate_profiles();
        let ext = settings
            .upsert_external_profile(DEFAULT_PROFILE_ID, EXT, None)
            .unwrap();
        settings
            .set_active(&ActiveSource::Profile(ext.clone()))
            .unwrap();
        assert_eq!(
            (settings.mode, settings.server_url.as_str()),
            (DataSourceMode::Server, EXT)
        );
        // An older client switched back to the LAN URL: that profile becomes active.
        let mut changed = settings.clone();
        changed.server_url = format!("{LAN}/");
        assert_eq!(changed.migrate_profiles(), ProfileMigration::Reconciled);
        assert_eq!(changed.active.as_deref(), Some(DEFAULT_PROFILE_ID));
        assert_eq!(changed.profiles.len(), 2);
        // ... typed a new server: a new active profile, the others stay.
        let mut changed = settings.clone();
        changed.server_url = "http://10.0.0.9:8790".into();
        assert_eq!(changed.migrate_profiles(), ProfileMigration::Reconciled);
        assert_eq!(changed.profiles.len(), 3);
        let active = changed.active_profile().unwrap();
        assert_eq!(active.server_url, "http://10.0.0.9:8790");
        assert_ne!(active.id, DEFAULT_PROFILE_ID);
        assert_eq!(changed.migrate_profiles(), ProfileMigration::Unchanged);
        changed.validate().unwrap();
        // ... switched to local mode.
        let mut changed = settings.clone();
        changed.mode = DataSourceMode::Local;
        assert_eq!(changed.migrate_profiles(), ProfileMigration::Reconciled);
        assert_eq!(changed.active_source(), ActiveSource::Local);
        assert_eq!(changed.server_url, EXT);
        assert_eq!(changed.legacy_profile_id(), Some(ext.clone()));
        // An older client also drops `profiles`/`active` when it saves: the old fields
        // are still a working single-server setting and migrate again.
        let json = serde_json::json!({"mode": "server", "server_url": EXT});
        let mut downgraded: DataSourceSettings = serde_json::from_value(json).unwrap();
        assert_eq!(downgraded.migrate_profiles(), ProfileMigration::Created);
        assert_eq!(downgraded.active_profile().unwrap().server_url, EXT);
    }

    #[test]
    fn mirroring_follows_the_active_profile() {
        let mut settings = server(LAN);
        settings.migrate_profiles();
        let other = settings
            .add_profile("办公室", "https://office.example.com/")
            .unwrap();
        assert_eq!(
            settings.profile(&other).unwrap().server_url,
            "https://office.example.com"
        );
        settings
            .set_active(&ActiveSource::Profile(other.clone()))
            .unwrap();
        assert_eq!(settings.server_url, "https://office.example.com");
        assert_eq!(settings.mode, DataSourceMode::Server);
        settings.set_active(&ActiveSource::Local).unwrap();
        assert_eq!(settings.mode, DataSourceMode::Local);
        assert_eq!(settings.server_url, "https://office.example.com");
        assert_eq!(settings.migrate_profiles(), ProfileMigration::Unchanged);
        assert!(
            settings
                .set_active(&ActiveSource::Profile("missing".into()))
                .is_err()
        );
        // Removing the active profile falls back to local mode.
        settings
            .set_active(&ActiveSource::Profile(other.clone()))
            .unwrap();
        settings.remove_profile(&other).unwrap();
        assert_eq!(settings.active_source(), ActiveSource::Local);
        assert_eq!(settings.mode, DataSourceMode::Local);
        assert!(settings.remove_profile(&other).is_err());
        assert!(settings.add_profile("", "no-scheme").is_err());
    }

    #[test]
    fn external_address_creates_or_updates_a_sibling_profile() {
        let mut settings = server(LAN);
        settings.migrate_profiles();
        settings
            .record_test(
                DEFAULT_PROFILE_ID,
                true,
                "ok · 0.3.6\nr12",
                1_791_460_727,
                Some("lib-1"),
            )
            .unwrap();
        let ext = settings
            .upsert_external_profile(DEFAULT_PROFILE_ID, &format!("{EXT}/"), Some("家里（外网）"))
            .unwrap();
        let lan = settings.profile(DEFAULT_PROFILE_ID).unwrap().clone();
        let sibling = settings.profile(&ext).unwrap().clone();
        assert_eq!(lan.server_url, LAN, "the LAN profile is not overwritten");
        assert_eq!(lan.last_test.as_ref().unwrap().summary, "ok · 0.3.6 r12");
        assert_eq!(sibling.server_url, EXT);
        assert_eq!(sibling.name, "家里（外网）");
        assert_eq!(sibling.kind, ProfileKind::External);
        assert_eq!(sibling.group_key(), lan.group_key());
        assert_eq!(sibling.library_id.as_deref(), Some("lib-1"));
        assert_eq!(settings.active.as_deref(), Some(DEFAULT_PROFILE_ID));
        assert_eq!(settings.server_url, LAN);
        // A changed public_url updates the same sibling.
        let again = settings
            .upsert_external_profile(DEFAULT_PROFILE_ID, "https://new.example.com", None)
            .unwrap();
        assert_eq!(again, ext);
        assert_eq!(settings.profiles.len(), 2);
        assert_eq!(
            settings.profile(&ext).unwrap().server_url,
            "https://new.example.com"
        );
        assert_eq!(settings.profile(&ext).unwrap().name, "家里（外网）");
        // Its own address is refused; a profile already saved with the URL joins the group.
        assert!(
            settings
                .upsert_external_profile(DEFAULT_PROFILE_ID, LAN, None)
                .is_err()
        );
        let saved = settings
            .add_profile("vps", "https://vps.example.net")
            .unwrap();
        let mut grouped = settings.clone();
        grouped.profile_mut(&ext).unwrap().kind = ProfileKind::Lan;
        let joined = grouped
            .upsert_external_profile(DEFAULT_PROFILE_ID, "https://vps.example.net", None)
            .unwrap();
        assert_eq!(joined, saved);
        assert_eq!(grouped.profile(&saved).unwrap().group, DEFAULT_PROFILE_ID);
        // Updating the active sibling mirrors its new URL.
        settings
            .set_active(&ActiveSource::Profile(ext.clone()))
            .unwrap();
        settings
            .upsert_external_profile(DEFAULT_PROFILE_ID, EXT, None)
            .unwrap();
        assert_eq!(settings.server_url, EXT);
        settings.validate().unwrap();
        assert!(
            settings
                .upsert_external_profile("missing", EXT, None)
                .is_err()
        );
    }

    #[test]
    fn profiles_are_validated() {
        let mut settings = server(LAN);
        settings.migrate_profiles();
        settings.validate().unwrap();
        let mut duplicate = settings.clone();
        duplicate.profiles.push(duplicate.profiles[0].clone());
        assert!(duplicate.validate().is_err());
        let mut reserved = settings.clone();
        reserved.profiles[0].id = ACTIVE_LOCAL.into();
        assert!(reserved.validate().is_err());
        let mut dangling = settings.clone();
        dangling.active = Some("gone".into());
        assert!(dangling.validate().is_err());
        let mut bad_url = settings.clone();
        bad_url.profiles[0].server_url = "nope".into();
        assert!(bad_url.validate().is_err());
        let mut long_name = settings.clone();
        long_name.profiles[0].name = "名".repeat(MAX_PROFILE_NAME_CHARS + 1);
        assert!(long_name.validate().is_err());
        let mut webdav = settings.clone();
        webdav.profiles[0].webdav = Some(WebDavSettings {
            base_url: Some("http:// spaced".into()),
            ..WebDavSettings::default()
        });
        assert!(webdav.validate().is_err());
        // Unknown profile fields from a newer client are ignored; missing ones default.
        let parsed: ServerProfile =
            serde_json::from_str(r#"{"id":"p-1","url":"http://h:1","future":[1]}"#).unwrap();
        assert_eq!(parsed.server_url, "http://h:1");
        assert_eq!(parsed.kind, ProfileKind::Lan);
    }

    #[test]
    fn address_kinds_are_guessed_from_the_host() {
        for (url, kind) in [
            ("http://192.168.1.10:8790", ProfileKind::Lan),
            ("http://10.1.2.3", ProfileKind::Lan),
            ("http://127.0.0.1:8790", ProfileKind::Lan),
            ("http://[fd00::1]:8790", ProfileKind::Lan),
            ("http://nas.example.lan:8790", ProfileKind::Lan),
            ("http://nas:8790", ProfileKind::Lan),
            ("https://audiolib.example.com:18086", ProfileKind::External),
            ("https://8.8.8.8", ProfileKind::External),
            ("http://[2001:db8::1]", ProfileKind::External),
        ] {
            assert_eq!(ProfileKind::guess(url), kind, "{url}");
        }
        assert!(same_server_url("http://H:8790/", "http://h:8790"));
        assert!(!same_server_url("http://h:8790", "https://h:8790"));
    }

    #[test]
    fn transcode_defaults_follow_the_address() {
        for (url, transcode) in [
            ("http://192.168.1.10:8790", false),
            ("http://10.0.0.2", false),
            ("http://172.16.5.1:8790", false),
            ("http://nas.example.lan:8790", false),
            ("http://nas:8790", false),
            ("http://127.0.0.1:8790", false),
            ("https://audiolib.example.com:18086", true),
            ("http://audiolib.example.com:8790", true),
            ("https://nas.local:8443", true),
            ("http://8.8.8.8:8790", true),
        ] {
            assert_eq!(default_transcode(url), transcode, "{url}");
        }
    }

    #[test]
    fn transcode_is_migrated_per_profile_and_folds_the_old_original_switch() {
        // A single-server LAN file: one profile, transcode off; mirrored for older builds.
        let mut lan = server("http://192.168.1.10:8790");
        lan.migrate_profiles();
        assert_eq!(lan.profiles[0].transcode, Some(false));
        assert!(!lan.audition_transcode() && lan.audition_original);
        // An external single-server file: on.
        let mut external = server("https://audiolib.example.com:18086");
        external.migrate_profiles();
        assert_eq!(external.profiles[0].transcode, Some(true));
        assert!(external.audition_transcode() && !external.audition_original);
        // `audition_original = true` from an earlier build: off, even for an external URL.
        let mut original = server("https://audiolib.example.com:18086");
        original.audition_original = true;
        original.migrate_profiles();
        assert_eq!(original.profiles[0].transcode, Some(false));
        assert!(!original.audition_transcode());
        // Idempotent: the second pass changes nothing.
        let settled = original.clone();
        assert_eq!(original.migrate_profiles(), ProfileMigration::Unchanged);
        assert_eq!(original, settled);
        // Profiles saved before the field (no `transcode` key) get their address default;
        // a stored choice is kept, and a key-less file parses as before.
        let mut profiles: DataSourceSettings = serde_json::from_str(
            r#"{"mode":"server","server_url":"http://192.168.1.10:8790","active":"default",
                "profiles":[{"id":"default","server_url":"http://192.168.1.10:8790"},
                            {"id":"ext","server_url":"https://audiolib.example.com:18086","kind":"external"},
                            {"id":"mine","server_url":"http://10.0.0.9:8790","transcode":true}]}"#,
        )
        .unwrap();
        assert!(
            profiles
                .profiles
                .iter()
                .take(2)
                .all(|p| p.transcode.is_none())
        );
        assert!(profiles.profiles[1].transcode_enabled());
        profiles.migrate_profiles();
        let values: Vec<_> = profiles.profiles.iter().map(|p| p.transcode).collect();
        assert_eq!(values, [Some(false), Some(true), Some(true)]);
        // Switching the active profile mirrors its choice into the legacy field.
        profiles
            .set_active(&ActiveSource::Profile("ext".into()))
            .unwrap();
        assert!(profiles.audition_transcode() && !profiles.audition_original);
        profiles.set_active(&ActiveSource::Local).unwrap();
        assert!(
            !profiles.audition_original,
            "local mode keeps the last value"
        );
        // The legacy field is never read back once profiles carry the choice.
        profiles
            .set_active(&ActiveSource::Profile("default".into()))
            .unwrap();
        profiles.audition_original = false;
        profiles.migrate_profiles();
        assert!(!profiles.audition_transcode() && profiles.audition_original);
        // Unmigrated settings without profiles fall back to the address and the old switch.
        let mut bare = server("https://audiolib.example.com:18086");
        assert!(bare.audition_transcode());
        bare.audition_original = true;
        assert!(!bare.audition_transcode());
    }

    #[test]
    fn new_and_external_profiles_get_their_transcode_default() {
        let mut settings = server("http://192.168.1.10:8790");
        settings.migrate_profiles();
        let lan = settings.add_profile("", "http://10.0.0.5:8790").unwrap();
        let public = settings
            .add_profile("", "http://audiolib.example.com:18086")
            .unwrap();
        assert_eq!(settings.profile(&lan).unwrap().transcode, Some(false));
        assert_eq!(settings.profile(&public).unwrap().transcode, Some(true));
        // "Use the external address": the new sibling transcodes, the LAN profile does not.
        let sibling = settings
            .upsert_external_profile(
                DEFAULT_PROFILE_ID,
                "https://audiolib.example.com:18086",
                None,
            )
            .unwrap();
        assert_eq!(settings.profile(&sibling).unwrap().transcode, Some(true));
        assert_eq!(
            settings.profile(DEFAULT_PROFILE_ID).unwrap().transcode,
            Some(false)
        );
        // A user's choice on an existing sibling survives a repeated "use external address".
        settings.profile_mut(&sibling).unwrap().transcode = Some(false);
        let again = settings
            .upsert_external_profile(
                DEFAULT_PROFILE_ID,
                "https://audiolib.example.com:18086",
                None,
            )
            .unwrap();
        assert_eq!(again, sibling);
        assert_eq!(settings.profile(&sibling).unwrap().transcode, Some(false));
        assert!(settings.validate().is_ok());
    }
}
