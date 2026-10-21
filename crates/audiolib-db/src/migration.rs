// crates/audiolib-db/src/migration.rs
//! Shared owned-library inspection and migration entry points for desktop, CLI and server.
mod checkpoint;
mod diagnose;
mod inspect;
mod job;
mod plan;
mod recipe;
mod runtime;
mod stage;
pub use diagnose::{DiagnosticCode, DiagnosticFinding, LibraryDiagnosis, diagnose};
pub use inspect::{BusinessDigest, LibraryInspection, TableDigest, inspect};
pub use job::{MigrationJob, MigrationPhase, MigrationProgress, apply, resume, rollback, verify};
pub use plan::{MigrationPlan, SpaceBudget, plan};
pub use recipe::SchemaChange;
pub(crate) use recipe::changes as schema_changes;
pub use runtime::discover;
pub(crate) use runtime::prepare_writable;

pub(crate) fn digest_business(
    conn: &rusqlite::Connection,
    cancel: &std::sync::atomic::AtomicBool,
) -> audiolib_core::Result<BusinessDigest> {
    inspect::business_digest(conn, cancel)
}

#[cfg(test)]
mod tests;
