// crates/audiolib-remote/src/stream.rs
//! In-memory HTTP streaming of server audio for audition (no file on disk).
//!
//! [`MemoryStream::start`] fetches `GET <url>` on its own thread into fixed-size chunks
//! of an entry in a process-wide [`MemoryCache`], while [`MemoryReader`]s (`Read + Seek`,
//! one per decoder) read whatever has arrived. The decoder never has to wait on the
//! network: the audio engine asks [`MemoryStream::is_buffered`] before decoding and
//! pauses ("buffering") instead of blocking. A reader that needs bytes far from the region
//! being received asks the thread to re-request from there (`Range: bytes=<offset>-`).
//!
//! * **Window.** An entry holds at most [`MemoryCache::window`] bytes of a large file
//!   (half the budget, at most 256 MiB): the thread fetches up to ¾ window ahead of the
//!   reader and drops chunks more than ¼ window behind it (the first and the last two
//!   chunks, with the WAV header and trailing chunks, are kept). Ahead of the window the
//!   response is held for up to 15 s, then closed and re-requested with `Range` later. A
//!   file that fits the window is kept whole.
//! * **LRU.** Entries are keyed by the caller (record ID, file version and route) and bound
//!   to the server's opaque `ETag`: a reused entry asks with `If-Range`, and a different
//!   version (`200` instead of `206`, or another `ETag`/length) replaces the old bytes.
//!   Whole entries nobody uses are evicted, least recently used first, when the cache is
//!   above its budget ([`DEFAULT_MEMORY_CACHE_BYTES`], configurable).
//! * **Cancel.** [`MemoryStream::cancel`] (or dropping the last handle) shuts the
//!   connection down at once, also while it waits for the first byte; the bytes received so
//!   far stay cached for a later selection.
//! * **FLAC preview** ([`StreamSource::flac`]): the chunked `codec=flac` response is decoded
//!   on the stream thread into a 16/24-bit PCM WAV with a 44-byte header, so the player sees
//!   an ordinary seekable WAV. Its length comes from the record's duration (the stream
//!   carries none); the decoded stream is cut or padded with silence to it. A seek to
//!   output frame `k` re-requests `t=<k / rate>` and writes from `44 + X-Start-Frame ×
//!   block_align`.
use crate::abort::{Abort, Timeouts, abortable_stream_agent};
use crate::bandwidth::{Throughput, bandwidth};
use crate::client::{agent, retry_after};
use crate::error::{RemoteError, Result};
use crate::util::base64;
use std::collections::HashMap;
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

/// Default wait for the response (first byte) of a stream or download request. The
/// server waits up to 90 s for a NAS disk to spin up and the file to open, so the client
/// waits longer; a slow first byte is a waking disk, not an offline server.
pub const FIRST_BYTE_TIMEOUT: Duration = Duration::from_secs(100);
/// After this long without a first byte, callers show "waking the library disk…".
pub const WAKING_HINT_AFTER: Duration = Duration::from_millis(1500);
/// Upper bound of the distance ahead of the byte being received that a reader waits for
/// rather than starting a new range request (see [`seek_jump_bytes`]).
pub const SEEK_JUMP_BYTES: u64 = 1024 * 1024;
/// Lower bound of that distance.
const MIN_SEEK_JUMP_BYTES: u64 = 64 * 1024;
/// An audition file up to this size is fetched whole as fast as the link allows.
pub const WHOLE_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// Larger files are fetched this far ahead of the player (see [`StreamSource::read_ahead`]).
pub const READ_AHEAD_SECONDS: u64 = 60;
/// A read fails when it waits this long without any new data arriving.
pub const READ_STALL_TIMEOUT: Duration = Duration::from_secs(30);
/// Retries for transient failures: connection lost, timeout and `503` after the first
/// response; `BUSY` (the server's stream cap, `503` + `Retry-After`) also before it.
pub const STREAM_RETRIES: u32 = 3;
/// Default budget of the audition memory cache: 512 MiB.
pub const DEFAULT_MEMORY_CACHE_BYTES: u64 = 512 * 1024 * 1024;
/// Smallest accepted budget (a few seconds of any file plus the pinned chunks).
pub const MIN_MEMORY_CACHE_BYTES: u64 = 32 * 1024 * 1024;
/// Upper bound for one retry wait, whatever `Retry-After` asks for.
const MAX_RETRY_WAIT: Duration = Duration::from_secs(10);
const BUFFER_BYTES: usize = 64 * 1024;
const WAIT_SLICE: Duration = Duration::from_millis(100);
/// Allocation unit of an entry; eviction inside the window works on whole chunks.
const CHUNK_BYTES: u64 = 256 * 1024;
/// Largest part of one file kept in memory.
const MAX_WINDOW_BYTES: u64 = 256 * 1024 * 1024;
/// How long a response ahead of the window is kept open while the reader catches up.
const HOLD_RESPONSE: Duration = Duration::from_secs(15);
/// Header of the PCM WAV a FLAC preview is decoded into.
const WAV_HEADER_BYTES: u64 = 44;

/// WebDAV Basic credentials; `Debug` never shows the password.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct WebDavAuth {
    pub username: String,
    pub password: String,
}

impl std::fmt::Debug for WebDavAuth {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WebDavAuth")
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish()
    }
}

impl WebDavAuth {
    /// `Authorization` header value, or `None` without a username.
    pub fn header(&self) -> Option<String> {
        (!self.username.is_empty()).then(|| {
            format!(
                "Basic {}",
                base64(format!("{}:{}", self.username, self.password).as_bytes())
            )
        })
    }
}

/// Credentials for a stream source; `Debug` never shows secrets.
#[derive(Clone, Default, PartialEq, Eq)]
pub enum SourceAuth {
    #[default]
    None,
    /// The server API token (`Authorization: Bearer`).
    Bearer(String),
    /// WebDAV Basic credentials.
    Basic(WebDavAuth),
}

impl SourceAuth {
    pub(crate) fn header(&self) -> Option<String> {
        match self {
            Self::None => None,
            Self::Bearer(token) => Some(format!("Bearer {token}")),
            Self::Basic(auth) => auth.header(),
        }
    }
}

impl std::fmt::Debug for SourceAuth {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => formatter.write_str("None"),
            Self::Bearer(_) => formatter.write_str("Bearer(<redacted>)"),
            Self::Basic(auth) => formatter.debug_tuple("Basic").field(auth).finish(),
        }
    }
}

/// Where a stream reads from: the server's `/audio/{id}/stream` or `/preview`, or a
/// WebDAV URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamSource {
    pub url: String,
    pub auth: SourceAuth,
    /// Wait for the response of each request ([`FIRST_BYTE_TIMEOUT`] by default).
    pub first_byte_timeout: Duration,
    /// `Some(duration)` for a `codec=flac` preview: decoded into a PCM WAV of that
    /// duration (the record's), positioned with `t=`.
    pub flac_duration: Option<Duration>,
    /// Bytes to fetch ahead of the player in a file larger than [`WHOLE_FILE_BYTES`]
    /// (about [`READ_AHEAD_SECONDS`] of audio); `None` uses the cache window.
    pub read_ahead: Option<u64>,
}

impl StreamSource {
    pub fn new(url: impl Into<String>, auth: SourceAuth) -> Self {
        Self {
            url: url.into(),
            auth,
            first_byte_timeout: FIRST_BYTE_TIMEOUT,
            flac_duration: None,
            read_ahead: None,
        }
    }

    /// A `codec=flac` preview URL (with its query) of a record lasting `duration`.
    pub fn flac(url: impl Into<String>, auth: SourceAuth, duration: Duration) -> Self {
        Self {
            flac_duration: Some(duration),
            ..Self::new(url, auth)
        }
    }

    pub fn with_first_byte_timeout(mut self, timeout: Duration) -> Self {
        self.first_byte_timeout = timeout;
        self
    }
}

/// Output format of a `/preview` response (`X-Preview-*` headers, §8.1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PreviewFormat {
    pub sample_rate: u32,
    pub bits: u16,
    pub channels: u16,
    /// `X-Preview-Frames` (PCM only).
    pub frames: Option<u64>,
}

impl PreviewFormat {
    fn from_headers(headers: &ureq::http::HeaderMap) -> Option<Self> {
        let number = |name: &str| header(headers, name).and_then(|value| value.parse::<u64>().ok());
        Some(Self {
            sample_rate: u32::try_from(number("x-preview-sample-rate")?).ok()?,
            bits: u16::try_from(number("x-preview-bits")?).ok()?,
            channels: u16::try_from(number("x-preview-channels")?).ok()?,
            frames: number("x-preview-frames"),
        })
    }

    fn block_align(&self) -> u64 {
        u64::from(self.channels) * u64::from(self.bits / 8)
    }
}

/// Merged, sorted half-open byte intervals that are in memory.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Coverage(Vec<(u64, u64)>);

impl Coverage {
    fn insert(&mut self, start: u64, end: u64) {
        if start >= end {
            return;
        }
        let (mut start, mut end) = (start, end);
        self.0.retain(|&(a, b)| {
            if b < start || a > end {
                true
            } else {
                start = start.min(a);
                end = end.max(b);
                false
            }
        });
        let index = self.0.partition_point(|&(a, _)| a < start);
        self.0.insert(index, (start, end));
    }

    fn remove(&mut self, start: u64, end: u64) {
        if start >= end {
            return;
        }
        let mut kept = Vec::with_capacity(self.0.len() + 1);
        for &(a, b) in &self.0 {
            if b <= start || a >= end {
                kept.push((a, b));
                continue;
            }
            if a < start {
                kept.push((a, start));
            }
            if b > end {
                kept.push((end, b));
            }
        }
        self.0 = kept;
    }

    /// End of the interval containing `offset`.
    fn end_of(&self, offset: u64) -> Option<u64> {
        self.0
            .iter()
            .find(|&&(a, b)| a <= offset && offset < b)
            .map(|&(_, b)| b)
    }

    fn contains(&self, offset: u64) -> bool {
        self.end_of(offset).is_some()
    }

    /// First missing offset at or after `from` (below `total`).
    fn next_gap(&self, from: u64, total: u64) -> Option<u64> {
        let mut at = from;
        for &(a, b) in &self.0 {
            if a <= at && at < b {
                at = b;
            }
        }
        (at < total).then_some(at)
    }

    fn complete(&self, total: u64) -> bool {
        total == 0 || self.0.first().is_some_and(|&(a, b)| a == 0 && b >= total)
    }
}

/// One running fetch thread of an entry.
struct Worker {
    cancel: AtomicBool,
    /// Shuts the connection down on cancel, even while a request still waits for its
    /// response.
    abort: Abort,
}

struct State {
    total: Option<u64>,
    etag: Option<String>,
    ranges: bool,
    covered: Coverage,
    /// `CHUNK_BYTES` pieces by index; a chunk exists once any byte of it arrived.
    chunks: HashMap<u64, Box<[u8]>>,
    /// Offset a reader (or the player's buffer check) needs and the worker should fetch
    /// next.
    wanted: Option<u64>,
    /// Where the player reads (outside the pinned chunks); the window follows it.
    reader_at: u64,
    failure: Option<RemoteError>,
    /// Bumped on every write so readers can tell a slow stream from a stalled one.
    progress: u64,
    /// Bumped by [`MemoryStream::interrupt_reads`]: reads waiting at that moment fail.
    read_epoch: u64,
    preview: Option<PreviewFormat>,
    worker: Option<Arc<Worker>>,
    /// Set by [`MemoryStream::cancel`] until the entry is started again.
    cancelled: bool,
}

impl State {
    fn complete(&self) -> bool {
        self.total.is_some_and(|total| self.covered.complete(total))
    }
}

struct Entry {
    key: String,
    source: StreamSource,
    state: Mutex<State>,
    signal: Condvar,
    /// Bytes allocated by the chunks (read by the cache without the state lock).
    bytes: AtomicU64,
    /// [`MemoryStream`] handles; the last one going away stops the fetch.
    handles: AtomicUsize,
    /// Open [`MemoryReader`]s; an entry in use is never evicted.
    readers: AtomicUsize,
    last_used: AtomicU64,
    window: u64,
}

impl Entry {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn wait<'a>(&self, state: MutexGuard<'a, State>) -> MutexGuard<'a, State> {
        self.signal
            .wait_timeout(state, WAIT_SLICE)
            .unwrap_or_else(PoisonError::into_inner)
            .0
    }

    fn in_use(&self) -> bool {
        self.handles.load(Ordering::Acquire) > 0 || self.readers.load(Ordering::Acquire) > 0
    }

    /// Byte range the entry keeps around the reader: everything for a file that fits.
    /// The byte range kept around the player: a file up to [`WHOLE_FILE_BYTES`] (and the
    /// window) entirely, else up to the source's read-ahead (at most 3/4 window) ahead and
    /// 1/4 window behind. Beyond it the fetch pauses (TCP backpressure, then a close).
    fn bounds(&self, state: &State, total: u64) -> (u64, u64) {
        if total <= self.window.min(WHOLE_FILE_BYTES) {
            return (0, total);
        }
        let ahead = (self.window * 3 / 4).min(
            self.source
                .read_ahead
                .map_or(u64::MAX, |ahead| ahead.max(4 * CHUNK_BYTES)),
        );
        let low = state.reader_at.saturating_sub(self.window / 4);
        let high = (state.reader_at + ahead).min(total);
        (low, high)
    }

    fn pinned(index: u64, total: u64) -> bool {
        let last = total.saturating_sub(1) / CHUNK_BYTES;
        index == 0 || index + 1 >= last
    }

    /// Note where the player reads (reads of the pinned header/trailer chunks don't move
    /// the window).
    fn note_reader(state: &mut State, offset: u64) {
        if let Some(total) = state.total
            && !Self::pinned(offset / CHUNK_BYTES, total)
        {
            state.reader_at = offset;
        }
    }

    /// Copy `data` into the chunks at `offset` (clamped to the length) and mark it present.
    fn write(&self, state: &mut State, offset: u64, data: &[u8]) {
        let Some(total) = state.total else {
            return;
        };
        let end = (offset + data.len() as u64).min(total);
        let mut at = offset;
        let mut added = 0;
        while at < end {
            let index = at / CHUNK_BYTES;
            let chunk_start = index * CHUNK_BYTES;
            let chunk_len = CHUNK_BYTES.min(total - chunk_start);
            let chunk = state.chunks.entry(index).or_insert_with(|| {
                added += chunk_len;
                vec![0_u8; chunk_len as usize].into_boxed_slice()
            });
            let within = (at - chunk_start) as usize;
            let len = ((end - at) as usize).min(chunk.len() - within);
            let source = (at - offset) as usize;
            chunk[within..within + len].copy_from_slice(&data[source..source + len]);
            at += len as u64;
        }
        state.covered.insert(offset, end);
        state.progress += 1;
        self.bytes.fetch_add(added, Ordering::AcqRel);
        self.trim_window(state, total);
        self.signal.notify_all();
    }

    /// Drop chunks outside the window until the entry fits it again.
    fn trim_window(&self, state: &mut State, total: u64) {
        // Without `Range` a dropped chunk could never be fetched again.
        if !state.ranges
            || self.bytes.load(Ordering::Acquire) <= self.window
            || total <= self.window
        {
            return;
        }
        let (low, high) = self.bounds(state, total);
        let mut outside: Vec<(u64, u64)> = state
            .chunks
            .keys()
            .copied()
            .filter(|&index| !Self::pinned(index, total))
            .filter_map(|index| {
                let start = index * CHUNK_BYTES;
                let end = (start + CHUNK_BYTES).min(total);
                if end <= low {
                    // Behind the reader: drop first, oldest first.
                    Some((u64::MAX - (low - end), index))
                } else if start >= high {
                    Some((start - high, index))
                } else {
                    None
                }
            })
            .collect();
        outside.sort_unstable_by(|a, b| b.cmp(a));
        for (_, index) in outside {
            if self.bytes.load(Ordering::Acquire) <= self.window {
                break;
            }
            if let Some(chunk) = state.chunks.remove(&index) {
                let start = index * CHUNK_BYTES;
                state.covered.remove(start, start + chunk.len() as u64);
                self.bytes.fetch_sub(chunk.len() as u64, Ordering::AcqRel);
            }
        }
    }

    /// Forget every byte (another file version, or evicted from the cache).
    fn clear(&self, state: &mut State) {
        state.chunks.clear();
        state.covered = Coverage::default();
        state.total = None;
        state.etag = None;
        state.preview = None;
        state.ranges = false;
        self.bytes.store(0, Ordering::Release);
    }

    /// Copy what is available at `offset` into `buffer`, waiting for missing bytes.
    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> io::Result<usize> {
        let mut state = self.lock();
        let mut progress = state.progress;
        let epoch = state.read_epoch;
        let mut deadline = Instant::now() + READ_STALL_TIMEOUT;
        loop {
            let Some(total) = state.total else {
                return Err(io::Error::other("audio stream not ready"));
            };
            if offset >= total {
                return Ok(0);
            }
            if let Some(end) = state.covered.end_of(offset) {
                Self::note_reader(&mut state, offset);
                let index = offset / CHUNK_BYTES;
                let chunk = state
                    .chunks
                    .get(&index)
                    .ok_or_else(|| io::Error::other("audio stream chunk missing"))?;
                let within = (offset - index * CHUNK_BYTES) as usize;
                let len = ((end - offset) as usize)
                    .min(chunk.len() - within)
                    .min(buffer.len());
                buffer[..len].copy_from_slice(&chunk[within..within + len]);
                return Ok(len);
            }
            if state.read_epoch != epoch {
                return Err(audiolib_core::read_interrupted_error());
            }
            if let Some(failure) = &state.failure {
                return Err(io::Error::other(failure.to_string()));
            }
            if state.cancelled || state.worker.is_none() {
                return Err(io::Error::other("audio stream cancelled"));
            }
            Self::note_reader(&mut state, offset);
            if state.ranges && state.wanted != Some(offset) {
                state.wanted = Some(offset);
                self.signal.notify_all();
            }
            if state.progress != progress {
                progress = state.progress;
                deadline = Instant::now() + READ_STALL_TIMEOUT;
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("no audio data from {} in time", self.source.url),
                ));
            }
            state = self.wait(state);
        }
    }

    /// Whether `offset..offset + len` (clamped to the length) is in memory. When it is not,
    /// the fetch is pointed at it, so a player that waits instead of reading still steers
    /// the download (a seek, the window moving on).
    fn is_buffered(&self, offset: u64, len: u64) -> bool {
        let mut state = self.lock();
        let Some(total) = state.total else {
            return false;
        };
        let end = offset.saturating_add(len).min(total);
        if offset >= end {
            return true;
        }
        Self::note_reader(&mut state, offset);
        match state.covered.end_of(offset) {
            Some(available) if available >= end => true,
            available => {
                let gap = available.unwrap_or(offset);
                if state.ranges && state.wanted != Some(gap) {
                    state.wanted = Some(gap);
                    self.signal.notify_all();
                }
                false
            }
        }
    }
}

struct CacheInner {
    budget: AtomicU64,
    entries: Mutex<Vec<Arc<Entry>>>,
    tick: AtomicU64,
}

/// Process-wide LRU of audition streams held in memory (see the module docs).
#[derive(Clone)]
pub struct MemoryCache {
    inner: Arc<CacheInner>,
}

impl std::fmt::Debug for MemoryCache {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MemoryCache")
            .field("budget", &self.budget())
            .field("used", &self.used_bytes())
            .finish()
    }
}

impl MemoryCache {
    /// A cache holding at most `budget` bytes (at least [`MIN_MEMORY_CACHE_BYTES`]).
    pub fn new(budget: u64) -> Self {
        Self {
            inner: Arc::new(CacheInner {
                budget: AtomicU64::new(budget.max(MIN_MEMORY_CACHE_BYTES)),
                entries: Mutex::new(Vec::new()),
                tick: AtomicU64::new(0),
            }),
        }
    }

    /// The shared cache of the process ([`DEFAULT_MEMORY_CACHE_BYTES`] until
    /// [`Self::set_budget`]).
    pub fn global() -> &'static MemoryCache {
        static GLOBAL: OnceLock<MemoryCache> = OnceLock::new();
        GLOBAL.get_or_init(|| MemoryCache::new(DEFAULT_MEMORY_CACHE_BYTES))
    }

    /// Change the budget; entries above it are evicted at once (unless in use).
    pub fn set_budget(&self, budget: u64) {
        self.inner
            .budget
            .store(budget.max(MIN_MEMORY_CACHE_BYTES), Ordering::Release);
        self.trim();
    }

    pub fn budget(&self) -> u64 {
        self.inner.budget.load(Ordering::Acquire)
    }

    /// Largest part of one file held at a time: half the budget, at most 256 MiB.
    pub fn window(&self) -> u64 {
        (self.budget() / 2).clamp(4 * CHUNK_BYTES, MAX_WINDOW_BYTES)
    }

    /// Bytes currently allocated by all entries.
    pub fn used_bytes(&self) -> u64 {
        self.entries()
            .iter()
            .map(|entry| entry.bytes.load(Ordering::Acquire))
            .sum()
    }

    pub fn len(&self) -> usize {
        self.entries().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Drop every entry nobody uses.
    pub fn clear(&self) {
        let removed: Vec<Arc<Entry>> = {
            let mut entries = self.entries();
            let (removed, kept) = entries.drain(..).partition(|entry| !entry.in_use());
            *entries = kept;
            removed
        };
        for entry in removed {
            entry.clear(&mut entry.lock());
        }
    }

    /// Drop the entries nobody uses whose key matches, e.g. those of records a server
    /// removed (an entry in use goes with its last reader, like any eviction).
    pub fn remove_where(&self, mut matches: impl FnMut(&str) -> bool) {
        let removed: Vec<Arc<Entry>> = {
            let mut entries = self.entries();
            let (removed, kept) = entries
                .drain(..)
                .partition(|entry| !entry.in_use() && matches(&entry.key));
            *entries = kept;
            removed
        };
        for entry in removed {
            entry.clear(&mut entry.lock());
        }
    }

    fn entries(&self) -> MutexGuard<'_, Vec<Arc<Entry>>> {
        self.inner
            .entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn touch(&self, entry: &Entry) {
        entry.last_used.store(
            self.inner.tick.fetch_add(1, Ordering::AcqRel) + 1,
            Ordering::Release,
        );
    }

    /// Evict whole least-recently-used entries nobody uses until the cache fits its budget.
    /// Lock order: the cache list, then an entry.
    fn trim(&self) {
        let budget = self.budget();
        let removed = {
            let mut entries = self.entries();
            let mut used: u64 = entries
                .iter()
                .map(|entry| entry.bytes.load(Ordering::Acquire))
                .sum();
            let mut candidates: Vec<(u64, usize)> = entries
                .iter()
                .enumerate()
                .filter(|(_, entry)| !entry.in_use())
                .map(|(index, entry)| (entry.last_used.load(Ordering::Acquire), index))
                .collect();
            candidates.sort_unstable();
            let mut evict = Vec::new();
            for (_, index) in candidates {
                if used <= budget {
                    break;
                }
                used = used.saturating_sub(entries[index].bytes.load(Ordering::Acquire));
                evict.push(index);
            }
            evict.sort_unstable_by(|a, b| b.cmp(a));
            evict
                .into_iter()
                .map(|index| entries.remove(index))
                .collect::<Vec<_>>()
        };
        for entry in removed {
            tracing::debug!(key = %entry.key, "audition stream evicted from memory");
            entry.clear(&mut entry.lock());
        }
    }

    /// Copy a complete in-memory copy of `url` (an original-file stream, not a preview)
    /// whose `ETag` is `etag` into `output`; `false` when there is none.
    pub fn copy_complete(
        &self,
        url: &str,
        etag: &str,
        length: Option<u64>,
        output: &mut dyn io::Write,
    ) -> io::Result<bool> {
        let found = self
            .entries()
            .iter()
            .find(|entry| entry.source.url == url && entry.source.flac_duration.is_none())
            .cloned();
        let Some(entry) = found else {
            return Ok(false);
        };
        let state = entry.lock();
        let Some(total) = state.total else {
            return Ok(false);
        };
        if !state.complete()
            || state.etag.as_deref() != Some(etag)
            || state.preview.is_some()
            || length.is_some_and(|length| length != total)
        {
            return Ok(false);
        }
        for index in 0..total.div_ceil(CHUNK_BYTES) {
            let chunk = state
                .chunks
                .get(&index)
                .ok_or_else(|| io::Error::other("audio stream chunk missing"))?;
            output.write_all(chunk)?;
        }
        Ok(true)
    }
}

/// An audition stream held in a [`MemoryCache`]. Clones share the same fetch; dropping the
/// last clone stops it like [`Self::cancel`].
#[derive(Clone)]
pub struct MemoryStream {
    handle: Arc<Handle>,
}

struct Handle {
    entry: Arc<Entry>,
    cache: MemoryCache,
}

impl Drop for Handle {
    fn drop(&mut self) {
        if self.entry.handles.fetch_sub(1, Ordering::AcqRel) == 1 {
            stop_worker(&self.entry);
        }
        self.cache.trim();
    }
}

fn stop_worker(entry: &Entry) {
    let mut state = entry.lock();
    if let Some(worker) = state.worker.take() {
        worker.cancel.store(true, Ordering::Release);
        worker.abort.abort();
        crate::abort::warm_spare(&entry.source.url);
    }
    entry.signal.notify_all();
}

impl std::fmt::Debug for MemoryStream {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MemoryStream")
            .field("key", &self.handle.entry.key)
            .field("url", &self.handle.entry.source.url)
            .finish()
    }
}

impl MemoryStream {
    /// Stream `source` into the entry `key` of `cache`, reusing what an earlier stream of
    /// the same key left there (revalidated by the `ETag` on the next request).
    pub fn start(cache: &MemoryCache, key: &str, source: StreamSource) -> Result<Self> {
        let entry = {
            let mut entries = cache.entries();
            let existing = entries
                .iter()
                .position(|entry| entry.key == key)
                .map(|index| entries[index].clone());
            match existing {
                Some(entry) if entry.source == source => entry,
                existing => {
                    if let Some(old) = existing {
                        entries.retain(|entry| !Arc::ptr_eq(entry, &old));
                        if !old.in_use() {
                            old.clear(&mut old.lock());
                        }
                    }
                    let entry = Arc::new(Entry {
                        key: key.to_owned(),
                        source,
                        state: Mutex::new(State {
                            total: None,
                            etag: None,
                            ranges: false,
                            covered: Coverage::default(),
                            chunks: HashMap::new(),
                            wanted: None,
                            reader_at: 0,
                            failure: None,
                            progress: 0,
                            read_epoch: 0,
                            preview: None,
                            worker: None,
                            cancelled: false,
                        }),
                        signal: Condvar::new(),
                        bytes: AtomicU64::new(0),
                        handles: AtomicUsize::new(0),
                        readers: AtomicUsize::new(0),
                        last_used: AtomicU64::new(0),
                        window: cache.window(),
                    });
                    entries.push(entry.clone());
                    entry
                }
            }
        };
        entry.handles.fetch_add(1, Ordering::AcqRel);
        cache.touch(&entry);
        let stream = Self {
            handle: Arc::new(Handle {
                entry: entry.clone(),
                cache: cache.clone(),
            }),
        };
        let worker = {
            let mut state = entry.lock();
            state.cancelled = false;
            state.failure = None;
            state.reader_at = 0;
            state.wanted = None;
            if state.complete() || state.worker.is_some() {
                None
            } else {
                let worker = Arc::new(Worker {
                    cancel: AtomicBool::new(false),
                    abort: Abort::default(),
                });
                state.worker = Some(worker.clone());
                Some(worker)
            }
        };
        if let Some(worker) = worker {
            let (job_entry, job_cache, job_worker) = (entry.clone(), cache.clone(), worker.clone());
            let spawned = std::thread::Builder::new()
                .name("audiolib-memory-stream".into())
                .spawn(move || run(&job_entry, &job_cache, &job_worker));
            if let Err(error) = spawned {
                entry.lock().worker = None;
                return Err(error.into());
            }
        }
        Ok(stream)
    }

    fn entry(&self) -> &Entry {
        &self.handle.entry
    }

    pub fn key(&self) -> &str {
        &self.entry().key
    }

    pub fn source(&self) -> &StreamSource {
        &self.entry().source
    }

    /// Whether both handles own the same cached bytes and cancellation state.
    /// Restarting an incomplete source can reuse its entry, even when a caller
    /// gives the new playback registration a different virtual path.
    pub fn shares_buffer_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.handle.entry, &other.handle.entry)
    }

    /// Wait for the first response: the total length, or why the stream failed.
    pub fn wait_ready(&self, cancel: &AtomicBool, timeout: Duration) -> Result<u64> {
        self.wait_ready_with(cancel, timeout, &mut |_| {})
    }

    /// [`Self::wait_ready`] that reports the time waited so far about every 100 ms while
    /// the server has not answered (for a "waking the library disk…" hint). Returns
    /// [`RemoteError::Cancelled`] within one tick of `cancel` being set.
    pub fn wait_ready_with(
        &self,
        cancel: &AtomicBool,
        timeout: Duration,
        waiting: &mut dyn FnMut(Duration),
    ) -> Result<u64> {
        let entry = self.entry();
        let started = Instant::now();
        let deadline = started + timeout;
        let mut state = entry.lock();
        loop {
            if let Some(failure) = &state.failure {
                return Err(failure.duplicate());
            }
            if let Some(total) = state.total {
                return Ok(total);
            }
            if cancel.load(Ordering::Acquire) || state.cancelled {
                return Err(RemoteError::Cancelled);
            }
            if Instant::now() >= deadline {
                return Err(RemoteError::Timeout(entry.source.url.clone()));
            }
            drop(state);
            waiting(started.elapsed());
            state = entry.lock();
            if state.total.is_none() && state.failure.is_none() {
                state = entry.wait(state);
            }
        }
    }

    /// The source's first-byte timeout (the stream thread gives up after it).
    pub fn first_byte_timeout(&self) -> Duration {
        self.entry().source.first_byte_timeout
    }

    /// Fail the reads that are waiting for missing bytes right now, without stopping the
    /// fetch. Stop uses this so a decoder blocked on the network releases the audio
    /// worker at once; new readers and later reads are unaffected.
    pub fn interrupt_reads(&self) {
        let entry = self.entry();
        let mut state = entry.lock();
        state.read_epoch = state.read_epoch.wrapping_add(1);
        entry.signal.notify_all();
    }

    /// Wait until the whole file is in memory (or the stream failed).
    pub fn wait_complete(&self, timeout: Duration) -> Result<u64> {
        let entry = self.entry();
        let deadline = Instant::now() + timeout;
        let mut state = entry.lock();
        loop {
            if let Some(failure) = &state.failure {
                return Err(failure.duplicate());
            }
            if let Some(total) = state.total.filter(|_| state.complete()) {
                return Ok(total);
            }
            if state.cancelled {
                return Err(RemoteError::Cancelled);
            }
            if state.worker.is_none() {
                return Err(RemoteError::Unreachable(format!(
                    "{}: the stream stopped before the end",
                    entry.source.url
                )));
            }
            if Instant::now() >= deadline {
                return Err(RemoteError::Timeout(entry.source.url.clone()));
            }
            state = entry.wait(state);
        }
    }

    /// A new independent reader positioned at byte 0. Requires a successful first response.
    pub fn reader(&self) -> io::Result<MemoryReader> {
        let entry = self.handle.entry.clone();
        let state = entry.lock();
        if let Some(failure) = &state.failure {
            return Err(io::Error::other(failure.to_string()));
        }
        let total = state
            .total
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "stream not ready"))?;
        drop(state);
        entry.readers.fetch_add(1, Ordering::AcqRel);
        Ok(MemoryReader {
            entry,
            position: 0,
            total,
        })
    }

    /// Whether `offset..offset + len` is in memory (see [`MemoryReader`]); a miss points the
    /// fetch at `offset`. Never blocks on the network.
    pub fn is_buffered(&self, offset: u64, len: u64) -> bool {
        self.entry().is_buffered(offset, len)
    }

    /// End of the bytes held contiguously from `offset` (`offset` itself when that byte is
    /// missing). Unlike [`Self::is_buffered`] it does not steer the fetch.
    pub fn contiguous_from(&self, offset: u64) -> u64 {
        self.entry().lock().covered.end_of(offset).unwrap_or(offset)
    }

    /// Stop fetching: the connection is shut down at once (also while it still waits for
    /// the server's first byte) and waiting reads fail. Received bytes stay cached.
    pub fn cancel(&self) {
        let entry = self.entry();
        entry.lock().cancelled = true;
        stop_worker(entry);
    }

    pub fn is_complete(&self) -> bool {
        self.entry().lock().complete()
    }

    pub fn is_failed(&self) -> bool {
        self.entry().lock().failure.is_some()
    }

    /// Whether the fetch thread is still running (not finished, failed or cancelled).
    pub fn is_active(&self) -> bool {
        self.entry().lock().worker.is_some()
    }

    /// Whether the server honours `Range` (known after the first response).
    pub fn uses_ranges(&self) -> bool {
        self.entry().lock().ranges
    }

    pub fn etag(&self) -> Option<String> {
        self.entry().lock().etag.clone()
    }

    /// Total length in bytes, once known.
    pub fn len(&self) -> Option<u64> {
        self.entry().lock().total
    }

    /// Known to be an empty file.
    pub fn is_empty(&self) -> bool {
        self.len() == Some(0)
    }

    /// The output format of a `/preview` stream (`None` for `/stream` or WebDAV).
    pub fn preview_format(&self) -> Option<PreviewFormat> {
        self.entry().lock().preview
    }

    /// Bytes of this stream held in memory.
    pub fn memory_bytes(&self) -> u64 {
        self.entry().bytes.load(Ordering::Acquire)
    }
}

/// `Read + Seek` view of a [`MemoryStream`]; reads block until the bytes have arrived
/// (the player checks [`MemoryStream::is_buffered`] first and so never waits).
pub struct MemoryReader {
    entry: Arc<Entry>,
    position: u64,
    total: u64,
}

impl Drop for MemoryReader {
    fn drop(&mut self) {
        self.entry.readers.fetch_sub(1, Ordering::AcqRel);
    }
}

impl MemoryReader {
    pub fn len(&self) -> u64 {
        self.total
    }

    pub fn is_empty(&self) -> bool {
        self.total == 0
    }
}

impl Read for MemoryReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() || self.position >= self.total {
            return Ok(0);
        }
        let read = self.entry.read_at(self.position, buffer)?;
        self.position += read as u64;
        Ok(read)
    }
}

impl Seek for MemoryReader {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let next = match position {
            SeekFrom::Start(offset) => Some(offset),
            SeekFrom::End(delta) => self.total.checked_add_signed(delta),
            SeekFrom::Current(delta) => self.position.checked_add_signed(delta),
        }
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek before byte 0"))?;
        self.position = next;
        Ok(next)
    }
}

/// What the fetch thread does next.
enum Next {
    Fetch(u64),
    /// Everything near the reader is in memory: wait for it to move on.
    Idle,
    Done,
}

/// A wanted offset at most this far ahead of the byte being received is waited for; a
/// farther one starts a new request. About 300 ms of transfer at the measured throughput
/// (64 KiB to 1 MiB): on a slow link a new request (one round trip) beats waiting for the
/// bytes in between.
fn seek_jump_bytes() -> u64 {
    let bps = bandwidth()
        .estimate_bps()
        .unwrap_or(crate::route::CONSERVATIVE_BANDWIDTH_BPS);
    ((bps / 8.0 * 0.3) as u64).clamp(MIN_SEEK_JUMP_BYTES, SEEK_JUMP_BYTES)
}

fn worker_cancelled(worker: &Worker) -> bool {
    worker.cancel.load(Ordering::Acquire)
}

/// Next request start: a reader's wanted offset, else the next gap after the reader
/// within the window.
fn next_start(entry: &Entry) -> Next {
    let mut state = entry.lock();
    let Some(total) = state.total else {
        return Next::Fetch(0);
    };
    if state.covered.complete(total) {
        return Next::Done;
    }
    if !state.ranges {
        return Next::Fetch(0);
    }
    if let Some(wanted) = state.wanted.take()
        && let Some(gap) = state.covered.next_gap(wanted, total)
    {
        return Next::Fetch(gap);
    }
    let (low, high) = entry.bounds(&state, total);
    for from in [state.reader_at.max(low), low] {
        if let Some(gap) = state.covered.next_gap(from, total)
            && gap < high
        {
            return Next::Fetch(gap);
        }
    }
    Next::Idle
}

/// Record the length, validator and preview format of a response. A different file
/// version replaces the entry's bytes when nobody reads them, and fails otherwise.
fn adopt(
    entry: &Entry,
    total: u64,
    etag: Option<&str>,
    ranges: bool,
    preview: Option<PreviewFormat>,
    replaced: bool,
) -> Result<()> {
    let mut state = entry.lock();
    let url = &entry.source.url;
    let changed = match (state.total, state.etag.as_deref(), etag) {
        _ if replaced && state.total.is_some() => Some(format!(
            "{url}: the server sent the whole file for an If-Range request"
        )),
        (Some(known), _, _) if known != total => Some(format!(
            "{url}: length changed from {known} to {total} bytes"
        )),
        (_, Some(known), Some(new)) if known != new => {
            Some(format!("{url}: ETag changed from {known} to {new}"))
        }
        _ if state.total.is_some() && state.preview != preview => {
            Some(format!("{url}: the preview format changed"))
        }
        _ => None,
    };
    if let Some(reason) = changed {
        if entry.readers.load(Ordering::Acquire) > 0 {
            return Err(RemoteError::Changed(reason));
        }
        tracing::info!(%reason, "cached audition stream replaced by a new file version");
        entry.clear(&mut state);
    }
    if state.total.is_none() {
        state.total = Some(total);
        state.ranges = ranges;
        state.preview = preview;
    }
    if state.etag.is_none() {
        state.etag = etag.map(str::to_owned);
    }
    entry.signal.notify_all();
    Ok(())
}

/// After `start..end` was written: false when the worker should abandon the current
/// response (a reader needs data elsewhere, or the response reached data already here).
fn keep_going(state: &mut State, end: u64, total: u64) -> bool {
    if let Some(wanted) = state.wanted {
        if state.covered.contains(wanted) || (wanted >= end && wanted - end <= seek_jump_bytes()) {
            state.wanted = None;
        } else {
            return false;
        }
    }
    !(end < total && state.covered.contains(end))
}

/// Whether the response may go on at `offset`: waits while it is ahead of the window
/// (the reader is slower than the network), at most [`HOLD_RESPONSE`]. False to close it.
fn hold(
    entry: &Entry,
    worker: &Worker,
    offset: u64,
    total: u64,
    throughput: &mut Throughput,
) -> bool {
    let mut state = entry.lock();
    if !state.ranges {
        return true;
    }
    let deadline = Instant::now() + HOLD_RESPONSE;
    loop {
        let (_, high) = entry.bounds(&state, total);
        if offset < high {
            return true;
        }
        if worker_cancelled(worker) || Instant::now() >= deadline {
            return false;
        }
        if state
            .wanted
            .is_some_and(|wanted| !(wanted >= offset && wanted - offset <= seek_jump_bytes()))
        {
            return false;
        }
        throughput.pause(bandwidth());
        state = entry.wait(state);
    }
}

fn run(entry: &Arc<Entry>, cache: &MemoryCache, worker: &Arc<Worker>) {
    let result = download(entry, cache, worker);
    {
        let mut state = entry.lock();
        let current = state
            .worker
            .as_ref()
            .is_some_and(|running| Arc::ptr_eq(running, worker));
        if current {
            state.worker = None;
        }
        if let Err(error) = &result
            && current
            && !matches!(error, RemoteError::Cancelled)
        {
            state.failure = Some(error.duplicate());
        }
        entry.signal.notify_all();
    }
    match &result {
        Err(RemoteError::Cancelled) | Ok(()) => {}
        Err(error) => tracing::warn!(url = %entry.source.url, %error, "audio stream failed"),
    }
    cache.trim();
}

/// Retried after the response was established (at most [`STREAM_RETRIES`] times, honouring
/// `Retry-After`): an offline root (`503 UNAVAILABLE`) gets the same short bounded retry,
/// never an endless loop; before establishment it fails at once with the server's text.
fn transient(error: &RemoteError) -> bool {
    matches!(
        error,
        RemoteError::Unreachable(_)
            | RemoteError::Timeout(_)
            | RemoteError::Starting { .. }
            | RemoteError::RootOffline { .. }
            | RemoteError::Busy { .. }
    )
}

/// Wait before stream retry `attempt` (1-based): the server's `Retry-After` when it sent
/// one, else 0.5, 1, 2 s; never longer than [`MAX_RETRY_WAIT`].
fn retry_backoff(attempt: u32, error: &RemoteError) -> Duration {
    crate::error::retry_wait(
        Duration::from_millis(500 << attempt.saturating_sub(1).min(4)),
        error.retry_after(),
        MAX_RETRY_WAIT,
    )
}

fn download(entry: &Entry, cache: &MemoryCache, worker: &Worker) -> Result<()> {
    let agent = abortable_stream_agent(
        entry.source.first_byte_timeout,
        READ_STALL_TIMEOUT,
        &worker.abort,
    );
    let mut buffer = vec![0_u8; BUFFER_BYTES];
    let mut attempts = 0;
    loop {
        if worker_cancelled(worker) {
            return Err(RemoteError::Cancelled);
        }
        let start = match next_start(entry) {
            Next::Done => return Ok(()),
            Next::Idle => {
                let state = entry.lock();
                drop(entry.wait(state));
                continue;
            }
            Next::Fetch(start) => start,
        };
        if attempts == 0 {
            // A new request (not a retry): the first one, a seek, a window move. Have a
            // spare connection ready for the next one (a seek, the next selection).
            crate::abort::warm_spare(&entry.source.url);
        }
        let result = if entry.source.flac_duration.is_some() {
            fetch_flac(entry, cache, worker, &agent, start)
        } else {
            fetch(entry, cache, worker, &agent, &mut buffer, start)
        };
        let error = match result {
            Ok(written) if written > 0 => {
                attempts = 0;
                continue;
            }
            Ok(_) => {
                if matches!(next_start(entry), Next::Done | Next::Idle) {
                    continue;
                }
                RemoteError::Unreachable(format!(
                    "{}: no data received from byte {start}",
                    entry.source.url
                ))
            }
            // A cancel shuts the connection down; whatever the request failed with, the
            // reason is the cancel.
            Err(_) if worker_cancelled(worker) => return Err(RemoteError::Cancelled),
            Err(error) => error,
        };
        // Failures before the first response (no length yet) are reported, not retried,
        // except `BUSY`: a full stream cap rejects new streams and asks to come back soon.
        // A slow first byte (a NAS disk spinning up) is simply waited for, up to the
        // source's first-byte timeout; only then is it a timeout.
        let established = entry.lock().total.is_some();
        let error = match error {
            RemoteError::Timeout(_) if !established => RemoteError::Timeout(format!(
                "{}: no response within {} s",
                entry.source.url,
                entry.source.first_byte_timeout.as_secs()
            )),
            error => error,
        };
        let retryable =
            transient(&error) && (established || matches!(error, RemoteError::Busy { .. }));
        if matches!(error, RemoteError::Cancelled) || !retryable || attempts >= STREAM_RETRIES {
            return Err(error);
        }
        attempts += 1;
        let resume = Instant::now() + retry_backoff(attempts, &error);
        while Instant::now() < resume {
            if worker_cancelled(worker) {
                return Err(RemoteError::Cancelled);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

fn header(headers: &ureq::http::HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.trim().to_owned())
}

/// `bytes <first>-<last>/<total>` or `bytes */<total>` -> (first, total).
fn content_range(value: &str) -> Option<(Option<u64>, Option<u64>)> {
    let rest = value.trim().strip_prefix("bytes")?.trim_start();
    let (range, total) = rest.split_once('/')?;
    let total = match total.trim() {
        "*" => None,
        total => Some(total.parse().ok()?),
    };
    let first = match range.trim() {
        "*" => None,
        range => Some(range.split_once('-')?.0.trim().parse().ok()?),
    };
    Some((first, total))
}

/// Result of [`probe_stream`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamProbe {
    /// `206` (Range honoured), `200` (Range ignored) or `416` (empty file).
    pub status: u16,
    /// Full length of the file, when reported.
    pub total: Option<u64>,
    pub etag: Option<String>,
}

/// Check a stream source with a one-byte `Range: bytes=0-0` request (the body is not
/// read). Errors are classified like stream errors (`404` missing, `401` token, `503`).
pub fn probe_stream(source: &StreamSource) -> Result<StreamProbe> {
    let agent = agent(Duration::from_secs(8), Duration::from_secs(10));
    let url = source.url.as_str();
    let mut request = agent.get(url).header("Range", "bytes=0-0");
    if let Some(value) = source.auth.header() {
        request = request.header("Authorization", &value);
    }
    let mut response = request
        .call()
        .map_err(|error| RemoteError::from_transport(error, url))?;
    let status = response.status().as_u16();
    let headers = response.headers();
    let etag = header(headers, "etag");
    let range_total = header(headers, "content-range")
        .and_then(|value| content_range(&value))
        .and_then(|(_, total)| total);
    match status {
        206 | 416 if range_total.is_some() => Ok(StreamProbe {
            status,
            total: range_total,
            etag,
        }),
        200 => Ok(StreamProbe {
            status,
            total: header(headers, "content-length").and_then(|value| value.parse().ok()),
            etag,
        }),
        206 | 416 => Err(RemoteError::Protocol(format!(
            "{url}: HTTP {status} without a valid Content-Range"
        ))),
        _ => Err(error_response(&mut response, status, source)),
    }
}

/// Classify a non-success response (reads at most 64 KiB of its body).
fn error_response(
    response: &mut ureq::http::Response<ureq::Body>,
    status: u16,
    source: &StreamSource,
) -> RemoteError {
    let url = source.url.as_str();
    match status {
        401 | 403 if source.auth == SourceAuth::None => {
            RemoteError::TokenRequired(format!("{url}: HTTP {status}"))
        }
        401 | 403 => RemoteError::Unauthorized(format!(
            "{url}: HTTP {status}; check the token or WebDAV credentials"
        )),
        _ => {
            let retry = retry_after(response.headers());
            let body = response
                .body_mut()
                .with_config()
                .limit(64 * 1024)
                .read_to_string()
                .unwrap_or_default();
            RemoteError::from_audio_response(status, &body, url, retry)
        }
    }
}

fn body_error(error: io::Error, url: &str) -> RemoteError {
    match error.kind() {
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => RemoteError::Timeout(url.to_owned()),
        _ => RemoteError::Unreachable(format!("{url}: {error}")),
    }
}

/// Bytes between two global trims while receiving.
const TRIM_EVERY: u64 = 4 * CHUNK_BYTES;

/// One byte-range request from `start`; returns how many bytes it stored.
fn fetch(
    entry: &Entry,
    cache: &MemoryCache,
    worker: &Worker,
    agent: &Timeouts,
    buffer: &mut [u8],
    start: u64,
) -> Result<u64> {
    let url = entry.source.url.as_str();
    let (if_range, known_total) = {
        let state = entry.lock();
        let if_range = state
            .etag
            .clone()
            .filter(|etag| state.ranges && !etag.starts_with("W/"));
        (if_range, state.total)
    };
    let range = format!("bytes={start}-");
    let mut response = crate::abort::call_fresh(|| {
        let mut request = agent.get(url).header("Range", &range);
        if let Some(value) = entry.source.auth.header() {
            request = request.header("Authorization", &value);
        }
        if let Some(etag) = &if_range {
            request = request.header("If-Range", etag);
        }
        request
    })
    .map_err(|error| RemoteError::from_transport(error, url))?;
    let status = response.status().as_u16();
    let headers = response.headers();
    let etag = header(headers, "etag");
    let length = header(headers, "content-length").and_then(|value| value.parse::<u64>().ok());
    let range = header(headers, "content-range").and_then(|value| content_range(&value));
    let preview = PreviewFormat::from_headers(headers);
    let (body_start, total, replaced) = match status {
        206 => {
            let Some((Some(first), total)) = range else {
                return Err(RemoteError::Protocol(format!(
                    "{url}: 206 without a valid Content-Range"
                )));
            };
            if first != start {
                return Err(RemoteError::Protocol(format!(
                    "{url}: asked for byte {start}, the server sent byte {first}"
                )));
            }
            let total = total.or(known_total).ok_or_else(|| {
                RemoteError::Protocol(format!("{url}: the server did not report a length"))
            })?;
            (first, total, false)
        }
        200 => {
            let total = length.ok_or_else(|| {
                RemoteError::Protocol(format!("{url}: the server did not report a length"))
            })?;
            // The whole file in answer to `If-Range`: another version, unless the ETag is
            // still the same (a server that ignores `Range`).
            let replaced = if_range.is_some() && etag != if_range;
            (0, total, replaced)
        }
        416 => {
            if let Some((None, Some(total))) = range {
                adopt(entry, total, etag.as_deref(), true, preview, false)?;
                if start >= total {
                    return Ok(0);
                }
            }
            return Err(RemoteError::Protocol(format!(
                "{url}: range from byte {start} not satisfiable"
            )));
        }
        _ => return Err(error_response(&mut response, status, &entry.source)),
    };
    let ranged = status == 206;
    adopt(entry, total, etag.as_deref(), ranged, preview, replaced)?;
    let mut reader = response.into_body().into_reader();
    let mut offset = body_start;
    let mut written = 0_u64;
    let mut throughput = Throughput::new();
    let mut since_trim = 0;
    while offset < total {
        if worker_cancelled(worker) {
            return Err(RemoteError::Cancelled);
        }
        if !hold(entry, worker, offset, total, &mut throughput) {
            throughput.pause(bandwidth());
            return Ok(written);
        }
        let read = reader
            .read(buffer)
            .map_err(|error| body_error(error, url))?;
        if read == 0 {
            break;
        }
        throughput.add(read as u64, bandwidth());
        let len = usize::try_from(total - offset)
            .unwrap_or(usize::MAX)
            .min(read);
        let go_on = {
            let mut state = entry.lock();
            entry.write(&mut state, offset, &buffer[..len]);
            offset += len as u64;
            written += len as u64;
            !ranged || keep_going(&mut state, offset, total)
        };
        since_trim += len as u64;
        if since_trim >= TRIM_EVERY {
            since_trim = 0;
            cache.trim();
        }
        if !go_on {
            throughput.pause(bandwidth());
            return Ok(written);
        }
    }
    throughput.pause(bandwidth());
    if offset < total {
        return Err(RemoteError::Unreachable(format!(
            "{url}: connection closed after byte {offset} of {total}"
        )));
    }
    Ok(written)
}

/// 44-byte PCM WAV header for `total` bytes of `format`.
fn wav_header(format: &PreviewFormat, total: u64) -> [u8; WAV_HEADER_BYTES as usize] {
    let data = total.saturating_sub(WAV_HEADER_BYTES);
    let riff = u32::try_from(total.saturating_sub(8)).unwrap_or(u32::MAX);
    let data32 = u32::try_from(data).unwrap_or(u32::MAX);
    let align = format.block_align() as u16;
    let mut header = [0_u8; WAV_HEADER_BYTES as usize];
    header[0..4].copy_from_slice(b"RIFF");
    header[4..8].copy_from_slice(&riff.to_le_bytes());
    header[8..12].copy_from_slice(b"WAVE");
    header[12..16].copy_from_slice(b"fmt ");
    header[16..20].copy_from_slice(&16_u32.to_le_bytes());
    header[20..22].copy_from_slice(&1_u16.to_le_bytes());
    header[22..24].copy_from_slice(&format.channels.to_le_bytes());
    header[24..28].copy_from_slice(&format.sample_rate.to_le_bytes());
    header[28..32].copy_from_slice(&(format.sample_rate * u32::from(align)).to_le_bytes());
    header[32..34].copy_from_slice(&align.to_le_bytes());
    header[34..36].copy_from_slice(&format.bits.to_le_bytes());
    header[36..40].copy_from_slice(b"data");
    header[40..44].copy_from_slice(&data32.to_le_bytes());
    header
}

/// HTTP body as an unseekable media source for Symphonia, counting what arrives and
/// telling a clean end of the body from a broken connection.
struct BodySource {
    inner: Mutex<ureq::BodyReader<'static>>,
    received: Arc<AtomicU64>,
    ended: Arc<AtomicBool>,
}

impl Read for BodySource {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let read = self
            .inner
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .read(buffer)?;
        if read == 0 {
            self.ended.store(true, Ordering::Release);
        }
        self.received.fetch_add(read as u64, Ordering::AcqRel);
        Ok(read)
    }
}

fn flac_error(error: symphonia::core::errors::Error, url: &str, ended: bool) -> RemoteError {
    match error {
        symphonia::core::errors::Error::IoError(error) if !ended => body_error(error, url),
        other => RemoteError::Protocol(format!("{url}: FLAC preview: {other}")),
    }
}

/// Store `count` bytes of silence at `offset` (the end of a FLAC preview shorter than the
/// record's duration, or a rounding gap after a seek).
fn write_silence(entry: &Entry, offset: u64, count: u64) {
    let zeros = vec![0_u8; BUFFER_BYTES];
    let mut at = offset;
    let end = offset + count;
    while at < end {
        let len = (end - at).min(BUFFER_BYTES as u64) as usize;
        entry.write(&mut entry.lock(), at, &zeros[..len]);
        at += len as u64;
    }
}

/// One `codec=flac` request covering byte `start` of the decoded WAV; returns how many
/// bytes it stored.
fn fetch_flac(
    entry: &Entry,
    cache: &MemoryCache,
    worker: &Worker,
    agent: &Timeouts,
    start: u64,
) -> Result<u64> {
    use symphonia::core::formats::{TrackType, probe::Hint};
    use symphonia::core::io::{MediaSourceStream, ReadOnlySource};

    let base = entry.source.url.as_str();
    let duration = entry.source.flac_duration.unwrap_or_default();
    let (known, known_total) = {
        let state = entry.lock();
        (state.preview, state.total)
    };
    // Ask one frame early: X-Start-Frame is rounded (non-integer rate ratios).
    let wanted_frame = match known {
        Some(format) if start > WAV_HEADER_BYTES && format.block_align() > 0 => {
            (start - WAV_HEADER_BYTES) / format.block_align()
        }
        _ => 0,
    };
    let ask = wanted_frame.saturating_sub(1);
    let url = match known {
        Some(format) if ask > 0 => format!(
            "{base}{}t={:.6}",
            if base.contains('?') { '&' } else { '?' },
            ask as f64 / f64::from(format.sample_rate)
        ),
        _ => base.to_owned(),
    };
    let mut response = crate::abort::call_fresh(|| {
        let mut request = agent.get(&url);
        if let Some(value) = entry.source.auth.header() {
            request = request.header("Authorization", &value);
        }
        request
    })
    .map_err(|error| RemoteError::from_transport(error, &url))?;
    let status = response.status().as_u16();
    match status {
        200 => {}
        // `t` at or past the real end (the record's duration is a little longer than the
        // file): the rest is silence.
        416 if known_total.is_some_and(|total| start < total) => {
            let total = known_total.unwrap_or(start);
            write_silence(entry, start, total - start);
            return Ok(total - start);
        }
        _ => return Err(error_response(&mut response, status, &entry.source)),
    }
    let headers = response.headers();
    let format = PreviewFormat::from_headers(headers)
        .filter(|format| matches!(format.bits, 16 | 24) && format.channels > 0)
        .ok_or_else(|| {
            RemoteError::Protocol(format!(
                "{url}: FLAC preview without a usable X-Preview format"
            ))
        })?;
    let format = PreviewFormat {
        frames: None,
        ..format
    };
    let start_frame = header(headers, "x-start-frame")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let align = format.block_align();
    // The length is fixed by the first response: the server's X-Total-Frames / X-Duration
    // (servers from 0.3.6; both count output frames from the start position, so the start
    // is added back), else the record's duration from the snapshot. A FLAC ETag (weak, it
    // varies with `t=`) is not used.
    let total = known_total.unwrap_or_else(|| {
        let rate = f64::from(format.sample_rate);
        let frames = header(headers, "x-total-frames")
            .and_then(|value| value.parse::<u64>().ok())
            .map(|frames| frames + start_frame)
            .or_else(|| {
                header(headers, "x-duration")
                    .and_then(|value| value.parse::<f64>().ok())
                    .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
                    .map(|seconds| (seconds * rate).round() as u64 + start_frame)
            })
            .filter(|frames| *frames > 0)
            .unwrap_or_else(|| (duration.as_secs_f64() * rate).round() as u64);
        WAV_HEADER_BYTES + frames * align
    });
    adopt(entry, total, None, true, Some(format), false)?;
    if !entry.lock().covered.contains(0) {
        entry.write(&mut entry.lock(), 0, &wav_header(&format, total));
    }
    let mut offset = (WAV_HEADER_BYTES + start_frame * align).min(total);
    let mut written = 0;
    if start >= WAV_HEADER_BYTES && offset > start {
        write_silence(entry, start, offset - start);
        written += offset - start;
    }
    let received = Arc::new(AtomicU64::new(0));
    let ended = Arc::new(AtomicBool::new(false));
    let body = BodySource {
        inner: Mutex::new(response.into_body().into_reader()),
        received: received.clone(),
        ended: ended.clone(),
    };
    let stream = MediaSourceStream::new(Box::new(ReadOnlySource::new(body)), Default::default());
    let mut hint = Hint::new();
    hint.with_extension("flac");
    let mut reader = symphonia::default::get_probe()
        .probe(&hint, stream, Default::default(), Default::default())
        .map_err(|error| flac_error(error, &url, ended.load(Ordering::Acquire)))?;
    let track = reader
        .default_track(TrackType::Audio)
        .ok_or_else(|| RemoteError::Protocol(format!("{url}: FLAC preview without audio")))?;
    let track_id = track.id;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|params| params.audio())
        .ok_or_else(|| RemoteError::Protocol(format!("{url}: FLAC preview without parameters")))?;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(params, &Default::default())
        .map_err(|error| flac_error(error, &url, true))?;
    let mut throughput = Throughput::new();
    let mut counted = 0;
    let mut samples16: Vec<i16> = Vec::new();
    let mut samples32: Vec<i32> = Vec::new();
    let mut bytes: Vec<u8> = Vec::new();
    let mut since_trim = 0;
    loop {
        if worker_cancelled(worker) {
            return Err(RemoteError::Cancelled);
        }
        if offset >= total {
            break;
        }
        if !hold(entry, worker, offset, total, &mut throughput) {
            throughput.pause(bandwidth());
            return Ok(written);
        }
        let packet = match reader.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            Err(_) if ended.load(Ordering::Acquire) => break,
            Err(error) => return Err(flac_error(error, &url, false)),
        };
        let now = received.load(Ordering::Acquire);
        throughput.add(now - counted, bandwidth());
        counted = now;
        if packet.track_id != track_id {
            continue;
        }
        let audio = match decoder.decode(&packet) {
            Ok(audio) => audio,
            Err(symphonia::core::errors::Error::DecodeError(reason)) => {
                tracing::debug!(%reason, "bad FLAC preview frame skipped");
                continue;
            }
            Err(error) => return Err(flac_error(error, &url, true)),
        };
        bytes.clear();
        if format.bits == 16 {
            audio.copy_to_vec_interleaved(&mut samples16);
            bytes.extend(samples16.iter().flat_map(|sample| sample.to_le_bytes()));
        } else {
            audio.copy_to_vec_interleaved(&mut samples32);
            bytes.extend(samples32.iter().flat_map(|sample| {
                let [_, a, b, c] = sample.to_le_bytes();
                [a, b, c]
            }));
        }
        let len = (bytes.len() as u64).min(total - offset);
        let go_on = {
            let mut state = entry.lock();
            entry.write(&mut state, offset, &bytes[..len as usize]);
            offset += len;
            written += len;
            keep_going(&mut state, offset, total)
        };
        since_trim += len;
        if since_trim >= TRIM_EVERY {
            since_trim = 0;
            cache.trim();
        }
        if !go_on {
            throughput.pause(bandwidth());
            return Ok(written);
        }
    }
    throughput.pause(bandwidth());
    if offset < total {
        // The stream ended before the record's duration: the rest is silence.
        write_silence(entry, offset, total - offset);
        written += total - offset;
    }
    Ok(written)
}

#[cfg(test)]
impl MemoryCache {
    /// A cache with an exact (tiny) budget for tests; the window is half of it.
    fn for_test(budget: u64) -> Self {
        let cache = Self::new(budget);
        cache.inner.budget.store(budget, Ordering::Release);
        cache
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_server::{TestRequest, TestResponse, TestServer};

    type Log = Arc<Mutex<Vec<(String, Option<String>)>>>;

    fn data(len: usize, seed: usize) -> Arc<Vec<u8>> {
        Arc::new((0..len).map(|i| ((i * 31 + seed) % 251) as u8).collect())
    }

    fn note(request: &TestRequest, log: &Log) {
        log.lock().unwrap().push((
            request.header("range").unwrap_or_default().to_owned(),
            request.header("if-range").map(str::to_owned),
        ));
    }

    /// The server's `/audio/{id}/stream` contract for single `bytes=` ranges.
    fn serve(request: &TestRequest, bytes: &[u8], etag: &str) -> TestResponse {
        let len = bytes.len();
        let full = || TestResponse::bytes(200, bytes.to_vec()).with_header("ETag", etag);
        let Some(spec) = request
            .header("range")
            .and_then(|range| range.strip_prefix("bytes="))
        else {
            return full();
        };
        if request
            .header("if-range")
            .is_some_and(|value| value != etag)
        {
            return full();
        }
        let (first, last) = spec.split_once('-').unwrap();
        let first: usize = first.parse().unwrap();
        if first >= len {
            return TestResponse::json(416, r#"{"error_code":"RANGE_NOT_SATISFIABLE"}"#)
                .with_header("Content-Range", &format!("bytes */{len}"));
        }
        let last = if last.is_empty() {
            len - 1
        } else {
            last.parse::<usize>().unwrap().min(len - 1)
        };
        TestResponse::bytes(206, bytes[first..=last].to_vec())
            .with_header("ETag", etag)
            .with_header("Content-Range", &format!("bytes {first}-{last}/{len}"))
    }

    fn cache() -> MemoryCache {
        MemoryCache::new(DEFAULT_MEMORY_CACHE_BYTES)
    }

    fn start_in(cache: &MemoryCache, key: &str, url: String) -> MemoryStream {
        MemoryStream::start(
            cache,
            key,
            StreamSource::new(url, SourceAuth::Bearer("secret-token".into())),
        )
        .unwrap()
    }

    fn start(key: &str, url: String) -> MemoryStream {
        start_in(&cache(), key, url)
    }

    fn ready(stream: &MemoryStream) -> u64 {
        stream
            .wait_ready(&AtomicBool::new(false), Duration::from_secs(5))
            .unwrap()
    }

    fn read_all(stream: &MemoryStream) -> Vec<u8> {
        let mut all = Vec::new();
        stream.reader().unwrap().read_to_end(&mut all).unwrap();
        all
    }

    fn wait_until(mut condition: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if condition() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        condition()
    }

    /// `/audio/{id}/stream` answering 503 with `body` (and `Retry-After: 5`).
    fn unavailable(body: &'static str) -> TestServer {
        TestServer::start(move |_| TestResponse::json(503, body).with_header("Retry-After", "5"))
    }

    #[test]
    fn starting_and_root_offline_503s_on_the_stream_endpoint_are_told_apart() {
        let cases: [(&'static str, bool); 4] = [
            (
                r#"{"error_code":"UNAVAILABLE","message":"The library is still opening; retry shortly"}"#,
                false,
            ),
            ("", false),
            (
                r#"{"error_code":"ROOT_OFFLINE","message":"Library root \"SFX\" is offline: Root directory not found (volume not mounted?)"}"#,
                true,
            ),
            (
                r#"{"error_code":"UNAVAILABLE","message":"Library root \"SFX\" is offline: Root directory not found"}"#,
                true,
            ),
        ];
        for (index, (body, offline)) in cases.into_iter().enumerate() {
            let server = unavailable(body);
            let url = format!("{}/api/v1/audio/{index}/stream", server.url);
            let stream = start(&index.to_string(), url.clone());
            let error = stream
                .wait_ready(&AtomicBool::new(false), Duration::from_secs(5))
                .unwrap_err();
            let probe = probe_stream(&StreamSource::new(url, SourceAuth::None)).unwrap_err();
            for error in [error, probe] {
                let expected = if offline {
                    matches!(
                        error,
                        RemoteError::RootOffline {
                            retry_after: Some(5),
                            ..
                        }
                    )
                } else {
                    matches!(
                        error,
                        RemoteError::Starting {
                            retry_after: Some(5),
                            ..
                        }
                    )
                };
                assert!(expected, "{body}: {error:?}");
            }
        }
    }

    #[test]
    fn coverage_merges_removes_and_finds_gaps() {
        let mut coverage = Coverage::default();
        coverage.insert(10, 20);
        coverage.insert(30, 40);
        coverage.insert(20, 25);
        assert_eq!(coverage.0, vec![(10, 25), (30, 40)]);
        assert_eq!(coverage.next_gap(0, 50), Some(0));
        assert_eq!(coverage.next_gap(12, 50), Some(25));
        assert_eq!(coverage.next_gap(35, 40), None);
        assert_eq!(coverage.end_of(31), Some(40));
        coverage.remove(12, 15);
        assert_eq!(coverage.0, vec![(10, 12), (15, 25), (30, 40)]);
        coverage.remove(0, 100);
        assert!(coverage.0.is_empty());
        coverage.insert(0, 50);
        assert!(coverage.complete(50));
        assert_eq!(content_range("bytes 5-9/10"), Some((Some(5), Some(10))));
        assert_eq!(content_range("bytes */42"), Some((None, Some(42))));
        assert_eq!(content_range("items 1-2/3"), None);
    }

    #[test]
    fn streams_ranges_into_memory_with_etag_and_bearer() {
        let bytes = data(300_000, 0);
        let log = Log::default();
        let server = {
            let (bytes, log) = (bytes.clone(), log.clone());
            TestServer::start(move |request| {
                note(request, &log);
                if request.header("authorization") != Some("Bearer secret-token") {
                    return TestResponse::json(401, "");
                }
                serve(request, &bytes, "\"v1\"").slow(16 * 1024, Duration::from_millis(2))
            })
        };
        let cache = cache();
        let stream = start_in(&cache, "1", format!("{}/api/v1/audio/1/stream", server.url));
        assert_eq!(ready(&stream), 300_000);
        assert_eq!(read_all(&stream), *bytes);
        assert_eq!(
            stream.wait_complete(Duration::from_secs(10)).unwrap(),
            300_000
        );
        assert!(stream.is_complete() && stream.uses_ranges());
        assert!(wait_until(|| !stream.is_active()));
        assert_eq!(stream.etag().as_deref(), Some("\"v1\""));
        assert_eq!(stream.preview_format(), None);
        assert_eq!(log.lock().unwrap()[0], ("bytes=0-".to_owned(), None));
        // Whole chunks are allocated: 300 000 bytes take two 256 KiB chunks.
        assert_eq!(stream.memory_bytes(), 300_000);
        assert_eq!(cache.used_bytes(), 300_000);
        // A reader opened after completion reads from memory.
        let mut late = stream.reader().unwrap();
        late.seek(SeekFrom::End(-10)).unwrap();
        let mut tail = Vec::new();
        late.read_to_end(&mut tail).unwrap();
        assert_eq!(tail, bytes[bytes.len() - 10..]);
        assert!(!format!("{stream:?} {:?}", stream.source()).contains("secret-token"));
        // The complete original can be copied out for drag and drop.
        let mut copy = Vec::new();
        assert!(
            cache
                .copy_complete(&stream.source().url, "\"v1\"", Some(300_000), &mut copy)
                .unwrap()
        );
        assert_eq!(copy, *bytes);
        assert!(
            !cache
                .copy_complete(&stream.source().url, "\"v2\"", None, &mut Vec::new())
                .unwrap()
        );
        // Nothing was written to disk: the stream has no path at all.
    }

    #[test]
    fn partial_data_is_reported_per_range_and_a_miss_steers_the_fetch() {
        let bytes = data(3 * 1024 * 1024, 7);
        let log = Log::default();
        let server = {
            let (bytes, log) = (bytes.clone(), log.clone());
            TestServer::start(move |request| {
                note(request, &log);
                serve(request, &bytes, "\"v1\"").slow(32 * 1024, Duration::from_millis(10))
            })
        };
        let stream = start("2", format!("{}/s", server.url));
        ready(&stream);
        assert!(wait_until(|| stream.is_buffered(0, 64 * 1024)));
        assert!(stream.is_buffered(10, 0), "an empty range is always there");
        let target = 2_621_440;
        // The player only asks; it does not read. The fetch still moves there.
        assert!(!stream.is_buffered(target, 4096));
        assert!(wait_until(|| stream.is_buffered(target, 4096)));
        assert!(
            log.lock()
                .unwrap()
                .contains(&(format!("bytes={target}-"), Some("\"v1\"".to_owned()))),
            "{:?}",
            log.lock().unwrap()
        );
        let mut reader = stream.reader().unwrap();
        reader.seek(SeekFrom::Start(target)).unwrap();
        let mut slice = vec![0; 4096];
        reader.read_exact(&mut slice).unwrap();
        assert_eq!(slice, bytes[target as usize..target as usize + 4096]);
        assert!(!stream.is_complete());
        // Beyond the end is "buffered" (nothing to wait for); the gaps get filled.
        assert!(stream.is_buffered(bytes.len() as u64, 10));
        assert_eq!(read_all(&stream), *bytes);
        stream.wait_complete(Duration::from_secs(20)).unwrap();
    }

    #[test]
    fn server_ignoring_range_is_streamed_sequentially() {
        let bytes = data(200_000, 3);
        let log = Log::default();
        let server = {
            let (bytes, log) = (bytes.clone(), log.clone());
            TestServer::start(move |request| {
                note(request, &log);
                TestResponse::bytes(200, bytes.to_vec())
                    .with_header("ETag", "\"v1\"")
                    .slow(16 * 1024, Duration::from_millis(2))
            })
        };
        let stream = start("3", format!("{}/plain.wav", server.url));
        ready(&stream);
        assert!(!stream.uses_ranges());
        let mut reader = stream.reader().unwrap();
        reader.seek(SeekFrom::End(-100)).unwrap();
        let mut tail = vec![0; 100];
        reader.read_exact(&mut tail).unwrap();
        assert_eq!(tail, bytes[bytes.len() - 100..]);
        reader.seek(SeekFrom::Start(0)).unwrap();
        let mut head = vec![0; 100];
        reader.read_exact(&mut head).unwrap();
        assert_eq!(head, bytes[..100]);
        stream.wait_complete(Duration::from_secs(10)).unwrap();
        assert_eq!(read_all(&stream), *bytes);
        assert_eq!(log.lock().unwrap().len(), 1);
    }

    #[test]
    fn interrupted_download_resumes_with_if_range() {
        let bytes = data(250_000, 5);
        let log = Log::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let server = {
            let (bytes, log, calls) = (bytes.clone(), log.clone(), calls.clone());
            TestServer::start(move |request| {
                note(request, &log);
                let response = serve(request, &bytes, "\"v1\"");
                if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    response.cut_after(100_000)
                } else {
                    response
                }
            })
        };
        let stream = start("4", format!("{}/s", server.url));
        stream.wait_complete(Duration::from_secs(10)).unwrap();
        assert_eq!(read_all(&stream), *bytes);
        let log = log.lock().unwrap();
        assert_eq!(log.len(), 2, "{log:?}");
        assert_eq!(
            log[1],
            ("bytes=100000-".to_owned(), Some("\"v1\"".to_owned()))
        );
    }

    /// Serves `old` (ETag v1, cut after 100 kB) on the first request and `new` (v2) after.
    fn changing_server(calls: Arc<AtomicUsize>) -> (TestServer, Arc<Vec<u8>>) {
        let old = data(250_000, 1);
        let new = data(260_000, 2);
        let served = new.clone();
        let server = TestServer::start(move |request| {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                serve(request, &old, "\"v1\"").cut_after(100_000)
            } else {
                serve(request, &new, "\"v2\"")
            }
        });
        (server, served)
    }

    #[test]
    fn file_changed_mid_stream_fails_without_mixing_versions() {
        let calls = Arc::new(AtomicUsize::new(0));
        let (server, _) = changing_server(calls.clone());
        let stream = start("5", format!("{}/s", server.url));
        ready(&stream);
        // A decoder is reading the first version: it must not get bytes of the second.
        let _reader = stream.reader().unwrap();
        assert!(matches!(
            stream.wait_complete(Duration::from_secs(10)),
            Err(RemoteError::Changed(_))
        ));
        assert!(stream.is_failed());
        assert!(stream.reader().is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_reused_entry_of_an_older_version_is_replaced() {
        let calls = Arc::new(AtomicUsize::new(0));
        let (server, new) = changing_server(calls.clone());
        let cache = cache();
        let url = format!("{}/s", server.url);
        {
            // Nobody reads: the cut first version stays partly cached after the cancel.
            let stream = start_in(&cache, "6", url.clone());
            ready(&stream);
            assert!(wait_until(
                || stream.is_buffered(0, 100_000) || stream.is_failed()
            ));
            stream.cancel();
        }
        let again = start_in(&cache, "6", url);
        again.wait_complete(Duration::from_secs(10)).unwrap();
        assert_eq!(read_all(&again), *new);
        assert_eq!(again.etag().as_deref(), Some("\"v2\""));
    }

    #[test]
    fn a_complete_entry_is_reused_without_a_request() {
        let bytes = data(100_000, 8);
        let requests = Arc::new(AtomicUsize::new(0));
        let server = {
            let (bytes, requests) = (bytes.clone(), requests.clone());
            TestServer::start(move |request| {
                requests.fetch_add(1, Ordering::SeqCst);
                serve(request, &bytes, "\"v1\"")
            })
        };
        let cache = cache();
        let url = format!("{}/s", server.url);
        start_in(&cache, "7", url.clone())
            .wait_complete(Duration::from_secs(5))
            .unwrap();
        let again = start_in(&cache, "7", url);
        assert!(again.is_complete());
        assert_eq!(read_all(&again), *bytes);
        assert_eq!(requests.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn whole_entries_are_evicted_least_recently_used_first() {
        let bytes = data(600_000, 9);
        let server = {
            let bytes = bytes.clone();
            TestServer::start(move |request| serve(request, &bytes, "\"v1\""))
        };
        // Room for two 600 kB entries (three chunks each).
        let cache = MemoryCache::for_test(1_700_000);
        let url = |n: usize| format!("{}/s?{n}", server.url);
        let held = start_in(&cache, "held", url(0));
        held.wait_complete(Duration::from_secs(5)).unwrap();
        for n in 1..=3 {
            let stream = start_in(&cache, &n.to_string(), url(n));
            stream.wait_complete(Duration::from_secs(5)).unwrap();
        }
        // "held" is still in use; 1 and 2 were evicted as 2 and 3 arrived.
        assert!(cache.used_bytes() <= 1_700_000, "{}", cache.used_bytes());
        let keys: Vec<String> = cache.entries().iter().map(|e| e.key.clone()).collect();
        assert_eq!(keys, ["held", "3"], "{keys:?}");
        assert_eq!(read_all(&held), *bytes);
        drop(held);
        cache.set_budget(0);
        assert!(cache.used_bytes() <= 1_700_000);
        cache.clear();
        assert!(cache.is_empty() && cache.used_bytes() == 0);
    }

    #[test]
    fn remove_where_drops_only_matching_entries_nobody_uses() {
        let bytes = data(100_000, 4);
        let server = {
            let bytes = bytes.clone();
            TestServer::start(move |request| serve(request, &bytes, "\"v1\""))
        };
        let cache = cache();
        let url = |n: usize| format!("{}/s?{n}", server.url);
        for key in ["lib/7/100/pcm/server", "lib/8/100/pcm/server"] {
            start_in(&cache, key, url(key.len()))
                .wait_complete(Duration::from_secs(5))
                .unwrap();
        }
        let held = start_in(&cache, "lib/9/100/pcm/server", url(9));
        held.wait_complete(Duration::from_secs(5)).unwrap();
        cache.remove_where(|key| key.split('/').nth(1) != Some("8"));
        let keys: Vec<String> = cache.entries().iter().map(|e| e.key.clone()).collect();
        assert_eq!(
            keys,
            ["lib/8/100/pcm/server", "lib/9/100/pcm/server"],
            "{keys:?}"
        );
        assert_eq!(read_all(&held), *bytes);
    }

    #[test]
    fn a_large_file_is_fetched_only_its_read_ahead_beyond_the_player() {
        // 12 MiB file, 16 MiB budget (8 MiB window): not fetched whole; with a 2 MiB
        // read-ahead (60 s of audio in the app) the fetch pauses about 2 MiB ahead.
        let bytes = data(12 * 1024 * 1024, 12);
        let server = {
            let bytes = bytes.clone();
            TestServer::start(move |request| {
                serve(request, &bytes, "\"v1\"").slow(256 * 1024, Duration::from_millis(1))
            })
        };
        let cache = MemoryCache::for_test(16 * 1024 * 1024);
        let mut source = StreamSource::new(format!("{}/s", server.url), SourceAuth::None);
        source.read_ahead = Some(2 * 1024 * 1024);
        let stream = MemoryStream::start(&cache, "ahead", source).unwrap();
        ready(&stream);
        assert!(wait_until(
            || stream.contiguous_from(0) >= 2 * 1024 * 1024 - CHUNK_BYTES
        ));
        std::thread::sleep(Duration::from_millis(300));
        assert!(
            stream.contiguous_from(0) <= 2 * 1024 * 1024 + 2 * CHUNK_BYTES,
            "{} bytes fetched",
            stream.contiguous_from(0)
        );
        // The player moves on: the fetch follows.
        assert!(!stream.is_buffered(5 * 1024 * 1024, 4096));
        assert!(wait_until(
            || stream.is_buffered(5 * 1024 * 1024, 1024 * 1024)
        ));
    }

    #[test]
    fn a_large_file_is_held_in_a_window_around_the_reader() {
        // 6 MiB file, 2 MiB budget: a window of 1 MiB (4 chunks) plus pinned chunks.
        let bytes = data(6 * 1024 * 1024, 10);
        let log = Log::default();
        let server = {
            let (bytes, log) = (bytes.clone(), log.clone());
            TestServer::start(move |request| {
                note(request, &log);
                serve(request, &bytes, "\"v1\"").slow(64 * 1024, Duration::from_millis(1))
            })
        };
        let cache = MemoryCache::for_test(2 * 1024 * 1024);
        assert_eq!(cache.window(), 1024 * 1024);
        let stream = start_in(&cache, "big", format!("{}/s", server.url));
        ready(&stream);
        // The window, a chunk it straddles, and the pinned header/trailer chunks.
        let pinned = 4 * CHUNK_BYTES;
        // The fetch stops ¾ window ahead of the reader and holds the response.
        assert!(wait_until(|| stream.is_buffered(0, 700 * 1024)));
        std::thread::sleep(Duration::from_millis(300));
        assert!(!stream.is_buffered(0, 1024 * 1024), "never past the window");
        assert!(stream.memory_bytes() <= cache.window() + pinned);
        // Play through it in order: memory stays bounded and the data is right.
        let mut reader = stream.reader().unwrap();
        let mut received = Vec::new();
        let mut piece = vec![0; 64 * 1024];
        loop {
            let read = reader.read(&mut piece).unwrap();
            if read == 0 {
                break;
            }
            received.extend_from_slice(&piece[..read]);
            assert!(
                stream.memory_bytes() <= cache.window() + pinned,
                "{} bytes held",
                stream.memory_bytes()
            );
        }
        assert_eq!(received, *bytes);
        assert!(
            !stream.is_complete(),
            "early chunks were dropped behind the reader"
        );
        // A seek back fetches from that offset (not the whole file again).
        let back = 1024 * 1024 + 4096;
        assert!(!stream.is_buffered(back, 4096));
        assert!(wait_until(|| stream.is_buffered(back, 4096)));
        assert!(
            log.lock()
                .unwrap()
                .iter()
                .any(|(range, _)| range == &format!("bytes={back}-")),
            "{:?}",
            log.lock().unwrap()
        );
        // The header chunk stayed: a decoder reopening the file reads it at once.
        assert!(stream.is_buffered(0, 44));
    }

    #[test]
    fn errors_are_classified_and_empty_files_complete() {
        let server = TestServer::start(|request| {
            match request.path.as_str() {
            "/missing" => TestResponse::json(
                404,
                r#"{"error_code":"NOT_FOUND","message":"file missing"}"#,
            ),
            "/token" => TestResponse::json(401, r#"{"error_code":"UNAUTHORIZED"}"#),
            "/starting" => TestResponse::json(503, r#"{"error_code":"UNAVAILABLE","message":"Library root \"SFX\" is offline: Root directory not found (volume not mounted?)"}"#)
                .with_header("Retry-After", "5"),
            "/pcm415" => TestResponse::json(415, r#"{"error_code":"PREVIEW_UNSUPPORTED_FORMAT","message":"cannot be previewed with codec=pcm"}"#),
            "/noffmpeg" => TestResponse::json(503, r#"{"error_code":"TRANSCODER_UNAVAILABLE","message":"ffmpeg missing"}"#),
            _ => serve(request, &[], "\"empty\""),
        }
        });
        let cancel = AtomicBool::new(false);
        let ready = |key: &str, path: &str| {
            start(key, format!("{}{path}", server.url)).wait_ready(&cancel, Duration::from_secs(5))
        };
        assert!(matches!(
            ready("10", "/missing"),
            Err(RemoteError::NotFound(_))
        ));
        assert!(matches!(
            ready("11", "/token"),
            Err(RemoteError::Unauthorized(_))
        ));
        assert!(matches!(
            ready("14", "/pcm415"),
            Err(RemoteError::PreviewUnavailable { .. })
        ));
        assert!(matches!(
            ready("15", "/noffmpeg"),
            Err(RemoteError::PreviewUnavailable { .. })
        ));
        // A stream's 503 UNAVAILABLE is an offline root (§6.4): the server's message, at
        // once, without waiting out Retry-After.
        let started = Instant::now();
        match ready("12", "/starting") {
            Err(RemoteError::RootOffline {
                message,
                retry_after: Some(5),
            }) => assert!(
                message.starts_with("Library root \"SFX\" is offline"),
                "{message}"
            ),
            other => panic!("{other:?}"),
        }
        assert!(started.elapsed() < Duration::from_secs(2));
        let empty = start("13", format!("{}/empty", server.url));
        assert_eq!(
            empty.wait_ready(&cancel, Duration::from_secs(5)).unwrap(),
            0
        );
        assert_eq!(empty.wait_complete(Duration::from_secs(5)).unwrap(), 0);
        assert!(read_all(&empty).is_empty());
    }

    fn busy_reply(retry_after: Option<&str>) -> TestResponse {
        let response = TestResponse::json(
            503,
            r#"{"error_code":"BUSY","message":"too many audio streams"}"#,
        );
        match retry_after {
            Some(value) => response.with_header("Retry-After", value),
            None => response,
        }
    }

    #[test]
    fn retry_backoff_doubles_honors_retry_after_and_is_capped() {
        let busy = |retry_after| RemoteError::Busy {
            message: "cap".into(),
            retry_after,
        };
        let waits: Vec<u128> = (1..=STREAM_RETRIES)
            .map(|attempt| retry_backoff(attempt, &busy(Some(1))).as_millis())
            .collect();
        // Retry-After wins over the (shorter or longer) backoff.
        assert_eq!(waits, [1000, 1000, 1000]);
        assert_eq!(
            retry_backoff(1, &busy(Some(3))),
            Duration::from_secs(3),
            "a longer Retry-After than the backoff is honoured"
        );
        assert_eq!(
            retry_backoff(1, &RemoteError::Unreachable("x".into())),
            Duration::from_millis(500)
        );
        assert_eq!(retry_backoff(1, &busy(Some(30))), MAX_RETRY_WAIT);
        assert_eq!(retry_backoff(3, &busy(None)), Duration::from_secs(2));
    }

    #[test]
    fn busy_stream_cap_is_retried_after_retry_after_and_then_streams() {
        let bytes = data(300_000, 5);
        let requests = Arc::new(AtomicUsize::new(0));
        let server = {
            let (bytes, requests) = (bytes.clone(), requests.clone());
            TestServer::start(move |request| {
                if requests.fetch_add(1, Ordering::SeqCst) < 2 {
                    return busy_reply(Some("1"));
                }
                serve(request, &bytes, "\"v1\"")
            })
        };
        let started = Instant::now();
        let stream = start("20", format!("{}/s", server.url));
        assert_eq!(
            stream
                .wait_ready(&AtomicBool::new(false), Duration::from_secs(10))
                .unwrap(),
            bytes.len() as u64
        );
        // Two waits of max(0.5 s / 1 s backoff, Retry-After: 1).
        assert!(started.elapsed() >= Duration::from_millis(1900));
        stream.wait_complete(Duration::from_secs(10)).unwrap();
        assert_eq!(read_all(&stream), *bytes);
        assert_eq!(requests.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn insufficient_storage_is_reported_at_once_without_retry() {
        let requests = Arc::new(AtomicUsize::new(0));
        let server = {
            let requests = requests.clone();
            TestServer::start(move |_| {
                requests.fetch_add(1, Ordering::SeqCst);
                TestResponse::json(
                    507,
                    r#"{"error_code":"INSUFFICIENT_STORAGE","message":"disk full"}"#,
                )
                .with_header("Retry-After", "1")
            })
        };
        let started = Instant::now();
        let stream = start("22", format!("{}/s", server.url));
        let result = stream.wait_ready(&AtomicBool::new(false), Duration::from_secs(15));
        assert!(
            matches!(result, Err(RemoteError::InsufficientStorage(_))),
            "{result:?}"
        );
        assert_eq!(
            requests.load(Ordering::SeqCst),
            1,
            "507 must not be retried"
        );
        assert!(started.elapsed() < Duration::from_millis(900));
    }

    #[test]
    fn busy_stream_cap_gives_up_after_bounded_retries() {
        let requests = Arc::new(AtomicUsize::new(0));
        let server = {
            let requests = requests.clone();
            TestServer::start(move |_| {
                requests.fetch_add(1, Ordering::SeqCst);
                busy_reply(None)
            })
        };
        let stream = start("21", format!("{}/s", server.url));
        let result = stream.wait_ready(&AtomicBool::new(false), Duration::from_secs(15));
        assert!(
            matches!(result, Err(RemoteError::Busy { .. })),
            "{result:?}"
        );
        assert_eq!(requests.load(Ordering::SeqCst), 1 + STREAM_RETRIES as usize);
    }

    #[test]
    fn cancel_fails_waiting_reads_and_keeps_what_arrived() {
        let bytes = data(2 * 1024 * 1024, 9);
        let server = {
            let bytes = bytes.clone();
            TestServer::start(move |request| {
                serve(request, &bytes, "\"v1\"").slow(16 * 1024, Duration::from_millis(20))
            })
        };
        let stream = start("22", format!("{}/s", server.url));
        ready(&stream);
        assert!(wait_until(|| stream.is_buffered(0, 32 * 1024)));
        let mut reader = stream.reader().unwrap();
        stream.cancel();
        assert!(wait_until(|| !stream.is_active()));
        let started = Instant::now();
        reader.seek(SeekFrom::Start(1_900_000)).unwrap();
        assert!(reader.read(&mut [0; 16]).is_err());
        assert!(started.elapsed() < Duration::from_secs(2));
        // The bytes that had arrived are still readable.
        reader.seek(SeekFrom::Start(0)).unwrap();
        let mut head = vec![0; 1000];
        reader.read_exact(&mut head).unwrap();
        assert_eq!(head, bytes[..1000]);
        assert!(matches!(
            stream.wait_complete(Duration::from_secs(1)),
            Err(RemoteError::Cancelled)
        ));
    }

    #[test]
    fn cancel_during_the_first_byte_wait_closes_the_connection() {
        let (url, closed) = crate::abort::tests::silent_server();
        let stream = start("23", url);
        // Let the request reach the server, which never answers.
        std::thread::sleep(Duration::from_millis(300));
        let cancelled_at = Instant::now();
        stream.cancel();
        let seen = closed
            .recv_timeout(Duration::from_secs(5))
            .expect("the server must see the connection closed");
        assert!(seen.duration_since(cancelled_at) < Duration::from_secs(2));
        assert!(wait_until(|| !stream.is_active()));
    }

    /// Accepts any number of connections, never answers; counts the open ones.
    fn silent_server_counting() -> (String, Arc<AtomicUsize>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!(
            "http://{}/api/v1/audio/1/stream",
            listener.local_addr().unwrap()
        );
        let open = Arc::new(AtomicUsize::new(0));
        let counter = open.clone();
        std::thread::spawn(move || {
            for socket in listener.incoming() {
                let Ok(mut socket) = socket else { continue };
                let counter = counter.clone();
                counter.fetch_add(1, Ordering::SeqCst);
                std::thread::spawn(move || {
                    let _ = socket.set_read_timeout(Some(Duration::from_secs(20)));
                    let mut buffer = [0_u8; 4096];
                    while let Ok(read) = socket.read(&mut buffer) {
                        if read == 0 {
                            break;
                        }
                    }
                    counter.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });
        (url, open)
    }

    #[test]
    fn superseded_selections_close_their_connections() {
        // Rapid selection: each new stream replaces the previous one, whose request the
        // server sees go away; connections never pile up.
        let (url, open) = silent_server_counting();
        let cache = cache();
        let mut current = None;
        for n in 0..20 {
            current = Some(start_in(&cache, &n.to_string(), format!("{url}?{n}")));
            std::thread::sleep(Duration::from_millis(30));
            assert!(
                open.load(Ordering::SeqCst) <= 2,
                "{} open",
                open.load(Ordering::SeqCst)
            );
        }
        assert!(
            wait_until(|| open.load(Ordering::SeqCst) == 1),
            "{} open after the last selection",
            open.load(Ordering::SeqCst)
        );
        drop(current);
        assert!(
            wait_until(|| open.load(Ordering::SeqCst) == 0),
            "{} open after dropping it",
            open.load(Ordering::SeqCst)
        );
    }

    /// Mocked NAS wake-up: the first request waits this long before any byte (the real
    /// disk takes 20-30 s); later range requests answer at once.
    const WAKE_DELAY: Duration = Duration::from_secs(3);

    fn waking_server(bytes: Arc<Vec<u8>>, requests: Arc<AtomicUsize>) -> TestServer {
        TestServer::start(move |request| {
            let first = requests.fetch_add(1, Ordering::SeqCst) == 0;
            let response = serve(request, &bytes, "\"v1\"");
            if first {
                response.delayed(WAKE_DELAY)
            } else {
                response
            }
        })
    }

    #[test]
    fn slow_first_byte_is_waited_for_and_reported_without_retries() {
        let bytes = data(200_000, 3);
        let requests = Arc::new(AtomicUsize::new(0));
        let server = waking_server(bytes.clone(), requests.clone());
        let stream = start("30", format!("{}/s", server.url));
        assert_eq!(stream.first_byte_timeout(), FIRST_BYTE_TIMEOUT);
        let started = Instant::now();
        let mut longest = Duration::ZERO;
        let total = stream
            .wait_ready_with(
                &AtomicBool::new(false),
                stream.first_byte_timeout() + Duration::from_secs(5),
                &mut |waited| longest = longest.max(waited),
            )
            .unwrap();
        assert_eq!(total, 200_000);
        assert!(started.elapsed() >= WAKE_DELAY - Duration::from_millis(200));
        assert!(
            longest >= WAKING_HINT_AFTER,
            "the waking hint threshold was reached"
        );
        stream.wait_complete(Duration::from_secs(10)).unwrap();
        assert_eq!(read_all(&stream), *bytes);
        assert_eq!(
            requests.load(Ordering::SeqCst),
            1,
            "a slow first byte is not retried"
        );
    }

    #[test]
    fn waiting_for_the_first_byte_is_cancelled_promptly() {
        let server = waking_server(data(50_000, 4), Arc::new(AtomicUsize::new(0)));
        let stream = start("31", format!("{}/s", server.url));
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let cancel = cancel.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(500));
                cancel.store(true, Ordering::Release);
            });
        }
        let started = Instant::now();
        let result = stream.wait_ready_with(&cancel, Duration::from_secs(100), &mut |_| {});
        assert!(matches!(result, Err(RemoteError::Cancelled)), "{result:?}");
        assert!(started.elapsed() < Duration::from_millis(1200));
    }

    #[test]
    fn first_byte_timeout_fails_after_the_limit_without_retries() {
        let requests = Arc::new(AtomicUsize::new(0));
        let server = waking_server(data(10_000, 5), requests.clone());
        let stream = MemoryStream::start(
            &cache(),
            "32",
            StreamSource::new(format!("{}/s", server.url), SourceAuth::None)
                .with_first_byte_timeout(Duration::from_secs(1)),
        )
        .unwrap();
        let started = Instant::now();
        let result = stream.wait_ready(&AtomicBool::new(false), Duration::from_secs(10));
        let elapsed = started.elapsed();
        assert!(
            matches!(&result, Err(RemoteError::Timeout(message)) if message.contains("no response within 1 s")),
            "{result:?}"
        );
        assert!(
            elapsed >= Duration::from_millis(900) && elapsed < WAKE_DELAY,
            "{elapsed:?}"
        );
        assert_eq!(requests.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn interrupted_reads_fail_at_once_while_the_download_continues() {
        let bytes = data(256 * 1024, 6);
        let server = {
            let bytes = bytes.clone();
            TestServer::start(move |request| {
                serve(request, &bytes, "\"v1\"").slow(16 * 1024, Duration::from_millis(250))
            })
        };
        let stream = start("33", format!("{}/s", server.url));
        ready(&stream);
        let mut reader = stream.reader().unwrap();
        // Within the smallest seek-jump distance: the read waits for the download.
        reader.seek(SeekFrom::Start(60_000)).unwrap();
        {
            let stream = stream.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(300));
                stream.interrupt_reads();
            });
        }
        let started = Instant::now();
        let error = reader.read(&mut [0; 16]).unwrap_err();
        assert!(error.to_string().contains("interrupted"), "{error}");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
        stream.wait_complete(Duration::from_secs(15)).unwrap();
        assert_eq!(read_all(&stream), *bytes);
    }

    #[test]
    fn pcm_preview_headers_are_captured() {
        let bytes = data(44 + 4800 * 4, 1);
        let server = {
            let bytes = bytes.clone();
            TestServer::start(move |request| {
                let mut response = serve(request, &bytes, "\"abc-pcm2-48000-16-2\"");
                for (name, value) in [
                    ("X-Preview-Sample-Rate", "48000"),
                    ("X-Preview-Bits", "16"),
                    ("X-Preview-Channels", "2"),
                    ("X-Preview-Frames", "4800"),
                ] {
                    response = response.with_header(name, value);
                }
                response
            })
        };
        let stream = start("pcm", format!("{}/p?codec=pcm", server.url));
        stream.wait_complete(Duration::from_secs(5)).unwrap();
        assert_eq!(
            stream.preview_format(),
            Some(PreviewFormat {
                sample_rate: 48_000,
                bits: 16,
                channels: 2,
                frames: Some(4800),
            })
        );
        // The ETag is opaque: whatever the server sends identifies the version.
        assert_eq!(stream.etag().as_deref(), Some("\"abc-pcm2-48000-16-2\""));
    }

    // ---- FLAC preview ----------------------------------------------------------------

    /// Minimal FLAC encoder (verbatim subframes, fixed block size) for test streams.
    struct Bits {
        bytes: Vec<u8>,
        used: u32,
    }

    impl Bits {
        fn new() -> Self {
            Self {
                bytes: Vec::new(),
                used: 0,
            }
        }
        fn put(&mut self, value: u64, bits: u32) {
            if self.used == 0 && bits.is_multiple_of(8) {
                for byte in (0..bits / 8).rev() {
                    self.bytes.push((value >> (byte * 8)) as u8);
                }
                return;
            }
            for bit in (0..bits).rev() {
                if self.used == 0 {
                    self.bytes.push(0);
                }
                let last = self.bytes.last_mut().unwrap();
                *last |= (((value >> bit) & 1) as u8) << (7 - self.used);
                self.used = (self.used + 1) % 8;
            }
        }
        fn align(&mut self) {
            self.used = 0;
        }
    }

    fn crc8(bytes: &[u8]) -> u8 {
        let mut crc = 0_u8;
        for &byte in bytes {
            crc ^= byte;
            for _ in 0..8 {
                crc = if crc & 0x80 != 0 {
                    (crc << 1) ^ 0x07
                } else {
                    crc << 1
                };
            }
        }
        crc
    }

    fn crc16(bytes: &[u8]) -> u16 {
        let mut crc = 0_u16;
        for &byte in bytes {
            crc ^= u16::from(byte) << 8;
            for _ in 0..8 {
                crc = if crc & 0x8000 != 0 {
                    (crc << 1) ^ 0x8005
                } else {
                    crc << 1
                };
            }
        }
        crc
    }

    const FLAC_RATE: u32 = 8000;
    const FLAC_BLOCK: u64 = 1024;

    fn sample(frame: u64, channel: u64) -> i16 {
        (((frame * 7 + channel * 1000) % 20_000) as i32 - 10_000) as i16
    }

    /// A 16-bit stereo FLAC stream of frames `first..end` (unknown total length).
    fn flac_stream(first: u64, end: u64) -> Vec<u8> {
        let mut out = b"fLaC".to_vec();
        let mut info = Bits::new();
        info.put(1, 1);
        info.put(0, 7);
        info.put(34, 24);
        info.put(FLAC_BLOCK, 16);
        info.put(FLAC_BLOCK, 16);
        info.put(0, 24);
        info.put(0, 24);
        info.put(u64::from(FLAC_RATE), 20);
        info.put(1, 3);
        info.put(15, 5);
        info.put(0, 36);
        info.put(0, 64);
        info.put(0, 64);
        out.extend(info.bytes);
        let mut at = first;
        let mut number = 0_u64;
        while at < end {
            let block = FLAC_BLOCK.min(end - at);
            let mut frame = Bits::new();
            frame.put(0b11_1111_1111_1110, 14);
            frame.put(0, 2);
            frame.put(0b0111, 4);
            frame.put(0b0000, 4);
            frame.put(1, 4);
            frame.put(0b100, 3);
            frame.put(0, 1);
            // UTF-8 style frame number.
            if number < 0x80 {
                frame.put(number, 8);
            } else if number < 0x800 {
                frame.put(0b110_00000 | (number >> 6), 8);
                frame.put(0b10_000000 | (number & 0x3f), 8);
            } else {
                assert!(number < 0x10000);
                frame.put(0b1110_0000 | (number >> 12), 8);
                frame.put(0b10_000000 | ((number >> 6) & 0x3f), 8);
                frame.put(0b10_000000 | (number & 0x3f), 8);
            }
            frame.put(block - 1, 16);
            let crc = crc8(&frame.bytes);
            frame.put(u64::from(crc), 8);
            for channel in 0..2 {
                frame.put(0, 1);
                frame.put(0b000001, 6);
                frame.put(0, 1);
                for index in at..at + block {
                    frame.put(u64::from(sample(index, channel) as u16), 16);
                }
            }
            frame.align();
            let crc = crc16(&frame.bytes);
            frame.put(u64::from(crc), 16);
            out.extend(frame.bytes);
            at += block;
            number += 1;
        }
        out
    }

    /// `codec=flac` per §8.1: `t=` → X-Start-Frame, chunked FLAC without a length.
    fn flac_server(frames: u64, log: Arc<Mutex<Vec<String>>>) -> TestServer {
        flac_server_with(frames, log, false)
    }

    /// [`flac_server`]; `lengths` adds the 0.3.6 headers: X-Total-Frames / X-Duration
    /// counted from the start position, and a weak ETag that varies with `t=`.
    fn flac_server_with(frames: u64, log: Arc<Mutex<Vec<String>>>, lengths: bool) -> TestServer {
        TestServer::start(move |request| {
            log.lock().unwrap().push(request.path.clone());
            let t: f64 = request
                .path
                .split(['?', '&'])
                .find_map(|part| part.strip_prefix("t="))
                .map_or(0.0, |value| value.parse().unwrap());
            let first = (t * f64::from(FLAC_RATE)).round() as u64;
            if first >= frames {
                return TestResponse::json(416, r#"{"error_code":"RANGE_NOT_SATISFIABLE"}"#);
            }
            let mut response = TestResponse::bytes(200, flac_stream(first, frames))
                .with_header("X-Preview-Sample-Rate", &FLAC_RATE.to_string())
                .with_header("X-Preview-Bits", "16")
                .with_header("X-Preview-Channels", "2")
                .with_header("X-Start-Frame", &first.to_string())
                .with_header("X-Start-Time", &format!("{:.6}", first as f64 / 8000.0));
            if lengths {
                let left = frames - first;
                response = response
                    .with_header("X-Total-Frames", &left.to_string())
                    .with_header("X-Duration", &format!("{:.6}", left as f64 / 8000.0))
                    .with_header("ETag", &format!("W/\"src-flac1-{first}\""));
            }
            response.slow(16 * 1024, Duration::from_millis(10))
        })
    }

    fn pcm_bytes(first: u64, end: u64) -> Vec<u8> {
        (first..end)
            .flat_map(|frame| {
                let [a, b] = sample(frame, 0).to_le_bytes();
                let [c, d] = sample(frame, 1).to_le_bytes();
                [a, b, c, d]
            })
            .collect()
    }

    #[test]
    fn a_flac_preview_is_decoded_into_a_seekable_pcm_wav() {
        let log = Arc::new(Mutex::new(Vec::new()));
        // 50 s of audio; the record says 50.1 s: the last 0.1 s is silence.
        let server = flac_server(400_000, log.clone());
        let stream = MemoryStream::start(
            &cache(),
            "flac",
            StreamSource::flac(
                format!("{}/api/v1/audio/9/preview?codec=flac&sr=8000", server.url),
                SourceAuth::None,
                Duration::from_millis(50_100),
            ),
        )
        .unwrap();
        let total = ready(&stream);
        assert_eq!(total, 44 + 400_800 * 4);
        let format = stream.preview_format().unwrap();
        assert_eq!(
            (format.sample_rate, format.bits, format.channels),
            (8000, 16, 2)
        );
        // Seek far ahead: the player only asks; a `t=` request starts one frame early.
        let frame = 380_000_u64;
        let offset = 44 + frame * 4;
        assert!(!stream.is_buffered(offset, 4096));
        assert!(wait_until(|| stream.is_buffered(offset, 4096)));
        let expected = format!("&t={:.6}", (frame - 1) as f64 / 8000.0);
        assert_eq!(expected, "&t=47.499875");
        assert!(
            log.lock()
                .unwrap()
                .iter()
                .any(|path| path.ends_with(&expected)),
            "{:?}",
            log.lock().unwrap()
        );
        let mut reader = stream.reader().unwrap();
        reader.seek(SeekFrom::Start(offset)).unwrap();
        let mut piece = vec![0; 4096];
        reader.read_exact(&mut piece).unwrap();
        assert_eq!(piece, pcm_bytes(frame, frame + 1024));
        // The whole file: header, every sample in place, then silence to the duration.
        stream.wait_complete(Duration::from_secs(20)).unwrap();
        let all = read_all(&stream);
        assert_eq!(all.len() as u64, total);
        assert_eq!(&all[..4], b"RIFF");
        assert_eq!(&all[36..40], b"data");
        assert_eq!(u32::from_le_bytes(all[24..28].try_into().unwrap()), 8000);
        assert!(all[44..44 + 1_600_000] == pcm_bytes(0, 400_000)[..]);
        assert!(all[44 + 1_600_000..].iter().all(|&byte| byte == 0));
    }

    #[test]
    fn a_flac_seek_past_the_real_end_is_silence() {
        let log = Arc::new(Mutex::new(Vec::new()));
        // 1 s of audio, the record says 3 s.
        let server = flac_server(8000, log.clone());
        let stream = MemoryStream::start(
            &cache(),
            "flac-end",
            StreamSource::flac(
                format!("{}/p?codec=flac", server.url),
                SourceAuth::None,
                Duration::from_secs(3),
            ),
        )
        .unwrap();
        ready(&stream);
        let offset = 44 + 20_000 * 4;
        assert!(!stream.is_buffered(offset, 1000));
        assert!(wait_until(|| stream.is_buffered(offset, 1000)));
        let mut reader = stream.reader().unwrap();
        reader.seek(SeekFrom::Start(offset)).unwrap();
        let mut piece = vec![1; 1000];
        reader.read_exact(&mut piece).unwrap();
        assert!(piece.iter().all(|&byte| byte == 0));
    }

    #[test]
    fn the_servers_flac_length_headers_win_over_the_records_duration() {
        // A 0.3.6 server: 1 s of audio by X-Total-Frames (counted from the start), the
        // record says 3 s. The weak ETag that changes with `t=` never fails the stream.
        let log = Arc::new(Mutex::new(Vec::new()));
        let server = flac_server_with(8000, log.clone(), true);
        let stream = MemoryStream::start(
            &cache(),
            "flac-lengths",
            StreamSource::flac(
                format!("{}/p?codec=flac", server.url),
                SourceAuth::None,
                Duration::from_secs(3),
            ),
        )
        .unwrap();
        assert_eq!(ready(&stream), 44 + 8000 * 4);
        let offset = 44 + 6000 * 4;
        assert!(!stream.is_buffered(offset, 4000));
        assert!(wait_until(|| stream.is_buffered(offset, 4000)));
        stream.wait_complete(Duration::from_secs(10)).unwrap();
        let all = read_all(&stream);
        assert!(all[44..] == pcm_bytes(0, 8000)[..]);
    }

    #[test]
    fn transfers_feed_the_bandwidth_estimate() {
        let bytes = data(400_000, 2);
        let server = {
            let bytes = bytes.clone();
            TestServer::start(move |request| {
                serve(request, &bytes, "\"v1\"").slow(32 * 1024, Duration::from_millis(15))
            })
        };
        let stream = start("bw", format!("{}/s", server.url));
        stream.wait_complete(Duration::from_secs(10)).unwrap();
        assert!(bandwidth().estimate_bps().is_some_and(|bps| bps > 0.0));
    }
}
