// crates/audiolib-remote/src/list_waveforms.rs
//! Server-mode list thumbnail Waveforms (doc/api/README.md §9.1–§9.3), client side.
//!
//! * [`ListWaveformsCapability`]: `GET /capabilities` `list_waveforms`. Only a server that
//!   reports version 1, `buckets` 128 and the `i16le-minmax-base64` encoding is used;
//!   otherwise the list hides its Waveform column. There is **no** fallback to per-item
//!   `GET /audio/{id}/waveform`.
//! * [`RemoteClient::list_waveform_batch`]: one `POST /waveforms` (up to 200 ids with the
//!   `known` entity tags, `Accept-Encoding: zstd`) over the shared keep-alive pool with the
//!   same authentication as every other request.
//! * [`ListWaveformStore`]: the client disk cache
//!   `<cache>/waveforms/server/<library_id hash>/list/`, one file per 1024 ids
//!   (client-private format, checksummed), replaced atomically, keyed by entity tag and
//!   bounded (640 MiB by default) by evicting least recently used shards.
//! * [`RemoteListWaveforms`]: the lookup a [`crate::RemoteProvider`] answers
//!   `DataProvider::list_waveforms` with: at most one `POST /waveforms` in flight per
//!   client, `503 BUSY` turned into a deferral by `Retry-After`, replies for another
//!   `library_id` discarded, `ok` stored, `not_modified` answered from the store, stale
//!   copies offered with `pending` and `root_offline`.
//! * [`RemoteClient::server_settings`] / [`RemoteClient::set_pregenerate_list_waveforms`] /
//!   [`RemoteClient::list_waveform_status`]: `GET`/`PATCH /settings` and
//!   `GET /waveforms/status` for the 服务器音效库 page.
//!
//! Retry policy (only still-visible `pending` ids, at most 5 times; `root_offline` after
//! `/roots` reports the root online; no automatic retry of `missing` or `error`) and the
//! audition yield (no POST during the first 3 s after a stream or preview starts) belong to
//! the desktop's list Waveform worker, which knows what is visible and playing.
use crate::client::{RemoteClient, call_cancellable, retry_after};
use crate::error::{RemoteError, Result};
use crate::manage::Capabilities;
use audiolib_core::{
    LIST_WAVEFORM_COLUMNS, LIST_WAVEFORM_ENCODING_VERSION, ListWaveform, ListWaveformBatch,
    ListWaveformItem, ListWaveformSource, ListWaveformState, MAX_LIST_WAVEFORM_IDS,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// `encoding` of a usable `list_waveforms` capability.
pub const LIST_WAVEFORM_ENCODING: &str = "i16le-minmax-base64";
/// Default size limit of one library's list Waveform disk cache (640 MiB).
pub const DEFAULT_LIST_WAVEFORM_CACHE_BYTES: u64 = 640 * 1024 * 1024;
/// Ids per cache shard file.
pub const LIST_WAVEFORM_SHARD_IDS: i64 = 1024;
/// `POST /waveforms` answers within the server's 2 s `stat` budget; its general limit
/// is 30 s.
const BATCH_TIMEOUT: Duration = Duration::from_secs(30);
/// Reply body limit after decompression (200 items are about 150 KB).
const MAX_BATCH_BYTES: u64 = 8 * 1024 * 1024;
/// Parsed shards kept in memory (each at most 1024 entries, about 0.6 MB).
const MEMORY_SHARDS: usize = 16;
/// A failed capability probe is repeated after this long.
const CAPABILITY_RETRY: Duration = Duration::from_secs(30);
/// Deferral of a `503 BUSY` without `Retry-After`.
const BUSY_DEFAULT: Duration = Duration::from_secs(1);
const SHARD_MAGIC: &[u8; 8] = b"ALXLWS1\n";
const SHARD_EXTENSION: &str = "lws";

/// `list_waveforms` of `GET /capabilities` (§9.2).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ListWaveformsCapability {
    /// List encoding version (`-l<version>` of the entity tags).
    pub version: u32,
    /// Accepted `buckets` values.
    pub buckets: Vec<u32>,
    /// Most ids per request.
    pub max_ids: usize,
    /// `peaks` encoding.
    pub encoding: String,
}

impl ListWaveformsCapability {
    /// Whether this client understands the server's list Waveforms.
    pub fn usable(&self) -> bool {
        self.version == LIST_WAVEFORM_ENCODING_VERSION
            && self.buckets.contains(&LIST_WAVEFORM_COLUMNS)
            && self.encoding == LIST_WAVEFORM_ENCODING
    }

    /// Ids per request: the server's `max_ids`, at most the UI page size.
    pub fn batch_limit(&self) -> usize {
        match self.max_ids {
            0 => MAX_LIST_WAVEFORM_IDS,
            limit => limit.min(MAX_LIST_WAVEFORM_IDS),
        }
    }
}

/// Per-item `status` of a `POST /waveforms` reply.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchStatus {
    Ok,
    NotModified,
    Pending,
    Missing,
    RootOffline,
    Error,
    /// A status this client does not know (treated like `pending`).
    #[serde(other)]
    Unknown,
}

/// One item of a `POST /waveforms` reply.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatchItem {
    pub id: i64,
    pub status: BatchStatus,
    #[serde(default)]
    pub etag: Option<String>,
    #[serde(default)]
    pub n: Option<usize>,
    #[serde(default)]
    pub peaks: Option<String>,
    #[serde(default)]
    pub root: Option<String>,
    #[serde(default)]
    pub error_code: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}

/// `POST /waveforms` reply.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatchReply {
    /// Library that answered; a different one than the open snapshot's is discarded.
    pub library_id: String,
    #[serde(default)]
    pub buckets: u32,
    /// Suggested retry interval of `pending` items (seconds).
    #[serde(default)]
    pub retry_after: Option<u64>,
    #[serde(default)]
    pub items: Vec<BatchItem>,
}

/// `GET /settings` / `PATCH /settings` reply (§9.2).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ServerSettings {
    pub pregenerate_list_waveforms: bool,
    /// `default`, `config` or `admin`.
    pub pregenerate_list_waveforms_source: String,
    /// The server would accept a `PATCH /settings` from this client (a hint).
    pub can_change: bool,
}

/// `GET /waveforms/status` (§9.2).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ListWaveformStatus {
    pub enabled: bool,
    /// `off`, `idle`, `running`, `yielding` or `paused`.
    pub state: String,
    /// `scan` or `audio` while yielding.
    pub yield_reason: Option<String>,
    pub total: u64,
    pub cached: u64,
    pub remaining: Option<u64>,
    pub queued_on_demand: u64,
    pub failed: u64,
    pub skipped_offline: u64,
    pub rate: u32,
    pub cache_bytes: u64,
    pub cache_limit_bytes: u64,
    pub over_budget: bool,
    pub pass_started_at: Option<i64>,
    pub pass_finished_at: Option<i64>,
}

fn decompress(context: &str, encoding: Option<&str>, body: Vec<u8>) -> Result<Vec<u8>> {
    match encoding {
        None => Ok(body),
        Some(value) if value.eq_ignore_ascii_case("identity") => Ok(body),
        Some(value) if value.eq_ignore_ascii_case("zstd") => {
            let mut plain = Vec::new();
            zstd::stream::read::Decoder::new(body.as_slice())
                .and_then(|decoder| decoder.take(MAX_BATCH_BYTES).read_to_end(&mut plain))
                .map_err(|error| RemoteError::Protocol(format!("{context}: zstd: {error}")))?;
            Ok(plain)
        }
        Some(other) => Err(RemoteError::Protocol(format!(
            "{context}: unexpected Content-Encoding {other}"
        ))),
    }
}

impl RemoteClient {
    /// `POST /waveforms` (§9.1) for 1..=200 ids with the entity tags of cached copies in
    /// `known`. `cancel` closes the connection at once. Errors keep their class:
    /// `503 BUSY` is [`RemoteError::Busy`] with the `Retry-After` seconds, a server without
    /// the endpoint is [`RemoteError::Unsupported`].
    pub fn list_waveform_batch(
        &self,
        ids: &[i64],
        known: &BTreeMap<i64, String>,
        cancel: &AtomicBool,
    ) -> Result<BatchReply> {
        if ids.is_empty() || ids.len() > MAX_LIST_WAVEFORM_IDS || ids.iter().any(|id| *id <= 0) {
            return Err(RemoteError::Validation(format!(
                "POST /waveforms takes 1..={MAX_LIST_WAVEFORM_IDS} positive ids"
            )));
        }
        let known: BTreeMap<String, &String> = known
            .iter()
            .filter(|(id, _)| ids.contains(id))
            .map(|(id, etag)| (id.to_string(), etag))
            .collect();
        let mut body = serde_json::json!({ "ids": ids, "buckets": LIST_WAVEFORM_COLUMNS });
        if !known.is_empty() {
            body["known"] = serde_json::json!(known);
        }
        let body = serde_json::to_vec(&body)
            .map_err(|error| RemoteError::Validation(format!("POST /waveforms: {error}")))?;
        let url = format!("{}/waveforms", self.api);
        let token = (!self.token.is_empty()).then(|| format!("Bearer {}", self.token));
        let context = "POST /waveforms";
        let (status, encoding, reply, retry) = call_cancellable(
            move |abort| {
                let agent = crate::abort::abortable_agent(BATCH_TIMEOUT, BATCH_TIMEOUT, abort);
                let send = || {
                    let mut request = agent
                        .post(&url)
                        .header("Accept-Encoding", "zstd")
                        .header("Content-Type", "application/json");
                    if let Some(token) = &token {
                        request = request.header("Authorization", token);
                    }
                    request.send(body.as_slice())
                };
                // Read-only despite the method: resent once on a stale pooled connection.
                let mut response = match send() {
                    Err(error) if crate::abort::stale_connection(&error) => send(),
                    result => result,
                }
                .map_err(|error| RemoteError::from_transport(error, &url))?;
                let encoding = response
                    .headers()
                    .get("content-encoding")
                    .and_then(|value| value.to_str().ok())
                    .map(|value| value.trim().to_owned());
                let retry = retry_after(response.headers());
                let reply = response
                    .body_mut()
                    .with_config()
                    .limit(MAX_BATCH_BYTES)
                    .read_to_vec()
                    .map_err(|error| RemoteError::from_transport(error, &url))?;
                Ok((response.status().as_u16(), encoding, reply, retry))
            },
            cancel,
            &mut |_| {},
        )?;
        if status == 200 {
            let json = decompress(context, encoding.as_deref(), reply)?;
            return serde_json::from_slice(&json)
                .map_err(|error| RemoteError::Protocol(format!("{context}: {error}")));
        }
        let text = decompress(context, encoding.as_deref(), reply)
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default();
        let error = match RemoteError::from_response(status, &text, context, retry) {
            RemoteError::NotFound(_) | RemoteError::Server { status: 405, .. } => {
                RemoteError::Unsupported(format!("{context}: the server has no list Waveforms"))
            }
            other => other,
        };
        Err(if self.token.is_empty() {
            error.without_token()
        } else {
            error
        })
    }

    /// `GET /settings` (§9.2): not gated by `manage_mode`.
    pub fn server_settings(&self) -> Result<ServerSettings> {
        self.manage_json("GET", "/settings", None)
    }

    /// `PATCH /settings` with `pregenerate_list_waveforms` (gated by `manage_mode`:
    /// `403 MANAGE_FORBIDDEN`; `409 BUSY` while the settings lock is taken).
    pub fn set_pregenerate_list_waveforms(&self, enabled: bool) -> Result<ServerSettings> {
        self.manage_json(
            "PATCH",
            "/settings",
            Some(&serde_json::json!({ "pregenerate_list_waveforms": enabled })),
        )
    }

    /// `GET /waveforms/status` (§9.2): pre-generation progress (kept in server memory).
    pub fn list_waveform_status(&self) -> Result<ListWaveformStatus> {
        self.manage_json("GET", "/waveforms/status", None)
    }
}

fn checksum(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// One stored list Waveform and its server entity tag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredListWaveform {
    pub etag: String,
    pub waveform: ListWaveform,
}

type Shard = BTreeMap<i64, StoredListWaveform>;

fn encode_shard(shard: &Shard) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(16 + shard.len() * 560);
    bytes.extend_from_slice(SHARD_MAGIC);
    bytes.extend_from_slice(&(shard.len() as u32).to_le_bytes());
    for (id, entry) in shard {
        let etag = entry.etag.as_bytes();
        bytes.extend_from_slice(&id.to_le_bytes());
        bytes.extend_from_slice(&(etag.len() as u16).to_le_bytes());
        bytes.extend_from_slice(etag);
        bytes.extend_from_slice(&(entry.waveform.columns() as u16).to_le_bytes());
        bytes.extend_from_slice(&entry.waveform.to_interleaved_le());
    }
    let sum = checksum(&bytes);
    bytes.extend_from_slice(&sum.to_le_bytes());
    bytes
}

fn decode_shard(bytes: &[u8]) -> Option<Shard> {
    let (body, sum) = bytes.split_at_checked(bytes.len().checked_sub(8)?)?;
    if checksum(body).to_le_bytes() != sum || !body.starts_with(SHARD_MAGIC) {
        return None;
    }
    fn take<'a>(rest: &mut &'a [u8], count: usize) -> Option<&'a [u8]> {
        let (head, tail) = rest.split_at_checked(count)?;
        *rest = tail;
        Some(head)
    }
    let mut rest = &body[SHARD_MAGIC.len()..];
    let count = u32::from_le_bytes(take(&mut rest, 4)?.try_into().ok()?);
    let mut shard = Shard::new();
    for _ in 0..count {
        let id = i64::from_le_bytes(take(&mut rest, 8)?.try_into().ok()?);
        let etag_len = u16::from_le_bytes(take(&mut rest, 2)?.try_into().ok()?) as usize;
        let etag = String::from_utf8(take(&mut rest, etag_len)?.to_vec()).ok()?;
        let columns = u16::from_le_bytes(take(&mut rest, 2)?.try_into().ok()?) as usize;
        let waveform = ListWaveform::from_interleaved_le(1, take(&mut rest, columns * 4)?).ok()?;
        shard.insert(id, StoredListWaveform { etag, waveform });
    }
    rest.is_empty().then_some(shard)
}

fn now_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos() as u64)
}

#[derive(Clone, Copy, Debug)]
struct ShardFile {
    bytes: u64,
    /// Last use (Unix nanoseconds): the file time at startup, then reads and writes.
    used: u64,
}

#[derive(Default)]
struct StoreInner {
    scanned: bool,
    files: HashMap<i64, ShardFile>,
    total: u64,
    memory: HashMap<i64, Shard>,
    memory_order: VecDeque<i64>,
}

/// Disk cache of one server library's list Waveforms (§9.3 client cache): shard
/// `id / 1024` per file, client-private format with a checksum, written to a temporary
/// file and renamed over the old one. Damaged shards are dropped. When the total exceeds
/// the limit, whole shards are evicted, least recently used first.
pub struct ListWaveformStore {
    directory: PathBuf,
    limit: u64,
    inner: Mutex<StoreInner>,
}

static TEMPORARY: AtomicU64 = AtomicU64::new(0);

impl ListWaveformStore {
    /// `<cache_root>/waveforms/server/<library_id hash>/list/` with the default limit.
    pub fn for_library(cache_root: &Path, library_id: &str) -> Self {
        Self::new(
            Self::library_directory(cache_root, library_id),
            DEFAULT_LIST_WAVEFORM_CACHE_BYTES,
        )
    }

    pub fn library_directory(cache_root: &Path, library_id: &str) -> PathBuf {
        cache_root
            .join("waveforms")
            .join("server")
            .join(format!("{:016x}", crate::util::fnv64(library_id)))
            .join("list")
    }

    pub fn new(directory: PathBuf, limit: u64) -> Self {
        Self {
            directory,
            limit,
            inner: Mutex::new(StoreInner::default()),
        }
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    fn lock(&self) -> MutexGuard<'_, StoreInner> {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if !inner.scanned {
            inner.scanned = true;
            self.scan(&mut inner);
        }
        inner
    }

    fn shard_path(&self, shard: i64) -> PathBuf {
        self.directory.join(format!("{shard}.{SHARD_EXTENSION}"))
    }

    /// Sizes and times of the existing shards; leftover temporary files are removed.
    fn scan(&self, inner: &mut StoreInner) {
        let Ok(entries) = std::fs::read_dir(&self.directory) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if name.contains(".tmp") {
                let _ = std::fs::remove_file(&path);
                continue;
            }
            let Some(shard) = name
                .strip_suffix(&format!(".{SHARD_EXTENSION}"))
                .and_then(|stem| stem.parse::<i64>().ok())
            else {
                continue;
            };
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            let used = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |elapsed| elapsed.as_nanos() as u64);
            inner.files.insert(
                shard,
                ShardFile {
                    bytes: metadata.len(),
                    used,
                },
            );
            inner.total += metadata.len();
        }
    }

    fn load<'a>(&self, inner: &'a mut StoreInner, shard: i64) -> &'a mut Shard {
        if !inner.memory.contains_key(&shard) {
            let loaded = match inner.files.contains_key(&shard) {
                true => match std::fs::read(self.shard_path(shard))
                    .ok()
                    .and_then(|bytes| decode_shard(&bytes))
                {
                    Some(loaded) => loaded,
                    None => {
                        tracing::warn!(shard, "damaged list Waveform cache shard dropped");
                        let _ = std::fs::remove_file(self.shard_path(shard));
                        if let Some(file) = inner.files.remove(&shard) {
                            inner.total -= file.bytes;
                        }
                        Shard::new()
                    }
                },
                false => Shard::new(),
            };
            inner.memory.insert(shard, loaded);
            inner.memory_order.push_back(shard);
            while inner.memory_order.len() > MEMORY_SHARDS {
                if let Some(oldest) = inner.memory_order.pop_front() {
                    inner.memory.remove(&oldest);
                }
            }
        }
        if let Some(file) = inner.files.get_mut(&shard) {
            file.used = now_nanos();
        }
        inner.memory.get_mut(&shard).expect("loaded shard")
    }

    /// Stored copies of `ids` (absent ones are left out).
    pub fn get_many(&self, ids: &[i64]) -> HashMap<i64, StoredListWaveform> {
        let mut inner = self.lock();
        let mut found = HashMap::new();
        for id in ids {
            let shard = id.div_euclid(LIST_WAVEFORM_SHARD_IDS);
            if let Some(entry) = self.load(&mut inner, shard).get(id) {
                found.insert(*id, entry.clone());
            }
        }
        found
    }

    pub fn get(&self, id: i64) -> Option<StoredListWaveform> {
        self.get_many(&[id]).remove(&id)
    }

    /// Store new copies (one atomic rewrite per touched shard), then evict old shards
    /// beyond the limit.
    pub fn put_many(&self, entries: Vec<(i64, StoredListWaveform)>) -> std::io::Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        let mut inner = self.lock();
        let mut touched = Vec::new();
        for (id, entry) in entries {
            let shard = id.div_euclid(LIST_WAVEFORM_SHARD_IDS);
            if self.load(&mut inner, shard).get(&id) != Some(&entry) {
                self.load(&mut inner, shard).insert(id, entry);
                if !touched.contains(&shard) {
                    touched.push(shard);
                }
            }
        }
        std::fs::create_dir_all(&self.directory)?;
        let mut result = Ok(());
        for shard in &touched {
            let bytes = encode_shard(self.load(&mut inner, *shard));
            match self.write_atomic(*shard, &bytes) {
                Ok(()) => {
                    let size = bytes.len() as u64;
                    let previous = inner.files.insert(
                        *shard,
                        ShardFile {
                            bytes: size,
                            used: now_nanos(),
                        },
                    );
                    inner.total = inner.total - previous.map_or(0, |file| file.bytes) + size;
                }
                Err(error) => result = Err(error),
            }
        }
        self.evict(&mut inner, &touched);
        result
    }

    fn write_atomic(&self, shard: i64, bytes: &[u8]) -> std::io::Result<()> {
        let temporary = self.directory.join(format!(
            "{shard}.tmp{}-{}",
            std::process::id(),
            TEMPORARY.fetch_add(1, Ordering::Relaxed)
        ));
        let written = std::fs::write(&temporary, bytes)
            .and_then(|()| std::fs::rename(&temporary, self.shard_path(shard)));
        if written.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        written
    }

    fn evict(&self, inner: &mut StoreInner, keep: &[i64]) {
        while inner.total > self.limit {
            let Some(oldest) = inner
                .files
                .iter()
                .filter(|(shard, _)| !keep.contains(shard))
                .min_by_key(|(_, file)| file.used)
                .map(|(shard, _)| *shard)
            else {
                break;
            };
            let _ = std::fs::remove_file(self.shard_path(oldest));
            if let Some(file) = inner.files.remove(&oldest) {
                inner.total -= file.bytes;
            }
            inner.memory.remove(&oldest);
            inner.memory_order.retain(|shard| *shard != oldest);
            tracing::info!(shard = oldest, "list Waveform cache shard evicted");
        }
    }

    /// Bytes on disk.
    pub fn bytes(&self) -> u64 {
        self.lock().total
    }
}

enum Probe {
    Unknown,
    Known(Option<ListWaveformsCapability>),
    Failed(Instant),
}

/// List Waveform lookups of one open server snapshot (see the module documentation).
pub struct RemoteListWaveforms {
    client: RemoteClient,
    library_id: String,
    store: ListWaveformStore,
    capability: Mutex<Probe>,
    in_flight: Mutex<()>,
}

impl RemoteListWaveforms {
    pub fn new(
        client: RemoteClient,
        library_id: impl Into<String>,
        store: ListWaveformStore,
    ) -> Self {
        Self {
            client,
            library_id: library_id.into(),
            store,
            capability: Mutex::new(Probe::Unknown),
            in_flight: Mutex::new(()),
        }
    }

    /// Use already fetched capabilities instead of probing `GET /capabilities`.
    pub fn with_capabilities(self, capabilities: &Capabilities) -> Self {
        *self
            .capability
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Probe::Known(usable(capabilities));
        self
    }

    pub fn library_id(&self) -> &str {
        &self.library_id
    }

    pub fn store(&self) -> &ListWaveformStore {
        &self.store
    }

    /// Blocking on first use (`GET /capabilities`, run it off the UI thread):
    /// [`ListWaveformSource::Server`] when the server offers usable list Waveforms,
    /// otherwise [`ListWaveformSource::Unavailable`]. A failed probe is repeated after 30 s.
    pub fn source(&self) -> ListWaveformSource {
        match self.capability() {
            Some(_) => ListWaveformSource::Server,
            None => ListWaveformSource::Unavailable,
        }
    }

    fn capability(&self) -> Option<ListWaveformsCapability> {
        {
            let probe = self
                .capability
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            match &*probe {
                Probe::Known(capability) => return capability.clone(),
                Probe::Failed(at) if at.elapsed() < CAPABILITY_RETRY => return None,
                _ => {}
            }
        }
        let probed = self.client.capabilities();
        let mut probe = self
            .capability
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match probed {
            Ok(capabilities) => {
                let capability = usable(&capabilities);
                *probe = Probe::Known(capability.clone());
                capability
            }
            Err(error) => {
                tracing::debug!(%error, "list Waveform capability probe failed");
                *probe = Probe::Failed(Instant::now());
                None
            }
        }
    }

    /// One `POST /waveforms` for up to the server's `max_ids` of `ids` (deduplicated,
    /// in order), resolved against the disk cache. Never more than one request of this
    /// client is in flight.
    pub fn lookup(&self, ids: &[i64], cancel: &AtomicBool) -> Result<ListWaveformBatch> {
        let Some(capability) = self.capability() else {
            return Err(RemoteError::Unsupported(
                "The server offers no list Waveforms".into(),
            ));
        };
        let mut seen = HashSet::new();
        let ids: Vec<i64> = ids
            .iter()
            .copied()
            .filter(|id| *id > 0 && seen.insert(*id))
            .take(capability.batch_limit())
            .collect();
        if ids.is_empty() {
            return Ok(ListWaveformBatch::default());
        }
        let stored = self.store.get_many(&ids);
        let known: BTreeMap<i64, String> = stored
            .iter()
            .map(|(id, entry)| (*id, entry.etag.clone()))
            .collect();
        let reply = {
            let _one = self
                .in_flight
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if cancel.load(Ordering::Acquire) {
                return Err(RemoteError::Cancelled);
            }
            self.client.list_waveform_batch(&ids, &known, cancel)
        };
        let reply = match reply {
            Ok(reply) => reply,
            Err(RemoteError::Busy { retry_after, .. }) => {
                return Ok(ListWaveformBatch {
                    deferred: Some(
                        retry_after
                            .map_or(BUSY_DEFAULT, |seconds| Duration::from_secs(seconds.max(1))),
                    ),
                    ..ListWaveformBatch::default()
                });
            }
            Err(error) => return Err(error),
        };
        if reply.library_id != self.library_id {
            return Err(RemoteError::LibraryChanged(format!(
                "POST /waveforms answered for library {} instead of the open snapshot's; reply discarded",
                reply.library_id
            )));
        }
        if reply.buckets != 0 && reply.buckets != LIST_WAVEFORM_COLUMNS {
            return Err(RemoteError::Protocol(format!(
                "POST /waveforms answered with {} buckets",
                reply.buckets
            )));
        }
        Ok(self.resolve(&ids, reply, &stored))
    }

    fn resolve(
        &self,
        ids: &[i64],
        reply: BatchReply,
        stored: &HashMap<i64, StoredListWaveform>,
    ) -> ListWaveformBatch {
        let previous = |id: i64| stored.get(&id).map(|entry| entry.waveform.clone());
        let mut answered = HashSet::new();
        let mut items = Vec::new();
        let mut store = Vec::new();
        for item in reply.items {
            if !ids.contains(&item.id) || !answered.insert(item.id) {
                continue;
            }
            let id = item.id;
            let state = match item.status {
                BatchStatus::Ok => {
                    let parsed = match (&item.etag, &item.peaks, item.n) {
                        (Some(etag), Some(peaks), Some(n)) => {
                            ListWaveform::from_base64_peaks(peaks, n)
                                .map(|waveform| (etag.clone(), waveform))
                                .map_err(|error| error.to_string())
                        }
                        _ => Err("incomplete ok item".into()),
                    };
                    match parsed {
                        Ok((etag, waveform)) => {
                            store.push((
                                id,
                                StoredListWaveform {
                                    etag,
                                    waveform: waveform.clone(),
                                },
                            ));
                            ListWaveformState::Ready { waveform }
                        }
                        Err(message) => {
                            tracing::warn!(id, %message, "malformed list Waveform item");
                            ListWaveformState::Failed { message }
                        }
                    }
                }
                BatchStatus::NotModified => match stored.get(&id) {
                    Some(entry) if item.etag.as_ref().is_none_or(|etag| *etag == entry.etag) => {
                        ListWaveformState::Ready {
                            waveform: entry.waveform.clone(),
                        }
                    }
                    // Not what was sent in `known`: ask again without it later.
                    _ => ListWaveformState::Pending { previous: None },
                },
                BatchStatus::Pending | BatchStatus::Unknown => ListWaveformState::Pending {
                    previous: previous(id),
                },
                BatchStatus::Missing => ListWaveformState::Missing,
                BatchStatus::RootOffline => ListWaveformState::RootOffline {
                    root: item.root.unwrap_or_default(),
                    previous: previous(id),
                },
                BatchStatus::Error => ListWaveformState::Failed {
                    message: item
                        .message
                        .or(item.error_code)
                        .unwrap_or_else(|| "error".into()),
                },
            };
            items.push(ListWaveformItem { id, state });
        }
        if let Err(error) = self.store.put_many(store) {
            tracing::warn!(%error, "list Waveform cache write failed");
        }
        ListWaveformBatch {
            items,
            retry_after: reply
                .retry_after
                .map(|seconds| Duration::from_secs(seconds.clamp(1, 30))),
            deferred: None,
        }
    }
}

fn usable(capabilities: &Capabilities) -> Option<ListWaveformsCapability> {
    capabilities
        .list_waveforms
        .clone()
        .filter(|capability| capabilities.supported && capability.usable())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_server::{TestRequest, TestResponse, TestServer};
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    const LIBRARY: &str = "lib-lw";
    const CAPABILITIES: &str = r#"{"manage_mode":"all","can_manage":true,"root_management":true,
        "list_waveforms":{"version":1,"buckets":[128],"max_ids":200,"encoding":"i16le-minmax-base64"},
        "pregenerate_list_waveforms":false}"#;

    fn waveform(values: &[i16]) -> ListWaveform {
        ListWaveform::new(1, values.to_vec()).unwrap()
    }

    fn base64(bytes: &[u8]) -> String {
        const TABLE: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut text = String::new();
        for chunk in bytes.chunks(3) {
            let word = (u32::from(chunk[0]) << 16)
                | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
                | u32::from(*chunk.get(2).unwrap_or(&0));
            for index in 0..4 {
                text.push(if index <= chunk.len() {
                    TABLE[(word >> (18 - 6 * index)) as usize & 63] as char
                } else {
                    '='
                });
            }
        }
        text
    }

    fn body_json(request: &TestRequest) -> serde_json::Value {
        serde_json::from_slice(&request.body).unwrap()
    }

    fn service(server: &TestServer, directory: &Path) -> RemoteListWaveforms {
        RemoteListWaveforms::new(
            RemoteClient::new(&server.url, "").unwrap(),
            LIBRARY,
            ListWaveformStore::new(directory.join("list"), DEFAULT_LIST_WAVEFORM_CACHE_BYTES),
        )
    }

    fn stored(etag: &str, values: &[i16]) -> StoredListWaveform {
        StoredListWaveform {
            etag: etag.into(),
            waveform: waveform(values),
        }
    }

    #[test]
    fn every_item_status_maps_to_a_state_and_ok_items_are_cached_by_etag() {
        let directory = tempfile::tempdir().unwrap();
        let posts = Arc::new(Mutex::new(Vec::new()));
        let server = {
            let posts = posts.clone();
            TestServer::start(move |request| match request.path.as_str() {
                "/api/v1/capabilities" => TestResponse::json(200, CAPABILITIES),
                "/api/v1/waveforms" => {
                    assert_eq!(request.method, "POST");
                    assert_eq!(request.header("accept-encoding"), Some("zstd"));
                    assert_eq!(request.header("content-type"), Some("application/json"));
                    let body = body_json(request);
                    posts.lock().unwrap().push(body.clone());
                    let ok = base64(&waveform(&[-100, 200, 0, 0]).to_interleaved_le());
                    let reply = serde_json::json!({
                        "library_id": LIBRARY, "buckets": 128, "retry_after": 4,
                        "items": [
                            {"id": 1, "status": "ok", "etag": "W/\"e1-l1\"", "n": 2, "peaks": ok},
                            {"id": 2, "status": "not_modified", "etag": "W/\"e2-l1\""},
                            {"id": 3, "status": "pending"},
                            {"id": 4, "status": "root_offline", "root": "SFX_Main"},
                            {"id": 5, "status": "missing"},
                            {"id": 6, "status": "error", "error_code": "AUDIO", "message": "Cannot decode"},
                            {"id": 7, "status": "ok", "etag": "W/\"e7-l1\"", "n": 0, "peaks": ""},
                            {"id": 99, "status": "ok", "etag": "x", "n": 0, "peaks": ""}
                        ]
                    });
                    let json = serde_json::to_vec(&reply).unwrap();
                    TestResponse::bytes(200, zstd::encode_all(json.as_slice(), 3).unwrap())
                        .with_header("Content-Encoding", "zstd")
                }
                other => panic!("unexpected {other}"),
            })
        };
        let service = service(&server, directory.path());
        service
            .store()
            .put_many(vec![
                (2, stored("W/\"e2-l1\"", &[-1, 1])),
                (3, stored("W/\"old3\"", &[-3, 3])),
                (4, stored("W/\"old4\"", &[-4, 4])),
            ])
            .unwrap();
        assert_eq!(service.source(), ListWaveformSource::Server);
        let cancel = AtomicBool::new(false);
        let batch = service.lookup(&[1, 2, 2, 3, 4, 5, 6, 7], &cancel).unwrap();
        let body = posts.lock().unwrap()[0].clone();
        assert_eq!(body["ids"], serde_json::json!([1, 2, 3, 4, 5, 6, 7]));
        assert_eq!(body["buckets"], 128);
        assert_eq!(
            body["known"],
            serde_json::json!({"2": "W/\"e2-l1\"", "3": "W/\"old3\"", "4": "W/\"old4\""})
        );
        assert_eq!(batch.retry_after, Some(Duration::from_secs(4)));
        assert_eq!(batch.deferred, None);
        let states: Vec<_> = batch
            .items
            .iter()
            .map(|item| (item.id, &item.state))
            .collect();
        assert_eq!(
            states,
            vec![
                (
                    1,
                    &ListWaveformState::Ready {
                        waveform: waveform(&[-100, 200, 0, 0])
                    }
                ),
                (
                    2,
                    &ListWaveformState::Ready {
                        waveform: waveform(&[-1, 1])
                    }
                ),
                (
                    3,
                    &ListWaveformState::Pending {
                        previous: Some(waveform(&[-3, 3]))
                    }
                ),
                (
                    4,
                    &ListWaveformState::RootOffline {
                        root: "SFX_Main".into(),
                        previous: Some(waveform(&[-4, 4]))
                    }
                ),
                (5, &ListWaveformState::Missing),
                (
                    6,
                    &ListWaveformState::Failed {
                        message: "Cannot decode".into()
                    }
                ),
                (
                    7,
                    &ListWaveformState::Ready {
                        waveform: waveform(&[])
                    }
                ),
            ],
            "unrequested id 99 is ignored"
        );
        // Stored by entity tag: a fresh store over the same directory sees them.
        let reopened = ListWaveformStore::new(
            directory.path().join("list"),
            DEFAULT_LIST_WAVEFORM_CACHE_BYTES,
        );
        assert_eq!(
            reopened.get(1),
            Some(stored("W/\"e1-l1\"", &[-100, 200, 0, 0]))
        );
        assert_eq!(reopened.get(7), Some(stored("W/\"e7-l1\"", &[])));
        assert_eq!(reopened.get(99), None);
        // The next lookup sends the new tag in `known`.
        service.lookup(&[1], &cancel).unwrap();
        assert_eq!(
            posts.lock().unwrap()[1]["known"],
            serde_json::json!({"1": "W/\"e1-l1\""})
        );
    }

    #[test]
    fn busy_defers_by_retry_after_and_another_library_is_discarded() {
        let directory = tempfile::tempdir().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let server = {
            let calls = calls.clone();
            TestServer::start(move |request| match request.path.as_str() {
                "/api/v1/capabilities" => TestResponse::json(200, CAPABILITIES),
                "/api/v1/waveforms" => match calls.fetch_add(1, Ordering::SeqCst) {
                    0 => TestResponse::json(
                        503,
                        r#"{"error_code":"BUSY","message":"Too many list Waveform requests"}"#,
                    )
                    .with_header("Retry-After", "3"),
                    1 => TestResponse::json(503, r#"{"error_code":"BUSY","message":"busy"}"#),
                    _ => TestResponse::json(
                        200,
                        r#"{"library_id":"replacement","buckets":128,"retry_after":null,
                            "items":[{"id":1,"status":"ok","etag":"e","n":0,"peaks":""}]}"#,
                    ),
                },
                other => panic!("unexpected {other}"),
            })
        };
        let service = service(&server, directory.path());
        let cancel = AtomicBool::new(false);
        let busy = service.lookup(&[1, 2], &cancel).unwrap();
        assert!(busy.items.is_empty());
        assert_eq!(busy.deferred, Some(Duration::from_secs(3)));
        let busy = service.lookup(&[1, 2], &cancel).unwrap();
        assert_eq!(busy.deferred, Some(BUSY_DEFAULT));
        assert!(matches!(
            service.lookup(&[1], &cancel),
            Err(RemoteError::LibraryChanged(_))
        ));
        assert_eq!(
            service.store().get(1),
            None,
            "a foreign reply is not stored"
        );
        assert_eq!(service.store().bytes(), 0);
    }

    #[test]
    fn without_a_usable_capability_nothing_is_posted() {
        let directory = tempfile::tempdir().unwrap();
        for capabilities in [
            None,
            Some(r#"{"manage_mode":"all"}"#),
            Some(
                r#"{"list_waveforms":{"version":2,"buckets":[128],"max_ids":200,"encoding":"i16le-minmax-base64"}}"#,
            ),
            Some(
                r#"{"list_waveforms":{"version":1,"buckets":[256],"max_ids":200,"encoding":"i16le-minmax-base64"}}"#,
            ),
            Some(
                r#"{"list_waveforms":{"version":1,"buckets":[128],"max_ids":200,"encoding":"f32"}}"#,
            ),
        ] {
            let posts = Arc::new(AtomicUsize::new(0));
            let server = {
                let posts = posts.clone();
                TestServer::start(move |request| match (request.path.as_str(), capabilities) {
                    ("/api/v1/capabilities", Some(body)) => TestResponse::json(200, body),
                    ("/api/v1/capabilities", None) => TestResponse::json(
                        404,
                        r#"{"error_code":"NOT_FOUND","message":"No such endpoint"}"#,
                    ),
                    _ => {
                        posts.fetch_add(1, Ordering::SeqCst);
                        TestResponse::json(500, "{}")
                    }
                })
            };
            let service = service(&server, directory.path());
            assert_eq!(
                service.source(),
                ListWaveformSource::Unavailable,
                "{capabilities:?}"
            );
            assert!(matches!(
                service.lookup(&[1], &AtomicBool::new(false)),
                Err(RemoteError::Unsupported(_))
            ));
            assert_eq!(
                posts.load(Ordering::SeqCst),
                0,
                "no per-item or batch fallback"
            );
        }
        // Known capabilities skip the probe.
        let server = TestServer::start(|request| panic!("unexpected {}", request.path));
        let caps: Capabilities = serde_json::from_str(CAPABILITIES).unwrap();
        assert!(caps.list_waveforms_usable());
        let known = service(&server, directory.path()).with_capabilities(&caps);
        assert_eq!(known.source(), ListWaveformSource::Server);
        let none = service(&server, directory.path()).with_capabilities(&Capabilities::unknown());
        assert_eq!(none.source(), ListWaveformSource::Unavailable);
    }

    #[test]
    fn the_disk_cache_evicts_least_recently_used_shards_and_drops_damaged_ones() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("list");
        let entry = |id: i64| (id, stored(&format!("W/\"{id}\""), &[-7, 7].repeat(128)));
        // One 128-column entry is about 530 bytes (shard header and checksum 20 more):
        // shards 0 (two entries), 1 and 2 fit in 2300 bytes, a fourth shard does not.
        let store = ListWaveformStore::new(root.clone(), 2300);
        store.put_many(vec![entry(1), entry(2)]).unwrap();
        assert_eq!(store.get(1).unwrap().etag, "W/\"1\"");
        let pause = || std::thread::sleep(Duration::from_millis(5));
        pause();
        store.put_many(vec![entry(1024)]).unwrap();
        pause();
        store.put_many(vec![entry(2048)]).unwrap();
        // Reading shard 0 makes shard 1 the least recently used one.
        pause();
        assert!(store.get(2).is_some());
        pause();
        store.put_many(vec![entry(3072)]).unwrap();
        assert!(store.bytes() <= 2300, "{}", store.bytes());
        assert!(
            store.get(1024).is_none(),
            "the least recently used shard is evicted"
        );
        assert!(store.get(1).is_some() && store.get(3072).is_some());
        let mut names: Vec<String> = std::fs::read_dir(&root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert!(names.iter().all(|name| name.ends_with(".lws")), "{names:?}");
        assert!(!names.contains(&"1.lws".to_owned()));
        // A damaged shard and a leftover temporary file are dropped on the next start.
        std::fs::write(root.join("3.lws"), b"ALXLWS1\ngarbage").unwrap();
        std::fs::write(root.join("0.tmp1-1"), b"partial").unwrap();
        let reopened = ListWaveformStore::new(root.clone(), 2300);
        assert!(reopened.get(3072).is_none());
        assert!(reopened.get(1).is_some());
        assert!(!root.join("3.lws").exists() && !root.join("0.tmp1-1").exists());
        // Shards round trip exactly.
        let mut shard = Shard::new();
        shard.insert(5, stored("tag", &[1, -1, 32767, -32767]));
        shard.insert(6, stored("", &[]));
        let bytes = encode_shard(&shard);
        assert_eq!(decode_shard(&bytes), Some(shard));
        let mut flipped = bytes.clone();
        flipped[12] ^= 1;
        assert_eq!(decode_shard(&flipped), None);
        assert_eq!(
            ListWaveformStore::library_directory(Path::new("c"), LIBRARY),
            Path::new("c")
                .join("waveforms")
                .join("server")
                .join(format!("{:016x}", crate::util::fnv64(LIBRARY)))
                .join("list")
        );
    }

    #[test]
    fn settings_and_status_use_the_settings_endpoints() {
        let patches = Arc::new(Mutex::new(Vec::new()));
        let server = {
            let patches = patches.clone();
            TestServer::start(move |request| {
                match (request.method.as_str(), request.path.as_str()) {
                    ("GET", "/api/v1/library") => TestResponse::json(
                        200,
                        r#"{"library_id":"lib-lw","schema_version":5,"revision":1}"#,
                    ),
                    ("GET", "/api/v1/settings") => TestResponse::json(
                        200,
                        r#"{"pregenerate_list_waveforms":false,"pregenerate_list_waveforms_source":"default","can_change":true}"#,
                    ),
                    ("PATCH", "/api/v1/settings") => {
                        patches.lock().unwrap().push(body_json(request));
                        TestResponse::json(
                            200,
                            r#"{"pregenerate_list_waveforms":true,"pregenerate_list_waveforms_source":"admin","can_change":true}"#,
                        )
                    }
                    ("GET", "/api/v1/waveforms/status") => TestResponse::json(
                        200,
                        r#"{"enabled":true,"state":"yielding","yield_reason":"audio","total":10,"cached":4,
                        "remaining":6,"queued_on_demand":0,"failed":1,"skipped_offline":0,"rate":10,
                        "cache_bytes":2048,"cache_limit_bytes":671088640,"over_budget":false,
                        "pass_started_at":1700000000,"pass_finished_at":null}"#,
                    ),
                    (method, path) => panic!("unexpected {method} {path}"),
                }
            })
        };
        let client = RemoteClient::new(&server.url, "").unwrap();
        let settings = client.server_settings().unwrap();
        assert!(!settings.pregenerate_list_waveforms && settings.can_change);
        let changed = client.set_pregenerate_list_waveforms(true).unwrap();
        assert!(changed.pregenerate_list_waveforms);
        assert_eq!(changed.pregenerate_list_waveforms_source, "admin");
        assert_eq!(
            patches.lock().unwrap()[0],
            serde_json::json!({"pregenerate_list_waveforms": true})
        );
        let status = client.list_waveform_status().unwrap();
        assert_eq!(
            (status.state.as_str(), status.cached, status.remaining),
            ("yielding", 4, Some(6))
        );
        assert_eq!(status.yield_reason.as_deref(), Some("audio"));
    }
}
