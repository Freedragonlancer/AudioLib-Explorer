// crates/audiolib-db/tests/library_management.rs
use audiolib_core::*;
use audiolib_db::{ScanCancellation, ScanOptions, ScanService, SqliteProvider};
use rusqlite::Connection;
use std::{fs, path::Path, sync::Arc};

fn wav(path: &Path, rate: u32) {
    let mut bytes = b"RIFF".to_vec();
    bytes.extend(38u32.to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16u32.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(rate.to_le_bytes());
    bytes.extend((rate * 2).to_le_bytes());
    bytes.extend(2u16.to_le_bytes());
    bytes.extend(16u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend(2u32.to_le_bytes());
    bytes.extend(0i16.to_le_bytes());
    fs::write(path, bytes).unwrap();
}
struct Fixture {
    temp: tempfile::TempDir,
    provider: Arc<SqliteProvider>,
    ids: Vec<i64>,
    paths: Vec<std::path::PathBuf>,
}
fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("audio");
    fs::create_dir_all(root.join("child")).unwrap();
    let paths = vec![root.join("metal.wav"), root.join("child").join("wood.wav")];
    for path in &paths {
        wav(path, 48000);
    }
    let provider = Arc::new(SqliteProvider::open(&temp.path().join("library.db")).unwrap());
    ScanService::new(provider.clone())
        .scan(
            &root,
            &ScanOptions::default(),
            &ScanCancellation::default(),
            |_| {},
        )
        .unwrap();
    let ids = paths
        .iter()
        .map(|path| provider.find_audio_by_path(path).unwrap().unwrap().id)
        .collect();
    Fixture {
        temp,
        provider,
        ids,
        paths,
    }
}
fn tags(f: &Fixture, id: i64) -> String {
    f.provider.get_audio(id).unwrap().tags
}
fn patch(tags: &str) -> MetadataPatch {
    MetadataPatch {
        tags: Some(tags.into()),
        ..Default::default()
    }
}

#[test]
fn version_two_upgrade_preserves_old_counts_without_inventing_events() {
    let f = fixture();
    let conn = Connection::open(f.provider.database_path()).unwrap();
    conn.execute(
        "UPDATE audio_files SET play_count=7 WHERE id=?1",
        [f.ids[0]],
    )
    .unwrap();
    conn.execute_batch("DROP TABLE play_history;PRAGMA user_version=2;")
        .unwrap();
    drop(conn);
    let upgraded = SqliteProvider::open_existing(f.provider.database_path()).unwrap();
    assert_eq!(upgraded.library_info().unwrap().schema_version, 5);
    assert_eq!(upgraded.get_audio(f.ids[0]).unwrap().play_count, 7);
    assert!(upgraded.play_history(20, 0).unwrap().is_empty());
    upgraded.record_play(f.ids[0]).unwrap();
    assert_eq!(upgraded.get_audio(f.ids[0]).unwrap().play_count, 8);
    assert_eq!(upgraded.play_history(20, 0).unwrap().len(), 1);
    assert!(upgraded.record_play(99999).is_err());
    assert_eq!(upgraded.play_history(20, 0).unwrap().len(), 1);
    assert!(upgraded.scan_history(0, 0).is_err());
}

#[test]
fn missing_visibility_lookup_and_stats_cover_hidden_records() {
    let f = fixture();
    let conn = Connection::open(f.provider.database_path()).unwrap();
    conn.execute("UPDATE audio_files SET missing=1 WHERE id=?1", [f.ids[0]])
        .unwrap();
    assert!(
        f.provider
            .find_audio_by_path(&f.paths[0])
            .unwrap()
            .unwrap()
            .missing
    );
    assert_eq!(f.provider.stats().unwrap().missing_files, 1);
    for (visibility, count) in [
        (FileVisibility::Present, 0),
        (FileVisibility::Missing, 1),
        (FileVisibility::All, 1),
    ] {
        let page = f
            .provider
            .search(&SearchRequest {
                query: "metal".into(),
                filters: SearchFilters {
                    visibility,
                    ..Default::default()
                },
                ..Default::default()
            })
            .unwrap();
        assert_eq!(page.total_count, count);
        if count > 0 {
            assert!(page.results[0].missing);
        }
    }
    assert_eq!(
        f.provider
            .search(&SearchRequest {
                filters: SearchFilters {
                    visibility: FileVisibility::All,
                    ..Default::default()
                },
                ..Default::default()
            })
            .unwrap()
            .total_count,
        2
    );
}

#[test]
fn batch_edits_are_atomic_deduplicated_and_unicode_token_aware() {
    let f = fixture();
    let id = f.ids[0];
    f.provider
        .update_metadata(id, &patch("ÉCHO, stone"))
        .unwrap();
    assert!(
        f.provider
            .update_metadata_batch(&BatchMetadataRequest {
                ids: vec![id, 99999],
                patch: patch("must roll back"),
                ..Default::default()
            })
            .is_err()
    );
    assert_eq!(tags(&f, id), "ÉCHO, stone");
    let result = f
        .provider
        .update_metadata_batch(&BatchMetadataRequest {
            ids: vec![id, id],
            patch: patch("écho; metal"),
            tag_mode: TagEditMode::Append,
        })
        .unwrap();
    assert_eq!(result.updated, 1);
    assert_eq!(tags(&f, id), "ÉCHO, stone, metal");
    f.provider
        .update_metadata_batch(&BatchMetadataRequest {
            ids: vec![id],
            patch: patch("écho"),
            tag_mode: TagEditMode::Remove,
        })
        .unwrap();
    assert_eq!(tags(&f, id), "stone, metal");
    assert!(
        f.provider
            .update_metadata_batch(&BatchMetadataRequest {
                ids: vec![id],
                patch: MetadataPatch {
                    color: Some("bad".into()),
                    ..Default::default()
                },
                ..Default::default()
            })
            .is_err()
    );
    assert_eq!(
        f.provider
            .search(&SearchRequest {
                query: "stone".into(),
                ..Default::default()
            })
            .unwrap()
            .total_count,
        1
    );
    assert!(fs::read(&f.paths[0]).unwrap().starts_with(b"RIFF"));
}

#[test]
fn backups_restore_wal_annotations_with_warm_readers_and_keep_rollback_copy() {
    let f = fixture();
    let id = f.ids[0];
    let original = fs::read(&f.paths[0]).unwrap();
    f.provider.update_metadata(id, &patch("snapshot")).unwrap();
    f.provider.record_play(id).unwrap();
    let backup = f.temp.path().join("backup.db");
    let report = f.provider.backup_library(&backup).unwrap();
    assert_eq!(report.total_records, 2);
    assert!(report.bytes > 0);
    let backup_bytes = fs::read(&backup).unwrap();
    assert!(f.provider.backup_library(&backup).is_err());
    assert_eq!(fs::read(&backup).unwrap(), backup_bytes);
    f.provider.update_metadata(id, &patch("later")).unwrap();
    f.provider.record_play(id).unwrap();
    assert_eq!(
        f.provider
            .search(&SearchRequest {
                query: "later".into(),
                ..Default::default()
            })
            .unwrap()
            .total_count,
        1
    );
    let restored = f.provider.restore_library(&backup).unwrap();
    assert!(restored.rollback_backup.is_file());
    assert_eq!(tags(&f, id), "snapshot");
    assert_eq!(f.provider.get_audio(id).unwrap().play_count, 1);
    assert_eq!(
        f.provider
            .search(&SearchRequest {
                query: "later".into(),
                ..Default::default()
            })
            .unwrap()
            .total_count,
        0
    );
    assert_eq!(
        f.provider
            .search(&SearchRequest {
                query: "snapshot".into(),
                ..Default::default()
            })
            .unwrap()
            .total_count,
        1
    );
    let rollback = SqliteProvider::open_read_only(&restored.rollback_backup).unwrap();
    assert_eq!(rollback.get_audio(id).unwrap().tags, "later");
    assert_eq!(rollback.get_audio(id).unwrap().play_count, 2);
    assert!(f.provider.check_library().unwrap().is_ok());
    assert_eq!(fs::read(&f.paths[0]).unwrap(), original);
}

#[test]
fn readonly_check_export_and_backup_do_not_change_source_and_reject_admin_writes() {
    let f = fixture();
    let snapshot = f.temp.path().join("readonly.db");
    f.provider.backup_library(&snapshot).unwrap();
    let before = fs::read(&snapshot).unwrap();
    let readonly = SqliteProvider::open_read_only(&snapshot).unwrap();
    assert!(readonly.check_library().unwrap().is_ok());
    readonly
        .export_library(
            &f.temp.path().join("readonly.jsonl"),
            LibraryExportFormat::JsonLines,
        )
        .unwrap();
    readonly
        .backup_library(&f.temp.path().join("readonly-copy.db"))
        .unwrap();
    assert!(
        readonly
            .restore_library(f.provider.database_path())
            .is_err()
    );
    assert!(
        readonly
            .maintain_library(MaintenanceAction::Optimize)
            .is_err()
    );
    assert_eq!(fs::read(&snapshot).unwrap(), before);
}

#[test]
fn invalid_or_future_restore_sources_preserve_current_library_and_existing_open_rejects_legacy() {
    let f = fixture();
    let id = f.ids[0];
    f.provider.update_metadata(id, &patch("safe")).unwrap();
    let corrupt = f.temp.path().join("corrupt.db");
    fs::write(&corrupt, b"not sqlite").unwrap();
    assert!(f.provider.restore_library(&corrupt).is_err());
    assert_eq!(tags(&f, id), "safe");
    let future = f.temp.path().join("future.db");
    f.provider.backup_library(&future).unwrap();
    Connection::open(&future)
        .unwrap()
        .execute_batch("PRAGMA user_version=999;")
        .unwrap();
    let before = fs::read(&future).unwrap();
    assert!(f.provider.restore_library(&future).is_err());
    assert!(SqliteProvider::open_existing(&future).is_err());
    assert_eq!(tags(&f, id), "safe");
    assert_eq!(fs::read(&future).unwrap(), before);
    let legacy = f.temp.path().join("legacy.db");
    Connection::open(&legacy)
        .unwrap()
        .execute_batch("CREATE TABLE private(value TEXT);INSERT INTO private VALUES('keep');")
        .unwrap();
    let before = fs::read(&legacy).unwrap();
    assert!(SqliteProvider::open_existing(&legacy).is_err());
    assert!(f.provider.restore_library(&legacy).is_err());
    assert_eq!(tags(&f, id), "safe");
    assert_eq!(f.provider.stats().unwrap().audio_files, 2);
    assert_eq!(fs::read(&legacy).unwrap(), before);
    let empty = f.temp.path().join("empty.db");
    Connection::open(&empty)
        .unwrap()
        .execute_batch("VACUUM;")
        .unwrap();
    let before = fs::read(&empty).unwrap();
    assert!(f.provider.restore_library(&empty).is_err());
    assert_eq!(fs::read(&empty).unwrap(), before);
    assert_eq!(tags(&f, id), "safe");
}

#[test]
fn fts_external_content_and_trigger_damage_is_detected_and_rebuilt_with_rollback() {
    let f = fixture();
    let id = f.ids[0];
    f.provider.update_metadata(id, &patch("known")).unwrap();
    assert_eq!(
        f.provider
            .search(&SearchRequest {
                query: "known".into(),
                ..Default::default()
            })
            .unwrap()
            .total_count,
        1
    );
    let conn = Connection::open(f.provider.database_path()).unwrap();
    conn.execute(
        "INSERT INTO audio_search(audio_search) VALUES('delete-all')",
        [],
    )
    .unwrap();
    let check = f.provider.check_library().unwrap();
    assert!(check.sqlite_ok);
    assert!(!check.fts_ok);
    assert!(!check.is_ok());
    let damaged = f.temp.path().join("damaged.db");
    f.provider.backup_library(&damaged).unwrap();
    assert!(f.provider.restore_library(&damaged).is_err());
    assert!(
        f.provider
            .maintain_library(MaintenanceAction::Optimize)
            .is_err()
    );
    assert!(!f.provider.check_library().unwrap().fts_ok);
    let repaired = f
        .provider
        .maintain_library(MaintenanceAction::RebuildSearch)
        .unwrap();
    assert!(repaired.rollback_backup.is_file());
    assert!(repaired.integrity.is_ok());
    assert_eq!(
        f.provider
            .search(&SearchRequest {
                query: "known".into(),
                ..Default::default()
            })
            .unwrap()
            .total_count,
        1
    );
    conn.execute_batch("DROP TRIGGER audio_files_au;").unwrap();
    let check = f.provider.check_library().unwrap();
    assert!(!check.fts_ok);
    assert!(check.errors.iter().any(|error| error.contains("triggers")));
    assert!(
        f.provider
            .maintain_library(MaintenanceAction::RebuildSearch)
            .unwrap()
            .integrity
            .is_ok()
    );
    f.provider.update_metadata(id, &patch("new term")).unwrap();
    assert_eq!(
        f.provider
            .search(&SearchRequest {
                query: "known".into(),
                ..Default::default()
            })
            .unwrap()
            .total_count,
        0
    );
    assert_eq!(
        f.provider
            .search(&SearchRequest {
                query: "term".into(),
                ..Default::default()
            })
            .unwrap()
            .total_count,
        1
    );
    assert!(
        f.provider
            .maintain_library(MaintenanceAction::Optimize)
            .unwrap()
            .integrity
            .is_ok()
    );
    // A standalone FTS5 replacement must not pass external-content validation.
    let original_sql: String = conn
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE name='audio_search'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let standalone = original_sql.replace("content='audio_files',content_rowid='id',", "");
    assert_ne!(standalone, original_sql);
    conn.execute_batch(&format!("DROP TABLE audio_search; {standalone};"))
        .unwrap();
    assert!(!f.provider.check_library().unwrap().fts_ok);
    assert!(
        f.provider
            .maintain_library(MaintenanceAction::RebuildSearch)
            .unwrap()
            .integrity
            .is_ok()
    );
}

#[test]
fn relink_refreshes_headers_preserves_identity_annotations_and_rejects_collisions() {
    let f = fixture();
    let id = f.ids[0];
    let source_bytes = fs::read(&f.paths[0]).unwrap();
    f.provider
        .update_metadata(
            id,
            &MetadataPatch {
                tags: Some("keep".into()),
                notes: Some("notes".into()),
                favorite: Some(true),
                color: Some("#123456".into()),
            },
        )
        .unwrap();
    f.provider.record_play(id).unwrap();
    assert!(f.provider.relink_audio(id, &f.paths[1]).is_err());
    assert_eq!(f.provider.get_audio(id).unwrap().sample_rate, 48000);
    let target = f.temp.path().join("moved.wav");
    wav(&target, 96000);
    let target_bytes = fs::read(&target).unwrap();
    let updated = f.provider.relink_audio(id, &target).unwrap();
    assert_eq!(updated.id, id);
    assert_eq!(updated.sample_rate, 96000);
    assert_eq!(updated.tags, "keep");
    assert_eq!(updated.play_count, 1);
    assert!(!updated.missing);
    assert_eq!(
        f.provider.find_audio_by_path(&target).unwrap().unwrap().id,
        id
    );
    assert!(
        f.provider
            .find_audio_by_path(&f.paths[0])
            .unwrap()
            .is_none()
    );
    let invalid = f.temp.path().join("bad.wav");
    fs::write(&invalid, b"broken").unwrap();
    assert!(f.provider.relink_audio(id, &invalid).is_err());
    assert_eq!(
        f.provider.find_audio_by_path(&target).unwrap().unwrap().id,
        id
    );
    assert_eq!(fs::read(&f.paths[0]).unwrap(), source_bytes);
    assert_eq!(fs::read(&target).unwrap(), target_bytes);
    assert!(f.provider.check_library().unwrap().is_ok());
}

#[test]
fn recursive_folder_removal_cleans_relations_and_keeps_source_wavs() {
    let f = fixture();
    let child = f.provider.get_audio(f.ids[1]).unwrap().folder_id.unwrap();
    let conn = Connection::open(f.provider.database_path()).unwrap();
    conn.execute_batch("INSERT INTO playlists(id,name) VALUES(1,'keep playlist');")
        .unwrap();
    conn.execute(
        "INSERT INTO playlist_items(playlist_id,audio_file_id) VALUES(1,?1)",
        [f.ids[1]],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO project_references(audio_file_id,project_path) VALUES(?1,'demo.rpp')",
        [f.ids[1]],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO waveform_cache(audio_file_id,zoom_level) VALUES(?1,'overview')",
        [f.ids[1]],
    )
    .unwrap();
    f.provider.record_play(f.ids[1]).unwrap();
    assert_eq!(f.provider.remove_folder(child).unwrap(), 1);
    assert!(f.provider.get_audio(f.ids[1]).is_err());
    for table in [
        "playlist_items",
        "project_references",
        "waveform_cache",
        "play_history",
    ] {
        assert_eq!(
            conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
    assert_eq!(f.provider.stats().unwrap().audio_files, 1);
    assert!(f.paths.iter().all(|path| path.is_file()));
    assert_eq!(f.provider.remove_audio(&[f.ids[0], f.ids[0]]).unwrap(), 1);
    assert_eq!(f.provider.stats().unwrap().audio_files, 0);
    assert!(f.provider.check_library().unwrap().is_ok());
}

#[test]
fn full_stream_exports_and_jsonl_csv_annotation_roundtrips_are_transactional() {
    let f = fixture();
    let id = f.ids[0];
    let notes = "line one\r\nline two, \"quoted\" 中文";
    f.provider
        .update_metadata(
            id,
            &MetadataPatch {
                tags: Some("engine, 金属".into()),
                notes: Some(notes.into()),
                favorite: Some(true),
                color: Some("#abcdef".into()),
            },
        )
        .unwrap();
    let conn = Connection::open(f.provider.database_path()).unwrap();
    conn.execute("UPDATE audio_files SET missing=1 WHERE id=?1", [f.ids[1]])
        .unwrap();
    for (extension, format) in [
        ("jsonl", LibraryExportFormat::JsonLines),
        ("csv", LibraryExportFormat::Csv),
    ] {
        let output = f.temp.path().join(format!("export-{extension}.data"));
        let report = f.provider.export_library(&output, format).unwrap();
        assert_eq!(report.records, 2);
        let exported = fs::read(&output).unwrap();
        assert!(f.provider.export_library(&output, format).is_err());
        assert_eq!(fs::read(&output).unwrap(), exported);
        f.provider
            .update_metadata(
                id,
                &MetadataPatch {
                    tags: Some("changed".into()),
                    notes: Some("changed".into()),
                    favorite: Some(false),
                    color: Some(String::new()),
                },
            )
            .unwrap();
        let imported = f.provider.import_annotations(&output).unwrap();
        assert_eq!(imported.records, 2);
        let audio = f.provider.get_audio(id).unwrap();
        assert_eq!(audio.notes, notes);
        assert_eq!(audio.tags, "engine, 金属");
        assert!(audio.favorite);
        assert_eq!(audio.color.as_deref(), Some("#abcdef"));
    }
    let bad = f.temp.path().join("bad.jsonl");
    let row = serde_json::json!({"id":f.ids[1],"file_path":f.provider.get_audio(id).unwrap().file_path,"tags":"rollback"});
    fs::write(&bad, format!("{row}\n{{invalid\n")).unwrap();
    assert!(f.provider.import_annotations(&bad).is_err());
    assert_eq!(tags(&f, id), "engine, 金属");
    let skipped = f.temp.path().join("skipped.jsonl");
    fs::write(
        &skipped,
        "{\"file_path\":\"unknown.wav\",\"tags\":\"skip\"}\n",
    )
    .unwrap();
    assert_eq!(f.provider.import_annotations(&skipped).unwrap().skipped, 1);
}

#[test]
fn administration_shares_scan_lock_while_snapshot_reads_continue() {
    let f = fixture();
    let mut name = f
        .provider
        .database_path()
        .file_name()
        .unwrap()
        .to_os_string();
    name.push(".scan.lock");
    let lock = fs::File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(f.provider.database_path().with_file_name(name))
        .unwrap();
    lock.try_lock().unwrap();
    assert!(matches!(
        f.provider.remove_audio(&[f.ids[0]]),
        Err(Error::Busy(_))
    ));
    assert!(matches!(
        f.provider.update_metadata_batch(&BatchMetadataRequest {
            ids: vec![f.ids[0]],
            patch: patch("busy"),
            ..Default::default()
        }),
        Err(Error::Busy(_))
    ));
    assert!(matches!(
        f.provider.maintain_library(MaintenanceAction::Optimize),
        Err(Error::Busy(_))
    ));
    f.provider
        .backup_library(&f.temp.path().join("while-scanning.db"))
        .unwrap();
    f.provider
        .export_library(
            &f.temp.path().join("while-scanning.jsonl"),
            LibraryExportFormat::JsonLines,
        )
        .unwrap();
    lock.unlock().unwrap();
    assert_eq!(f.provider.remove_audio(&[f.ids[0]]).unwrap(), 1);
}

#[test]
fn root_relocation_refuses_existing_indexed_destinations_and_precommit_cancellation() {
    let f = fixture();
    let source = f.temp.path().join("audio");
    let destination = f.temp.path().join("destination");
    fs::create_dir_all(destination.join("child")).unwrap();
    fs::copy(&f.paths[0], destination.join("metal.wav")).unwrap();
    fs::copy(&f.paths[1], destination.join("child").join("wood.wav")).unwrap();
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(true));
    assert!(matches!(
        f.provider
            .relink_folder_cancellable(&source, &destination, cancel),
        Err(Error::Cancelled)
    ));
    ScanService::new(f.provider.clone())
        .scan(
            &destination,
            &ScanOptions::default(),
            &ScanCancellation::default(),
            |_| {},
        )
        .unwrap();
    assert!(matches!(
        f.provider.relink_folder(&source, &destination),
        Err(Error::Validation(_))
    ));
    for (id, path) in f.ids.iter().zip(&f.paths) {
        assert_eq!(
            f.provider.get_audio(*id).unwrap().file_path,
            audiolib_db::display_path(&path.canonicalize().unwrap())
        );
    }
    assert_eq!(f.provider.stats().unwrap().audio_files, 4);
}

#[test]
fn root_relocation_after_a_case_only_rename_preserves_records_and_source_audio() {
    let f = fixture();
    let source = f.temp.path().join("audio");
    let target = f.temp.path().join("Audio");
    if target.exists() {
        // The regression requires a case-sensitive directory. Windows and some
        // macOS volumes treat these paths as aliases; the Windows no-op test covers it.
        return;
    }
    let original: Vec<_> = f.paths.iter().map(|path| fs::read(path).unwrap()).collect();
    let first = f.provider.get_audio(f.ids[0]).unwrap();
    f.provider
        .update_metadata(
            first.id,
            &MetadataPatch {
                tags: Some("retained tag".into()),
                notes: Some("retained note".into()),
                favorite: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
    f.provider.record_play(first.id).unwrap();
    let conn = Connection::open(f.provider.database_path()).unwrap();
    conn.execute(
        "UPDATE audio_files SET metadata_json=json_set(metadata_json,'$.unknown_extension','keep') WHERE id=?1",
        [first.id],
    )
    .unwrap();
    drop(conn);
    let root_id = f.provider.folders(None).unwrap()[0].id;
    let child_id = f.provider.folders(Some(root_id)).unwrap()[0].id;

    fs::rename(&source, &target).unwrap();
    assert!(!source.exists());
    assert_eq!(f.provider.relink_folder(&source, &target).unwrap(), 2);
    for ((id, relative), bytes) in f
        .ids
        .iter()
        .zip(["metal.wav", "child/wood.wav"])
        .zip(original)
    {
        let moved = target.join(relative);
        let audio = f.provider.get_audio(*id).unwrap();
        assert_eq!(
            audio.file_path,
            audiolib_db::display_path(&moved.canonicalize().unwrap())
        );
        assert_eq!(fs::read(&moved).unwrap(), bytes);
    }
    let moved = f.provider.get_audio(first.id).unwrap();
    assert_eq!(moved.folder_id, first.folder_id);
    assert_eq!(
        (
            moved.tags.as_str(),
            moved.notes.as_str(),
            moved.favorite,
            moved.play_count
        ),
        ("retained tag", "retained note", true, 1)
    );
    assert_eq!(moved.attributes["unknown_extension"], "keep");
    assert_eq!(
        f.provider.get_folder(root_id).unwrap().path,
        audiolib_db::folder_path_text(&target.canonicalize().unwrap())
    );
    assert_eq!(f.provider.folders(Some(root_id)).unwrap()[0].id, child_id);
    assert_eq!(f.provider.stats().unwrap().audio_files, 2);
    assert_eq!(f.provider.play_history(10, 0).unwrap().len(), 1);
    let rescanned = ScanService::new(f.provider.clone())
        .scan(
            &target,
            &ScanOptions::default(),
            &ScanCancellation::default(),
            |_| {},
        )
        .unwrap();
    assert_eq!(rescanned.added, 0);
    assert_eq!(
        f.provider.get_audio(first.id).unwrap().notes,
        "retained note"
    );
}

#[cfg(windows)]
#[test]
fn root_relocation_keeps_windows_case_aliases_as_a_noop() {
    let f = fixture();
    let source = f.temp.path().join("audio");
    let equivalent = std::path::PathBuf::from(source.to_string_lossy().to_uppercase());
    assert!(equivalent.is_dir());
    let before: Vec<_> = f
        .ids
        .iter()
        .map(|id| f.provider.get_audio(*id).unwrap().file_path)
        .collect();
    assert_eq!(f.provider.relink_folder(&source, &equivalent).unwrap(), 0);
    assert_eq!(f.provider.stats().unwrap().audio_files, 2);
    for (id, path) in f.ids.iter().zip(before) {
        assert_eq!(f.provider.get_audio(*id).unwrap().file_path, path);
    }
}

#[test]
fn root_relocation_obeys_cross_process_scan_and_administration_leases() {
    let f = fixture();
    let source = f.temp.path().join("audio");
    let target = f.temp.path().join("target");
    fs::create_dir_all(&target).unwrap();
    for suffix in [".scan.lock", ".admin.lock"] {
        let database = f.provider.database_path().canonicalize().unwrap();
        let mut name = database.file_name().unwrap().to_os_string();
        name.push(suffix);
        let lock = fs::File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(database.with_file_name(name))
            .unwrap();
        lock.try_lock().unwrap();
        assert!(
            matches!(
                f.provider.relink_folder(&source, &target),
                Err(Error::Busy(_))
            ),
            "{suffix}"
        );
        lock.unlock().unwrap();
    }
    assert!(source.join("metal.wav").is_file());
    assert_eq!(f.provider.stats().unwrap().audio_files, 2);
}

#[test]
fn root_relocation_rejects_volume_roots_before_changing_any_paths() {
    let f = fixture();
    let filesystem_root = f.paths[0].ancestors().last().unwrap();
    assert!(matches!(
        f.provider
            .relink_folder(filesystem_root, &f.temp.path().join("audio")),
        Err(Error::Validation(_))
    ));
    assert_eq!(f.provider.stats().unwrap().audio_files, 2);
}
