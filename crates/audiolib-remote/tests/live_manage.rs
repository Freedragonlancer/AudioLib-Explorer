// crates/audiolib-remote/tests/live_manage.rs
//! Opt-in, DESTRUCTIVE end-to-end test of the server management client (API §6.6–§6.9)
//! against a disposable test server. Off by default: it is `#[ignore]`d and also needs
//! `AUDIOLIB_LIVE_MANAGE_URL`; it refuses the production port 8790.
//!
//! ```text
//! $env:AUDIOLIB_LIVE_MANAGE_URL = "http://server.example.lan:8799"
//! cargo test -p audiolib-remote --test live_manage -- --ignored --nocapture
//! ```
//!
//! It starts, pauses, resumes and cancels scans, removes a few audio records and one
//! leaf folder (database records only, files untouched), then runs a full scan that adds
//! them back with new ids. Ids are discovered from `/roots` and a synced snapshot in a
//! temporary data directory, never hard-coded. `AUDIOLIB_LIVE_MANAGE_TOKEN` is sent as
//! the bearer token when set (never printed).
use audiolib_remote::{
    CacheState, CacheUpdate, RemoteCache, RemoteClient, RemoteError, RemoteWriter, ScanControl,
    ScanMode, ScanRequest, ServerScan, sync_library,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

const MISSING_ID: i64 = 999_999_999;

fn wait_inactive(client: &RemoteClient, id: i64) -> ServerScan {
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        let scan = client.scan_status(id).expect("GET /scans/{id}");
        if !scan.is_active() {
            return scan;
        }
        assert!(
            Instant::now() < deadline,
            "scan {id} still active after 180 s"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn wait_idle(client: &RemoteClient) {
    while let Some(scan) = client.current_scan().expect("GET /scans/current") {
        eprintln!("waiting for running scan {}", scan.progress.session_id);
        wait_inactive(client, scan.progress.session_id);
    }
}

fn sync(client: &RemoteClient, cache: &RemoteCache) -> CacheState {
    let cancel = AtomicBool::new(false);
    sync_library(client, cache, &mut |_| {}, &cancel, false)
        .expect("snapshot sync")
        .state
}

fn snapshot(cache: &RemoteCache, state: &CacheState) -> Connection {
    Connection::open_with_flags(cache.database_path(state), OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("open snapshot")
}

fn audio_path(db: &Connection, id: i64) -> Option<String> {
    db.query_row(
        "SELECT file_path FROM audio_files WHERE id = ?1",
        [id],
        |row| row.get(0),
    )
    .optional()
    .unwrap()
}

fn audio_id(db: &Connection, path: &str) -> Option<i64> {
    db.query_row(
        "SELECT id FROM audio_files WHERE file_path = ?1",
        [path],
        |row| row.get(0),
    )
    .optional()
    .unwrap()
}

/// Start a one-worker full scan and pause it right away; `None` when the scan finished
/// before the pause landed.
fn paused_scan(client: &RemoteClient, root: &str) -> Option<i64> {
    let started = client
        .start_scan(&ScanRequest {
            root: root.into(),
            subpath: None,
            mode: ScanMode::Full,
            workers: Some(1),
        })
        .expect("POST /scans");
    let mut id = started.progress.session_id;
    if id == 0 {
        id = client
            .current_scan()
            .unwrap()
            .map(|scan| scan.progress.session_id)
            .unwrap_or(0);
    }
    assert!(id > 0, "the started scan has a session id");
    assert_eq!(started.root.as_deref(), Some(root));
    assert_eq!(started.mode.as_deref(), Some("full"));
    match client.control_scan(id, ScanControl::Pause) {
        Ok(reply) => assert!(reply.pause_requested || reply.progress.paused || !reply.active),
        Err(RemoteError::ScanState(_)) => {
            wait_inactive(client, id);
            return None;
        }
        Err(error) => panic!("pause: {error:?}"),
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let scan = client.scan_status(id).unwrap();
        if !scan.is_active() {
            return None;
        }
        if scan.progress.paused {
            return Some(id);
        }
        assert!(Instant::now() < deadline, "pause not confirmed within 30 s");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
#[ignore = "destructive: needs AUDIOLIB_LIVE_MANAGE_URL pointing at a disposable test server"]
fn live_management_round_trip() {
    let Ok(url) = std::env::var("AUDIOLIB_LIVE_MANAGE_URL") else {
        eprintln!("AUDIOLIB_LIVE_MANAGE_URL not set; skipping");
        return;
    };
    assert!(
        !url.contains(":8790"),
        "refusing to run destructive tests against the production port 8790"
    );
    let token = std::env::var("AUDIOLIB_LIVE_MANAGE_TOKEN").unwrap_or_default();
    let client = RemoteClient::new(&url, &token).expect("client");
    let directory = tempfile::tempdir().unwrap();
    let cache = RemoteCache::new(directory.path());

    // §6.9 capabilities.
    let capabilities = client.capabilities().expect("GET /capabilities");
    eprintln!(
        "capabilities: mode {:?}, external {:?}, can_manage {:?}",
        capabilities.manage_mode, capabilities.external, capabilities.can_manage
    );
    assert!(capabilities.supported && capabilities.may_manage());

    // §6.4 roots with the new fields.
    wait_idle(&client);
    let roots = client.roots().expect("GET /roots");
    let root = roots.first().expect("at least one root").clone();
    eprintln!(
        "root {} ({} files, missing {:?}, scanning {:?})",
        root.name,
        root.file_count.unwrap_or(0),
        root.missing_count,
        root.scanning
    );
    assert!(root.missing_count.is_some());
    assert_eq!(root.scanning, Some(false));

    // §6.6 pause / 409 BUSY while paused / resume / cancel.
    let mut busy_checked = false;
    for attempt in 1..=3 {
        let Some(id) = paused_scan(&client, &root.name) else {
            eprintln!("attempt {attempt}: the scan finished before the pause landed");
            continue;
        };
        eprintln!("scan {id} paused");
        let current = client
            .current_scan()
            .unwrap()
            .expect("paused scan is current");
        assert_eq!(current.progress.session_id, id);
        assert!(current.progress.paused && current.active);
        assert_eq!(
            client.roots().unwrap()[0].scanning,
            Some(true),
            "paused counts as scanning"
        );
        // Removals are refused while paused, even for ids that do not exist.
        let busy = client.remove_audio(&[MISSING_ID]).unwrap_err();
        assert!(matches!(busy, RemoteError::Busy { .. }), "{busy:?}");
        let busy = client.remove_folder(MISSING_ID).unwrap_err();
        assert!(matches!(busy, RemoteError::Busy { .. }), "{busy:?}");
        // So is a second scan.
        let second = client
            .start_scan(&ScanRequest {
                root: root.name.clone(),
                ..ScanRequest::default()
            })
            .unwrap_err();
        assert!(matches!(second, RemoteError::Busy { .. }), "{second:?}");
        // Idempotent repeat, then resume and cancel.
        client
            .control_scan(id, ScanControl::Pause)
            .expect("repeated pause");
        let resumed = client
            .control_scan(id, ScanControl::Resume)
            .expect("resume");
        assert!(resumed.active);
        match client.control_scan(id, ScanControl::Cancel) {
            Ok(_) | Err(RemoteError::ScanState(_)) => {}
            Err(error) => panic!("cancel: {error:?}"),
        }
        let finished = wait_inactive(&client, id);
        eprintln!("scan {id} ended as {:?}", finished.progress.state);
        // A control the finished session no longer allows: 409 SCAN_STATE.
        let state = client.control_scan(id, ScanControl::Pause).unwrap_err();
        assert!(matches!(state, RemoteError::ScanState(_)), "{state:?}");
        // §6.7 history lists the session, newest first.
        let history = client.scan_history(5, 0).expect("GET /history/scans");
        assert!(history.iter().any(|session| session.id == id));
        assert!(history.windows(2).all(|pair| pair[0].id > pair[1].id));
        busy_checked = true;
        break;
    }
    assert!(busy_checked, "could not pause a scan in 3 attempts");

    // Discover ids from a synced snapshot.
    let state = sync(&client, &cache);
    let db = snapshot(&cache, &state);
    let root_folder: i64 = db
        .query_row(
            "SELECT id FROM folders WHERE folder_path = ?1",
            [&root.server_path],
            |row| row.get(0),
        )
        .expect("the root's top-level folder");
    let (leaf, leaf_path, leaf_files): (i64, String, i64) = db
        .query_row(
            "SELECT f.id, f.folder_path, COUNT(a.id) FROM folders f
             JOIN audio_files a ON a.folder_id = f.id
             WHERE f.id != ?1 AND f.parent_id IS NOT NULL
               AND NOT EXISTS (SELECT 1 FROM folders c WHERE c.parent_id = f.id)
             GROUP BY f.id ORDER BY COUNT(a.id), f.id LIMIT 1",
            [root_folder],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("a leaf folder with files");
    let leaf_sample: String = db
        .query_row(
            "SELECT file_path FROM audio_files WHERE folder_id = ?1 LIMIT 1",
            [leaf],
            |row| row.get(0),
        )
        .unwrap();
    // Low ids, so re-added records cannot get the same id back.
    let picked: Vec<(i64, String)> = db
        .prepare(
            "SELECT id, file_path FROM audio_files WHERE folder_id != ?1 AND missing = 0
             ORDER BY id LIMIT 2 OFFSET 5",
        )
        .unwrap()
        .query_map([leaf], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(picked.len(), 2);
    drop(db);
    let (a, b) = (picked[0].0, picked[1].0);
    eprintln!(
        "revision {}; removing audio {a}, {b} and folder {leaf} ({leaf_files} files)",
        state.revision
    );

    // §6.8 audio removal (+ not_found, duplicates, repeat).
    let writer = RemoteWriter::new(client.clone(), cache.clone(), state.library_id.clone());
    let outcome = writer
        .remove_audio(&[a, b, MISSING_ID, a])
        .expect("remove audio");
    assert_eq!(outcome.value.removed, 2);
    assert_eq!(outcome.value.not_found, vec![MISSING_ID]);
    assert_eq!(outcome.revision_before, state.revision);
    assert_eq!(outcome.revision, state.revision + 1);
    assert!(matches!(outcome.cache, CacheUpdate::Resync { .. }));
    let repeat = client.remove_audio(&[a, b, MISSING_ID]).unwrap();
    assert_eq!(repeat.removed, 0);
    assert_eq!(repeat.not_found, vec![a, b, MISSING_ID]);
    assert!(!repeat.changed(), "nothing removed keeps the revision");
    // The revision bump is the trigger to re-sync; the removed ids are gone.
    let state = sync(&client, &cache);
    assert_eq!(state.revision, outcome.revision);
    let db = snapshot(&cache, &state);
    assert!(audio_path(&db, a).is_none() && audio_path(&db, b).is_none());
    drop(db);

    // §6.8 folder removal (+ repeat 404, unknown 404, root 400).
    let outcome = writer.remove_folder(leaf).expect("remove folder");
    assert_eq!(outcome.value.removed, leaf_files as u64);
    assert!(outcome.value.folders_removed >= 1);
    assert_eq!(outcome.revision, state.revision + 1);
    assert!(matches!(
        client.remove_folder(leaf),
        Err(RemoteError::NotFound(_))
    ));
    assert!(matches!(
        client.remove_folder(MISSING_ID),
        Err(RemoteError::NotFound(_))
    ));
    assert!(matches!(
        client.remove_folder(root_folder),
        Err(RemoteError::Validation(_))
    ));
    let state = sync(&client, &cache);
    assert_eq!(state.revision, outcome.revision);
    let db = snapshot(&cache, &state);
    assert!(audio_id(&db, &leaf_sample).is_none());
    drop(db);
    eprintln!("removed folder {leaf_path}");

    // A full scan adds the files back with NEW ids: cached ids must not be reused.
    let started = client
        .start_scan(&ScanRequest {
            root: root.name.clone(),
            subpath: None,
            mode: ScanMode::Full,
            workers: None,
        })
        .expect("restoring full scan");
    let finished = wait_inactive(&client, started.progress.session_id);
    assert_eq!(finished.progress.state, audiolib_core::ScanState::Completed);
    assert!(
        finished.needs_sync(state.revision),
        "the scan re-added records"
    );
    let state = sync(&client, &cache);
    let db = snapshot(&cache, &state);
    for (old, path) in &picked {
        let new = audio_id(&db, path).expect("re-added by the scan");
        assert_ne!(new, *old, "{path} came back with a new id");
    }
    assert!(audio_id(&db, &leaf_sample).is_some());
    let roots = client.roots().unwrap();
    let last = roots[0].last_scan.as_ref().expect("last_scan");
    assert_eq!(last.id, started.progress.session_id);
    assert_eq!(last.state, "completed");
    eprintln!(
        "restored: revision {}, last scan {} added {}",
        state.revision, last.id, last.added
    );
}
