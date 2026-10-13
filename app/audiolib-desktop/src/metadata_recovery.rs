// app/audiolib-desktop/src/metadata_recovery.rs
//! Preserve annotations from a replaced server library before handoff or close.
use super::*;
use std::io::Write;

const FORMAT: &str = "audiolib-server-metadata-recovery-v1";

fn recovery_directory(root: &Path) -> PathBuf {
    // Snapshot/cache pruning must never remove user annotation recovery files.
    root.join("recovery").join("server-metadata")
}

impl Controller {
    /// A confirmed replacement cannot accept the old record IDs. Drain already
    /// requested work without retrying those IDs; archive its owned drafts later.
    pub(in crate::controller) fn enqueue_transition_metadata(&self) -> Result<u64, String> {
        if self.remote_recovery_snapshot().is_some() {
            Ok(self.metadata.lock().unwrap().sequence)
        } else {
            self.metadata.lock().unwrap().enqueue_all()
        }
    }

    /// Run off the UI thread while the source/close gate prevents further edits.
    /// Ordinary save failures still block the transition. Only a confirmed server
    /// identity replacement permits durable local recovery instead of a server save.
    pub(in crate::controller) fn wait_transition_metadata(
        &self,
        sequence: u64,
    ) -> Result<(), String> {
        self.wait_metadata(sequence, false)?;
        let Some(snapshot) = self.remote_recovery_snapshot() else {
            return self.wait_metadata(sequence, true).map(|_| ());
        };
        let provider = self.provider();
        let generation = self.library_generation.load(Ordering::Acquire);
        let (revision, completed, drafts, actions, failures) = {
            let state = self.metadata.lock().unwrap();
            if !state.jobs.is_empty() || state.in_flight.is_some() {
                return Err("Metadata recovery requires the write queue to drain".into());
            }
            (
                state.revision,
                state.completed,
                state.drafts.clone(),
                state.failed_actions.clone(),
                state.failures.keys().copied().collect::<HashSet<_>>(),
            )
        };
        let mut ids = drafts
            .keys()
            .chain(actions.keys())
            .chain(failures.iter())
            .copied()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        ids.sort_unstable();
        if ids.is_empty() {
            return Ok(());
        }
        let records = ids
            .iter()
            .map(|id| {
                // These reads belong to the still-open old snapshot, never the server.
                let file = provider.get_audio(*id).map_err(|error| {
                    format!("Could not read old-library record {id} for Metadata recovery: {error}. The original drafts were retained.")
                })?;
                let patch = drafts.get(id).map(|draft| draft.patch.clone()).or_else(|| {
                    actions.get(id).and_then(|action| {
                        quick_metadata_patch(*action, file.favorite)
                    })
                });
                Ok::<_, String>(serde_json::json!({
                    "record_id": id,
                    "file_path": file.file_path,
                    "filename": file.filename,
                    "patch": patch,
                    "draft_revision": drafts.get(id).map(|draft| draft.revision),
                    "quick_action": actions.get(id),
                    "had_failed_write": failures.contains(id),
                    "snapshot_metadata": {
                        "tags": file.tags, "notes": file.notes,
                        "favorite": file.favorite, "color": file.color,
                    },
                }))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let payload = serde_json::json!({
            "format": FORMAT,
            "reason": "server_library_replaced",
            "library_id": snapshot.library_id,
            "snapshot_file": snapshot.database_file,
            "snapshot_revision": snapshot.revision,
            "provider_database": provider.database_path(),
            "metadata_revision": revision,
            "completed_sequence": completed,
            "records": records,
            "instructions": "Review these annotations against their original library and WAV paths. Never apply the record IDs to a replacement library. This file is not automatically uploaded or replayed.",
        });
        if !Arc::ptr_eq(&provider, &self.provider())
            || self.library_generation.load(Ordering::Acquire) != generation
            || !self.remote_snapshot_current(&snapshot)
            || self.remote_recovery_snapshot().is_none()
        {
            return Err("The displayed server snapshot changed during Metadata recovery".into());
        }
        // A source change drains twice (connection, then provider). Reuse only a
        // fully readable, identical durable artifact; lost files are recreated.
        let previous = self.metadata_recovery_notice.lock().unwrap().clone();
        let path = previous
            .filter(|path| {
                std::fs::read(path)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                    .is_some_and(|saved| saved == payload)
            })
            .map(Ok)
            .unwrap_or_else(|| {
                write_recovery(&self.config.lock().unwrap().data_root(), &payload)
            })?;
        tracing::warn!(
            library_id = %snapshot.library_id,
            records = ids.len(),
            path = %path.display(),
            "Unsaved server Metadata preserved in a local recovery file"
        );
        *self.metadata_recovery_notice.lock().unwrap() = Some(path);
        // Keep drafts/failures in memory until the actual provider handoff succeeds.
        // A rejected target open or failed close must still show the original edits.
        Ok(())
    }

    /// Publish after operation status updates so the recovery location stays visible.
    pub(in crate::controller) fn publish_metadata_recovery_notice(&self, ui: &AppWindow) {
        let Some(path) = self.metadata_recovery_notice.lock().unwrap().clone() else {
            return;
        };
        let message = if self.chinese.load(Ordering::Acquire) {
            format!(
                "旧服务器素材库的未保存 Metadata 已保存在本机恢复文件中，请按原素材库身份核对后使用：{}",
                path.display()
            )
        } else {
            format!(
                "Unsaved Metadata from the previous server library is preserved locally. Review it against the original library before applying it: {}",
                path.display()
            )
        };
        let append = |current: slint::SharedString| {
            if current.is_empty() {
                message.clone()
            } else if current.contains(&message) {
                current.to_string()
            } else {
                format!("{current}\n{message}")
            }
        };
        ui.set_status(append(ui.get_status()).into());
        ui.set_ds_message(append(ui.get_ds_message()).into());
        ui.set_management_report(append(ui.get_management_report()).into());
    }

    /// Surface durable recovery again after restart; no annotations are reapplied.
    pub(in crate::controller) fn find_metadata_recovery(self: &Arc<Self>) {
        let directory = recovery_directory(&self.config.lock().unwrap().data_root());
        if !directory.is_dir() {
            return;
        }
        let c = self.clone();
        std::thread::spawn(move || {
            let Some(path) = std::fs::read_dir(directory).ok().and_then(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.path())
                    .filter(|path| {
                        path.extension()
                            .is_some_and(|extension| extension == "json")
                    })
                    .max()
            }) else {
                return;
            };
            let valid = std::fs::read(&path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                .is_some_and(|value| {
                    value["format"] == FORMAT
                        && value["records"]
                            .as_array()
                            .is_some_and(|records| !records.is_empty())
                });
            if !valid {
                return;
            }
            self::post_existing_recovery(c, path);
        });
    }
}

fn post_existing_recovery(controller: Arc<Controller>, path: PathBuf) {
    let c = controller.clone();
    post(controller.ui.clone(), move |ui| {
        if c.shutdown.load(Ordering::Acquire) {
            return;
        }
        let mut notice = c.metadata_recovery_notice.lock().unwrap();
        if notice.is_none() {
            *notice = Some(path);
        }
        drop(notice);
        c.publish_metadata_recovery_notice(&ui);
    });
}

fn write_recovery(root: &Path, payload: &serde_json::Value) -> Result<PathBuf, String> {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let directory = recovery_directory(root);
    let created = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    let stem = format!(
        "{created}-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let temporary = directory.join(format!("{stem}.part"));
    let path = directory.join(format!("{stem}.json"));
    let result = (|| -> std::io::Result<()> {
        std::fs::create_dir_all(&directory)?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(&serde_json::to_vec_pretty(payload)?)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, &path)?;
        // Reopen the published artifact and flush its handle before admitting close.
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)?
            .sync_all()
    })();
    if let Err(error) = result {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!(
            "Could not preserve unsaved Metadata in {}: {error}. The original drafts were retained.",
            directory.display()
        ));
    }
    Ok(path)
}

#[cfg(test)]
#[path = "metadata_recovery_tests.rs"]
mod tests;
