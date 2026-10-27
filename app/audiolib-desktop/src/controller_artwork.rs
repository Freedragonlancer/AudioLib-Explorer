// app/audiolib-desktop/src/controller_artwork.rs
//! Independent artwork worker and client-local cover management, outside the audio path.
use super::*;
use audiolib_remote::ArtworkReply;

#[derive(Default)]
pub(super) struct ArtworkActions {
    pub(super) pending: usize,
    failure: Option<String>,
}

/// Destinations are captured while admitting the action on the event loop. An
/// accepted edit belongs to that source even if the user switches sources later.
pub(super) struct ArtworkActionContext {
    cache_root: PathBuf,
    thumbnail_path: Option<PathBuf>,
    identity: Option<(PathBuf, String)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ArtworkActionError {
    Input(String),
    Transient(String),
    Persistence(String),
}

impl ArtworkActionError {
    fn into_message(self) -> String {
        match self {
            Self::Input(message) | Self::Transient(message) | Self::Persistence(message) => message,
        }
    }
}

pub(super) struct ArtworkActionGuard {
    controller: Arc<Controller>,
    result: Option<Result<bool, ArtworkActionError>>,
}

impl ArtworkActionGuard {
    pub(super) fn complete(mut self, result: Result<bool, ArtworkActionError>) {
        self.result = Some(result);
    }
}

impl Drop for ArtworkActionGuard {
    fn drop(&mut self) {
        let mut actions = self.controller.artwork_actions.lock().unwrap();
        match self.result.take() {
            Some(Ok(true)) => actions.failure = None,
            Some(
                Ok(false) | Err(ArtworkActionError::Input(_) | ArtworkActionError::Transient(_)),
            ) => {}
            Some(Err(ArtworkActionError::Persistence(error))) => actions.failure = Some(error),
            None => {
                actions.failure = Some(internal_error(
                    self.controller.chinese.load(Ordering::Acquire),
                    "保存封面设置",
                    "Saving artwork settings",
                ));
            }
        }
        actions.pending -= 1;
        self.controller.artwork_actions_wake.notify_all();
    }
}

impl Controller {
    pub(super) fn begin_artwork_action(self: &Arc<Self>) -> ArtworkActionGuard {
        self.artwork_actions.lock().unwrap().pending += 1;
        ArtworkActionGuard {
            controller: self.clone(),
            result: None,
        }
    }

    /// Drain admitted persistent edits, including while the close gate is held.
    pub(super) fn wait_artwork_actions(&self) -> Result<(), String> {
        let mut actions = self.artwork_actions.lock().unwrap();
        while actions.pending != 0 {
            actions = self.artwork_actions_wake.wait(actions).unwrap();
        }
        actions.failure.clone().map_or(Ok(()), |error| {
            Err(format!(
                "{}: {error}",
                cn_or(
                    self.chinese.load(Ordering::Acquire),
                    "封面设置未保存",
                    "Artwork settings were not saved",
                )
            ))
        })
    }

    pub(super) fn artwork_action_context(&self, file: Option<&AudioFile>) -> ArtworkActionContext {
        let cache_root = self.config.lock().unwrap().paths.cache.clone();
        ArtworkActionContext {
            cache_root,
            thumbnail_path: file.and_then(|file| self.cached_artwork_path(file)),
            identity: file.and_then(|file| self.artwork_identity(file)),
        }
    }

    pub(super) fn album_facets_available(&self) -> bool {
        // Schema 6 has no indexed album projection. Keep its remote search path free
        // of a full-library JSON aggregation and never upgrade a snapshot for covers.
        !self.remote_read_only()
            || self
                .remote
                .lock()
                .unwrap()
                .state
                .as_ref()
                .is_some_and(|state| state.schema_version >= 7)
    }

    pub(super) fn album_thumbnail(
        &self,
        provider: &SqliteProvider,
        album: &str,
        index: usize,
        cancel: Arc<AtomicBool>,
    ) -> Option<(Vec<u8>, u32, u32)> {
        if index >= ALBUM_THUMBNAILS || !self.show_album_artwork.load(Ordering::Acquire) {
            return None;
        }
        // A slow cover request must not stall the search/facet worker.
        let _cache = self.artwork_cache_lock.try_lock().ok()?;
        let request = SearchRequest {
            filters: SearchFilters {
                album: Some(album.into()),
                ..Default::default()
            },
            limit: 1,
            ..Default::default()
        };
        let file = provider
            .search_cancellable(&request, cancel)
            .ok()?
            .results
            .into_iter()
            .next()?;
        let (overrides, key) = self.artwork_identity(&file)?;
        if overrides.join(format!("{key}.hidden")).is_file() {
            return None;
        }
        let custom = crate::artwork::read_thumbnail(&overrides.join(format!("{key}.png")), 40);
        // Facets read generated pixels only. No stat/canonicalize/decode of a source
        // WAV here: a sleeping NAS must not block the next search's directory counts.
        let pixels = custom.or_else(|| {
            self.hinted_album_artwork(provider, album)
                .and_then(|path| crate::artwork::read_thumbnail(&path, 40))
                .or_else(|| {
                    self.cached_artwork_path(&file)
                        .and_then(|path| crate::artwork::read_thumbnail(&path, 40))
                })
        })?;
        let (width, height) = pixels.dimensions();
        Some((pixels.into_raw(), width, height))
    }

    pub(super) fn cached_artwork_path(&self, file: &AudioFile) -> Option<PathBuf> {
        let root = self.config.lock().unwrap().paths.cache.clone();
        if !self.remote_read_only() {
            return Some(crate::artwork::thumbnail_path(
                &root,
                self.provider().database_path(),
                file,
                &audiolib_core::album(&file.attributes),
            ));
        }
        let state = self.remote.lock().unwrap().state.clone()?;
        let key = crate::artwork::key(&[
            "remote-v1",
            &state.library_id,
            &state.revision.to_string(),
            &file.id.to_string(),
        ]);
        Some(root.join("artwork").join(format!("{key}.png")))
    }

    pub(super) fn artwork_identity(&self, file: &AudioFile) -> Option<(PathBuf, String)> {
        let config = self.config.lock().unwrap();
        let directory = config.paths.settings.parent()?.join("artwork-overrides");
        drop(config);
        let library = if self.remote_read_only() {
            format!(
                "server:{}",
                self.remote.lock().unwrap().state.as_ref()?.library_id
            )
        } else {
            format!("local:{}", self.provider().database_path().display())
        };
        let album = audiolib_core::album(&file.attributes);
        let group = if album.is_empty() {
            format!("file:{}", file.id)
        } else {
            format!("album:{album}")
        };
        Some((directory, crate::artwork::key(&[&library, &group])))
    }

    pub(super) fn cancel_artwork(&self) {
        self.artwork_epoch.fetch_add(1, Ordering::AcqRel);
        self.artwork_cancel
            .lock()
            .unwrap()
            .store(true, Ordering::Release);
        *self.artwork_job.lock().unwrap() = None;
    }

    pub(super) fn enqueue_artwork(&self, file: &AudioFile) {
        if !self.show_album_artwork.load(Ordering::Acquire) {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        std::mem::replace(&mut *self.artwork_cancel.lock().unwrap(), cancel.clone())
            .store(true, Ordering::Release);
        *self.artwork_job.lock().unwrap() = Some(ArtworkJob {
            file: file.clone(),
            generation: self.selection_generation.load(Ordering::Acquire),
            source_generation: self.source_generation.load(Ordering::Acquire),
            epoch: self.artwork_epoch.load(Ordering::Acquire),
            cancel,
        });
        self.artwork_wake.notify_one();
    }

    pub(super) fn artwork_current(&self, job: &ArtworkJob) -> bool {
        self.available()
            && !self.shutdown.load(Ordering::Acquire)
            && !job.cancel.load(Ordering::Acquire)
            && self.selection_generation.load(Ordering::Acquire) == job.generation
            && self.source_generation.load(Ordering::Acquire) == job.source_generation
            && self.artwork_epoch.load(Ordering::Acquire) == job.epoch
            && self.show_album_artwork.load(Ordering::Acquire)
    }

    pub(super) fn load_artwork(&self, job: &ArtworkJob) -> Option<image::RgbaImage> {
        if !self.artwork_current(job) {
            return None;
        }
        let (overrides, key) = self.artwork_identity(&job.file)?;
        {
            let _cache = self.artwork_cache_lock.lock().unwrap();
            if !self.artwork_current(job) || overrides.join(format!("{key}.hidden")).is_file() {
                return None;
            }
            if let Some(pixels) =
                crate::artwork::read_thumbnail(&overrides.join(format!("{key}.png")), 512)
            {
                return Some(pixels);
            }
        }
        if job.file.missing {
            return None;
        }
        let (cache_root, allowed) = {
            let config = self.config.lock().unwrap();
            (
                config.paths.cache.clone(),
                config.allowed_scan_roots.clone(),
            )
        };
        if !self.remote_read_only() {
            // Keep the immutable destination before touching a potentially sleeping
            // source. Cache actions never wait for source validation or decoding.
            let cache = self.cached_artwork_path(&job.file)?;
            let path =
                audiolib_core::validate_wav_path(Path::new(&job.file.file_path), &allowed).ok()?;
            let pixels = audiolib_audio::probe_with_artwork(&path)
                .ok()?
                .1
                .and_then(|art| crate::artwork::decode_thumbnail(&art.bytes, 512))?;
            let _cache = self.artwork_cache_lock.lock().unwrap();
            if self.artwork_current(job) {
                let _ = crate::artwork::save_thumbnail(&cache, &pixels);
                let _ = crate::artwork::clean_cache(&cache_root, false);
            }
            return Some(pixels);
        }
        let (client, state) = self.remote_artwork_context()?;
        let cache_key = crate::artwork::key(&[
            "remote-v1",
            &state.library_id,
            &state.revision.to_string(),
            &job.file.id.to_string(),
        ]);
        let image_path = cache_root.join("artwork").join(format!("{cache_key}.png"));
        let etag_path = image_path.with_extension("etag");
        let (cached, etag) = {
            let _cache = self.artwork_cache_lock.lock().unwrap();
            if !self.artwork_current(job) {
                return None;
            }
            let cached = crate::artwork::read_thumbnail(&image_path, 512);
            let etag = cached
                .as_ref()
                .and_then(|_| std::fs::metadata(&etag_path).ok())
                .filter(|metadata| metadata.len() <= 4096)
                .and_then(|_| std::fs::read_to_string(&etag_path).ok());
            (cached, etag)
        };
        // The socket can remain busy while a server root wakes; no cache lock is
        // held across HTTP, and cancellation still interrupts this request.
        match client.artwork_for_library(
            &state.library_id,
            job.file.id,
            Some(512),
            etag.as_deref(),
            &job.cancel,
        ) {
            Ok(ArtworkReply::NotModified) => cached,
            Ok(ArtworkReply::Data { bytes, etag, .. }) => {
                let pixels = crate::artwork::decode_thumbnail(&bytes, 512)?;
                let _cache = self.artwork_cache_lock.lock().unwrap();
                if self.artwork_current(job) {
                    if crate::artwork::save_thumbnail(&image_path, &pixels).is_ok() {
                        if let Some(etag) = etag {
                            let _ = std::fs::write(&etag_path, etag);
                        } else {
                            let _ = std::fs::remove_file(&etag_path);
                        }
                    }
                    let _ = crate::artwork::clean_cache(&cache_root, false);
                }
                Some(pixels)
            }
            Ok(ArtworkReply::Missing) => {
                let _cache = self.artwork_cache_lock.lock().unwrap();
                if self.artwork_current(job) {
                    let _ = std::fs::remove_file(image_path);
                    let _ = std::fs::remove_file(etag_path);
                }
                None
            }
            Err(audiolib_remote::RemoteError::LibraryChanged(_)) => None,
            Err(_) => cached,
        }
    }

    pub(super) fn start_artwork_worker(self: &Arc<Self>) {
        let c = self.clone();
        std::thread::Builder::new()
            .name("audiolib-artwork".into())
            .spawn(move || {
                loop {
                    let mut pending = c.artwork_job.lock().unwrap();
                    while pending.is_none() && !c.shutdown.load(Ordering::Acquire) {
                        pending = c.artwork_wake.wait(pending).unwrap();
                    }
                    if c.shutdown.load(Ordering::Acquire) {
                        return;
                    }
                    let job = pending.take().unwrap();
                    drop(pending);
                    if !c.artwork_current(&job) {
                        continue;
                    }
                    let pixels = c.load_artwork(&job);
                    let confirmed = c.clone();
                    post(c.ui.clone(), move |ui| {
                        if !confirmed.artwork_current(&job) {
                            return;
                        }
                        let ready = pixels.is_some();
                        let image = pixels
                            .map(|pixels| {
                                Image::from_rgba8(
                                    SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
                                        pixels.as_raw(),
                                        pixels.width(),
                                        pixels.height(),
                                    ),
                                )
                            })
                            .unwrap_or_default();
                        ui.set_album_cover(image.clone());
                        ui.set_album_cover_ready(ready);
                        if !ready {
                            return;
                        }
                        confirmed.remember_album_artwork(&job);
                        let album = audiolib_core::album(&job.file.attributes);
                        for index in 0..ui.get_albums().row_count() {
                            if let Some(mut row) = ui.get_albums().row_data(index)
                                && row.name.as_str() == album
                            {
                                row.cover = image.clone();
                                ui.get_albums().set_row_data(index, row);
                            }
                        }
                    });
                }
            })
            .expect("spawn artwork worker");
    }

    pub(super) fn perform_artwork_action(
        &self,
        context: ArtworkActionContext,
        action: i32,
        chosen: Option<PathBuf>,
        chinese: bool,
    ) -> Result<bool, ArtworkActionError> {
        // Decode a chosen source before taking the cache lock, just as automatic
        // extraction does. All subsequent mutations use admission-time paths.
        let chosen_pixels = if action == 2 {
            let Some(path) = chosen else { return Ok(false) };
            Some(crate::artwork::read_thumbnail(&path, 512).ok_or_else(|| {
                ArtworkActionError::Input(cn_or(
                    chinese,
                    "请选择有效 PNG/JPEG（不超过 8 MiB、4096×4096）",
                    "Choose a valid PNG/JPEG (up to 8 MiB, 4096×4096)",
                ))
            })?)
        } else {
            None
        };
        let _cache = self.artwork_cache_lock.lock().unwrap();
        if action == 0 {
            if let Some(path) = context.thumbnail_path {
                remove_if_present(&path).map_err(ArtworkActionError::Transient)?;
                remove_if_present(&path.with_extension("etag"))
                    .map_err(ArtworkActionError::Transient)?;
            }
            return Ok(true);
        }
        if action == 1 {
            crate::artwork::clean_cache(&context.cache_root, true)
                .map_err(|error| ArtworkActionError::Transient(error.to_string()))?;
            return Ok(true);
        }
        let (directory, key) = context
            .identity
            .ok_or_else(|| ArtworkActionError::Input("No active library".to_string()))?;
        let image_path = directory.join(format!("{key}.png"));
        let hidden_path = directory.join(format!("{key}.hidden"));
        match action {
            2 => {
                crate::artwork::save_thumbnail(&image_path, &chosen_pixels.unwrap())
                    .map_err(|error| ArtworkActionError::Persistence(error.to_string()))?;
                remove_if_present(&hidden_path).map_err(ArtworkActionError::Persistence)?;
            }
            3 => {
                std::fs::create_dir_all(&directory)
                    .map_err(|error| ArtworkActionError::Persistence(error.to_string()))?;
                std::fs::write(&hidden_path, [])
                    .map_err(|error| ArtworkActionError::Persistence(error.to_string()))?;
                remove_if_present(&image_path).map_err(ArtworkActionError::Persistence)?;
            }
            4 => {
                remove_if_present(&image_path).map_err(ArtworkActionError::Persistence)?;
                remove_if_present(&hidden_path).map_err(ArtworkActionError::Persistence)?;
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    pub(super) fn finish_artwork_action(
        self: &Arc<Self>,
        ui: &AppWindow,
        source_generation: u64,
        chinese: bool,
        result: Result<bool, String>,
    ) {
        ui.set_artwork_busy(false);
        if self.source_generation.load(Ordering::Acquire) != source_generation || !self.available()
        {
            return;
        }
        let refresh_search = match result {
            Ok(false) => false,
            Ok(true) => {
                ui.set_artwork_message(
                    cn_or(chinese, "封面设置已更新", "Artwork settings updated").into(),
                );
                ui.set_album_cover_ready(false);
                ui.set_album_cover(Image::default());
                true
            }
            Err(error) => {
                ui.set_artwork_message(error.into());
                true
            }
        };
        // Cancellation and errors must resume the automatic request invalidated
        // at admission. Otherwise cancelling the picker can leave a blank cover.
        let file = self.selected.lock().unwrap().clone();
        if let Some(file) = file {
            self.queue_artwork(&file);
        }
        if refresh_search {
            self.request_search(ui, true, false);
        }
    }

    pub(super) fn bind_artwork(self: &Arc<Self>, ui: &AppWindow) {
        let c = self.clone();
        ui.on_artwork_enabled_requested(move |enabled| {
            if !c.available() {
                return;
            }
            c.cancel_artwork();
            c.show_album_artwork.store(enabled, Ordering::Release);
            *c.single_click_play_pending.lock().unwrap() = true;
            c.single_click_play_wake.notify_one();
            if let Some(ui) = c.ui.upgrade() {
                ui.set_artwork_enabled(enabled);
                ui.set_album_cover_ready(false);
                ui.set_album_cover(Image::default());
                ui.set_artwork_message("".into());
                let file = c.selected.lock().unwrap().clone();
                if let Some(file) = file {
                    c.queue_artwork(&file);
                }
                c.request_search(&ui, true, false);
            }
        });
        let c = self.clone();
        ui.on_artwork_action_requested(move |action| {
            if !c.available() {
                return;
            }
            let Some(ui) = c.ui.upgrade() else { return };
            if ui.get_artwork_busy() {
                return;
            }
            let file = c.selected.lock().unwrap().clone();
            if action != 1 && file.is_none() {
                return;
            }
            let context = c.artwork_action_context(file.as_ref());
            let source_generation = c.source_generation.load(Ordering::Acquire);
            let owner = crate::dialogs::owner_handle(ui.window());
            let chinese = c.chinese.load(Ordering::Acquire);
            c.cancel_artwork();
            ui.set_artwork_busy(true);
            ui.set_artwork_message("".into());
            let guard = c.begin_artwork_action();
            let worker = c.clone();
            std::thread::spawn(move || {
                // Native selection happens before taking the cache lock; playback and
                // selection remain independent while the user chooses a picture.
                let result = survive_panic(
                    || {
                        let chosen = if action == 2 {
                            crate::dialogs::choose(
                                false,
                                crate::dialogs::FileKind::Image,
                                chinese,
                                owner,
                            )
                        } else {
                            Ok(None)
                        }
                        .map_err(ArtworkActionError::Input)?;
                        worker.perform_artwork_action(context, action, chosen, chinese)
                    },
                    || {
                        ArtworkActionError::Persistence(internal_error(
                            chinese,
                            "保存封面设置",
                            "Saving artwork settings",
                        ))
                    },
                );
                guard.complete(result.clone());
                let confirmed = worker.clone();
                post(worker.ui.clone(), move |ui| {
                    confirmed.finish_artwork_action(
                        &ui,
                        source_generation,
                        chinese,
                        result.map_err(ArtworkActionError::into_message),
                    );
                });
            });
        });
    }
}

fn remove_if_present(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn cn_or(chinese: bool, chinese_text: &str, english_text: &str) -> String {
    if chinese { chinese_text } else { english_text }.to_string()
}
