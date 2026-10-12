// crates/audiolib-remote/src/writes.rs
//! Server-mode metadata writes and plays (API §6.3) and the revision rule that keeps the
//! local snapshot current without a full download.
//!
//! Every write reply carries `revision_before` and `revision`. When the cached snapshot
//! is exactly at `revision_before`, nothing else changed on the server in between, so the
//! returned records are written into the snapshot and the cache moves to `revision`.
//! Otherwise (another client wrote meanwhile, no snapshot yet, apply failed) the caller
//! runs a full snapshot sync. A write whose outcome is unknown (timeout / `408`, dropped
//! connection, unreadable reply) also needs a sync: the server may have committed it.
use crate::client::{BatchWrite, FileWrite, RemoteClient};
use crate::error::{RemoteError, Result};
use crate::manage::{RemoveReport, manage_error_text};
use crate::sync::RemoteCache;
use audiolib_core::{
    AudioFile, BackupReport, BatchMetadataReport, BatchMetadataRequest, DataProvider, Error,
    Folder, IntegrityReport, LibraryExportFormat, LibraryInfo, LibraryStats, MaintenanceAction,
    MaintenanceReport, MetadataPatch, PlayRecord, RestoreReport, ScanSession, SearchPage,
    SearchRequest, TransferReport,
};
use audiolib_db::SqliteProvider;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Serializes read-modify-write of `cache.json` + snapshot between concurrent writes
/// (an edit and a play finishing at the same time).
static APPLY_LOCK: Mutex<()> = Mutex::new(());

/// What happened to the local snapshot after a successful server write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CacheUpdate {
    /// The returned records were written into the snapshot; the cache is at `revision`.
    Applied { revision: u64 },
    /// The snapshot could not be advanced safely: run a full snapshot sync.
    Resync { reason: String },
}

/// A successful server write and its effect on the local cache.
#[derive(Clone, Debug)]
pub struct WriteOutcome<T> {
    pub value: T,
    pub revision_before: u64,
    pub revision: u64,
    pub cache: CacheUpdate,
}

/// Apply the revision rule for one write reply: if the cached snapshot of `library_id`
/// is at `revision_before`, write `files` into it and record `revision`; otherwise ask
/// for a full sync. Never downloads anything itself.
pub fn apply_server_write(
    cache: &RemoteCache,
    library_id: &str,
    files: &[AudioFile],
    revision_before: u64,
    revision: u64,
) -> CacheUpdate {
    let _guard = APPLY_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(mut state) = cache.state(library_id) else {
        return CacheUpdate::Resync {
            reason: "no cached snapshot for this library".into(),
        };
    };
    if state.revision != revision_before || revision <= revision_before {
        return CacheUpdate::Resync {
            reason: format!(
                "cached revision {} does not match revision_before {revision_before} (server now {revision})",
                state.revision
            ),
        };
    }
    if let Err(error) = SqliteProvider::apply_snapshot_records(&cache.database_path(&state), files)
    {
        return CacheUpdate::Resync {
            reason: format!("could not update the cached snapshot: {error}"),
        };
    }
    state.revision = revision;
    match cache.save_state(&state) {
        Ok(()) => CacheUpdate::Applied { revision },
        Err(error) => CacheUpdate::Resync {
            reason: format!("could not record the new revision: {error}"),
        },
    }
}

/// Metadata writes and plays for one server library, applied to its local cache.
#[derive(Clone, Debug)]
pub struct RemoteWriter {
    client: RemoteClient,
    cache: RemoteCache,
    library_id: String,
}

impl RemoteWriter {
    pub fn new(client: RemoteClient, cache: RemoteCache, library_id: impl Into<String>) -> Self {
        Self {
            client,
            cache,
            library_id: library_id.into(),
        }
    }

    pub fn library_id(&self) -> &str {
        &self.library_id
    }

    /// The API client (capabilities, root checks and server scans, which are not
    /// `DataProvider` operations).
    pub fn client(&self) -> &RemoteClient {
        &self.client
    }

    /// The snapshot after a server removal (§6.8). The reply carries no records to apply,
    /// so a removal that moved the revision always means a full sync; removed ids may come
    /// back with new ids after a rescan, so callers also drop ids they cached (selection,
    /// waveform/preview keys). An unchanged revision removed nothing: the snapshot stays
    /// valid when it is at that revision.
    fn removed(&self, report: RemoveReport) -> WriteOutcome<RemoveReport> {
        let (before, after) = (report.revision_before, report.revision);
        let cached = self
            .cache
            .state(&self.library_id)
            .map(|state| state.revision);
        let cache = if !report.changed() && cached == Some(after) {
            CacheUpdate::Applied { revision: after }
        } else {
            let reason = format!(
                "{} records removed on the server (revision {before} -> {after})",
                report.removed
            );
            tracing::info!(%reason, "server removal needs a full snapshot sync");
            CacheUpdate::Resync { reason }
        };
        WriteOutcome {
            value: report,
            revision_before: before,
            revision: after,
            cache,
        }
    }

    /// `POST /audio/remove`: database records only (files untouched), then a sync.
    pub fn remove_audio(&self, ids: &[i64]) -> Result<WriteOutcome<RemoveReport>> {
        Ok(self.removed(self.client.remove_audio(ids)?))
    }

    /// `DELETE /folders/{id}`: database records only (files untouched), then a sync.
    pub fn remove_folder(&self, id: i64) -> Result<WriteOutcome<RemoveReport>> {
        Ok(self.removed(self.client.remove_folder(id)?))
    }

    fn finish<T>(&self, value: T, files: &[AudioFile], before: u64, after: u64) -> WriteOutcome<T> {
        let cache = apply_server_write(&self.cache, &self.library_id, files, before, after);
        if let CacheUpdate::Resync { reason } = &cache {
            tracing::info!(%reason, before, after, "server write needs a full snapshot sync");
        }
        WriteOutcome {
            value,
            revision_before: before,
            revision: after,
            cache,
        }
    }

    /// `PATCH /audio/{id}/metadata`, then the revision rule.
    pub fn update_metadata(
        &self,
        id: i64,
        patch: &MetadataPatch,
    ) -> Result<WriteOutcome<AudioFile>> {
        let FileWrite {
            file,
            revision_before,
            revision,
        } = self.client.patch_metadata(id, patch)?;
        Ok(self.finish(
            file.clone(),
            std::slice::from_ref(&file),
            revision_before,
            revision,
        ))
    }

    /// `POST /audio/metadata/batch`, then the revision rule. The value carries the
    /// server's report and the written records (request order, de-duplicated).
    pub fn update_metadata_batch(
        &self,
        request: &BatchMetadataRequest,
    ) -> Result<WriteOutcome<(BatchMetadataReport, Vec<AudioFile>)>> {
        let BatchWrite {
            report,
            files,
            revision_before,
            revision,
        } = self.client.batch_metadata(request)?;
        let outcome = self.finish((), &files, revision_before, revision);
        Ok(WriteOutcome {
            value: (report, files),
            revision_before: outcome.revision_before,
            revision: outcome.revision,
            cache: outcome.cache,
        })
    }

    /// `POST /audio/{id}/plays`, then the revision rule.
    pub fn record_play(&self, id: i64) -> Result<WriteOutcome<AudioFile>> {
        let FileWrite {
            file,
            revision_before,
            revision,
        } = self.client.record_play(id)?;
        Ok(self.finish(
            file.clone(),
            std::slice::from_ref(&file),
            revision_before,
            revision,
        ))
    }
}

/// Cache effect of a write, reported to the GUI through [`RemoteProvider`]'s listener.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WriteEvent {
    /// The snapshot now reflects the write and is at `revision`.
    Applied { revision: u64 },
    /// Run a full snapshot sync (revision mismatch, apply failure or uncertain outcome).
    Resync { reason: String },
}

/// Listener for [`WriteEvent`]s (called on the writing thread).
pub type WriteListener = Arc<dyn Fn(WriteEvent) + Send + Sync>;

/// `DataProvider` for server mode: reads come from the read-only snapshot, metadata
/// writes and plays go to the server ([`RemoteWriter`]) and are applied to the snapshot
/// by the revision rule. Record and folder removal go to the server's management
/// endpoints (database records only; servers without them answer "unsupported") and are
/// followed by a full sync when they changed the revision. Restore, maintenance, relink and import are not offered by
/// the server API and are refused. The desktop removes server records from the
/// 服务器音效库 page and its removal menus, only for the active server with `can_manage`.
pub struct RemoteProvider {
    snapshot: Arc<SqliteProvider>,
    writer: RemoteWriter,
    chinese: bool,
    listener: Option<WriteListener>,
}

impl RemoteProvider {
    pub fn new(
        snapshot: Arc<SqliteProvider>,
        writer: RemoteWriter,
        chinese: bool,
        listener: Option<WriteListener>,
    ) -> Self {
        Self {
            snapshot,
            writer,
            chinese,
            listener,
        }
    }

    pub fn writer(&self) -> &RemoteWriter {
        &self.writer
    }

    fn emit(&self, event: WriteEvent) {
        if let Some(listener) = &self.listener {
            listener(event);
        }
    }

    fn settle<T>(&self, result: Result<WriteOutcome<T>>) -> audiolib_core::Result<T> {
        match result {
            Ok(outcome) => {
                self.emit(match outcome.cache {
                    CacheUpdate::Applied { revision } => WriteEvent::Applied { revision },
                    CacheUpdate::Resync { reason } => WriteEvent::Resync { reason },
                });
                Ok(outcome.value)
            }
            Err(error) => {
                if error.write_may_have_committed() {
                    self.emit(WriteEvent::Resync {
                        reason: format!("write outcome uncertain: {error}"),
                    });
                }
                Err(core_error(error, self.chinese))
            }
        }
    }

    /// Like `settle` for management writes, with the management error wording
    /// (`403 MANAGE_FORBIDDEN`, `409 BUSY`, `503 ROOT_OFFLINE`, older server).
    fn settle_manage<T>(&self, result: Result<WriteOutcome<T>>) -> audiolib_core::Result<T> {
        match result {
            Err(error) if !error.write_may_have_committed() => {
                let text = manage_error_text(&error, self.chinese);
                Err(match error {
                    RemoteError::NotFound(_) => Error::NotFound(text),
                    RemoteError::Busy { .. }
                    | RemoteError::Starting { .. }
                    | RemoteError::RootOffline { .. } => Error::Busy(text),
                    RemoteError::Cancelled => Error::Cancelled,
                    RemoteError::Validation(_)
                    | RemoteError::Forbidden(_)
                    | RemoteError::Unsupported(_) => Error::Validation(text),
                    _ => Error::Database(text),
                })
            }
            other => self.settle(other),
        }
    }

    fn refused(&self) -> Error {
        Error::Validation(if self.chinese {
            "服务器模式不支持此写操作（服务器 API 只提供标注编辑和播放计数）".into()
        } else {
            "Not available in server mode (the server API only offers annotation edits and play counts)".into()
        })
    }
}

/// A classified remote failure as a core error carrying the localized text.
pub fn core_error(error: RemoteError, chinese: bool) -> Error {
    let text = error.localized(chinese);
    match error {
        RemoteError::NotFound(_) => Error::NotFound(text),
        RemoteError::Busy { .. } | RemoteError::Starting { .. } => Error::Busy(text),
        RemoteError::Validation(_) | RemoteError::PayloadTooLarge(_) => Error::Validation(text),
        RemoteError::Cancelled => Error::Cancelled,
        _ => Error::Database(text),
    }
}

impl DataProvider for RemoteProvider {
    fn search(&self, request: &SearchRequest) -> audiolib_core::Result<SearchPage> {
        self.snapshot.search(request)
    }
    fn search_folders(&self, request: &SearchRequest) -> audiolib_core::Result<Vec<Folder>> {
        self.snapshot.search_folders(request)
    }
    fn get_audio(&self, id: i64) -> audiolib_core::Result<AudioFile> {
        self.snapshot.get_audio(id)
    }
    fn folders(&self, parent_id: Option<i64>) -> audiolib_core::Result<Vec<Folder>> {
        self.snapshot.folders(parent_id)
    }
    fn update_metadata(&self, id: i64, patch: &MetadataPatch) -> audiolib_core::Result<AudioFile> {
        self.settle(self.writer.update_metadata(id, patch))
    }
    fn record_play(&self, id: i64) -> audiolib_core::Result<()> {
        self.settle(self.writer.record_play(id)).map(drop)
    }
    fn stats(&self) -> audiolib_core::Result<LibraryStats> {
        self.snapshot.stats()
    }
    fn library_info(&self) -> audiolib_core::Result<LibraryInfo> {
        self.snapshot.library_info()
    }
    fn check_library(&self) -> audiolib_core::Result<IntegrityReport> {
        self.snapshot.check_library()
    }
    fn backup_library(&self, destination: &Path) -> audiolib_core::Result<BackupReport> {
        self.snapshot.backup_library(destination)
    }
    fn restore_library(&self, _source: &Path) -> audiolib_core::Result<RestoreReport> {
        Err(self.refused())
    }
    fn maintain_library(
        &self,
        _action: MaintenanceAction,
    ) -> audiolib_core::Result<MaintenanceReport> {
        Err(self.refused())
    }
    fn update_metadata_batch(
        &self,
        request: &BatchMetadataRequest,
    ) -> audiolib_core::Result<BatchMetadataReport> {
        self.settle(self.writer.update_metadata_batch(request))
            .map(|(report, _)| report)
    }
    fn find_audio_by_path(&self, path: &Path) -> audiolib_core::Result<Option<AudioFile>> {
        self.snapshot.find_audio_by_path(path)
    }
    fn relink_audio(&self, _id: i64, _path: &Path) -> audiolib_core::Result<AudioFile> {
        Err(self.refused())
    }
    fn remove_audio(&self, ids: &[i64]) -> audiolib_core::Result<u64> {
        self.settle_manage(self.writer.remove_audio(ids))
            .map(|report| report.removed)
    }
    fn remove_folder(&self, id: i64) -> audiolib_core::Result<u64> {
        self.settle_manage(self.writer.remove_folder(id))
            .map(|report| report.removed)
    }
    fn scan_history(&self, limit: usize, offset: usize) -> audiolib_core::Result<Vec<ScanSession>> {
        self.snapshot.scan_history(limit, offset)
    }
    fn play_history(&self, limit: usize, offset: usize) -> audiolib_core::Result<Vec<PlayRecord>> {
        // Applying a play reply updates audio_files and the cached revision, but the
        // reply does not carry the server's new history row. Read authoritative history
        // through the API so a caught-up revision never hides our own recent plays.
        self.writer
            .client()
            .play_history(limit, offset)
            .map_err(|error| core_error(error, self.chinese))
    }
    fn export_library(
        &self,
        destination: &Path,
        format: LibraryExportFormat,
    ) -> audiolib_core::Result<TransferReport> {
        self.snapshot.export_library(destination, format)
    }
    fn import_annotations(&self, _source: &Path) -> audiolib_core::Result<TransferReport> {
        Err(self.refused())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::CacheState;
    use crate::test_server::{TestResponse, TestServer};
    use audiolib_core::{SearchRequest, TagEditMode};
    use std::sync::Mutex as StdMutex;

    const LIBRARY: &str = "lib-w";

    /// A cached snapshot with records 3 (`door.wav`) and 7 (`雷声 近.wav`) at `revision`.
    fn cached_library(directory: &Path, revision: u64) -> RemoteCache {
        let cache = RemoteCache::new(directory);
        let library_dir = cache.library_dir(LIBRARY);
        std::fs::create_dir_all(&library_dir).unwrap();
        let database = library_dir.join("library-r1-1.db");
        drop(SqliteProvider::open(&database).unwrap());
        let conn = rusqlite::Connection::open(&database).unwrap();
        conn.execute_batch(
            "INSERT INTO audio_files(id,file_path,path_key,filename,filename_tokenized)
             VALUES(3,'/v/door.wav','/v/door.wav','door.wav','door'),
                   (7,'/v/thunder.wav','/v/thunder.wav','雷声 近.wav','雷声 近');",
        )
        .unwrap();
        conn.query_row("PRAGMA journal_mode=DELETE", [], |row| {
            row.get::<_, String>(0)
        })
        .unwrap();
        drop(conn);
        cache
            .save_state(&CacheState {
                library_id: LIBRARY.into(),
                revision,
                schema_version: audiolib_db::SCHEMA_VERSION,
                database_file: "library-r1-1.db".into(),
                ..CacheState::default()
            })
            .unwrap();
        cache
    }

    /// `legacy_rating` is still emitted: servers that have not dropped ratings send it and
    /// the client must tolerate (ignore) it.
    fn file_json(id: i64, tags: &str, legacy_rating: u8, color: &str, plays: u64) -> String {
        let color = if color.is_empty() {
            "null".to_string()
        } else {
            format!("\"{color}\"")
        };
        format!(
            r#"{{"id":{id},"file_path":"/v/{id}.wav","filename":"{id}.wav","folder_id":null,"file_size":52,"sample_rate":48000,"bit_depth":16,"channels":1,"duration":0.1,"tags":"{tags}","rating":{legacy_rating},"color":{color},"notes":"","favorite":false,"bext_description":"","play_count":{plays},"attributes":{{}},"missing":false}}"#
        )
    }

    struct Seen {
        method: String,
        path: String,
        body: String,
    }

    fn server(
        seen: Arc<StdMutex<Vec<Seen>>>,
        reply: impl Fn(&str, &str) -> TestResponse + Send + Sync + 'static,
    ) -> TestServer {
        TestServer::start(move |request| {
            seen.lock().unwrap().push(Seen {
                method: request.method.clone(),
                path: request.path.clone(),
                body: String::from_utf8_lossy(&request.body).into_owned(),
            });
            reply(&request.method, &request.path)
        })
    }

    fn snapshot(cache: &RemoteCache) -> SqliteProvider {
        let state = cache.state(LIBRARY).unwrap();
        SqliteProvider::open_read_only(&cache.database_path(&state)).unwrap()
    }

    #[test]
    fn provider_play_history_reads_server_after_play_advances_snapshot_revision() {
        let directory = tempfile::tempdir().unwrap();
        let cache = cached_library(directory.path(), 41);
        let reader = Arc::new(snapshot(&cache));
        let seen = Arc::new(StdMutex::new(Vec::new()));
        let server = server(seen.clone(), |method, path| match (method, path) {
            ("POST", "/api/v1/audio/3/plays") => TestResponse::json(
                200,
                &format!(
                    r#"{{"file":{},"revision_before":41,"revision":42}}"#,
                    file_json(3, "", 0, "", 1)
                ),
            ),
            ("GET", "/api/v1/history/plays?limit=10000&offset=17") => TestResponse::json(
                200,
                r#"[{"id":9,"audio_file_id":3,"played_at":"2026-10-11 09:00:00","filename":"door.wav","file_path":"/v/door.wav","missing":false}]"#,
            ),
            _ => TestResponse::json(404, "{}"),
        });
        let writer = RemoteWriter::new(
            RemoteClient::new(&server.url, "").unwrap(),
            cache.clone(),
            LIBRARY,
        );
        let provider = RemoteProvider::new(reader.clone(), writer, false, None);
        provider.record_play(3).unwrap();
        assert_eq!(cache.state(LIBRARY).unwrap().revision, 42);
        assert_eq!(reader.get_audio(3).unwrap().play_count, 1);
        assert!(reader.play_history(100, 0).unwrap().is_empty());

        let history = provider.play_history(usize::MAX, 17).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].audio_file_id, 3);
        assert_eq!(history[0].filename, "door.wav");
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[1].path, "/api/v1/history/plays?limit=10000&offset=17");
    }

    #[test]
    fn provider_play_history_preserves_remote_error_and_minimum_page_limit() {
        let directory = tempfile::tempdir().unwrap();
        let cache = cached_library(directory.path(), 1);
        let seen = Arc::new(StdMutex::new(Vec::new()));
        let server = server(seen.clone(), |_, _| {
            TestResponse::json(
                401,
                r#"{"error_code":"UNAUTHORIZED","message":"Missing or invalid token"}"#,
            )
        });
        let writer = RemoteWriter::new(
            RemoteClient::new(&server.url, "").unwrap(),
            cache.clone(),
            LIBRARY,
        );
        let provider = RemoteProvider::new(Arc::new(snapshot(&cache)), writer, true, None);
        let error = provider.play_history(0, 7).unwrap_err();
        assert!(matches!(error, Error::Database(ref text) if text.contains("服务器要求访问令牌")));
        assert_eq!(
            seen.lock().unwrap()[0].path,
            "/api/v1/history/plays?limit=1&offset=7"
        );
        assert_eq!(cache.state(LIBRARY).unwrap().revision, 1);
    }

    #[test]
    fn patch_applies_on_matching_revision_and_asks_for_sync_otherwise() {
        let directory = tempfile::tempdir().unwrap();
        let cache = cached_library(directory.path(), 41);
        let reader = snapshot(&cache);
        let seen = Arc::new(StdMutex::new(Vec::new()));
        let revision = Arc::new(StdMutex::new((41u64, 42u64)));
        let server = {
            let revision = revision.clone();
            server(seen.clone(), move |_, _| {
                let (before, after) = *revision.lock().unwrap();
                TestResponse::json(
                    200,
                    &format!(
                        r#"{{"file":{},"revision_before":{before},"revision":{after}}}"#,
                        file_json(3, "loud door", 4, "#FF8800", 0)
                    ),
                )
            })
        };
        let writer = RemoteWriter::new(
            RemoteClient::new(&server.url, "").unwrap(),
            cache.clone(),
            LIBRARY,
        );
        let patch = MetadataPatch {
            tags: Some("loud door".into()),
            color: Some("#FF8800".into()),
            ..MetadataPatch::default()
        };
        let outcome = writer.update_metadata(3, &patch).unwrap();
        assert_eq!(outcome.cache, CacheUpdate::Applied { revision: 42 });
        assert_eq!((outcome.revision_before, outcome.revision), (41, 42));
        assert_eq!(cache.state(LIBRARY).unwrap().revision, 42);
        {
            let seen = seen.lock().unwrap();
            assert_eq!(seen[0].method, "PATCH");
            assert_eq!(seen[0].path, "/api/v1/audio/3/metadata");
            let body: serde_json::Value = serde_json::from_str(&seen[0].body).unwrap();
            assert_eq!(
                body,
                serde_json::json!({"tags":"loud door","color":"#FF8800"}),
                "omitted fields must not be sent as null"
            );
        }
        // The provider that already had the snapshot open sees the applied values,
        // including the FTS columns.
        let record = reader.get_audio(3).unwrap();
        assert_eq!(record.tags.as_str(), "loud door");
        assert_eq!(record.color.as_deref(), Some("#FF8800"));
        let hits = reader
            .search(&SearchRequest {
                query: "loud".into(),
                limit: 10,
                ..SearchRequest::default()
            })
            .unwrap();
        assert_eq!(hits.results.iter().map(|f| f.id).collect::<Vec<_>>(), [3]);

        // Another client wrote in between: the reply's revision_before is not ours.
        *revision.lock().unwrap() = (50, 51);
        let outcome = writer
            .update_metadata(
                3,
                &MetadataPatch {
                    favorite: Some(true),
                    ..MetadataPatch::default()
                },
            )
            .unwrap();
        assert!(matches!(outcome.cache, CacheUpdate::Resync { .. }));
        assert_eq!(
            cache.state(LIBRARY).unwrap().revision,
            42,
            "left for the sync"
        );

        // Clearing the color is sent as "" and applied as no color.
        *revision.lock().unwrap() = (42, 43);
        let cleared = apply_server_write(
            &cache,
            LIBRARY,
            &[serde_json::from_str(&file_json(3, "loud door", 4, "", 0)).unwrap()],
            42,
            43,
        );
        assert_eq!(cleared, CacheUpdate::Applied { revision: 43 });
        assert_eq!(reader.get_audio(3).unwrap().color, None);
        let body = crate::client::patch_json(&MetadataPatch {
            color: Some(String::new()),
            ..MetadataPatch::default()
        });
        assert_eq!(body, serde_json::json!({"color":""}));

        // An id the snapshot does not have means it diverged: sync, revision unchanged.
        let unknown = apply_server_write(
            &cache,
            LIBRARY,
            &[serde_json::from_str(&file_json(99, "", 0, "", 0)).unwrap()],
            43,
            44,
        );
        assert!(matches!(unknown, CacheUpdate::Resync { .. }));
        assert_eq!(cache.state(LIBRARY).unwrap().revision, 43);
        assert!(matches!(
            apply_server_write(&cache, "other-library", &[], 43, 44),
            CacheUpdate::Resync { .. }
        ));
    }

    #[test]
    fn batch_keeps_server_order_without_duplicates_and_applies_all_records() {
        let directory = tempfile::tempdir().unwrap();
        let cache = cached_library(directory.path(), 7);
        let seen = Arc::new(StdMutex::new(Vec::new()));
        let server = server(seen.clone(), |_, _| {
            TestResponse::json(
                200,
                &format!(
                    r#"{{"report":{{"updated":2}},"files":[{},{}],"revision_before":7,"revision":8}}"#,
                    file_json(7, "rain wood", 0, "", 2),
                    file_json(3, "door wood", 0, "", 0)
                ),
            )
        });
        let writer = RemoteWriter::new(
            RemoteClient::new(&server.url, "").unwrap(),
            cache.clone(),
            LIBRARY,
        );
        let outcome = writer
            .update_metadata_batch(&BatchMetadataRequest {
                ids: vec![7, 3, 7],
                patch: MetadataPatch {
                    tags: Some("wood".into()),
                    ..MetadataPatch::default()
                },
                tag_mode: TagEditMode::Append,
            })
            .unwrap();
        let (report, files) = outcome.value;
        assert_eq!(report.updated, 2);
        assert_eq!(files.iter().map(|f| f.id).collect::<Vec<_>>(), [7, 3]);
        assert_eq!(outcome.cache, CacheUpdate::Applied { revision: 8 });
        let seen = seen.lock().unwrap();
        assert_eq!(
            (seen[0].method.as_str(), seen[0].path.as_str()),
            ("POST", "/api/v1/audio/metadata/batch")
        );
        let body: serde_json::Value = serde_json::from_str(&seen[0].body).unwrap();
        assert_eq!(
            body,
            serde_json::json!({"ids":[7,3,7],"patch":{"tags":"wood"},"tag_mode":"append"})
        );
        let reader = snapshot(&cache);
        assert_eq!(reader.get_audio(7).unwrap().tags, "rain wood");
        assert_eq!(reader.get_audio(7).unwrap().play_count, 2);
        assert_eq!(reader.get_audio(3).unwrap().tags, "door wood");
    }

    #[test]
    fn write_errors_are_classified_and_uncertain_ones_request_a_sync() {
        let directory = tempfile::tempdir().unwrap();
        let cache = cached_library(directory.path(), 1);
        let seen = Arc::new(StdMutex::new(Vec::new()));
        let server = server(seen.clone(), |_, path| match path {
            "/api/v1/audio/1/metadata" => TestResponse::json(
                400,
                r#"{"error_code":"VALIDATION","message":"color must be empty or #RRGGBB"}"#,
            ),
            "/api/v1/audio/2/metadata" => {
                TestResponse::json(404, r#"{"error_code":"NOT_FOUND","message":"no audio 2"}"#)
            }
            "/api/v1/audio/3/metadata" => {
                TestResponse::json(409, r#"{"error_code":"BUSY","message":"scan running"}"#)
            }
            "/api/v1/audio/4/metadata" => TestResponse::json(
                413,
                r#"{"error_code":"PAYLOAD_TOO_LARGE","message":"body over 8 MiB"}"#,
            ),
            "/api/v1/audio/5/metadata" => TestResponse::json(
                408,
                r#"{"error_code":"TIMEOUT","message":"request took longer than 30 s"}"#,
            ),
            "/api/v1/audio/6/metadata" => TestResponse::json(200, r#"{"file":{"id":6"#),
            _ => TestResponse::json(500, r#"{"error_code":"INTERNAL","message":"?"}"#),
        });
        let writer = RemoteWriter::new(
            RemoteClient::new(&server.url, "").unwrap(),
            cache.clone(),
            LIBRARY,
        );
        let patch = MetadataPatch {
            notes: Some("x".into()),
            ..MetadataPatch::default()
        };
        let error = |id| writer.update_metadata(id, &patch).unwrap_err();
        let validation = error(1);
        assert!(matches!(validation, RemoteError::Validation(ref m) if m.contains("color")));
        assert!(matches!(error(2), RemoteError::NotFound(_)));
        assert!(matches!(error(3), RemoteError::Busy { .. }));
        let too_large = error(4);
        assert!(matches!(too_large, RemoteError::PayloadTooLarge(_)));
        assert!(too_large.localized(true).contains("413"));
        let timeout = error(5);
        assert!(matches!(timeout, RemoteError::Timeout(_)));
        assert!(timeout.write_may_have_committed());
        assert!(error(6).write_may_have_committed(), "unreadable reply");
        assert!(!validation.write_may_have_committed());
        assert!(!too_large.write_may_have_committed());
        // Local validation fails before any request.
        let before = seen.lock().unwrap().len();
        let invalid = writer
            .update_metadata(
                1,
                &MetadataPatch {
                    color: Some("red".into()),
                    ..MetadataPatch::default()
                },
            )
            .unwrap_err();
        assert!(matches!(invalid, RemoteError::Validation(_)));
        let empty = writer
            .update_metadata_batch(&BatchMetadataRequest {
                ids: vec![1],
                patch: MetadataPatch::default(),
                tag_mode: TagEditMode::Replace,
            })
            .unwrap_err();
        assert!(matches!(empty, RemoteError::Validation(_)));
        assert_eq!(seen.lock().unwrap().len(), before, "rejected locally");

        // Through the DataProvider: a 408 asks the GUI for a sync, a 404 does not.
        let events = Arc::new(StdMutex::new(Vec::new()));
        let listener: WriteListener = {
            let events = events.clone();
            Arc::new(move |event| events.lock().unwrap().push(event))
        };
        let provider = RemoteProvider::new(
            Arc::new(snapshot(&cache)),
            writer.clone(),
            true,
            Some(listener),
        );
        let missing = provider.update_metadata(2, &patch).unwrap_err();
        assert!(matches!(missing, Error::NotFound(ref m) if m.contains("找不到")));
        assert!(events.lock().unwrap().is_empty());
        assert!(provider.update_metadata(5, &patch).is_err());
        assert!(matches!(
            events.lock().unwrap().as_slice(),
            [WriteEvent::Resync { reason }] if reason.contains("uncertain")
        ));
        assert!(provider.remove_audio(&[1]).is_err());
        assert!(provider.relink_audio(1, Path::new("x.wav")).is_err());
        assert_eq!(cache.state(LIBRARY).unwrap().revision, 1);
    }

    #[test]
    fn removals_go_to_the_server_and_resync_when_the_revision_moved() {
        let directory = tempfile::tempdir().unwrap();
        let cache = cached_library(directory.path(), 10);
        let seen = Arc::new(StdMutex::new(Vec::new()));
        let server = server(seen.clone(), |method, path| match (method, path) {
            ("POST", "/api/v1/audio/remove") => TestResponse::json(
                200,
                r#"{"removed":2,"not_found":[],"revision_before":10,"revision":11}"#,
            ),
            ("DELETE", "/api/v1/folders/8") => TestResponse::json(
                200,
                r#"{"removed":0,"folders_removed":0,"revision_before":10,"revision":10}"#,
            ),
            ("DELETE", "/api/v1/folders/5") => {
                TestResponse::json(409, r#"{"error_code":"BUSY","message":"scan running"}"#)
            }
            ("DELETE", "/api/v1/folders/6") => TestResponse::json(
                403,
                r#"{"error_code":"MANAGE_FORBIDDEN","message":"Library management (scans, removing records or folders) is disabled on this server (manage_mode = off)"}"#,
            ),
            _ => TestResponse::json(
                404,
                r#"{"error_code":"NOT_FOUND","message":"No such endpoint"}"#,
            ),
        });
        let events = Arc::new(StdMutex::new(Vec::new()));
        let listener: WriteListener = {
            let events = events.clone();
            Arc::new(move |event| events.lock().unwrap().push(event))
        };
        let writer = RemoteWriter::new(
            RemoteClient::new(&server.url, "").unwrap(),
            cache.clone(),
            LIBRARY,
        );
        assert!(writer.client().api_url().ends_with("/api/v1"));
        let provider =
            RemoteProvider::new(Arc::new(snapshot(&cache)), writer, true, Some(listener));
        assert_eq!(provider.remove_audio(&[3, 7]).unwrap(), 2);
        assert!(matches!(
            events.lock().unwrap().as_slice(),
            [WriteEvent::Resync { reason }] if reason.contains("removed")
        ));
        // The snapshot itself is untouched until the sync replaces it.
        assert_eq!(provider.get_audio(3).unwrap().id, 3);
        assert_eq!(cache.state(LIBRARY).unwrap().revision, 10);
        let busy = provider.remove_folder(5).unwrap_err();
        assert!(
            matches!(busy, Error::Busy(ref m) if m.contains("409")),
            "{busy:?}"
        );
        let forbidden = provider.remove_folder(6).unwrap_err();
        assert!(
            matches!(forbidden, Error::Validation(ref m) if m.contains("已关闭远程管理")),
            "{forbidden:?}"
        );
        let old = provider.remove_folder(7).unwrap_err();
        assert!(
            matches!(old, Error::Validation(ref m) if m.contains("不支持")),
            "{old:?}"
        );
        assert_eq!(events.lock().unwrap().len(), 1, "refusals change nothing");
        // Nothing removed and the snapshot at that revision: no sync.
        assert_eq!(provider.remove_folder(8).unwrap(), 0);
        assert!(matches!(
            events.lock().unwrap().last(),
            Some(WriteEvent::Applied { revision: 10 })
        ));
        assert_eq!(seen.lock().unwrap()[1].method, "DELETE");
    }

    #[test]
    fn plays_post_without_body_and_apply_the_new_play_count() {
        let directory = tempfile::tempdir().unwrap();
        let cache = cached_library(directory.path(), 43);
        let seen = Arc::new(StdMutex::new(Vec::new()));
        let server = server(seen.clone(), |_, _| {
            TestResponse::json(
                200,
                &format!(
                    r#"{{"file":{},"revision_before":43,"revision":44}}"#,
                    file_json(3, "", 0, "", 1)
                ),
            )
        });
        let events = Arc::new(StdMutex::new(Vec::new()));
        let listener: WriteListener = {
            let events = events.clone();
            Arc::new(move |event| events.lock().unwrap().push(event))
        };
        let provider = RemoteProvider::new(
            Arc::new(snapshot(&cache)),
            RemoteWriter::new(
                RemoteClient::new(&server.url, "").unwrap(),
                cache.clone(),
                LIBRARY,
            ),
            false,
            Some(listener),
        );
        provider.record_play(3).unwrap();
        assert_eq!(
            events.lock().unwrap().as_slice(),
            [WriteEvent::Applied { revision: 44 }]
        );
        let seen = seen.lock().unwrap();
        assert_eq!(
            (
                seen[0].method.as_str(),
                seen[0].path.as_str(),
                seen[0].body.as_str()
            ),
            ("POST", "/api/v1/audio/3/plays", "")
        );
        assert_eq!(provider.get_audio(3).unwrap().play_count, 1);
        assert_eq!(cache.state(LIBRARY).unwrap().revision, 44);
    }
}
