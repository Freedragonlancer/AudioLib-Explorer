// crates/audiolib-db/tests/search_unicode.rs
//! Scanner/provider regressions for Unicode word boundaries and derived-index upgrades.
use audiolib_core::{DataProvider, MaintenanceAction, MetadataPatch, SearchRequest};
use audiolib_db::{ScanCancellation, ScanOptions, ScanService, SqliteProvider};
use rusqlite::Connection;
use std::{collections::BTreeSet, fs, path::Path, sync::Arc};

fn chunk(bytes: &mut Vec<u8>, name: &[u8; 4], value: &[u8]) {
    bytes.extend(name);
    bytes.extend((value.len() as u32).to_le_bytes());
    bytes.extend(value);
    if !value.len().is_multiple_of(2) {
        bytes.push(0);
    }
}

fn wav(path: &Path, description: Option<&str>) {
    let mut bytes = b"RIFF\0\0\0\0WAVE".to_vec();
    let mut fmt = Vec::new();
    fmt.extend(1_u16.to_le_bytes());
    fmt.extend(1_u16.to_le_bytes());
    fmt.extend(48000_u32.to_le_bytes());
    fmt.extend(96000_u32.to_le_bytes());
    fmt.extend(2_u16.to_le_bytes());
    fmt.extend(16_u16.to_le_bytes());
    chunk(&mut bytes, b"fmt ", &fmt);
    chunk(&mut bytes, b"data", &[0; 128]);
    if let Some(description) = description {
        let mut info = b"INFO".to_vec();
        chunk(&mut info, b"ICMT", description.as_bytes());
        chunk(&mut bytes, b"LIST", &info);
    }
    let size = bytes.len() as u32 - 8;
    bytes[4..8].copy_from_slice(&size.to_le_bytes());
    fs::write(path, bytes).unwrap();
}

fn scan(provider: &Arc<SqliteProvider>, root: &Path) {
    let progress = ScanService::new(provider.clone())
        .scan(
            root,
            &ScanOptions::default(),
            &ScanCancellation::default(),
            |_| {},
        )
        .unwrap();
    assert_eq!(progress.errors, 0);
}

fn names(provider: &SqliteProvider, query: &str) -> BTreeSet<String> {
    provider
        .search(&SearchRequest {
            query: query.into(),
            ..Default::default()
        })
        .unwrap()
        .results
        .into_iter()
        .map(|file| file.filename)
        .collect()
}

#[test]
fn scanner_search_keeps_unicode_words_and_does_not_match_fragments() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("audio");
    fs::create_dir(&root).unwrap();
    let expected = BTreeSet::from([
        "cafe.wav".to_string(),
        "café.wav".into(),
        "Cafe\u{301}.wav".into(),
    ]);
    for name in expected
        .iter()
        .map(String::as_str)
        .chain(["irrelevant.wav"])
    {
        wav(&root.join(name), None);
    }
    let provider = Arc::new(SqliteProvider::open(&temp.path().join("library.db")).unwrap());
    scan(&provider, &root);
    let irrelevant = provider
        .search(&SearchRequest::default())
        .unwrap()
        .results
        .into_iter()
        .find(|file| file.filename == "irrelevant.wav")
        .unwrap();
    provider
        .update_metadata(
            irrelevant.id,
            &MetadataPatch {
                tags: Some("caf e".into()),
                ..Default::default()
            },
        )
        .unwrap();
    for query in ["cafe", "café", "Cafe\u{301}", "\"café\"", "café*"] {
        assert_eq!(names(&provider, query), expected, "{query}");
    }
    // Rebuild and read-only access must retain the same Unicode semantics.
    provider
        .maintain_library(MaintenanceAction::RebuildSearch)
        .unwrap();
    for query in ["cafe", "café", "Cafe\u{301}"] {
        assert_eq!(names(&provider, query), expected);
    }
    let readonly = SqliteProvider::open_read_only(provider.database_path()).unwrap();
    assert_eq!(names(&readonly, "café"), expected);
}

#[test]
fn embedded_source_words_and_chinese_search_survive_rebuild() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("audio");
    fs::create_dir(&root).unwrap();
    wav(
        &root.join("embedded.wav"),
        Some("café 爆炸声音 数字音频工作站"),
    );
    let provider = Arc::new(SqliteProvider::open(&temp.path().join("library.db")).unwrap());
    scan(&provider, &root);
    for rebuild in [false, true] {
        if rebuild {
            provider
                .maintain_library(MaintenanceAction::RebuildSearch)
                .unwrap();
        }
        for query in [
            "cafe",
            "café",
            "Cafe\u{301}",
            "爆炸",
            "声音",
            "数字音频工作站",
            "音频",
            "café,爆炸",
            "(café) \"爆炸\"*",
        ] {
            assert_eq!(
                names(&provider, query),
                BTreeSet::from(["embedded.wav".into()]),
                "{query}"
            );
        }
        assert!(
            names(&provider, "café OR 爆炸").is_empty(),
            "OR remains a literal word"
        );
        assert!(names(&provider, "caf e").is_empty());
    }
}

#[test]
fn internal_punctuation_search_is_and_across_nonadjacent_tokens_and_metadata_fields() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("audio");
    fs::create_dir(&root).unwrap();
    for name in [
        "separated.wav",
        "adjacent.wav",
        "cross-field.wav",
        "accent.wav",
    ] {
        wav(&root.join(name), None);
    }
    let provider = Arc::new(SqliteProvider::open(&temp.path().join("library.db")).unwrap());
    scan(&provider, &root);
    for file in provider.search(&SearchRequest::default()).unwrap().results {
        let (tags, notes) = match file.filename.as_str() {
            "separated.wav" => ("foo middle bar don middle t 1 middle 2", ""),
            "adjacent.wav" => ("foo bar don t 1 2", ""),
            "cross-field.wav" => ("foo don 1", "bar t 2"),
            "accent.wav" => ("café middle door", ""),
            other => panic!("Unexpected fixture {other}"),
        };
        provider
            .update_metadata(
                file.id,
                &MetadataPatch {
                    tags: Some(tags.into()),
                    notes: Some(notes.into()),
                    ..Default::default()
                },
            )
            .unwrap();
    }
    let all = BTreeSet::from([
        "separated.wav".into(),
        "adjacent.wav".into(),
        "cross-field.wav".into(),
    ]);
    for query in [
        "foo:bar",
        "foo·bar",
        "foo：bar",
        "foo‿bar",
        "don't",
        "don’t",
        "1,2",
    ] {
        assert_eq!(names(&provider, query), all, "{query}");
    }
    for query in ["café:door", "Cafe\u{301}:door"] {
        assert_eq!(
            names(&provider, query),
            BTreeSet::from(["accent.wav".into()]),
            "{query}"
        );
    }
    // Dot compounds and decimal searches were already phrases in the old compiler.
    for query in ["foo.bar", "foo_bar", "1.2"] {
        assert_eq!(
            names(&provider, query),
            BTreeSet::from(["adjacent.wav".into()]),
            "{query}"
        );
    }
    let readonly = SqliteProvider::open_read_only(provider.database_path()).unwrap();
    assert_eq!(names(&readonly, "foo:bar"), all);
}

#[test]
fn existing_v3_rebuilds_old_derived_words_once_without_changing_annotations() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("audio");
    fs::create_dir(&root).unwrap();
    let audio = root.join("embedded.wav");
    wav(&audio, Some("café 爆炸声音"));
    let original = fs::read(&audio).unwrap();
    let database = temp.path().join("library.db");
    let provider = Arc::new(SqliteProvider::open(&database).unwrap());
    scan(&provider, &root);
    let id = provider.search(&SearchRequest::default()).unwrap().results[0].id;
    provider
        .update_metadata(
            id,
            &MetadataPatch {
                tags: Some("Retain 私人".into()),
                notes: Some("Keep notes".into()),
                favorite: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
    let before = provider.get_audio(id).unwrap();
    drop(provider);
    let conn = Connection::open(&database).unwrap();
    // Simulate the previous v3 tokenizer's derived postings and absent feature marker.
    conn.execute_batch(
        "UPDATE audio_files SET bwf_description_tokenized='caf é 爆炸 声音';
        DELETE FROM audiolib_schema_features WHERE feature='tokenization';",
    )
    .unwrap();
    drop(conn);
    let old_source_bytes = fs::read(&database).unwrap();
    let readonly = SqliteProvider::open_read_only(&database).unwrap();
    assert_ne!(readonly.database_path(), database.as_path());
    let owned_directory = readonly.database_path().parent().unwrap().to_path_buf();
    for query in ["cafe", "café", "Cafe\u{301}", "爆炸"] {
        assert_eq!(
            names(&readonly, query),
            BTreeSet::from(["embedded.wav".into()]),
            "readonly: {query}"
        );
    }
    assert_eq!(
        serde_json::to_value(readonly.get_audio(id).unwrap()).unwrap(),
        serde_json::to_value(&before).unwrap()
    );
    assert_eq!(fs::read(&database).unwrap(), old_source_bytes);
    assert_eq!(fs::read(&audio).unwrap(), original);
    assert!(owned_directory.is_dir());
    drop(readonly);
    assert!(
        !owned_directory.exists(),
        "Reader connections must close before owned snapshot cleanup"
    );
    assert_eq!(fs::read(&database).unwrap(), old_source_bytes);
    let provider = SqliteProvider::open_existing(&database).unwrap();
    for query in ["cafe", "café", "Cafe\u{301}", "爆炸"] {
        assert_eq!(
            names(&provider, query),
            BTreeSet::from(["embedded.wav".into()]),
            "{query}"
        );
    }
    assert_eq!(
        serde_json::to_value(provider.get_audio(id).unwrap()).unwrap(),
        serde_json::to_value(before).unwrap()
    );
    assert_eq!(fs::read(&audio).unwrap(), original);
    drop(provider);
    let conn = Connection::open(&database).unwrap();
    let rootpage: i64 = conn
        .query_row(
            "SELECT rootpage FROM sqlite_schema WHERE name='audio_search_data'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        5
    );
    assert_eq!(
        conn.query_row(
            "SELECT version FROM audiolib_schema_features WHERE feature='tokenization'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    drop(conn);
    drop(SqliteProvider::open_existing(&database).unwrap());
    let conn = Connection::open(&database).unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT rootpage FROM sqlite_schema WHERE name='audio_search_data'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        rootpage
    );
}
