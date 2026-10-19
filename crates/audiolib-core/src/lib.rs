// crates/audiolib-core/src/lib.rs
//! Owned domain contracts shared by desktop, CLI, database, and audio services.
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

pub mod config;
pub mod library;
mod metadata;
pub mod profiles;
pub mod server_url;
pub use library::*;
pub use metadata::{
    SOURCE_TAG_KEYS, display_tags, source_description, source_metadata_search_text, source_tags,
};

/// Compatible operation envelope for CLI and local control replies.
pub fn success(data: serde_json::Value) -> serde_json::Value {
    serde_json::json!({"success":true,"error":null,"error_code":null,"warnings":[],
        "metadata":{},"execution_time_ms":0.0,"timestamp":std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs(),"data":data})
}

/// An explicit failure rather than a misleading successful empty result.
pub fn failure(message: &str) -> serde_json::Value {
    serde_json::json!({"success":false,"error":message,"error_code":"OPERATION_FAILED",
        "warnings":[],"metadata":{},"execution_time_ms":0.0,"timestamp":std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs(),"data":null})
}

pub type Result<T> = std::result::Result<T, Error>;

/// Message of the I/O error a streaming reader returns when Stop interrupts a read that
/// is blocked on network data. Audio engines treat it as a stop, never as a failure.
pub const AUDIO_READ_INTERRUPTED: &str = "audio read interrupted";

/// The I/O error for a read interrupted by Stop (see [`AUDIO_READ_INTERRUPTED`]). Not
/// `ErrorKind::Interrupted`: `read_exact` would silently retry that kind.
pub fn read_interrupted_error() -> std::io::Error {
    std::io::Error::other(AUDIO_READ_INTERRUPTED)
}

impl Error {
    /// Whether this failure is a read deliberately interrupted by Stop (also when a
    /// decoder wrapped the I/O error into its own message).
    pub fn is_read_interrupted(&self) -> bool {
        match self {
            Error::Validation(_) | Error::Cancelled => false,
            error => error.to_string().contains(AUDIO_READ_INTERRUPTED),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Validation: {0}")]
    Validation(String),
    #[error("Database: {0}")]
    Database(String),
    #[error("Audio: {0}")]
    Audio(String),
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("Operation cancelled")]
    Cancelled,
    #[error("Busy: {0}")]
    Busy(String),
    #[error("Not found: {0}")]
    NotFound(String),
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AudioMetadata {
    pub sample_rate: u32,
    pub bit_depth: u16,
    pub channels: u16,
    pub frames: u64,
    pub duration: f64,
    pub encoding: String,
    pub bext_description: String,
    pub attributes: serde_json::Value,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AudioFile {
    pub id: i64,
    pub file_path: String,
    pub filename: String,
    pub folder_id: Option<i64>,
    pub file_size: u64,
    pub sample_rate: u32,
    pub bit_depth: u16,
    pub channels: u16,
    pub duration: f64,
    pub tags: String,
    pub color: Option<String>,
    pub notes: String,
    pub favorite: bool,
    pub bext_description: String,
    pub play_count: u64,
    pub attributes: serde_json::Value,
    #[serde(default)]
    pub missing: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Folder {
    pub id: i64,
    pub path: String,
    pub name: String,
    pub parent_id: Option<i64>,
    pub file_count: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SearchFilters {
    #[serde(default)]
    pub visibility: FileVisibility,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u16>,
    pub channels: Option<u16>,
    pub duration_min: Option<f64>,
    pub duration_max: Option<f64>,
    pub favorites_only: bool,
    pub tagged_only: bool,
    /// Case-insensitive literal substring of library tags; percent/underscore are literal.
    pub tag_contains: Option<String>,
    /// Exact library color, ignoring hex letter case.
    pub color: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SortOrder {
    #[default]
    Relevance,
    Filename,
    Duration,
    SampleRate,
    Modified,
    /// Numeric channel count, with stable audio identity as the pagination tie-breaker.
    Channels,
    /// Case-insensitive library tags, or extracted source tags when library tags are empty.
    Tags,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchRequest {
    pub query: String,
    pub folder_id: Option<i64>,
    pub include_subfolders: bool,
    pub filters: SearchFilters,
    pub limit: usize,
    pub offset: usize,
    pub order_by: SortOrder,
    pub descending: bool,
}
impl Default for SearchRequest {
    fn default() -> Self {
        Self {
            query: String::new(),
            folder_id: None,
            include_subfolders: true,
            filters: SearchFilters::default(),
            limit: 200,
            offset: 0,
            order_by: SortOrder::Relevance,
            descending: false,
        }
    }
}
impl SearchRequest {
    /// Validate bounded search input before compiling a parameterized FTS5 query.
    pub fn validate(&self) -> Result<()> {
        if self.query.chars().count() > 500 || self.query.contains(';') || self.query.contains('\0')
        {
            return Err(Error::Validation(
                "Query exceeds 500 characters or contains a forbidden delimiter".into(),
            ));
        }
        if self.limit == 0 || self.limit > 10_000 {
            return Err(Error::Validation("Search limit must be 1..=10000".into()));
        }
        for duration in [self.filters.duration_min, self.filters.duration_max]
            .into_iter()
            .flatten()
        {
            if !duration.is_finite() || duration < 0.0 {
                return Err(Error::Validation(
                    "Duration must be finite and nonnegative".into(),
                ));
            }
        }
        if matches!((self.filters.duration_min, self.filters.duration_max), (Some(min), Some(max)) if min > max)
        {
            return Err(Error::Validation("Minimum duration exceeds maximum".into()));
        }
        if self
            .filters
            .tag_contains
            .as_ref()
            .is_some_and(|tag| tag.chars().count() > 500 || tag.contains('\0'))
        {
            return Err(Error::Validation(
                "Tag filter exceeds 500 characters or contains NUL".into(),
            ));
        }
        if let Some(color) = &self.filters.color {
            MetadataPatch {
                color: Some(color.clone()),
                ..Default::default()
            }
            .validate()?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SearchPage {
    pub results: Vec<AudioFile>,
    pub total_count: u64,
    pub query: String,
    pub execution_time_ms: f64,
    pub cache_hit: bool,
    pub returned_count: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MetadataPatch {
    pub tags: Option<String>,
    pub color: Option<String>,
    pub notes: Option<String>,
    pub favorite: Option<bool>,
}
impl MetadataPatch {
    /// Validate library-only editing; never modifies the source WAV.
    pub fn validate(&self) -> Result<()> {
        if self.tags.as_ref().is_some_and(|value| value.len() > 16_384)
            || self
                .notes
                .as_ref()
                .is_some_and(|value| value.len() > 1_048_576)
        {
            return Err(Error::Validation(
                "Metadata text exceeds the allowed size".into(),
            ));
        }
        if let Some(color) = &self.color
            && !color.is_empty()
            && !(color.len() == 7
                && color.starts_with('#')
                && color[1..].bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err(Error::Validation("Color must be empty or #RRGGBB".into()));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScanState {
    #[default]
    Running,
    Completed,
    Cancelled,
    Failed,
}
/// Scan progress. Also the server's scan reply shape (`/scans*`), so missing fields
/// default and unknown fields are ignored.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ScanProgress {
    pub session_id: i64,
    pub state: ScanState,
    /// Confirmed worker pause at a safe boundary; the session and scan lease remain live.
    #[serde(default)]
    pub paused: bool,
    /// Final atomic database reconciliation has begun; controls are no longer accepted.
    #[serde(default)]
    pub finalizing: bool,
    pub discovered: u64,
    pub processed: u64,
    pub added: u64,
    pub updated: u64,
    pub unchanged: u64,
    #[serde(default)]
    pub removed: u64,
    pub errors: u64,
    #[serde(default)]
    pub error_details: Vec<ScanIssue>,
    #[serde(default)]
    pub errors_truncated: u64,
    pub current_path: String,
    pub message: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LibraryStats {
    pub audio_files: u64,
    pub folders: u64,
    pub total_duration: f64,
    pub total_bytes: u64,
    #[serde(default)]
    pub missing_files: u64,
}

/// All business data operations pass through this interface and return owned DTOs.
pub trait DataProvider: Send + Sync {
    /// Provider-local change token for search caches. Compare only within this provider's
    /// lifetime, before and after reading; `None` disables cache reuse.
    fn search_revision(&self) -> Result<Option<u64>> {
        Ok(None)
    }
    fn search(&self, request: &SearchRequest) -> Result<SearchPage>;
    /// Search that can be superseded by a newer GUI request. Providers may additionally
    /// interrupt in-flight work; the default checks cancellation before and after I/O.
    fn search_cancellable(
        &self,
        request: &SearchRequest,
        cancel: Arc<AtomicBool>,
    ) -> Result<SearchPage> {
        if cancel.load(Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        let result = self.search(request);
        if cancel.load(Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        result
    }
    /// Positive directory matches for the query and filters, including descendant totals.
    /// Ignores the selected folder, pagination and sorting so sibling matches stay navigable.
    fn search_folders(&self, request: &SearchRequest) -> Result<Vec<Folder>>;
    /// Cancellable directory facets with the same scope semantics as `search_folders`.
    fn search_folders_cancellable(
        &self,
        request: &SearchRequest,
        cancel: Arc<AtomicBool>,
    ) -> Result<Vec<Folder>> {
        if cancel.load(Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        let result = self.search_folders(request);
        if cancel.load(Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        result
    }
    fn get_audio(&self, id: i64) -> Result<AudioFile>;
    fn folders(&self, parent_id: Option<i64>) -> Result<Vec<Folder>>;
    /// Retrieve one owned folder by its stable identity; absence is `NotFound`.
    /// Providers may implement a direct indexed lookup instead of the tree fallback.
    fn get_folder(&self, id: i64) -> Result<Folder> {
        let mut parents = vec![None];
        let mut seen = std::collections::HashSet::new();
        while let Some(parent) = parents.pop() {
            for folder in self.folders(parent)? {
                if folder.id == id {
                    return Ok(folder);
                }
                if seen.insert(folder.id) {
                    parents.push(Some(folder.id));
                }
            }
        }
        Err(Error::NotFound(format!("Folder {id}")))
    }
    fn update_metadata(&self, id: i64, patch: &MetadataPatch) -> Result<AudioFile>;
    fn record_play(&self, id: i64) -> Result<()>;
    fn stats(&self) -> Result<LibraryStats>;
    fn library_info(&self) -> Result<LibraryInfo>;
    fn check_library(&self) -> Result<IntegrityReport>;
    fn backup_library(&self, destination: &Path) -> Result<BackupReport>;
    fn restore_library(&self, source: &Path) -> Result<RestoreReport>;
    fn maintain_library(&self, action: MaintenanceAction) -> Result<MaintenanceReport>;
    fn update_metadata_batch(&self, request: &BatchMetadataRequest) -> Result<BatchMetadataReport>;
    fn find_audio_by_path(&self, path: &Path) -> Result<Option<AudioFile>>;
    fn relink_audio(&self, id: i64, path: &Path) -> Result<AudioFile>;
    /// Relocate an indexed top-level folder as one operation while retaining record
    /// identities and annotations. Every indexed WAV must have a valid corresponding
    /// target beneath `target`; providers without this facility refuse the operation.
    fn relink_folder(&self, _source: &Path, _target: &Path) -> Result<u64> {
        Err(Error::Validation(
            "This data provider cannot relocate library folders".into(),
        ))
    }
    /// Relocation that stops before its atomic commit when the request is abandoned.
    fn relink_folder_cancellable(
        &self,
        source: &Path,
        target: &Path,
        cancel: Arc<AtomicBool>,
    ) -> Result<u64> {
        if cancel.load(Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        self.relink_folder(source, target)
    }
    fn remove_audio(&self, ids: &[i64]) -> Result<u64>;
    fn remove_folder(&self, id: i64) -> Result<u64>;
    fn scan_history(&self, limit: usize, offset: usize) -> Result<Vec<ScanSession>>;
    fn play_history(&self, limit: usize, offset: usize) -> Result<Vec<PlayRecord>>;
    fn export_library(
        &self,
        destination: &Path,
        format: LibraryExportFormat,
    ) -> Result<TransferReport>;
    fn import_annotations(&self, source: &Path) -> Result<TransferReport>;
}

/// Resolve an existing scan root using actual filesystem identity, not prefix strings.
pub fn validate_scan_root(path: &Path, allowed_roots: &[PathBuf]) -> Result<PathBuf> {
    if path.as_os_str().is_empty() || !path.is_dir() {
        return Err(Error::Validation(
            "Scan root must be an existing directory".into(),
        ));
    }
    let canonical = path.canonicalize()?;
    if !allowed_roots.is_empty()
        && !allowed_roots.iter().any(|root| {
            root.canonicalize()
                .is_ok_and(|root| canonical.starts_with(root))
        })
    {
        return Err(Error::Validation(
            "Directory is outside allowed scan roots".into(),
        ));
    }
    Ok(canonical)
}

/// Resolve a WAV's real filesystem target before checking its permitted directory.
/// Callers must pass the returned path to the operation, rather than the original link.
pub fn validate_wav_path(path: &Path, allowed_roots: &[PathBuf]) -> Result<PathBuf> {
    let canonical = path.canonicalize()?;
    if !canonical.is_file()
        || !canonical
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("wav"))
    {
        return Err(Error::Validation(
            "Target must be an existing WAV file".into(),
        ));
    }
    let parent = canonical
        .parent()
        .ok_or_else(|| Error::Validation("WAV parent folder is required".into()))?;
    validate_scan_root(parent, allowed_roots)?;
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wav_validation_resolves_targets_and_rejects_invalid_or_outside_files() {
        let directory = tempfile::tempdir().unwrap();
        let allowed = directory.path().join("allowed");
        let outside = directory.path().join("outside");
        std::fs::create_dir_all(&allowed).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let permitted_file = allowed.join("sound.WAV");
        let forbidden_file = outside.join("sound.wav");
        let wrong_extension = allowed.join("sound.txt");
        for path in [&permitted_file, &forbidden_file, &wrong_extension] {
            std::fs::write(path, []).unwrap();
        }
        let roots = [allowed.clone()];
        assert_eq!(
            validate_wav_path(&permitted_file, &roots).unwrap(),
            permitted_file.canonicalize().unwrap()
        );
        assert!(validate_wav_path(&forbidden_file, &roots).is_err());
        assert!(validate_wav_path(&wrong_extension, &roots).is_err());
        assert!(validate_wav_path(&allowed, &roots).is_err());
        assert!(validate_wav_path(&allowed.join("absent.wav"), &roots).is_err());
    }

    #[test]
    #[cfg(any(unix, windows))]
    fn wav_link_cannot_escape_allowed_roots_and_allowed_link_returns_real_target() {
        let directory = tempfile::tempdir().unwrap();
        let allowed = directory.path().join("allowed");
        let outside = directory.path().join("outside");
        std::fs::create_dir_all(&allowed).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let target = outside.join("outside.wav");
        let allowed_target = allowed.join("real.wav");
        std::fs::write(&target, []).unwrap();
        std::fs::write(&allowed_target, []).unwrap();
        let forbidden_link = allowed.join("linked.wav");
        let permitted_link = allowed.join("permitted.wav");
        #[cfg(windows)]
        let link_result = std::os::windows::fs::symlink_file(&target, &forbidden_link);
        #[cfg(unix)]
        let link_result = std::os::unix::fs::symlink(&target, &forbidden_link);
        #[cfg(windows)]
        if link_result
            .as_ref()
            .is_err_and(|error| error.raw_os_error() == Some(1314))
        {
            eprintln!("Symbolic-link regression requires Windows Developer Mode or link privilege");
            return;
        }
        link_result.unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(&allowed_target, &permitted_link).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&allowed_target, &permitted_link).unwrap();
        let roots = [allowed];
        assert!(validate_wav_path(&forbidden_link, &roots).is_err());
        assert_eq!(
            validate_wav_path(&permitted_link, &roots).unwrap(),
            allowed_target.canonicalize().unwrap()
        );
    }

    #[test]
    fn bounded_search_and_metadata() {
        let mut request = SearchRequest::default();
        assert!(request.validate().is_ok());
        request.filters.duration_min = Some(0.0);
        assert!(request.validate().is_ok());
        request.query = "x".repeat(501);
        assert!(request.validate().is_err());
        assert!(
            MetadataPatch {
                color: Some("red".into()),
                ..Default::default()
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn sortable_columns_preserve_named_wire_contract() {
        for (name, expected) in [
            ("channels", SortOrder::Channels),
            ("tags", SortOrder::Tags),
            ("modified", SortOrder::Modified),
        ] {
            let encoded = serde_json::to_string(&expected).unwrap();
            assert_eq!(encoded, format!("\"{name}\""));
            assert_eq!(
                serde_json::from_str::<SortOrder>(&encoded).unwrap(),
                expected
            );
        }
    }
}
