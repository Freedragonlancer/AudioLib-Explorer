// server/audiolib-server/tests/startup_upgrade.rs
//! Server startup exposes synchronous library-upgrade progress in ordinary info logs.
use audiolib_db::SqliteProvider;
use audiolib_server::{AppState, ServerConfig};
use rusqlite::Connection;
use std::{
    io::{self, Write},
    sync::{Arc, Mutex},
};

struct LogWriter(Arc<Mutex<Vec<u8>>>);

impl Write for LogWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn server_startup_logs_upgrade_phases_rows_and_elapsed_time() {
    let dir = tempfile::tempdir().unwrap();
    let database = dir.path().join("library.db");
    drop(SqliteProvider::open(&database).unwrap());
    let conn = Connection::open(&database).unwrap();
    conn.execute_batch(
        "WITH RECURSIVE records(id) AS (SELECT 1 UNION ALL SELECT id+1 FROM records WHERE id<5001)
         INSERT INTO audio_files(id,file_path,filename,user_notes)
         SELECT id,'root/clip-'||id||'.wav','clip-'||id||'.wav','retained notes' FROM records;
         UPDATE audiolib_schema_features SET version=1 WHERE feature='tokenization';",
    )
    .unwrap();
    drop(conn);
    let config = ServerConfig::from_toml_str("library_db = 'library.db'", dir.path()).unwrap();
    let logs = Arc::new(Mutex::new(Vec::new()));
    let output = logs.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_max_level(tracing::Level::INFO)
        .with_writer(move || LogWriter(output.clone()))
        .finish();
    let state =
        tracing::subscriber::with_default(subscriber, || AppState::open(&config, None)).unwrap();
    let text = String::from_utf8(logs.lock().unwrap().clone()).unwrap();
    let expected = [
        "phase=\"preparing\"",
        "phase=\"tokenizing\" completed=0 total=5001",
        "phase=\"tokenizing\" completed=5000 total=5001",
        "phase=\"tokenizing\" completed=5001 total=5001",
        "phase=\"indexing\"",
        "phase=\"source_tags\"",
        "Library startup: database opened",
    ];
    let mut after = 0;
    for event in expected {
        let index = text[after..]
            .find(event)
            .unwrap_or_else(|| panic!("Missing {event:?} in {text}"));
        after += index + event.len();
    }
    assert!(text.contains("elapsed_seconds="), "{text}");
    assert!(text.contains("committed rows"), "{text}");
    assert_eq!(state.audio(1).unwrap().notes, "retained notes");
    assert!(SqliteProvider::has_current_search_state(&database).unwrap());
}
