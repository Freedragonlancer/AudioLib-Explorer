// crates/audiolib-audio/examples/first-sound-loopback/load.rs
//! Concurrent list Waveform work for `first-sound-loopback`: one background-mode thread
//! computing list Waveforms with the same function and yield rules as the desktop worker
//! (`app/audiolib-desktop/src/list_waveforms.rs`), or without the yield rules (`stress`).
use audiolib_audio::list_waveform_with_cancel;
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

/// Same values as the desktop worker.
const PREBUFFER_YIELD: Duration = Duration::from_secs(3);
const BACKFILL_RESUME_DELAY: Duration = Duration::from_secs(5);
const BACKFILL_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Rows scrolling into view: one on-demand decode after another.
    OnDemand,
    /// "Pre-generate list Waveforms": at most one file start per 200 ms.
    Backfill,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    /// The desktop rules: no new decode during the first 3 s of an audition; the
    /// backfill pauses for the whole audition and 5 s after it.
    App,
    /// No yielding at all (worst case).
    Stress,
}

#[derive(Default)]
struct Audition {
    started: Option<Instant>,
    playing: bool,
    ended: Option<Instant>,
}

#[derive(Default)]
struct Stats {
    /// Decode intervals in ms on the capture clock.
    busy: Vec<(f64, f64)>,
    failures: u64,
}

struct Shared {
    stop: AtomicBool,
    cancel: AtomicBool,
    audition: Mutex<Audition>,
    stats: Mutex<Stats>,
}

pub struct Loader {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    kind: Kind,
    policy: Policy,
    files: usize,
}

impl Loader {
    pub fn start(
        kind: Kind,
        policy: Policy,
        files: Vec<PathBuf>,
        clock: Instant,
    ) -> Result<Self, String> {
        if files.is_empty() {
            return Err("--files has no .wav files".into());
        }
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            audition: Mutex::new(Audition::default()),
            stats: Mutex::new(Stats::default()),
        });
        let count = files.len();
        let worker = shared.clone();
        let thread = thread::Builder::new()
            .name("list-waveform-load".into())
            .spawn(move || {
                audiolib_windows::enter_background_thread_mode();
                let mut next = 0_usize;
                let mut last_start: Option<Instant> = None;
                while !worker.stop.load(Ordering::Acquire) {
                    let now = Instant::now();
                    let gate = {
                        let audition = worker.audition.lock().unwrap();
                        let prebuffer = audition
                            .started
                            .map(|at| (at + PREBUFFER_YIELD).saturating_duration_since(now))
                            .filter(|wait| !wait.is_zero());
                        match (kind, policy) {
                            (_, Policy::Stress) => None,
                            (Kind::OnDemand, Policy::App) => prebuffer,
                            (Kind::Backfill, Policy::App) => {
                                if audition.playing {
                                    Some(Duration::from_millis(20))
                                } else {
                                    audition
                                        .ended
                                        .map(|at| {
                                            (at + BACKFILL_RESUME_DELAY)
                                                .saturating_duration_since(now)
                                        })
                                        .filter(|wait| !wait.is_zero())
                                        .or(prebuffer)
                                }
                            }
                        }
                    };
                    let rate = (kind == Kind::Backfill)
                        .then(|| {
                            last_start
                                .map(|at| (at + BACKFILL_INTERVAL).saturating_duration_since(now))
                        })
                        .flatten()
                        .filter(|wait| !wait.is_zero());
                    if let Some(wait) = gate.or(rate) {
                        thread::sleep(wait.min(Duration::from_millis(20)));
                        continue;
                    }
                    let path = &files[next % files.len()];
                    next += 1;
                    let begin = Instant::now();
                    last_start = Some(begin);
                    let result = list_waveform_with_cancel(path, &worker.cancel);
                    let end = Instant::now();
                    let mut stats = worker.stats.lock().unwrap();
                    stats.failures += u64::from(result.is_err());
                    let ms = |at: Instant| at.duration_since(clock).as_secs_f64() * 1000.0;
                    stats.busy.push((ms(begin), ms(end)));
                }
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            shared,
            thread: Some(thread),
            kind,
            policy,
            files: count,
        })
    }

    /// Load/Play/Seek sent (the desktop's `note_audition_start`).
    pub fn audition_started(&self) {
        let mut audition = self.shared.audition.lock().unwrap();
        audition.started = Some(Instant::now());
        audition.playing = true;
    }

    /// Stop sent (the desktop's audition end).
    pub fn audition_stopped(&self) {
        let mut audition = self.shared.audition.lock().unwrap();
        audition.playing = false;
        audition.ended = Some(Instant::now());
    }

    /// Stop the worker; returns the decode intervals (ms) and a summary.
    pub fn finish(mut self, elapsed_ms: f64) -> (Vec<(f64, f64)>, Value) {
        self.shared.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let stats = self.shared.stats.lock().unwrap();
        let mut durations: Vec<f64> = stats.busy.iter().map(|(b, e)| e - b).collect();
        durations.sort_by(f64::total_cmp);
        let busy: f64 = durations.iter().sum();
        let summary = json!({
            "kind": format!("{:?}", self.kind), "policy": format!("{:?}", self.policy),
            "files": self.files, "decodes": durations.len(), "failures": stats.failures,
            "decode_ms_median": durations.get(durations.len() / 2).copied(),
            "decode_ms_max": durations.last().copied(),
            "busy_fraction": if elapsed_ms > 0.0 { busy / elapsed_ms } else { 0.0 },
        });
        (stats.busy.clone(), summary)
    }
}
