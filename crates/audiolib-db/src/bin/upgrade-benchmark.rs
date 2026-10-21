// crates/audiolib-db/src/bin/upgrade-benchmark.rs
//! Measure a real snapshot upgrade in a disposable owned copy; never upgrade the input.
use audiolib_core::IntegrityReport;
use audiolib_db::{
    SnapshotUpgradeProgress, SqliteProvider, ensure_free_space, snapshot_schema_supported,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    error::Error,
    ffi::OsString,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const USAGE: &str = "Usage: upgrade-benchmark --source <existing-snapshot.db> [--live]

Opens the input read-only and pins one SQLite read transaction while copying it
to a disposable temporary directory. Only that owned copy is upgraded. JSON
results go to stdout; timestamped phase and row progress go to stderr. The copy
is removed on completion. --live exercises resumable WAL startup and its rollback
backup, still ONLY in the disposable copy. Measure process peak memory externally.";

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Measure(PathBuf),
    MeasureLive(PathBuf),
    Help,
}

fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<Command> {
    let mut args = args.into_iter();
    let mut source = None;
    let mut live = false;
    while let Some(arg) = args.next() {
        if arg == "--source" {
            if source.is_some() {
                return Err("--source must be specified exactly once".into());
            }
            let path = args.next().ok_or("--source requires a file path")?;
            if path.is_empty() || path.to_string_lossy().starts_with('-') {
                return Err("--source requires a file path".into());
            }
            source = Some(PathBuf::from(path));
        } else if arg == "--live" && !live {
            live = true;
        } else if arg == "--help" || arg == "-h" {
            return Ok(Command::Help);
        } else {
            return Err("Unknown argument; use --source <existing-snapshot.db>".into());
        }
    }
    let source = source.ok_or("--source is required")?;
    Ok(if live {
        Command::MeasureLive(source)
    } else {
        Command::Measure(source)
    })
}

#[derive(Serialize)]
struct SnapshotState {
    schema_version: i64,
    tokenization_revision: Option<i64>,
    audio_records: u64,
    sqlite_version: String,
    page_size: u64,
    page_count: u64,
}

fn snapshot_state(conn: &Connection) -> Result<SnapshotState> {
    let features: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='audiolib_schema_features')",
        [],
        |row| row.get(0),
    )?;
    let tokenization_revision = if features {
        conn.query_row(
            "SELECT version FROM audiolib_schema_features WHERE feature='tokenization'",
            [],
            |row| row.get(0),
        )
        .optional()?
    } else {
        None
    };
    Ok(SnapshotState {
        schema_version: conn.pragma_query_value(None, "user_version", |row| row.get(0))?,
        tokenization_revision,
        audio_records: u64::try_from(conn.query_row(
            "SELECT COUNT(*) FROM audio_files",
            [],
            |row| row.get::<_, i64>(0),
        )?)?,
        sqlite_version: conn.query_row("SELECT sqlite_version()", [], |row| row.get(0))?,
        page_size: u64::try_from(
            conn.pragma_query_value(None, "page_size", |row| row.get::<_, i64>(0))?,
        )?,
        page_count: u64::try_from(
            conn.pragma_query_value(None, "page_count", |row| row.get::<_, i64>(0))?,
        )?,
    })
}

fn validate_source(conn: &Connection) -> Result<()> {
    let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if !snapshot_schema_supported(version) {
        return Err("Source is outside the supported AudioLib snapshot schema range".into());
    }
    let columns = conn
        .prepare("PRAGMA table_info(audio_files)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if ["id", "file_path", "filename"]
        .iter()
        .any(|required| !columns.iter().any(|column| column == required))
    {
        return Err("Source is not an AudioLib library".into());
    }
    let quick = conn
        .prepare("PRAGMA quick_check")?
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if quick != ["ok"] {
        return Err("Source failed SQLite quick_check".into());
    }
    let foreign_keys = u64::try_from(conn.query_row(
        "SELECT COUNT(*) FROM pragma_foreign_key_check",
        [],
        |row| row.get::<_, i64>(0),
    )?)?;
    if foreign_keys != 0 {
        return Err("Source has foreign-key violations".into());
    }
    Ok(())
}

fn require_integrity(report: &IntegrityReport) -> Result<()> {
    if report.is_ok() {
        Ok(())
    } else {
        Err(format!(
            "Owned snapshot integrity failed: {}",
            report.errors.join("; ")
        )
        .into())
    }
}

fn progress_event(progress: SnapshotUpgradeProgress, started: Instant) -> Value {
    let (phase, completed, total) = match progress {
        SnapshotUpgradeProgress::Preparing => ("preparing", None, None),
        SnapshotUpgradeProgress::BackingUp => ("backing_up", None, None),
        SnapshotUpgradeProgress::Copying { completed, total } => {
            ("copying", Some(completed), Some(total))
        }
        SnapshotUpgradeProgress::Compacting => ("compacting", None, None),
        SnapshotUpgradeProgress::Publishing => ("publishing", None, None),
        SnapshotUpgradeProgress::Validating => ("validating", None, None),
        SnapshotUpgradeProgress::Checkpoint => ("checkpoint", None, None),
        SnapshotUpgradeProgress::Indexing { completed, total } => {
            ("indexing", Some(completed), Some(total))
        }
        SnapshotUpgradeProgress::Tokenizing { completed, total } => {
            ("tokenizing", Some(completed), Some(total))
        }
        SnapshotUpgradeProgress::BuildingFts => ("building_fts", None, None),
        SnapshotUpgradeProgress::SourceTags => ("source_tags", None, None),
    };
    json!({
        "phase": phase,
        "completed": completed,
        "total": total,
        "elapsed_seconds": started.elapsed().as_secs_f64(),
        "timestamp_unix_ms": SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis(),
    })
}

fn measure(path: &Path) -> Result<Value> {
    measure_mode(path, false)
}

fn measure_mode(path: &Path, live: bool) -> Result<Value> {
    if !path.is_file() {
        return Err("--source must name an existing snapshot file".into());
    }
    let overall = Instant::now();
    let source = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    source.busy_timeout(Duration::from_secs(5))?;
    source.execute_batch("PRAGMA query_only=ON; BEGIN;")?;
    // The first read pins one source snapshot, including its committed WAL contents.
    let validated = Instant::now();
    validate_source(&source)?;
    let source_state = snapshot_state(&source)?;
    let source_validation_seconds = validated.elapsed().as_secs_f64();
    let directory = tempfile::Builder::new()
        .prefix("audiolib-upgrade-benchmark-")
        .tempdir()?;
    let owned_path = directory.path().join("snapshot.db");
    eprintln!(
        "{}",
        json!({"phase":"copying", "owned_directory": directory.path()})
    );
    let estimated_bytes = source_state
        .page_size
        .checked_mul(source_state.page_count)
        .ok_or("Source size overflow")?;
    ensure_free_space(directory.path(), estimated_bytes)?;
    let copying = Instant::now();
    let mut owned = Connection::open(&owned_path)?;
    rusqlite::backup::Backup::new(&source, &mut owned)?.run_to_completion(
        4096,
        Duration::ZERO,
        None,
    )?;
    let copy_seconds = copying.elapsed().as_secs_f64();
    source.execute_batch("ROLLBACK;")?;
    drop(source);
    // All subsequent writes and full FTS5 integrity checks are confined to this copy.
    owned.execute_batch("PRAGMA journal_mode=DELETE;")?;
    drop(owned);
    let validating = Instant::now();
    let before_integrity = SqliteProvider::check_owned_file(&owned_path)?;
    require_integrity(&before_integrity)?;
    let owned_validation_seconds = validating.elapsed().as_secs_f64();
    let upgrading = Instant::now();
    let mut events = Vec::new();
    let mut report = |progress| {
        let event = progress_event(progress, upgrading);
        eprintln!("{event}");
        events.push(event);
    };
    let changed = if live {
        let changed = !SqliteProvider::has_current_search_state(&owned_path)?;
        let provider = SqliteProvider::open_existing_with_progress(&owned_path, &mut report)?;
        let wal_bytes = std::fs::metadata(format!("{}-wal", owned_path.display()))
            .map(|m| m.len())
            .unwrap_or(0);
        if wal_bytes != 0 {
            return Err("Live upgrade retained a nonempty WAL".into());
        }
        drop(provider);
        changed
    } else {
        SqliteProvider::upgrade_owned_snapshot_with_progress_cancellable(
            &owned_path,
            &AtomicBool::new(false),
            &mut report,
        )?
    };
    let upgrade_seconds = upgrading.elapsed().as_secs_f64();
    let verifying = Instant::now();
    // This is already an exclusively owned copy: validation needs no second full copy.
    let after_integrity = SqliteProvider::check_owned_file(&owned_path)?;
    require_integrity(&after_integrity)?;
    let after = Connection::open_with_flags(&owned_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let upgraded_state = snapshot_state(&after)?;
    if upgraded_state.audio_records != source_state.audio_records {
        return Err("Upgrade changed the number of audio records".into());
    }
    drop(after);
    let post_validation_seconds = verifying.elapsed().as_secs_f64();
    // Complete cleanup before reporting success, including any owned SQLite sidecars.
    directory.close()?;
    Ok(json!({
        "benchmark": "owned_snapshot_upgrade",
        "mode": if live { "resumable_live_wal" } else { "atomic_owned_delete" },
        "version": env!("CARGO_PKG_VERSION"),
        "os": std::env::consts::OS,
        "architecture": std::env::consts::ARCH,
        "source": source_state,
        "upgraded": upgraded_state,
        "upgrade_required": changed,
        "source_validation_seconds": source_validation_seconds,
        "copy_seconds": copy_seconds,
        "owned_validation_seconds": owned_validation_seconds,
        "upgrade_seconds": upgrade_seconds,
        "post_validation_seconds": post_validation_seconds,
        "total_seconds": overall.elapsed().as_secs_f64(),
        "before_integrity": before_integrity,
        "after_integrity": after_integrity,
        "events": events,
        "conditions": "Read-only source pinned during backup; real recorded revision, without forced downgrade. Source quick_check, FK checks and owned FTS5 validation precede upgrade and warm filesystem caches. Atomic mode uses one DELETE transaction; live mode uses resumable WAL batches and includes rollback backup validation, index verification and checked checkpoints. Post-validation runs in the owned copy. Temporary files removed; process peak memory, disk peak and source checksum measured externally.",
    }))
}

fn main() -> Result<()> {
    match parse_args(std::env::args_os().skip(1))? {
        Command::Help => println!("{USAGE}"),
        Command::Measure(source) => {
            println!("{}", serde_json::to_string_pretty(&measure(&source)?)?)
        }
        Command::MeasureLive(source) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&measure_mode(&source, true)?)?
            )
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_require_one_explicit_source() {
        let parse = |args: &[&str]| parse_args(args.iter().map(|arg| OsString::from(*arg)));
        assert!(parse(&[]).is_err());
        assert!(parse(&["--source"]).is_err());
        assert!(parse(&["--unknown"]).is_err());
        assert!(parse(&["--source", "a.db", "--source", "b.db"]).is_err());
        assert_eq!(
            parse(&["--source", "a.db"]).unwrap(),
            Command::Measure("a.db".into())
        );
        assert_eq!(parse(&["--help"]).unwrap(), Command::Help);
        assert_eq!(
            parse(&["--source", "a.db", "--live"]).unwrap(),
            Command::MeasureLive("a.db".into())
        );
    }

    #[test]
    fn measurement_never_creates_a_missing_source() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("missing.db");
        assert!(measure(&missing).is_err());
        assert!(!missing.exists());
    }

    #[test]
    fn measurement_upgrades_only_its_owned_copy_and_preserves_the_source() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("source.db");
        let conn = Connection::open(&database).unwrap();
        conn.execute_batch(include_str!("../../tests/fixtures/schema-v5.sql"))
            .unwrap();
        conn.execute_batch(
            "INSERT INTO audio_files(id,file_path,filename,user_tags,user_notes,favorite,user_color,metadata_json)
             VALUES(1,'root/clip.wav','clip.wav','retained tag','retained note',1,'#abcdef',
             '{\"unknown_extension\":{\"retained\":true}}');
             UPDATE audiolib_schema_features SET version=2 WHERE feature='tokenization';",
        )
        .unwrap();
        drop(conn);
        let before = std::fs::read(&database).unwrap();

        let report = measure(&database).unwrap();

        assert_eq!(report["source"]["tokenization_revision"], 2);
        assert_eq!(report["upgraded"]["tokenization_revision"], 3);
        assert_eq!(report["source"]["audio_records"], 1);
        assert_eq!(report["upgraded"]["audio_records"], 1);
        assert_eq!(report["upgrade_required"], true);
        for name in ["before_integrity", "after_integrity"] {
            let integrity: IntegrityReport = serde_json::from_value(report[name].clone()).unwrap();
            assert!(integrity.is_ok(), "{name}: {integrity:?}");
        }
        assert!(report["events"].as_array().unwrap().iter().any(|event| {
            event["phase"] == "indexing" && event["completed"] == 1 && event["total"] == 1
        }));
        assert_eq!(std::fs::read(&database).unwrap(), before);
        let source =
            Connection::open_with_flags(&database, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        assert_eq!(
            snapshot_state(&source).unwrap().tokenization_revision,
            Some(2)
        );
        let annotations: (String, String, bool, String, String) = source
            .query_row(
                "SELECT user_tags,user_notes,favorite,user_color,metadata_json FROM audio_files WHERE id=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .unwrap();
        assert_eq!(
            annotations,
            (
                "retained tag".into(),
                "retained note".into(),
                true,
                "#abcdef".into(),
                r#"{"unknown_extension":{"retained":true}}"#.into()
            )
        );
    }

    #[test]
    fn measurement_rejects_unrelated_sqlite_without_changing_it() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("unrelated.db");
        let source = Connection::open(&database).unwrap();
        source
            .execute_batch("PRAGMA user_version=5; CREATE TABLE unrelated(value TEXT);")
            .unwrap();
        drop(source);
        let before = std::fs::read(&database).unwrap();
        assert!(measure(&database).is_err());
        assert_eq!(std::fs::read(&database).unwrap(), before);
    }
}
