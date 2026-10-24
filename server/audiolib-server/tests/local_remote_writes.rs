// server/audiolib-server/tests/local_remote_writes.rs
//! Local edits concurrent with server annotations must invalidate incremental snapshots.
use audiolib_core::{BatchMetadataRequest, DataProvider, MetadataPatch};
use audiolib_db::SqliteProvider;
use audiolib_remote::{CacheState, CacheUpdate, RemoteCache, apply_server_write};
use audiolib_server::{AppState, ServerConfig};

struct Fixture {
    state: AppState,
    local: SqliteProvider,
    cache: RemoteCache,
    initial: CacheState,
    _root: tempfile::TempDir,
}

fn fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("authority.db");
    let local = SqliteProvider::open(&database).unwrap();
    {
        let conn = rusqlite::Connection::open(&database).unwrap();
        audiolib_db::register_library_functions(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO audio_files(id,file_path,path_key,filename) VALUES
             (1,'/a.wav','/a.wav','a.wav'),(2,'/b.wav','/b.wav','b.wav'),
             (3,'/c.wav','/c.wav','c.wav');",
        )
        .unwrap();
    }
    let config = ServerConfig::from_toml_str(
        "bind='127.0.0.1:0'\nlibrary_db='authority.db'\nsnapshot_dir='snapshots'\npregenerate_list_waveforms=false\n",
        root.path(),
    ).unwrap();
    let state = AppState::open(&config, None).unwrap();
    let revision = state.poll_revision().unwrap();
    let cache = RemoteCache::new(&root.path().join("client"));
    let directory = cache.library_dir(&revision.library_id);
    std::fs::create_dir_all(&directory).unwrap();
    state
        .provider()
        .backup_library(&directory.join("snapshot.db"))
        .unwrap();
    let initial = CacheState {
        library_id: revision.library_id,
        revision: revision.revision,
        schema_version: audiolib_db::SCHEMA_VERSION,
        database_file: "snapshot.db".into(),
        ..Default::default()
    };
    cache.save_state(&initial).unwrap();
    Fixture {
        state,
        local,
        cache,
        initial,
        _root: root,
    }
}

fn notes(value: &str) -> MetadataPatch {
    MetadataPatch {
        notes: Some(value.into()),
        ..Default::default()
    }
}

#[test]
fn isolated_single_and_batch_annotations_keep_incremental_cache_updates() {
    for batch in [false, true] {
        let fixture = fixture();
        let (files, before, after) = if batch {
            let reply = fixture
                .state
                .update_metadata_batch(&BatchMetadataRequest {
                    ids: vec![1, 2],
                    patch: notes("remote"),
                    ..Default::default()
                })
                .unwrap();
            (reply.files, reply.revision_before, reply.revision)
        } else {
            let reply = fixture.state.update_metadata(1, &notes("remote")).unwrap();
            (vec![reply.file], reply.revision_before, reply.revision)
        };
        assert_eq!(before, fixture.initial.revision);
        assert_eq!(after, before + 1);
        assert_eq!(
            apply_server_write(
                &fixture.cache,
                &fixture.initial.library_id,
                &files,
                before,
                after
            ),
            CacheUpdate::Applied { revision: after }
        );
        assert_eq!(fixture.state.poll_revision().unwrap().revision, after);
        let snapshot =
            SqliteProvider::open_read_only(&fixture.cache.database_path(&fixture.initial)).unwrap();
        for file in files {
            let cached = snapshot.get_audio(file.id).unwrap();
            assert_eq!(cached.notes, "remote");
            assert_eq!(
                cached.attributes["modified_at"],
                file.attributes["modified_at"]
            );
        }
    }
}

#[test]
fn incomplete_timestamp_replies_require_resync_without_advancing_the_cache() {
    let fixture = fixture();
    let reply = fixture.state.update_metadata(1, &notes("remote")).unwrap();
    for timestamp in [None, Some(serde_json::json!("invalid"))] {
        let mut file = reply.file.clone();
        if let Some(timestamp) = timestamp {
            file.attributes["modified_at"] = timestamp;
        } else {
            file.attributes
                .as_object_mut()
                .unwrap()
                .remove("modified_at");
        }
        assert!(matches!(
            apply_server_write(
                &fixture.cache,
                &fixture.initial.library_id,
                &[file],
                reply.revision_before,
                reply.revision
            ),
            CacheUpdate::Resync { .. }
        ));
        assert_eq!(fixture.cache.active().unwrap(), fixture.initial);
        let snapshot =
            SqliteProvider::open_read_only(&fixture.cache.database_path(&fixture.initial)).unwrap();
        assert_eq!(snapshot.get_audio(1).unwrap().notes, "");
    }
}

#[test]
fn overlapping_local_edits_require_resync_for_single_and_batch_annotations() {
    for batch in [false, true] {
        let fixture = fixture();
        let reply = fixture
            .state
            .write(|provider| {
                fixture
                    .local
                    .update_metadata(3, &notes("local concurrent edit"))?;
                if batch {
                    provider.update_metadata_batch(&BatchMetadataRequest {
                        ids: vec![1, 2],
                        patch: notes("remote"),
                        ..Default::default()
                    })?;
                    Ok(vec![provider.get_audio(1)?, provider.get_audio(2)?])
                } else {
                    Ok(vec![provider.update_metadata(1, &notes("remote"))?])
                }
            })
            .unwrap();
        assert!(reply.revision_before > fixture.initial.revision);
        assert!(matches!(
            apply_server_write(
                &fixture.cache,
                &fixture.initial.library_id,
                &reply.value,
                reply.revision_before,
                reply.revision
            ),
            CacheUpdate::Resync { .. }
        ));
        assert_eq!(fixture.cache.active().unwrap(), fixture.initial);
        assert_eq!(
            fixture.state.poll_revision().unwrap().revision,
            reply.revision
        );
        assert_eq!(
            fixture.state.provider().get_audio(3).unwrap().notes,
            "local concurrent edit"
        );
    }
}
