// crates/audiolib-db/tests/keyword_list_search.rs
//! Regression: every item of a `;`-separated embedded keyword list is full-text searchable.
use audiolib_core::{DataProvider, MaintenanceAction, SearchRequest};
use audiolib_db::{ScanCancellation, ScanOptions, ScanService, SqliteProvider};
use rusqlite::Connection;
use std::{fs, path::Path, sync::Arc};

fn append(bytes: &mut Vec<u8>, name: &[u8; 4], value: &[u8]) {
    bytes.extend(name);
    bytes.extend((value.len() as u32).to_le_bytes());
    bytes.extend(value);
    if !value.len().is_multiple_of(2) {
        bytes.push(0);
    }
}

fn wav(path: &Path, keywords: &str) {
    let mut bytes = b"RIFF\0\0\0\0WAVE".to_vec();
    let mut fmt = Vec::new();
    fmt.extend(1_u16.to_le_bytes());
    fmt.extend(1_u16.to_le_bytes());
    fmt.extend(48000_u32.to_le_bytes());
    fmt.extend(96000_u32.to_le_bytes());
    fmt.extend(2_u16.to_le_bytes());
    fmt.extend(16_u16.to_le_bytes());
    append(&mut bytes, b"fmt ", &fmt);
    append(&mut bytes, b"data", &[0; 128]);
    let mut info = b"INFO".to_vec();
    append(&mut info, b"IKEY", format!("{keywords}\0").as_bytes());
    append(&mut bytes, b"LIST", &info);
    let size = bytes.len() as u32 - 8;
    bytes[4..8].copy_from_slice(&size.to_le_bytes());
    fs::write(path, bytes).unwrap();
}

fn search(provider: &SqliteProvider, query: &str) -> u64 {
    provider
        .search(&SearchRequest {
            query: query.into(),
            ..Default::default()
        })
        .unwrap()
        .total_count
}

fn fts(path: &Path, term: &str) -> i64 {
    Connection::open(path)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM audio_search WHERE audio_search MATCH ?1",
            [format!("\"{term}\"")],
            |row| row.get(0),
        )
        .unwrap()
}

#[test]
fn semicolon_keyword_list_items_are_all_searchable() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("audio");
    fs::create_dir(&root).unwrap();
    wav(&root.join("a.wav"), "Zebragroan; thunderclap");
    wav(&root.join("b.wav"), "Zebragroan; thunderclap");
    let db = temp.path().join("library.db");
    let provider = Arc::new(SqliteProvider::open(&db).unwrap());
    ScanService::new(provider.clone())
        .scan(
            &root,
            &ScanOptions::default(),
            &ScanCancellation::default(),
            |_| {},
        )
        .unwrap();
    let check = |stage: &str| {
        for term in ["zebragroan", "thunderclap", "Thunderclap"] {
            assert_eq!(fts(&db, &term.to_lowercase()), 2, "{stage}: fts {term}");
            assert_eq!(search(&provider, term), 2, "{stage}: search {term}");
        }
        assert_eq!(
            search(&provider, "zebragroan thunderclap"),
            2,
            "{stage}: both"
        );
    };
    check("after scan");
    provider
        .maintain_library(MaintenanceAction::RebuildSearch)
        .unwrap();
    check("after rebuild");
}

#[test]
fn sql_seeded_metadata_json_keywords_are_searchable_after_rebuild() {
    let temp = tempfile::tempdir().unwrap();
    let db = temp.path().join("library.db");
    let provider = SqliteProvider::open(&db).unwrap();
    let json = r#"{"wav_metadata":{"fields":{"keywords":"Zebragroan; thunderclap"}}}"#;
    {
        let conn = Connection::open(&db).unwrap();
        for id in [364, 2104] {
            conn.execute(
                "INSERT INTO audio_files(id,file_path,filename,metadata_json,missing) VALUES(?1,?2,?2,?3,0)",
                rusqlite::params![id, format!("{id}.wav"), json],
            )
            .unwrap();
        }
    }
    // Seeded rows carry no derived search text until the maintenance rebuild fills it.
    provider
        .maintain_library(MaintenanceAction::RebuildSearch)
        .unwrap();
    for term in ["zebragroan", "thunderclap"] {
        assert_eq!(search(&provider, term), 2, "{term}");
        assert_eq!(fts(&db, term), 2, "fts {term}");
    }
}
