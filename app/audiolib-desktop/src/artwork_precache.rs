//! Opt-in, bounded local cover warmup. One representative directory per Metadata album.
use super::*;
use crate::directory_artwork_choices::Choice;

impl Controller {
    pub(super) fn cancel_artwork_precache(&self) {
        self.artwork_precache_cancel
            .lock()
            .unwrap()
            .store(true, Ordering::Release);
    }

    pub(super) fn start_artwork_precache(self: &Arc<Self>) {
        if !self.pregenerate_album_artwork.load(Ordering::Acquire)
            || !self.show_album_artwork.load(Ordering::Acquire)
            || self.remote_read_only()
            || self.closing.load(Ordering::Acquire)
            || self.shutdown.load(Ordering::Acquire)
            || self.artwork_precache_busy.swap(true, Ordering::AcqRel)
        {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        *self.artwork_precache_cancel.lock().unwrap() = cancel.clone();
        let provider = self.provider();
        let source_generation = self.source_generation.load(Ordering::Acquire);
        let worker = self.clone();
        post(self.ui.clone(), |ui| {
            ui.set_artwork_precache_busy(true);
            ui.set_artwork_precache_status(
                if ui.get_chinese() {
                    "正在准备厂牌封面…"
                } else {
                    "Preparing library covers…"
                }
                .into(),
            );
        });
        let spawned = std::thread::Builder::new()
            .name("audiolib-artwork-precache".into())
            .spawn(move || {
                let result = worker.precache_artwork(&provider, source_generation, &cancel);
                worker.artwork_precache_busy.store(false, Ordering::Release);
                let changed_source =
                    worker.source_generation.load(Ordering::Acquire) != source_generation;
                let confirmed = worker.clone();
                post(worker.ui.clone(), move |ui| {
                    if confirmed.source_generation.load(Ordering::Acquire) != source_generation {
                        return;
                    }
                    ui.set_artwork_precache_busy(false);
                    ui.set_artwork_precache_status(
                        match result {
                            Ok((done, total, found)) if cancel.load(Ordering::Acquire) => {
                                if ui.get_chinese() {
                                    format!("已取消：检查 {done}/{total} 个厂牌，找到 {found} 张封面")
                                } else {
                                    format!("Cancelled: checked {done}/{total} libraries, found {found} covers")
                                }
                            }
                            Ok((_, total, found)) => {
                                if ui.get_chinese() {
                                    format!("已检查 {total} 个厂牌，找到 {found} 张封面")
                                } else {
                                    format!("Checked {total} libraries, found {found} covers")
                                }
                            }
                            Err(error) => error.to_string(),
                        }
                        .into(),
                    );
                    ui.invoke_artwork_cache_refresh_requested();
                });
                if changed_source {
                    worker.start_artwork_precache();
                }
            });
        if let Err(error) = spawned {
            self.artwork_precache_busy.store(false, Ordering::Release);
            post(self.ui.clone(), move |ui| {
                ui.set_artwork_precache_busy(false);
                ui.set_artwork_precache_status(error.to_string().into());
            });
        }
    }

    fn precache_artwork(
        self: &Arc<Self>,
        provider: &SqliteProvider,
        source_generation: u64,
        cancel: &Arc<AtomicBool>,
    ) -> audiolib_core::Result<(usize, usize, usize)> {
        let request = SearchRequest::default();
        let albums = match provider.album_facets_cancellable(&request, cancel.clone()) {
            Ok(albums) => albums,
            Err(audiolib_core::Error::Cancelled) => return Ok((0, 0, 0)),
            Err(error) => return Err(error),
        };
        let albums: Vec<_> = albums
            .into_iter()
            .filter(|facet| facet.album.kind == AlbumKind::Metadata)
            .collect();
        let total = albums.len();
        let mut done = 0;
        let mut found = 0;
        let allowed = self.config.lock().unwrap().allowed_scan_roots.clone();
        for facet in albums {
            if cancel.load(Ordering::Acquire)
                || self.shutdown.load(Ordering::Acquire)
                || self.source_generation.load(Ordering::Acquire) != source_generation
                || !self.pregenerate_album_artwork.load(Ordering::Acquire)
            {
                break;
            }
            let album = facet.album;
            let Some(representative) = album
                .representative_audio_id
                .and_then(|id| provider.get_audio(id).ok())
            else {
                done += 1;
                continue;
            };
            let folder = Self::artwork_folder_context_for(provider, &representative);
            let Some(path) = self.directory_cached_artwork_path(&album, &folder) else {
                done += 1;
                continue;
            };
            let existing = {
                let _cache = self.artwork_cache_lock.lock().unwrap();
                match self.directory_artwork_choice(&album, &folder, 40) {
                    Ok(Some(Choice::Hidden)) => Some(false),
                    Ok(Some(Choice::Pixels(_))) => Some(true),
                    Ok(Some(Choice::Embedded) | None) => {
                        crate::artwork::read_thumbnail(&path, 40).map(|_| true)
                    }
                    Err(_) => Some(false),
                }
            };
            let available = if let Some(available) = existing {
                available
            } else {
                let members_result = match folder.folder_id {
                    Some(id) => provider.album_members_in_folder(
                        album.id,
                        id,
                        audiolib_core::MAX_ALBUM_MEMBERS,
                        cancel.clone(),
                    ),
                    None => provider.album_members_in_unassigned_directory(
                        album.id,
                        &folder.path,
                        audiolib_core::MAX_ALBUM_MEMBERS,
                        cancel.clone(),
                    ),
                };
                let members = match members_result {
                    Ok(members) => members,
                    Err(audiolib_core::Error::Cancelled) => break,
                    Err(_) => {
                        done += 1;
                        continue;
                    }
                };
                let fingerprint = crate::artwork::key(
                    &members
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
                        .collect::<Vec<_>>()
                        .iter()
                        .map(String::as_str)
                        .collect::<Vec<_>>(),
                );
                let mut available = false;
                for member in members {
                    if cancel.load(Ordering::Acquire) {
                        break;
                    }
                    let Some(source) =
                        audiolib_core::validate_wav_path(Path::new(&member.file_path), &allowed)
                            .ok()
                    else {
                        continue;
                    };
                    let Some(pixels) = audiolib_audio::probe_with_artwork(&source)
                        .ok()
                        .and_then(|(_, art)| art)
                        .and_then(|art| crate::artwork::decode_thumbnail(&art.bytes, 512))
                    else {
                        continue;
                    };
                    let _cache = self.artwork_cache_lock.lock().unwrap();
                    if !cancel.load(Ordering::Acquire)
                        && self.source_generation.load(Ordering::Acquire) == source_generation
                        && crate::artwork::save_thumbnail(&path, &pixels).is_ok()
                    {
                        let _ = std::fs::write(path.with_extension("etag"), &fingerprint);
                        let cache = self.config.lock().unwrap().paths.cache.clone();
                        let _ = crate::artwork::clean_cache(&cache, false);
                        available = true;
                    }
                    break;
                }
                available
            };
            done += 1;
            found += usize::from(available);
            let album_id = album.id.to_string();
            let thumbnail = available
                .then(|| self.album_thumbnail(provider, &album, 0))
                .flatten();
            let confirmed = self.clone();
            post(self.ui.clone(), move |ui| {
                if confirmed.source_generation.load(Ordering::Acquire) != source_generation {
                    return;
                }
                if let Some((pixels, width, height)) = thumbnail {
                    for index in 0..ui.get_albums().row_count() {
                        if let Some(mut row) = ui.get_albums().row_data(index)
                            && row.id.as_str() == album_id
                        {
                            row.cover = Image::from_rgba8(
                                SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
                                    &pixels, width, height,
                                ),
                            );
                            ui.get_albums().set_row_data(index, row);
                            break;
                        }
                    }
                }
                ui.set_artwork_precache_status(
                    if ui.get_chinese() {
                        format!("正在补齐厂牌封面：{done}/{total}，找到 {found} 张")
                    } else {
                        format!("Preparing covers: {done}/{total}, found {found}")
                    }
                    .into(),
                );
            });
        }
        Ok((done, total, found))
    }
}
