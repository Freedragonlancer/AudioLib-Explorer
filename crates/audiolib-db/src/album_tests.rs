// crates/audiolib-db/src/album_tests.rs
//! Album upgrades, old snapshots and scan preservation use only owned fixtures.
use crate::*;
use audiolib_core::{DataProvider, SearchFilters};

fn previous_fixture() -> (tempfile::TempDir, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("previous.db");
    drop(SqliteProvider::open(&path).unwrap());
    let conn = Connection::open(&path).unwrap();
    register_library_functions(&conn).unwrap();
    conn.execute_batch(
        "PRAGMA journal_mode=DELETE;
         INSERT INTO audio_files(id,file_path,filename,metadata_json,user_notes,modified_at)
         VALUES(-9223372036854775808,'lowest.wav','metal.wav','{\"wav_metadata\":{\"fields\":{\"library\":\"Library A\"}},\"private\":42}','keep note','2020-01-02 03:04:05'),
               (2,'two.wav','water.wav','{\"wav_metadata\":{\"fields\":{\"library\":\"Library B\"}}}','second note','2020-01-02 03:04:05');
         DROP INDEX idx_audio_visible_album;
         ALTER TABLE audio_files DROP COLUMN album;
         PRAGMA user_version=6;",
    ).unwrap();
    drop(conn);
    (root, path)
}

#[test]
fn album_upgrade_respects_scan_administration_and_reader_leases() {
    let (_root, path) = previous_fixture();
    let before = fs::read(&path).unwrap();
    let scan = management::LibraryLease::acquire(&path).unwrap();
    assert!(matches!(
        SqliteProvider::open_existing(&path),
        Err(Error::Busy(_))
    ));
    drop(scan);
    let admin = management::AdministrationLease::exclusive(&path).unwrap();
    assert!(matches!(
        SqliteProvider::open_existing(&path),
        Err(Error::Busy(_))
    ));
    drop(admin);
    let reader = management::SearchUpgradeLease::acquire(&path, false).unwrap();
    assert!(matches!(
        SqliteProvider::open_existing(&path),
        Err(Error::Busy(_))
    ));
    drop(reader);
    assert_eq!(fs::read(&path).unwrap(), before);
    let provider = SqliteProvider::open_existing(&path).unwrap();
    assert_eq!(provider.get_audio(i64::MIN).unwrap().album, "Library A");
    assert_eq!(provider.get_audio(i64::MIN).unwrap().notes, "keep note");
    assert_eq!(
        provider.get_audio(i64::MIN).unwrap().attributes["private"],
        42
    );
    assert_eq!(
        provider.library_info().unwrap().schema_version,
        SCHEMA_VERSION
    );
}

#[test]
fn album_upgrade_cancellation_rolls_back_column_and_rows() {
    let (_root, path) = previous_fixture();
    let before = fs::read(&path).unwrap();
    let cancel = AtomicBool::new(false);
    let result = SqliteProvider::open_existing_with_progress_cancellable(&path, &cancel, |event| {
        if matches!(event, SnapshotUpgradeProgress::Tokenizing { .. }) {
            cancel.store(true, Ordering::Release);
        }
    });
    assert!(matches!(result, Err(Error::Cancelled)));
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(
        album_expression(&Connection::open(&path).unwrap()).unwrap(),
        "audiolib_album_key(af.metadata_json)"
    );
}

#[test]
fn album_upgrade_does_not_publish_damaged_identity_or_relationships() {
    for damage in [
        "UPDATE audiolib_audio_id_sequence SET high_water=0",
        "PRAGMA foreign_keys=OFF; INSERT INTO playlist_items(playlist_id,audio_file_id) VALUES(123,2)",
    ] {
        let (_root, path) = previous_fixture();
        Connection::open(&path)
            .unwrap()
            .execute_batch(damage)
            .unwrap();
        let before = fs::read(&path).unwrap();
        assert!(SqliteProvider::open_existing(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
    }
}

#[test]
fn previous_read_only_snapshots_support_album_facets_filters_and_metadata_patches() {
    let (_root, path) = previous_fixture();
    let before = fs::read(&path).unwrap();
    let provider = SqliteProvider::open_read_only(&path).unwrap();
    assert_eq!(
        provider.album_facets(&SearchRequest::default()).unwrap(),
        vec![("Library A".into(), 1), ("Library B".into(), 1)]
    );
    let request = SearchRequest {
        filters: SearchFilters {
            album: Some("Library A".into()),
            ..Default::default()
        },
        ..Default::default()
    };
    let page = provider.search(&request).unwrap();
    assert_eq!(page.total_count, 1);
    assert_eq!(page.results[0].id, i64::MIN);
    assert_eq!(page.results[0].album, "Library A");
    let query = SearchRequest {
        query: "metal".into(),
        ..Default::default()
    };
    assert_eq!(
        provider.album_facets(&query).unwrap(),
        vec![("Library A".into(), 1)]
    );
    assert_eq!(fs::read(&path).unwrap(), before);
    assert!(matches!(
        provider.album_facets_cancellable(&query, Arc::new(AtomicBool::new(true))),
        Err(Error::Cancelled)
    ));
    let mut patched = page.results[0].clone();
    patched.notes = "updatedunique".into();
    SqliteProvider::apply_snapshot_records(&path, &[patched]).unwrap();
    assert_eq!(
        provider
            .search(&SearchRequest {
                query: "updatedunique".into(),
                ..Default::default()
            })
            .unwrap()
            .total_count,
        1
    );
}

#[test]
fn captured_v5_migration_publishes_a_filled_album_column_before_open_returns() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("legacy.db");
    let conn = Connection::open(&path).unwrap();
    register_library_functions(&conn).unwrap();
    conn.execute_batch(include_str!("../tests/fixtures/schema-v5.sql"))
        .unwrap();
    conn.execute_batch(
        "INSERT INTO audio_files(id,file_path,filename,user_notes,metadata_json)
         VALUES(1,'one.wav','metal.wav','retain annotation','{\"wav_metadata\":{\"fields\":{\"library\":\"Captured Album\"}},\"private\":42}');"
    ).unwrap();
    drop(conn);
    let provider = SqliteProvider::open_existing(&path).unwrap();
    let file = provider.get_audio(1).unwrap();
    assert_eq!(file.album, "Captured Album");
    assert_eq!(file.notes, "retain annotation");
    assert_eq!(file.attributes["private"], 42);
    assert!(schema::current_format_ready(&Connection::open(&path).unwrap()).unwrap());
    assert_eq!(
        provider.album_facets(&SearchRequest::default()).unwrap(),
        vec![("Captured Album".into(), 1)]
    );
}

#[cfg(feature = "scanner")]
#[test]
fn rescanning_updates_declared_album_and_preserves_user_annotations() {
    let root = tempfile::tempdir().unwrap();
    let provider = SqliteProvider::open(&root.path().join("scan.db")).unwrap();
    let mut item = ScannedAudio {
        path: root.path().join("one.wav"),
        root: root.path().to_owned(),
        size: 100,
        mtime_ns: 1,
        metadata: AudioMetadata {
            attributes: serde_json::json!({"wav_metadata":{"fields":{"library":"Library A"}}}),
            ..Default::default()
        },
    };
    let session = provider.start_scan(root.path(), false).unwrap();
    provider
        .write_scan_batch(std::slice::from_ref(&item), session)
        .unwrap();
    let file = provider
        .search(&SearchRequest::default())
        .unwrap()
        .results
        .remove(0);
    provider
        .update_metadata(
            file.id,
            &MetadataPatch {
                notes: Some("preserve notes".into()),
                tags: Some("my tag".into()),
                favorite: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
    item.metadata.attributes =
        serde_json::json!({"wav_metadata":{"fields":{"library":"Library B"}}});
    item.mtime_ns = 2;
    provider.write_scan_batch(&[item], session).unwrap();
    let updated = provider.get_audio(file.id).unwrap();
    assert_eq!(updated.album, "Library B");
    assert_eq!(updated.notes, "preserve notes");
    assert_eq!(updated.tags, "my tag");
    assert!(updated.favorite);
    assert_eq!(
        provider.album_facets(&SearchRequest::default()).unwrap(),
        vec![("Library B".into(), 1)]
    );
}
