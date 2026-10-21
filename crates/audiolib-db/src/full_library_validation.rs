// crates/audiolib-db/src/full_library_validation.rs
//! Explicit opt-in, read-only full-library migration oracles and provider timings.
//! Run ignored release tests with AUDIOLIB_VALIDATION_SOURCE, _TARGET and _REPORT.
//! The report path must be explicit; source and target libraries are never modified.
use audiolib_core::{DataProvider, SearchFilters, SearchRequest, SortOrder};
use rusqlite::{Connection, OpenFlags, types::ValueRef};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Instant,
};
type R<T> = std::result::Result<T, Box<dyn std::error::Error>>;
fn log(value: Value) {
    println!("{value}");
}
fn save(path: &Path, value: &Value) -> R<()> {
    fs::write(path, serde_json::to_vec_pretty(value)?)?;
    Ok(())
}
fn create_report(path: &Path) -> R<()> {
    // A mistaken report argument must never truncate a library or old evidence.
    // Subsequent checkpoints overwrite only the file claimed by this invocation.
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    Ok(())
}
fn input(name: &str) -> R<PathBuf> {
    let path = PathBuf::from(std::env::var(name)?);
    if !path.is_absolute() {
        return Err(format!("{name} must be absolute").into());
    }
    Ok(path)
}
fn ro(path: &Path) -> R<Connection> {
    let c = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    c.execute_batch("PRAGMA cache_size=-65536; PRAGMA temp_store=MEMORY;")?;
    c.pragma_update(None, "mmap_size", crate::sqlite_reader_mmap_bytes(path))?;
    Ok(c)
}
fn digest_value(h: &mut Sha256, v: ValueRef<'_>) {
    match v {
        ValueRef::Null => h.update([0]),
        ValueRef::Integer(v) => {
            h.update([1]);
            h.update(v.to_le_bytes());
        }
        ValueRef::Real(v) => {
            h.update([2]);
            h.update(v.to_bits().to_le_bytes());
        }
        ValueRef::Text(v) => {
            h.update([3]);
            h.update((v.len() as u64).to_le_bytes());
            h.update(v);
        }
        ValueRef::Blob(v) => {
            h.update([4]);
            h.update((v.len() as u64).to_le_bytes());
            h.update(v);
        }
    }
}
fn digest_sql(c: &Connection, sql: &str) -> R<Value> {
    let mut s = c.prepare(sql)?;
    let cols = s.column_count();
    let mut rows = s.query([])?;
    let mut h = Sha256::new();
    let mut count = 0u64;
    while let Some(row) = rows.next()? {
        h.update([255]);
        for i in 0..cols {
            digest_value(&mut h, row.get_ref(i)?);
        }
        count += 1;
    }
    Ok(json!({"rows":count,"sha256":format!("{:x}",h.finalize())}))
}
fn text_queries() -> Vec<String> {
    [
        "ex",
        "plos",
        "metal",
        "door",
        "impact",
        "wood",
        "water",
        "foot",
        "car",
        "fire",
        "air",
        "gun",
        "voice",
        "rain",
        "wind",
        "ambience",
        "engine",
        "click",
        "glass",
        "stone",
        "human",
        "bird",
        "close",
        "open",
        "roll",
        "loop",
        "swoosh",
        "whoosh",
        "explosion",
        "metal door",
        "door slam",
        "water splash",
        "engine start",
        "footsteps gravel",
        "zzqvnonexistent987654",
        "café",
        "Cafe\u{301}",
        "Straße",
        "STRASSE",
        "naïve",
        "Äpfel",
        "Ελληνικά",
        "爆炸",
        "风",
        "金属",
        "脚步",
        "数字音频工作站",
        "café爆炸声音",
        "100",
        "1.2",
        "foo.bar",
        "foo_bar",
        "foo:bar",
        "don't",
        "1,2",
        "a metal",
        "g000065000078",
        "OR metal",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}
fn cases(path: &Path) -> R<Vec<SearchRequest>> {
    let mut out: Vec<_> = text_queries()
        .into_iter()
        .map(|query| SearchRequest {
            query,
            ..Default::default()
        })
        .collect();
    out.push(SearchRequest::default());
    for order in [
        SortOrder::Filename,
        SortOrder::Duration,
        SortOrder::SampleRate,
        SortOrder::Modified,
        SortOrder::Channels,
        SortOrder::Tags,
    ] {
        out.push(SearchRequest {
            query: "metal".into(),
            order_by: order,
            ..Default::default()
        });
        out.push(SearchRequest {
            query: "door".into(),
            order_by: order,
            descending: true,
            offset: 200,
            ..Default::default()
        });
    }
    for offset in [200, 1000, 10000] {
        out.push(SearchRequest {
            query: "ex".into(),
            offset,
            ..Default::default()
        });
    }
    for filters in [
        SearchFilters {
            sample_rate: Some(48000),
            ..Default::default()
        },
        SearchFilters {
            channels: Some(2),
            ..Default::default()
        },
        SearchFilters {
            duration_min: Some(1.0),
            duration_max: Some(10.0),
            ..Default::default()
        },
        SearchFilters {
            tagged_only: true,
            ..Default::default()
        },
        SearchFilters {
            tag_contains: Some("metal".into()),
            ..Default::default()
        },
        SearchFilters {
            favorites_only: true,
            ..Default::default()
        },
    ] {
        out.push(SearchRequest {
            query: "metal".into(),
            filters,
            ..Default::default()
        });
    }
    let c = ro(path)?;
    let folder:i64=c.query_row("SELECT folder_id FROM audio_files WHERE missing=0 AND folder_id IS NOT NULL GROUP BY folder_id ORDER BY count(*) DESC LIMIT 1",[],|r|r.get(0))?;
    for query in ["", "ex", "metal"] {
        for include_subfolders in [true, false] {
            out.push(SearchRequest {
                query: query.into(),
                folder_id: Some(folder),
                include_subfolders,
                ..Default::default()
            });
        }
    }
    Ok(out)
}

fn full_query(c: &Connection, query: &str) -> R<Value> {
    let compiled = crate::search_text::compile_query(&crate::load_tokenizer(), query)?;
    let start = Instant::now();
    let mut s=c.prepare("SELECT rowid,bm25(audio_search) FROM audio_search WHERE audio_search MATCH ?1 ORDER BY rowid")?;
    let mut rows = s.query([&compiled])?;
    let mut ids = Sha256::new();
    let mut scores = Sha256::new();
    let mut count = 0u64;
    while let Some(row) = rows.next()? {
        let id: i64 = row.get(0)?;
        let score: f64 = row.get(1)?;
        ids.update(id.to_le_bytes());
        scores.update(id.to_le_bytes());
        scores.update(score.to_bits().to_le_bytes());
        count += 1;
    }
    Ok(
        json!({"query":query,"count":count,"ids_sha256":format!("{:x}",ids.finalize()),"scores_sha256":format!("{:x}",scores.finalize()),"seconds":start.elapsed().as_secs_f64()}),
    )
}
fn hash_json<T: serde::Serialize>(v: &T) -> R<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(v)?)))
}
fn bench(path: &Path, out: &Path, only: Option<usize>) -> R<()> {
    let cs = cases(path)?;
    let init = Instant::now();
    let p = crate::SqliteProvider::open_read_only(path)?;
    assert_eq!(p.database_path(), path, "unexpected owned upgrade");
    let open_ms = init.elapsed().as_secs_f64() * 1000.0;
    create_report(out)?;
    let mut records = Vec::new();
    for pass in 0..3 {
        for (index, r) in cs.iter().enumerate() {
            if only.is_some_and(|wanted| wanted != index) {
                continue;
            }
            let started = Instant::now();
            let page = p.search(r)?;
            let page_ms = started.elapsed().as_secs_f64() * 1000.0;
            // Both sources use the current DTO projection. No result fields are
            // normalized here: all returned Metadata, ordered IDs and facets must match.
            let fstart = Instant::now();
            let folders = p.search_folders(r)?;
            let folder_ms = fstart.elapsed().as_secs_f64() * 1000.0;
            let record = json!({"case":index,"pass":pass,"request":r,"page_ms":page_ms,"folder_ms":folder_ms,"combined_ms":page_ms+folder_ms,"count":page.total_count,"returned":page.returned_count,"page_sha256":hash_json(&page.results)?,"folders_sha256":hash_json(&folders)?,"folder_rows":folders.len(),"ids":page.results.iter().map(|f|f.id).collect::<Vec<_>>(),"cache_hit":page.cache_hit});
            log(
                json!({"phase":"bench","case":index,"pass":pass,"page_ms":page_ms,"folder_ms":folder_ms,"count":page.total_count,"sqlite":rusqlite::version()}),
            );
            records.push(record);
            save(
                out,
                &json!({"sqlite":rusqlite::version(),"database":path,"provider_open_ms":open_ms,"only_case":only,"cases":cs.len(),"records":records,"complete":false,"cache_condition":"OS cache uncontrolled; pass 0 first query use in this provider, pass 1/2 reuse same provider"}),
            )?;
        }
    }
    save(
        out,
        &json!({"sqlite":rusqlite::version(),"database":path,"provider_open_ms":open_ms,"only_case":only,"cases":cs.len(),"records":records,"complete":true,"cache_condition":"OS cache uncontrolled; pass 0 first query use in this provider, pass 1/2 reuse same provider"}),
    )?;
    Ok(())
}

fn custom_oracle(c: &Connection, queries: &[String]) -> R<Value> {
    c.execute_batch("BEGIN;")?;
    let lengths = crate::search_bm25::load_lengths(c, None)?;
    let tokenizer = crate::load_tokenizer();
    let mut result = Vec::new();
    for query in queries {
        let compiled = crate::search_text::compile_query(&tokenizer, query)?;
        let prepared = crate::search_bm25::Prepared::empty(compiled.clone());
        prepared.set_lengths(lengths.clone())?;
        let _guard = crate::search_bm25::register_prepared(c, prepared)?;
        let mut s=c.prepare("SELECT rowid,bm25(audio_search),audiolib_bm25(audio_search) FROM audio_search WHERE audio_search MATCH ?1 ORDER BY rowid")?;
        let mut rows = s.query([&compiled])?;
        let mut count = 0u64;
        let mut max_ulp = 0u64;
        while let Some(r) = rows.next()? {
            let native: f64 = r.get(1)?;
            let custom: f64 = r.get(2)?;
            let ulp = native.to_bits().abs_diff(custom.to_bits());
            assert!(
                native.is_finite()
                    && custom.is_finite()
                    && (native == custom
                        || (native.is_sign_negative() == custom.is_sign_negative() && ulp <= 2)),
                "BM25 mismatch for query {query}, id {}, native {native}, custom {custom}",
                r.get::<_, i64>(0)?
            );
            max_ulp = max_ulp.max(ulp);
            count += 1;
        }
        log(json!({"phase":"custom_bm25","query":query,"rows":count,"max_ulp":max_ulp}));
        result.push(json!({"query":query,"rows":count,"max_ulp":max_ulp}));
    }
    c.execute_batch("COMMIT")?;
    Ok(json!({"queries":result,"pass":true,"tolerance_ulp":2}))
}

#[test]
#[ignore = "requires two explicit isolated full-library inputs and a report path"]
fn integrated_equivalence() -> R<()> {
    let source = input("AUDIOLIB_VALIDATION_SOURCE")?;
    let target = input("AUDIOLIB_VALIDATION_TARGET")?;
    let report = input("AUDIOLIB_VALIDATION_REPORT")?;
    assert_ne!(source, target);
    let a = ro(&source)?;
    let b = ro(&target)?;
    create_report(&report)?;
    a.execute_batch("BEGIN; SELECT COUNT(*) FROM sqlite_schema;")?;
    b.execute_batch("BEGIN; SELECT COUNT(*) FROM sqlite_schema;")?;
    let source_version: i64 = a.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    assert!((3..=5).contains(&source_version));
    assert_eq!(
        b.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?,
        6
    );
    assert_eq!(
        b.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name='play_history'",
            [],
            |r| r.get::<_, i64>(0)
        )?,
        0
    );
    assert_eq!(b.query_row("SELECT count(*) FROM pragma_table_info('audio_files') WHERE name IN('play_count','last_played_at') OR name LIKE '%_tokenized'",[],|r|r.get::<_,i64>(0))?,0);
    let metadata_a = digest_sql(&a, "SELECT id,last_played_at FROM audio_files ORDER BY id")?;
    // Raw retired values remain in the immutable rollback backup, independently
    // verified by the migration/rollback oracle. They must not exist in the target.
    assert_eq!(b.query_row("SELECT count(*) FROM pragma_table_info('audio_files') WHERE name='last_played_at' COLLATE NOCASE", [], |r|r.get::<_,i64>(0))?, 0);
    let docsize_a = digest_sql(&a, "SELECT id,sz FROM audio_search_docsize ORDER BY id")?;
    let docsize_b = digest_sql(&b, "SELECT id,sz FROM audio_search_docsize ORDER BY id")?;
    assert_eq!(docsize_a, docsize_b, "document lengths");
    let averages_a: Vec<u8> =
        a.query_row("SELECT block FROM audio_search_data WHERE id=1", [], |r| {
            r.get(0)
        })?;
    let averages_b: Vec<u8> =
        b.query_row("SELECT block FROM audio_search_data WHERE id=1", [], |r| {
            r.get(0)
        })?;
    assert_eq!(averages_a, averages_b, "FTS aggregate statistics");
    a.execute_batch(
        "CREATE VIRTUAL TABLE temp.audit_vocab USING fts5vocab(main,audio_search,row);",
    )?;
    b.execute_batch(
        "CREATE VIRTUAL TABLE temp.audit_vocab USING fts5vocab(main,audio_search,row);",
    )?;
    let vocabulary_a = digest_sql(&a, "SELECT term,doc,cnt FROM audit_vocab ORDER BY term")?;
    let vocabulary_b = digest_sql(&b, "SELECT term,doc,cnt FROM audit_vocab ORDER BY term")?;
    assert_eq!(
        vocabulary_a, vocabulary_b,
        "vocabulary frequency and occurrences"
    );
    log(json!({"phase":"index_statistics","equal":true,"vocabulary":vocabulary_a}));
    let mut comparisons = Vec::new();
    for query in text_queries() {
        let x = full_query(&a, &query)?;
        let y = full_query(&b, &query)?;
        for key in ["count", "ids_sha256", "scores_sha256"] {
            assert_eq!(x[key], y[key], "{query}/{key}");
        }
        log(json!({"phase":"full_query","query":query,"count":x["count"],"equal":true}));
        comparisons.push(json!({"source":x,"target":y}));
        save(&report, &json!({"complete":false,"queries":comparisons}))?;
    }
    a.execute_batch("COMMIT")?;
    b.execute_batch("COMMIT")?;
    let custom = custom_oracle(&b, &text_queries())?;
    save(
        &report,
        &json!({"complete":true,"source":source,"target":target,
        "sqlite":rusqlite::version(),"retired_last_played_at_source_digest":metadata_a,"docsize":docsize_a,
        "aggregate_statistics_equal":true,"vocabulary":vocabulary_a,
        "retired_play_schema_absent":true,"queries":comparisons,"custom_bm25":custom}),
    )
}

#[test]
#[ignore = "requires an explicit read-only database and report path; run alone for timing"]
fn integrated_provider_benchmark() -> R<()> {
    let database = input("AUDIOLIB_VALIDATION_TARGET")?;
    let report = input("AUDIOLIB_VALIDATION_REPORT")?;
    let only = std::env::var("AUDIOLIB_VALIDATION_CASE")
        .ok()
        .map(|value| value.parse())
        .transpose()?;
    bench(&database, &report, only)
}
