// crates/audiolib-db/src/bin/search-benchmark.rs
//! Standalone 1M+ record benchmark through the real Rust DataProvider search pipeline.
use audiolib_core::{DataProvider, SearchFilters, SearchRequest, SortOrder};
use audiolib_db::SqliteProvider;
use rusqlite::{Connection, params};
use std::{path::PathBuf, time::Instant};

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut records = 1_000_000usize;
    let mut queries = 100usize;
    let mut database = None;
    let mut existing_database = None;
    let mut folder_facets = false;
    let mut first_provider = false;
    let mut edge_cases = false;
    let mut missing_records = 0usize;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--folder-facets" => folder_facets = true,
            "--first-provider" => first_provider = true,
            "--edge-cases" => edge_cases = true,
            "--missing-records" => {
                missing_records = arguments
                    .next()
                    .ok_or("--missing-records requires a number")?
                    .parse()?
            }
            "--records" => {
                records = arguments
                    .next()
                    .ok_or("--records requires a number")?
                    .parse()?
            }
            "--queries" => {
                queries = arguments
                    .next()
                    .ok_or("--queries requires a number")?
                    .parse()?
            }
            "--db" => {
                database = Some(PathBuf::from(
                    arguments.next().ok_or("--db requires a path")?,
                ))
            }
            "--existing-db" => {
                existing_database = Some(PathBuf::from(
                    arguments.next().ok_or("--existing-db requires a path")?,
                ));
            }
            "--help" | "-h" => {
                println!(
                    "search-benchmark [--records 1000000] [--queries 100] [--folder-facets] [--first-provider] [--edge-cases] [--missing-records N] [--db NEW_DATABASE | --existing-db MIGRATED_DATABASE]"
                );
                println!(
                    "--first-provider measures one fresh read-only provider per case: first search, then first folder facets; provider opening is reported separately and the OS page cache is not cleared."
                );
                return Ok(());
            }
            _ => return Err(format!("Unknown argument {argument}").into()),
        }
    }
    let mut cases = vec![
        SearchRequest {
            query: "explosion".into(),
            ..Default::default()
        },
        SearchRequest {
            query: "metal".into(),
            ..Default::default()
        },
        SearchRequest {
            query: "爆炸".into(),
            ..Default::default()
        },
        SearchRequest {
            query: "发动机".into(),
            ..Default::default()
        },
        SearchRequest {
            query: "door outdoor".into(),
            folder_id: Some(1),
            ..Default::default()
        },
        SearchRequest {
            query: "ambient".into(),
            filters: SearchFilters {
                sample_rate: Some(48000),
                duration_min: Some(0.0),
                duration_max: Some(5.0),
                ..Default::default()
            },
            ..Default::default()
        },
        SearchRequest {
            filters: SearchFilters {
                favorites_only: true,
                ..Default::default()
            },
            ..Default::default()
        },
        SearchRequest {
            query: "notpresent".into(),
            ..Default::default()
        },
        SearchRequest {
            query: "vehicle".into(),
            offset: 1000,
            ..Default::default()
        },
        SearchRequest {
            folder_id: Some(1),
            ..Default::default()
        },
        // Extracted-tag filters (user tags OR source tags).
        SearchRequest {
            filters: SearchFilters {
                tagged_only: true,
                ..Default::default()
            },
            ..Default::default()
        },
        SearchRequest {
            filters: SearchFilters {
                tag_contains: Some("rareword".into()),
                ..Default::default()
            },
            ..Default::default()
        },
        SearchRequest {
            filters: SearchFilters {
                tag_contains: Some("outdoor".into()),
                ..Default::default()
            },
            ..Default::default()
        },
        SearchRequest {
            query: "rareword".into(),
            ..Default::default()
        },
    ];
    if edge_cases {
        cases.extend([
            SearchRequest {
                query: "metal".into(),
                folder_id: Some(1),
                ..Default::default()
            },
            SearchRequest {
                query: "metal".into(),
                filters: SearchFilters {
                    sample_rate: Some(48000),
                    ..Default::default()
                },
                ..Default::default()
            },
            SearchRequest {
                query: "metal".into(),
                offset: 200_000,
                ..Default::default()
            },
            SearchRequest {
                query: "metal".into(),
                filters: SearchFilters {
                    favorites_only: true,
                    ..Default::default()
                },
                ..Default::default()
            },
            SearchRequest {
                query: "metal".into(),
                order_by: SortOrder::Filename,
                ..Default::default()
            },
        ]);
    }
    let minimum_queries = cases.len();
    if records == 0 || queries < minimum_queries {
        return Err(format!(
            "records must be positive and queries must be at least {minimum_queries}"
        )
        .into());
    }
    let temporary = tempfile::tempdir()?;
    if existing_database.is_some() && database.is_some() {
        return Err("--db and --existing-db are mutually exclusive".into());
    }
    let existing = existing_database.is_some();
    if existing && missing_records != 0 {
        return Err("--missing-records only applies to a newly generated fixture".into());
    }
    if missing_records > records {
        return Err("missing-records must not exceed records".into());
    }
    let path = existing_database
        .or(database)
        .unwrap_or_else(|| temporary.path().join("benchmark.db"));
    if !existing && path.exists() {
        return Err("Benchmark refuses to overwrite an existing database".into());
    }
    let provider = if existing {
        SqliteProvider::open_read_only(&path)?
    } else {
        SqliteProvider::open(&path)?
    };
    let start = Instant::now();
    if existing {
        let stats = provider.stats()?;
        records = usize::try_from(
            stats
                .audio_files
                .checked_add(stats.missing_files)
                .ok_or("Total library record count overflow")?,
        )?;
        missing_records = usize::try_from(stats.missing_files)?;
    } else {
        let mut conn = Connection::open(&path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;
        INSERT INTO folders(id,folder_path,folder_name,depth) VALUES(1,'synthetic','synthetic',0);
        INSERT INTO folder_closure VALUES(1,1,0);",
        )?;
        let types = [
            "explosion",
            "footstep",
            "door",
            "ambient",
            "weapon",
            "vehicle",
            "nature",
            "impact",
            "swoosh",
            "ui",
        ];
        for begin in (0..records).step_by(10_000) {
            let tx = conn.transaction()?;
            {
                let mut insert=tx.prepare("INSERT INTO audio_files(file_path,path_key,filename,filename_tokenized,user_tags,user_tags_tokenized,
                user_notes,user_notes_tokenized,bwf_description,bwf_description_tokenized,folder_id,file_size,sample_rate,bit_depth,channels,duration,user_rating,favorite,missing,metadata_json,source_tags_text)
                VALUES(?1,?1,?2,?2,?3,?3,?4,?5,?6,?7,1,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)")?;
                for index in begin..(begin + 10_000).min(records) {
                    let kind = types[index % types.len()];
                    let filename = format!("{kind}_metal_{index:07}.wav");
                    insert.execute(params![
                        format!("synthetic/{filename}"),
                        filename,
                        format!("{kind} metal outdoor"),
                        if index % 5 == 0 {
                            "车辆发动机环境声"
                        } else {
                            ""
                        },
                        if index % 5 == 0 {
                            "车辆 发动机 环境 声"
                        } else {
                            ""
                        },
                        if index % 10 == 0 {
                            "大型金属爆炸"
                        } else {
                            ""
                        },
                        format!(
                            "{}{}{}",
                            if index % 10 == 0 { "大型 金属 爆炸" } else { "" },
                            if index % 4 == 0 { format!(" {kind} outdoor SFX") } else { String::new() },
                            if index % 1000 == 0 { " rareword" } else { "" },
                        ),
                        100_000 + (index % 5_000_000) as i64,
                        if index % 3 == 0 { 96000 } else { 48000 },
                        if index % 2 == 0 { 24 } else { 16 },
                        if index % 2 == 0 { 2 } else { 1 },
                        0.5 + (index % 200) as f64 / 10.0,
                        (index % 6) as i64,
                        index % 17 == 0,
                        index < missing_records,
                        match (index % 4 == 0, index % 1000 == 0) {
                            (true, true) => format!(
                                "{{\"wav_metadata\":{{\"fields\":{{\"keywords\":\"{kind}, outdoor, rareword\",\"category\":\"SFX\"}}}}}}"
                            ),
                            (true, false) => format!(
                                "{{\"wav_metadata\":{{\"fields\":{{\"keywords\":\"{kind}, outdoor\",\"category\":\"SFX\"}}}}}}"
                            ),
                            _ => "{}".to_string(),
                        },
                        match (index % 4 == 0, index % 1000 == 0) {
                            (true, true) => format!("{kind}, outdoor, rareword, sfx"),
                            (true, false) => format!("{kind}, outdoor, sfx"),
                            _ => String::new(),
                        }
                    ])?;
                }
            }
            tx.commit()?;
        }
        conn.execute_batch("ANALYZE; PRAGMA wal_checkpoint(TRUNCATE);")?;
    }
    let generation_ms = if existing {
        0.0
    } else {
        start.elapsed().as_secs_f64() * 1000.0
    };
    // Every timed query is a full service call: validation, tokenization, SQL COUNT, page DTOs.
    // Report the bounded ranking cache explicitly; Metadata remains a snapshot read.
    // No search has run on any of these providers. Run this phase before the
    // shared-provider initial/warm samples; OS pages remain shared and are not flushed.
    let mut first_provider_cases = Vec::new();
    let mut first_open_samples = Vec::new();
    let mut first_search_samples = Vec::new();
    let mut first_facet_samples = Vec::new();
    let mut first_total_samples = Vec::new();
    if first_provider {
        for request in &cases {
            let opened = Instant::now();
            let fresh = SqliteProvider::open_read_only(&path)?;
            let provider_open_ms = opened.elapsed().as_secs_f64() * 1000.0;
            // Measure search then facets strictly sequentially. The first facets
            // call may use state naturally produced by this provider's first
            // search, with no preceding query or explicit cache prewarm.
            let started = Instant::now();
            let page = fresh.search(request)?;
            let search_ms = started.elapsed().as_secs_f64() * 1000.0;
            let facets_started = Instant::now();
            let folders = fresh.search_folders(request)?;
            let facet_ms = facets_started.elapsed().as_secs_f64() * 1000.0;
            let total_ms = started.elapsed().as_secs_f64() * 1000.0;
            first_open_samples.push(provider_open_ms);
            first_search_samples.push(search_ms);
            first_facet_samples.push(facet_ms);
            first_total_samples.push(total_ms);
            first_provider_cases.push(serde_json::json!({
                "request": request,
                "provider_open_ms": provider_open_ms,
                "search_ms": search_ms,
                "facet_ms": facet_ms,
                "total_ms": total_ms,
                "search_execution_ms": page.execution_time_ms,
                "search_ranking_cache_hit": page.cache_hit,
                "total_count": page.total_count,
                "matching_folders": folders.len()
            }));
        }
    }
    let measure = |request: &SearchRequest| -> audiolib_core::Result<(f64, f64, bool)> {
        let started = Instant::now();
        let page = provider.search(request)?;
        if folder_facets {
            provider.search_folders(request)?;
            Ok((
                started.elapsed().as_secs_f64() * 1000.0,
                page.execution_time_ms,
                page.cache_hit,
            ))
        } else {
            Ok((
                page.execution_time_ms,
                page.execution_time_ms,
                page.cache_hit,
            ))
        }
    };
    let mut cold = Vec::new();
    let mut initial_cache_hits = Vec::new();
    for request in &cases {
        let (latency, _, cache_hit) = measure(request)?;
        cold.push(latency);
        initial_cache_hits.push(cache_hit);
    }
    let initial_samples = cold.clone();
    let mut samples = Vec::with_capacity(queries);
    let mut page_samples = Vec::with_capacity(queries);
    let mut per_case = vec![Vec::<f64>::new(); cases.len()];
    let mut ranking_cache_hits = 0usize;
    for index in 0..queries {
        let (latency, page_latency, cache_hit) = measure(&cases[index % cases.len()])?;
        ranking_cache_hits += usize::from(cache_hit);
        samples.push(latency);
        page_samples.push(page_latency);
        per_case[index % cases.len()].push(latency);
    }
    samples.sort_by(f64::total_cmp);
    page_samples.sort_by(f64::total_cmp);
    cold.sort_by(f64::total_cmp);
    first_open_samples.sort_by(f64::total_cmp);
    first_search_samples.sort_by(f64::total_cmp);
    first_facet_samples.sort_by(f64::total_cmp);
    first_total_samples.sort_by(f64::total_cmp);
    let percentile = |values: &[f64], percent: f64| {
        values[((values.len() - 1) as f64 * percent).ceil() as usize]
    };
    let cases_report:Vec<_>=cases.iter().zip(per_case).enumerate().map(|(index,(request,mut values))|{
        values.sort_by(f64::total_cmp);
        serde_json::json!({"request":request,"p95_ms":percentile(&values,0.95),"samples_ms":values,"initial_pass_ms":initial_samples[index],"initial_ranking_cache_hit":initial_cache_hits[index]})
    }).collect();
    let report = serde_json::json!({"records":records,"queries":queries,"cache_enabled":true,"cache_kind":"bounded ranked IDs; Metadata is read from the current snapshot","ranking_cache_hits":ranking_cache_hits,"edge_cases":edge_cases,"missing_records":missing_records,"folder_facets":folder_facets,
        "database":path,"database_bytes":std::fs::metadata(&path)?.len(),"existing_database":existing,
        "reader_pool_size":4,"sqlite_cache_kb_per_reader":65536,"mmap_bytes":1073741824u64,
        "generation_ms":generation_ms,"p50_ms":percentile(&samples,0.5),
        "p95_ms":percentile(&samples,0.95),"p99_ms":percentile(&samples,0.99),
        "result_page_p50_ms":percentile(&page_samples,0.5),"result_page_p95_ms":percentile(&page_samples,0.95),
        "initial_pass_p95_ms":percentile(&cold,0.95),"target_p95_ms":500,
        "meets_target":percentile(&samples,0.95)<500.0,"latencies_ms":samples,"cases":cases_report,
        "first_provider":first_provider,"first_provider_order":"search_then_facets",
        "first_provider_condition":"One new read-only DataProvider per case, before shared-provider initial/warm samples; no earlier search on that provider; OS page cache is not cleared; total_ms excludes provider opening; facets may use state naturally produced by the first search.",
        "first_provider_samples_per_case":usize::from(first_provider),"first_provider_cases":first_provider_cases,
        "first_provider_open_p95_ms":first_provider.then(||percentile(&first_open_samples,0.95)),
        "first_provider_search_p95_ms":first_provider.then(||percentile(&first_search_samples,0.95)),
        "first_provider_facet_p95_ms":first_provider.then(||percentile(&first_facet_samples,0.95)),
        "first_provider_total_p95_ms":first_provider.then(||percentile(&first_total_samples,0.95)),
        "first_provider_search_meets_target":first_provider.then(||percentile(&first_search_samples,0.95)<500.0),
        "first_provider_facet_meets_target":first_provider.then(||percentile(&first_facet_samples,0.95)<500.0),
        "first_provider_sequential_meets_target":first_provider.then(||percentile(&first_total_samples,0.95)<500.0)});
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
