// crates/audiolib-db/src/search_rank_parallel.rs
//! Bounded exact ranking across readers verified to share the master's snapshot.
//! Phrase statistics and document lengths are prepared concurrently; workers
//! share only owned plans after their pinned snapshots have been verified.
use crate::{Error, Result, SqliteProvider, db_error, search_bm25, search_cache};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    path::Path,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

const BUDGET_BATCH: usize = 1024;
/// Maximum extra snapshot readers used by one provider's active ranking job.
pub(crate) const AUXILIARY_READERS_MAX: usize = crate::SQLITE_AUXILIARY_SEARCH_READERS;
pub(crate) type SplitPoints = [i64; AUXILIARY_READERS_MAX];
pub(crate) type RankedRows = Vec<(i64, f64)>;

/// Observer identity from the master's already-pinned transaction.
/// The caller retains its read gate until collection and every worker have drained.
pub(crate) struct Snapshot<'a> {
    pub revision: u64,
    pub cache_eligible: bool,
    pub cancel: Option<&'a Arc<AtomicBool>>,
}

struct Reservation<'a>(&'a AtomicBool);

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

#[derive(Default)]
struct StartGate {
    run: Mutex<Option<bool>>,
    wake: Condvar,
}

impl StartGate {
    fn decide(&self, run: bool) {
        *self.run.lock().unwrap_or_else(|error| error.into_inner()) = Some(run);
        self.wake.notify_all();
    }

    fn wait(&self, shared: &Shared) -> bool {
        let mut decision = self.run.lock().unwrap_or_else(|error| error.into_inner());
        while decision.is_none() && !shared.stopped() {
            let (next, _) = self
                .wake
                .wait_timeout(decision, Duration::from_millis(20))
                .unwrap_or_else(|error| error.into_inner());
            decision = next;
        }
        *decision == Some(true) && !shared.stopped()
    }
}

struct Shared {
    external: Arc<AtomicBool>,
    abort: AtomicBool,
    overflow: AtomicBool,
    reserved: AtomicUsize,
}

impl Shared {
    fn stopped(&self) -> bool {
        self.external.load(Ordering::Acquire) || self.abort.load(Ordering::Acquire)
    }

    fn claim(&self, remaining: &mut usize) -> bool {
        if *remaining == 0 {
            match self
                .reserved
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                    (used < search_cache::MAX_RANKED_IDS)
                        .then(|| used + (search_cache::MAX_RANKED_IDS - used).min(BUDGET_BATCH))
                }) {
                Ok(previous) => {
                    *remaining = (search_cache::MAX_RANKED_IDS - previous).min(BUDGET_BATCH);
                }
                Err(_) => {
                    self.overflow.store(true, Ordering::Release);
                    self.abort.store(true, Ordering::Release);
                    return false;
                }
            }
        }
        *remaining -= 1;
        true
    }
}

#[derive(Debug)]
enum WorkerFailure {
    // An auxiliary reader could not start, or was cancelled by local cleanup.
    // The original cursor remains valid and can finish the complete serial read.
    Unavailable,
    // SQL/data errors are never converted into a successful partial ranking.
    Fatal(Error),
}

type WorkerResult<T> = std::result::Result<T, WorkerFailure>;

fn prepare_error(error: rusqlite::Error) -> WorkerFailure {
    if matches!(&error, rusqlite::Error::SqliteFailure(code, _) if matches!(
        code.code,
        rusqlite::ErrorCode::DatabaseBusy
            | rusqlite::ErrorCode::DatabaseLocked
            | rusqlite::ErrorCode::CannotOpen
            | rusqlite::ErrorCode::OperationInterrupted
    )) {
        WorkerFailure::Unavailable
    } else {
        WorkerFailure::Fatal(db_error(error))
    }
}

struct Parallel<'a> {
    boundary: i64,
    shared: Arc<Shared>,
    prepare_gate: Arc<StartGate>,
    score_gate: Arc<StartGate>,
    prepared: mpsc::Receiver<search_bm25::PartialStats>,
    workers: Vec<JoinHandle<WorkerResult<RankedRows>>>,
    _reservation: Reservation<'a>,
}

impl Parallel<'_> {
    fn partials(
        &mut self,
        cancel: Option<&Arc<AtomicBool>>,
    ) -> Result<Option<Vec<search_bm25::PartialStats>>> {
        let mut partials = Vec::with_capacity(AUXILIARY_READERS_MAX);
        while partials.len() < AUXILIARY_READERS_MAX {
            check_cancel(cancel)?;
            if self.shared.stopped() {
                self.stop()?;
                return Ok(None);
            }
            match self.prepared.recv_timeout(Duration::from_millis(20)) {
                Ok(partial) => partials.push(partial),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.stop()?;
                    return Ok(None);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
        Ok(Some(partials))
    }

    fn finish(&mut self) -> WorkerResult<Vec<RankedRows>> {
        let mut parts = Vec::with_capacity(self.workers.len());
        let mut unavailable = false;
        let mut fatal = None;
        for worker in self.workers.drain(..) {
            match worker.join() {
                Ok(Ok(rows)) => parts.push(rows),
                Ok(Err(WorkerFailure::Unavailable)) => {
                    self.shared.abort.store(true, Ordering::Release);
                    unavailable = true;
                }
                Ok(Err(WorkerFailure::Fatal(error))) => {
                    self.shared.abort.store(true, Ordering::Release);
                    fatal.get_or_insert(error);
                }
                Err(_) => {
                    self.shared.abort.store(true, Ordering::Release);
                    fatal.get_or_insert_with(|| {
                        Error::Database("Parallel ranking worker panicked".into())
                    });
                }
            }
        }
        if let Some(error) = fatal {
            Err(WorkerFailure::Fatal(error))
        } else if unavailable {
            Err(WorkerFailure::Unavailable)
        } else {
            Ok(parts)
        }
    }

    fn stop(&mut self) -> Result<()> {
        self.shared.abort.store(true, Ordering::Release);
        self.prepare_gate.decide(false);
        self.score_gate.decide(false);
        match self.finish() {
            Err(WorkerFailure::Fatal(error)) => Err(error),
            _ => Ok(()),
        }
    }
}

impl Drop for Parallel<'_> {
    fn drop(&mut self) {
        self.shared.abort.store(true, Ordering::Release);
        self.prepare_gate.decide(false);
        self.score_gate.decide(false);
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

fn check_cancel(cancel: Option<&Arc<AtomicBool>>) -> Result<()> {
    if cancel.is_some_and(|flag| flag.load(Ordering::Acquire)) {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

fn worker_connection(path: &Path, shared: Arc<Shared>) -> WorkerResult<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(prepare_error)?;
    conn.busy_timeout(Duration::from_millis(50))
        .map_err(prepare_error)?;
    conn.progress_handler(2000, Some(move || shared.stopped()))
        .map_err(prepare_error)?;
    conn.pragma_update(None, "mmap_size", crate::SQLITE_READER_MMAP_BYTES)
        .map_err(prepare_error)?;
    conn.execute_batch(
        "PRAGMA query_only=ON; PRAGMA cache_size=-65536;
         PRAGMA temp_store=MEMORY; BEGIN;",
    )
    .map_err(prepare_error)?;
    conn.query_row("SELECT id FROM audio_files LIMIT 1", [], |row| {
        row.get::<_, i64>(0)
    })
    .optional()
    .map_err(prepare_error)?;
    Ok(conn)
}

fn plan_error(error: Error, shared: &Shared) -> WorkerFailure {
    if matches!(&error, Error::Busy(_)) || (shared.stopped() && matches!(&error, Error::Cancelled))
    {
        WorkerFailure::Unavailable
    } else {
        WorkerFailure::Fatal(error)
    }
}

struct WorkerPlan {
    plan: Arc<search_bm25::Prepared>,
    index: usize,
    phrases: Vec<usize>,
    // Production always obtains cuts from the completed plan. Tiny fixtures
    // override them to exercise the same score protocol without 262k test rows.
    fixture_cuts: Option<SplitPoints>,
}

struct WorkerGates {
    shared: Arc<Shared>,
    prepare: Arc<StartGate>,
    score: Arc<StartGate>,
    ready: mpsc::SyncSender<()>,
    prepared: mpsc::SyncSender<search_bm25::PartialStats>,
}

fn rank_worker(path: &Path, work: WorkerPlan, gates: &WorkerGates) -> WorkerResult<RankedRows> {
    let WorkerPlan {
        plan,
        index,
        phrases,
        fixture_cuts,
    } = work;
    let shared = &gates.shared;
    let conn = worker_connection(path, shared.clone())?;
    if gates.ready.send(()).is_err() || !gates.prepare.wait(shared) {
        return Err(WorkerFailure::Unavailable);
    }
    // All auxiliary read transactions are pinned before this gate opens. Each
    // phrase's global DF is computed exactly once, without document-size work.
    let partial = search_bm25::prepare_phrases(&conn, &plan, &phrases)
        .map_err(|error| plan_error(error, shared))?;
    if gates.prepared.send(partial).is_err() {
        return Err(WorkerFailure::Unavailable);
    }
    if !gates.score.wait(shared) {
        // A rare query can use these complete statistics on the root alone.
        // This clean decision differs from cancellation or a failed worker.
        return if shared.stopped() {
            Err(WorkerFailure::Unavailable)
        } else {
            Ok(Vec::new())
        };
    }
    let cuts = plan.split_points().or(fixture_cuts).ok_or_else(|| {
        WorkerFailure::Fatal(Error::Database(
            "Parallel ranking split keys are missing".into(),
        ))
    })?;
    let lower = cuts[index];
    let upper = cuts.get(index + 1).copied();
    let _registration = search_bm25::register_prepared(&conn, plan.clone())
        .map_err(|error| plan_error(error, shared))?;
    // Only the row-ID bounds differ. All cursors bind the complete MATCH and
    // reuse the exact global statistics published before the second gate.
    let sql = if upper.is_some() {
        "SELECT rowid,rank FROM audio_search WHERE audio_search MATCH ?1
         AND rank MATCH 'audiolib_bm25()' AND rowid>=?2 AND rowid<?3 ORDER BY rowid ASC"
    } else {
        "SELECT rowid,rank FROM audio_search WHERE audio_search MATCH ?1
         AND rank MATCH 'audiolib_bm25()' AND rowid>=?2 ORDER BY rowid ASC"
    };
    let mut statement = conn.prepare(sql).map_err(prepare_error)?;
    let mut rows = if let Some(upper) = upper {
        statement.query(params![plan.query(), lower, upper])
    } else {
        statement.query(params![plan.query(), lower])
    }
    .map_err(prepare_error)?;
    let mut ranked = Vec::new();
    let mut remaining = 0;
    loop {
        let row = rows.next().map_err(|error| {
            if shared.stopped()
                && matches!(&error, rusqlite::Error::SqliteFailure(code, _)
                    if code.code == rusqlite::ErrorCode::OperationInterrupted)
            {
                WorkerFailure::Unavailable
            } else {
                WorkerFailure::Fatal(db_error(error))
            }
        })?;
        let Some(row) = row else { break };
        if ranked.len().is_multiple_of(BUDGET_BATCH) && shared.stopped() {
            return Err(WorkerFailure::Unavailable);
        }
        if !shared.claim(&mut remaining) {
            return Err(WorkerFailure::Unavailable);
        }
        ranked.push((
            row.get(0)
                .map_err(|error| WorkerFailure::Fatal(db_error(error)))?,
            row.get(1)
                .map_err(|error| WorkerFailure::Fatal(db_error(error)))?,
        ));
    }
    Ok(ranked)
}

fn run_worker(path: &Path, work: WorkerPlan, gates: &WorkerGates) -> WorkerResult<RankedRows> {
    let result =
        catch_unwind(AssertUnwindSafe(|| rank_worker(path, work, gates))).unwrap_or_else(|_| {
            Err(WorkerFailure::Fatal(Error::Database(
                "Parallel ranking worker panicked".into(),
            )))
        });
    if result.is_err() {
        gates.shared.abort.store(true, Ordering::Release);
        // Each worker may publish at most one ready and one preparation result.
        // A second failure marker could fill either bounded channel ahead of
        // another worker's first message and deadlock cleanup's join. The abort
        // flag wakes channel receivers and either gate within their timeouts.
    }
    result
}

fn try_start<'a>(
    provider: &'a SqliteProvider,
    plan: Arc<search_bm25::Prepared>,
    revision: u64,
    cancel: Option<&Arc<AtomicBool>>,
    fixture_cuts: Option<SplitPoints>,
) -> Result<Option<Parallel<'a>>> {
    check_cancel(cancel)?;
    if !plan.has_layout()
        || provider
            .parallel_rank_reserved
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
    {
        return Ok(None);
    }
    let reservation = Reservation(&provider.parallel_rank_reserved);
    let shared = Arc::new(Shared {
        external: cancel
            .cloned()
            .unwrap_or_else(|| Arc::new(AtomicBool::new(false))),
        abort: AtomicBool::new(false),
        overflow: AtomicBool::new(false),
        reserved: AtomicUsize::new(0),
    });
    let (prepared, receiving) = mpsc::sync_channel(AUXILIARY_READERS_MAX);
    let mut parallel = Parallel {
        boundary: 0,
        shared,
        prepare_gate: Arc::new(StartGate::default()),
        score_gate: Arc::new(StartGate::default()),
        prepared: receiving,
        workers: Vec::with_capacity(AUXILIARY_READERS_MAX),
        _reservation: reservation,
    };
    let (ready, waiting) = mpsc::sync_channel(AUXILIARY_READERS_MAX);
    let groups = plan.phrase_groups();
    if groups.len() != AUXILIARY_READERS_MAX {
        return Err(Error::Database(
            "Parallel phrase groups are incomplete".into(),
        ));
    }
    for (index, phrases) in groups.into_iter().enumerate() {
        let path = provider.path.clone();
        let work = WorkerPlan {
            plan: plan.clone(),
            index,
            phrases,
            fixture_cuts,
        };
        let gates = WorkerGates {
            shared: parallel.shared.clone(),
            prepare: parallel.prepare_gate.clone(),
            score: parallel.score_gate.clone(),
            ready: ready.clone(),
            prepared: prepared.clone(),
        };
        let spawned = thread::Builder::new()
            .name(format!("audiolib-rank-{index}"))
            .spawn(move || run_worker(&path, work, &gates));
        match spawned {
            Ok(worker) => parallel.workers.push(worker),
            Err(_) => {
                parallel.stop()?;
                return Ok(None);
            }
        }
    }
    drop(ready);
    drop(prepared);
    for _ in 0..AUXILIARY_READERS_MAX {
        loop {
            check_cancel(cancel)?;
            match waiting.recv_timeout(Duration::from_millis(20)) {
                Ok(()) => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    parallel.stop()?;
                    return Ok(None);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            if parallel.shared.stopped() {
                parallel.stop()?;
                return Ok(None);
            }
        }
    }
    // Every worker pinned its transaction before reporting ready. With no commit
    // between the master's observer probe and this one, all readers see the same
    // data. Workers bypass reader()/read_gate to avoid a waiting admin deadlock.
    if parallel.shared.stopped() || provider.observe_search_revision()? != revision {
        parallel.stop()?;
        return Ok(None);
    }
    parallel.prepare_gate.decide(true);
    Ok(Some(parallel))
}

/// Prepare disjoint global phrase statistics while the pinned master loads its
/// document lengths, then collect exact scores with up to seven extra readers.
/// Preparation/revision races retain serial scoring on the master's snapshot;
/// SQL/data failures propagate. None requests the existing SQL paging fallback
/// when the bounded ranking budget cannot retain this complete result.
pub(crate) fn collect(
    provider: &SqliteProvider,
    tx: &Connection,
    query: String,
    snapshot: Snapshot<'_>,
) -> Result<Option<RankedRows>> {
    collect_inner(
        provider,
        tx,
        search_bm25::Prepared::empty(query),
        snapshot,
        None,
    )
}

fn collect_inner(
    provider: &SqliteProvider,
    tx: &Connection,
    plan: Arc<search_bm25::Prepared>,
    snapshot: Snapshot<'_>,
    fixture_cuts: Option<SplitPoints>,
) -> Result<Option<RankedRows>> {
    let Snapshot {
        revision,
        cache_eligible,
        cancel,
    } = snapshot;
    check_cancel(cancel)?;
    // Layout reads the first matching row's global shape without enumerating
    // phrase DFs or decoding docsize. It releases its cursor before fanout.
    if !search_bm25::prepare_layout(tx, &plan)? {
        check_cancel(cancel)?;
        return Ok(Some(Vec::new()));
    }
    let mut parallel = if cache_eligible && (fixture_cuts.is_some() || plan.is_large()) {
        try_start(provider, plan.clone(), revision, cancel, fixture_cuts)?
    } else {
        None
    };
    // Workers are already past the verified snapshot gate and enumerating
    // disjoint DFs while the root alone reads or reuses its owned docsize cache.
    let lengths = provider.rank_document_lengths(
        tx,
        cache_eligible.then_some(revision),
        cancel.map(Arc::as_ref),
    )?;
    let has_lengths = lengths.is_some();
    plan.set_lengths(lengths)?;
    if let Some(mut active) = parallel.take()
        && let Some(partials) = active.partials(cancel)?
    {
        plan.complete_preparation(partials)?;
        let cuts = plan
            .split_points()
            .or(fixture_cuts)
            .filter(|cuts| cuts.windows(2).all(|pair| pair[0] < pair[1]));
        if has_lengths
            && (fixture_cuts.is_some() || plan.parallel_worthy())
            && let Some(cuts) = cuts
        {
            active.boundary = cuts[0];
            // Publishing all statistics and lengths precedes opening this gate.
            // Each worker then obtains its own bounds from the completed plan.
            active.score_gate.decide(true);
            parallel = Some(active);
        } else {
            // DFs remain useful for rare queries even when scoring stays serial.
            active.score_gate.decide(false);
            if let Err(WorkerFailure::Fatal(error)) = active.finish() {
                return Err(error);
            }
        }
    }
    check_cancel(cancel)?;
    // Sparse or oversized docsize layouts retain SQLite's true native rank
    // path. An incomplete preparation likewise lets this root cursor initialize
    // global statistics once, without using partial results from other readers.
    let _registration = has_lengths
        .then(|| search_bm25::register_prepared(tx, plan.clone()))
        .transpose()?;
    let sql = if has_lengths {
        "SELECT rowid,rank FROM audio_search WHERE audio_search MATCH ?1
         AND rank MATCH 'audiolib_bm25()' ORDER BY rowid ASC"
    } else {
        "SELECT rowid,rank FROM audio_search WHERE audio_search MATCH ?1
         AND rank MATCH 'bm25()' ORDER BY rowid ASC"
    };
    let mut statement = tx.prepare(sql).map_err(db_error)?;
    let mut rows = statement.query([plan.query()]).map_err(db_error)?;
    let mut pending = None;
    let mut ranked = Vec::new();
    let mut remaining = 0;
    loop {
        let item = if let Some(item) = pending.take() {
            item
        } else if let Some(row) = rows.next().map_err(db_error)? {
            (row.get(0).map_err(db_error)?, row.get(1).map_err(db_error)?)
        } else {
            break;
        };
        if ranked.len().is_multiple_of(BUDGET_BATCH) {
            check_cancel(cancel)?;
        }
        if let Some(active) = parallel.as_ref() {
            if item.0 >= active.boundary || active.shared.stopped() {
                pending = Some(item);
                break;
            }
            if !active.shared.claim(&mut remaining) {
                pending = Some(item);
                break;
            }
        }
        if ranked.len() == search_cache::MAX_RANKED_IDS {
            return Ok(None);
        }
        ranked.push(item);
    }
    if let Some(mut active) = parallel.take() {
        let parts = match active.finish() {
            Err(WorkerFailure::Fatal(error)) => return Err(error),
            other => other,
        };
        check_cancel(cancel)?;
        if active.shared.overflow.load(Ordering::Acquire) {
            return Ok(None);
        }
        if let Ok(parts) = parts {
            for part in parts {
                if ranked.len() + part.len() > search_cache::MAX_RANKED_IDS {
                    return Ok(None);
                }
                ranked.extend(part);
            }
            check_cancel(cancel)?;
            return Ok(Some(ranked));
        }
        drop(active);
        // Every auxiliary reader has drained. Resume at the unconsumed master's
        // row, not a new query or snapshot, and discard all partial worker data.
        if let Some(item) = pending {
            if ranked.len() == search_cache::MAX_RANKED_IDS {
                return Ok(None);
            }
            ranked.push(item);
        }
        while let Some(row) = rows.next().map_err(db_error)? {
            if ranked.len().is_multiple_of(BUDGET_BATCH) {
                check_cancel(cancel)?;
            }
            if ranked.len() == search_cache::MAX_RANKED_IDS {
                return Ok(None);
            }
            ranked.push((row.get(0).map_err(db_error)?, row.get(1).map_err(db_error)?));
        }
    }
    check_cancel(cancel)?;
    Ok(Some(ranked))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DataProvider, SearchRequest};

    #[test]
    fn unavailable_readers_never_hide_corruption_or_io_errors() {
        for code in [
            rusqlite::ffi::SQLITE_BUSY,
            rusqlite::ffi::SQLITE_LOCKED,
            rusqlite::ffi::SQLITE_CANTOPEN,
            rusqlite::ffi::SQLITE_INTERRUPT,
        ] {
            let failure = prepare_error(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(code),
                None,
            ));
            assert!(matches!(failure, WorkerFailure::Unavailable));
        }
        for code in [rusqlite::ffi::SQLITE_CORRUPT, rusqlite::ffi::SQLITE_IOERR] {
            let failure = prepare_error(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(code),
                None,
            ));
            assert!(matches!(failure, WorkerFailure::Fatal(Error::Database(_))));
        }
    }

    fn fixture() -> (tempfile::TempDir, SqliteProvider) {
        let directory = tempfile::tempdir().unwrap();
        let provider = SqliteProvider::open(&directory.path().join("parallel.db")).unwrap();
        let mut writer = Connection::open(provider.database_path()).unwrap();
        let tx = writer.transaction().unwrap();
        for index in 0..512_i64 {
            let id = if index < 256 {
                i64::MIN + index
            } else {
                i64::MAX - (511 - index)
            };
            tx.execute(
                "INSERT INTO audio_files(id,file_path,filename,user_tags,missing)
                 VALUES(?1,?2,'metal.wav',?3,0)",
                params![
                    id,
                    format!("{id}.wav"),
                    format!(
                        "{}{}",
                        "metal ".repeat(index as usize % 3),
                        if index % 31 == 0 { "rare" } else { "" }
                    )
                ],
            )
            .unwrap();
        }
        tx.commit().unwrap();
        (directory, provider)
    }

    fn native(tx: &Connection, query: &str) -> Vec<(i64, u64)> {
        tx.prepare(
            "SELECT rowid,bm25(audio_search) FROM audio_search
             WHERE audio_search MATCH ?1 ORDER BY rowid",
        )
        .unwrap()
        .query_map([query], |row| {
            Ok((row.get(0)?, row.get::<_, f64>(1)?.to_bits()))
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
    }

    fn fixture_cuts() -> SplitPoints {
        [
            i64::MIN + 64,
            i64::MIN + 128,
            i64::MIN + 192,
            i64::MAX - 255,
            i64::MAX - 191,
            i64::MAX - 127,
            i64::MAX - 63,
        ]
    }

    #[test]
    fn shared_global_statistics_preserve_native_bits_across_extreme_id_ranges() {
        let (_directory, provider) = fixture();
        let mut reader = provider.reader().unwrap();
        let tx = reader.transaction().unwrap();
        tx.query_row("SELECT id FROM audio_files LIMIT 1", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap();
        let compiled = crate::compile_query(&provider.tokenizer, "metal").unwrap();
        for query in [
            compiled.clone(),
            format!("({compiled}) OR user_tags : \"rare\""),
            format!("({compiled}) AND filename : \"metal\""),
        ] {
            let expected = native(&tx, &query);
            let plan = search_bm25::Prepared::empty(query);
            let actual = collect_inner(
                &provider,
                &tx,
                plan.clone(),
                Snapshot {
                    revision: provider.observe_search_revision().unwrap(),
                    cache_eligible: true,
                    cancel: None,
                },
                Some(fixture_cuts()),
            )
            .unwrap()
            .unwrap()
            .into_iter()
            .map(|(id, score)| (id, score.to_bits()))
            .collect::<Vec<_>>();
            assert_eq!(actual, expected);
            assert_eq!(actual.len(), 512);
            assert!(plan.is_ready());
            assert!(!provider.parallel_rank_reserved.load(Ordering::Acquire));
        }
    }

    #[test]
    fn sparse_lengths_keep_native_ranking_after_parallel_phrase_preparation() {
        let (_directory, provider) = fixture();
        let writer = Connection::open(provider.database_path()).unwrap();
        writer
            .execute(
                "INSERT INTO audio_files(id,file_path,filename,missing)
                 VALUES(0,'middle.wav','metal.wav',0)",
                [],
            )
            .unwrap();
        let mut reader = provider.reader().unwrap();
        let tx = reader.transaction().unwrap();
        tx.query_row("SELECT id FROM audio_files LIMIT 1", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap();
        // Three extreme clusters cannot fit the bounded two-domain cache.
        assert!(search_bm25::load_lengths(&tx, None).unwrap().is_none());
        let query = crate::compile_query(&provider.tokenizer, "metal").unwrap();
        let expected = native(&tx, &query);
        let plan = search_bm25::Prepared::empty(query);
        let actual = collect_inner(
            &provider,
            &tx,
            plan.clone(),
            Snapshot {
                revision: provider.observe_search_revision().unwrap(),
                cache_eligible: true,
                cancel: None,
            },
            Some([
                i64::MIN + 64,
                i64::MIN + 128,
                i64::MIN + 192,
                0,
                i64::MAX - 191,
                i64::MAX - 127,
                i64::MAX - 63,
            ]),
        )
        .unwrap()
        .unwrap()
        .into_iter()
        .map(|(id, score)| (id, score.to_bits()))
        .collect::<Vec<_>>();
        assert_eq!(actual, expected);
        assert!(plan.is_ready());
        assert!(plan.split_points().is_none());
        assert!(!provider.parallel_rank_reserved.load(Ordering::Acquire));
    }

    #[test]
    fn a_commit_between_master_and_worker_pins_resumes_the_original_snapshot() {
        let (_directory, provider) = fixture();
        let mut reader = provider.reader().unwrap();
        let tx = reader.transaction().unwrap();
        tx.query_row("SELECT id FROM audio_files LIMIT 1", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap();
        let query = crate::compile_query(&provider.tokenizer, "metal").unwrap();
        let expected = native(&tx, &query);
        let revision = provider.observe_search_revision().unwrap();
        let plan = search_bm25::Prepared::empty(query);
        let writer = SqliteProvider::open_existing(provider.database_path()).unwrap();
        writer
            .update_metadata(
                i64::MAX,
                &crate::MetadataPatch {
                    tags: Some("different".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        let actual = collect_inner(
            &provider,
            &tx,
            plan,
            Snapshot {
                revision,
                cache_eligible: true,
                cancel: None,
            },
            Some(fixture_cuts()),
        )
        .unwrap()
        .unwrap()
        .into_iter()
        .map(|(id, score)| (id, score.to_bits()))
        .collect::<Vec<_>>();
        assert_eq!(actual, expected);
        assert!(!provider.parallel_rank_reserved.load(Ordering::Acquire));
    }

    #[test]
    fn cancelled_or_failed_startup_workers_never_publish_second_readiness() {
        let (directory, provider) = fixture();
        let mut reader = provider.reader().unwrap();
        let tx = reader.transaction().unwrap();
        tx.query_row("SELECT id FROM audio_files LIMIT 1", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap();
        let query = crate::compile_query(&provider.tokenizer, "metal").unwrap();
        let plan = search_bm25::Prepared::empty(query);
        assert!(search_bm25::prepare_layout(&tx, &plan).unwrap());
        for fail_startup in [false, true] {
            let shared = Arc::new(Shared {
                external: Arc::new(AtomicBool::new(false)),
                abort: AtomicBool::new(false),
                overflow: AtomicBool::new(false),
                reserved: AtomicUsize::new(0),
            });
            let prepare = Arc::new(StartGate::default());
            let score = Arc::new(StartGate::default());
            let (ready, waiting) = mpsc::sync_channel(AUXILIARY_READERS_MAX);
            let (prepared, receiving) = mpsc::sync_channel(AUXILIARY_READERS_MAX);
            let make_gates = || WorkerGates {
                shared: shared.clone(),
                prepare: prepare.clone(),
                score: score.clone(),
                ready: ready.clone(),
                prepared: prepared.clone(),
            };
            let count = AUXILIARY_READERS_MAX - usize::from(fail_startup);
            let mut workers = Vec::new();
            for index in 0..count {
                let path = provider.path.clone();
                let work = WorkerPlan {
                    plan: plan.clone(),
                    index,
                    phrases: Vec::new(),
                    fixture_cuts: Some(fixture_cuts()),
                };
                let gates = make_gates();
                workers.push(thread::spawn(move || run_worker(&path, work, &gates)));
            }
            // All successful readers have sent readiness exactly once and now
            // wait at the unopened prepare gate. Force each failure path only
            // after these first messages have been consumed deterministically.
            for _ in 0..count {
                waiting.recv_timeout(Duration::from_secs(5)).unwrap();
            }
            if fail_startup {
                let missing = directory.path().join("missing-parent").join("missing.db");
                let failure = run_worker(
                    &missing,
                    WorkerPlan {
                        plan: plan.clone(),
                        index: count,
                        phrases: Vec::new(),
                        fixture_cuts: Some(fixture_cuts()),
                    },
                    &make_gates(),
                );
                assert!(matches!(failure, Err(WorkerFailure::Unavailable)));
            } else {
                shared.external.store(true, Ordering::Release);
            }
            for worker in workers {
                assert!(matches!(
                    worker.join().unwrap(),
                    Err(WorkerFailure::Unavailable)
                ));
            }
            assert!(waiting.try_recv().is_err());
            assert!(receiving.try_recv().is_err());
        }
    }

    #[test]
    fn cancelled_workers_never_publish_a_second_preparation_result() {
        let (_directory, provider) = fixture();
        let mut reader = provider.reader().unwrap();
        let tx = reader.transaction().unwrap();
        tx.query_row("SELECT id FROM audio_files LIMIT 1", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap();
        let query = crate::compile_query(&provider.tokenizer, "metal").unwrap();
        let plan = search_bm25::Prepared::empty(query);
        assert!(search_bm25::prepare_layout(&tx, &plan).unwrap());
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut active = try_start(
            &provider,
            plan,
            provider.observe_search_revision().unwrap(),
            Some(&cancelled),
            Some(fixture_cuts()),
        )
        .unwrap()
        .unwrap();
        // Every worker has published its sole result, and the score gate remains
        // undecided. Cancellation now forces every worker down the error path.
        assert_eq!(
            active.partials(Some(&cancelled)).unwrap().unwrap().len(),
            AUXILIARY_READERS_MAX
        );
        cancelled.store(true, Ordering::Release);
        assert!(matches!(active.finish(), Err(WorkerFailure::Unavailable)));
        assert!(active.prepared.try_recv().is_err());
        drop(active);
        assert!(!provider.parallel_rank_reserved.load(Ordering::Acquire));
    }

    #[test]
    fn cancellation_at_either_gate_releases_reservation_and_leaves_reader_usable() {
        let (_directory, provider) = fixture();
        for finish_preparation in [false, true] {
            let mut reader = provider.reader().unwrap();
            let tx = reader.transaction().unwrap();
            tx.query_row("SELECT id FROM audio_files LIMIT 1", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap();
            let query = crate::compile_query(&provider.tokenizer, "metal").unwrap();
            let plan = search_bm25::Prepared::empty(query);
            assert!(search_bm25::prepare_layout(&tx, &plan).unwrap());
            let cancelled = Arc::new(AtomicBool::new(false));
            let mut active = try_start(
                &provider,
                plan.clone(),
                provider.observe_search_revision().unwrap(),
                Some(&cancelled),
                Some(fixture_cuts()),
            )
            .unwrap()
            .unwrap();
            if finish_preparation {
                let lengths = search_bm25::load_lengths(&tx, None).unwrap();
                plan.set_lengths(lengths).unwrap();
                let partials = active.partials(Some(&cancelled)).unwrap().unwrap();
                plan.complete_preparation(partials).unwrap();
                active.score_gate.decide(true);
            }
            cancelled.store(true, Ordering::Release);
            let _ = active.finish();
            drop(active);
            assert!(!provider.parallel_rank_reserved.load(Ordering::Acquire));
        }
        assert!(!provider.parallel_rank_reserved.load(Ordering::Acquire));
        assert_eq!(
            provider
                .search(&SearchRequest::default())
                .unwrap()
                .total_count,
            512
        );
    }
}
