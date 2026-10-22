// crates/audiolib-core/src/library.rs
//! Owned contracts for library management; no SQLite or GUI dependencies.
use crate::{LibraryStats, MetadataPatch, ScanState};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FileVisibility {
    #[default]
    Present,
    Missing,
    All,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ScanIssue {
    pub path: String,
    pub message: String,
}

/// One scan history entry (also returned by the server's `GET /history/scans`; missing
/// fields default).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ScanSession {
    pub id: i64,
    pub start_time: String,
    pub end_time: Option<String>,
    pub scan_path: String,
    pub scan_type: String,
    pub state: ScanState,
    pub scanned: u64,
    pub added: u64,
    pub updated: u64,
    pub removed: u64,
    pub unchanged: u64,
    pub errors: u64,
    pub error_details: Vec<ScanIssue>,
    pub errors_truncated: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LibraryInfo {
    /// Server builds may omit the database path (it is a server-local detail); a missing
    /// field deserializes as an empty path instead of failing the whole response.
    #[serde(default)]
    pub database: PathBuf,
    pub schema_version: i64,
    pub sqlite_version: String,
    pub database_bytes: u64,
    pub wal_bytes: u64,
    pub stats: LibraryStats,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct IntegrityReport {
    pub sqlite_ok: bool,
    pub foreign_key_violations: u64,
    pub fts_ok: bool,
    pub errors: Vec<String>,
}
impl IntegrityReport {
    /// True only when the database, relations and external-content FTS5 agree.
    pub fn is_ok(&self) -> bool {
        self.sqlite_ok && self.foreign_key_violations == 0 && self.fts_ok && self.errors.is_empty()
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BackupReport {
    pub destination: PathBuf,
    pub total_records: u64,
    pub bytes: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RestoreReport {
    pub source: PathBuf,
    pub rollback_backup: PathBuf,
    pub library: LibraryInfo,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MaintenanceAction {
    Optimize,
    RebuildSearch,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MaintenanceReport {
    pub action: MaintenanceAction,
    pub rollback_backup: PathBuf,
    pub integrity: IntegrityReport,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TagEditMode {
    #[default]
    Replace,
    Append,
    Remove,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BatchMetadataRequest {
    pub ids: Vec<i64>,
    pub patch: MetadataPatch,
    pub tag_mode: TagEditMode,
}
impl BatchMetadataRequest {
    /// Bound each atomic edit and reject ambiguous empty batches.
    pub fn validate(&self) -> crate::Result<()> {
        if self.ids.is_empty() || self.ids.len() > 10_000 || self.ids.iter().any(|id| *id <= 0) {
            return Err(crate::Error::Validation(
                "Batch requires 1..=10000 positive IDs".into(),
            ));
        }
        if self.patch.tags.is_none()
            && self.patch.color.is_none()
            && self.patch.notes.is_none()
            && self.patch.favorite.is_none()
        {
            return Err(crate::Error::Validation(
                "Batch requires at least one annotation field".into(),
            ));
        }
        self.patch.validate()
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BatchMetadataReport {
    pub updated: u64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LibraryExportFormat {
    JsonLines,
    Csv,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TransferReport {
    pub path: PathBuf,
    pub records: u64,
    pub skipped: u64,
    pub bytes: u64,
}

#[cfg(test)]
mod tests {
    #[test]
    fn library_info_without_database_field_deserializes() {
        let mut value = serde_json::to_value(super::LibraryInfo {
            schema_version: 7,
            sqlite_version: "3.50.0".into(),
            database_bytes: 42,
            ..Default::default()
        })
        .unwrap();
        assert!(value.as_object_mut().unwrap().remove("database").is_some());
        let info: super::LibraryInfo = serde_json::from_value(value).unwrap();
        assert_eq!(info.database, std::path::PathBuf::new());
        assert_eq!(info.schema_version, 7);
        assert_eq!(info.database_bytes, 42);
    }

    use super::*;
    #[test]
    fn batch_rejects_empty_fields_and_invalid_ids_before_writing() {
        let mut request = BatchMetadataRequest {
            ids: vec![1, 2],
            ..Default::default()
        };
        assert!(request.validate().is_err());
        request.patch.favorite = Some(false);
        assert!(request.validate().is_ok());
        request.ids.push(0);
        assert!(request.validate().is_err());
        request.ids = vec![1; 10_001];
        assert!(request.validate().is_err());
    }
}
