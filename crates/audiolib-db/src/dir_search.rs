// crates/audiolib-db/src/dir_search.rs
//! Directory-segment path search for `scope=all` and `scope=path`.
//!
//! `audio_dir_search` stores one FTS row per folder (rowid = `folders.id`). Only that
//! folder's own name is indexed, after NFC and fullwidth-to-halfwidth normalization.
//! Library roots (`parent_id IS NULL`) keep a row but store no name, so a query that
//! only matches the mount prefix does not hit every file. `audio_dir_runs` stores
//! maximal consecutive file-id ranges for each folder. A file path-matches a term when
//! its folder or any ancestor segment matches.
//!
//! Ranking for `scope=all` is three tiers: main-field bm25 (the s8 order), then mixed
//! matches by file id, then path-only matches by file id. `scope=path` omits the pure
//! main tier. When a term's directory set is empty and that term also misses the main
//! FTS index (`LIMIT 1`), the page is empty and the full main scan is not run.

use crate::db_error;
use crate::search_text::{compile_query, compile_query_columns, query_terms};
use audiolib_core::{Error, FileVisibility, Result, SearchScope};
use jieba_rs::Jieba;
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

const PATH_RAW: &str = "{path_tokens}";
const PATH_DERIVED: &str = "{path_tokens_tokenized}";
/// Directory-segment index layout. Cursors carry this value; a mismatch is stale.
pub(crate) const SEARCH_PATH_REV: i64 = 1;
/// At or below this many estimated files, only that folder subtree's runs are scanned.
const SUBTREE_FILE_LIMIT: i64 = 50_000;

pub(crate) const CREATE_DIR_FTS: &str = "
CREATE VIRTUAL TABLE audio_dir_search USING fts5(
 path_tokens,path_tokens_tokenized,
 content='',contentless_delete=1,detail=full,columnsize=1,
 tokenize='unicode61 remove_diacritics 2');";

pub(crate) const CREATE_RUNS: &str = "
CREATE TABLE audio_dir_runs(
 dir_id INTEGER NOT NULL,
 lo INTEGER NOT NULL,
 hi INTEGER NOT NULL,
 n INTEGER NOT NULL,
 PRIMARY KEY(dir_id, lo)
) WITHOUT ROWID;";

const INSERT_SEGMENTS: &str = "
INSERT INTO audio_dir_search(rowid,path_tokens,path_tokens_tokenized)
SELECT id,
 CASE WHEN parent_id IS NULL THEN '' ELSE t3_norm(COALESCE(folder_name,'')) END,
 CASE WHEN parent_id IS NULL THEN '' ELSE audiolib_tokenize(t3_norm(COALESCE(folder_name,''))) END
FROM folders;";

pub(crate) const INSERT_RUNS: &str = "
INSERT INTO audio_dir_runs
SELECT folder_id, min(id), max(id), count(*)
FROM (
 SELECT id, folder_id, sum(chg) OVER (ORDER BY id) AS g
 FROM (
  SELECT id, folder_id,
   CASE WHEN folder_id IS lag(folder_id) OVER (ORDER BY id) THEN 0 ELSE 1 END AS chg
  FROM audio_files
  WHERE folder_id IS NOT NULL
 )
)
GROUP BY g;";

/// NFC, then map ideographic space and fullwidth ASCII into their halfwidth forms.
pub(crate) fn t3_norm(value: &str) -> String {
    let nfc = icu_normalizer::ComposingNormalizerBorrowed::new_nfc();
    nfc.normalize(value)
        .chars()
        .map(|character| match character as u32 {
            0x3000 => ' ',
            unit @ 0xFF01..=0xFF5E => char::from_u32(unit - 0xFEE0).unwrap_or(character),
            _ => character,
        })
        .collect()
}

pub(crate) fn register_t3(conn: &Connection) -> Result<()> {
    conn.create_scalar_function(
        "t3_norm",
        1,
        rusqlite::functions::FunctionFlags::SQLITE_UTF8
            | rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC,
        |context| {
            let value = context.get::<Option<String>>(0)?.unwrap_or_default();
            Ok(t3_norm(&value))
        },
    )
    .map_err(db_error)
}

/// A path set wider than half the library is ignored and the term stays on the main index.
pub(crate) fn path_set_too_wide(estimated_files: i64, total_files: i64) -> bool {
    total_files > 0 && estimated_files.saturating_mul(2) > total_files
}

/// Subtree runs are a worthwhile filter only while the estimate stays modest.
pub(crate) fn restrict_to_subtree(estimated_files: i64) -> bool {
    estimated_files <= SUBTREE_FILE_LIMIT
}

pub(crate) fn ready(conn: &Connection) -> Result<bool> {
    let tables: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='audio_dir_search')
             AND EXISTS(SELECT 1 FROM sqlite_schema WHERE name='audio_dir_runs')",
            [],
            |row| row.get(0),
        )
        .map_err(db_error)?;
    if !tables {
        return Ok(false);
    }
    Ok(meta_i64(conn, "search_path_rev")? == Some(SEARCH_PATH_REV))
}

/// Drop and rebuild both directory tables. Caller holds the library writer.
pub(crate) fn rebuild(conn: &Connection) -> Result<()> {
    let own = conn.is_autocommit();
    if own {
        conn.execute_batch("BEGIN IMMEDIATE").map_err(db_error)?;
    }
    let result = (|| -> Result<()> {
        conn.execute_batch(
            "DROP TABLE IF EXISTS audio_dir_search; DROP TABLE IF EXISTS audio_dir_runs;",
        )
        .map_err(db_error)?;
        conn.execute_batch(CREATE_DIR_FTS).map_err(db_error)?;
        conn.execute_batch(CREATE_RUNS).map_err(db_error)?;
        conn.execute_batch(INSERT_SEGMENTS).map_err(db_error)?;
        conn.execute_batch(INSERT_RUNS).map_err(db_error)?;
        conn.execute(
            "INSERT INTO audio_dir_search(audio_dir_search) VALUES('optimize')",
            [],
        )
        .map_err(db_error)?;
        write_meta(conn, "search_path_rev", &SEARCH_PATH_REV.to_string())?;
        // Directory-segment index id from the schema 8→9 design. `sd1`, not `sp1`.
        write_meta(
            conn,
            "snapshot_format",
            audiolib_core::compatibility::PLAN_B_V9_FORMAT,
        )?;
        let generation = meta_i64(conn, "dir_index_gen")?
            .unwrap_or(0)
            .saturating_add(1);
        write_meta(conn, "dir_index_gen", &generation.to_string())?;
        Ok(())
    })();
    if own {
        if result.is_ok() {
            conn.execute_batch("COMMIT").map_err(db_error)?;
        } else {
            let _ = conn.execute_batch("ROLLBACK");
        }
    }
    result
}

/// Reindex renamed or deleted folders and recompute runs whose files changed.
/// No-op until [`rebuild`] has published `search_path_rev`.
pub(crate) fn maintain(
    conn: &Connection,
    dirs_changed: &[i64],
    dirs_files_changed: &[i64],
) -> Result<()> {
    if !ready(conn)? || (dirs_changed.is_empty() && dirs_files_changed.is_empty()) {
        return Ok(());
    }
    for id in dirs_changed {
        reindex_segment(conn, *id)?;
    }
    conn.execute(
        "DELETE FROM audio_dir_runs WHERE dir_id NOT IN (SELECT id FROM folders)",
        [],
    )
    .map_err(db_error)?;
    for id in dirs_files_changed {
        conn.execute("DELETE FROM audio_dir_runs WHERE dir_id=?1", [id])
            .map_err(db_error)?;
        conn.execute(
            "INSERT INTO audio_dir_runs
             SELECT folder_id, min(id), max(id), count(*)
             FROM (
              SELECT id, folder_id, sum(chg) OVER (ORDER BY id) AS g
              FROM (
               SELECT id, folder_id,
                CASE WHEN folder_id IS lag(folder_id) OVER (ORDER BY id) THEN 0 ELSE 1 END AS chg
               FROM audio_files
               WHERE id BETWEEN
                (SELECT coalesce(min(id), 0) FROM audio_files WHERE folder_id=?1)
                AND (SELECT coalesce(max(id), -1) FROM audio_files WHERE folder_id=?1)
              )
             )
             WHERE folder_id=?1
             GROUP BY g",
            [id],
        )
        .map_err(db_error)?;
    }
    let generation = meta_i64(conn, "dir_index_gen")?
        .unwrap_or(0)
        .saturating_add(1);
    write_meta(conn, "dir_index_gen", &generation.to_string())
}

pub(crate) fn bump_generation(conn: &Connection) -> Result<()> {
    if !ready(conn)? {
        return Ok(());
    }
    let generation = meta_i64(conn, "dir_index_gen")?
        .unwrap_or(0)
        .saturating_add(1);
    write_meta(conn, "dir_index_gen", &generation.to_string())
}

#[derive(Clone, Copy)]
struct Run {
    lo: i64,
    hi: i64,
    dense: bool,
    dir: i64,
}

struct RunIdx {
    all: Vec<Run>,
    by_dir: HashMap<i64, Vec<usize>>,
    parent: HashMap<i64, i64>,
    children: HashMap<i64, Vec<i64>>,
    tfc: HashMap<i64, i64>,
}

#[derive(Default)]
pub(crate) struct DirIndexCache {
    inner: Mutex<Option<(i64, Arc<RunIdx>)>>,
}

pub(crate) struct DirHit {
    pub tier: u8,
    pub score: f64,
    pub id: i64,
}

pub(crate) struct DirPage {
    pub hits: Vec<DirHit>,
    pub short_circuited: bool,
    pub has_more: bool,
    /// Hits actually observed, including skipped rows and the lookahead row.
    pub observed_count: usize,
}

#[derive(Clone, Copy)]
struct Cursor {
    tier: u8,
    score: f64,
    id: i64,
    rev: i64,
}

pub(crate) fn encode_cursor(tier: u8, score: f64, id: i64, rev: i64) -> String {
    let raw = format!("v1|{tier}|{}|{id}|{rev}", score.to_bits());
    base64url(raw.as_bytes())
}

fn decode_cursor(value: &str) -> Result<Cursor> {
    let bytes = base64url_decode(value)
        .map_err(|_| Error::Validation("Search cursor is invalid".into()))?;
    let text = String::from_utf8(bytes)
        .map_err(|_| Error::Validation("Search cursor is invalid".into()))?;
    let mut parts = text.split('|');
    let invalid = || Error::Validation("Search cursor is invalid".into());
    if parts.next() != Some("v1") {
        return Err(invalid());
    }
    let tier: u8 = parts
        .next()
        .and_then(|part| part.parse().ok())
        .ok_or_else(invalid)?;
    let bits: u64 = parts
        .next()
        .and_then(|part| part.parse().ok())
        .ok_or_else(invalid)?;
    let id: i64 = parts
        .next()
        .and_then(|part| part.parse().ok())
        .ok_or_else(invalid)?;
    let rev: i64 = parts
        .next()
        .and_then(|part| part.parse().ok())
        .ok_or_else(invalid)?;
    if parts.next().is_some() || tier > 2 {
        return Err(invalid());
    }
    Ok(Cursor {
        tier,
        score: f64::from_bits(bits),
        id,
        rev,
    })
}

/// Inputs for one directory-aware page. `offset` is ignored when `cursor` is set.
#[derive(Clone, Copy)]
pub(crate) struct DirSearch<'a> {
    pub conn: &'a Connection,
    pub cache: &'a DirIndexCache,
    pub jieba: &'a Jieba,
    pub query: &'a str,
    pub scope: SearchScope,
    pub visibility: FileVisibility,
    pub cursor: Option<&'a str>,
    pub offset: usize,
    pub limit: usize,
}

/// One page of directory-aware hits.
pub(crate) fn search(request: &DirSearch<'_>) -> Result<DirPage> {
    let DirSearch {
        conn,
        cache,
        jieba,
        query,
        scope,
        visibility,
        cursor,
        offset,
        limit,
    } = *request;
    let full_match = compile_query(jieba, query)?;
    let terms = query_terms(jieba, query);
    if terms.is_empty() {
        return Err(Error::Validation("Search requires a word or number".into()));
    }
    let cursor = cursor.map(decode_cursor).transpose()?;
    if let Some(cursor) = cursor
        && cursor.rev != SEARCH_PATH_REV
    {
        return Err(Error::Validation(format!(
            "CURSOR_STALE: search_path_rev {} != {SEARCH_PATH_REV}",
            cursor.rev
        )));
    }
    let index = load_index(conn, cache)?;
    let mut sets = Vec::with_capacity(terms.len());
    for term in &terms {
        let matched = compile_query_columns(jieba, &t3_norm(term), PATH_RAW, PATH_DERIVED)?;
        sets.push(segment_matches(conn, &matched)?);
    }
    let total_files: i64 = index.all.iter().map(|run| run.hi - run.lo + 1).sum();
    for set in &mut sets {
        if path_set_too_wide(estimate(set, &index), total_files) {
            set.clear();
        }
    }
    // A term with no directory hit can still be satisfied by the main index. One
    // posting is enough to prove that. Zero postings means every tier is empty,
    // so the full main AND and the nopath scan never start.
    for (index_term, term) in terms.iter().enumerate() {
        if !sets[index_term].is_empty() {
            continue;
        }
        let main = compile_query(jieba, term)?;
        if !main_has_hit(conn, &main)? {
            return Ok(DirPage {
                hits: Vec::new(),
                short_circuited: true,
                has_more: false,
                observed_count: 0,
            });
        }
    }

    let skip = if cursor.is_some() { 0 } else { offset };
    let want = skip.saturating_add(limit).saturating_add(1);
    let mut produced = Vec::new();
    if scope == SearchScope::All && cursor.map(|item| item.tier).unwrap_or(0) == 0 {
        let after = cursor
            .filter(|item| item.tier == 0)
            .map(|item| (item.score, item.id));
        produced.extend(tier0(conn, &full_match, after, want, visibility)?);
    }
    if produced.len() < want {
        let term_match = terms
            .iter()
            .map(|term| compile_query(jieba, term))
            .collect::<Result<Vec<_>>>()?;
        for tier in [1_u8, 2] {
            if produced.len() >= want {
                break;
            }
            if cursor.is_some_and(|item| item.tier > tier) {
                continue;
            }
            let after_id = cursor
                .filter(|item| item.tier == tier)
                .map(|item| item.id)
                .unwrap_or(i64::MIN);
            let ids = tier_ids(
                &TierQuery {
                    conn,
                    index: &index,
                    sets: &sets,
                    term_match: &term_match,
                    full_match: &full_match,
                    scope,
                    visibility,
                },
                tier,
                after_id,
                want - produced.len(),
            )?;
            let score = if tier == 1 { 1.0 } else { 2.0 };
            produced.extend(ids.into_iter().map(|id| DirHit { tier, score, id }));
        }
    }
    let observed_count = produced.len();
    let mut rest = produced.into_iter().skip(skip).collect::<Vec<_>>();
    let has_more = rest.len() > limit;
    rest.truncate(limit);
    Ok(DirPage {
        hits: rest,
        short_circuited: false,
        has_more,
        observed_count,
    })
}

fn tier0(
    conn: &Connection,
    full_match: &str,
    after: Option<(f64, i64)>,
    limit: usize,
    visibility: FileVisibility,
) -> Result<Vec<DirHit>> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let visibility = visibility_sql(visibility);
    let (sql, owned_params): (String, Vec<rusqlite::types::Value>) =
        if let Some((score, id)) = after {
            (
                format!(
                    "SELECT s, id FROM (
                    SELECT rowid AS id, bm25(audio_search) AS s
                    FROM audio_search WHERE audio_search MATCH ?1
                 ) ranked
                 WHERE (s > ?2 OR (s = ?2 AND id > ?3))
                   AND EXISTS (
                    SELECT 1 FROM audio_files af WHERE af.id = ranked.id AND {visibility}
                   )
                 ORDER BY s, id LIMIT ?4"
                ),
                vec![
                    rusqlite::types::Value::Text(full_match.to_owned()),
                    rusqlite::types::Value::Real(score),
                    rusqlite::types::Value::Integer(id),
                    rusqlite::types::Value::Integer(limit as i64),
                ],
            )
        } else {
            (
                format!(
                    "SELECT s, id FROM (
                    SELECT rowid AS id, bm25(audio_search) AS s
                    FROM audio_search WHERE audio_search MATCH ?1
                 ) ranked
                 WHERE EXISTS (
                    SELECT 1 FROM audio_files af WHERE af.id = ranked.id AND {visibility}
                 )
                 ORDER BY s, id LIMIT ?2"
                ),
                vec![
                    rusqlite::types::Value::Text(full_match.to_owned()),
                    rusqlite::types::Value::Integer(limit as i64),
                ],
            )
        };
    let mut statement = conn.prepare_cached(&sql).map_err(db_error)?;
    let rows = statement
        .query_map(rusqlite::params_from_iter(owned_params), |row| {
            Ok(DirHit {
                tier: 0,
                score: row.get(0)?,
                id: row.get(1)?,
            })
        })
        .map_err(db_error)?;
    rows.collect::<rusqlite::Result<_>>().map_err(db_error)
}

#[derive(Clone, Copy)]
struct TierQuery<'a> {
    conn: &'a Connection,
    index: &'a RunIdx,
    sets: &'a [HashSet<i64>],
    term_match: &'a [String],
    full_match: &'a str,
    scope: SearchScope,
    visibility: FileVisibility,
}

fn tier_ids(query: &TierQuery<'_>, tier: u8, after_id: i64, limit: usize) -> Result<Vec<i64>> {
    let TierQuery {
        conn,
        index,
        sets,
        term_match,
        full_match,
        scope,
        visibility,
    } = *query;
    if limit == 0 {
        return Ok(Vec::new());
    }
    if tier == 1 && (term_match.len() < 2 || sets.iter().all(HashSet::is_empty)) {
        return Ok(Vec::new());
    }
    if tier == 2 && sets.iter().any(HashSet::is_empty) {
        return Ok(Vec::new());
    }
    if tier == 2 {
        let Some(smallest) = smallest_set(sets, index) else {
            return Ok(Vec::new());
        };
        let runs = runs_for(index, &sets[smallest]);
        let mut main = MainPostings::new(full_match, after_id, i64::MAX);
        return stream_runs(
            conn,
            &runs,
            after_id,
            limit,
            visibility,
            |dir| path_full(sets, index, dir),
            // Path scope retains full path hits even when main also matches.
            // All scope excludes its earlier tier-0 hits with one advancing stream.
            |id| Ok(scope != SearchScope::All || !main.contains(conn, id)?),
        );
    }
    stream_mixed(query, after_id, limit)
}

/// A bounded rowid-ordered FTS stream. No posting list is materialized in full,
/// and each refill advances its lower bound instead of restarting MATCH per file.
struct MainPostings<'a> {
    expression: &'a str,
    after: i64,
    upper: i64,
    ids: Vec<i64>,
    position: usize,
    done: bool,
}

impl<'a> MainPostings<'a> {
    fn new(expression: &'a str, after: i64, upper: i64) -> Self {
        Self {
            expression,
            after,
            upper,
            ids: Vec::new(),
            position: 0,
            done: false,
        }
    }

    fn head(&mut self, conn: &Connection) -> Result<Option<i64>> {
        if self.position == self.ids.len() {
            if self.done {
                return Ok(None);
            }
            let mut statement = conn
                .prepare_cached(
                    "SELECT rowid FROM audio_search WHERE audio_search MATCH ?1
                     AND rowid>?2 AND rowid<=?3 ORDER BY rowid LIMIT 256",
                )
                .map_err(db_error)?;
            self.ids = statement
                .query_map(params![self.expression, self.after, self.upper], |row| {
                    row.get(0)
                })
                .map_err(db_error)?
                .collect::<rusqlite::Result<_>>()
                .map_err(db_error)?;
            self.position = 0;
            self.done = self.ids.len() < 256;
            if let Some(last) = self.ids.last() {
                self.after = *last;
            }
        }
        Ok(self.ids.get(self.position).copied())
    }

    fn advance(&mut self) {
        self.position += 1;
    }

    /// Call with increasing IDs; skip untouched gaps before refilling a batch.
    fn contains(&mut self, conn: &Connection, id: i64) -> Result<bool> {
        self.position = self.ids.partition_point(|candidate| *candidate < id);
        if self.position == self.ids.len() {
            self.after = self.after.max(id.saturating_sub(1));
        }
        Ok(self.head(conn)? == Some(id))
    }
}

fn stream_mixed(query: &TierQuery<'_>, after_id: i64, limit: usize) -> Result<Vec<i64>> {
    let TierQuery {
        conn,
        index,
        sets,
        term_match,
        full_match,
        visibility,
        ..
    } = *query;
    let mut missing_by_dir = HashMap::new();
    let mut needed = vec![false; sets.len()];
    let mut low = i64::MAX;
    let mut high = i64::MIN;
    for (&dir, slots) in &index.by_dir {
        let missed = sets
            .iter()
            .enumerate()
            .filter_map(|(term, set)| (!ancestor_has(set, index, dir)).then_some(term))
            .collect::<Vec<_>>();
        // Pure main and complete path directories cannot contribute to tier 1.
        // In particular, a full-path page need not open the main FTS table at all.
        if missed.is_empty() || missed.len() == sets.len() {
            continue;
        }
        let mut has_remaining_run = false;
        for &slot in slots {
            let run = index.all[slot];
            if run.hi > after_id {
                low = low.min(run.lo);
                high = high.max(run.hi);
                has_remaining_run = true;
            }
        }
        if has_remaining_run {
            for &term in &missed {
                needed[term] = true;
            }
            missing_by_dir.insert(dir, missed);
        }
    }
    if missing_by_dir.is_empty() {
        return Ok(Vec::new());
    }
    let lower = after_id.max(low.saturating_sub(1));
    let mut streams = term_match
        .iter()
        .enumerate()
        .filter(|(term, _)| needed[*term])
        .map(|(term, expression)| (term, MainPostings::new(expression, lower, high)))
        .collect::<Vec<_>>();
    let mut main_full = MainPostings::new(full_match, lower, high);
    let mut main_terms = vec![false; sets.len()];
    let mut visible = conn
        .prepare_cached(&format!(
            "SELECT 1 FROM audio_files WHERE id=?1 AND folder_id=?2 AND {}",
            visibility_sql(visibility)
        ))
        .map_err(db_error)?;
    let mut out = Vec::new();
    while out.len() < limit {
        let mut next = None;
        for (_, stream) in &mut streams {
            if let Some(id) = stream.head(conn)? {
                next = Some(next.map_or(id, |previous: i64| previous.min(id)));
            }
        }
        let Some(id) = next else { break };
        main_terms.fill(false);
        for (term, stream) in &mut streams {
            if stream.head(conn)? == Some(id) {
                main_terms[*term] = true;
                stream.advance();
            }
        }
        let Some(dir) = dir_of(index, id) else {
            continue;
        };
        let Some(missed) = missing_by_dir.get(&dir) else {
            continue;
        };
        if missed.iter().all(|term| main_terms[*term])
            && !main_full.contains(conn, id)?
            && visible.exists(params![id, dir]).map_err(db_error)?
        {
            out.push(id);
        }
    }
    Ok(out)
}

fn stream_runs(
    conn: &Connection,
    runs: &[Run],
    after_id: i64,
    limit: usize,
    visibility: FileVisibility,
    mut accept_dir: impl FnMut(i64) -> bool,
    mut accept_id: impl FnMut(i64) -> Result<bool>,
) -> Result<Vec<i64>> {
    let mut out = Vec::new();
    let mut run_index = runs.partition_point(|run| run.hi <= after_id);
    while run_index < runs.len() && out.len() < limit {
        let run = runs[run_index];
        run_index += 1;
        if !accept_dir(run.dir) {
            continue;
        }
        let mut from = after_id.saturating_add(1).max(run.lo);
        while from <= run.hi && out.len() < limit {
            let mut ids = Vec::new();
            expand(conn, &run, from, 256, visibility, &mut ids)?;
            let Some(last) = ids.last().copied() else {
                break;
            };
            for id in ids {
                if accept_id(id)? {
                    out.push(id);
                    if out.len() == limit {
                        break;
                    }
                }
            }
            from = last.saturating_add(1);
        }
    }
    Ok(out)
}

fn expand(
    conn: &Connection,
    run: &Run,
    from: i64,
    limit: usize,
    visibility: FileVisibility,
    out: &mut Vec<i64>,
) -> Result<()> {
    let lo = run.lo.max(from);
    if lo > run.hi || limit == 0 {
        return Ok(());
    }
    if run.dense && visibility == FileVisibility::All {
        let mut id = lo;
        while id <= run.hi && out.len() < limit {
            out.push(id);
            id += 1;
        }
        return Ok(());
    }
    let sql = format!(
        "SELECT id FROM audio_files WHERE id>=?1 AND id<=?2 AND folder_id=?4 AND {} ORDER BY id LIMIT ?3",
        visibility_sql(visibility)
    );
    let mut statement = conn.prepare_cached(&sql).map_err(db_error)?;
    let rows = statement
        .query_map(params![lo, run.hi, limit as i64, run.dir], |row| row.get(0))
        .map_err(db_error)?;
    for id in rows {
        out.push(id.map_err(db_error)?);
    }
    Ok(())
}

fn visibility_sql(visibility: FileVisibility) -> &'static str {
    match visibility {
        FileVisibility::Present => "missing=0",
        FileVisibility::Missing => "missing=1",
        FileVisibility::All => "1=1",
    }
}

fn path_full(sets: &[HashSet<i64>], index: &RunIdx, dir: i64) -> bool {
    sets.iter().all(|set| ancestor_has(set, index, dir))
}

fn ancestor_has(set: &HashSet<i64>, index: &RunIdx, mut dir: i64) -> bool {
    for _ in 0..256 {
        if set.contains(&dir) {
            return true;
        }
        match index.parent.get(&dir) {
            Some(&parent) if parent != dir => dir = parent,
            _ => return false,
        }
    }
    false
}

fn dir_of(index: &RunIdx, id: i64) -> Option<i64> {
    let position = index.all.partition_point(|run| run.hi < id);
    index
        .all
        .get(position)
        .filter(|run| run.lo <= id)
        .map(|run| run.dir)
}

fn top_dirs(set: &HashSet<i64>, index: &RunIdx) -> Vec<i64> {
    set.iter()
        .copied()
        .filter(|&dir| {
            let mut cursor = dir;
            for _ in 0..256 {
                match index.parent.get(&cursor) {
                    Some(&parent) if parent != cursor => {
                        if set.contains(&parent) {
                            return false;
                        }
                        cursor = parent;
                    }
                    _ => return true,
                }
            }
            true
        })
        .collect()
}

fn subtree_dirs(set: &HashSet<i64>, index: &RunIdx) -> Vec<i64> {
    let mut out = Vec::new();
    let mut stack = top_dirs(set, index);
    while let Some(dir) = stack.pop() {
        out.push(dir);
        if let Some(children) = index.children.get(&dir) {
            stack.extend(children);
        }
    }
    out
}

fn subtree_files(index: &RunIdx, dir: i64) -> i64 {
    let mut count = 0;
    let mut stack = vec![dir];
    let mut seen = HashSet::new();
    while let Some(dir) = stack.pop() {
        if !seen.insert(dir) {
            continue;
        }
        if let Some(runs) = index.by_dir.get(&dir) {
            for &slot in runs {
                let run = index.all[slot];
                count += run.hi - run.lo + 1;
            }
        }
        if let Some(children) = index.children.get(&dir) {
            stack.extend(children);
        }
    }
    count
}

fn estimate(set: &HashSet<i64>, index: &RunIdx) -> i64 {
    top_dirs(set, index)
        .into_iter()
        .map(|dir| {
            let counted = index.tfc.get(&dir).copied().unwrap_or(0);
            if counted > 0 {
                counted
            } else {
                subtree_files(index, dir)
            }
        })
        .sum()
}

fn smallest_set(sets: &[HashSet<i64>], index: &RunIdx) -> Option<usize> {
    sets.iter()
        .enumerate()
        .filter(|(_, set)| !set.is_empty())
        .min_by_key(|(_, set)| estimate(set, index))
        .map(|(slot, _)| slot)
}

fn runs_for(index: &RunIdx, set: &HashSet<i64>) -> Vec<Run> {
    if set.is_empty() {
        return Vec::new();
    }
    if !restrict_to_subtree(estimate(set, index)) {
        return index.all.clone();
    }
    let mut runs = Vec::new();
    for dir in subtree_dirs(set, index) {
        if let Some(slots) = index.by_dir.get(&dir) {
            for &slot in slots {
                runs.push(index.all[slot]);
            }
        }
    }
    runs.sort_unstable_by_key(|run| run.lo);
    runs
}

fn segment_matches(conn: &Connection, match_expr: &str) -> Result<HashSet<i64>> {
    let mut statement = conn
        .prepare_cached("SELECT rowid FROM audio_dir_search WHERE audio_dir_search MATCH ?1")
        .map_err(db_error)?;
    statement
        .query_map(params![match_expr], |row| row.get(0))
        .map_err(db_error)?
        .collect::<rusqlite::Result<_>>()
        .map_err(db_error)
}

fn main_has_hit(conn: &Connection, match_expr: &str) -> Result<bool> {
    let mut statement = conn
        .prepare_cached("SELECT rowid FROM audio_search WHERE audio_search MATCH ?1 LIMIT 1")
        .map_err(db_error)?;
    statement.exists(params![match_expr]).map_err(db_error)
}

fn load_index(conn: &Connection, cache: &DirIndexCache) -> Result<Arc<RunIdx>> {
    let generation = meta_i64(conn, "dir_index_gen")?.unwrap_or(0);
    let mut guard = cache
        .inner
        .lock()
        .map_err(|_| Error::Database("Directory index cache lock was poisoned".into()))?;
    if let Some((cached, index)) = guard.as_ref()
        && *cached == generation
    {
        return Ok(Arc::clone(index));
    }
    let index = Arc::new(read_index(conn)?);
    *guard = Some((generation, Arc::clone(&index)));
    Ok(index)
}

fn read_index(conn: &Connection) -> Result<RunIdx> {
    let mut statement = conn
        .prepare("SELECT lo, hi, n, dir_id FROM audio_dir_runs ORDER BY lo")
        .map_err(db_error)?;
    let all = statement
        .query_map([], |row| {
            let (lo, hi, count): (i64, i64, i64) = (row.get(0)?, row.get(1)?, row.get(2)?);
            Ok(Run {
                lo,
                hi,
                dense: count == hi - lo + 1,
                dir: row.get(3)?,
            })
        })
        .map_err(db_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(db_error)?;
    let mut by_dir: HashMap<i64, Vec<usize>> = HashMap::new();
    for (slot, run) in all.iter().enumerate() {
        by_dir.entry(run.dir).or_default().push(slot);
    }
    let mut parent = HashMap::new();
    let mut children: HashMap<i64, Vec<i64>> = HashMap::new();
    let mut tfc = HashMap::new();
    let mut folders = conn
        .prepare("SELECT id, parent_id, COALESCE(total_file_count, 0) FROM folders")
        .map_err(db_error)?;
    let rows = folders
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(db_error)?;
    for row in rows {
        let (id, parent_id, files) = row.map_err(db_error)?;
        tfc.insert(id, files);
        if let Some(parent_id) = parent_id {
            parent.insert(id, parent_id);
            children.entry(parent_id).or_default().push(id);
        }
    }
    Ok(RunIdx {
        all,
        by_dir,
        parent,
        children,
        tfc,
    })
}

pub(crate) fn reindex_segment(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM audio_dir_search WHERE rowid=?1", [id])
        .map_err(db_error)?;
    conn.execute(
        "INSERT INTO audio_dir_search(rowid,path_tokens,path_tokens_tokenized)
         SELECT id,
          CASE WHEN parent_id IS NULL THEN '' ELSE t3_norm(COALESCE(folder_name,'')) END,
          CASE WHEN parent_id IS NULL THEN '' ELSE audiolib_tokenize(t3_norm(COALESCE(folder_name,''))) END
         FROM folders WHERE id=?1",
        [id],
    )
    .map_err(db_error)?;
    Ok(())
}

fn meta_i64(conn: &Connection, key: &str) -> Result<Option<i64>> {
    let value: Option<String> = conn
        .query_row(
            "SELECT value FROM library_meta WHERE key=?1",
            [key],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_error)?;
    value
        .map(|text| {
            text.parse().map_err(|_| {
                Error::Database(format!("Directory index metadata {key} is not an integer"))
            })
        })
        .transpose()
}

fn write_meta(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO library_meta(key,value) VALUES(?1,?2)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![key, value],
    )
    .map_err(db_error)?;
    Ok(())
}

fn base64url(bytes: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    let mut index = 0;
    while index + 3 <= bytes.len() {
        let block = u32::from(bytes[index]) << 16
            | u32::from(bytes[index + 1]) << 8
            | u32::from(bytes[index + 2]);
        out.push(TABLE[((block >> 18) & 63) as usize] as char);
        out.push(TABLE[((block >> 12) & 63) as usize] as char);
        out.push(TABLE[((block >> 6) & 63) as usize] as char);
        out.push(TABLE[(block & 63) as usize] as char);
        index += 3;
    }
    let rest = bytes.len() - index;
    if rest == 1 {
        let block = u32::from(bytes[index]) << 16;
        out.push(TABLE[((block >> 18) & 63) as usize] as char);
        out.push(TABLE[((block >> 12) & 63) as usize] as char);
    } else if rest == 2 {
        let block = u32::from(bytes[index]) << 16 | u32::from(bytes[index + 1]) << 8;
        out.push(TABLE[((block >> 18) & 63) as usize] as char);
        out.push(TABLE[((block >> 12) & 63) as usize] as char);
        out.push(TABLE[((block >> 6) & 63) as usize] as char);
    }
    out
}

fn base64url_decode(value: &str) -> std::result::Result<Vec<u8>, ()> {
    fn value_of(byte: u8) -> std::result::Result<u8, ()> {
        Ok(match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return Err(()),
        })
    }
    let bytes = value.as_bytes();
    if bytes.len() % 4 == 1 {
        return Err(());
    }
    let mut out = Vec::new();
    let mut index = 0;
    while index + 4 <= bytes.len() {
        let block = (u32::from(value_of(bytes[index])?) << 18)
            | (u32::from(value_of(bytes[index + 1])?) << 12)
            | (u32::from(value_of(bytes[index + 2])?) << 6)
            | u32::from(value_of(bytes[index + 3])?);
        out.push((block >> 16) as u8);
        out.push((block >> 8) as u8);
        out.push(block as u8);
        index += 4;
    }
    let rest = bytes.len() - index;
    if rest == 2 {
        let block = (u32::from(value_of(bytes[index])?) << 18)
            | (u32::from(value_of(bytes[index + 1])?) << 12);
        out.push((block >> 16) as u8);
    } else if rest == 3 {
        let block = (u32::from(value_of(bytes[index])?) << 18)
            | (u32::from(value_of(bytes[index + 1])?) << 12)
            | (u32::from(value_of(bytes[index + 2])?) << 6);
        out.push((block >> 16) as u8);
        out.push((block >> 8) as u8);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DataProvider, SqliteProvider};
    use audiolib_core::SearchRequest;
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn normalization_folds_fullwidth_and_keeps_halfwidth() {
        assert_eq!(t3_norm("Ｋｉｔ"), "Kit");
        assert_eq!(t3_norm("Kit"), "Kit");
        assert_eq!(t3_norm("Ａ　Ｂ"), "A B");
        assert!(path_set_too_wide(3, 4));
        assert!(!path_set_too_wide(1, 4));
        assert!(restrict_to_subtree(50_000));
        assert!(!restrict_to_subtree(50_001));
    }

    #[test]
    fn cursor_roundtrip_keeps_tier_score_and_rev() {
        let encoded = encode_cursor(1, 1.0, 42, SEARCH_PATH_REV);
        let cursor = decode_cursor(&encoded).unwrap();
        assert_eq!(cursor.tier, 1);
        assert_eq!(cursor.id, 42);
        assert_eq!(cursor.rev, SEARCH_PATH_REV);
        assert!(decode_cursor("not-a-cursor").is_err());
    }

    fn library() -> (tempfile::TempDir, SqliteProvider) {
        let dir = tempfile::tempdir().unwrap();
        let provider = SqliteProvider::open(&dir.path().join("library.db")).unwrap();
        provider
            .writer()
            .unwrap()
            .execute_batch(
                "INSERT INTO folders(id,folder_path,folder_name,parent_id,total_file_count) VALUES
                 (1,'/lib','LibraryRoot',NULL,5),
                 (2,'/lib/Kit','Kit',1,2),
                 (3,'/lib/Kit/Explosion','Explosion',2,1),
                 (4,'/lib/Other','Other',1,1),
                 (5,'/lib/Doors','Doors',1,1);
                 INSERT INTO folder_closure(ancestor_id,descendant_id,depth) VALUES
                 (1,1,0),(2,2,0),(3,3,0),(4,4,0),(5,5,0),(1,2,1),(1,3,2),(2,3,1),(1,4,1),(1,5,1);
                 INSERT INTO audio_files(id,file_path,filename,folder_id,missing) VALUES
                 (9,'/lib/explosion kit.wav','explosion kit.wav',1,0),
                 (10,'/lib/Kit/explosion.wav','explosion.wav',2,0),
                 (11,'/lib/Kit/Explosion/quiet.wav','quiet.wav',3,0),
                 (12,'/lib/Other/explosion_b.wav','explosion_b.wav',4,0),
                 (20,'/lib/kit explosion.wav','kit explosion.wav',1,0),
                 (30,'/lib/Doors/hinge.wav','hinge.wav',5,0);",
            )
            .unwrap();
        provider.rebuild_directory_search().unwrap();
        let format: String = provider
            .writer()
            .unwrap()
            .query_row(
                "SELECT value FROM library_meta WHERE key='snapshot_format'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(format, audiolib_core::compatibility::PLAN_B_V9_FORMAT);
        assert!(format.ends_with("-sd1"));
        (dir, provider)
    }

    fn ids(provider: &SqliteProvider, query: &str, scope: SearchScope) -> Vec<i64> {
        provider
            .search(&SearchRequest {
                query: query.into(),
                scope,
                limit: 20,
                ..Default::default()
            })
            .unwrap()
            .results
            .into_iter()
            .map(|file| file.id)
            .collect()
    }

    #[test]
    fn layered_results_keep_s8_prefix_then_path_hits_by_id() {
        let (_dir, provider) = library();
        let main = ids(&provider, "explosion Kit", SearchScope::Main);
        let all = ids(&provider, "explosion Kit", SearchScope::All);
        assert!(!main.is_empty(), "main query should hit filenames");
        assert_eq!(&all[..main.len()], main.as_slice());
        assert_eq!(&all[main.len()..], &[10, 11]);
        let path = ids(&provider, "Kit", SearchScope::Path);
        assert_eq!(path, vec![10, 11]);
        assert!(ids(&provider, "LibraryRoot", SearchScope::Path).is_empty());
        assert!(ids(&provider, "LibraryRoot", SearchScope::All).is_empty());
    }

    #[test]
    fn empty_path_term_without_main_hit_returns_empty_without_full_scan() {
        let (_dir, provider) = library();
        assert!(!ids(&provider, "explosion", SearchScope::All).is_empty());
        let conn = provider.reader().unwrap();
        let page = search(&DirSearch {
            conn: &conn,
            cache: &provider.dir_index,
            jieba: &provider.tokenizer,
            query: "zzznopath explosion",
            scope: SearchScope::All,
            visibility: FileVisibility::Present,
            cursor: None,
            offset: 0,
            limit: 20,
        })
        .unwrap();
        assert!(page.short_circuited);
        assert!(page.hits.is_empty());
        assert!(!page.has_more);
        let via_api = provider
            .search(&SearchRequest {
                query: "zzznopath explosion".into(),
                scope: SearchScope::All,
                ..Default::default()
            })
            .unwrap();
        assert!(via_api.results.is_empty());
        assert_eq!(via_api.total_count, 0);
        assert!(via_api.next_cursor.is_none());
    }

    #[test]
    fn keyset_cursor_pages_tiers_without_duplicates() {
        let (_dir, provider) = library();
        let full = ids(&provider, "explosion Kit", SearchScope::All);
        let mut seen = Vec::new();
        let mut cursor = None;
        for _ in 0..8 {
            let page = provider
                .search(&SearchRequest {
                    query: "explosion Kit".into(),
                    scope: SearchScope::All,
                    limit: 1,
                    cursor: cursor.clone(),
                    // A cursor replaces offset. The first page has no cursor.
                    offset: if cursor.is_some() { 50 } else { 0 },
                    ..Default::default()
                })
                .unwrap();
            if page.results.is_empty() {
                assert!(page.next_cursor.is_none());
                break;
            }
            assert_eq!(page.results.len(), 1);
            seen.push(page.results[0].id);
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        assert_eq!(seen, full);
        assert!(cursor.is_none());
    }

    #[test]
    fn stale_cursor_is_rejected() {
        let (_dir, provider) = library();
        let error = provider
            .search(&SearchRequest {
                query: "Kit".into(),
                scope: SearchScope::Path,
                cursor: Some(encode_cursor(2, 2.0, 1, SEARCH_PATH_REV + 1)),
                ..Default::default()
            })
            .unwrap_err();
        assert!(error.to_string().contains("CURSOR_STALE"), "{error}");
    }

    #[test]
    fn wide_directory_set_is_dropped_and_rename_updates_one_segment() {
        let dir = tempfile::tempdir().unwrap();
        let provider = SqliteProvider::open(&dir.path().join("library.db")).unwrap();
        provider
            .writer()
            .unwrap()
            .execute_batch(
                "INSERT INTO folders(id,folder_path,folder_name,parent_id) VALUES
                 (1,'/lib','LibraryRoot',NULL),
                 (2,'/lib/Wide','Wide',1),
                 (3,'/lib/Other','Other',1);
                 INSERT INTO audio_files(id,file_path,filename,folder_id,missing) VALUES
                 (1,'/lib/Wide/a.wav','a.wav',2,0),
                 (2,'/lib/Wide/b.wav','b.wav',2,0),
                 (3,'/lib/Wide/c.wav','c.wav',2,0),
                 (4,'/lib/Other/d.wav','d.wav',3,0);",
            )
            .unwrap();
        provider.rebuild_directory_search().unwrap();
        assert!(ids(&provider, "Wide", SearchScope::Path).is_empty());

        let (_dir, provider) = library();
        {
            let writer = provider.writer().unwrap();
            writer
                .execute("UPDATE folders SET folder_name='Metalbox' WHERE id=2", [])
                .unwrap();
            maintain(&writer, &[2], &[]).unwrap();
        }
        // A name that still begins with Kit would stay a letter-fragment hit.
        assert!(ids(&provider, "Kit", SearchScope::Path).is_empty());
        assert_eq!(ids(&provider, "Metalbox", SearchScope::Path), vec![10, 11]);
    }

    #[test]
    fn main_scope_ignores_directory_names() {
        let (_dir, provider) = library();
        assert!(ids(&provider, "Doors", SearchScope::Main).is_empty());
        assert_eq!(ids(&provider, "Doors", SearchScope::Path), vec![30]);
        let main = ids(&provider, "explosion", SearchScope::Main);
        let all = ids(&provider, "explosion", SearchScope::All);
        assert_eq!(&all[..main.len()], main.as_slice());
        assert!(all.contains(&11));
        assert!(!main.contains(&11));
    }

    fn overlapping_library() -> (tempfile::TempDir, SqliteProvider) {
        let dir = tempfile::tempdir().unwrap();
        let provider = SqliteProvider::open(&dir.path().join("library.db")).unwrap();
        provider
            .writer()
            .unwrap()
            .execute_batch(
                "INSERT INTO folders(id,folder_path,folder_name,parent_id,total_file_count) VALUES
                 (1,'/lib','LibraryRoot',NULL,12),
                 (2,'/lib/Borg','Borg',1,5),
                 (3,'/lib/Borg/Sound','Sound',2,3),
                 (4,'/lib/Other','Other',1,6),
                 (5,'/lib/Sound','Sound',1,1);
                 INSERT INTO audio_files(id,file_path,filename,folder_id,missing) VALUES
                 (2,'/lib/Borg/Sound/borg sound.wav','borg sound.wav',3,0),
                 (4,'/lib/Borg/Sound/quiet.wav','quiet.wav',3,1),
                 (8,'/lib/Borg/Sound/quiet2.wav','quiet2.wav',3,0),
                 (10,'/lib/Borg/sound.wav','sound.wav',2,0),
                 (12,'/lib/Borg/borg sound.wav','borg sound.wav',2,0),
                 (14,'/lib/Sound/borg.wav','borg.wav',5,0);
                 WITH RECURSIVE n(id) AS (VALUES(20) UNION ALL SELECT id+1 FROM n WHERE id<25)
                 INSERT INTO audio_files(id,file_path,filename,folder_id,missing)
                 SELECT id,'/lib/Other/'||id||'.wav','quiet.wav',4,0 FROM n;",
            )
            .unwrap();
        provider.rebuild_directory_search().unwrap();
        (dir, provider)
    }

    #[test]
    fn overlapping_path_terms_keep_mixed_main_and_sparse_pages_distinct() {
        let (_dir, provider) = overlapping_library();
        let main = ids(&provider, "Borg Sound", SearchScope::Main);
        assert_eq!(main, vec![2, 12]);
        let all = ids(&provider, "Borg Sound", SearchScope::All);
        assert_eq!(&all[..main.len()], main.as_slice());
        assert_eq!(&all[main.len()..], &[10, 14, 8]);
        // Full path hits remain in path scope even when main also matches (2).
        // A partial path hit already satisfied by main stays excluded (12).
        assert_eq!(
            ids(&provider, "Borg Sound", SearchScope::Path),
            vec![10, 14, 2, 8]
        );

        for scope in [SearchScope::Path, SearchScope::All] {
            let full = ids(&provider, "Borg Sound", scope);
            let mut cursor = None;
            let mut paged = Vec::new();
            for (page_index, expected) in full.chunks(2).enumerate() {
                let request = SearchRequest {
                    query: "Borg Sound".into(),
                    scope,
                    limit: 2,
                    ..Default::default()
                };
                let offset_page = provider
                    .search(&SearchRequest {
                        offset: page_index * 2,
                        ..request.clone()
                    })
                    .unwrap();
                let cursor_page = provider
                    .search(&SearchRequest {
                        offset: if cursor.is_some() { 999 } else { 0 },
                        cursor: cursor.clone(),
                        ..request
                    })
                    .unwrap();
                let returned = cursor_page
                    .results
                    .iter()
                    .map(|file| file.id)
                    .collect::<Vec<_>>();
                assert_eq!(returned, expected);
                assert_eq!(
                    offset_page
                        .results
                        .iter()
                        .map(|file| file.id)
                        .collect::<Vec<_>>(),
                    returned
                );
                let remaining = full.len() - page_index * 2;
                assert_eq!(cursor_page.total_count, remaining.min(3) as u64);
                assert_eq!(
                    offset_page.total_count,
                    (page_index * 2 + remaining.min(3)) as u64
                );
                paged.extend(returned);
                cursor = cursor_page.next_cursor;
                assert_eq!(cursor.is_some(), remaining > 2);
            }
            assert_eq!(paged, full);
            let beyond_end = provider
                .search(&SearchRequest {
                    query: "Borg Sound".into(),
                    scope,
                    offset: 999,
                    limit: 2,
                    ..Default::default()
                })
                .unwrap();
            assert!(beyond_end.results.is_empty());
            assert_eq!(beyond_end.total_count, full.len() as u64);
            assert!(beyond_end.next_cursor.is_none());
        }

        for (visibility, expected) in [
            (FileVisibility::Missing, vec![4]),
            (FileVisibility::All, vec![10, 14, 2, 4, 8]),
        ] {
            let mut request = SearchRequest {
                query: "Borg Sound".into(),
                scope: SearchScope::Path,
                ..Default::default()
            };
            request.filters.visibility = visibility;
            let page = provider.search(&request).unwrap();
            assert_eq!(
                page.results.iter().map(|file| file.id).collect::<Vec<_>>(),
                expected
            );
        }
    }

    fn directory_first_page_vm_work(files: usize, partial: bool) -> usize {
        let dir = tempfile::tempdir().unwrap();
        let provider = SqliteProvider::open(&dir.path().join("library.db")).unwrap();
        {
            let writer = provider.writer().unwrap();
            writer
                .execute_batch(
                    "INSERT INTO folders(id,folder_path,folder_name,parent_id) VALUES
                     (1,'/lib','LibraryRoot',NULL),
                     (2,'/lib/Borg Sound','Borg Sound',1),
                     (3,'/lib/Other','Other',1);",
                )
                .unwrap();
            writer
                .execute(
                    "WITH RECURSIVE n(id) AS (VALUES(1) UNION ALL SELECT id+1 FROM n WHERE id<?1)
                     INSERT INTO audio_files(id,file_path,filename,folder_id,missing)
                     SELECT id*2,'/lib/Borg Sound/'||id||'.wav','quiet.wav',2,0 FROM n
                     UNION ALL
                     SELECT ?1*2+id,'/lib/Other/'||id||'.wav','quiet.wav',3,0 FROM n",
                    [files as i64],
                )
                .unwrap();
            writer
                .execute(
                    "UPDATE folders SET total_file_count=CASE WHEN id=1 THEN ?1*2 ELSE ?1 END",
                    [files as i64],
                )
                .unwrap();
            if partial {
                writer
                    .execute_batch(
                        "UPDATE folders SET folder_name='Borg' WHERE id=2;
                         UPDATE folders SET folder_name='Sound' WHERE id=3;
                         UPDATE audio_files SET filename='sound.wav' WHERE id=2;
                         UPDATE audio_files SET filename='borg.wav'
                         WHERE id=(SELECT min(id) FROM audio_files WHERE folder_id=3);",
                    )
                    .unwrap();
            }
        }
        provider.rebuild_directory_search().unwrap();
        let conn = provider.reader().unwrap();
        // Separate one-time RunIdx loading from page work. The path run is sparse
        // and visibility is checked in SQLite, so expanding the whole subtree
        // produces VM work proportional to its size even without main probes.
        load_index(&conn, &provider.dir_index).unwrap();
        conn.authorizer(Some(move |context: AuthContext<'_>| {
            if matches!(context.action, AuthAction::Read { table_name, .. }
                if table_name == "audio_search" && !partial)
            {
                Authorization::Deny
            } else {
                Authorization::Allow
            }
        }))
        .unwrap();
        let ticks = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&ticks);
        conn.progress_handler(
            100,
            Some(move || {
                observed.fetch_add(1, Ordering::Relaxed);
                false
            }),
        )
        .unwrap();
        let result = search(&DirSearch {
            conn: &conn,
            cache: &provider.dir_index,
            jieba: &provider.tokenizer,
            query: "Borg Sound",
            scope: SearchScope::Path,
            visibility: FileVisibility::Present,
            cursor: None,
            offset: 0,
            limit: 5,
        });
        conn.progress_handler(0, None::<fn() -> bool>).unwrap();
        conn.authorizer(None::<fn(AuthContext<'_>) -> Authorization>)
            .unwrap();
        let page = result.unwrap();
        let expected = if partial {
            vec![2, files as i64 * 2 + 1]
        } else {
            vec![2, 4, 6, 8, 10]
        };
        assert_eq!(
            page.hits.iter().map(|hit| hit.id).collect::<Vec<_>>(),
            expected
        );
        assert_eq!(page.observed_count, if partial { 2 } else { 6 });
        assert_eq!(page.has_more, !partial);
        ticks.load(Ordering::Relaxed)
    }

    #[test]
    fn all_path_first_page_does_not_scan_subtree_or_probe_main() {
        let small = directory_first_page_vm_work(512, false);
        let large = directory_first_page_vm_work(8192, false);
        assert!(small > 0);
        // The same five-row page may have small B-tree-depth overhead, but 16x
        // more subtree rows must not cause an exhaustive tier-1 file scan.
        assert!(
            large <= small * 2,
            "page VM callbacks grew from {small} to {large}"
        );
    }

    #[test]
    fn sparse_mixed_matches_do_not_restart_main_match_for_each_path_file() {
        let small = directory_first_page_vm_work(512, true);
        let large = directory_first_page_vm_work(8192, true);
        assert!(small > 0);
        assert!(
            large <= small * 2,
            "mixed page VM callbacks grew from {small} to {large}"
        );
    }

    #[test]
    fn mixed_stream_requires_every_missing_term_and_excludes_full_main() {
        let (_dir, provider) = overlapping_library();
        provider
            .writer()
            .unwrap()
            .execute_batch(
                "UPDATE audio_files SET filename='sound indoor.wav' WHERE id=10;
             UPDATE audio_files SET filename='borg indoor.wav' WHERE id=14;
             UPDATE audio_files SET filename='borg sound indoor.wav' WHERE id=2;
             INSERT INTO audio_files(id,file_path,filename,folder_id,missing) VALUES
             (11,'/lib/folderless-gap.wav','sound indoor.wav',NULL,0),
             (30,'/lib/folderless.wav','borg sound indoor.wav',NULL,0);",
            )
            .unwrap();
        provider.rebuild_directory_search().unwrap();
        assert_eq!(
            ids(&provider, "Borg Sound Indoor", SearchScope::Path),
            vec![10, 14]
        );
        let all = ids(&provider, "Borg Sound Indoor", SearchScope::All);
        let main = ids(&provider, "Borg Sound Indoor", SearchScope::Main);
        assert_eq!(&all[..main.len()], main.as_slice());
        assert_eq!(&all[main.len()..], &[10, 14]);
    }

    #[test]
    fn mixed_posting_batches_and_full_main_exclusions_survive_cursor_boundaries() {
        let (_dir, provider) = overlapping_library();
        provider
            .writer()
            .unwrap()
            .execute_batch(
                "WITH RECURSIVE n(id) AS (VALUES(100) UNION ALL SELECT id+1 FROM n WHERE id<699)
             INSERT INTO audio_files(id,file_path,filename,folder_id,missing)
             SELECT id,'/lib/Borg/'||id||'.wav',
                    CASE WHEN id%10=0 THEN 'borg sound.wav' ELSE 'sound.wav' END,2,0 FROM n;",
            )
            .unwrap();
        provider.rebuild_directory_search().unwrap();
        for scope in [SearchScope::Path, SearchScope::All] {
            let request = SearchRequest {
                query: "Borg Sound".into(),
                scope,
                limit: 1000,
                ..Default::default()
            };
            let full = provider
                .search(&request)
                .unwrap()
                .results
                .into_iter()
                .map(|file| file.id)
                .collect::<Vec<_>>();
            if scope == SearchScope::Path {
                let expected = [10, 14]
                    .into_iter()
                    .chain((100..700).filter(|id| id % 10 != 0))
                    .chain([2, 8])
                    .collect::<Vec<_>>();
                assert_eq!(full, expected);
            }
            let mut cursor = None;
            let mut paged = Vec::new();
            loop {
                let page = provider
                    .search(&SearchRequest {
                        limit: 137,
                        cursor,
                        ..request.clone()
                    })
                    .unwrap();
                paged.extend(page.results.into_iter().map(|file| file.id));
                cursor = page.next_cursor;
                if cursor.is_none() {
                    break;
                }
            }
            assert_eq!(paged, full);
        }
    }

    #[test]
    fn full_path_sparse_runs_do_not_assign_folderless_gaps_to_the_directory() {
        let (_dir, provider) = overlapping_library();
        provider
            .writer()
            .unwrap()
            .execute_batch(
                "INSERT INTO audio_files(id,file_path,filename,folder_id,missing) VALUES
                 (3,'/lib/folderless-missing.wav','quiet.wav',NULL,1),
                 (6,'/lib/folderless-main.wav','borg sound.wav',NULL,0);",
            )
            .unwrap();
        provider.rebuild_directory_search().unwrap();
        assert_eq!(
            ids(&provider, "Borg Sound", SearchScope::Path),
            vec![10, 14, 2, 8]
        );
        let main = ids(&provider, "Borg Sound", SearchScope::Main);
        assert!(main.contains(&6));
        let all = ids(&provider, "Borg Sound", SearchScope::All);
        assert_eq!(&all[..main.len()], main.as_slice());
        assert_eq!(&all[main.len()..], &[10, 14, 8]);
        for (visibility, expected) in [
            (FileVisibility::Missing, vec![4]),
            (FileVisibility::All, vec![10, 14, 2, 4, 8]),
        ] {
            let mut request = SearchRequest {
                query: "Borg Sound".into(),
                scope: SearchScope::Path,
                ..Default::default()
            };
            request.filters.visibility = visibility;
            let page = provider.search(&request).unwrap();
            assert_eq!(
                page.results.iter().map(|file| file.id).collect::<Vec<_>>(),
                expected
            );
        }
    }
}
