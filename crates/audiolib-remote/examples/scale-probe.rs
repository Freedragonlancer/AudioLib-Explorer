// crates/audiolib-remote/examples/scale-probe.rs
//! Server-mode scale probe: times one forced snapshot sync against a running server
//! (download, validation, save), then cold open, search, stats, folder tree, revision
//! polls and one PATCH applied into the snapshot, and prints JSON.
//!
//! `cargo run --release -p audiolib-remote --example scale-probe -- --server URL
//! --data TEMP_DIR [--patch-id ID --allow-write] [--no-sync]`
//!
//! Use a temporary data directory: the probe refuses the application's real data
//! directory (`%LOCALAPPDATA%\AudioLibExplorer`). It only reads from the server unless
//! asked otherwise: `--patch-id` changes that record's favorite flag on the server (and
//! restores it afterwards), so it also needs an explicit `--allow-write` and must only be
//! pointed at a test server, never at the production library.
use audiolib_core::{DataProvider, MetadataPatch, SearchRequest, SortOrder};
use audiolib_db::SqliteProvider;
use audiolib_remote::{RemoteCache, RemoteClient, SyncProgress, apply_server_write, sync_library};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Instant;

fn ms(started: Instant) -> f64 {
    (started.elapsed().as_secs_f64() * 1000.0 * 10.0).round() / 10.0
}

/// First (cold) run and the median of `runs` warm runs, in ms.
fn timed<T>(runs: usize, mut call: impl FnMut() -> T) -> (f64, f64, T) {
    let started = Instant::now();
    let mut last = call();
    let cold = ms(started);
    let mut samples: Vec<f64> = (0..runs)
        .map(|_| {
            let started = Instant::now();
            last = call();
            ms(started)
        })
        .collect();
    samples.sort_by(f64::total_cmp);
    let warm = samples.get(samples.len() / 2).copied().unwrap_or(cold);
    (cold, warm, last)
}

/// Whether `data` is (inside) the application's real data directory.
fn is_real_data_dir(data: &Path) -> bool {
    let Some(local) = std::env::var_os("LOCALAPPDATA") else {
        return false;
    };
    let normalize = |path: &Path| {
        let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let text = absolute.to_string_lossy().replace('/', "\\").to_lowercase();
        text.trim_end_matches('\\').to_owned()
    };
    let real = normalize(&Path::new(&local).join("AudioLibExplorer"));
    let data = normalize(data);
    data == real || data.starts_with(&format!("{real}\\"))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut server = String::new();
    let mut data = None;
    let mut patch_id = None;
    let mut allow_write = false;
    let mut sync = true;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--server" => server = arguments.next().ok_or("--server URL")?,
            "--data" => data = Some(PathBuf::from(arguments.next().ok_or("--data DIR")?)),
            "--patch-id" => patch_id = Some(arguments.next().ok_or("--patch-id ID")?.parse()?),
            "--allow-write" => allow_write = true,
            "--no-sync" => sync = false,
            _ => return Err(format!("unknown argument {argument}").into()),
        }
    }
    let data = data.ok_or("--data is required (use a temporary directory)")?;
    if is_real_data_dir(&data) {
        return Err(format!(
            "refusing the application's real data directory {}; use a temporary --data directory",
            data.display()
        )
        .into());
    }
    if patch_id.is_some() && !allow_write {
        return Err(
            "--patch-id writes to the server library: add --allow-write, and only use a test server"
                .into(),
        );
    }
    let client = RemoteClient::new(&server, "")?;
    let cache = RemoteCache::new(&data);
    let mut report = serde_json::Map::new();

    if sync {
        let started = Instant::now();
        let mut marks: Vec<(&str, f64)> = Vec::new();
        let mut last = "";
        let cancel = AtomicBool::new(false);
        let outcome = sync_library(
            &client,
            &cache,
            &mut |progress| {
                let name = match progress {
                    SyncProgress::Checking => "checking",
                    // Streamed decompression overlaps download; keep one timing
                    // interval rather than recording alternating per-chunk marks.
                    SyncProgress::Downloading { .. } | SyncProgress::Decompressing { .. } => {
                        "downloading"
                    }
                    SyncProgress::Writing => "writing",
                    SyncProgress::Validating => "validating",
                    SyncProgress::Preparing(_) => "preparing cache",
                    SyncProgress::Saving => "saving",
                };
                if name != last {
                    marks.push((name, ms(started)));
                    last = name;
                }
            },
            &cancel,
            true,
        )?;
        let total = ms(started);
        let at = |name: &str| marks.iter().find(|(mark, _)| *mark == name).map(|m| m.1);
        let phase = |from: &str, to: Option<f64>| at(from).zip(to).map(|(a, b)| b - a);
        report.insert(
            "sync".into(),
            json!({
                "total_ms": total,
                "check_ms": at("downloading"),
                "download_ms": phase("downloading", at("validating")),
                "validate_ms": phase("validating", at("saving")),
                "save_and_prune_ms": at("saving").map(|s| total - s),
                "snapshot_bytes": outcome.state.snapshot_bytes,
                "audio_files": outcome.state.audio_files,
            }),
        );
    }

    let state = cache.active().ok_or("no synced library")?;
    let database = cache.database_path(&state);
    let started = Instant::now();
    let provider = SqliteProvider::open_read_only(&database)?;
    let open_ms = ms(started);
    let (first_query_ms, _, _) = timed(0, || provider.stats());
    report.insert(
        "open".into(),
        json!({"open_read_only_ms": open_ms, "first_stats_ms": first_query_ms}),
    );

    let mut searches = serde_json::Map::new();
    let cases: Vec<(&str, SearchRequest)> = vec![
        (
            "browse_all_by_name",
            SearchRequest {
                order_by: SortOrder::Filename,
                ..Default::default()
            },
        ),
        (
            "fts_common_metal",
            SearchRequest {
                query: "metal".into(),
                ..Default::default()
            },
        ),
        (
            "fts_common_chinese",
            SearchRequest {
                query: "爆炸".into(),
                ..Default::default()
            },
        ),
        (
            "fts_two_terms",
            SearchRequest {
                query: "metal impact".into(),
                ..Default::default()
            },
        ),
        (
            "fts_prefix_expl",
            SearchRequest {
                query: "expl".into(),
                ..Default::default()
            },
        ),
        (
            "fts_prefix_chinese",
            SearchRequest {
                query: "金".into(),
                ..Default::default()
            },
        ),
        (
            "tag_term_trailer",
            SearchRequest {
                query: "trailer".into(),
                ..Default::default()
            },
        ),
        (
            "no_hit",
            SearchRequest {
                query: "zzqqxxnotpresent".into(),
                ..Default::default()
            },
        ),
        (
            "deep_page_offset_40000",
            SearchRequest {
                order_by: SortOrder::Filename,
                offset: 40_000,
                ..Default::default()
            },
        ),
        (
            "deep_page_metal_offset_10000",
            SearchRequest {
                query: "metal".into(),
                offset: 10_000,
                ..Default::default()
            },
        ),
    ];
    for (name, mut request) in cases {
        request.limit = 200;
        let (cold, warm, page) = timed(5, || provider.search(&request));
        let page = page?;
        searches.insert(
            name.into(),
            json!({"cold_ms": cold, "warm_median_ms": warm, "total": page.total_count, "returned": page.returned_count}),
        );
    }
    report.insert("search".into(), Value::Object(searches));

    let (cold, warm, stats) = timed(5, || provider.stats());
    report.insert(
        "stats".into(),
        json!({"cold_ms": cold, "warm_median_ms": warm, "audio_files": stats?.audio_files}),
    );

    // Whole folder tree, level by level, as a tree view expanding everything would.
    let (cold, warm, count) = timed(2, || -> audiolib_core::Result<usize> {
        let mut pending = vec![None];
        let mut count = 0;
        while let Some(parent) = pending.pop() {
            for folder in provider.folders(parent)? {
                count += 1;
                pending.push(Some(folder.id));
            }
        }
        Ok(count)
    });
    report.insert(
        "folder_tree_full".into(),
        json!({"cold_ms": cold, "warm_median_ms": warm, "folders": count?}),
    );
    let (cold, warm, roots) = timed(5, || provider.folders(None));
    report.insert(
        "folder_tree_top".into(),
        json!({"cold_ms": cold, "warm_median_ms": warm, "folders": roots?.len()}),
    );
    let request = SearchRequest {
        query: "metal".into(),
        ..Default::default()
    };
    let (cold, warm, facets) = timed(5, || provider.search_folders(&request));
    report.insert(
        "folder_facets_metal".into(),
        json!({"cold_ms": cold, "warm_median_ms": warm, "folders": facets?.len()}),
    );

    let (cold, warm, library) = timed(9, || client.library());
    let library = library?;
    report.insert(
        "poll_get_library".into(),
        json!({"cold_ms": cold, "warm_median_ms": warm, "revision": library.revision}),
    );
    let started = Instant::now();
    let _ = sync_library(&client, &cache, &mut |_| {}, &AtomicBool::new(false), false)?;
    report.insert("unchanged_revision_sync_ms".into(), json!(ms(started)));

    if let Some(id) = patch_id {
        let original = provider.get_audio(id)?;
        let patch = MetadataPatch {
            favorite: Some(!original.favorite),
            ..Default::default()
        };
        let started = Instant::now();
        let write = client.patch_metadata(id, &patch)?;
        let http_ms = ms(started);
        let started = Instant::now();
        let update = apply_server_write(
            &cache,
            &state.library_id,
            std::slice::from_ref(&write.file),
            write.revision_before,
            write.revision,
        );
        let apply_ms = ms(started);
        let visible = provider.get_audio(id)?.favorite == patch.favorite.unwrap_or_default();
        let restore = MetadataPatch {
            favorite: Some(original.favorite),
            ..Default::default()
        };
        let write = client.patch_metadata(id, &restore)?;
        let _ = apply_server_write(
            &cache,
            &state.library_id,
            std::slice::from_ref(&write.file),
            write.revision_before,
            write.revision,
        );
        report.insert(
            "patch".into(),
            json!({"http_ms": http_ms, "apply_into_snapshot_ms": apply_ms, "update": format!("{update:?}"), "visible_in_open_provider": visible}),
        );
    }
    println!("{}", serde_json::to_string_pretty(&Value::Object(report))?);
    Ok(())
}
