// app/audiolib-desktop/src/shared_album_artwork.rs
//! Shared entity cover resolution; source WAVs supply pixels but never own choices.
use super::*;
use audiolib_remote::ArtworkReply;

#[cfg(test)]
#[path = "shared_album_artwork_migration_tests.rs"]
mod migration_tests;

fn cn_or(chinese: bool, cn: &str, en: &str) -> String {
    if chinese { cn } else { en }.to_owned()
}

impl Controller {
    pub(super) fn album_facets_available(&self) -> bool {
        // Every admitted GUI provider has an owned schema-8 album projection,
        // including copies of snapshots from older servers.
        !self.remote_read_only() || self.remote.lock().unwrap().state.is_some()
    }

    fn legacy_artwork_scope(&self) -> Option<String> {
        if self.remote_read_only() {
            Some(format!(
                "server:{}",
                self.remote.lock().unwrap().state.as_ref()?.library_id
            ))
        } else {
            self.provider()
                .legacy_album_scope()
                .map(|path| format!("local:{path}"))
        }
    }

    fn shared_artwork_scope(&self) -> Option<(String, bool)> {
        if self.remote_read_only() {
            let remote = self.remote.lock().unwrap();
            let state = remote.state.as_ref()?;
            let legacy = state
                .format
                .as_ref()
                .and_then(|format| format.source.as_ref())
                .map_or(state.schema_version < 8, |source| source.schema_version < 8);
            let scope = if legacy {
                format!("server:{}", state.library_id)
            } else {
                format!(
                    "server:{}:{}",
                    state.library_id,
                    self.provider().album_namespace()
                )
            };
            Some((scope, legacy))
        } else {
            Some((
                format!("local:{}", self.provider().album_namespace()),
                false,
            ))
        }
    }

    pub(super) fn album_artwork_identity(&self, album: &Album) -> Option<(PathBuf, String)> {
        let directory = self
            .config
            .lock()
            .unwrap()
            .paths
            .settings
            .parent()?
            .join("artwork-overrides");
        let (scope, legacy_projection) = self.shared_artwork_scope()?;
        let group = if legacy_projection {
            match album.kind {
                AlbumKind::Metadata => {
                    format!("shared-metadata:{}", album.metadata_key.as_deref()?)
                }
                AlbumKind::Folder => album
                    .folder_path
                    .as_deref()
                    .map(|path| format!("shared-folder:{path}"))
                    .unwrap_or_else(|| "shared-unassigned-folder".into()),
            }
        } else {
            format!("album-id:{}", album.id)
        };
        Some((directory, crate::artwork::key(&[&scope, &group])))
    }

    pub(super) fn artwork_identity(&self, file: &AudioFile) -> Option<(PathBuf, String)> {
        let album = file.album_entity.as_ref()?;
        (file.album_id == Some(album.id)).then_some(())?;
        self.album_artwork_identity(album)
    }

    pub(super) fn album_cached_artwork_path(&self, album: &Album) -> Option<PathBuf> {
        let (_, identity) = self.album_artwork_identity(album)?;
        let cache = self.config.lock().unwrap().paths.cache.clone();
        // Generated data is disposable, while its owner is the same persistent
        // entity regardless of which member is selected or which query is shown.
        let key = crate::artwork::key(&["shared-generated-v1", &identity]);
        Some(cache.join("artwork").join(format!("{key}.png")))
    }

    pub(super) fn cached_artwork_path(&self, file: &AudioFile) -> Option<PathBuf> {
        self.album_cached_artwork_path(file.album_entity.as_ref()?)
    }

    pub(super) fn album_thumbnail(
        &self,
        provider: &SqliteProvider,
        album: &Album,
        index: usize,
    ) -> Option<(Vec<u8>, u32, u32)> {
        if index >= ALBUM_THUMBNAILS || !self.show_album_artwork.load(Ordering::Acquire) {
            return None;
        }
        let _cache = self.artwork_cache_lock.try_lock().ok()?;
        let (directory, key) = self.album_artwork_identity(album)?;
        if directory.join(format!("{key}.hidden")).is_file() {
            return None;
        }
        let custom = crate::artwork::read_thumbnail(&directory.join(format!("{key}.png")), 40);
        let cached = || {
            self.hinted_album_artwork(provider, album)
                .or_else(|| self.album_cached_artwork_path(album))
                .and_then(|path| crate::artwork::read_thumbnail(&path, 40))
        };
        let pixels = custom.or_else(cached)?;
        let (width, height) = pixels.dimensions();
        Some((pixels.into_raw(), width, height))
    }

    pub(super) fn cancel_artwork(&self) {
        self.artwork_epoch.fetch_add(1, Ordering::AcqRel);
        self.artwork_cancel
            .lock()
            .unwrap()
            .store(true, Ordering::Release);
        *self.artwork_job.lock().unwrap() = None;
        *self.artwork_inflight.lock().unwrap() = None;
    }

    pub(super) fn enqueue_artwork(&self, file: &AudioFile) {
        let Some(album) = file
            .album_entity
            .as_ref()
            .filter(|album| file.album_id == Some(album.id))
        else {
            self.cancel_artwork();
            return;
        };
        if !self.show_album_artwork.load(Ordering::Acquire) {
            return;
        }
        let token = (
            self.source_generation.load(Ordering::Acquire),
            album.id,
            self.artwork_epoch.load(Ordering::Acquire),
        );
        let request_cancelled = self.artwork_cancel.lock().unwrap().load(Ordering::Acquire);
        {
            let mut inflight = self.artwork_inflight.lock().unwrap();
            if *inflight == Some(token) && !request_cancelled {
                return;
            }
            *inflight = Some(token);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        std::mem::replace(&mut *self.artwork_cancel.lock().unwrap(), cancel.clone())
            .store(true, Ordering::Release);
        *self.artwork_job.lock().unwrap() = Some(ArtworkJob {
            album: album.clone(),
            source_generation: token.0,
            epoch: token.2,
            cancel,
            notice: Mutex::new(None),
        });
        self.artwork_wake.notify_one();
    }

    pub(super) fn artwork_current(&self, job: &ArtworkJob) -> bool {
        let selected = self.selected.lock().unwrap();
        let same_album = selected
            .as_ref()
            .is_some_and(|file| file.album_id == Some(job.album.id));
        self.available()
            && !self.shutdown.load(Ordering::Acquire)
            && !job.cancel.load(Ordering::Acquire)
            && self.source_generation.load(Ordering::Acquire) == job.source_generation
            && self.artwork_epoch.load(Ordering::Acquire) == job.epoch
            && same_album
            && self.show_album_artwork.load(Ordering::Acquire)
    }

    pub(super) fn artwork_job_finished(&self, job: &ArtworkJob) {
        let token = (job.source_generation, job.album.id, job.epoch);
        let mut inflight = self.artwork_inflight.lock().unwrap();
        if *inflight == Some(token) {
            *inflight = None;
        }
    }

    fn migrate_album_choices(
        &self,
        job: &ArtworkJob,
        provider: &SqliteProvider,
        album: &Album,
        directory: &Path,
        key: &str,
    ) {
        let already = self
            .artwork_override_migrations
            .lock()
            .unwrap()
            .contains(&(job.source_generation, key.to_owned()));
        if already {
            return;
        }
        let result = (|| -> Result<crate::album_override_migration::Outcome, String> {
            let existing = crate::album_override_migration::choice_keys(directory)
                .map_err(|e| e.to_string())?;
            let Some(scope) = self.legacy_artwork_scope() else {
                return Ok(crate::album_override_migration::Outcome::NotNeeded);
            };
            let mut keys = Vec::new();
            let shared_alias = match album.kind {
                AlbumKind::Metadata => album
                    .metadata_key
                    .as_deref()
                    .map(|name| format!("shared-metadata:{name}")),
                AlbumKind::Folder => Some(
                    album
                        .folder_path
                        .as_deref()
                        .map(|path| format!("shared-folder:{path}"))
                        .unwrap_or_else(|| "shared-unassigned-folder".into()),
                ),
            }
            .map(|group| crate::artwork::key(&[&scope, &group]));
            if let Some(alias) = shared_alias.filter(|alias| existing.contains(alias)) {
                keys.push(alias);
            }
            if album.kind == AlbumKind::Metadata {
                if let Some(name) = album.metadata_key.as_deref() {
                    let old = crate::artwork::key(&[&scope, &format!("album:{name}")]);
                    if existing.contains(&old) {
                        keys.push(old);
                    }
                }
            } else if !existing.is_empty() && !existing.contains(key) {
                let mut after = None;
                loop {
                    if !self.artwork_current(job) {
                        return Err("Cancelled".into());
                    }
                    let ids = provider
                        .album_member_ids(album.id, after, 4096, job.cancel.clone())
                        .map_err(|e| e.to_string())?;
                    if ids.is_empty() {
                        break;
                    }
                    after = ids.last().copied();
                    for id in ids {
                        let old = crate::artwork::key(&[&scope, &format!("file:{id}")]);
                        if existing.contains(&old) {
                            keys.push(old);
                        }
                    }
                }
            }
            let _cache = self.artwork_cache_lock.lock().unwrap();
            if !self.artwork_current(job) {
                return Err("Cancelled".into());
            }
            crate::album_override_migration::migrate(directory, key, &keys)
                .map_err(|e| e.to_string())
        })();
        if !self.artwork_current(job) {
            return;
        }
        if result.is_ok() {
            self.artwork_override_migrations
                .lock()
                .unwrap()
                .insert((job.source_generation, key.to_owned()));
        }
        let message = match result {
            Ok(crate::album_override_migration::Outcome::Conflict(paths)) => {
                Some(if self.chinese.load(Ordering::Acquire) {
                    format!(
                        "此专辑有 {} 个冲突的旧封面选择，请重新指定专辑封面；原文件已保留",
                        paths.len()
                    )
                } else {
                    format!(
                        "{} conflicting legacy cover choices; choose this album's cover. Original files are retained.",
                        paths.len()
                    )
                })
            }
            Err(error) => Some(format!(
                "{}: {error}",
                cn_or(
                    self.chinese.load(Ordering::Acquire),
                    "旧封面设置迁移未完成",
                    "Legacy cover migration did not complete"
                )
            )),
            _ => None,
        };
        if let Some(message) = message {
            *job.notice.lock().unwrap() = Some(message);
        }
    }

    pub(super) fn load_artwork(&self, job: &ArtworkJob) -> Option<image::RgbaImage> {
        if !self.artwork_current(job) {
            return None;
        }
        let album = &job.album;
        let provider = self.provider();
        let (directory, key) = self.album_artwork_identity(album)?;
        self.migrate_album_choices(job, &provider, album, &directory, &key);
        let image_path = self.album_cached_artwork_path(album)?;
        let etag_path = image_path.with_extension("etag");
        let (cached, etag) = {
            let _cache = self.artwork_cache_lock.lock().unwrap();
            if !self.artwork_current(job) || directory.join(format!("{key}.hidden")).is_file() {
                return None;
            }
            if let Some(custom) =
                crate::artwork::read_thumbnail(&directory.join(format!("{key}.png")), 512)
            {
                return Some(custom);
            }
            let cached = crate::artwork::read_thumbnail(&image_path, 512);
            let etag = cached
                .as_ref()
                .and_then(|_| std::fs::metadata(&etag_path).ok())
                .filter(|info| info.len() <= 4096)
                .and_then(|_| std::fs::read_to_string(&etag_path).ok());
            (cached, etag)
        };
        let members = provider
            .album_members(
                album.id,
                audiolib_core::MAX_ALBUM_MEMBERS,
                job.cancel.clone(),
            )
            .ok()?;
        if !self.artwork_current(job) {
            return None;
        }
        if self.remote_read_only() {
            let (client, state, capabilities) = self.remote_artwork_context()?;
            let ids = members.iter().map(|file| file.id).collect::<Vec<_>>();
            match client.album_artwork_for_snapshot(
                &state,
                &capabilities,
                &provider.album_namespace(),
                album.id,
                &ids,
                Some(512),
                etag.as_deref(),
                &job.cancel,
            ) {
                Ok(ArtworkReply::NotModified) => cached,
                Ok(ArtworkReply::Data { bytes, etag, .. }) => {
                    let pixels = crate::artwork::decode_thumbnail(&bytes, 512)?;
                    self.publish_album_cache(job, &image_path, &pixels, etag.as_deref());
                    Some(pixels)
                }
                Ok(ArtworkReply::Missing) => {
                    self.remove_album_cache(job, &image_path);
                    None
                }
                Err(audiolib_remote::RemoteError::LibraryChanged(_)) => None,
                Err(_) => cached,
            }
        } else {
            let versions = members
                .iter()
                .map(|file| {
                    format!(
                        "{}:{}:{}:{}",
                        file.id,
                        file.file_path,
                        file.file_size,
                        file.attributes["modified_at"].as_str().unwrap_or_default()
                    )
                })
                .collect::<Vec<_>>();
            let fingerprint =
                crate::artwork::key(&versions.iter().map(String::as_str).collect::<Vec<_>>());
            if etag.as_deref() == Some(fingerprint.as_str()) && cached.is_some() {
                return cached;
            }
            let allowed = self.config.lock().unwrap().allowed_scan_roots.clone();
            for member in members {
                if !self.artwork_current(job) {
                    return None;
                }
                let Some(path) =
                    audiolib_core::validate_wav_path(Path::new(&member.file_path), &allowed).ok()
                else {
                    continue;
                };
                let pixels = audiolib_audio::probe_with_artwork(&path)
                    .ok()
                    .and_then(|(_, art)| art)
                    .and_then(|art| crate::artwork::decode_thumbnail(&art.bytes, 512));
                if let Some(pixels) = pixels {
                    self.publish_album_cache(job, &image_path, &pixels, Some(&fingerprint));
                    return Some(pixels);
                }
            }
            self.remove_album_cache(job, &image_path);
            None
        }
    }

    fn publish_album_cache(
        &self,
        job: &ArtworkJob,
        path: &Path,
        pixels: &image::RgbaImage,
        etag: Option<&str>,
    ) {
        let _cache = self.artwork_cache_lock.lock().unwrap();
        if !self.artwork_current(job) {
            return;
        }
        if crate::artwork::save_thumbnail(path, pixels).is_ok() {
            if let Some(etag) = etag {
                let _ = std::fs::write(path.with_extension("etag"), etag);
            } else {
                let _ = std::fs::remove_file(path.with_extension("etag"));
            }
        }
        let cache = self.config.lock().unwrap().paths.cache.clone();
        let _ = crate::artwork::clean_cache(&cache, false);
    }

    fn remove_album_cache(&self, job: &ArtworkJob, path: &Path) {
        let _cache = self.artwork_cache_lock.lock().unwrap();
        if self.artwork_current(job) {
            let _ = std::fs::remove_file(path);
            let _ = std::fs::remove_file(path.with_extension("etag"));
        }
    }
}
