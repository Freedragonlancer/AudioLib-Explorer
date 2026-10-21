// crates/audiolib-db/src/migration/plan.rs
//! Read-only planning. Format and business evidence bind every later mutation.
use super::inspect::{business_digest, check_cancel, inspect_connection, table_exists};
use crate::{db_error, schema, search_storage};
use audiolib_core::{
    Error, Result,
    compatibility::{CATALOG_REVISION, ExecutionState, LibraryAccess, MigrationDisposition},
};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{path::Path, sync::atomic::AtomicBool};

/// Conservative additional free-space estimate, never a promised final size.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SpaceBudget {
    /// Logical SQLite pages, including committed WAL content.
    pub source_bytes: u64,
    /// Backup, staging, compaction, publication journal and optional legacy copies.
    pub additional_bytes: u64,
    /// Whether a separate owned legacy normalization copy is required.
    pub normalizes_legacy: bool,
}

/// Reviewable, source-bound request. Apply rechecks every field under write exclusion.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MigrationPlan {
    /// Protocol revision understood by the shared executor.
    pub protocol: u32,
    /// Exact producing application identity.
    pub application_version: String,
    /// Compatibility catalog used to choose the route.
    pub catalog_revision: u32,
    /// Tokenizer implementation/dictionary identity for deterministic resume.
    pub tokenizer_identity: String,
    /// Stable compiled route identity; changing a step cannot silently resume an old job.
    pub recipe_identity: String,
    /// Exact reviewed additions/removals, including authorized historical-data retirement.
    pub schema_changes: Vec<super::SchemaChange>,
    /// Fully traversed source evidence.
    pub source: super::LibraryInspection,
    /// Actual SQL schema identity, excluding only this executor's internal objects.
    pub schema_sha256: String,
    /// Route-specific disk-space estimate.
    pub space: SpaceBudget,
}

pub(super) fn invalid(message: impl Into<String>) -> Error {
    Error::Validation(message.into())
}

pub(super) fn tokenizer_identity() -> String {
    let mut hash = Sha256::new();
    hash.update(b"jieba-rs=0.11.0;tokenization=3;source_metadata=2;");
    hash.update(include_bytes!("../../../../resources/audio_terms.txt"));
    format!("{:x}", hash.finalize())
}

pub(super) fn schema_digest(conn: &Connection) -> Result<String> {
    let objects = conn
        .prepare(
            "SELECT type,name,tbl_name,sql FROM sqlite_schema WHERE sql IS NOT NULL
             AND name NOT IN ('audiolib_storage_migration','audiolib_migration_receipts')
             AND name NOT GLOB 'audiolib_storage_guard_*' ORDER BY type,name",
        )
        .map_err(db_error)?
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(db_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(db_error)?;
    let bytes = serde_json::to_vec(&objects).map_err(|error| invalid(error.to_string()))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

pub(super) fn check_source(
    conn: &Connection,
    plan: &MigrationPlan,
    cancel: &AtomicBool,
) -> Result<()> {
    check_cancel(cancel)?;
    let version: u32 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(db_error)?;
    if version != plan.source.format.schema_version
        || schema_digest(conn)? != plan.schema_sha256
        || Some(business_digest(conn, cancel)?) != plan.source.business
    {
        return Err(invalid(
            "Migration source changed after planning; inspect and plan again",
        ));
    }
    // Feature/revision values are deliberately excluded from the business hash.
    let actual = inspect_connection(
        conn,
        plan.source.source.clone(),
        LibraryAccess::Write,
        false,
        cancel,
    )?;
    if actual.format != plan.source.format {
        return Err(invalid("Migration source format changed after planning"));
    }
    Ok(())
}

/// Plan a supported derived-state migration without opening the source for writing.
/// Legacy v1/v2 imports and unknown/corrupt formats remain explicit blocked decisions.
pub fn plan(path: &Path, cancel: &AtomicBool) -> Result<MigrationPlan> {
    check_cancel(cancel)?;
    crate::sqlite_engine::require_plan_b()?;
    let source_path = path.canonicalize()?;
    let conn = Connection::open_with_flags(&source_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(db_error)?;
    conn.execute_batch("PRAGMA query_only=ON; BEGIN; SELECT COUNT(*) FROM sqlite_schema;")
        .map_err(db_error)?;
    if table_exists(&conn, "audiolib_storage_migration")? {
        return Err(Error::Busy(
            "Storage migration already exists; use resume or rollback".into(),
        ));
    }
    let diagnosis = super::diagnose::diagnose_connection(&conn, source_path, cancel)?;
    if diagnosis
        .findings
        .iter()
        .any(|finding| finding.code != super::DiagnosticCode::DerivedDataInvalid)
    {
        return Err(invalid(format!(
            "Migration diagnosis blocked execution: {}",
            serde_json::to_string(&diagnosis).map_err(|error| invalid(error.to_string()))?
        )));
    }
    let source = diagnosis.inspection.ok_or_else(|| {
        invalid(format!(
            "Migration diagnosis requires attention: {:?}",
            diagnosis.decision
        ))
    })?;
    if source.decision.execution_state != ExecutionState::Ready
        || source.decision.disposition != Some(MigrationDisposition::Migratable)
        || source.business.is_none()
    {
        return Err(invalid(format!(
            "No executable storage migration: {:?}",
            source.decision
        )));
    }
    let normalizes_legacy = source.format.schema_version < 5
        || schema::pending(&conn)?
        || (source.format.schema_version == 5 && schema::legacy_needs_upgrade(&conn)?);
    if !normalizes_legacy {
        // Reject unsupported extension dependencies before creating a job or backup.
        search_storage::blueprint_with(&conn, &super::recipe::changes())?;
    }
    let source_bytes: u64 = conn.query_row(
        "SELECT (SELECT page_count FROM pragma_page_count)*(SELECT page_size FROM pragma_page_size)",
        [], |row| crate::unsigned(row, 0),
    ).map_err(db_error)?;
    let copies = if normalizes_legacy { 8 } else { 5 };
    let additional_bytes = source_bytes
        .checked_mul(copies)
        .and_then(|value| value.checked_add(1 << 30))
        .ok_or_else(|| invalid("Migration space estimate overflow"))?;
    Ok(MigrationPlan {
        protocol: 1,
        application_version: env!("CARGO_PKG_VERSION").into(),
        catalog_revision: CATALOG_REVISION,
        tokenizer_identity: tokenizer_identity(),
        recipe_identity: super::recipe::identity(),
        schema_changes: super::recipe::changes(),
        schema_sha256: schema_digest(&conn)?,
        source,
        space: SpaceBudget {
            source_bytes,
            additional_bytes,
            normalizes_legacy,
        },
    })
}
