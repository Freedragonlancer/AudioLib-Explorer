// app/audiolib-desktop/src/controller_history.rs
//! Client-local playback history, separate from library databases and remote snapshots.
use super::*;
use audiolib_db::local_play_history::{LocalPlayHistory, LocalPlayRecord, local_library_id};

#[derive(Clone)]
struct Target {
    data_root: PathBuf,
    library_id: String,
    limit: u32,
}

impl Target {
    fn open(&self) -> audiolib_core::Result<LocalPlayHistory> {
        LocalPlayHistory::open(&self.data_root, &self.library_id, self.limit)
    }
}

/// The confirmation owns its original library and generation, never a later selection.
pub(super) struct PendingClear {
    target: Target,
    library_generation: u64,
    source_generation: u64,
}

fn current_record(
    provider: &SqliteProvider,
    play: &LocalPlayRecord,
) -> audiolib_core::Result<Option<AudioFile>> {
    if let Some(id) = play.audio_id {
        match provider.get_audio(id) {
            Ok(file) if same_path(Path::new(&file.file_path), Path::new(&play.file_path)) => {
                return Ok(Some(file));
            }
            // Restoring another valid library at this location can replace an
            // existing ID. The historical path must still identify the sound.
            Ok(_) | Err(audiolib_core::Error::NotFound(_)) => {}
            Err(error) => return Err(error),
        }
    }
    provider.find_audio_by_path(Path::new(&play.file_path))
}

fn played_at_local(value: &str) -> String {
    let instant = chrono::DateTime::parse_from_rfc3339(value)
        .map(|instant| instant.with_timezone(&chrono::Utc))
        .or_else(|_| {
            chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S")
                .map(|instant| instant.and_utc())
        });
    instant.map_or_else(
        |_| value.to_owned(),
        |instant| {
            instant
                .with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        },
    )
}

impl Controller {
    /// Resolve on a worker: local IDs use canonical library identity; server IDs
    /// come from the opened snapshot and never from its replaceable cache filename.
    fn history_target(&self) -> audiolib_core::Result<Target> {
        let config = self.config.lock().unwrap().clone();
        let library_id = if self.remote_read_only() {
            self.remote
                .lock()
                .unwrap()
                .state
                .as_ref()
                .map(|state| format!("server-{}", state.library_id))
                .ok_or_else(|| {
                    audiolib_core::Error::Validation(
                        "Sync a server library before recording playback history".into(),
                    )
                })?
        } else {
            local_library_id(self.provider().database_path())?
        };
        Ok(Target {
            data_root: config.data_root(),
            library_id,
            limit: config.play_history_limit,
        })
    }

    /// Called only by the status worker after a successful playback-session start.
    /// A failure is diagnostic; the caller consumes the event without affecting audio.
    #[cfg(test)]
    pub(super) fn record_local_history(&self, path: &Path) -> audiolib_core::Result<()> {
        self.record_history_for_generation(
            path,
            self.library_generation.load(Ordering::Acquire),
            self.source_generation.load(Ordering::Acquire),
        )
    }

    pub(super) fn record_history_best_effort(
        &self,
        path: &Path,
        library_generation: u64,
        source_generation: u64,
    ) {
        if let Err(error) =
            self.record_history_for_generation(path, library_generation, source_generation)
        {
            crate::diagnostics::record(crate::diagnostics::Event::PlaybackHistoryFailed);
            tracing::warn!(%error, "Could not save client-local playback history");
        }
    }

    pub(super) fn record_history_for_generation(
        &self,
        path: &Path,
        library_generation: u64,
        source_generation: u64,
    ) -> audiolib_core::Result<()> {
        let target = self.history_target()?;
        let file = if self.remote_read_only() {
            self.remote_wave_file(path)
                .or_else(|| {
                    self.remote_audio_id(path)
                        .and_then(|id| self.provider().get_audio(id).ok())
                })
                .or_else(|| self.provider().find_audio_by_path(path).ok().flatten())
        } else {
            self.provider().find_audio_by_path(path)?
        };
        let audio_id = file.as_ref().map(|file| file.id);
        let file_path = file
            .as_ref()
            .map_or_else(|| path.display().to_string(), |file| file.file_path.clone());
        let filename = file.as_ref().map_or_else(
            || {
                path.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            },
            |file| file.filename.clone(),
        );
        if self.library_generation.load(Ordering::Acquire) != library_generation
            || self.source_generation.load(Ordering::Acquire) != source_generation
        {
            return Err(audiolib_core::Error::Cancelled);
        }
        if target
            .open()?
            .record(audio_id, &file_path, &filename)?
            .is_some()
        {
            crate::diagnostics::record(crate::diagnostics::Event::PlaybackHistoryRecorded);
        }
        Ok(())
    }

    pub(super) fn request_history_page(self: &Arc<Self>, offset: usize) {
        if !self.available() {
            return;
        }
        let Some(ui) = self.ui.upgrade() else { return };
        let kind = ui.get_history_kind();
        ui.set_history_detail("".into());
        ui.set_history_rows(ModelRc::new(VecModel::<slint::SharedString>::default()));
        ui.set_history_missing(ModelRc::new(VecModel::<bool>::default()));
        let generation = self.history_generation.fetch_add(1, Ordering::AcqRel) + 1;
        let library_generation = self.library_generation.load(Ordering::Acquire);
        let source_generation = self.source_generation.load(Ordering::Acquire);
        let provider = self.provider();
        let remote = self.remote_read_only();
        let c = self.clone();
        std::thread::spawn(move || {
            let scans = if kind == 0 {
                provider.scan_history(HISTORY_SIZE, offset).map(Some)
            } else {
                Ok(None)
            };
            let plays = if kind == 1 {
                c.history_target()
                    .and_then(|target| target.open()?.page(HISTORY_SIZE, offset))
                    .map(Some)
            } else {
                Ok(None)
            };
            let missing: Vec<bool> = plays
                .as_ref()
                .ok()
                .and_then(Option::as_ref)
                .into_iter()
                .flatten()
                .map(|play| match current_record(&provider, play) {
                    Ok(Some(file)) => {
                        file.missing || (!remote && !Path::new(&file.file_path).is_file())
                    }
                    _ => remote || !Path::new(&play.file_path).is_file(),
                })
                .collect();
            let c2 = c.clone();
            post(c.ui.clone(), move |ui| {
                if c2.history_generation.load(Ordering::Acquire) != generation
                    || c2.library_generation.load(Ordering::Acquire) != library_generation
                    || c2.source_generation.load(Ordering::Acquire) != source_generation
                    || !c2.available()
                {
                    return;
                }
                let chinese = c2.chinese.load(Ordering::Acquire);
                let rows = match (scans, plays) {
                    (Ok(Some(scans)), _) => {
                        let rows = scans
                            .iter()
                            .map(|scan| {
                                slint::SharedString::from(format!(
                                    "{} · {} · {} · +{} / ~{} / −{} · {} {}\n{}",
                                    scan.start_time,
                                    if chinese {
                                        if scan.scan_type == "full" {
                                            "完整"
                                        } else {
                                            "增量"
                                        }
                                    } else {
                                        &scan.scan_type
                                    },
                                    scan_state_label(scan.state, chinese),
                                    scan.added,
                                    scan.updated,
                                    scan.removed,
                                    scan.errors,
                                    if chinese { "个错误" } else { "errors" },
                                    scan.scan_path
                                ))
                            })
                            .collect::<Vec<_>>();
                        *c2.scans.lock().unwrap() = scans;
                        rows
                    }
                    (_, Ok(Some(plays))) => {
                        let rows = plays
                            .iter()
                            .zip(&missing)
                            .map(|(play, missing)| {
                                slint::SharedString::from(format!(
                                    "{} · {}{}\n{}",
                                    played_at_local(&play.played_at),
                                    if *missing {
                                        if chinese { "[缺失] " } else { "[Missing] " }
                                    } else {
                                        ""
                                    },
                                    play.filename,
                                    play.file_path
                                ))
                            })
                            .collect::<Vec<_>>();
                        *c2.plays.lock().unwrap() = plays;
                        rows
                    }
                    (Err(error), _) | (_, Err(error)) => {
                        ui.set_history_detail(
                            if chinese {
                                format!("无法读取本机历史：{error}")
                            } else {
                                format!("Could not load local history: {error}")
                            }
                            .into(),
                        );
                        return;
                    }
                    _ => Vec::new(),
                };
                ui.set_history_next(rows.len() == HISTORY_SIZE);
                ui.set_history_previous(offset > 0);
                ui.set_history_page((offset / HISTORY_SIZE + 1).to_string().into());
                ui.set_history_missing(ModelRc::new(VecModel::from(missing)));
                ui.set_history_rows(ModelRc::new(VecModel::from(rows)));
            });
        });
    }

    pub(super) fn history_selected(self: &Arc<Self>, index: i32) {
        if !self.available() {
            return;
        }
        let Some(ui) = self.ui.upgrade() else { return };
        if ui.get_history_kind() == 0 {
            if let Some(scan) = self.scans.lock().unwrap().get(index as usize) {
                ui.set_history_detail(
                    scan_session_detail(scan, self.chinese.load(Ordering::Acquire)).into(),
                );
            }
            return;
        }
        let Some(play) = self.plays.lock().unwrap().get(index as usize).cloned() else {
            return;
        };
        let generation = self.library_generation.load(Ordering::Acquire);
        let source = self.source_generation.load(Ordering::Acquire);
        let provider = self.provider();
        let c = self.clone();
        std::thread::spawn(move || {
            let result = c.history_target().and_then(|target| {
                if target.library_id != play.library_id {
                    return Ok(None);
                }
                current_record(&provider, &play)
            });
            let c2 = c.clone();
            post(c.ui.clone(), move |ui| {
                if !c2.available()
                    || c2.library_generation.load(Ordering::Acquire) != generation
                    || c2.source_generation.load(Ordering::Acquire) != source
                {
                    return;
                }
                match result {
                    Ok(Some(file)) => {
                        ui.set_library_visible(false);
                        ui.set_inspector_visible(true);
                        c2.select_id(file.id, false);
                    }
                    _ => ui.set_history_detail(
                        if c2.chinese.load(Ordering::Acquire) {
                            format!("当前素材库中找不到这条记录：{}", play.file_path)
                        } else {
                            format!(
                                "This sound is unavailable in the current library: {}",
                                play.file_path
                            )
                        }
                        .into(),
                    ),
                }
            });
        });
    }

    pub(super) fn propose_history_clear(self: &Arc<Self>) {
        if !self.available() {
            return;
        }
        let library_generation = self.library_generation.load(Ordering::Acquire);
        let source_generation = self.source_generation.load(Ordering::Acquire);
        let c = self.clone();
        std::thread::spawn(move || {
            let target = c.history_target();
            let c2 = c.clone();
            post(c.ui.clone(), move |ui| {
                if !c2.available()
                    || c2.library_generation.load(Ordering::Acquire) != library_generation
                    || c2.source_generation.load(Ordering::Acquire) != source_generation
                    || ui.get_history_kind() != 1
                    || ui.get_confirmation_visible()
                {
                    return;
                }
                match target {
                    Ok(target) => {
                        c2.pending_operation.lock().unwrap().take();
                        *c2.pending_history_clear.lock().unwrap() = Some(PendingClear {
                            target,
                            library_generation,
                            source_generation,
                        });
                        ui.set_confirmation_text(if c2.chinese.load(Ordering::Acquire) {
                            "清除当前素材库在本机的全部播放历史？此操作不可撤销。".into()
                        } else { "Clear all playback history for this library on this device? This cannot be undone.".into() });
                        ui.set_confirmation_visible(true);
                    }
                    Err(error) => ui.set_history_detail(error.to_string().into()),
                }
            });
        });
    }

    pub(super) fn confirm_history_clear(self: &Arc<Self>, accepted: bool) -> bool {
        let Some(pending) = self.pending_history_clear.lock().unwrap().take() else {
            return false;
        };
        if !accepted
            || !self.available()
            || self.library_generation.load(Ordering::Acquire) != pending.library_generation
            || self.source_generation.load(Ordering::Acquire) != pending.source_generation
        {
            return true;
        }
        let c = self.clone();
        std::thread::spawn(move || {
            let result = c.history_target().and_then(|current| {
                if current.library_id != pending.target.library_id
                    || c.library_generation.load(Ordering::Acquire) != pending.library_generation
                    || c.source_generation.load(Ordering::Acquire) != pending.source_generation
                {
                    return Err(audiolib_core::Error::Cancelled);
                }
                pending.target.open()?.clear()
            });
            let c2 = c.clone();
            post(c.ui.clone(), move |ui| {
                if !c2.available()
                    || c2.library_generation.load(Ordering::Acquire) != pending.library_generation
                    || c2.source_generation.load(Ordering::Acquire) != pending.source_generation
                {
                    return;
                }
                match result {
                    Ok(_) => c2.load_history(0),
                    Err(error) => ui.set_history_detail(error.to_string().into()),
                }
            });
        });
        true
    }
}
