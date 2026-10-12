// crates/audiolib-db/tests/search_benchmark.rs
//! Existing benchmark fixtures report every row without changing library visibility.
use audiolib_core::DataProvider;
use audiolib_db::SqliteProvider;
use rusqlite::Connection;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn fixture() -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("existing.db");
    let provider = SqliteProvider::open(&database).unwrap();
    let conn = Connection::open(&database).unwrap();
    conn.execute_batch(
        "INSERT INTO folders(id,folder_path,folder_name,depth)
         VALUES(1,'synthetic','synthetic',0);
         INSERT INTO folder_closure VALUES(1,1,0);
         INSERT INTO audio_files(id,file_path,filename,folder_id,missing,user_notes) VALUES
         (1,'synthetic/metal-present.wav','metal-present.wav',1,0,'retain present'),
         (2,'synthetic/metal-missing.wav','metal-missing.wav',1,1,'retain missing'),
         (3,'synthetic/ambient-present.wav','ambient-present.wav',1,0,'retain ambient');",
    )
    .unwrap();
    let stats = provider.stats().unwrap();
    assert_eq!(stats.audio_files, 2);
    assert_eq!(stats.missing_files, 1);
    drop(conn);
    drop(provider);
    // Capture a self-contained library so its complete bytes can be compared.
    let conn = Connection::open(&database).unwrap();
    let mode: String = conn
        .query_row("PRAGMA journal_mode=DELETE", [], |row| row.get(0))
        .unwrap();
    assert_eq!(mode, "delete");
    drop(conn);
    (directory, database)
}

fn benchmark(database: &Path, extra_arguments: &[&str]) -> serde_json::Value {
    let output = Command::new(env!("CARGO_BIN_EXE_search-benchmark"))
        .arg("--existing-db")
        .arg(database)
        .args(["--queries", "19", "--edge-cases"])
        .args(extra_arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn existing_fixture_report_counts_missing_rows_without_rewriting_library() {
    let (_directory, database) = fixture();
    let original = fs::read(&database).unwrap();
    let report = benchmark(&database, &[]);
    assert_eq!(report["existing_database"], true);
    assert_eq!(report["records"], 3);
    assert_eq!(report["missing_records"], 1);
    assert_eq!(report["generation_ms"], 0.0);
    assert_eq!(report["first_provider"], false);
    assert!(
        report["first_provider_cases"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(report["first_provider_total_p95_ms"].is_null());
    assert_eq!(fs::read(&database).unwrap(), original);

    let readonly = SqliteProvider::open_read_only(&database).unwrap();
    assert_eq!(readonly.stats().unwrap().missing_files, 1);
    assert!(readonly.get_audio(2).unwrap().missing);
    assert_eq!(readonly.get_audio(2).unwrap().notes, "retain missing");
}

#[test]
fn first_provider_measures_each_search_then_facets_without_query_prewarming() {
    let (_directory, database) = fixture();
    let original = fs::read(&database).unwrap();
    let report = benchmark(&database, &["--first-provider", "--folder-facets"]);
    assert_eq!(report["first_provider"], true);
    assert_eq!(report["folder_facets"], true);
    assert_eq!(report["first_provider_order"], "search_then_facets");
    assert_eq!(report["first_provider_samples_per_case"], 1);
    let first = report["first_provider_cases"].as_array().unwrap();
    let shared = report["cases"].as_array().unwrap();
    assert_eq!(first.len(), 19);
    assert_eq!(first.len(), shared.len());
    for (first_case, shared_case) in first.iter().zip(shared) {
        assert_eq!(first_case["request"], shared_case["request"]);
        // Repeated metal queries across edge cases must still start uncached.
        assert_eq!(first_case["search_ranking_cache_hit"], false);
        for metric in [
            "provider_open_ms",
            "search_ms",
            "facet_ms",
            "total_ms",
            "search_execution_ms",
        ] {
            let value = first_case[metric].as_f64().unwrap();
            assert!(value.is_finite() && value >= 0.0, "{metric}: {value}");
        }
        let total = first_case["total_ms"].as_f64().unwrap();
        let search = first_case["search_ms"].as_f64().unwrap();
        let facets = first_case["facet_ms"].as_f64().unwrap();
        assert!(total + 0.000001 >= search + facets);
    }
    for metric in [
        "first_provider_open_p95_ms",
        "first_provider_search_p95_ms",
        "first_provider_facet_p95_ms",
        "first_provider_total_p95_ms",
    ] {
        assert!(report[metric].as_f64().unwrap().is_finite());
    }
    assert_eq!(report["records"], 3);
    assert_eq!(report["missing_records"], 1);
    assert_eq!(fs::read(&database).unwrap(), original);
}

#[test]
fn small_synthetic_fixtures_measure_every_case_and_index_extracted_keywords() {
    let directory = tempfile::tempdir().unwrap();
    for records in [1, 31, 999] {
        let database = directory.path().join(format!("synthetic-{records}.db"));
        let output = Command::new(env!("CARGO_BIN_EXE_search-benchmark"))
            .args(["--records", &records.to_string(), "--queries", "14", "--db"])
            .arg(&database)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let cases = report["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 14);
        for case in cases {
            assert_eq!(case["samples_ms"].as_array().unwrap().len(), 1);
            assert!(case["p95_ms"].as_f64().unwrap().is_finite());
        }
        let provider = SqliteProvider::open_read_only(&database).unwrap();
        assert_eq!(provider.stats().unwrap().audio_files, records);
        // SFX exists only in extracted source Metadata, not filename/user annotations.
        let page = provider
            .search(&audiolib_core::SearchRequest {
                query: "sfx".into(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(page.total_count, records.div_ceil(4));
    }
}

#[test]
fn insufficient_queries_fail_before_creating_a_synthetic_database() {
    let directory = tempfile::tempdir().unwrap();
    for (queries, edge_cases, minimum) in [(13, false, 14), (18, true, 19)] {
        let database = directory.path().join(format!("insufficient-{queries}.db"));
        let mut command = Command::new(env!("CARGO_BIN_EXE_search-benchmark"));
        command
            .args(["--records", "1", "--queries", &queries.to_string(), "--db"])
            .arg(&database);
        if edge_cases {
            command.arg("--edge-cases");
        }
        let output = command.output().unwrap();
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .contains(&format!("queries must be at least {minimum}"))
        );
        assert!(!database.exists());
    }
}
