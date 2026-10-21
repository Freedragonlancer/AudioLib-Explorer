// crates/audiolib-db/tests/search_benchmark.rs
//! Existing benchmark fixtures report every row without changing library visibility.
use audiolib_core::{DataProvider, SearchRequest};
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
    audiolib_db::register_library_functions(&conn).unwrap();
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
    audiolib_db::register_library_functions(&conn).unwrap();
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

#[test]
fn concurrent_writer_reports_baseline_commits_and_fallback_reasons() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("concurrent.db");
    let output = Command::new(env!("CARGO_BIN_EXE_search-benchmark"))
        .args([
            "--records",
            "31",
            "--queries",
            "2",
            "--query",
            "metal",
            "--concurrent-writer",
            "--db",
        ])
        .arg(&database)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["concurrent_writer"], true);
    assert_eq!(report["sqlite_cache_kb_per_auxiliary_reader"], 8192);
    assert!(
        report["parallel_ranking_auxiliary_readers_active"]
            .as_u64()
            .unwrap()
            <= 7
    );
    let scenario = &report["concurrent_writer_report"];
    assert_eq!(scenario["queries"], 2);
    assert!(scenario["writer_commits"].as_u64().unwrap() >= 1);
    for key in ["baseline_samples_ms", "samples_ms"] {
        let values = scenario[key].as_array().unwrap();
        assert_eq!(values.len(), 2);
        assert!(
            values
                .iter()
                .all(|value| value.as_f64().is_some_and(|v| v.is_finite() && v >= 0.0))
        );
    }
    // Tiny fixtures do not meet the fanout threshold. Preserve zero counts rather
    // than misreporting this runner smoke test as evidence of serial fallback.
    for key in ["baseline_parallel_ranking", "parallel_ranking"] {
        assert_eq!(scenario[key]["attempts"], 0);
        let reasons = scenario[key]["fallbacks"].as_object().unwrap();
        assert_eq!(reasons.len(), 8);
        assert!(reasons.values().all(|value| value == 0));
        assert!(reasons.contains_key("reader_busy"));
        assert!(reasons.contains_key("snapshot_changed"));
    }
}

#[test]
fn concurrent_writer_refuses_existing_libraries_before_opening_them() {
    let (_directory, database) = fixture();
    let original = fs::read(&database).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_search-benchmark"))
        .arg("--existing-db")
        .arg(&database)
        .arg("--concurrent-writer")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("requires a new synthetic fixture"));
    assert_eq!(fs::read(&database).unwrap(), original);
}

#[test]
fn first_only_custom_requests_keep_real_scope_results_and_source_bytes() {
    let (directory, database) = fixture();
    let conn = Connection::open(&database).unwrap();
    audiolib_db::register_library_functions(&conn).unwrap();
    conn.execute_batch(
        "INSERT INTO folders(id,folder_path,folder_name,depth) VALUES(9,'other','other',0);
         INSERT INTO folder_closure VALUES(9,9,0);
         INSERT INTO audio_files(id,file_path,filename,folder_id,missing) VALUES
         (4,'other/metal-present.wav','metal-present.wav',9,0),
         (5,'other/legacy-negative.wav','legacy-negative.wav',9,-1),
         (6,'other/legacy-other.wav','legacy-other.wav',9,2);",
    )
    .unwrap();
    drop(conn);
    let requests = vec![
        SearchRequest {
            query: "metal present".into(),
            ..Default::default()
        },
        SearchRequest {
            query: "metal present".into(),
            folder_id: Some(9),
            ..Default::default()
        },
    ];
    let request_file = directory.path().join("requests.json");
    fs::write(&request_file, serde_json::to_vec(&requests).unwrap()).unwrap();
    let original = fs::read(&database).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_search-benchmark"))
        .arg("--existing-db")
        .arg(&database)
        .arg("--request-file")
        .arg(&request_file)
        .args(["--first-only", "--queries", "0"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["first_only"], true);
    assert_eq!(report["pre_query_stats"], false);
    assert_eq!(report["statistics_collected_after_timing"], true);
    assert_eq!(report["shared_provider_measured"], false);
    assert_eq!(report["cases"], serde_json::json!([]));
    assert_eq!(report["queries"], 2);
    assert_eq!(report["records"], 6);
    assert_eq!(report["missing_records"], 1);
    assert_eq!(report["source_same"], true);
    assert_eq!(report["owned_copy"], false);
    let first = report["first_provider_cases"].as_array().unwrap();
    assert_eq!(first.len(), 2);
    assert_eq!(first[0]["total_count"], 2);
    assert_eq!(first[1]["total_count"], 1);
    let oracle = SqliteProvider::open_read_only(&database).unwrap();
    for (sample, request) in first.iter().zip(&requests) {
        assert_eq!(sample["request"], serde_json::to_value(request).unwrap());
        assert_eq!(sample["search_ranking_cache_hit"], false);
        let page = oracle.search(request).unwrap();
        assert_eq!(
            sample["result_ids"],
            serde_json::json!(
                page.results
                    .iter()
                    .map(|audio| audio.id)
                    .collect::<Vec<_>>()
            )
        );
        let mut counts = oracle
            .search_folders(request)
            .unwrap()
            .into_iter()
            .map(|folder| (folder.id, folder.file_count))
            .collect::<Vec<_>>();
        counts.sort_unstable_by_key(|&(id, _)| id);
        assert_eq!(sample["folder_counts"], serde_json::json!(counts));
        assert!(
            sample["total_ms"].as_f64().unwrap() + 0.000001
                >= sample["search_ms"].as_f64().unwrap() + sample["facet_ms"].as_f64().unwrap()
        );
    }
    drop(oracle);
    assert_eq!(fs::read(&database).unwrap(), original);

    let selected = Command::new(env!("CARGO_BIN_EXE_search-benchmark"))
        .arg("--existing-db")
        .arg(&database)
        .arg("--request-file")
        .arg(&request_file)
        .args(["--first-only", "--case-index", "1"])
        .output()
        .unwrap();
    assert!(
        selected.status.success(),
        "{}",
        String::from_utf8_lossy(&selected.stderr)
    );
    let selected: serde_json::Value = serde_json::from_slice(&selected.stdout).unwrap();
    assert_eq!(selected["queries"], 1);
    assert_eq!(
        selected["first_provider_cases"].as_array().unwrap().len(),
        1
    );
    assert_eq!(selected["first_provider_cases"][0]["case_index"], 1);
    assert_eq!(
        selected["first_provider_cases"][0]["request"]["folder_id"],
        9
    );
    assert_eq!(fs::read(&database).unwrap(), original);
}

#[test]
fn first_only_refuses_generation_invalid_requests_and_implicit_upgrades() {
    let (directory, database) = fixture();
    let generated = directory.path().join("must-not-create.db");
    let output = Command::new(env!("CARGO_BIN_EXE_search-benchmark"))
        .args(["--first-only", "--db"])
        .arg(&generated)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!generated.exists());
    let request_file = directory.path().join("requests.json");
    for requests in [
        Vec::new(),
        vec![SearchRequest {
            query: "a".into(),
            ..Default::default()
        }],
    ] {
        fs::write(&request_file, serde_json::to_vec(&requests).unwrap()).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_search-benchmark"))
            .arg("--existing-db")
            .arg(directory.path().join("absent.db"))
            .arg("--request-file")
            .arg(&request_file)
            .arg("--first-only")
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(!directory.path().join("absent.db").exists());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(if requests.is_empty() {
                "at least one SearchRequest"
            } else {
                "at least two characters"
            })
        );
    }
    let conn = Connection::open(&database).unwrap();
    audiolib_db::register_library_functions(&conn).unwrap();
    conn.execute_batch("DROP INDEX idx_audio_search_facets")
        .unwrap();
    drop(conn);
    let original = fs::read(&database).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_search-benchmark"))
        .arg("--existing-db")
        .arg(&database)
        .args(["--first-only", "--case-index", "1"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("requires current derived search state")
    );
    assert_eq!(fs::read(&database).unwrap(), original);
}
