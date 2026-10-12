// server/audiolib-server/src/roots.rs
//! Cached online/offline status of the configured library roots.
//!
//! Roots usually live on network or sleeping disks (a NAS HDD may need 20-30 s or more to
//! spin up), so no request handler other than `/audio/{id}/stream` ever touches a root
//! path. Instead one background thread per root runs a check (`realpath` + `stat` of the
//! root directory only; no listing, no file reads) at startup, every `root_check_secs`
//! and whenever a stream request asks for a recheck, and caches the result. `GET /roots`
//! only reads that cache.
//!
//! A slow check is not a failure: while a check or a stream file open is pending the
//! previous result stays (pending work after an online result is reported as `waking`
//! once it has taken longer than [`RootPolicy::waking_after`]). A root becomes `offline`
//! only on a definite failure (missing directory, permission denied, I/O or connection
//! error) or when a check or file open has not answered for [`RootPolicy::offline_after`]
//! since the last success; it comes back `online` as soon as a check or a successful
//! stream open succeeds. Stream opens register themselves ([`RootMonitor::begin_open`]),
//! so a hung open (a sleeping disk, a pending macOS permission prompt) drives the root to
//! `waking` and then `offline` even between periodic checks. [`RootMonitor::check_now`]
//! runs a bounded inline recheck. Nothing here keeps a disk awake beyond the periodic
//! `stat`; set a long `root_check_secs` if even that is unwanted.
use crate::config::RootConfig;
use serde::{Deserialize, Serialize};
use std::{
    borrow::Cow,
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex, MutexGuard},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// Root check timing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RootPolicy {
    /// Time between periodic checks of a root.
    pub interval: Duration,
    /// A pending check after an online result is reported as `waking` after this long.
    pub waking_after: Duration,
    /// A check that has not answered for this long marks the root `offline`.
    pub offline_after: Duration,
}

impl Default for RootPolicy {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(300),
            waking_after: Duration::from_secs(5),
            offline_after: Duration::from_secs(120),
        }
    }
}

/// Client-visible root state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RootState {
    /// First check still running (treated as online).
    Checking,
    /// Last check succeeded.
    Online,
    /// A check after an online result is taking long, e.g. a disk spinning up (online).
    Waking,
    /// Definite failure, or no answer for longer than [`RootPolicy::offline_after`].
    Offline,
}

/// Cached status of one root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RootStatus {
    /// Current state.
    pub state: RootState,
    /// Reason for `offline`; `None` otherwise.
    pub message: Option<String>,
    /// Unix seconds of the last completed check or stream outcome, if any.
    pub checked_at: Option<u64>,
    /// Canonical root directory from the most recent successful check (kept while
    /// offline), if any.
    pub canonical: Option<PathBuf>,
}

impl RootStatus {
    /// Everything but `offline` counts as online.
    pub fn online(&self) -> bool {
        self.state != RootState::Offline
    }
}

/// Outcome of probing a root directory: its canonical path or a failure reason.
pub type ProbeResult = Result<PathBuf, String>;
/// Blocking root probe; replaceable for tests.
pub type RootProbe = Arc<dyn Fn(&Path) -> ProbeResult + Send + Sync>;

/// Default probe: `realpath` + `stat` of the root directory (no listing, no file reads).
pub fn probe_root(path: &Path) -> ProbeResult {
    let canonical = fs::canonicalize(path).map_err(|error| describe_io_error(&error))?;
    let metadata = fs::metadata(&canonical).map_err(|error| describe_io_error(&error))?;
    if metadata.is_dir() {
        Ok(canonical)
    } else {
        Err("Root path is not a directory".into())
    }
}

/// Short, path-free description of a root access failure.
pub fn describe_io_error(error: &io::Error) -> String {
    match error.kind() {
        io::ErrorKind::NotFound => "Root directory not found (volume not mounted?)".into(),
        io::ErrorKind::PermissionDenied => "Permission denied for the root directory".into(),
        _ => format!("Root directory is not accessible: {error}"),
    }
}

/// Whether a file access error means the volume or network connection is (perhaps only
/// briefly) unreachable rather than that the file is bad: timeouts, dropped SMB/NFS
/// connections, unreachable hosts and stale network file handles.
pub fn is_transient_network_error(error: &io::Error) -> bool {
    use io::ErrorKind::*;
    matches!(
        error.kind(),
        TimedOut
            | NotConnected
            | ConnectionReset
            | ConnectionAborted
            | ConnectionRefused
            | HostUnreachable
            | NetworkUnreachable
            | NetworkDown
            | StaleNetworkFileHandle
    )
}

#[derive(Default)]
struct SlotState {
    /// Last completed outcome with its wall-clock time.
    last: Option<(ProbeResult, u64)>,
    /// Monotonic time of the last outcome.
    last_at: Option<Instant>,
    /// When the running check started.
    pending_since: Option<Instant>,
    /// Stream file opens in progress (id -> start).
    opens: BTreeMap<u64, Instant>,
    next_open: u64,
    /// Checks started / finished so far (for [`RootMonitor::check_now`]).
    started: u64,
    finished: u64,
    /// Canonical path from the most recent success.
    canonical: Option<PathBuf>,
    /// A recheck was requested before the next periodic check.
    requested: bool,
    stopped: bool,
}

impl SlotState {
    fn store(&mut self, result: ProbeResult) {
        if let Ok(path) = &result {
            self.canonical = Some(path.clone());
        }
        self.last = Some((result, unix_now()));
        self.last_at = Some(Instant::now());
    }

    /// Start of the oldest check or file open still running that began after the last
    /// successful outcome (work that started earlier is answered by that success).
    fn oldest_pending(&self) -> Option<Instant> {
        let since = match (&self.last, self.last_at) {
            (Some((Ok(_), _)), Some(at)) => Some(at),
            _ => None,
        };
        self.pending_since
            .into_iter()
            .chain(self.opens.values().copied())
            .filter(|start| since.is_none_or(|since| *start >= since))
            .min()
    }
}

struct Slot {
    state: Mutex<SlotState>,
    /// Wakes the checker thread (recheck requested or monitor stopped).
    wake: Condvar,
    /// Signalled whenever a check finishes.
    done: Condvar,
}

/// Registration of one stream file open; dropping it ends the registration.
pub struct OpenGuard {
    slot: Arc<Slot>,
    id: u64,
}

impl Drop for OpenGuard {
    fn drop(&mut self) {
        self.slot.lock().opens.remove(&self.id);
    }
}

impl Slot {
    fn lock(&self) -> MutexGuard<'_, SlotState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Background checker and cache for all configured roots. Dropping it stops the checker
/// threads after their current check.
pub struct RootMonitor {
    roots: Vec<RootConfig>,
    policy: RootPolicy,
    slots: Vec<Arc<Slot>>,
}

impl RootMonitor {
    /// Start one checker thread per root with the default probe.
    pub fn start(roots: &[RootConfig], policy: RootPolicy) -> Self {
        Self::start_with_probe(roots, policy, Arc::new(probe_root))
    }

    /// Start one checker thread per root with a custom probe (tests simulate slow or
    /// failing disks with it).
    pub fn start_with_probe(roots: &[RootConfig], policy: RootPolicy, probe: RootProbe) -> Self {
        let slots: Vec<Arc<Slot>> = roots
            .iter()
            .map(|_| {
                Arc::new(Slot {
                    state: Mutex::new(SlotState::default()),
                    wake: Condvar::new(),
                    done: Condvar::new(),
                })
            })
            .collect();
        for (root, slot) in roots.iter().zip(&slots) {
            let slot = slot.clone();
            let probe = probe.clone();
            let name = root.name.clone();
            let path = root.server_path.clone();
            let interval = policy.interval;
            let spawned = std::thread::Builder::new()
                .name(format!("root-check-{name}"))
                .spawn(move || check_loop(&name, &path, &slot, probe.as_ref(), interval));
            if let Err(error) = spawned {
                tracing::error!(root = %root.name, %error, "Cannot start the root checker");
            }
        }
        Self {
            roots: roots.to_vec(),
            policy,
            slots,
        }
    }

    /// Configured roots in order.
    pub fn roots(&self) -> &[RootConfig] {
        &self.roots
    }

    /// Cached status of root `index`; never touches the filesystem.
    pub fn status(&self, index: usize) -> RootStatus {
        let state = self.slots[index].lock();
        let pending = state.oldest_pending().map(|since| since.elapsed());
        let canonical = state.canonical.clone();
        let (last_ok, failure, checked_at) = match &state.last {
            Some((Ok(_), at)) => (true, None, Some(*at)),
            Some((Err(message), at)) => (false, Some(message.clone()), Some(*at)),
            None => (false, None, None),
        };
        let (state, message) = match pending {
            Some(elapsed) if elapsed >= self.policy.offline_after => (
                RootState::Offline,
                Some(format!("Root has not answered for {} s", elapsed.as_secs())),
            ),
            _ if failure.is_some() => (RootState::Offline, failure),
            Some(elapsed) if last_ok && elapsed >= self.policy.waking_after => {
                (RootState::Waking, None)
            }
            _ if last_ok => (RootState::Online, None),
            _ => (RootState::Checking, None),
        };
        RootStatus {
            state,
            message,
            checked_at,
            canonical,
        }
    }

    /// Ask the checker of root `index` to run as soon as its current check (if any) ends.
    pub fn request_check(&self, index: usize) {
        let slot = &self.slots[index];
        slot.lock().requested = true;
        slot.wake.notify_all();
    }

    /// Ask for a recheck and wait up to `budget` for a check that starts after this call
    /// to finish; returns the status then. A hung check is not waited for beyond the
    /// budget (it keeps running in the background), so the result may still be the old
    /// state, `checking` or `waking`.
    pub fn check_now(&self, index: usize, budget: Duration) -> RootStatus {
        let slot = &self.slots[index];
        let deadline = Instant::now() + budget;
        {
            let mut state = slot.lock();
            let target = state.started + 1;
            state.requested = true;
            slot.wake.notify_all();
            while state.finished < target && !state.stopped {
                let now = Instant::now();
                if now >= deadline {
                    break;
                }
                state = slot
                    .done
                    .wait_timeout(state, deadline - now)
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .0;
            }
        }
        self.status(index)
    }

    /// Register a stream file open under root `index` until the guard is dropped. While
    /// it runs it counts like a pending check (`waking`, then `offline`).
    pub fn begin_open(&self, index: usize) -> OpenGuard {
        let slot = self.slots[index].clone();
        let id = {
            let mut state = slot.lock();
            let id = state.next_open;
            state.next_open += 1;
            state.opens.insert(id, Instant::now());
            id
        };
        OpenGuard { slot, id }
    }

    /// Stream file opens currently in progress under root `index`.
    pub fn opens_in_progress(&self, index: usize) -> usize {
        self.slots[index].lock().opens.len()
    }

    /// Record the outcome of real root access on the stream path (a successful open
    /// brings the root back online at once; a definite failure marks it offline).
    pub fn record(&self, index: usize, result: ProbeResult) {
        let mut state = self.slots[index].lock();
        log_transition(&self.roots[index].name, state.last.as_ref(), &result);
        state.store(result);
    }

    /// Index of the configured root whose `server_path` lexically contains `path`
    /// (component-wise; no filesystem access). The longest match wins.
    pub fn root_index_for(&self, path: &Path) -> Option<usize> {
        self.roots
            .iter()
            .enumerate()
            .filter(|(index, root)| {
                path.starts_with(&root.server_path)
                    || self.status(*index).canonical.is_some_and(|canonical| {
                        // Canonical file paths carry the verbatim prefix like the root's;
                        // library paths never do.
                        path.starts_with(&canonical)
                            || path.starts_with(without_verbatim(&canonical))
                    })
            })
            .max_by_key(|(_, root)| root.server_path.components().count())
            .map(|(index, _)| index)
    }
}

impl Drop for RootMonitor {
    fn drop(&mut self) {
        for slot in &self.slots {
            slot.lock().stopped = true;
            slot.wake.notify_all();
        }
    }
}

fn check_loop(
    name: &str,
    path: &Path,
    slot: &Slot,
    probe: &(dyn Fn(&Path) -> ProbeResult + Send + Sync),
    interval: Duration,
) {
    loop {
        {
            let mut state = slot.lock();
            if state.stopped {
                return;
            }
            state.requested = false;
            state.started += 1;
            state.pending_since = Some(Instant::now());
        }
        let result = probe(path);
        let mut state = slot.lock();
        log_transition(name, state.last.as_ref(), &result);
        state.store(result);
        state.pending_since = None;
        state.finished = state.started;
        slot.done.notify_all();
        let deadline = Instant::now() + interval;
        while !state.stopped && !state.requested {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            state = slot
                .wake
                .wait_timeout(state, deadline - now)
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .0;
        }
        if state.stopped {
            return;
        }
    }
}

fn log_transition(name: &str, previous: Option<&(ProbeResult, u64)>, next: &ProbeResult) {
    let was_ok = previous.map(|(result, _)| result.is_ok());
    match (was_ok, next) {
        (Some(true), Ok(_)) | (Some(false), Err(_)) => {}
        (_, Ok(_)) => tracing::info!(root = %name, "Library root is online"),
        (_, Err(message)) => tracing::warn!(root = %name, %message, "Library root is offline"),
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|age| age.as_secs())
        .unwrap_or_default()
}

/// `path` without the Windows verbatim prefix (`\\?\C:\x` -> `C:\x`, `\\?\UNC\h\s` ->
/// `\\h\s`) that `fs::canonicalize` adds on Windows, in the form the library stores
/// paths ([`audiolib_db::display_path`]). Every other path is returned unchanged, so this
/// is a no-op on macOS.
pub(crate) fn without_verbatim(path: &Path) -> Cow<'_, Path> {
    match path.to_str() {
        Some(text) if text.starts_with(r"\\?\") => {
            Cow::Owned(PathBuf::from(audiolib_db::display_path(path)))
        }
        _ => Cow::Borrowed(path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn root(name: &str, path: &Path) -> RootConfig {
        RootConfig {
            name: name.into(),
            server_path: path.to_path_buf(),
            webdav_url: None,
        }
    }

    fn wait_for(monitor: &RootMonitor, state: RootState) -> RootStatus {
        let start = Instant::now();
        loop {
            let status = monitor.status(0);
            if status.state == state {
                return status;
            }
            assert!(start.elapsed() < Duration::from_secs(5), "{status:?}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn policy() -> RootPolicy {
        RootPolicy {
            interval: Duration::from_secs(3600),
            waking_after: Duration::from_millis(100),
            offline_after: Duration::from_millis(400),
        }
    }

    #[test]
    fn real_directories_are_online_and_missing_ones_offline() {
        let dir = tempfile::tempdir().unwrap();
        let roots = [root("A", dir.path()), root("B", &dir.path().join("gone"))];
        let monitor = RootMonitor::start(&roots, policy());
        let online = wait_for(&monitor, RootState::Online);
        assert!(online.online() && online.checked_at.is_some());
        assert_eq!(online.canonical, Some(dir.path().canonicalize().unwrap()));
        let start = Instant::now();
        while monitor.status(1).state != RootState::Offline {
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(5));
        }
        let offline = monitor.status(1);
        assert!(!offline.online());
        assert!(offline.message.unwrap().contains("not found"));
        assert_eq!(
            monitor.root_index_for(&dir.path().join("x").join("y.wav")),
            Some(0)
        );
        assert_eq!(monitor.root_index_for(Path::new("/elsewhere/y.wav")), None);
    }

    #[test]
    fn slow_checks_wake_instead_of_going_offline_until_the_limit() {
        // The probe blocks until released, like a NAS disk spinning up.
        let gate = Arc::new((Mutex::new(0_usize), Condvar::new()));
        let calls = Arc::new(AtomicUsize::new(0));
        let probe: RootProbe = {
            let gate = gate.clone();
            let calls = calls.clone();
            Arc::new(move |path: &Path| {
                let call = calls.fetch_add(1, Ordering::SeqCst);
                if call > 0 {
                    let (lock, wake) = &*gate;
                    let mut released = lock.lock().unwrap();
                    while *released < call {
                        released = wake.wait(released).unwrap();
                    }
                }
                Ok(path.to_path_buf())
            })
        };
        let release = |count: usize| {
            *gate.0.lock().unwrap() = count;
            gate.1.notify_all();
        };
        let monitor =
            RootMonitor::start_with_probe(&[root("A", Path::new("/nas/A"))], policy(), probe);
        wait_for(&monitor, RootState::Online);
        monitor.request_check(0);
        // Pending but young: still online; then waking (still online); then offline.
        assert!(monitor.status(0).online());
        let waking = wait_for(&monitor, RootState::Waking);
        assert!(waking.online() && waking.message.is_none());
        let offline = wait_for(&monitor, RootState::Offline);
        assert!(offline.message.unwrap().contains("has not answered"));
        // A successful stream open proves the root is reachable even though the old
        // check still hangs.
        monitor.record(0, Ok(PathBuf::from("/nas/A")));
        assert_eq!(monitor.status(0).state, RootState::Online);
        // The check finally answers: online again without any request.
        release(1);
        wait_for(&monitor, RootState::Online);
        // Online was already cached by record() above. Wait for the released check
        // to finish so its late success cannot overwrite the next simulated failure.
        let deadline = Instant::now() + Duration::from_secs(2);
        while monitor.slots[0].lock().finished < 2 {
            assert!(
                Instant::now() < deadline,
                "Released root check did not finish"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        // A failed stream open marks it offline, a successful one brings it back.
        monitor.record(
            0,
            Err("Root directory not found (volume not mounted?)".into()),
        );
        assert_eq!(monitor.status(0).state, RootState::Offline);
        monitor.record(0, Ok(PathBuf::from("/nas/A")));
        assert_eq!(monitor.status(0).state, RootState::Online);
        release(usize::MAX);
    }

    /// A root configured through another spelling (`alias`) whose canonical path is
    /// `canonical`: library paths (no verbatim prefix) and canonical file paths below it
    /// both map to it, paths elsewhere do not.
    fn assert_canonical_root_matches(
        alias: &str,
        canonical: &str,
        library_file: &str,
        canonical_file: &str,
        alias_file: &str,
        elsewhere: &str,
    ) {
        let canonical = PathBuf::from(canonical);
        let probe: RootProbe = Arc::new(move |_: &Path| Ok(canonical.clone()));
        let monitor =
            RootMonitor::start_with_probe(&[root("A", Path::new(alias))], policy(), probe);
        wait_for(&monitor, RootState::Online);
        for (path, expected) in [
            (library_file, Some(0)),
            (canonical_file, Some(0)),
            (alias_file, Some(0)),
            (elsewhere, None),
        ] {
            assert_eq!(monitor.root_index_for(Path::new(path)), expected, "{path}");
        }
    }

    /// What `fs::canonicalize` returns on Windows (`\\?\C:\…`, `\\?\UNC\host\share\…`)
    /// against what the library stores (`C:\…`, `\\host\share\…`). In a verbatim path
    /// only `\` separates components, so these inputs only parse as intended on Windows.
    #[cfg(windows)]
    #[test]
    fn library_paths_match_a_root_known_only_by_its_verbatim_canonical_path() {
        assert_canonical_root_matches(
            r"C:\srv\alias\A",
            r"\\?\C:\srv\real\A",
            r"C:\srv\real\A\x.wav",
            r"\\?\C:\srv\real\A\x.wav",
            r"C:\srv\alias\A\x.wav",
            r"C:\srv\real\B\x.wav",
        );
        assert_canonical_root_matches(
            r"\\alias\share\A",
            r"\\?\UNC\nas\share\A",
            r"\\nas\share\A\x.wav",
            r"\\?\UNC\nas\share\A\x.wav",
            r"\\alias\share\A\x.wav",
            r"\\nas\share\B\x.wav",
        );
    }

    /// The same shape with Unix paths behind a `\\?\` marker, which is an ordinary
    /// first component outside Windows: exercises the prefix stripping on every platform.
    #[cfg(not(windows))]
    #[test]
    fn library_paths_match_a_root_known_only_by_its_verbatim_canonical_path() {
        assert_canonical_root_matches(
            "/srv/alias/A",
            r"\\?\/srv/real/A",
            "/srv/real/A/x.wav",
            r"\\?\/srv/real/A/x.wav",
            "/srv/alias/A/x.wav",
            "/srv/real/B/x.wav",
        );
    }

    /// Pure string logic: runs on every platform.
    #[test]
    fn without_verbatim_drops_disk_and_unc_prefixes_only() {
        for (input, expected) in [
            (r"\\?\C:\Sounds", r"C:\Sounds"),
            (r"\\?\C:\Lib\A\x.wav", r"C:\Lib\A\x.wav"),
            (r"\\?\UNC\nas\share", r"\\nas\share"),
            (r"\\?\UNC\server\share\A", r"\\server\share\A"),
            (r"C:\Sounds", r"C:\Sounds"),
            (r"\\server\share\A", r"\\server\share\A"),
            ("/Volumes/A", "/Volumes/A"),
        ] {
            let stripped = without_verbatim(Path::new(input));
            assert_eq!(stripped.to_str(), Some(expected), "{input}");
            assert_eq!(
                matches!(stripped, Cow::Borrowed(_)),
                input == expected,
                "{input}: borrowed only when unchanged"
            );
        }
    }

    #[test]
    fn first_check_reports_checking_while_pending() {
        let (sender, receiver) = std::sync::mpsc::channel::<()>();
        let receiver = Mutex::new(receiver);
        let probe: RootProbe = Arc::new(move |path: &Path| {
            let _ = receiver.lock().unwrap().recv();
            Ok(path.to_path_buf())
        });
        let monitor = RootMonitor::start_with_probe(
            &[root("A", Path::new("/nas/A"))],
            RootPolicy {
                waking_after: Duration::from_millis(10),
                ..policy()
            },
            probe,
        );
        std::thread::sleep(Duration::from_millis(50));
        let status = monitor.status(0);
        assert_eq!(
            status.state,
            RootState::Checking,
            "never waking before a first result"
        );
        assert!(status.online() && status.checked_at.is_none());
        sender.send(()).unwrap();
        wait_for(&monitor, RootState::Online);
        drop(sender);
    }

    #[test]
    fn check_now_waits_for_a_fresh_check_within_its_budget() {
        let online = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let hang = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let probe: RootProbe = {
            let (online, hang) = (online.clone(), hang.clone());
            Arc::new(move |path: &Path| {
                while hang.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(5));
                }
                if online.load(Ordering::SeqCst) {
                    Ok(path.to_path_buf())
                } else {
                    Err("Root directory not found (volume not mounted?)".into())
                }
            })
        };
        let monitor =
            RootMonitor::start_with_probe(&[root("A", Path::new("/nas/A"))], policy(), probe);
        wait_for(&monitor, RootState::Offline);
        // The volume is back: an inline recheck sees it at once, no periodic wait.
        online.store(true, Ordering::SeqCst);
        let status = monitor.check_now(0, Duration::from_secs(5));
        assert_eq!(status.state, RootState::Online);
        // A hung check never holds the caller longer than the budget.
        hang.store(true, Ordering::SeqCst);
        online.store(false, Ordering::SeqCst);
        monitor.request_check(0);
        let started = Instant::now();
        let status = monitor.check_now(0, Duration::from_millis(150));
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(
            status.online(),
            "unresolved: still the last good state {status:?}"
        );
        hang.store(false, Ordering::SeqCst);
        wait_for(&monitor, RootState::Offline);
    }

    #[test]
    fn hung_stream_opens_turn_a_root_waking_then_offline() {
        let dir = tempfile::tempdir().unwrap();
        let monitor = RootMonitor::start(&[root("A", dir.path())], policy());
        wait_for(&monitor, RootState::Online);
        let guard = monitor.begin_open(0);
        assert_eq!(monitor.opens_in_progress(0), 1);
        wait_for(&monitor, RootState::Waking);
        let offline = wait_for(&monitor, RootState::Offline);
        assert!(offline.message.unwrap().contains("has not answered"));
        drop(guard);
        assert_eq!(monitor.opens_in_progress(0), 0);
        assert_eq!(monitor.status(0).state, RootState::Online);
        // An open that started before a later success no longer counts.
        let old = monitor.begin_open(0);
        monitor.record(0, Ok(dir.path().to_path_buf()));
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(monitor.status(0).state, RootState::Online);
        drop(old);
    }

    #[test]
    fn network_failures_are_transient_and_bad_files_are_not() {
        use io::ErrorKind::*;
        for kind in [
            TimedOut,
            NotConnected,
            ConnectionReset,
            HostUnreachable,
            NetworkDown,
        ] {
            assert!(
                is_transient_network_error(&io::Error::from(kind)),
                "{kind:?}"
            );
        }
        for kind in [NotFound, PermissionDenied, InvalidData, UnexpectedEof] {
            assert!(
                !is_transient_network_error(&io::Error::from(kind)),
                "{kind:?}"
            );
        }
        #[cfg(target_os = "macos")]
        for errno in [60, 57, 70] {
            // ETIMEDOUT, ENOTCONN and ESTALE as an SMB mount reports them.
            let error = io::Error::from_raw_os_error(errno);
            assert!(is_transient_network_error(&error), "{error}");
        }
    }
}
