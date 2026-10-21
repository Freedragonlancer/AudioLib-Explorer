// server/audiolib-server/tests/compatibility.rs
//! Real router and remote-client negotiation against an owned temporary library.
use audiolib_core::{DataProvider, MetadataPatch, SearchRequest, compatibility::PLAN_B_FORMAT};
use audiolib_db::SqliteProvider;
use audiolib_remote::{CacheUpdate, RemoteCache, RemoteClient, RemoteWriter, sync_library};
use audiolib_server::{AppState, BearerToken, ServeOptions, ServerConfig, router, serve};
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use std::{sync::atomic::AtomicBool, time::Duration};
use tower::ServiceExt;

#[tokio::test]
async fn mutation_admission_rejects_invalid_peer_catalogs_without_writing() {
    use audiolib_core::compatibility::{CLIENT_COMPATIBILITY_HEADER, compatibility_manifest};
    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("library.db");
    drop(SqliteProvider::open(&database).unwrap());
    let conn = rusqlite::Connection::open(&database).unwrap();
    audiolib_db::register_library_functions(&conn).unwrap();
    conn.execute("INSERT INTO audio_files(id,file_path,filename,user_notes) VALUES(1,'owned.wav','owned.wav','preserved')", []).unwrap();
    drop(conn);
    let config = ServerConfig::from_toml_str(
        "bind='127.0.0.1:0'\nlibrary_db='library.db'\nsnapshot_dir='snapshots'\nauth=true\n",
        root.path(),
    )
    .unwrap();
    let token = "compatibility-test-token-0123456789";
    let state = AppState::open(&config, BearerToken::new(token).unwrap()).unwrap();
    let revision = state.revision().unwrap().revision;
    let settings_path = config.admin_dir().join("admin-settings.json");
    let original_settings = std::fs::read(&settings_path).ok();
    let conn = rusqlite::Connection::open_with_flags(
        &database,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let data_version: i64 = conn
        .pragma_query_value(None, "data_version", |row| row.get(0))
        .unwrap();
    for mode in [
        "version",
        "mutual_range",
        "protocol",
        "malformed",
        "duplicate",
        "oversized",
    ] {
        let mut catalog = compatibility_manifest();
        match mode {
            "version" => catalog.application_version = "99.0.0".into(),
            "mutual_range" => catalog.peer_application_ranges = vec!["<0.4.0".into()],
            "protocol" => catalog.api_protocols.clear(),
            _ => {}
        }
        let value = match mode {
            "malformed" => "{}".into(),
            "oversized" => "x".repeat(4097),
            _ => serde_json::to_string(&catalog).unwrap(),
        };
        for (path, body) in [
            ("/api/v1/audio/1/metadata", r#"{"notes":"must not apply"}"#),
            ("/api/v1/settings", r#"{"list_waveform_rate":20}"#),
        ] {
            let mut request = Request::patch(path)
                .header("Authorization", format!("Bearer {token}"))
                .header("x-audiolib-library-id", state.library_id())
                .header("Content-Type", "application/json")
                .header(CLIENT_COMPATIBILITY_HEADER, &value);
            if mode == "duplicate" {
                request = request.header(CLIENT_COMPATIBILITY_HEADER, &value);
            }
            let response = router(state.clone())
                .oneshot(request.body(Body::from(body)).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::CONFLICT, "{mode} {path}");
            let body: serde_json::Value =
                serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                    .unwrap();
            assert_eq!(
                body["error_code"], "PEER_VERSION_UNSUPPORTED",
                "{mode} {path}"
            );
            assert_eq!(std::fs::read(&settings_path).ok(), original_settings);
            assert_eq!(state.revision().unwrap().revision, revision);
            assert_eq!(
                conn.pragma_query_value(None, "data_version", |row| row.get::<_, i64>(0))
                    .unwrap(),
                data_version
            );
            assert_eq!(
                conn.query_row("SELECT user_notes FROM audio_files WHERE id=1", [], |row| {
                    row.get::<_, String>(0)
                })
                .unwrap(),
                "preserved"
            );
        }
    }
    // Existing headerless JSON clients retain their compatibility endpoint.
    let response = router(state.clone())
        .oneshot(
            Request::post("/api/v1/audio/1/plays")
                .header("Authorization", format!("Bearer {token}"))
                .header("x-audiolib-library-id", state.library_id())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(state.revision().unwrap().revision, revision);
    assert_eq!(
        conn.pragma_query_value(None, "data_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        data_version
    );

    // Authentication and management policy still take precedence over a bad peer
    // declaration. No settings or database write occurs on either refusal.
    for (authenticated, manage_mode, status, code) in [
        (
            false,
            audiolib_server::ManageMode::All,
            StatusCode::UNAUTHORIZED,
            "UNAUTHORIZED",
        ),
        (
            true,
            audiolib_server::ManageMode::Off,
            StatusCode::FORBIDDEN,
            "MANAGE_FORBIDDEN",
        ),
    ] {
        state
            .security()
            .update_settings(|settings| settings.manage_mode = Some(manage_mode))
            .unwrap();
        let before_settings = std::fs::read(&settings_path).unwrap();
        let mut request = Request::patch("/api/v1/settings")
            .header("Content-Type", "application/json")
            .header(CLIENT_COMPATIBILITY_HEADER, "{}");
        if authenticated {
            request = request.header("Authorization", format!("Bearer {token}"));
        }
        let response = router(state.clone())
            .oneshot(
                request
                    .body(Body::from(r#"{"list_waveform_rate":20}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        let body: serde_json::Value =
            serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        assert_eq!(body["error_code"], code);
        assert_eq!(std::fs::read(&settings_path).unwrap(), before_settings);
        assert_eq!(state.revision().unwrap().revision, revision);
        assert_eq!(
            conn.pragma_query_value(None, "data_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            data_version
        );
    }
    state
        .security()
        .update_settings(|settings| settings.manage_mode = Some(audiolib_server::ManageMode::All))
        .unwrap();
    // Both the explicit legacy exception and a supported modern peer may change
    // settings. These configuration writes must not bump the library revision.
    for (modern, rate) in [(false, 20), (true, 21)] {
        let mut request = Request::patch("/api/v1/settings")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json");
        if modern {
            request = request.header(
                CLIENT_COMPATIBILITY_HEADER,
                serde_json::to_string(&compatibility_manifest()).unwrap(),
            );
        }
        let response = router(state.clone())
            .oneshot(
                request
                    .body(Body::from(
                        serde_json::json!({"list_waveform_rate": rate}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "modern={modern}");
        let body: serde_json::Value =
            serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        assert_eq!(body["list_waveform_rate"], rate);
        let persisted: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&settings_path).unwrap()).unwrap();
        assert_eq!(persisted["list_waveform_rate"], rate);
        assert_eq!(state.revision().unwrap().revision, revision);
        assert_eq!(
            conn.pragma_query_value(None, "data_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            data_version
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn old_client_is_refused_before_preparation_and_modern_client_syncs_actual_format() {
    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("library.db");
    let provider = SqliteProvider::open(&database).unwrap();
    let conn = rusqlite::Connection::open(&database).unwrap();
    audiolib_db::register_library_functions(&conn).unwrap();
    conn.execute(
        "INSERT INTO audio_files(id,file_path,filename,user_tags) VALUES(1,'owned.wav','metal.wav','privateannotation')",
        [],
    ).unwrap();
    drop(conn);
    drop(provider);
    let config = ServerConfig::from_toml_str(
        "bind='127.0.0.1:0'\nlibrary_db='library.db'\nsnapshot_dir='snapshots'\nauth=true\n",
        root.path(),
    )
    .unwrap();
    let token = "compatibility-test-token-0123456789";
    let state = AppState::open(&config, BearerToken::new(token).unwrap()).unwrap();
    let response = router(state.clone())
        .oneshot(
            Request::get("/api/v1/library/snapshot")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["error_code"], "SNAPSHOT_FORMAT_UNSUPPORTED");
    assert!(body["message"].as_str().unwrap().contains(PLAN_B_FORMAT));
    assert_eq!(
        std::fs::read_dir(root.path().join("snapshots")).map_or(0, |entries| entries.count()),
        0
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(serve(
        listener,
        router(state.clone()),
        async {
            let _ = stopped.await;
        },
        ServeOptions {
            shutdown_grace: Duration::from_secs(2),
            ..Default::default()
        },
    ));
    let data = root.path().join("client");
    tokio::task::spawn_blocking(move || {
        let client = RemoteClient::new(&url, token).unwrap();
        let remote = client.library().unwrap();
        assert_eq!(remote.snapshot_format.as_ref().unwrap().id, PLAN_B_FORMAT);
        let capabilities = client.capabilities().unwrap();
        assert_eq!(
            capabilities.compatibility.unwrap().application_version,
            env!("CARGO_PKG_VERSION")
        );
        let observer = rusqlite::Connection::open_with_flags(
            &database,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let before: i64 = observer
            .pragma_query_value(None, "data_version", |row| row.get(0))
            .unwrap();
        let cache = RemoteCache::new(&data);
        let outcome =
            sync_library(&client, &cache, &mut |_| {}, &AtomicBool::new(false), false).unwrap();
        assert!(outcome.downloaded);
        assert_eq!(outcome.state.schema_version, 6);
        let format = outcome.state.format.as_ref().unwrap();
        assert_eq!(format.source.as_ref(), Some(&format.local));
        assert_eq!(format.generation, 0);
        let cached = SqliteProvider::open_read_only(&cache.database_path(&outcome.state)).unwrap();
        assert_eq!(
            cached
                .search(&SearchRequest {
                    query: "privateannotation".into(),
                    ..Default::default()
                })
                .unwrap()
                .total_count,
            1
        );
        assert_eq!(cached.get_audio(1).unwrap().play_count, 0);
        let playback = audiolib_remote::RemoteProvider::new(
            std::sync::Arc::new(cached),
            RemoteWriter::new(client.clone(), cache.clone(), &remote.library_id),
            false,
            None,
        );
        playback.record_play(1).unwrap();
        assert!(playback.play_history(100, 0).unwrap().is_empty());
        assert_eq!(client.library().unwrap().revision, remote.revision);
        assert_eq!(
            observer
                .pragma_query_value(None, "data_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            before
        );
        drop(playback);
        let second =
            sync_library(&client, &cache, &mut |_| {}, &AtomicBool::new(false), false).unwrap();
        assert!(!second.downloaded);
        assert_eq!(outcome.state.revision, second.state.revision);
        // Equal server revisions do not authorize reuse of an unproven source format.
        let mut obsolete = second.state.clone();
        obsolete
            .format
            .as_mut()
            .unwrap()
            .source
            .as_mut()
            .unwrap()
            .features
            .insert("tokenization".into(), 2);
        cache.save_state(&obsolete).unwrap();
        let refreshed =
            sync_library(&client, &cache, &mut |_| {}, &AtomicBool::new(false), false).unwrap();
        assert!(refreshed.downloaded);
        assert_eq!(refreshed.state.revision, second.state.revision);
        assert_eq!(refreshed.state.format, second.state.format);
        let writer = RemoteWriter::new(client.clone(), cache.clone(), &remote.library_id);
        let changed = writer
            .update_metadata(
                1,
                &MetadataPatch {
                    notes: Some("retainedmigrationannotation".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(
            changed.cache,
            CacheUpdate::Applied {
                revision: changed.revision
            }
        );
        assert_eq!(changed.revision, refreshed.state.revision + 1);
        let patched = cache.active().unwrap();
        assert_eq!(patched.format, refreshed.state.format);
        let reader = SqliteProvider::open_read_only(&cache.database_path(&patched)).unwrap();
        assert_eq!(
            reader
                .search(&SearchRequest {
                    query: "retainedmigrationannotation".into(),
                    ..Default::default()
                })
                .unwrap()
                .total_count,
            1
        );
        let third =
            sync_library(&client, &cache, &mut |_| {}, &AtomicBool::new(false), false).unwrap();
        assert!(!third.downloaded);
        assert_eq!(third.state.revision, changed.revision);
    })
    .await
    .unwrap();
    let _ = stop.send(());
    task.await.unwrap();
}
