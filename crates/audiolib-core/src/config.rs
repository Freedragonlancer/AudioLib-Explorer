// crates/audiolib-core/src/config.rs
//! Portable defaults and one-time, read-only interpretation of legacy YAML settings.
use crate::{Error, Result, SortOrder};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub use crate::profiles::{
    ACTIVE_LOCAL, ActiveSource, DEFAULT_PROFILE_ID, ProfileKind, ProfileMigration, ServerProfile,
    TestSummary, default_transcode,
};

/// Where the desktop reads library data from. Old settings files default to `Local`.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DataSourceMode {
    /// The local SQLite library and local asset paths (the original behaviour).
    #[default]
    Local,
    /// A read-only cached snapshot of an AudioLib server library, audio over WebDAV.
    Server,
}

/// Non-secret WebDAV preferences. The password is stored separately by audiolib-remote.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct WebDavSettings {
    /// Quick fallback: `<base_url>/<root name>/<relative path>` when neither a user
    /// override nor a server-provided URL exists for a scan root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default)]
    pub username: String,
    /// User WebDAV URL per server scan root name. An entry overrides the server's
    /// suggested `webdav_url`; a missing or empty entry uses the server default.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub root_urls: BTreeMap<String, String>,
}

/// Default seconds between background server revision checks.
pub const DEFAULT_POLL_INTERVAL_SECS: u32 = 60;
/// Smallest accepted non-zero revision check interval.
pub const MIN_POLL_INTERVAL_SECS: u32 = 10;
/// Largest accepted revision check interval (one day).
pub const MAX_POLL_INTERVAL_SECS: u32 = 86_400;

fn default_poll_interval() -> u32 {
    DEFAULT_POLL_INTERVAL_SECS
}

/// Default memory for server-mode audition streams (MiB).
pub const DEFAULT_AUDITION_MEMORY_MB: u32 = 512;
/// Accepted range of [`DataSourceSettings::audition_memory_mb`].
pub const MIN_AUDITION_MEMORY_MB: u32 = 32;
pub const MAX_AUDITION_MEMORY_MB: u32 = 8192;

fn default_audition_memory() -> u32 {
    DEFAULT_AUDITION_MEMORY_MB
}

/// Default maximum client-local playback events retained separately for each library.
pub const PLAY_HISTORY_LIMIT_PER_LIBRARY: u32 = 5_000;

fn default_play_history_limit() -> u32 {
    PLAY_HISTORY_LIMIT_PER_LIBRARY
}

/// Data source preferences persisted in settings.json. Secrets (API token and WebDAV
/// password) are deliberately absent: they live in a protected sidecar file.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DataSourceSettings {
    #[serde(default)]
    pub mode: DataSourceMode,
    /// Server base URL such as `http://server.example.lan:8790` or
    /// `https://audiolib.example.com:8443/audiolib`; `/api/v1` is appended by clients
    /// (see [`crate::server_url`]).
    #[serde(default)]
    pub server_url: String,
    #[serde(default)]
    pub webdav: WebDavSettings,
    /// Seconds between background `GET /library` revision checks in server mode;
    /// `0` disables periodic checks (manual refresh still checks).
    #[serde(default = "default_poll_interval")]
    pub poll_interval_secs: u32,
    /// Download and open a new snapshot automatically when the server revision or
    /// library identity changes, instead of only showing a notice with a Sync action.
    #[serde(default)]
    pub auto_sync: bool,
    /// Memory (MiB) for server-mode audition: streams are held in memory, least recently
    /// used first out, never written to disk.
    #[serde(default = "default_audition_memory")]
    pub audition_memory_mb: u32,
    /// Legacy "always audition the original" switch of earlier builds. Superseded by the
    /// per-profile [`crate::profiles::ServerProfile::transcode`]: read once when profiles
    /// are migrated (`true` → transcode off), afterwards written as the inverse of the
    /// active profile's choice so older builds behave the same.
    #[serde(default)]
    pub audition_original: bool,
    /// Active data source: [`ACTIVE_LOCAL`] or a profile id; `None` in settings written
    /// before profiles existed (see [`DataSourceSettings::migrate_profiles`]). `mode` and
    /// `server_url` above mirror it for older clients.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<String>,
    /// Saved server connections (LAN and external addresses are separate profiles).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub profiles: Vec<ServerProfile>,
}

impl Default for DataSourceSettings {
    fn default() -> Self {
        Self {
            mode: DataSourceMode::default(),
            server_url: String::new(),
            webdav: WebDavSettings::default(),
            poll_interval_secs: DEFAULT_POLL_INTERVAL_SECS,
            auto_sync: false,
            audition_memory_mb: DEFAULT_AUDITION_MEMORY_MB,
            audition_original: false,
            active: None,
            profiles: Vec::new(),
        }
    }
}

impl DataSourceSettings {
    /// Validate server-mode fields before they are saved or used for a connection.
    pub fn validate(&self) -> Result<()> {
        if self.mode == DataSourceMode::Server {
            crate::server_url::ServerAddress::parse(&self.server_url)?;
        }
        if self.poll_interval_secs != 0
            && !(MIN_POLL_INTERVAL_SECS..=MAX_POLL_INTERVAL_SECS).contains(&self.poll_interval_secs)
        {
            return Err(Error::Validation(format!(
                "Revision check interval must be 0 (off) or {MIN_POLL_INTERVAL_SECS}..={MAX_POLL_INTERVAL_SECS} seconds"
            )));
        }
        if !(MIN_AUDITION_MEMORY_MB..=MAX_AUDITION_MEMORY_MB).contains(&self.audition_memory_mb) {
            return Err(Error::Validation(format!(
                "Audition memory must be {MIN_AUDITION_MEMORY_MB}..={MAX_AUDITION_MEMORY_MB} MiB"
            )));
        }
        if let Some(base) = self
            .webdav
            .base_url
            .as_deref()
            .filter(|url| !url.is_empty())
        {
            validate_http_url(base, "WebDAV base URL")?;
        }
        for (name, url) in &self.webdav.root_urls {
            if !url.trim().is_empty() {
                validate_http_url(url, &format!("WebDAV URL for {name}"))?;
            }
        }
        if self.webdav.username.len() > 256 || self.webdav.username.contains(['\r', '\n']) {
            return Err(Error::Validation("WebDAV username is invalid".into()));
        }
        self.validate_profiles()?;
        Ok(())
    }

    /// Why server mode cannot start from these settings, or `None` when it can: server
    /// mode needs `active` to name an existing profile whose `server_url` is non-empty
    /// and parses as a server address. Local mode always passes.
    pub fn server_mode_problem(&self) -> Option<String> {
        if self.mode != DataSourceMode::Server {
            return None;
        }
        let Some(active) = self.active.as_deref() else {
            return Some("no active server profile is selected".into());
        };
        let Some(profile) = self.active_profile() else {
            return Some(format!("active server profile {active:?} does not exist"));
        };
        let url = profile.server_url.trim();
        if url.is_empty() {
            return Some(format!("server profile {:?} has no server URL", profile.id));
        }
        crate::server_url::ServerAddress::parse(url)
            .err()
            .map(|error| {
                format!(
                    "server profile {:?} has an invalid URL: {error}",
                    profile.id
                )
            })
    }

    /// Startup guard: switch an unusable server selection to local mode in memory and
    /// return the reason for the caller to log. Settings on disk and saved profiles are
    /// not changed here.
    pub fn guard_server_mode(&mut self) -> Option<String> {
        let problem = self.server_mode_problem()?;
        self.mode = DataSourceMode::Local;
        Some(problem)
    }
}

/// Validate one set of WebDAV preferences (a profile override has the global shape).
pub(crate) fn validate_webdav(webdav: &WebDavSettings) -> Result<()> {
    if let Some(base) = webdav.base_url.as_deref().filter(|url| !url.is_empty()) {
        validate_http_url(base, "WebDAV base URL")?;
    }
    for (name, url) in &webdav.root_urls {
        if !url.trim().is_empty() {
            validate_http_url(url, &format!("WebDAV URL for {name}"))?;
        }
    }
    if webdav.username.len() > 256 || webdav.username.contains(['\r', '\n']) {
        return Err(Error::Validation("WebDAV username is invalid".into()));
    }
    Ok(())
}

/// Accept only absolute http(s) URLs with a host and no embedded credentials.
pub fn validate_http_url(url: &str, label: &str) -> Result<()> {
    let trimmed = url.trim();
    let rest = trimmed
        .strip_prefix("http://")
        .or_else(|| trimmed.strip_prefix("https://"))
        .ok_or_else(|| Error::Validation(format!("{label} must start with http:// or https://")))?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.is_empty()
        || authority.contains('@')
        || trimmed.len() > 2048
        || trimmed.chars().any(char::is_whitespace)
    {
        return Err(Error::Validation(format!(
            "{label} requires a host and must not contain spaces or credentials"
        )));
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppPaths {
    pub database: PathBuf,
    pub cache: PathBuf,
    pub logs: PathBuf,
    pub settings: PathBuf,
    pub legacy_database: Option<PathBuf>,
    pub audio_terms: Option<PathBuf>,
}

/// Audio result table widths in logical pixels and the library-wide search sort.
/// Missing preferences retain the default layout, while all widths stay within safe bounds.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AudioTablePreferences {
    /// File name column width, at least 120 logical pixels.
    #[serde(deserialize_with = "deserialize_name_width")]
    pub name_width: f32,
    /// Duration column width, at least 60 logical pixels.
    #[serde(deserialize_with = "deserialize_duration_width")]
    pub duration_width: f32,
    /// Sample rate and bit depth column width, at least 82 logical pixels.
    #[serde(deserialize_with = "deserialize_format_width")]
    pub format_width: f32,
    /// Channel count column width, at least 48 logical pixels.
    #[serde(deserialize_with = "deserialize_channels_width")]
    pub channels_width: f32,
    /// Tags column width, at least 80 logical pixels.
    #[serde(deserialize_with = "deserialize_tags_width")]
    pub tags_width: f32,
    /// Search ordering; this includes the existing Modified ordering.
    pub order_by: SortOrder,
    /// Reverse the selected ordering when true.
    pub descending: bool,
}

impl Default for AudioTablePreferences {
    fn default() -> Self {
        Self {
            name_width: 360.0,
            duration_width: 80.0,
            format_width: 100.0,
            channels_width: 64.0,
            tags_width: 160.0,
            order_by: SortOrder::Relevance,
            descending: false,
        }
    }
}

impl AudioTablePreferences {
    /// Return safe widths for UI use or persistence: finite values are clamped to
    /// each column's minimum and 2000 pixels, and non-finite values use its default.
    pub fn normalized(&self) -> Self {
        let defaults = Self::default();
        Self {
            name_width: normalize_table_width(self.name_width, 120.0, defaults.name_width),
            duration_width: normalize_table_width(
                self.duration_width,
                60.0,
                defaults.duration_width,
            ),
            format_width: normalize_table_width(self.format_width, 82.0, defaults.format_width),
            channels_width: normalize_table_width(
                self.channels_width,
                48.0,
                defaults.channels_width,
            ),
            tags_width: normalize_table_width(self.tags_width, 80.0, defaults.tags_width),
            order_by: self.order_by,
            descending: self.descending,
        }
    }
}

fn normalize_table_width(width: f32, minimum: f32, default: f32) -> f32 {
    if width.is_finite() {
        width.clamp(minimum, 2000.0)
    } else {
        default
    }
}

fn deserialize_table_width<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
    minimum: f32,
    default: f32,
) -> std::result::Result<f32, D::Error> {
    // Recover malformed UI-only widths without discarding otherwise valid library preferences.
    let value = serde_json::Value::deserialize(deserializer)?;
    let width = value.as_f64().map(|value| value as f32).unwrap_or(default);
    Ok(normalize_table_width(width, minimum, default))
}

fn deserialize_name_width<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<f32, D::Error> {
    deserialize_table_width(deserializer, 120.0, 360.0)
}

fn deserialize_duration_width<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<f32, D::Error> {
    deserialize_table_width(deserializer, 60.0, 80.0)
}

fn deserialize_format_width<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<f32, D::Error> {
    deserialize_table_width(deserializer, 82.0, 100.0)
}

fn deserialize_channels_width<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<f32, D::Error> {
    deserialize_table_width(deserializer, 48.0, 64.0)
}

fn deserialize_tags_width<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<f32, D::Error> {
    deserialize_table_width(deserializer, 80.0, 160.0)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppConfig {
    pub paths: AppPaths,
    pub language: String,
    pub playback: serde_json::Value,
    /// Start preview playback when an audio list item is selected by a single click.
    /// Disabled by default so established settings retain double-click audition.
    #[serde(default)]
    pub audition_play_on_single_click: bool,
    /// Client-local playback event limit per library; zero disables new records.
    /// History lives under the user data root, independently of libraries and caches.
    #[serde(default = "default_play_history_limit")]
    pub play_history_limit: u32,
    /// Local mode: pre-generate list thumbnail Waveforms in the background after scans
    /// (doc/api/README.md §9.3). Off by default: only visible rows are computed.
    #[serde(default)]
    pub pregenerate_list_waveforms: bool,
    /// Server mode: hold new `POST /api/v1/waveforms` batches until 3 s after the latest
    /// audition start (deferred, not dropped). Off by default.
    #[serde(default)]
    pub hold_list_waveform_batches_after_audition: bool,
    /// Persistent table layout and sorting, applied to every search page.
    #[serde(default)]
    pub audio_table: AudioTablePreferences,
    #[serde(default)]
    pub allowed_scan_roots: Vec<PathBuf>,
    #[serde(default)]
    pub data_source: DataSourceSettings,
    /// Set by [`AppConfig::guard_server_mode`] when an unusable server selection was
    /// opened as local for this session only. Never persisted; see [`AppConfig::save`].
    #[serde(skip)]
    pub data_source_fallback: Option<DataSourceFallback>,
}

/// The persisted data-source selection replaced in memory by the startup guard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataSourceFallback {
    pub mode: DataSourceMode,
    pub active: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct SavedPreferences {
    #[serde(default)]
    active_database: Option<PathBuf>,
    #[serde(default = "english")]
    language: String,
    #[serde(default = "default_playback")]
    playback: serde_json::Value,
    #[serde(default)]
    audition_play_on_single_click: bool,
    #[serde(default = "default_play_history_limit")]
    play_history_limit: u32,
    #[serde(default)]
    pregenerate_list_waveforms: bool,
    #[serde(default)]
    hold_list_waveform_batches_after_audition: bool,
    #[serde(default)]
    audio_table: AudioTablePreferences,
    #[serde(default)]
    allowed_scan_roots: Vec<PathBuf>,
    #[serde(default)]
    data_source: DataSourceSettings,
}
fn english() -> String {
    "en".into()
}
fn default_playback() -> serde_json::Value {
    serde_json::json!({"backend":"wasapi","device_id":null,"buffer_frames":0,
        "output_pair":0,"source_pair":0,"volume":0.8,"rate_policy":"device_default"})
}

impl AppConfig {
    /// Load user preferences, finding legacy settings without modifying the old library.
    pub fn load_default() -> Result<Self> {
        Self::load_in(Self::default_root()?)
    }

    /// The user data root: `AUDIOLIB_DATA_DIR`, else `%LOCALAPPDATA%/AudioLibExplorer/v1`.
    pub fn default_root() -> Result<PathBuf> {
        Ok(
            if let Some(override_path) = std::env::var_os("AUDIOLIB_DATA_DIR") {
                PathBuf::from(override_path)
            } else if let Some(local) = std::env::var_os("LOCALAPPDATA") {
                PathBuf::from(local).join("AudioLibExplorer").join("v1")
            } else {
                std::env::current_dir()?.join(".audiolib-user")
            },
        )
    }

    /// Directory holding settings.json and all per-user runtime state.
    pub fn data_root(&self) -> PathBuf {
        self.paths
            .settings
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
    }

    /// Load settings from an explicit user-data root; useful for isolated validation.
    pub fn load_in(root: PathBuf) -> Result<Self> {
        let mut roots = vec![std::env::current_dir()?];
        if let Ok(exe) = std::env::current_exe() {
            roots.extend(exe.ancestors().skip(1).take(5).map(PathBuf::from));
        }
        Self::load_with_roots(root, &roots)
    }

    fn load_with_roots(root: PathBuf, roots: &[PathBuf]) -> Result<Self> {
        let legacy_root = roots
            .iter()
            .find(|path| path.join("config/audiolib_config.yaml").is_file())
            .cloned();
        let settings = root.join("settings.json");
        let mut config = Self {
            paths: AppPaths {
                database: root.join("library.db"),
                cache: root.join("cache"),
                logs: root.join("logs"),
                settings,
                legacy_database: None,
                audio_terms: roots
                    .iter()
                    .map(|path| path.join("resources/audio_terms.txt"))
                    .find(|path| path.is_file()),
            },
            language: english(),
            playback: default_playback(),
            audition_play_on_single_click: false,
            play_history_limit: PLAY_HISTORY_LIMIT_PER_LIBRARY,
            pregenerate_list_waveforms: false,
            hold_list_waveform_batches_after_audition: false,
            audio_table: AudioTablePreferences::default(),
            allowed_scan_roots: Vec::new(),
            data_source: DataSourceSettings::default(),
            data_source_fallback: None,
        };
        // Established Rust state is independent of legacy YAML validity or presence.
        if !config.paths.settings.exists()
            && !config.paths.database.exists()
            && let Some(legacy_root) = legacy_root
        {
            let yaml_path = legacy_root.join("config/audiolib_config.yaml");
            let yaml: serde_yaml::Value = serde_yaml::from_slice(&std::fs::read(yaml_path)?)
                .map_err(|error| Error::Validation(format!("Legacy config: {error}")))?;
            let old_path = yaml["database"]["path"]
                .as_str()
                .unwrap_or("data/audiolib.db");
            let path = PathBuf::from(old_path);
            let path = if path.is_absolute() {
                path
            } else {
                legacy_root.join(path)
            };
            if path.is_file() && path != config.paths.database {
                config.paths.legacy_database = Some(path);
            }
            if let Some(language) = yaml["application"]["language"].as_str() {
                config.language = language.into();
            }
            if let Some(volume) = yaml["playback"]["default_volume"].as_f64()
                && volume.is_finite()
                && (0.0..=1.0).contains(&volume)
            {
                config.playback["volume"] = serde_json::json!(volume);
            }
            if let Some(roots) = yaml["security"]["allowed_scan_roots"].as_sequence() {
                config.allowed_scan_roots = roots
                    .iter()
                    .filter_map(|value| value.as_str().map(PathBuf::from))
                    .collect();
            }
        }
        if config.paths.settings.exists() {
            let saved: SavedPreferences =
                serde_json::from_slice(&std::fs::read(&config.paths.settings)?)
                    .map_err(|error| Error::Validation(format!("User settings: {error}")))?;
            config.language = saved.language;
            config.playback = saved.playback;
            config.audition_play_on_single_click = saved.audition_play_on_single_click;
            config.play_history_limit = saved.play_history_limit;
            config.pregenerate_list_waveforms = saved.pregenerate_list_waveforms;
            config.hold_list_waveform_batches_after_audition =
                saved.hold_list_waveform_batches_after_audition;
            config.audio_table = saved.audio_table.normalized();
            config.allowed_scan_roots = saved.allowed_scan_roots;
            config.data_source = saved.data_source;
            // Single-server settings become one profile; older-client edits of the old
            // fields are folded into the profiles. Persisted by the next save; the first
            // migration keeps a one-time copy of the original file.
            if config.data_source.migrate_profiles() == ProfileMigration::Created {
                let backup = config.paths.settings.with_extension("json.pre-profiles");
                if !backup.exists() {
                    let _ = std::fs::copy(&config.paths.settings, &backup);
                }
            }
            if let Some(path) = saved.active_database {
                config.paths.database = if path.is_absolute() {
                    path
                } else {
                    root.join(path)
                };
                config.paths.legacy_database = None;
            }
        }
        if config.language != "cn" && config.language != "en" {
            config.language = english();
        }
        for directory in [&root, &config.paths.cache, &config.paths.logs] {
            std::fs::create_dir_all(directory)?;
        }
        Ok(config)
    }

    /// Atomically persist new preferences separately from legacy YAML.
    pub fn save(&self) -> Result<()> {
        let saved = SavedPreferences {
            active_database: Some(if self.paths.database.is_absolute() {
                self.paths.database.clone()
            } else {
                std::env::current_dir()?.join(&self.paths.database)
            }),
            language: self.language.clone(),
            playback: self.playback.clone(),
            audition_play_on_single_click: self.audition_play_on_single_click,
            play_history_limit: self.play_history_limit,
            pregenerate_list_waveforms: self.pregenerate_list_waveforms,
            hold_list_waveform_batches_after_audition: self
                .hold_list_waveform_batches_after_audition,
            audio_table: self.audio_table.normalized(),
            allowed_scan_roots: self.allowed_scan_roots.clone(),
            data_source: {
                // Code that edits `mode` / `server_url` directly keeps working: the
                // profiles follow those fields before they are written.
                let mut data_source = self.persisted_data_source();
                data_source.migrate_profiles();
                data_source
            },
        };
        let data = serde_json::to_vec_pretty(&saved)
            .map_err(|error| Error::Validation(error.to_string()))?;
        let temporary = self
            .paths
            .settings
            .with_extension(format!("tmp-{}", std::process::id()));
        let mut file = std::fs::File::create(&temporary)?;
        use std::io::Write;
        file.write_all(&data)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, &self.paths.settings)?;
        Ok(())
    }

    /// Startup guard: open an unusable server selection as local for this session (see
    /// [`DataSourceSettings::server_mode_problem`]) and return the reason to log. The
    /// original `mode` / `active` are remembered so [`AppConfig::save`] keeps them on disk.
    pub fn guard_server_mode(&mut self) -> Option<String> {
        let fallback = DataSourceFallback {
            mode: self.data_source.mode,
            active: self.data_source.active.clone(),
        };
        let problem = self.data_source.guard_server_mode()?;
        self.data_source_fallback = Some(fallback);
        Some(problem)
    }

    /// The data source to write: while the startup fallback is in effect and the user has
    /// not selected a source in this session (still local with the original `active`), the
    /// original `mode` / `active` are written instead of the in-memory local fallback.
    fn persisted_data_source(&self) -> DataSourceSettings {
        let mut data_source = self.data_source.clone();
        if let Some(fallback) = &self.data_source_fallback
            && data_source.mode == DataSourceMode::Local
            && data_source.active == fallback.active
        {
            data_source.mode = fallback.mode;
            data_source.active = fallback.active.clone();
        }
        data_source
    }

    /// Select an existing, already validated library; saving remains an explicit step.
    pub fn select_database(&mut self, path: &Path) -> Result<()> {
        if !path.is_file() {
            return Err(Error::NotFound(format!(
                "Library database {}",
                path.display()
            )));
        }
        self.paths.database = path.canonicalize()?;
        self.paths.legacy_database = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_local_history_limit_defaults_and_round_trips_without_moving_data_root() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("preferences");
        let mut config = AppConfig::load_with_roots(root.clone(), &[]).unwrap();
        assert_eq!(config.play_history_limit, PLAY_HISTORY_LIMIT_PER_LIBRARY);
        let alternate = directory.path().join("elsewhere.db");
        std::fs::write(&alternate, []).unwrap();
        config.select_database(&alternate).unwrap();
        for limit in [0, 12_345] {
            config.play_history_limit = limit;
            config.save().unwrap();
            let loaded = AppConfig::load_with_roots(root.clone(), &[]).unwrap();
            assert_eq!(loaded.play_history_limit, limit);
            assert_eq!(loaded.data_root(), root);
            assert_eq!(loaded.paths.database, alternate.canonicalize().unwrap());
        }
        std::fs::write(root.join("settings.json"), r#"{"language":"cn"}"#).unwrap();
        assert_eq!(
            AppConfig::load_with_roots(root, &[])
                .unwrap()
                .play_history_limit,
            PLAY_HISTORY_LIMIT_PER_LIBRARY
        );
    }

    #[test]
    fn old_settings_keep_double_click_audition() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("settings.json"),
            r#"{"language":"cn","playback":{"volume":0.35},"allowed_scan_roots":[]}"#,
        )
        .unwrap();
        let config = AppConfig::load_with_roots(directory.path().to_owned(), &[]).unwrap();
        assert!(!config.audition_play_on_single_click);
        assert!(!config.pregenerate_list_waveforms);
        assert!(!config.hold_list_waveform_batches_after_audition);
        assert_eq!(config.audio_table, AudioTablePreferences::default());
        assert_eq!(config.language, "cn");
        assert_eq!(config.playback["volume"], serde_json::json!(0.35));
        config.save().unwrap();
        let loaded = AppConfig::load_with_roots(directory.path().to_owned(), &[]).unwrap();
        assert_eq!(loaded.audio_table, AudioTablePreferences::default());
        assert_eq!(loaded.language, config.language);
        assert_eq!(loaded.playback, config.playback);
    }

    #[test]
    fn table_preferences_round_trip_without_changing_existing_settings() {
        let directory = tempfile::tempdir().unwrap();
        let library = directory.path().join("selected.db");
        std::fs::write(&library, []).unwrap();
        let root = directory.path().join("userdata");
        let mut config = AppConfig::load_with_roots(root.clone(), &[]).unwrap();
        config.select_database(&library).unwrap();
        config.language = "cn".into();
        config.playback = serde_json::json!({"volume":0.35,"device_id":"custom-device"});
        config.audition_play_on_single_click = true;
        config.pregenerate_list_waveforms = true;
        config.hold_list_waveform_batches_after_audition = true;
        config.allowed_scan_roots = vec![directory.path().join("sounds")];
        config.data_source.webdav.username = "existing-user".into();
        config.audio_table = AudioTablePreferences {
            name_width: 452.5,
            duration_width: 90.0,
            format_width: 115.5,
            channels_width: 72.0,
            tags_width: 245.0,
            order_by: SortOrder::Modified,
            descending: true,
        };
        config.save().unwrap();
        let loaded = AppConfig::load_with_roots(root, &[]).unwrap();
        assert_eq!(loaded.audio_table, config.audio_table);
        assert_eq!(loaded.paths.database, library.canonicalize().unwrap());
        assert_eq!(loaded.language, config.language);
        assert_eq!(loaded.playback, config.playback);
        assert_eq!(
            loaded.audition_play_on_single_click,
            config.audition_play_on_single_click
        );
        assert!(loaded.pregenerate_list_waveforms);
        assert!(loaded.hold_list_waveform_batches_after_audition);
        assert_eq!(loaded.allowed_scan_roots, config.allowed_scan_roots);
        assert_eq!(loaded.data_source, config.data_source);
    }

    #[test]
    fn malformed_and_extreme_table_widths_recover_without_losing_settings() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("settings.json"),
            r#"{"language":"cn","playback":{"volume":0.35},"audio_table":{
                "name_width":"wide","duration_width":null,"format_width":-15,
                "channels_width":10000000000,"tags_width":{},
                "order_by":"modified","descending":true}}"#,
        )
        .unwrap();
        let config = AppConfig::load_with_roots(directory.path().to_owned(), &[]).unwrap();
        assert_eq!(
            config.audio_table,
            AudioTablePreferences {
                name_width: 360.0,
                duration_width: 80.0,
                format_width: 82.0,
                channels_width: 2000.0,
                tags_width: 160.0,
                order_by: SortOrder::Modified,
                descending: true,
            }
        );
        assert_eq!(config.language, "cn");
        assert_eq!(config.playback["volume"], serde_json::json!(0.35));
        config.save().unwrap();
        let reloaded = AppConfig::load_with_roots(directory.path().to_owned(), &[]).unwrap();
        assert_eq!(reloaded.audio_table, config.audio_table);
    }

    #[test]
    fn table_width_normalization_caps_all_columns_and_nonfinite_values_before_save() {
        let undersized = AudioTablePreferences {
            name_width: -1.0,
            duration_width: -1.0,
            format_width: -1.0,
            channels_width: -1.0,
            tags_width: -1.0,
            ..Default::default()
        }
        .normalized();
        assert_eq!(undersized.name_width, 120.0);
        assert_eq!(undersized.duration_width, 60.0);
        assert_eq!(undersized.format_width, 82.0);
        assert_eq!(undersized.channels_width, 48.0);
        assert_eq!(undersized.tags_width, 80.0);
        let oversized = AudioTablePreferences {
            name_width: f32::MAX,
            duration_width: f32::MAX,
            format_width: f32::MAX,
            channels_width: f32::MAX,
            tags_width: f32::MAX,
            ..Default::default()
        }
        .normalized();
        assert_eq!(oversized.name_width, 2000.0);
        assert_eq!(oversized.duration_width, 2000.0);
        assert_eq!(oversized.format_width, 2000.0);
        assert_eq!(oversized.channels_width, 2000.0);
        assert_eq!(oversized.tags_width, 2000.0);
        let directory = tempfile::tempdir().unwrap();
        let mut config = AppConfig::load_with_roots(directory.path().to_owned(), &[]).unwrap();
        config.audio_table = AudioTablePreferences {
            name_width: f32::NAN,
            duration_width: f32::INFINITY,
            format_width: f32::NEG_INFINITY,
            channels_width: f32::NAN,
            tags_width: f32::INFINITY,
            ..Default::default()
        };
        config.save().unwrap();
        let loaded = AppConfig::load_with_roots(directory.path().to_owned(), &[]).unwrap();
        assert_eq!(loaded.audio_table, AudioTablePreferences::default());
    }

    #[test]
    fn single_click_audition_preference_survives_enabling_and_disabling() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().to_owned();
        let mut config = AppConfig::load_with_roots(root.clone(), &[]).unwrap();
        assert!(!config.audition_play_on_single_click);
        config.language = "cn".into();
        config.playback["volume"] = serde_json::json!(0.35);
        for enabled in [true, false] {
            config.audition_play_on_single_click = enabled;
            config.save().unwrap();
            config.save().unwrap();
            let saved: serde_json::Value = serde_json::from_slice(
                &std::fs::read(directory.path().join("settings.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(saved["audition_play_on_single_click"], enabled);
            config = AppConfig::load_with_roots(root.clone(), &[]).unwrap();
            assert_eq!(config.audition_play_on_single_click, enabled);
            assert_eq!(config.language, "cn");
            assert_eq!(config.playback["volume"], serde_json::json!(0.35));
        }
    }

    #[test]
    fn selected_library_is_persisted_independently_of_working_directory() {
        let directory = tempfile::tempdir().unwrap();
        let library = directory.path().join("another.db");
        std::fs::write(&library, []).unwrap();
        let mut config = AppConfig::load_in(directory.path().join("userdata")).unwrap();
        config.select_database(&library).unwrap();
        config.save().unwrap();
        let loaded = AppConfig::load_in(directory.path().join("userdata")).unwrap();
        assert_eq!(loaded.paths.database, library.canonicalize().unwrap());
        assert!(loaded.paths.legacy_database.is_none());
    }
    #[test]
    fn old_settings_load_as_local_mode_and_data_source_round_trips() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("settings.json"),
            r#"{"language":"cn","active_database":null,"allowed_scan_roots":[]}"#,
        )
        .unwrap();
        let mut config = AppConfig::load_in(directory.path().to_owned()).unwrap();
        assert_eq!(config.data_source, DataSourceSettings::default());
        assert_eq!(config.data_source.mode, DataSourceMode::Local);
        assert_eq!(
            config.data_source.poll_interval_secs,
            DEFAULT_POLL_INTERVAL_SECS
        );
        assert!(!config.data_source.auto_sync);
        config.data_source.poll_interval_secs = 120;
        config.data_source.auto_sync = true;
        config.data_source.mode = DataSourceMode::Server;
        config.data_source.server_url = "http://server.example.lan:8790".into();
        config.data_source.webdav.username = "sound".into();
        config
            .data_source
            .webdav
            .root_urls
            .insert("SFX".into(), "http://nas/dav/SFX".into());
        config.data_source.validate().unwrap();
        config.save().unwrap();
        let saved = std::fs::read_to_string(directory.path().join("settings.json")).unwrap();
        assert!(saved.contains("\"mode\": \"server\""));
        assert!(!saved.contains("token") && !saved.contains("password"));
        let loaded = AppConfig::load_in(directory.path().to_owned()).unwrap();
        // The server set through the old fields was saved as the active profile.
        let mut expected = config.data_source.clone();
        assert_eq!(expected.migrate_profiles(), ProfileMigration::Created);
        assert_eq!(loaded.data_source, expected);
        assert_eq!(
            loaded.data_source.active.as_deref(),
            Some(DEFAULT_PROFILE_ID)
        );
        assert_eq!(
            loaded.data_source.profiles[0].server_url,
            "http://server.example.lan:8790"
        );
        assert_eq!(loaded.language, "cn");
    }

    #[test]
    fn data_source_urls_are_validated() {
        let mut settings = DataSourceSettings {
            mode: DataSourceMode::Server,
            server_url: "ftp://host".into(),
            ..Default::default()
        };
        assert!(settings.validate().is_err());
        settings.server_url = "http://user:pw@host:8790".into();
        assert!(settings.validate().is_err());
        settings.server_url = "https://host:8790/".into();
        assert!(settings.validate().is_ok());
        settings.webdav.base_url = Some("http:// spaced".into());
        assert!(settings.validate().is_err());
        settings.webdav.base_url = None;
        settings.webdav.root_urls.insert("A".into(), String::new());
        assert!(settings.validate().is_ok());
        settings.mode = DataSourceMode::Local;
        settings.server_url.clear();
        assert!(settings.validate().is_ok());
        for (interval, valid) in [
            (0, true),
            (5, false),
            (10, true),
            (86_400, true),
            (86_401, false),
        ] {
            settings.poll_interval_secs = interval;
            assert_eq!(settings.validate().is_ok(), valid, "interval {interval}");
        }
        // A data_source block written by the previous release has no poll fields.
        let old: DataSourceSettings =
            serde_json::from_str(r#"{"mode":"server","server_url":"http://h:8790"}"#).unwrap();
        assert_eq!(old.poll_interval_secs, DEFAULT_POLL_INTERVAL_SECS);
        assert!(!old.auto_sync);
        assert_eq!(old.audition_memory_mb, DEFAULT_AUDITION_MEMORY_MB);
        assert!(!old.audition_original);
        settings.poll_interval_secs = 0;
        for (megabytes, valid) in [
            (16, false),
            (32, true),
            (512, true),
            (8192, true),
            (8193, false),
        ] {
            settings.audition_memory_mb = megabytes;
            assert_eq!(settings.validate().is_ok(), valid, "{megabytes} MiB");
        }
        // Saved URLs of earlier releases stay valid and are kept verbatim (no rewrite).
        let mut legacy = old.clone();
        assert!(legacy.validate().is_ok());
        assert_eq!(
            serde_json::to_value(&legacy).unwrap()["server_url"],
            "http://h:8790"
        );
        legacy.server_url = "https://audiolib.example.com:8443/audiolib".into();
        assert!(legacy.validate().is_ok());
        legacy.server_url = "https://host:99999".into();
        assert!(legacy.validate().is_err());
    }

    #[test]
    fn user_settings_survive_repeated_save_and_override_legacy() {
        let directory = tempfile::tempdir().unwrap();
        let mut config = AppConfig::load_in(directory.path().to_owned()).unwrap();
        config.language = "cn".into();
        config.playback["volume"] = serde_json::json!(0.3);
        config.save().unwrap();
        config.save().unwrap();
        let loaded = AppConfig::load_in(directory.path().to_owned()).unwrap();
        assert_eq!(loaded.language, "cn");
        assert_eq!(loaded.playback["volume"], serde_json::json!(0.3));
    }

    #[test]
    fn saved_rust_settings_and_existing_library_ignore_broken_legacy_yaml() {
        let temp = tempfile::tempdir().unwrap();
        let legacy = temp.path().join("legacy");
        std::fs::create_dir_all(legacy.join("config")).unwrap();
        std::fs::write(legacy.join("config/audiolib_config.yaml"), "broken: [").unwrap();
        let established = temp.path().join("saved");
        let mut config = AppConfig::load_with_roots(established.clone(), &[]).unwrap();
        config.language = "cn".into();
        config.playback["volume"] = serde_json::json!(0.4);
        config.save().unwrap();
        let loaded =
            AppConfig::load_with_roots(established, std::slice::from_ref(&legacy)).unwrap();
        assert_eq!(loaded.language, "cn");
        assert_eq!(loaded.playback["volume"], serde_json::json!(0.4));
        assert!(loaded.paths.legacy_database.is_none());
        let existing = temp.path().join("existing");
        std::fs::create_dir_all(&existing).unwrap();
        std::fs::write(existing.join("library.db"), []).unwrap();
        assert!(AppConfig::load_with_roots(existing, std::slice::from_ref(&legacy)).is_ok());
        assert!(AppConfig::load_with_roots(temp.path().join("first-run"), &[legacy]).is_err());
    }

    #[test]
    fn first_run_still_imports_valid_legacy_paths_and_preferences() {
        let temp = tempfile::tempdir().unwrap();
        let legacy = temp.path().join("legacy");
        std::fs::create_dir_all(legacy.join("config")).unwrap();
        std::fs::create_dir_all(legacy.join("data")).unwrap();
        std::fs::write(legacy.join("data/old.db"), []).unwrap();
        std::fs::write(legacy.join("config/audiolib_config.yaml"),"database:\n  path: data/old.db\napplication:\n  language: cn\nplayback:\n  default_volume: 0.35\nsecurity:\n  allowed_scan_roots: []\n").unwrap();
        let config = AppConfig::load_with_roots(
            temp.path().join("first-run"),
            std::slice::from_ref(&legacy),
        )
        .unwrap();
        assert_eq!(
            config.paths.legacy_database,
            Some(legacy.join("data/old.db"))
        );
        assert_eq!(config.language, "cn");
        assert_eq!(config.playback["volume"], serde_json::json!(0.35));
        config.save().unwrap();
        std::fs::write(legacy.join("config/audiolib_config.yaml"), "broken: [").unwrap();
        let loaded = AppConfig::load_with_roots(
            config.paths.settings.parent().unwrap().to_path_buf(),
            &[legacy],
        )
        .unwrap();
        assert_eq!(loaded.playback["volume"], serde_json::json!(0.35));
    }

    #[test]
    fn server_mode_guard_requires_existing_profile_with_valid_url() {
        let mut settings = DataSourceSettings {
            mode: DataSourceMode::Server,
            server_url: "http://192.0.2.10:8790".into(),
            ..Default::default()
        };
        // Never migrated: no active profile at all.
        assert!(settings.clone().guard_server_mode().is_some());
        settings.migrate_profiles();
        assert_eq!(settings.server_mode_problem(), None);
        let mut usable = settings.clone();
        assert_eq!(usable.guard_server_mode(), None);
        assert_eq!(usable.mode, DataSourceMode::Server);

        let mut dangling = settings.clone();
        dangling.active = Some("missing".into());
        let reason = dangling.guard_server_mode().unwrap();
        assert!(reason.contains("does not exist"), "{reason}");
        assert_eq!(dangling.mode, DataSourceMode::Local);
        assert_eq!(dangling.profiles, settings.profiles, "profiles are kept");

        let mut empty = settings.clone();
        empty.profiles[0].server_url = "  ".into();
        assert!(empty.guard_server_mode().unwrap().contains("no server URL"));
        assert_eq!(empty.mode, DataSourceMode::Local);

        let mut invalid = settings.clone();
        invalid.profiles[0].server_url = "htp:/oops".into();
        assert!(invalid.guard_server_mode().unwrap().contains("invalid URL"));
        assert_eq!(invalid.mode, DataSourceMode::Local);

        let mut local = DataSourceSettings::default();
        assert_eq!(local.guard_server_mode(), None);
        assert_eq!(local.mode, DataSourceMode::Local);
    }

    #[test]
    fn guarded_server_mode_is_not_persisted_as_local() {
        let root = std::env::temp_dir().join(format!(
            "audiolib-guard-save-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut config = AppConfig::load_in(root.clone()).unwrap();
        config.data_source.mode = DataSourceMode::Server;
        config.data_source.server_url = "http://192.0.2.10:8790".into();
        config.save().unwrap();
        // Corrupt the active profile's URL on disk, as an older client or hand edit might.
        let mut on_disk: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&config.paths.settings).unwrap()).unwrap();
        on_disk["data_source"]["profiles"][0]["server_url"] = "htp:/broken".into();
        on_disk["data_source"]["server_url"] = "htp:/broken".into();
        std::fs::write(
            &config.paths.settings,
            serde_json::to_vec(&on_disk).unwrap(),
        )
        .unwrap();
        let original = AppConfig::load_in(root.clone()).unwrap().data_source;
        assert_eq!(original.mode, DataSourceMode::Server);

        let mut guarded = AppConfig::load_in(root.clone()).unwrap();
        assert!(guarded.guard_server_mode().is_some());
        assert_eq!(guarded.data_source.mode, DataSourceMode::Local);
        // Unrelated preference saves (language, playback, close) keep the server selection.
        guarded.language = "cn".into();
        guarded.save().unwrap();
        let reloaded = AppConfig::load_in(root.clone()).unwrap();
        assert_eq!(reloaded.language, "cn");
        assert_eq!(reloaded.data_source.mode, original.mode);
        assert_eq!(reloaded.data_source.active, original.active);
        assert_eq!(reloaded.data_source.profiles, original.profiles);

        // An explicit source choice in this session is written as chosen.
        guarded
            .data_source
            .set_active(&ActiveSource::Local)
            .unwrap();
        guarded.save().unwrap();
        let chosen = AppConfig::load_in(root.clone()).unwrap();
        assert_eq!(chosen.data_source.mode, DataSourceMode::Local);
        assert_eq!(chosen.data_source.active.as_deref(), Some(ACTIVE_LOCAL));
        assert_eq!(chosen.data_source.profiles, original.profiles);
        if root.parent() == Some(std::env::temp_dir().as_path()) {
            std::fs::remove_dir_all(&root).unwrap();
        }
    }
}
