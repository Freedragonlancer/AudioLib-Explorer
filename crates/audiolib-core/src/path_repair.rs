// crates/audiolib-core/src/path_repair.rs
//! Owned local audio path matching contracts. Rows and candidate indices refer to one preview.
use std::path::PathBuf;

/// The indexed directory may be offline or use another operating system's spelling.
#[derive(Clone, Debug)]
pub struct RepairRequest {
    pub source: String,
    pub target: PathBuf,
    pub missing_only: bool,
}

/// A verified filesystem candidate and its observed revision.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub path: PathBuf,
    pub size: u64,
    pub modified_ns: i64,
}

/// Basis for displaying and preselecting reviewed candidates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchKind {
    Relative,
    Filename,
    Ambiguous,
    Missing,
    Occupied,
    Inaccessible,
}

/// The original record identity and the complete set of selectable alternatives.
#[derive(Clone, Debug)]
pub struct RepairRow {
    pub id: i64,
    pub original: String,
    pub kind: MatchKind,
    pub candidates: std::sync::Arc<[Candidate]>,
}

/// Preview bound to an open provider. Directory traversal issues are counted with bounded examples.
#[derive(Clone, Debug)]
pub struct RepairPreview {
    pub session: String,
    pub library_namespace: String,
    pub request: RepairRequest,
    pub rows: Vec<RepairRow>,
    pub issue_count: usize,
    pub issues: Vec<String>,
}
