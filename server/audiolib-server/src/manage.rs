// server/audiolib-server/src/manage.rs
//! Library management that removes records: `POST /audio/remove` and
//! `DELETE /folders/{id}` (doc/api/README.md §6.8), plus `GET /capabilities`.
//!
//! Removal is the library's own `remove_audio` / `remove_folder` (local-mode semantics):
//! it deletes `audio_files` rows and the rows that reference them (`playlist_items`,
//! `project_references`, `waveform_cache`, `play_history`), and for a folder the folder
//! rows of its subtree and their `folder_closure` entries, then refreshes the folder
//! statistics, all in one transaction. **No file is touched**: files still on disk come
//! back with the next scan. Both endpoints are refused with 409 `BUSY` while a scan (of
//! this server, or a CLI / local-mode desktop scan or administration operation holding
//! the library lease) runs or is paused, and are gated by `manage_mode`
//! (`crate::auth::ManageMode`, 403 `MANAGE_FORBIDDEN`) like scans.
use crate::{
    ApiError, AppState,
    auth::ManageMode,
    state::{folder_key, is_ancestor},
};
use audiolib_core::{DataProvider, Error};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Largest accepted `ids` list of `POST /audio/remove` (the library's own bound).
pub const MAX_REMOVE_IDS: usize = 10_000;
/// Ids per existence query (well below SQLite's bound-parameter limit).
const ID_QUERY_CHUNK: usize = 500;

/// Body of `POST /audio/remove`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoveAudioRequest {
    /// 1..=10000 positive audio ids; duplicates are ignored.
    pub ids: Vec<i64>,
}

/// Reply of `POST /audio/remove`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoveAudioResponse {
    /// Records removed from the library.
    pub removed: u64,
    /// Requested ids without a record (not an error), in request order without duplicates.
    pub not_found: Vec<i64>,
    /// Library revision covering every commit before this request.
    pub revision_before: u64,
    /// `revision_before + 1` when something was removed, otherwise `revision_before`.
    pub revision: u64,
}

/// Reply of `DELETE /folders/{id}`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoveFolderResponse {
    /// Audio records removed (the whole subtree).
    pub removed: u64,
    /// Folder records removed: the folder itself and every folder below it.
    pub folders_removed: u64,
    /// Library revision covering every commit before this request.
    pub revision_before: u64,
    /// `revision_before + 1`.
    pub revision: u64,
}

/// Reply of `GET /capabilities`: what this client may do on this server.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapabilitiesResponse {
    /// Effective `manage_mode`: `all`, `lan` or `off`.
    pub manage_mode: ManageMode,
    /// Whether this request was classified as external (through a reverse proxy or from
    /// a non-private peer, doc/api/README.md §2.2).
    pub external: bool,
    /// Whether this client may start/control scans and remove records or folders now.
    pub can_manage: bool,
}

impl AppState {
    /// Blocking. 409 `BUSY` while a scan of this server runs or is paused, or while any
    /// process holds the library's scan lease (CLI or local-mode desktop scans,
    /// restore, maintenance, ...).
    pub fn ensure_no_scan(&self) -> Result<(), ApiError> {
        if let Some(scan) = self.scans().current() {
            let id = scan.progress.session_id;
            return Err(Error::Busy(if id > 0 {
                format!(
                    "A scan is running for this library (session {id}); retry when it has finished"
                )
            } else {
                "A scan is starting for this library; retry when it has finished".into()
            })
            .into());
        }
        Ok(self.provider().ensure_library_idle()?)
    }

    /// Blocking. Ids of `ids` that have an `audio_files` record.
    fn existing_audio_ids(&self, ids: &[i64]) -> audiolib_core::Result<HashSet<i64>> {
        let mut found = HashSet::new();
        for chunk in ids.chunks(ID_QUERY_CHUNK) {
            let placeholders = vec!["?"; chunk.len()].join(",");
            let sql = format!("SELECT id FROM audio_files WHERE id IN ({placeholders})");
            self.read_query(|conn| {
                let mut statement = conn.prepare(&sql)?;
                let rows = statement.query_map(rusqlite::params_from_iter(chunk), |row| {
                    row.get::<_, i64>(0)
                })?;
                for id in rows {
                    found.insert(id?);
                }
                Ok(())
            })?;
        }
        Ok(found)
    }

    /// Blocking. `POST /audio/remove`: remove the records of `ids` (never any file).
    /// Unknown ids are reported in `not_found`; the revision is bumped once only when a
    /// record was removed. When none of the ids exists the library is not written at all.
    pub fn remove_audio(&self, ids: &[i64]) -> Result<RemoveAudioResponse, ApiError> {
        if ids.is_empty() || ids.len() > MAX_REMOVE_IDS {
            return Err(ApiError::validation(format!(
                "ids must contain 1..={MAX_REMOVE_IDS} audio ids, got {}",
                ids.len()
            )));
        }
        if let Some(bad) = ids.iter().find(|id| **id <= 0) {
            return Err(ApiError::validation(format!(
                "ids must be positive integers, got {bad}"
            )));
        }
        let mut seen = HashSet::new();
        let ids: Vec<i64> = ids.iter().copied().filter(|id| seen.insert(*id)).collect();
        self.ensure_no_scan()?;
        let outcome = self.write_if(
            |provider| {
                let existing = self.existing_audio_ids(&ids)?;
                let (present, not_found): (Vec<i64>, Vec<i64>) =
                    ids.iter().copied().partition(|id| existing.contains(id));
                let removed = if present.is_empty() {
                    0
                } else {
                    provider.remove_audio(&present)?
                };
                Ok((removed, not_found))
            },
            |(removed, _)| *removed > 0,
        )?;
        let (removed, not_found) = outcome.value;
        if outcome.revision != outcome.revision_before {
            self.invalidate_snapshot();
        }
        tracing::info!(
            requested = ids.len(),
            removed,
            not_found = not_found.len(),
            revision = outcome.revision,
            "Removed audio records (files untouched)"
        );
        Ok(RemoveAudioResponse {
            removed,
            not_found,
            revision_before: outcome.revision_before,
            revision: outcome.revision,
        })
    }

    /// Blocking. Path of folder `id` and the number of folders in its subtree (itself
    /// included); `None` when there is no such folder.
    fn folder_subtree(&self, id: i64) -> audiolib_core::Result<Option<(String, u64)>> {
        self.read_query(|conn| {
            let Some(path) = conn
                .query_row("SELECT folder_path FROM folders WHERE id=?1", [id], |row| {
                    row.get::<_, String>(0)
                })
                .optional()?
            else {
                return Ok(None);
            };
            let count: i64 = conn.query_row(
                "WITH RECURSIVE tree(id) AS (SELECT ?1 UNION \
                 SELECT f.id FROM folders f JOIN tree ON f.parent_id=tree.id) \
                 SELECT COUNT(*) FROM tree",
                [id],
                |row| row.get(0),
            )?;
            Ok(Some((path, u64::try_from(count).unwrap_or_default())))
        })
    }

    /// Name of the configured root whose top-level folder is `path`, or that lies below
    /// `path` (removing `path` would remove that root's whole tree).
    fn root_covered_by(&self, path: &str) -> Option<String> {
        let folder = folder_key(path);
        let monitor = self.root_monitor();
        monitor
            .roots()
            .iter()
            .enumerate()
            .find(|(index, root)| {
                let configured = Some(root.server_path.clone());
                let canonical = monitor.status(*index).canonical;
                [configured, canonical].into_iter().flatten().any(|root| {
                    let key = folder_key(&root.to_string_lossy());
                    !key.is_empty() && (key == folder || is_ancestor(&folder, &key))
                })
            })
            .map(|(_, root)| root.name.clone())
    }

    /// Blocking. `DELETE /folders/{id}`: remove the records of the folder's whole subtree
    /// (never any file). Unknown folders are 404; a root's top-level folder (or a folder
    /// containing a root) is refused with 400 `VALIDATION`.
    pub fn remove_folder(&self, id: i64) -> Result<RemoveFolderResponse, ApiError> {
        self.remove_folder_tree(id, true)
    }

    /// Blocking. Remove folder `id`'s subtree; `protect_roots` refuses a root's top-level
    /// folder (the admin page's root removal passes `false`).
    fn remove_folder_tree(
        &self,
        id: i64,
        protect_roots: bool,
    ) -> Result<RemoveFolderResponse, ApiError> {
        self.ensure_no_scan()?;
        let Some((path, _)) = self.folder_subtree(id)? else {
            return Err(Error::NotFound(format!("Folder {id}")).into());
        };
        if let Some(root) = protect_roots.then(|| self.root_covered_by(&path)).flatten() {
            return Err(ApiError::validation(format!(
                "Folder {id} is the top-level folder of library root {root:?} (or contains it) and cannot be removed through the API; remove the library root on the server's admin page (管理页 /admin → 库根目录)"
            )));
        }
        let outcome = self.write_if(
            |provider| {
                let folders = self
                    .folder_subtree(id)?
                    .map(|(_, count)| count)
                    .ok_or_else(|| Error::NotFound(format!("Folder {id}")))?;
                Ok((provider.remove_folder(id)?, folders))
            },
            |_| true,
        )?;
        let (removed, folders_removed) = outcome.value;
        self.invalidate_snapshot();
        tracing::info!(
            folder = id,
            removed,
            folders_removed,
            revision = outcome.revision,
            "Removed a folder's records (files untouched)"
        );
        Ok(RemoveFolderResponse {
            removed,
            folders_removed,
            revision_before: outcome.revision_before,
            revision: outcome.revision,
        })
    }
}

/// `POST /admin/api/roots`.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AddRootRequest {
    /// Client-visible root name (unique, case-insensitively).
    pub name: String,
    /// Absolute directory on the server.
    pub path: String,
}

/// `DELETE /admin/api/roots/{name}`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoveRootResponse {
    pub name: String,
    /// Audio records removed with the root.
    pub removed: u64,
    /// Folder records removed with the root.
    pub folders_removed: u64,
    pub revision_before: u64,
    pub revision: u64,
}

impl AppState {
    /// Blocking. Add a library root (admin page): the directory must exist, be absolute
    /// and neither lie inside nor contain another root. It is stored in
    /// `admin-settings.json` (replacing the `config.toml` list from then on) and applies
    /// at once.
    pub fn add_root(&self, request: AddRootRequest) -> Result<crate::RootResponse, ApiError> {
        let _edit = self.roots_edit_lock();
        let name = request.name;
        crate::config::check_root_name(&name).map_err(|e| ApiError::validation(e.to_string()))?;
        let path = crate::browse::resolve_directory(request.path.trim())?;
        let monitor = self.root_monitor();
        let mut roots = monitor.roots().to_vec();
        if roots
            .iter()
            .any(|root| root.name.eq_ignore_ascii_case(&name))
        {
            return Err(ApiError::validation(format!(
                "A library root named {name:?} already exists"
            )));
        }
        let new_key = folder_key(&path.to_string_lossy());
        for (index, root) in roots.iter().enumerate() {
            let canonical = monitor.status(index).canonical;
            for existing in [Some(root.server_path.clone()), canonical]
                .into_iter()
                .flatten()
            {
                let key = folder_key(&existing.to_string_lossy());
                if key == new_key || is_ancestor(&key, &new_key) || is_ancestor(&new_key, &key) {
                    return Err(ApiError::validation(format!(
                        "{} overlaps the library root {:?} ({}): a root may neither lie inside nor contain another",
                        path.display(),
                        root.name,
                        root.server_path.display()
                    )));
                }
            }
        }
        roots.push(crate::config::RootConfig {
            name: name.clone(),
            server_path: path,
            webdav_url: None,
        });
        self.security()
            .update_settings(|settings| settings.roots = Some(roots.clone()))?;
        self.replace_roots(&roots);
        tracing::info!(root = %name, "Library root added on the admin page");
        let monitor = self.root_monitor();
        let index = roots.len() - 1;
        Ok(self.root_response(&monitor, index, monitor.status(index))?)
    }

    /// Folder ids of the library holding exactly `root`'s top-level directory.
    fn root_folder_ids(
        &self,
        root: &crate::config::RootConfig,
        canonical: Option<std::path::PathBuf>,
    ) -> Result<Vec<i64>, ApiError> {
        let mut texts: Vec<String> = [Some(root.server_path.clone()), canonical]
            .into_iter()
            .flatten()
            .map(|path| {
                audiolib_db::folder_path_text(&path)
                    .trim_end_matches('/')
                    .to_string()
            })
            .collect();
        texts.sort();
        texts.dedup();
        let mut ids = Vec::new();
        for text in texts {
            let found = self.read_query(|conn| {
                conn.query_row(
                    "SELECT id FROM folders WHERE folder_path = ?1 COLLATE NOCASE",
                    [&text],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
            })?;
            ids.extend(found);
        }
        ids.dedup();
        Ok(ids)
    }

    /// Blocking. Remove library root `name` (admin page): its folder and audio records go
    /// (like `DELETE /folders/{id}`; no file is touched) and it disappears from the root
    /// list, persisted and applied at once. 409 `BUSY` while a scan runs, 404 when unknown.
    pub fn remove_root(&self, name: &str) -> Result<RemoveRootResponse, ApiError> {
        let _edit = self.roots_edit_lock();
        self.ensure_no_scan()?;
        let monitor = self.root_monitor();
        let index = monitor
            .roots()
            .iter()
            .position(|root| root.name == name)
            .ok_or_else(|| ApiError::not_found(format!("No library root named {name:?}")))?;
        let root = monitor.roots()[index].clone();
        let ids = self.root_folder_ids(&root, monitor.status(index).canonical)?;
        let (mut removed, mut folders_removed) = (0, 0);
        let (mut before, mut after) = {
            let info = self.revision()?;
            (info.revision, info.revision)
        };
        for (position, id) in ids.iter().enumerate() {
            match self.remove_folder_tree(*id, false) {
                Ok(done) => {
                    if position == 0 {
                        before = done.revision_before;
                    }
                    removed += done.removed;
                    folders_removed += done.folders_removed;
                    after = done.revision;
                }
                Err(error) if error.status() == axum::http::StatusCode::NOT_FOUND => {}
                Err(error) => return Err(error),
            }
        }
        let roots: Vec<_> = monitor
            .roots()
            .iter()
            .filter(|other| other.name != name)
            .cloned()
            .collect();
        self.security()
            .update_settings(|settings| settings.roots = Some(roots.clone()))?;
        self.replace_roots(&roots);
        tracing::info!(root = %name, removed, folders_removed, "Library root removed on the admin page (files untouched)");
        Ok(RemoveRootResponse {
            name: name.to_string(),
            removed,
            folders_removed,
            revision_before: before,
            revision: after,
        })
    }
}
