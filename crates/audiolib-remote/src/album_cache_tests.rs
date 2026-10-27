// crates/audiolib-remote/src/album_cache_tests.rs
//! Legacy server albums are upgraded only in owned files and retain their provenance.
use super::*;
use audiolib_core::{DataProvider, compatibility::SnapshotFormat};

pub(super) fn legacy_seven(path: &Path) {
    drop(SqliteProvider::open(path).unwrap());
    let conn = rusqlite::Connection::open(path).unwrap();
    audiolib_db::register_library_functions(&conn).unwrap();
    conn.execute_batch(
        "PRAGMA journal_mode=DELETE;
         DROP TRIGGER IF EXISTS audiolib_album_audio_insert;
         DROP TRIGGER IF EXISTS audiolib_album_audio_update;
         DROP TRIGGER IF EXISTS audiolib_album_folder_delete;
         DROP TRIGGER IF EXISTS audiolib_album_id_high_water;
         DROP INDEX idx_audio_visible_album;
         DROP INDEX IF EXISTS idx_audio_album_members;
         ALTER TABLE audio_files DROP COLUMN album_id;
         DROP TABLE albums;
         DROP TABLE audiolib_album_id_sequence;
         ALTER TABLE audio_files ADD COLUMN album TEXT NOT NULL DEFAULT '';
         CREATE INDEX idx_audio_visible_album ON audio_files(album,id) WHERE missing=0;
         DELETE FROM audiolib_schema_features WHERE feature IN ('album_entities','metadata_storage');
         DELETE FROM library_meta WHERE key='metadata_storage_policy';
         INSERT INTO audio_files(id,file_path,path_key,filename,metadata_json,user_notes,modified_at,album)
         VALUES(100,'one.wav','one.wav','one.wav','{\"wav_metadata\":{\"fields\":{\"library\":\"Shared\"}},\"extension\":{\"keep\":true}}','retained','2020-01-02 03:04:05','Shared'),
               (200,'two.wav','two.wav','two.wav','{\"wav_metadata\":{\"fields\":{\"library\":\"Shared\"}}}','second','2020-01-02 03:04:05','Shared');
         PRAGMA user_version=7;",
    ).unwrap();
}

#[test]
fn legacy_seven_album_projection_keeps_source_and_is_reused_with_annotation_replies() {
    let root = tempfile::tempdir().unwrap();
    let cache = RemoteCache::new(root.path());
    let mut state = CacheState {
        library_id: "legacy-albums".into(),
        revision: 12,
        schema_version: 7,
        database_file: "original.db".into(),
        server_url: "http://legacy.test/api/v1".into(),
        ..Default::default()
    };
    std::fs::create_dir_all(cache.library_dir(&state.library_id)).unwrap();
    let source = cache.database_path(&state);
    legacy_seven(&source);
    let cancel = AtomicBool::new(false);
    let source_format = observed_format(&source, &cancel).unwrap();
    state.format = Some(Box::new(CacheFormat {
        source: Some(source_format.clone()),
        local: source_format.clone(),
        generation: 0,
    }));
    cache.save_state(&state).unwrap();
    let original = std::fs::read(&source).unwrap();
    let prepared = cache.prepare_cached_state(&state).unwrap();
    assert_ne!(prepared.database_file, state.database_file);
    assert_eq!(prepared.schema_version, audiolib_db::SCHEMA_VERSION);
    assert_eq!(std::fs::read(&source).unwrap(), original);
    assert_eq!(
        prepared.format.as_ref().unwrap().source.as_ref(),
        Some(&source_format)
    );
    assert_eq!(
        prepared.format.as_ref().unwrap().local,
        LibraryFormat::target()
    );
    assert_eq!(cache.prepare_cached_state(&prepared).unwrap(), prepared);
    let server = RemoteLibrary {
        library_id: state.library_id.clone(),
        revision: state.revision,
        schema_version: 7,
        snapshot_format: Some(SnapshotFormat {
            id: audiolib_core::compatibility::PLAN_B_V7_FORMAT.into(),
            format: source_format,
        }),
        ..Default::default()
    };
    assert!(reusable_source(&prepared, &server, &state.server_url));
    let provider = SqliteProvider::open_read_only(&cache.database_path(&prepared)).unwrap();
    let mut file = provider.get_audio(100).unwrap();
    let album_id = file.album_id.unwrap();
    assert_eq!(provider.get_audio(200).unwrap().album_id, Some(album_id));
    let entity = file.album_entity.clone();
    file.album_id = None;
    file.album_entity = None;
    file.notes = "legacy reply annotations".into();
    file.attributes["modified_at"] = serde_json::json!("2026-10-28 12:00:00");
    assert!(matches!(
        crate::apply_server_write(&cache, &state.library_id, &[file], 12, 13),
        crate::CacheUpdate::Applied { revision: 13 }
    ));
    let updated = provider.get_audio(100).unwrap();
    assert_eq!(updated.album_id, Some(album_id));
    assert_eq!(updated.album_entity, entity);
    assert_eq!(updated.notes, "legacy reply annotations");
    assert_eq!(updated.attributes["extension"]["keep"], true);
    assert_eq!(std::fs::read(source).unwrap(), original);
}

#[test]
fn legacy_sync_inherits_album_ids_and_namespace_without_repeated_downloads() {
    use crate::test_server::{TestResponse, TestServer};
    use std::sync::{Arc, atomic::AtomicUsize};
    let root = tempfile::tempdir().unwrap();
    let first = root.path().join("first.db");
    let second = root.path().join("second.db");
    legacy_seven(&first);
    legacy_seven(&second);
    let conn = rusqlite::Connection::open(&second).unwrap();
    audiolib_db::register_library_functions(&conn).unwrap();
    conn.execute_batch("INSERT INTO audio_files(id,file_path,path_key,filename,metadata_json,album) VALUES(1,'earlier.wav','earlier.wav','earlier.wav','{\"wav_metadata\":{\"fields\":{\"library\":\"Before\"}}}','Before');").unwrap();
    drop(conn);
    let cancel = AtomicBool::new(false);
    let source_format = observed_format(&first, &cancel).unwrap();
    let payloads = [
        std::fs::read(&first).unwrap(),
        std::fs::read(&second).unwrap(),
    ];
    let generation = Arc::new(AtomicUsize::new(0));
    let active = generation.clone();
    let downloads = Arc::new(AtomicUsize::new(0));
    let counted = downloads.clone();
    let server = TestServer::start(move |request| {
        let index = active.load(Ordering::Acquire);
        let revision = index + 1;
        match request.path.as_str() {
            "/api/v1/health" => crate::test_server::marker_health(),
            "/api/v1/capabilities" => crate::test_server::marker_capabilities(serde_json::json!({})),
            "/api/v1/roots" => TestResponse::json(200, "[]"),
            "/api/v1/library" => TestResponse::json(200, &serde_json::json!({
                "library_id":"stable-albums", "revision":revision, "schema_version":7,
                "snapshot_format":{"id":audiolib_core::compatibility::PLAN_B_V7_FORMAT,"format":source_format}
            }).to_string()),
            "/api/v1/library/snapshot" => {
                counted.fetch_add(1, Ordering::Relaxed);
                TestResponse::bytes(200, payloads[index].clone())
                    .with_header("x-library-id", "stable-albums")
                    .with_header("x-library-revision", &revision.to_string())
                    .with_header("x-schema-version", "7")
                    .with_header(audiolib_core::compatibility::SERVER_COMPATIBILITY_HEADER, &serde_json::to_string(&audiolib_core::compatibility::compatibility_manifest()).unwrap())
                    .with_header(audiolib_core::compatibility::SNAPSHOT_FORMAT_HEADER, audiolib_core::compatibility::PLAN_B_V7_FORMAT)
                    .with_header(audiolib_core::compatibility::LIBRARY_FORMAT_HEADER, &serde_json::to_string(&source_format).unwrap())
                    .with_header("x-search-storage-revision", "1")
                    .with_header("x-tokenization-revision", "3")
                    .with_header("x-source-search-revision", "2")
                    .with_header("x-source-tags-revision", "2")
            }
            _ => TestResponse::json(404, "{}"),
        }
    });
    let cache = RemoteCache::new(&root.path().join("cache"));
    let client = RemoteClient::new(&server.url, "token").unwrap();
    let before = sync_library(&client, &cache, &mut |_| {}, &cancel, false).unwrap();
    let old = SqliteProvider::open_read_only(&cache.database_path(&before.state)).unwrap();
    let original_id = old.get_audio(100).unwrap().album_id;
    let namespace = old.album_namespace();
    generation.store(1, Ordering::Release);
    let after = sync_library(&client, &cache, &mut |_| {}, &cancel, false).unwrap();
    let new = SqliteProvider::open_read_only(&cache.database_path(&after.state)).unwrap();
    assert_eq!(new.get_audio(100).unwrap().album_id, original_id);
    assert_eq!(new.album_namespace(), namespace);
    assert!(new.get_audio(1).unwrap().album_id > original_id);
    let reused = sync_library(&client, &cache, &mut |_| {}, &cancel, false).unwrap();
    assert!(!reused.downloaded);
    assert_eq!(reused.state.database_file, after.state.database_file);
    assert_eq!(downloads.load(Ordering::Acquire), 2);
}
