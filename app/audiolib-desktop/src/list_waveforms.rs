// app/audiolib-desktop/src/list_waveforms.rs
//! List thumbnail Waveforms of the result table (doc/api/README.md §9.3).
//!
//! One low-priority background worker answers the visible rows (plus one screen above
//! and below) through the [`DataProvider`]: it first looks stored list Waveforms up
//! (cheap, never decodes), then computes missing ones one file at a time (local mode),
//! and finally, when the user enabled "pre-generate list Waveforms", backfills the
//! library at a bounded rate. Rows that scroll out of the window are dropped from the
//! queues and an on-demand decode of such a row is cancelled.
//!
//! Zero-latency rules: the worker never runs on the event loop or in the audio
//! callback; it takes no new decode during the first [`PREBUFFER_YIELD`] after an
//! audition starts (Load/Play/Seek), uses at most one decode at a time, and the backfill
//! pauses for the whole audition and while a scan runs.
use audiolib_core::{
    DataProvider, Error, LIST_WAVEFORM_COLUMNS, ListWaveform, ListWaveformSource, ListWaveformState,
};
use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{
        Arc, Condvar, Mutex, MutexGuard, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

/// No new decode starts this long after an audition (Load/Play/Seek) starts.
pub(crate) const PREBUFFER_YIELD: Duration = Duration::from_secs(3);
/// The backfill resumes this long after the last audition ended.
pub(crate) const BACKFILL_RESUME_DELAY: Duration = Duration::from_secs(5);
/// Backfill starts at most one file per interval (5 files per second).
pub(crate) const BACKFILL_INTERVAL: Duration = Duration::from_millis(200);
/// Ids per lookup request.
const LOOKUP_BATCH: usize = 64;
/// Ids per backlog query.
const BACKLOG_BATCH: usize = 256;
/// Results kept in memory for redisplay without a lookup.
const CACHE_CAPACITY: usize = 4096;
/// Re-evaluation interval of an idle worker (provider availability, scans).
const IDLE_POLL: Duration = Duration::from_secs(1);

/// What a result row shows in its Waveform column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ListWaveformCell {
    /// Columns to draw; zero columns (an empty file) draw a center line.
    Ready(ListWaveform),
    /// Missing file or undecodable: the "no Waveform" placeholder.
    Unavailable,
}

/// The desktop side the worker needs; implemented by the controller.
pub(crate) trait ListWaveformHost: Send + Sync + 'static {
    /// The provider to use now, or `None` while library work must not run (library
    /// operations, source changes, closing).
    fn list_waveform_provider(&self) -> Option<Arc<dyn DataProvider>>;
    /// Whether a scan is running or paused (the backfill yields to it).
    fn list_waveform_scanning(&self) -> bool;
    /// Publish results; the host checks `generation` on the event loop.
    fn deliver_list_waveforms(&self, generation: u64, cells: Vec<(i64, ListWaveformCell)>);
}

#[derive(Debug)]
pub(crate) enum Task {
    Lookup {
        generation: u64,
        ids: Vec<i64>,
    },
    Compute {
        generation: u64,
        id: i64,
        cancel: Arc<AtomicBool>,
        on_demand: bool,
    },
    Backlog {
        generation: u64,
        after: i64,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Step {
    Exit,
    Wait(Option<Duration>),
    Run,
}

struct Current {
    id: i64,
    cancel: Arc<AtomicBool>,
    on_demand: bool,
}

#[derive(Default)]
struct Backfill {
    active: bool,
    cursor: i64,
    queue: VecDeque<i64>,
    last_start: Option<Instant>,
}

#[derive(Default)]
pub(crate) struct State {
    generation: u64,
    /// Wanted ids in priority order (visible first) and as a set.
    window: Vec<i64>,
    in_window: HashSet<i64>,
    cache: HashMap<i64, ListWaveformCell>,
    cache_order: VecDeque<i64>,
    /// Cached ids to look up again (files may have changed, e.g. after a scan).
    verify: HashSet<i64>,
    /// Window ids already looked up in this generation.
    looked_up: HashSet<i64>,
    /// Looked-up `Pending` ids waiting for a local decode.
    compute: HashSet<i64>,
    /// Ids whose decode ended without a result; not retried while they stay in view.
    attempted: HashSet<i64>,
    current: Option<Current>,
    busy: bool,
    suspended: bool,
    shutdown: bool,
    playing: bool,
    playback_start: Option<Instant>,
    playback_end: Option<Instant>,
    pregenerate: bool,
    backfill: Backfill,
    task: Option<Task>,
}

impl State {
    fn remember(&mut self, id: i64, cell: ListWaveformCell) {
        if self.cache.insert(id, cell).is_none() {
            self.cache_order.push_back(id);
        }
        while self.cache.len() > CACHE_CAPACITY {
            let Some(oldest) = self.cache_order.pop_front() else {
                break;
            };
            // Keep what is on screen.
            if self.in_window.contains(&oldest) {
                self.cache_order.push_back(oldest);
                if self.cache_order.len() <= self.in_window.len() {
                    break;
                }
                continue;
            }
            self.cache.remove(&oldest);
            self.verify.remove(&oldest);
        }
    }

    fn cancel_current(&mut self, only_on_demand_outside_window: bool) {
        if let Some(current) = &self.current
            && (!only_on_demand_outside_window
                || (current.on_demand && !self.in_window.contains(&current.id)))
        {
            current.cancel.store(true, Ordering::Release);
        }
    }

    /// Choose the next task (stored in `self.task`) or how long to wait. Pure policy:
    /// `now`, `scanning` and `source` come from the caller.
    pub(crate) fn next_step(
        &mut self,
        now: Instant,
        scanning: bool,
        source: ListWaveformSource,
    ) -> Step {
        if self.shutdown {
            return Step::Exit;
        }
        if self.suspended || source == ListWaveformSource::Unavailable {
            return Step::Wait(Some(IDLE_POLL));
        }
        // 1. Cheap lookups of the window, visible rows first.
        let ids: Vec<i64> = self
            .window
            .iter()
            .copied()
            .filter(|id| {
                !self.looked_up.contains(id)
                    && (!self.cache.contains_key(id) || self.verify.contains(id))
            })
            .take(LOOKUP_BATCH)
            .collect();
        if !ids.is_empty() {
            self.looked_up.extend(&ids);
            self.task = Some(Task::Lookup {
                generation: self.generation,
                ids,
            });
            return Step::Run;
        }
        if source != ListWaveformSource::Local {
            return Step::Wait(None);
        }
        let prebuffering = self
            .playback_start
            .map(|start| now.saturating_duration_since(start))
            .filter(|elapsed| *elapsed < PREBUFFER_YIELD)
            .map(|elapsed| PREBUFFER_YIELD - elapsed);
        // 2. On-demand decodes of window rows, in window order.
        if let Some(id) = self
            .window
            .iter()
            .copied()
            .find(|id| self.compute.contains(id) && !self.attempted.contains(id))
        {
            if let Some(remaining) = prebuffering {
                return Step::Wait(Some(remaining));
            }
            self.compute.remove(&id);
            self.task = Some(self.compute_task(id, true));
            return Step::Run;
        }
        // 3. Rate-limited backfill, only while nothing is auditioned or scanned.
        if !(self.pregenerate && self.backfill.active) {
            return Step::Wait(None);
        }
        if scanning || self.playing {
            return Step::Wait(Some(IDLE_POLL));
        }
        if let Some(remaining) = prebuffering {
            return Step::Wait(Some(remaining));
        }
        if let Some(end) = self.playback_end {
            let elapsed = now.saturating_duration_since(end);
            if elapsed < BACKFILL_RESUME_DELAY {
                return Step::Wait(Some(BACKFILL_RESUME_DELAY - elapsed));
            }
        }
        while let Some(id) = self.backfill.queue.front().copied() {
            if self.cache.contains_key(&id) && !self.verify.contains(&id) {
                self.backfill.queue.pop_front();
                continue;
            }
            if let Some(last) = self.backfill.last_start {
                let elapsed = now.saturating_duration_since(last);
                if elapsed < BACKFILL_INTERVAL {
                    return Step::Wait(Some(BACKFILL_INTERVAL - elapsed));
                }
            }
            self.backfill.queue.pop_front();
            self.backfill.last_start = Some(now);
            self.task = Some(self.compute_task(id, false));
            return Step::Run;
        }
        self.task = Some(Task::Backlog {
            generation: self.generation,
            after: self.backfill.cursor,
        });
        Step::Run
    }

    fn compute_task(&mut self, id: i64, on_demand: bool) -> Task {
        let cancel = Arc::new(AtomicBool::new(false));
        self.current = Some(Current {
            id,
            cancel: cancel.clone(),
            on_demand,
        });
        Task::Compute {
            generation: self.generation,
            id,
            cancel,
            on_demand,
        }
    }

    /// Apply a finished task; returns the cells to publish.
    pub(crate) fn finish(&mut self, outcome: Outcome) -> Vec<(i64, ListWaveformCell)> {
        self.current = None;
        let mut cells = Vec::new();
        match outcome {
            Outcome::Lookup {
                generation,
                ids,
                result,
            } => {
                if generation != self.generation {
                    return cells;
                }
                let Ok(batch) = result else {
                    // Retried once the rows scroll out and back (or after a scan).
                    return cells;
                };
                for item in batch.items {
                    if !ids.contains(&item.id) {
                        continue;
                    }
                    self.verify.remove(&item.id);
                    let cell = match item.state {
                        ListWaveformState::Ready { waveform } => ListWaveformCell::Ready(waveform),
                        ListWaveformState::Missing | ListWaveformState::Failed { .. } => {
                            ListWaveformCell::Unavailable
                        }
                        ListWaveformState::Pending { previous }
                        | ListWaveformState::RootOffline { previous, .. } => {
                            if self.in_window.contains(&item.id) {
                                self.compute.insert(item.id);
                            }
                            if let Some(previous) = previous {
                                cells.push((item.id, ListWaveformCell::Ready(previous)));
                            }
                            continue;
                        }
                    };
                    self.remember(item.id, cell.clone());
                    cells.push((item.id, cell));
                }
            }
            Outcome::Compute {
                generation,
                id,
                on_demand,
                result,
            } => {
                if generation != self.generation {
                    return cells;
                }
                let cell = match result {
                    Ok(ListWaveformState::Ready { waveform }) => ListWaveformCell::Ready(waveform),
                    Ok(ListWaveformState::Missing | ListWaveformState::Failed { .. }) => {
                        ListWaveformCell::Unavailable
                    }
                    Err(Error::Cancelled) => {
                        // Scrolled out (or suspended): look it up again when it returns.
                        self.looked_up.remove(&id);
                        return cells;
                    }
                    Ok(_) | Err(_) => {
                        // The file changed while decoding, or I/O failed: not retried
                        // while the row stays in view.
                        self.attempted.insert(id);
                        return cells;
                    }
                };
                self.verify.remove(&id);
                if on_demand || self.in_window.contains(&id) {
                    self.remember(id, cell.clone());
                }
                cells.push((id, cell));
            }
            Outcome::Backlog { generation, result } => {
                if generation != self.generation {
                    return cells;
                }
                match result {
                    Ok(ids) if !ids.is_empty() => {
                        self.backfill.cursor = *ids.last().unwrap();
                        self.backfill.queue.extend(ids);
                    }
                    // Done (or the library refused): the next scan starts a new pass.
                    _ => self.backfill.active = false,
                }
            }
        }
        cells
    }
}

/// Result of a [`Task`] run without the state lock.
pub(crate) enum Outcome {
    Lookup {
        generation: u64,
        ids: Vec<i64>,
        result: audiolib_core::Result<audiolib_core::ListWaveformBatch>,
    },
    Compute {
        generation: u64,
        id: i64,
        on_demand: bool,
        result: audiolib_core::Result<ListWaveformState>,
    },
    Backlog {
        generation: u64,
        result: audiolib_core::Result<Vec<i64>>,
    },
}

/// Blocking: run one task against `provider`.
pub(crate) fn run_task(provider: &dyn DataProvider, task: Task) -> Outcome {
    match task {
        Task::Lookup { generation, ids } => Outcome::Lookup {
            result: provider.list_waveforms(&ids, Arc::new(AtomicBool::new(false))),
            generation,
            ids,
        },
        Task::Compute {
            generation,
            id,
            cancel,
            on_demand,
        } => Outcome::Compute {
            generation,
            id,
            on_demand,
            result: provider.compute_list_waveform(id, cancel),
        },
        Task::Backlog { generation, after } => Outcome::Backlog {
            generation,
            result: provider.list_waveform_backlog(after, BACKLOG_BATCH),
        },
    }
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

/// Handle owned by the controller; cheap to clone.
#[derive(Clone)]
pub(crate) struct ListWaveforms {
    shared: Arc<Shared>,
}

impl ListWaveforms {
    pub(crate) fn new(pregenerate: bool) -> Self {
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new(State {
                    pregenerate,
                    backfill: Backfill {
                        active: pregenerate,
                        ..Backfill::default()
                    },
                    ..State::default()
                }),
                wake: Condvar::new(),
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn update(&self, change: impl FnOnce(&mut State)) {
        let mut state = self.lock();
        change(&mut state);
        self.shared.wake.notify_all();
    }

    /// Start the worker thread; it ends on [`ListWaveforms::shutdown`] or when the host
    /// is dropped.
    pub(crate) fn start(&self, host: Weak<dyn ListWaveformHost>) {
        let service = self.clone();
        std::thread::Builder::new()
            .name("audiolib-list-waveforms".into())
            .spawn(move || {
                // Lowest CPU and I/O priority: never compete with audition decoding.
                audiolib_windows::enter_background_thread_mode();
                service.run(host);
            })
            .expect("spawn list Waveform worker");
    }

    fn run(&self, host: Weak<dyn ListWaveformHost>) {
        loop {
            let Some(strong) = host.upgrade() else { return };
            let provider = strong.list_waveform_provider();
            let scanning = strong.list_waveform_scanning();
            drop(strong);
            let source = provider
                .as_ref()
                .map_or(ListWaveformSource::Unavailable, |provider| {
                    provider.list_waveform_source()
                });
            let mut state = self.lock();
            match state.next_step(Instant::now(), scanning, source) {
                Step::Exit => return,
                Step::Wait(timeout) => {
                    let timeout = timeout.unwrap_or(IDLE_POLL).min(IDLE_POLL);
                    drop(
                        self.shared
                            .wake
                            .wait_timeout(state, timeout)
                            .unwrap_or_else(std::sync::PoisonError::into_inner),
                    );
                }
                Step::Run => {
                    let (Some(task), Some(provider)) = (state.task.take(), provider) else {
                        continue;
                    };
                    state.busy = true;
                    drop(state);
                    let outcome = run_task(provider.as_ref(), task);
                    drop(provider);
                    let (generation, cells) = {
                        let mut state = self.lock();
                        state.busy = false;
                        let cells = state.finish(outcome);
                        self.shared.wake.notify_all();
                        (state.generation, cells)
                    };
                    if !cells.is_empty()
                        && let Some(host) = host.upgrade()
                    {
                        host.deliver_list_waveforms(generation, cells);
                    }
                }
            }
        }
    }

    /// Current generation; results of older generations are stale.
    pub(crate) fn generation(&self) -> u64 {
        self.lock().generation
    }

    /// Replace the wanted ids (visible rows first, then the rows one screen below and
    /// above). Unsent work for other ids is dropped and their on-demand decode cancelled.
    pub(crate) fn set_window(&self, ids: Vec<i64>) {
        self.update(|state| {
            let wanted: HashSet<i64> = ids.iter().copied().collect();
            state.looked_up.retain(|id| wanted.contains(id));
            state.compute.retain(|id| wanted.contains(id));
            state.attempted.retain(|id| wanted.contains(id));
            state.window = ids;
            state.in_window = wanted;
            state.cancel_current(true);
        });
    }

    /// Cached cell of `id`, if any (for rows being (re)published).
    pub(crate) fn cached(&self, id: i64) -> Option<ListWaveformCell> {
        self.lock().cache.get(&id).cloned()
    }

    /// A different library, source or snapshot: forget everything and stop.
    pub(crate) fn invalidate(&self) {
        self.update(|state| {
            state.generation += 1;
            state.cancel_current(false);
            state.window.clear();
            state.in_window.clear();
            state.cache.clear();
            state.cache_order.clear();
            state.verify.clear();
            state.looked_up.clear();
            state.compute.clear();
            state.attempted.clear();
            state.backfill = Backfill {
                active: state.pregenerate,
                ..Backfill::default()
            };
        });
    }

    /// A scan finished: cached cells stay visible but are looked up again (files may have
    /// changed), and an enabled pre-generation starts a new pass.
    pub(crate) fn scan_finished(&self) {
        self.update(|state| {
            state.generation += 1;
            state.verify = state.cache.keys().copied().collect();
            state.looked_up.clear();
            state.compute.clear();
            state.attempted.clear();
            state.backfill = Backfill {
                active: state.pregenerate,
                ..Backfill::default()
            };
        });
    }

    /// The "pre-generate list Waveforms" setting changed. Turning it off lets a running
    /// decode finish but starts no new one.
    pub(crate) fn set_pregenerate(&self, enabled: bool) {
        self.update(|state| {
            if enabled && !state.pregenerate {
                state.backfill = Backfill {
                    active: true,
                    ..Backfill::default()
                };
            }
            if !enabled {
                state.backfill = Backfill::default();
            }
            state.pregenerate = enabled;
        });
    }

    /// An audition is starting (Load, Play or Seek): no new decode for
    /// [`PREBUFFER_YIELD`]. Cheap and non-blocking; safe on the event loop.
    pub(crate) fn note_audition_start(&self) {
        self.update(|state| state.playback_start = Some(Instant::now()));
    }

    /// Observed transport state from the playback status worker.
    pub(crate) fn observe_playback(&self, playing: bool) {
        let mut state = self.lock();
        if playing == state.playing {
            return;
        }
        let now = Instant::now();
        if playing {
            state.playback_start = Some(now);
        } else {
            state.playback_end = Some(now);
        }
        state.playing = playing;
        self.shared.wake.notify_all();
    }

    /// Stop taking work (library operations) and cancel a running decode.
    pub(crate) fn suspend(&self) {
        self.update(|state| {
            state.suspended = true;
            state.cancel_current(false);
        });
    }

    pub(crate) fn resume(&self) {
        self.update(|state| state.suspended = false);
    }

    /// Wait until no task runs, at most `timeout`; returns whether the worker is idle.
    pub(crate) fn wait_idle(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut state = self.lock();
        while state.busy {
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            state = self
                .shared
                .wake
                .wait_timeout(state, deadline - now)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
        true
    }

    pub(crate) fn shutdown(&self) {
        self.update(|state| {
            state.shutdown = true;
            state.cancel_current(false);
        });
    }
}

/// Window ids for rows `first .. first + count` of `ids`: the visible rows, then one
/// screen below, then one screen above (nearest first).
pub(crate) fn window_ids(ids: &[i64], first: usize, count: usize) -> Vec<i64> {
    let len = ids.len();
    let first = first.min(len);
    let visible_end = first.saturating_add(count).min(len);
    let below_end = visible_end.saturating_add(count).min(len);
    let above_start = first.saturating_sub(count);
    let mut window = Vec::with_capacity(below_end - above_start);
    window.extend_from_slice(&ids[first..visible_end]);
    window.extend_from_slice(&ids[visible_end..below_end]);
    window.extend(ids[above_start..first].iter().rev());
    window
}

/// Thumbnail height in pixels; the table stretches it to the cell.
pub(crate) const THUMBNAIL_HEIGHT: u32 = 24;
const WAVE_COLOR: Rgba8Pixel = Rgba8Pixel {
    r: 143,
    g: 174,
    b: 166,
    a: 230,
};

/// Rasterize `waveform`: one pixel column per Waveform column (the table stretches
/// `n < 128` columns to the full cell width). `None` for 0 columns (center line).
pub(crate) fn render_list_waveform(
    waveform: &ListWaveform,
) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    let columns = waveform.columns().min(LIST_WAVEFORM_COLUMNS as usize);
    if columns == 0 {
        return None;
    }
    let height = THUMBNAIL_HEIGHT;
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(columns as u32, height);
    let pixels = buffer.make_mut_slice();
    let row = |value: i16| -> u32 {
        let scaled = (32767.0 - f32::from(value.max(-32767))) / 65534.0 * (height - 1) as f32;
        (scaled.round() as u32).min(height - 1)
    };
    for column in 0..columns {
        let (low, high) = waveform.column(column).unwrap_or((0, 0));
        let (top, bottom) = (row(high.max(low)), row(low.min(high)));
        for y in top..=bottom {
            pixels[y as usize * columns + column] = WAVE_COLOR;
        }
    }
    Some(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use audiolib_core::{ListWaveformBatch, ListWaveformItem};

    fn waveform(columns: usize) -> ListWaveform {
        ListWaveform::new(1, [-100, 100].repeat(columns)).unwrap()
    }

    fn state_with_window(ids: &[i64]) -> (ListWaveforms, State) {
        let service = ListWaveforms::new(false);
        service.set_window(ids.to_vec());
        let state = std::mem::take(&mut *service.lock());
        (service, state)
    }

    fn take_task(state: &mut State) -> Task {
        state.task.take().expect("task")
    }

    #[test]
    fn window_is_visible_rows_then_below_then_above() {
        let ids: Vec<i64> = (0..20).collect();
        assert_eq!(window_ids(&ids, 5, 3), vec![5, 6, 7, 8, 9, 10, 4, 3, 2]);
        assert_eq!(window_ids(&ids, 0, 4), vec![0, 1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(window_ids(&ids, 18, 4), vec![18, 19, 17, 16, 15, 14]);
        assert!(window_ids(&[], 3, 4).is_empty());
    }

    #[test]
    fn lookups_precede_decodes_which_yield_to_audition_start() {
        let (_service, mut state) = state_with_window(&[1, 2, 3]);
        let now = Instant::now();
        let local = ListWaveformSource::Local;
        assert_eq!(state.next_step(now, false, local), Step::Run);
        let Task::Lookup { generation, ids } = take_task(&mut state) else {
            panic!("lookup first")
        };
        assert_eq!(ids, vec![1, 2, 3]);
        let cells = state.finish(Outcome::Lookup {
            generation,
            ids: ids.clone(),
            result: Ok(ListWaveformBatch {
                items: vec![
                    ListWaveformItem {
                        id: 1,
                        state: ListWaveformState::Ready {
                            waveform: waveform(128),
                        },
                    },
                    ListWaveformItem {
                        id: 2,
                        state: ListWaveformState::Pending { previous: None },
                    },
                    ListWaveformItem {
                        id: 3,
                        state: ListWaveformState::Missing,
                    },
                ],
                retry_after: None,
            }),
        });
        assert_eq!(
            cells,
            vec![
                (1, ListWaveformCell::Ready(waveform(128))),
                (3, ListWaveformCell::Unavailable)
            ]
        );
        // An audition started 1 s ago: the decode waits for the rest of the 3 s window.
        state.playback_start = Some(now - Duration::from_secs(1));
        assert_eq!(
            state.next_step(now, false, local),
            Step::Wait(Some(Duration::from_secs(2)))
        );
        let later = now + Duration::from_secs(2);
        assert_eq!(state.next_step(later, false, local), Step::Run);
        let Task::Compute { id, on_demand, .. } = take_task(&mut state) else {
            panic!("decode")
        };
        assert_eq!((id, on_demand), (2, true));
        // A server source never decodes on the client.
        let (_service, mut server) = state_with_window(&[9]);
        assert_eq!(
            server.next_step(now, false, ListWaveformSource::Server),
            Step::Run
        );
        take_task(&mut server);
        server.compute.insert(9);
        assert_eq!(
            server.next_step(now, false, ListWaveformSource::Server),
            Step::Wait(None)
        );
        assert_eq!(
            server.next_step(now, false, ListWaveformSource::Unavailable),
            Step::Wait(Some(IDLE_POLL))
        );
    }

    #[test]
    fn scrolling_away_drops_unsent_work_and_cancels_the_running_decode() {
        let service = ListWaveforms::new(false);
        service.set_window(vec![1, 2]);
        let now = Instant::now();
        let cancel = {
            let mut state = service.lock();
            assert_eq!(
                state.next_step(now, false, ListWaveformSource::Local),
                Step::Run
            );
            let Task::Lookup { generation, ids } = take_task(&mut state) else {
                panic!()
            };
            state.finish(Outcome::Lookup {
                generation,
                ids,
                result: Ok(ListWaveformBatch {
                    items: [1, 2]
                        .map(|id| ListWaveformItem {
                            id,
                            state: ListWaveformState::Pending { previous: None },
                        })
                        .to_vec(),
                    retry_after: None,
                }),
            });
            assert_eq!(
                state.next_step(now, false, ListWaveformSource::Local),
                Step::Run
            );
            let Task::Compute { id, cancel, .. } = take_task(&mut state) else {
                panic!()
            };
            assert_eq!(id, 1);
            cancel
        };
        service.set_window(vec![7]);
        assert!(
            cancel.load(Ordering::Acquire),
            "scrolled-out decode is cancelled"
        );
        let mut state = service.lock();
        assert!(
            state.compute.is_empty(),
            "unsent decode of row 2 is dropped"
        );
        let generation = state.generation;
        assert!(
            state
                .finish(Outcome::Compute {
                    generation,
                    id: 1,
                    on_demand: true,
                    result: Err(Error::Cancelled),
                })
                .is_empty()
        );
        assert_eq!(
            state.next_step(now, false, ListWaveformSource::Local),
            Step::Run
        );
        let Task::Lookup { ids, .. } = take_task(&mut state) else {
            panic!()
        };
        assert_eq!(ids, vec![7]);
    }

    #[test]
    fn backfill_is_rate_limited_and_yields_to_scans_and_auditions() {
        let service = ListWaveforms::new(true);
        let mut state = std::mem::take(&mut *service.lock());
        let now = Instant::now();
        let local = ListWaveformSource::Local;
        assert_eq!(
            state.next_step(now, true, local),
            Step::Wait(Some(IDLE_POLL))
        );
        state.playing = true;
        assert_eq!(
            state.next_step(now, false, local),
            Step::Wait(Some(IDLE_POLL))
        );
        state.playing = false;
        state.playback_end = Some(now - Duration::from_secs(1));
        assert_eq!(
            state.next_step(now, false, local),
            Step::Wait(Some(Duration::from_secs(4)))
        );
        let now = now + Duration::from_secs(4);
        assert_eq!(state.next_step(now, false, local), Step::Run);
        let Task::Backlog { generation, after } = take_task(&mut state) else {
            panic!()
        };
        assert_eq!(after, 0);
        state.finish(Outcome::Backlog {
            generation,
            result: Ok(vec![10, 11]),
        });
        assert_eq!(state.next_step(now, false, local), Step::Run);
        let Task::Compute { id, on_demand, .. } = take_task(&mut state) else {
            panic!()
        };
        assert_eq!((id, on_demand), (10, false));
        state.current = None;
        assert_eq!(
            state.next_step(now + Duration::from_millis(50), false, local),
            Step::Wait(Some(Duration::from_millis(150)))
        );
        let next = now + BACKFILL_INTERVAL;
        assert_eq!(state.next_step(next, false, local), Step::Run);
        take_task(&mut state);
        assert_eq!(
            state.next_step(next + BACKFILL_INTERVAL, false, local),
            Step::Run
        );
        let Task::Backlog { generation, after } = take_task(&mut state) else {
            panic!()
        };
        assert_eq!(after, 11);
        state.finish(Outcome::Backlog {
            generation,
            result: Ok(Vec::new()),
        });
        assert!(
            !state.backfill.active,
            "the pass ends at the end of the backlog"
        );
        assert_eq!(state.next_step(next, false, local), Step::Wait(None));
    }

    #[test]
    fn stale_generations_are_dropped_and_scans_reverify_cached_rows() {
        let service = ListWaveforms::new(false);
        service.set_window(vec![1]);
        {
            let mut state = service.lock();
            state.remember(1, ListWaveformCell::Ready(waveform(2)));
        }
        let old = service.generation();
        service.scan_finished();
        assert_eq!(
            service.cached(1),
            Some(ListWaveformCell::Ready(waveform(2)))
        );
        let mut state = service.lock();
        assert!(
            state
                .finish(Outcome::Compute {
                    generation: old,
                    id: 1,
                    on_demand: true,
                    result: Ok(ListWaveformState::Missing),
                })
                .is_empty()
        );
        assert_eq!(
            state.next_step(Instant::now(), false, ListWaveformSource::Local),
            Step::Run
        );
        let Task::Lookup { ids, .. } = take_task(&mut state) else {
            panic!()
        };
        assert_eq!(ids, vec![1]);
        drop(state);
        service.invalidate();
        assert_eq!(service.cached(1), None);
    }

    #[test]
    fn suspension_cancels_and_idle_wait_returns() {
        let service = ListWaveforms::new(false);
        let cancel = Arc::new(AtomicBool::new(false));
        service.lock().current = Some(Current {
            id: 1,
            cancel: cancel.clone(),
            on_demand: false,
        });
        service.suspend();
        assert!(cancel.load(Ordering::Acquire));
        assert_eq!(
            service
                .lock()
                .next_step(Instant::now(), false, ListWaveformSource::Local),
            Step::Wait(Some(IDLE_POLL))
        );
        assert!(service.wait_idle(Duration::from_millis(10)));
        service.lock().busy = true;
        assert!(!service.wait_idle(Duration::from_millis(10)));
        service.shutdown();
        assert_eq!(
            service
                .lock()
                .next_step(Instant::now(), false, ListWaveformSource::Local),
            Step::Exit
        );
    }

    #[test]
    fn thumbnails_have_one_column_per_peak_and_empty_files_none() {
        assert!(render_list_waveform(&waveform(0)).is_none());
        let full = ListWaveform::new(1, vec![-32767, 32767, 0, 0]).unwrap();
        let image = render_list_waveform(&full).unwrap();
        assert_eq!((image.width(), image.height()), (2, THUMBNAIL_HEIGHT));
        let pixels = image.as_slice();
        let filled = |column: usize| {
            (0..THUMBNAIL_HEIGHT as usize)
                .filter(|y| pixels[y * 2 + column].a > 0)
                .count()
        };
        assert_eq!(filled(0), THUMBNAIL_HEIGHT as usize);
        assert_eq!(filled(1), 1, "silence keeps a one-pixel line");
        let short = render_list_waveform(&waveform(125)).unwrap();
        assert_eq!(short.width(), 125);
    }
}
