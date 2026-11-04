// crates/audiolib-db/src/vendor_import/storage.rs
//! Durable business provenance and incremental vendor projection. No source audio is modified.
use super::{Result, db_error, invalid, json};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

const TABLES: &[(&str, &[&str])] = &[
    (
        "vendor_metadata",
        &[
            "audio_file_id",
            "import_id",
            "field",
            "value",
            "source_file",
            "sheet",
            "row",
        ],
    ),
    (
        "vendor_import_log",
        &[
            "import_id",
            "audio_file_id",
            "before_rows",
            "before_vendor_json",
            "before_derived",
        ],
    ),
    (
        "audiolib_vendor_baseline",
        &["audio_file_id", "rows_json", "vendor_json"],
    ),
    (
        "audiolib_vendor_jobs",
        &[
            "job_order",
            "import_id",
            "idempotency_key",
            "options_hash",
            "state",
            "checkpoint",
            "written",
            "restored",
            "created",
            "preview_json",
            "options_json",
            "commit_result",
        ],
    ),
    (
        "audiolib_vendor_events",
        &[
            "import_id",
            "audio_file_id",
            "field",
            "value",
            "source_file",
            "sheet",
            "row",
            "overwrite",
        ],
    ),
];

pub(super) fn schema_exists(conn: &Connection) -> Result<bool> {
    conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='audiolib_vendor_jobs')", [], |r| r.get(0)).map_err(db_error)
}

fn validate_existing(conn: &Connection, required: bool) -> Result<()> {
    for (table, columns) in TABLES {
        let object: Option<String> = conn
            .query_row(
                "SELECT type FROM sqlite_schema WHERE name=?1",
                [table],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_error)?;
        match object.as_deref() {
            None if !required => continue,
            Some("table") => {}
            _ => {
                return Err(invalid(format!(
                    "Vendor storage object {table} is missing or not a table"
                )));
            }
        }
        // Names are compile-time constants, never user supplied SQL identifiers.
        let mut statement = conn
            .prepare(&format!("PRAGMA table_info(\"{table}\")"))
            .map_err(db_error)?;
        let actual = statement
            .query_map([], |r| r.get::<_, String>(1))
            .map_err(db_error)?
            .collect::<rusqlite::Result<BTreeSet<_>>>()
            .map_err(db_error)?;
        if actual.len() != columns.len() || columns.iter().any(|name| !actual.contains(*name)) {
            return Err(invalid(format!(
                "Unknown vendor storage layout for {table}"
            )));
        }
        let primary: Vec<(String, i64)> = conn
            .prepare(&format!("PRAGMA table_info(\"{table}\")"))
            .map_err(db_error)?
            .query_map([], |r| Ok((r.get(1)?, r.get(5)?)))
            .map_err(db_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db_error)?
            .into_iter()
            .filter(|(_, position)| *position > 0)
            .collect();
        let expected: &[(&str, i64)] = match *table {
            "vendor_metadata" => &[("audio_file_id", 1), ("field", 2), ("value", 3)],
            "vendor_import_log" => &[("import_id", 1), ("audio_file_id", 2)],
            "audiolib_vendor_baseline" => &[("audio_file_id", 1)],
            "audiolib_vendor_jobs" => &[("job_order", 1)],
            "audiolib_vendor_events" => &[
                ("import_id", 1),
                ("audio_file_id", 2),
                ("field", 3),
                ("value", 4),
            ],
            _ => return Err(invalid("Unregistered vendor table")),
        };
        if primary.len() != expected.len()
            || expected.iter().any(|(name, position)| {
                !primary
                    .iter()
                    .any(|(actual, p)| actual.as_str() == *name && p == position)
            })
        {
            return Err(invalid(format!(
                "Unknown vendor primary-key contract for {table}"
            )));
        }
    }
    let index: Option<(String, String)> = conn
        .query_row(
            "SELECT type,tbl_name FROM sqlite_schema WHERE name='audiolib_vendor_events_audio'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(db_error)?;
    match index {
        Some((kind, table)) if kind == "index" && table == "audiolib_vendor_events" => {
            let columns = conn
                .prepare("PRAGMA index_info(audiolib_vendor_events_audio)")
                .map_err(db_error)?
                .query_map([], |r| r.get::<_, String>(2))
                .map_err(db_error)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(db_error)?;
            if columns != ["audio_file_id"] {
                return Err(invalid("Unknown vendor event index layout"));
            }
        }
        None if !required => {}
        _ => {
            return Err(invalid(
                "Vendor event index is missing or occupied by an unknown object",
            ));
        }
    }
    Ok(())
}

/// Create additive owned tables only on a validated writable library. Unknown layouts are refused.
pub(crate) fn ensure_schema(conn: &Connection) -> Result<()> {
    validate_existing(conn, false)?;
    conn.execute_batch(
        "SAVEPOINT audiolib_vendor_schema;
        CREATE TABLE IF NOT EXISTS vendor_metadata(
          audio_file_id INTEGER NOT NULL REFERENCES audio_files(id) ON DELETE CASCADE,
          import_id TEXT NOT NULL, field TEXT NOT NULL, value TEXT NOT NULL,
          source_file TEXT NOT NULL, sheet TEXT NOT NULL, row INTEGER NOT NULL,
          PRIMARY KEY(audio_file_id,field,value));
        CREATE TABLE IF NOT EXISTS vendor_import_log(
          import_id TEXT NOT NULL,audio_file_id INTEGER NOT NULL,
          before_rows TEXT NOT NULL,before_vendor_json TEXT NOT NULL,before_derived TEXT NOT NULL,
          PRIMARY KEY(import_id,audio_file_id));
        CREATE TABLE IF NOT EXISTS audiolib_vendor_baseline(
          audio_file_id INTEGER PRIMARY KEY, rows_json TEXT NOT NULL,vendor_json TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS audiolib_vendor_jobs(
          job_order INTEGER PRIMARY KEY AUTOINCREMENT,import_id TEXT NOT NULL UNIQUE,
          idempotency_key TEXT NOT NULL UNIQUE,options_hash TEXT NOT NULL,state TEXT NOT NULL,
          checkpoint INTEGER NOT NULL DEFAULT 0,written INTEGER NOT NULL DEFAULT 0,
          restored INTEGER NOT NULL DEFAULT 0,created INTEGER NOT NULL,
          preview_json TEXT NOT NULL,options_json TEXT NOT NULL,commit_result TEXT);
        CREATE TABLE IF NOT EXISTS audiolib_vendor_events(
          import_id TEXT NOT NULL REFERENCES audiolib_vendor_jobs(import_id),
          audio_file_id INTEGER NOT NULL REFERENCES audio_files(id) ON DELETE CASCADE,
          field TEXT NOT NULL,value TEXT NOT NULL,source_file TEXT NOT NULL,sheet TEXT NOT NULL,
          row INTEGER NOT NULL,overwrite INTEGER NOT NULL,
          PRIMARY KEY(import_id,audio_file_id,field,value));
        CREATE INDEX IF NOT EXISTS audiolib_vendor_events_audio ON audiolib_vendor_events(audio_file_id);
        RELEASE audiolib_vendor_schema;",
    )
    .map_err(|error| {
        let _ = conn
            .execute_batch("ROLLBACK TO audiolib_vendor_schema; RELEASE audiolib_vendor_schema;");
        db_error(error)
    })?;
    validate_schema(conn)
}

/// Read-only validation for a format that declares vendor storage.
pub(crate) fn validate_schema(conn: &Connection) -> Result<()> {
    validate_existing(conn, true)
}

/// Remove this module's additive structures only inside a caller-owned snapshot transaction.
/// The caller must prove exclusive ownership of the copy. Retain the unknown Metadata
/// mirror and already indexed words: old read-only clients can search all WAV sources.
pub(crate) fn drop_legacy_snapshot_storage(conn: &Connection) -> Result<()> {
    if conn.is_autocommit() {
        return Err(invalid(
            "Legacy vendor stripping requires an owned snapshot transaction",
        ));
    }
    validate_existing(conn, false)?;
    conn.execute_batch(
        "DROP TABLE IF EXISTS audiolib_vendor_events;
      DROP TABLE IF EXISTS vendor_import_log;
      DROP TABLE IF EXISTS audiolib_vendor_baseline;
      DROP TABLE IF EXISTS audiolib_vendor_jobs;
      DROP TABLE IF EXISTS vendor_metadata;",
    )
    .map_err(db_error)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Entry {
    import_id: String,
    field: String,
    value: String,
    source_file: String,
    sheet: String,
    row: u64,
}

fn rows(conn: &Connection, id: i64) -> Result<Vec<Entry>> {
    let mut query = conn.prepare("SELECT import_id,field,value,source_file,sheet,row FROM vendor_metadata WHERE audio_file_id=?1 ORDER BY field,value,import_id").map_err(db_error)?;
    query
        .query_map([id], |row| {
            Ok(Entry {
                import_id: row.get(0)?,
                field: row.get(1)?,
                value: row.get(2)?,
                source_file: row.get(3)?,
                sheet: row.get(4)?,
                row: crate::unsigned(row, 5)?,
            })
        })
        .map_err(db_error)?
        .collect::<rusqlite::Result<_>>()
        .map_err(db_error)
}

pub(super) fn save_before(conn: &Connection, import: &str, id: i64) -> Result<()> {
    let captured:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM vendor_import_log WHERE import_id=?1 AND audio_file_id=?2)",params![import,id],|r|r.get(0)).map_err(db_error)?;
    if captured {
        let baseline: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM audiolib_vendor_baseline WHERE audio_file_id=?1)",
                [id],
                |r| r.get(0),
            )
            .map_err(db_error)?;
        if !baseline {
            return Err(invalid(
                "Vendor baseline is missing for a persisted before-image",
            ));
        }
        return Ok(());
    }
    let (metadata, derived): (String, String) = conn.query_row("SELECT metadata_json,audiolib_tokenize(audiolib_source_text(bwf_description,metadata_json)) FROM audio_files WHERE id=?1", [id], |r| Ok((r.get(0)?, r.get(1)?))).map_err(db_error)?;
    let metadata: Value = serde_json::from_str(&metadata).map_err(invalid)?;
    let vendor = metadata
        .get("vendor_metadata")
        .cloned()
        .unwrap_or(Value::Null);
    let original = json(&rows(conn, id)?)?;
    let vendor = json(&vendor)?;
    conn.execute("INSERT OR IGNORE INTO vendor_import_log(import_id,audio_file_id,before_rows,before_vendor_json,before_derived) VALUES(?1,?2,?3,?4,?5)", params![import,id,original,vendor,derived]).map_err(db_error)?;
    // The baseline is taken from the first persisted before-image, never reconstructed
    // from max IDs or the current post-import projection during recovery.
    conn.execute("INSERT OR IGNORE INTO audiolib_vendor_baseline(audio_file_id,rows_json,vendor_json) SELECT audio_file_id,before_rows,before_vendor_json FROM vendor_import_log WHERE import_id=?1 AND audio_file_id=?2", params![import,id]).map_err(db_error)?;
    Ok(())
}

pub(super) struct EventRow<'a> {
    pub import: &'a str,
    pub id: i64,
    pub field: &'a str,
    pub value: &'a str,
    pub source_file: &'a str,
    pub sheet: &'a str,
    pub row: u64,
    pub overwrite: bool,
}

pub(super) fn add_event(conn: &Connection, event: EventRow<'_>) -> Result<()> {
    conn.execute("INSERT OR IGNORE INTO audiolib_vendor_events(import_id,audio_file_id,field,value,source_file,sheet,row,overwrite) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)", params![event.import,event.id,event.field,event.value,event.source_file,event.sheet,super::sql_u64(event.row)?,event.overwrite]).map_err(db_error)?;
    Ok(())
}

/// Recompute the materialized table and metadata mirror from active immutable events.
/// Distinct source batches can own the same value without duplicating searchable text.
pub(super) fn project(conn: &Connection, id: i64) -> Result<()> {
    let baseline: (String, String) = conn
        .query_row(
            "SELECT rows_json,vendor_json FROM audiolib_vendor_baseline WHERE audio_file_id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(db_error)?;
    let originals: Vec<Entry> = serde_json::from_str(&baseline.0).map_err(invalid)?;
    let mut active = BTreeMap::<(String, String), Entry>::new();
    for entry in originals {
        active.insert((entry.field.clone(), entry.value.clone()), entry);
    }
    let mut query = conn.prepare("SELECT e.import_id,e.field,e.value,e.source_file,e.sheet,e.row,e.overwrite FROM audiolib_vendor_events e JOIN audiolib_vendor_jobs j USING(import_id) WHERE e.audio_file_id=?1 ORDER BY j.job_order,e.field,e.value").map_err(db_error)?;
    let events = query
        .query_map([id], |r| {
            Ok((
                Entry {
                    import_id: r.get(0)?,
                    field: r.get(1)?,
                    value: r.get(2)?,
                    source_file: r.get(3)?,
                    sheet: r.get(4)?,
                    row: crate::unsigned(r, 5)?,
                },
                r.get::<_, bool>(6)?,
            ))
        })
        .map_err(db_error)?;
    let mut overwritten = BTreeSet::new();
    let mut has_events = false;
    for event in events {
        let (entry, overwrite) = event.map_err(db_error)?;
        has_events = true;
        if overwrite && overwritten.insert((entry.import_id.clone(), entry.field.clone())) {
            active.retain(|(field, _), _| field != &entry.field);
        }
        active
            .entry((entry.field.clone(), entry.value.clone()))
            .or_insert(entry);
    }
    conn.execute("DELETE FROM vendor_metadata WHERE audio_file_id=?1", [id])
        .map_err(db_error)?;
    let mut fields = BTreeMap::<String, Vec<String>>::new();
    for entry in active.values() {
        conn.execute("INSERT INTO vendor_metadata(audio_file_id,import_id,field,value,source_file,sheet,row) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![id,entry.import_id,entry.field,entry.value,entry.source_file,entry.sheet,super::sql_u64(entry.row)?]).map_err(db_error)?;
        fields
            .entry(entry.field.clone())
            .or_default()
            .push(entry.value.clone());
    }
    if let Some(values) = fields.get_mut("keywords") {
        let mut keywords = BTreeMap::new();
        for value in values.iter() {
            for term in value
                .split([',', ';', '|', '，', '；'])
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                keywords
                    .entry(term.to_lowercase())
                    .or_insert_with(|| term.to_owned());
            }
        }
        *values = keywords.into_values().collect();
    }
    let preferred:Option<String>=conn.query_row("SELECT j.options_json FROM audiolib_vendor_jobs j WHERE EXISTS(SELECT 1 FROM audiolib_vendor_events e WHERE e.import_id=j.import_id AND e.audio_file_id=?1) ORDER BY j.job_order DESC LIMIT 1",[id],|r|r.get(0)).optional().map_err(db_error)?;
    let english_first = preferred
        .as_deref()
        .and_then(|s| {
            serde_json::from_str::<audiolib_core::vendor_import::VendorImportOptions>(s).ok()
        })
        .and_then(|o| o.preferred_languages.into_iter().next())
        .is_some_and(|s| s.eq_ignore_ascii_case("en"));
    for values in fields.values_mut() {
        values.sort_by_key(|s| {
            (
                s.chars().any(|c| ('\u{3400}'..='\u{9fff}').contains(&c)) == english_first,
                s.clone(),
            )
        });
        values.dedup();
    }
    let baseline_vendor = serde_json::from_str::<Value>(&baseline.1).map_err(invalid)?;
    let mirror = if !has_events {
        baseline_vendor
    } else {
        let mut object = baseline_vendor.as_object().cloned().unwrap_or_default();
        object.insert(
            "fields".into(),
            serde_json::to_value(fields).map_err(invalid)?,
        );
        Value::Object(object)
    };
    let original: String = conn
        .query_row(
            "SELECT metadata_json FROM audio_files WHERE id=?1",
            [id],
            |r| r.get(0),
        )
        .map_err(db_error)?;
    let mut metadata: Value = serde_json::from_str(&original).map_err(invalid)?;
    let object = metadata
        .as_object_mut()
        .ok_or_else(|| invalid("Audio Metadata is not a JSON object"))?;
    if mirror.is_null() {
        object.remove("vendor_metadata");
    } else {
        object.insert("vendor_metadata".into(), mirror);
    }
    let updated = json(&metadata)?;
    // Existing production triggers update contentless FTS in this same transaction.
    // User annotations and embedded fields are never part of this SET clause.
    conn.execute(
        "UPDATE audio_files SET metadata_json=?1 WHERE id=?2",
        params![updated, id],
    )
    .map_err(db_error)?;
    Ok(())
}
