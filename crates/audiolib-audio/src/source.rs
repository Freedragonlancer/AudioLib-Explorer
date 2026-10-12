// crates/audiolib-audio/src/source.rs
//! Byte sources behind playback paths.
//!
//! Playback identity stays path based (`AudioCommand::Load(PathBuf)`), but a caller may
//! register an opener for a path so decoders read through any `Read + Seek` adapter (for
//! example a network range stream that fills a local cache file) instead of `File::open`.
//! Lookups happen only when a decoder opens its source on the engine's command/fill worker;
//! the output callback never touches the registry.
//!
//! A network source can also register a [`SourceBuffer`]: the engine then asks it whether
//! the bytes the decoder needs next are in memory, and pauses playback with a short fade
//! ("buffering") instead of letting a decoder read block while the output plays.
use std::{
    collections::BTreeMap,
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
};

/// A seekable byte stream the decoder can read audio from.
pub trait ByteSource: Read + Seek + Send + Sync {}
impl<T: Read + Seek + Send + Sync + ?Sized> ByteSource for T {}

/// Opens a fresh, independent reader positioned at byte 0 and returns the total byte length.
/// Each decoder (load, seek re-open, audition) calls it once on a worker thread.
pub type SourceOpener = Arc<dyn Fn() -> io::Result<(Box<dyn ByteSource>, u64)> + Send + Sync>;

/// What a network-backed source already holds in memory. Queried on the engine worker
/// (never in the output callback); implementations must not block.
pub trait SourceBuffer: Send + Sync {
    /// Whether bytes `offset..offset + len` (clamped to the end of the source) can be read
    /// now without waiting for the network.
    fn is_buffered(&self, offset: u64, len: u64) -> bool;
}

struct Registered {
    opener: SourceOpener,
    buffer: Option<Arc<dyn SourceBuffer>>,
}

static REGISTERED: AtomicUsize = AtomicUsize::new(0);
static REGISTRY: Mutex<BTreeMap<PathBuf, Registered>> = Mutex::new(BTreeMap::new());

fn registry() -> MutexGuard<'static, BTreeMap<PathBuf, Registered>> {
    REGISTRY.lock().unwrap_or_else(PoisonError::into_inner)
}

fn register(path: PathBuf, opener: SourceOpener, buffer: Option<Arc<dyn SourceBuffer>>) {
    let mut registry = registry();
    registry.insert(path, Registered { opener, buffer });
    REGISTERED.store(registry.len(), Ordering::Release);
}

/// Route decoder opens for `path` through `opener` (replacing an earlier registration).
pub fn register_stream_source(path: impl Into<PathBuf>, opener: SourceOpener) {
    register(path.into(), opener, None);
}

/// [`register_stream_source`] for a source that fills from the network: playback waits
/// (buffering, without glitches) until `buffer` holds the bytes the decoder needs.
pub fn register_buffered_stream_source(
    path: impl Into<PathBuf>,
    opener: SourceOpener,
    buffer: Arc<dyn SourceBuffer>,
) {
    register(path.into(), opener, Some(buffer));
}

/// Remove a registration; later opens of `path` use the file system again.
pub fn unregister_stream_source(path: &Path) -> bool {
    let mut registry = registry();
    let removed = registry.remove(path).is_some();
    REGISTERED.store(registry.len(), Ordering::Release);
    removed
}

/// Whether decoder opens of `path` currently go through a registered opener.
pub fn is_stream_source(path: &Path) -> bool {
    REGISTERED.load(Ordering::Acquire) > 0 && registry().contains_key(path)
}

/// Open `path` through its registered opener, or as a regular file.
pub(crate) fn open_source(path: &Path) -> io::Result<(Box<dyn ByteSource>, u64)> {
    if REGISTERED.load(Ordering::Acquire) > 0 {
        let opener = registry().get(path).map(|entry| entry.opener.clone());
        if let Some(opener) = opener {
            return opener();
        }
    }
    let file = File::open(path)?;
    let len = file.metadata()?.len();
    Ok((Box::new(file), len))
}

/// The [`SourceBuffer`] registered for `path`, if any.
pub(crate) fn source_buffer(path: &Path) -> Option<Arc<dyn SourceBuffer>> {
    if REGISTERED.load(Ordering::Acquire) == 0 {
        return None;
    }
    registry().get(path).and_then(|entry| entry.buffer.clone())
}

/// Reader that publishes its byte position, so a [`SourceBuffer`] can be asked about the
/// bytes after it.
pub(crate) struct Tracked {
    inner: Box<dyn ByteSource>,
    position: Arc<AtomicU64>,
    at: u64,
}

impl Tracked {
    pub(crate) fn new(inner: Box<dyn ByteSource>, position: Arc<AtomicU64>) -> Self {
        position.store(0, Ordering::Release);
        Self {
            inner,
            position,
            at: 0,
        }
    }
}

impl Read for Tracked {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buf)?;
        self.at += read as u64;
        self.position.store(self.at, Ordering::Release);
        Ok(read)
    }
}

impl Seek for Tracked {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.at = self.inner.seek(pos)?;
        self.position.store(self.at, Ordering::Release);
        Ok(self.at)
    }
}

/// Symphonia media source over any [`ByteSource`] with a known length.
pub(crate) struct MediaAdapter {
    inner: Box<dyn ByteSource>,
    len: u64,
}

impl MediaAdapter {
    pub(crate) fn new(inner: Box<dyn ByteSource>, len: u64) -> Self {
        Self { inner, len }
    }
}

impl Read for MediaAdapter {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl Seek for MediaAdapter {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.inner.seek(pos)
    }
}

impl symphonia::core::io::MediaSource for MediaAdapter {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        Some(self.len)
    }
}
