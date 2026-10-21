// crates/audiolib-db/src/local_play_history.rs
//! Client-local playback events, independent of DataProvider, library files and snapshots.
//! Callers perform writes on a worker and log failures without interrupting playback.
use crate::db_error;
use audiolib_core::{Error, Result};
use chrono::{DateTime, NaiveDate, NaiveDateTime, SecondsFormat, Utc};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

const APPLICATION_ID: i32 = 0x414c5048;
const SCHEMA_VERSION: i32 = 1;
const FORMAT: &str = "audiolib-local-play-history-v1";
const MAX_LIBRARY_ID_BYTES: usize = 150;
const OBJECTS: &[(&str, &str)] = &[
    (
        "plays",
        "CREATE TABLE plays(id INTEGER PRIMARY KEY,library_id TEXT NOT NULL,audio_id INTEGER,file_path TEXT NOT NULL,filename TEXT NOT NULL,played_at TEXT NOT NULL)",
    ),
    (
        "meta",
        "CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT NOT NULL) WITHOUT ROWID",
    ),
    (
        "legacy_events",
        "CREATE TABLE legacy_events(event_sha256 TEXT PRIMARY KEY) WITHOUT ROWID",
    ),
    (
        "idx_plays_played",
        "CREATE INDEX idx_plays_played ON plays(played_at DESC,id DESC)",
    ),
    (
        "idx_plays_key",
        "CREATE INDEX idx_plays_key ON plays(library_id,audio_id,file_path)",
    ),
];

/// One playback event. This type is never part of server or main-library DTOs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalPlayRecord {
    pub id: i64,
    pub library_id: String,
    pub audio_id: Option<i64>,
    pub file_path: String,
    pub filename: String,
    /// UTC RFC3339 with exactly millisecond precision.
    pub played_at: String,
}

/// Current library lookup for an event; the historical path/name are retained.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocatedPlayRecord {
    pub record: LocalPlayRecord,
    pub current_audio_id: Option<i64>,
    pub missing: bool,
}

impl LocalPlayRecord {
    /// Resolve by the caller's current library ID/path rules. Remote paths need not
    /// exist on this machine; no filesystem existence test is performed here.
    pub fn locate(
        &self,
        mut resolve: impl FnMut(Option<i64>, &str) -> Result<Option<i64>>,
    ) -> Result<LocatedPlayRecord> {
        let current_audio_id = resolve(self.audio_id, &self.file_path)?;
        Ok(LocatedPlayRecord {
            record: self.clone(),
            missing: current_audio_id.is_none(),
            current_audio_id,
        })
    }
}

/// Binding supplied only by the local-library migration path after its complete
/// immutable rollback backup has passed SHA256 verification. Server libraries and
/// downloaded snapshots must never be passed to this explicitly local import API.
#[derive(Clone, Copy, Debug)]
pub struct LocalHistoryImportSource<'a> {
    pub library_id: &'a str,
    pub verified_backup_path: &'a Path,
    pub verified_backup_sha256: &'a str,
}

/// Complete import receipt stored atomically with imported events in history.db.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalHistoryImportReport {
    pub binding: String,
    pub library_id: String,
    pub backup_path: PathBuf,
    pub backup_sha256: String,
    pub imported_at: String,
    pub selected: u64,
    pub inserted: u64,
    pub disabled: bool,
    pub already_imported: bool,
}

/// An owned handle to `<data_root>/play-history/<encoded-library-id>/history.db`.
/// The limit affects new writes/imports; opening, reading or disabling never prunes.
#[derive(Clone, Debug)]
pub struct LocalPlayHistory {
    path: PathBuf,
    library_id: String,
    limit: u32,
}

fn invalid(message: impl Into<String>) -> Error {
    Error::Validation(message.into())
}

/// An injective, unpadded lowercase base32 encoding of every UTF-8 byte, including
/// on case-insensitive Windows filesystems. `A`, `a`, `/` and escape-looking IDs
/// stay distinct. The fixed prefix avoids device names; 150 bytes encode to at
/// most 243 component characters. No truncation or filename hash is used.
pub fn safe_library_id(library_id: &str) -> Result<String> {
    if library_id.is_empty() || library_id.len() > MAX_LIBRARY_ID_BYTES {
        return Err(invalid(
            "History library ID must contain 1..=150 UTF-8 bytes",
        ));
    }
    const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
    let mut encoded = String::with_capacity(3 + (library_id.len() * 8).div_ceil(5));
    encoded.push_str("id-");
    let mut buffer = 0u32;
    let mut bits = 0u32;
    for byte in library_id.bytes() {
        buffer = (buffer << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            encoded.push(ALPHABET[((buffer >> bits) & 31) as usize] as char);
        }
        buffer &= (1 << bits) - 1;
    }
    if bits != 0 {
        encoded.push(ALPHABET[(buffer << (5 - bits)) as usize] as char);
    }
    Ok(encoded)
}

/// Stable client-local scope for a library's canonical location, without writing it.
/// Existing files resolve normally; an import destination may be absent when its
/// parent already exists. Moving the library to another path starts another scope.
/// Server IDs remain separate from the `local-<sha256>` namespace used here.
pub fn local_library_id(database_path: &Path) -> Result<String> {
    if database_path.file_name().is_none() {
        return Err(invalid("Local history library needs a file name"));
    }
    let canonical = match database_path.canonicalize() {
        Ok(path) if path.is_file() => path,
        Ok(_) => return Err(invalid("Local history library path is not a file")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let name = database_path
                .file_name()
                .ok_or_else(|| invalid("Local history library needs a file name"))?;
            let parent = database_path
                .parent()
                .filter(|path| !path.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
            let parent = parent.canonicalize()?;
            if !parent.is_dir() {
                return Err(invalid("Local history library parent is not a directory"));
            }
            parent.join(name)
        }
        Err(error) => return Err(error.into()),
    };
    let mut hash = Sha256::new();
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        hash.update(b"audiolib-local-path-windows-v1;");
        // ASCII case is insensitive on ordinary Windows library locations. Do not
        // apply Unicode lowercase: it could merge distinct filesystem names.
        for unit in canonical.as_os_str().encode_wide() {
            let unit = if (u16::from(b'A')..=u16::from(b'Z')).contains(&unit) {
                unit + 32
            } else {
                unit
            };
            hash.update(unit.to_le_bytes());
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        hash.update(b"audiolib-local-path-unix-v1;");
        hash.update(canonical.as_os_str().as_bytes());
    }
    #[cfg(not(any(windows, unix)))]
    {
        hash.update(b"audiolib-local-path-utf8-v1;");
        hash.update(
            canonical
                .to_str()
                .ok_or_else(|| invalid("Local history path is not UTF-8"))?
                .as_bytes(),
        );
    }
    Ok(format!("local-{:x}", hash.finalize()))
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn checked_directory(parent: &Path, name: &str) -> Result<PathBuf> {
    let path = parent.join(name);
    match fs::create_dir(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    let metadata = fs::symlink_metadata(&path)?;
    if !metadata.is_dir() || is_link(&metadata) {
        return Err(invalid(
            "History directory must not be a link or non-directory",
        ));
    }
    let canonical = path.canonicalize()?;
    if canonical.parent() != Some(parent) {
        return Err(invalid(
            "History directory escaped its configured data root",
        ));
    }
    Ok(canonical)
}

fn check_files(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| invalid("History path has no parent"))?;
    let directory_meta = fs::symlink_metadata(parent)?;
    if !directory_meta.is_dir() || is_link(&directory_meta) || parent.canonicalize()? != parent {
        return Err(invalid("History directory identity changed"));
    }
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let file = sidecar(path, suffix);
        match fs::symlink_metadata(&file) {
            Ok(metadata) => {
                if !metadata.is_file()
                    || is_link(&metadata)
                    || file.canonicalize()?.parent() != Some(parent)
                {
                    return Err(invalid(
                        "History database or SQLite sidecar is an unsafe link",
                    ));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn connection(path: &Path, writable: bool) -> Result<Connection> {
    check_files(path)?;
    let flags = if writable {
        OpenFlags::SQLITE_OPEN_READ_WRITE
    } else {
        OpenFlags::SQLITE_OPEN_READ_ONLY
    };
    let conn = Connection::open_with_flags(path, flags | OpenFlags::SQLITE_OPEN_NOFOLLOW)
        .map_err(db_error)?;
    conn.busy_timeout(Duration::from_secs(1))
        .map_err(db_error)?;
    Ok(conn)
}

fn validate(conn: &Connection, library_id: &str) -> Result<()> {
    let application_id: i32 = conn
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .map_err(db_error)?;
    let version: i32 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(db_error)?;
    if application_id != APPLICATION_ID || version != SCHEMA_VERSION {
        return Err(invalid("Not a supported client-local history database"));
    }
    let actual = conn
        .prepare("SELECT name,sql FROM sqlite_schema WHERE name NOT GLOB 'sqlite_*' ORDER BY name")
        .map_err(db_error)?
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(db_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(db_error)?;
    if actual.len() != OBJECTS.len()
        || actual.iter().any(|(name, sql)| {
            !OBJECTS.iter().any(|(expected, definition)| {
                name.as_str() == *expected && crate::schema::definition::equivalent(sql, definition)
            })
        })
    {
        return Err(invalid(
            "History database has an unsupported schema or extra objects",
        ));
    }
    for (key, expected) in [("format", FORMAT), ("library_id", library_id)] {
        let found: Option<String> = conn
            .query_row("SELECT value FROM meta WHERE key=?1", [key], |row| {
                row.get(0)
            })
            .optional()
            .map_err(db_error)?;
        if found.as_deref() != Some(expected) {
            return Err(invalid(
                "History database belongs to another library or format",
            ));
        }
    }
    Ok(())
}

fn timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn legacy_timestamp(value: &str) -> Result<String> {
    if let Ok(value) = DateTime::parse_from_rfc3339(value) {
        return Ok(timestamp(value.with_timezone(&Utc)));
    }
    if let Ok(value) = NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f") {
        return Ok(timestamp(value.and_utc()));
    }
    if let Ok(value) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        return Ok(timestamp(
            value.and_hms_opt(0, 0, 0).expect("midnight").and_utc(),
        ));
    }
    Err(invalid(
        "Legacy playback event has an invalid UTC timestamp",
    ))
}

fn prune(conn: &Connection, limit: u32) -> Result<()> {
    conn.execute("DELETE FROM plays WHERE id NOT IN (SELECT id FROM plays ORDER BY played_at DESC,id DESC LIMIT ?1)", [limit]).map_err(db_error)?;
    Ok(())
}

fn row_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<LocalPlayRecord> {
    Ok(LocalPlayRecord {
        id: row.get(0)?,
        library_id: row.get(1)?,
        audio_id: row.get(2)?,
        file_path: row.get(3)?,
        filename: row.get(4)?,
        played_at: row.get(5)?,
    })
}

impl LocalPlayHistory {
    /// Open/create only this library's client-local event store. `data_root` should
    /// be AppConfig::data_root(), not a library, server-cache or snapshot directory.
    pub fn open(data_root: &Path, library_id: &str, limit: u32) -> Result<Self> {
        let encoded = safe_library_id(library_id)?;
        fs::create_dir_all(data_root)?;
        let data_root = data_root.canonicalize()?;
        let history_root = checked_directory(&data_root, "play-history")?;
        let directory = checked_directory(&history_root, &encoded)?;
        let path = directory.join("history.db");
        check_files(&path)?;
        let created = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => {
                file.sync_all()?;
                true
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => false,
            Err(error) => return Err(error.into()),
        };
        // Inspect an existing file read-only before any journal-mode or schema write.
        // A fresh zero-byte reservation contains no unrelated data and can initialize
        // after an interrupted first creation; all nonempty unknown files are refused.
        let empty = fs::metadata(&path)?.len() == 0;
        if !created && !empty {
            validate(&connection(&path, false)?, library_id)?;
        }
        let mut conn = connection(&path, true)?;
        if created || empty {
            let tx = conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(db_error)?;
            let count: i64 = tx
                .query_row("SELECT COUNT(*) FROM sqlite_schema", [], |row| row.get(0))
                .map_err(db_error)?;
            if count == 0 {
                for (_, sql) in OBJECTS {
                    tx.execute_batch(sql).map_err(db_error)?;
                }
                tx.execute(
                    "INSERT INTO meta VALUES('format',?1),('library_id',?2)",
                    params![FORMAT, library_id],
                )
                .map_err(db_error)?;
                tx.pragma_update(None, "application_id", APPLICATION_ID)
                    .map_err(db_error)?;
                tx.pragma_update(None, "user_version", SCHEMA_VERSION)
                    .map_err(db_error)?;
            }
            tx.commit().map_err(db_error)?;
        }
        validate(&conn, library_id)?;
        let mode: String = conn
            .query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))
            .map_err(db_error)?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(invalid("Client-local history requires WAL mode"));
        }
        conn.execute_batch("PRAGMA synchronous=FULL;")
            .map_err(db_error)?;
        Ok(Self {
            path,
            library_id: library_id.into(),
            limit,
        })
    }

    /// Persistent local file, never a main library or downloadable snapshot.
    pub fn database_path(&self) -> &Path {
        &self.path
    }

    fn read(&self) -> Result<Connection> {
        let conn = connection(&self.path, false)?;
        validate(&conn, &self.library_id)?;
        Ok(conn)
    }

    fn write(&self) -> Result<Connection> {
        self.read()?;
        let conn = connection(&self.path, true)?;
        validate(&conn, &self.library_id)?;
        conn.execute_batch("PRAGMA synchronous=FULL;")
            .map_err(db_error)?;
        Ok(conn)
    }

    /// Add one successful playback event. A zero limit returns None without writes.
    /// No count, last-played field or request to a server is produced.
    pub fn record(
        &self,
        audio_id: Option<i64>,
        file_path: &str,
        filename: &str,
    ) -> Result<Option<LocalPlayRecord>> {
        if self.limit == 0 {
            return Ok(None);
        }
        if file_path.is_empty()
            || file_path.len() > 1_048_576
            || filename.len() > 65_536
            || file_path.contains('\0')
            || filename.contains('\0')
        {
            return Err(invalid(
                "Playback history path/name is empty, oversized or contains NUL",
            ));
        }
        let mut conn = self.write()?;
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let played_at = timestamp(Utc::now());
        tx.execute("INSERT INTO plays(library_id,audio_id,file_path,filename,played_at) VALUES(?1,?2,?3,?4,?5)", params![self.library_id,audio_id,file_path,filename,played_at]).map_err(db_error)?;
        let record = LocalPlayRecord {
            id: tx.last_insert_rowid(),
            library_id: self.library_id.clone(),
            audio_id,
            file_path: file_path.into(),
            filename: filename.into(),
            played_at,
        };
        prune(&tx, self.limit)?;
        tx.commit().map_err(db_error)?;
        Ok(Some(record))
    }

    /// Events newest first, with a stable ID tie break. Reads never apply retention.
    pub fn page(&self, limit: usize, offset: usize) -> Result<Vec<LocalPlayRecord>> {
        if limit == 0 || limit > 10_000 {
            return Err(invalid("History page limit must be 1..=10000"));
        }
        let offset =
            i64::try_from(offset).map_err(|_| invalid("History offset exceeds SQLite range"))?;
        let conn = self.read()?;
        conn.prepare("SELECT id,library_id,audio_id,file_path,filename,played_at FROM plays ORDER BY played_at DESC,id DESC LIMIT ?1 OFFSET ?2").map_err(db_error)?
            .query_map(params![limit as i64,offset], row_record).map_err(db_error)?
            .collect::<rusqlite::Result<Vec<_>>>().map_err(db_error)
    }

    /// Clear only this library's local events. Import receipts stay so clear/reopen
    /// cannot resurrect retired main-library history; no source audio is removed.
    pub fn clear(&self) -> Result<u64> {
        let conn = self.write()?;
        Ok(conn.execute("DELETE FROM plays", []).map_err(db_error)? as u64)
    }

    /// Import only a local library's verified, completed pre-v6 backup. The caller
    /// proves local scope and SHA256 before this call; this method additionally binds
    /// the read-only connection's real path, library identity and the full receipt.
    /// It never writes the source or opens server/snapshot history implicitly.
    pub fn import_v5_local(
        &self,
        backup: &Connection,
        source: LocalHistoryImportSource<'_>,
    ) -> Result<LocalHistoryImportReport> {
        if source.library_id != self.library_id
            || source.verified_backup_sha256.len() != 64
            || !source
                .verified_backup_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(invalid(
                "Legacy history import requires the matching verified local backup",
            ));
        }
        if !backup.is_readonly("main").map_err(db_error)? {
            return Err(invalid(
                "Legacy history backup connection must be read-only",
            ));
        }
        let backup_path = source.verified_backup_path.canonicalize()?;
        let opened = backup
            .path()
            .ok_or_else(|| invalid("Legacy history backup must be a persistent file"))?;
        if Path::new(opened).canonicalize()? != backup_path
            || is_link(&fs::symlink_metadata(source.verified_backup_path)?)
            || backup_path == self.path
        {
            return Err(invalid(
                "Legacy history connection does not match the verified backup path",
            ));
        }
        for suffix in ["-wal", "-journal"] {
            match fs::metadata(sidecar(&backup_path, suffix)) {
                Ok(metadata) if metadata.len() > 0 => {
                    return Err(invalid(
                        "Legacy history requires a completed standalone rollback backup",
                    ));
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        let version: i64 = backup
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(db_error)?;
        if !(1..=5).contains(&version) {
            return Err(invalid(
                "Legacy history import requires a pre-v6 local library backup",
            ));
        }
        crate::schema::validate_library_identity(backup)?;
        let binding_data = serde_json::to_vec(&(
            FORMAT,
            &self.library_id,
            &backup_path,
            source.verified_backup_sha256.to_ascii_lowercase(),
        ))
        .map_err(|error| invalid(error.to_string()))?;
        let binding = format!("{:x}", Sha256::digest(binding_data));
        let key = format!("imported_from_db_v5:{binding}");
        let mut conn = self.write()?;
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let previous: Option<String> = tx
            .query_row("SELECT value FROM meta WHERE key=?1", [&key], |row| {
                row.get(0)
            })
            .optional()
            .map_err(db_error)?;
        if let Some(previous) = previous {
            let mut report: LocalHistoryImportReport =
                serde_json::from_str(&previous).map_err(|error| invalid(error.to_string()))?;
            if report.library_id != self.library_id
                || report.backup_path != backup_path
                || !report
                    .backup_sha256
                    .eq_ignore_ascii_case(source.verified_backup_sha256)
                || report.binding != binding
            {
                return Err(invalid("Legacy history receipt binding is inconsistent"));
            }
            report.already_imported = true;
            return Ok(report);
        }
        let mut report = LocalHistoryImportReport {
            binding,
            library_id: self.library_id.clone(),
            backup_path,
            backup_sha256: source.verified_backup_sha256.to_ascii_lowercase(),
            imported_at: timestamp(Utc::now()),
            selected: 0,
            inserted: 0,
            disabled: self.limit == 0,
            already_imported: false,
        };
        let has_history: bool = backup.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='play_history' COLLATE NOCASE)",[],|row|row.get(0)).map_err(db_error)?;
        if self.limit != 0 && has_history {
            // Retired history may contain orphan audio references. Keep its original
            // path/name and a nullable current audio ID for later path-based location.
            let mut statement = backup.prepare("SELECT ph.id,ph.audio_file_id,af.id,ph.played_at,COALESCE(NULLIF(ph.file_path,''),af.file_path,''),COALESCE(NULLIF(ph.filename,''),af.filename,'') FROM play_history ph LEFT JOIN audio_files af ON af.id=ph.audio_file_id ORDER BY ph.played_at DESC,ph.id DESC LIMIT ?1").map_err(db_error)?;
            let rows = statement
                .query_map([self.limit], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, Option<i64>>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                })
                .map_err(db_error)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(db_error)?;
            report.selected = rows.len() as u64;
            // Oldest first preserves the old ID ordering for equal timestamps.
            for (legacy_id, original_audio_id, audio_id, played_at, file_path, filename) in
                rows.into_iter().rev()
            {
                let played_at = legacy_timestamp(&played_at)?;
                let event = serde_json::to_vec(&(
                    &self.library_id,
                    legacy_id,
                    original_audio_id,
                    &played_at,
                    &file_path,
                    &filename,
                ))
                .map_err(|error| invalid(error.to_string()))?;
                let event_sha256 = format!("{:x}", Sha256::digest(event));
                if tx
                    .execute(
                        "INSERT OR IGNORE INTO legacy_events VALUES(?1)",
                        [event_sha256],
                    )
                    .map_err(db_error)?
                    != 0
                {
                    tx.execute("INSERT INTO plays(library_id,audio_id,file_path,filename,played_at) VALUES(?1,?2,?3,?4,?5)",params![self.library_id,audio_id,file_path,filename,played_at]).map_err(db_error)?;
                    report.inserted += 1;
                }
            }
            prune(&tx, self.limit)?;
        }
        let encoded = serde_json::to_string(&report).map_err(|error| invalid(error.to_string()))?;
        tx.execute("INSERT INTO meta VALUES(?1,?2)", params![key, encoded])
            .map_err(db_error)?;
        tx.commit().map_err(db_error)?;
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn legacy(path: &Path, bad_time: bool) -> (Connection, String) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE audio_files(id INTEGER PRIMARY KEY,file_path TEXT,filename TEXT);
             CREATE TABLE play_history(id INTEGER PRIMARY KEY,audio_file_id INTEGER,played_at TEXT,filename TEXT,file_path TEXT);
             INSERT INTO audio_files VALUES(1,'one.wav','one.wav'),(2,'two.wav','two.wav'),(3,'three.wav','three.wav');
             INSERT INTO play_history VALUES(1,1,'2024-01-01 12:34:56.123','one.wav','one.wav'),
               (2,2,'2024-01-02 12:34:56.789','two.wav','two.wav'),
               (3,3,'2024-01-02 12:34:56.789','three.wav','three.wav');
             PRAGMA user_version=5;",
        ).unwrap();
        if bad_time {
            conn.execute("UPDATE play_history SET played_at='invalid' WHERE id=3", [])
                .unwrap();
        }
        drop(conn);
        let sha256 = format!("{:x}", Sha256::digest(fs::read(path).unwrap()));
        let readonly = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        (readonly, sha256)
    }

    #[test]
    fn safe_ids_are_injective_on_windows_and_fit_component_boundaries() {
        let values = [
            "A", "a", "/", "_2f", "..", "CON", "con", "中文", "a\\b", "a/b", "id-ie", "\0",
        ];
        let mut seen = BTreeSet::new();
        for value in values {
            let encoded = safe_library_id(value).unwrap();
            assert!(
                encoded
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            );
            assert!(seen.insert(encoded.to_ascii_lowercase()), "{value:?}");
        }
        for value in ["a".repeat(128), "界".repeat(50)] {
            let encoded = safe_library_id(&value).unwrap();
            assert_eq!(encoded.len(), 3 + (value.len() * 8).div_ceil(5));
            assert!(encoded.len() < 255);
            let mut distinct = value.clone();
            distinct.replace_range(0..value.chars().next().unwrap().len_utf8(), "A");
            assert_ne!(encoded, safe_library_id(&distinct).unwrap());
        }
        assert!(safe_library_id("").is_err());
        assert!(safe_library_id(&"x".repeat(151)).is_err());
        for (value, expected) in [
            ("f", "my"),
            ("fo", "mzxq"),
            ("foo", "mzxw6"),
            ("foob", "mzxw6yq"),
            ("fooba", "mzxw6ytb"),
            ("foobar", "mzxw6ytboi"),
        ] {
            assert_eq!(safe_library_id(value).unwrap(), format!("id-{expected}"));
        }
    }

    #[test]
    fn local_scope_is_canonical_and_supports_a_future_import_destination() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("Library.db");
        let future = local_library_id(&destination).unwrap();
        fs::write(&destination, b"unchanged").unwrap();
        assert_eq!(future, local_library_id(&destination).unwrap());
        assert_eq!(
            future,
            local_library_id(&root.path().join(".").join("Library.db")).unwrap()
        );
        assert_ne!(
            future,
            local_library_id(&root.path().join("Other.db")).unwrap()
        );
        assert!(local_library_id(&root.path().join("missing").join("library.db")).is_err());
        assert!(local_library_id(root.path()).is_err());
        assert_eq!(fs::read(destination).unwrap(), b"unchanged");
    }

    #[test]
    fn records_are_events_with_paging_retention_and_independent_library_scopes() {
        let root = tempfile::tempdir().unwrap();
        let history = LocalPlayHistory::open(root.path(), "A", 2).unwrap();
        let other = LocalPlayHistory::open(root.path(), "a", 2).unwrap();
        for id in 1..=4 {
            history.record(Some(id), "same.wav", "same.wav").unwrap();
        }
        other.record(None, "remote/path.wav", "remote.wav").unwrap();
        other.record(None, "remote/path.wav", "remote.wav").unwrap();
        assert_eq!(
            other.page(10, 0).unwrap().len(),
            2,
            "each playback is an event"
        );
        let page = history.page(10, 0).unwrap();
        assert_eq!(page.len(), 2);
        assert_eq!(
            page.iter()
                .map(|row| row.audio_id.unwrap())
                .collect::<Vec<_>>(),
            [4, 3]
        );
        assert_eq!(history.page(1, 1).unwrap(), vec![page[1].clone()]);
        for record in &page {
            let parsed = DateTime::parse_from_rfc3339(&record.played_at).unwrap();
            assert_eq!(parsed.offset().local_minus_utc(), 0);
            assert_eq!(parsed.timestamp_subsec_nanos() % 1_000_000, 0);
            assert_eq!(record.played_at.len(), 24);
        }
        assert_eq!(
            LocalPlayHistory::open(root.path(), "A", 2)
                .unwrap()
                .page(10, 0)
                .unwrap(),
            page
        );
        let disabled = LocalPlayHistory::open(root.path(), "A", 0).unwrap();
        assert!(
            disabled
                .record(Some(99), "disabled.wav", "disabled.wav")
                .unwrap()
                .is_none()
        );
        assert_eq!(disabled.page(10, 0).unwrap(), page);
        fs::create_dir(root.path().join("cache")).unwrap();
        fs::write(root.path().join("cache/snapshot.db"), b"disposable").unwrap();
        fs::remove_dir_all(root.path().join("cache")).unwrap();
        assert_eq!(history.page(10, 0).unwrap(), page);
        assert_eq!(history.clear().unwrap(), 2);
        assert!(history.page(10, 0).unwrap().is_empty());
        let remote = other.page(10, 0).unwrap().remove(0);
        assert!(remote.audio_id.is_none());
        assert!(remote.locate(|_, _| Ok(None)).unwrap().missing);
        let found = remote
            .locate(|old, path| {
                assert_eq!(old, None);
                assert_eq!(path, "remote/path.wav");
                Ok(Some(42))
            })
            .unwrap();
        assert_eq!(found.current_audio_id, Some(42));
        assert!(!found.missing);
        assert!(history.page(0, 0).is_err());
    }

    #[test]
    fn local_import_is_atomic_capped_and_bound_to_each_verified_backup() {
        let root = tempfile::tempdir().unwrap();
        let backup_path = root.path().join("original-v5.db");
        let (backup, sha256) = legacy(&backup_path, false);
        let original = fs::read(&backup_path).unwrap();
        let history = LocalPlayHistory::open(root.path(), "local-test", 2).unwrap();
        let source = LocalHistoryImportSource {
            library_id: "local-test",
            verified_backup_path: &backup_path,
            verified_backup_sha256: &sha256,
        };
        let report = history.import_v5_local(&backup, source).unwrap();
        assert_eq!((report.selected, report.inserted), (2, 2));
        assert!(!report.already_imported);
        let rows = history.page(10, 0).unwrap();
        assert_eq!(
            rows.iter().map(|row| row.audio_id).collect::<Vec<_>>(),
            [Some(3), Some(2)]
        );
        assert_eq!(rows[0].played_at, "2024-01-02T12:34:56.789Z");
        assert!(
            history
                .import_v5_local(&backup, source)
                .unwrap()
                .already_imported
        );
        assert_eq!(history.page(10, 0).unwrap(), rows);
        let next_path = root.path().join("rollback-new-backup.db");
        fs::copy(&backup_path, &next_path).unwrap();
        let next =
            Connection::open_with_flags(&next_path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let repeated = history
            .import_v5_local(
                &next,
                LocalHistoryImportSource {
                    verified_backup_path: &next_path,
                    ..source
                },
            )
            .unwrap();
        assert_ne!(repeated.binding, report.binding);
        assert_eq!(repeated.inserted, 0);
        assert_eq!(history.page(10, 0).unwrap(), rows);
        history.clear().unwrap();
        assert!(
            history
                .import_v5_local(&backup, source)
                .unwrap()
                .already_imported
        );
        assert!(history.page(10, 0).unwrap().is_empty());
        drop(next);
        let changed = Connection::open(&next_path).unwrap();
        changed.execute_batch("INSERT INTO play_history VALUES(4,1,'2025-01-01 00:00:00.001','later.wav','later.wav');").unwrap();
        drop(changed);
        let changed_sha256 = format!("{:x}", Sha256::digest(fs::read(&next_path).unwrap()));
        let changed =
            Connection::open_with_flags(&next_path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let newer = history
            .import_v5_local(
                &changed,
                LocalHistoryImportSource {
                    verified_backup_path: &next_path,
                    verified_backup_sha256: &changed_sha256,
                    ..source
                },
            )
            .unwrap();
        assert_eq!((newer.selected, newer.inserted), (2, 1));
        assert_ne!(newer.binding, repeated.binding);
        let new_rows = history.page(10, 0).unwrap();
        assert_eq!(new_rows.len(), 1);
        assert_eq!(new_rows[0].file_path, "later.wav");
        assert_eq!(fs::read(&backup_path).unwrap(), original);
        assert_eq!(
            backup
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            5
        );
        assert!(
            history
                .import_v5_local(
                    &backup,
                    LocalHistoryImportSource {
                        library_id: "other",
                        ..source
                    }
                )
                .is_err()
        );
    }

    #[test]
    fn mixed_case_legacy_table_and_orphan_events_are_preserved_with_complete_receipt() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("mixed-case-v5.db");
        let (backup, _) = legacy(&path, false);
        drop(backup);
        let edit = Connection::open(&path).unwrap();
        edit.execute_batch(
            "ALTER TABLE play_history RENAME TO temporary_history_name;
             ALTER TABLE temporary_history_name RENAME TO Play_History;
             INSERT INTO Play_History VALUES(4,999,'2024-01-03 12:34:56.001','orphan.wav','old/orphan.wav'),
               (5,998,'2024-01-03 12:34:56.001',NULL,NULL);
             UPDATE Play_History SET filename='',file_path='' WHERE id=2;",
        ).unwrap();
        drop(edit);
        let original = fs::read(&path).unwrap();
        let sha256 = format!("{:x}", Sha256::digest(&original));
        let backup = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let history = LocalPlayHistory::open(root.path(), "local-test", 10).unwrap();
        let report = history
            .import_v5_local(
                &backup,
                LocalHistoryImportSource {
                    library_id: "local-test",
                    verified_backup_path: &path,
                    verified_backup_sha256: &sha256,
                },
            )
            .unwrap();
        assert_eq!((report.selected, report.inserted), (5, 5));
        let rows = history.page(10, 0).unwrap();
        assert_eq!(rows.len(), 5);
        assert_eq!(
            (
                rows[0].audio_id,
                rows[0].file_path.as_str(),
                rows[0].filename.as_str()
            ),
            (None, "", "")
        );
        assert_eq!(
            (
                rows[1].audio_id,
                rows[1].file_path.as_str(),
                rows[1].filename.as_str()
            ),
            (None, "old/orphan.wav", "orphan.wav")
        );
        assert_eq!(
            (
                rows[3].audio_id,
                rows[3].file_path.as_str(),
                rows[3].filename.as_str()
            ),
            (Some(2), "two.wav", "two.wav")
        );
        let stored: String = history
            .read()
            .unwrap()
            .query_row(
                "SELECT value FROM meta WHERE key=?1",
                [format!("imported_from_db_v5:{}", report.binding)],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<LocalHistoryImportReport>(&stored).unwrap(),
            report
        );
        assert_eq!(fs::read(path).unwrap(), original);
    }

    #[test]
    fn failed_import_rolls_back_rows_dedup_state_and_receipt_without_touching_v5() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("bad-v5.db");
        let (backup, sha256) = legacy(&path, true);
        let original = fs::read(&path).unwrap();
        let history = LocalPlayHistory::open(root.path(), "local-test", 10).unwrap();
        let source = LocalHistoryImportSource {
            library_id: "local-test",
            verified_backup_path: &path,
            verified_backup_sha256: &sha256,
        };
        assert!(history.import_v5_local(&backup, source).is_err());
        assert!(history.page(10, 0).unwrap().is_empty());
        let audit = history.read().unwrap();
        assert_eq!(
            audit
                .query_row("SELECT COUNT(*) FROM legacy_events", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            audit
                .query_row(
                    "SELECT COUNT(*) FROM meta WHERE key GLOB 'imported_from_db_v5:*'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        assert_eq!(fs::read(path).unwrap(), original);
    }

    #[test]
    fn disabled_import_records_success_without_deleting_existing_local_events() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("v5.db");
        let (backup, sha256) = legacy(&path, false);
        let history = LocalPlayHistory::open(root.path(), "local-test", 3).unwrap();
        history.record(Some(100), "kept.wav", "kept.wav").unwrap();
        let disabled = LocalPlayHistory::open(root.path(), "local-test", 0).unwrap();
        let report = disabled
            .import_v5_local(
                &backup,
                LocalHistoryImportSource {
                    library_id: "local-test",
                    verified_backup_path: &path,
                    verified_backup_sha256: &sha256,
                },
            )
            .unwrap();
        assert!(report.disabled);
        assert_eq!((report.selected, report.inserted), (0, 0));
        assert_eq!(disabled.page(10, 0).unwrap().len(), 1);
    }

    #[test]
    fn foreign_unknown_and_replaced_databases_are_rejected_without_writes() {
        let root = tempfile::tempdir().unwrap();
        let history = LocalPlayHistory::open(root.path(), "lib", 3).unwrap();
        let conn = history.write().unwrap();
        conn.pragma_update(None, "user_version", 2).unwrap();
        drop(conn);
        let original = fs::read(history.database_path()).unwrap();
        assert!(LocalPlayHistory::open(root.path(), "lib", 3).is_err());
        assert_eq!(fs::read(history.database_path()).unwrap(), original);
        let other_dir = root
            .path()
            .join("play-history")
            .join(safe_library_id("other").unwrap());
        fs::create_dir(&other_dir).unwrap();
        let path = other_dir.join("history.db");
        let unrelated = Connection::open(&path).unwrap();
        unrelated
            .execute_batch(
                "CREATE TABLE important(value TEXT); INSERT INTO important VALUES('keep');",
            )
            .unwrap();
        drop(unrelated);
        let original = fs::read(&path).unwrap();
        assert!(LocalPlayHistory::open(root.path(), "other", 3).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        assert!(!sidecar(&path, "-wal").exists());
        assert!(!sidecar(&path, "-shm").exists());
    }

    #[test]
    fn write_errors_are_reported_without_modifying_main_library_or_events() {
        let root = tempfile::tempdir().unwrap();
        let main = root.path().join("library.db");
        fs::write(&main, b"untouched library").unwrap();
        let history = LocalPlayHistory::open(root.path(), "lib", 3).unwrap();
        let blocker = history.write().unwrap();
        blocker.execute_batch("BEGIN IMMEDIATE;").unwrap();
        assert!(history.record(Some(1), "one.wav", "one.wav").is_err());
        blocker.execute_batch("ROLLBACK;").unwrap();
        assert!(history.page(10, 0).unwrap().is_empty());
        assert_eq!(fs::read(main).unwrap(), b"untouched library");
    }

    #[cfg(unix)]
    #[test]
    fn linked_history_directories_and_sqlite_sidecars_are_not_followed() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("play-history")).unwrap();
        assert!(LocalPlayHistory::open(root.path(), "lib", 2).is_err());
        fs::remove_file(root.path().join("play-history")).unwrap();
        let history = LocalPlayHistory::open(root.path(), "lib", 2).unwrap();
        let precious = outside.path().join("precious");
        fs::write(&precious, b"keep").unwrap();
        std::os::unix::fs::symlink(&precious, sidecar(history.database_path(), "-wal")).unwrap();
        assert!(history.record(Some(1), "one.wav", "one.wav").is_err());
        assert_eq!(fs::read(precious).unwrap(), b"keep");
    }
}
