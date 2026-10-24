// crates/audiolib-db/src/bin/tag-exclude-diagnose.rs
//! Read-only exclude-only tag filter plans and stage timings on an existing library.
//! `shared_*` probes are the full-scan statements every tag filter shape used before
//! exclude-only filters were counted by subtraction; the other probes are its stages.
//! Plans are also reported without ANALYZE statistics. These SQL probes are not
//! DataProvider or GUI end-to-end measurements; use search-benchmark for those.
use rusqlite::{Connection, OpenFlags, params_from_iter, types::Value};
use std::{path::Path, time::Instant};

const TAGGED: &str = "(COALESCE(trim(af.user_tags),'')<>'' OR af.source_tags_text<>'')";
const PASSES: &str = "((COALESCE(trim(af.user_tags),'')='' AND af.source_tags_text='') OR audiolib_tag_terms(af.user_tags,af.source_tags_text,?1)=1)";
const EXCLUDED: &str = "audiolib_tag_terms(af.user_tags,af.source_tags_text,?1)=0";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args().skip(1);
    let path = arguments
        .next()
        .ok_or("Usage: tag-exclude-diagnose DATABASE [LOWERCASE_EXCLUDE_TERM...]")?;
    let mut terms = arguments.collect::<Vec<_>>();
    if terms.is_empty() {
        terms.push("explosion".into());
    }
    let terms_json = serde_json::to_string(
        &terms
            .iter()
            .map(|term| (true, term.to_lowercase()))
            .collect::<Vec<_>>(),
    )?;
    let conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    audiolib_db::register_library_functions(&conn)?;
    conn.execute_batch("PRAGMA cache_size=-65536; PRAGMA temp_store=MEMORY;")?;
    conn.pragma_update(
        None,
        "mmap_size",
        audiolib_db::sqlite_reader_mmap_bytes(Path::new(&path)),
    )?;
    let page_at = |order: &str, direction: &str, from: &str, offset: u32| {
        format!(
            "SELECT af.id,af.{order} AS sort_value FROM {from} WHERE af.missing=0 AND {PASSES}
             ORDER BY sort_value {direction},af.id ASC LIMIT 200 OFFSET {offset}"
        )
    };
    let page = |order: &str, direction: &str, from: &str| page_at(order, direction, from, 0);
    let probes = [
        (
            "shared_count",
            format!("SELECT COUNT(*) FROM audio_files af WHERE af.missing=0 AND {PASSES}"),
        ),
        (
            "shared_page_duration_asc",
            page("duration", "ASC", "audio_files af"),
        ),
        (
            "shared_page_duration_desc",
            page("duration", "DESC", "audio_files af"),
        ),
        (
            "indexed_page_duration_asc",
            page("duration", "ASC", "audio_files af INDEXED BY idx_duration"),
        ),
        (
            "indexed_page_duration_desc",
            page("duration", "DESC", "audio_files af INDEXED BY idx_duration"),
        ),
        (
            "indexed_page_duration_desc_offset_100000",
            page_at(
                "duration",
                "DESC",
                "audio_files af INDEXED BY idx_duration",
                100_000,
            ),
        ),
        (
            "indexed_page_duration_desc_offset_850000",
            page_at(
                "duration",
                "DESC",
                "audio_files af INDEXED BY idx_duration",
                850_000,
            ),
        ),
        (
            "unindexed_page_duration_desc_offset_850000",
            page_at("duration", "DESC", "audio_files af NOT INDEXED", 850_000),
        ),
        (
            "shared_page_sample_rate_desc",
            page("sample_rate", "DESC", "audio_files af"),
        ),
        (
            "shared_page_modified_desc",
            page("modified_at", "DESC", "audio_files af"),
        ),
        (
            "shared_page_filename",
            format!(
                "SELECT af.id,af.filename COLLATE NOCASE AS sort_value FROM audio_files af
                 WHERE af.missing=0 AND {PASSES} ORDER BY sort_value ASC,af.id ASC LIMIT 200"
            ),
        ),
        (
            "shared_facet_ids",
            format!("SELECT af.id FROM audio_files af WHERE af.missing=0 AND {PASSES}"),
        ),
        (
            "visible_count",
            "SELECT COUNT(*) FROM audio_files af WHERE af.missing=0 AND ?1 IS NOT NULL"
                .to_string(),
        ),
        (
            "tagged_scan_without_terms",
            format!(
                "SELECT COUNT(*) FROM audio_files af INDEXED BY idx_audio_visible_tagged_folders
                 WHERE af.missing=0 AND {TAGGED} AND af.user_tags IS NOT ?1 AND af.source_tags_text IS NOT ?1"
            ),
        ),
        (
            "excluded_count",
            format!(
                "SELECT COUNT(*) FROM audio_files af INDEXED BY idx_audio_visible_tagged_folders
                 WHERE af.missing=0 AND {TAGGED} AND {EXCLUDED}"
            ),
        ),
        (
            "visible_folders",
            "SELECT af.folder_id,COUNT(*) FROM audio_files af WHERE af.missing=0 AND ?1 IS NOT NULL
             GROUP BY af.folder_id"
                .to_string(),
        ),
        (
            "excluded_folders",
            format!(
                "SELECT af.folder_id,COUNT(*) FROM audio_files af INDEXED BY idx_audio_visible_tagged_folders
                 WHERE af.missing=0 AND {TAGGED} AND {EXCLUDED} GROUP BY af.folder_id"
            ),
        ),
    ];
    let values = [Value::Text(terms_json.clone())];
    // Real libraries often have no ANALYZE statistics. Plan the same statements on an
    // empty in-memory copy of the table/index schema, where SQLite uses its defaults.
    let unanalyzed = Connection::open_in_memory()?;
    audiolib_db::register_library_functions(&unanalyzed)?;
    let schema = conn
        .prepare(
            "SELECT sql FROM sqlite_schema WHERE type IN ('table','index') AND sql IS NOT NULL
             AND tbl_name IN ('audio_files','folders','folder_closure') ORDER BY type DESC",
        )?
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for sql in schema {
        unanalyzed.execute_batch(&sql)?;
    }
    let mut unanalyzed_plans = serde_json::Map::new();
    for (label, sql) in &probes {
        let plan = unanalyzed
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?
            .query_map(params_from_iter(&values), |row| row.get::<_, String>(3))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        unanalyzed_plans.insert((*label).into(), plan.into());
    }
    let mut reports = Vec::new();
    for (label, sql) in &probes {
        let plan = conn
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?
            .query_map(params_from_iter(&values), |row| row.get::<_, String>(3))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut samples = Vec::new();
        let mut rows = 0usize;
        let mut first = None;
        for _ in 0..5 {
            let start = Instant::now();
            let mut statement = conn.prepare(sql)?;
            let mut cursor = statement.query(params_from_iter(&values))?;
            rows = 0;
            while let Some(row) = cursor.next()? {
                if rows == 0 {
                    first = Some(row.get::<_, Value>(0)?);
                }
                rows += 1;
            }
            samples.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        samples.sort_by(f64::total_cmp);
        reports.push(serde_json::json!({
            "label":label,"plan":plan,"rows":rows,"first":format!("{first:?}"),
            "median_ms":samples[2],"min_ms":samples[0],"max_ms":samples[4]
        }));
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "sqlite":rusqlite::version(),"terms":terms_json,
            "scope":"SQL stages only; OS cache uncontrolled; five passes each","probes":reports,
            "plans_without_statistics":unanalyzed_plans
        }))?
    );
    Ok(())
}
