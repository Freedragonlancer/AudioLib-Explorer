// crates/audiolib-db/src/wal.rs
//! Explicit checkpoints inspect SQLite's busy result instead of discarding its row.
use crate::{SqliteProvider, db_error, management};
use audiolib_core::{Error, Result};
use rusqlite::Connection;

/// Observation points while the cross-process administration lease excludes writers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CheckpointStage {
    /// Observe outstanding commits before moving WAL pages.
    Before,
    /// The WAL was successfully truncated; persist the new physical baseline.
    After,
}

pub(crate) fn truncate(conn: &Connection) -> Result<()> {
    let (busy, log, checkpointed): (i64, i64, i64) = conn
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .map_err(db_error)?;
    if busy != 0 || log > 0 || checkpointed > 0 {
        return Err(Error::Busy(format!(
            "WAL checkpoint blocked by an active reader (busy={busy}, frames={log}, checkpointed={checkpointed}); close other library readers and retry"
        )));
    }
    Ok(())
}

impl SqliteProvider {
    /// Truncate the WAL, reporting busy instead of silently retaining its contents.
    pub fn checkpoint_wal(&self) -> Result<()> {
        self.checkpoint_wal_observed(|_| Ok(()))
    }

    /// Checkpoint under exclusive administration. The observer may persist revision
    /// metadata at both stages but must not call this provider or write the library.
    pub fn checkpoint_wal_observed(
        &self,
        mut observe: impl FnMut(CheckpointStage) -> Result<()>,
    ) -> Result<()> {
        let _lease = management::AdministrationLease::exclusive(&self.path)?;
        let conn = self.writer_unlocked()?;
        observe(CheckpointStage::Before)?;
        truncate(&conn)?;
        observe(CheckpointStage::After)
    }
}
