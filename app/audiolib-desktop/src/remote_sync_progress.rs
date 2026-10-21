// app/audiolib-desktop/src/remote_sync_progress.rs
//! Coalesced snapshot progress: wire downloads and streamed decompression stay visible together.
use audiolib_db::SnapshotUpgradeProgress;
use audiolib_remote::{CachePreparation, SyncProgress};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Default)]
struct Bytes {
    received: u64,
    total: Option<u64>,
}

impl Bytes {
    fn fraction(self) -> f32 {
        match self.total {
            Some(total) if total > 0 => {
                (self.received as f64 / total as f64).clamp(0.0, 1.0) as f32
            }
            _ => -1.0,
        }
    }

    fn label(self, chinese: bool, decompressing: bool) -> String {
        let stage = match (chinese, decompressing) {
            (true, false) => "正在下载快照",
            (true, true) => "正在解压快照",
            (false, false) => "Downloading snapshot",
            (false, true) => "Decompressing snapshot",
        };
        format!("{stage} {}", self.amount())
    }

    fn amount(self) -> String {
        let mib = self.received as f64 / 1_048_576.0;
        match self.total {
            Some(total) if total > 0 => {
                format!("{mib:.1} / {:.1} MiB", total as f64 / 1_048_576.0)
            }
            _ => format!("{mib:.1} MiB"),
        }
    }
}

pub(super) struct ProgressDisplay {
    pub text: String,
    pub fraction: f32,
    pub decompression_text: String,
    pub decompression_fraction: f32,
}

impl ProgressDisplay {
    fn stage(text: String, fraction: f32) -> Self {
        Self {
            text,
            fraction,
            decompression_text: String::new(),
            decompression_fraction: -1.0,
        }
    }
}

#[derive(Default)]
pub(super) struct ProgressReporter {
    download: Bytes,
    decompression: Option<Bytes>,
    replaying_from: Option<u64>,
    last_post: Option<Instant>,
    last_progress: Option<SyncProgress>,
}

impl ProgressReporter {
    pub fn report(
        &mut self,
        progress: SyncProgress,
        chinese: bool,
        now: Instant,
    ) -> Option<ProgressDisplay> {
        // Update both counters before throttling. Alternating events belong to one
        // transfer and must neither flood the event loop nor discard the other counter.
        let mut transfer_changed = false;
        let mut completed = false;
        match progress {
            SyncProgress::Replaying { received, total } => {
                self.download = Bytes { received, total };
                self.decompression = None;
                self.replaying_from = Some(received);
                transfer_changed = true;
            }
            SyncProgress::Downloading { received, total } => {
                if self.replaying_from.is_some_and(|offset| received > offset) {
                    self.replaying_from = None;
                    transfer_changed = true;
                }
                completed = total.is_some_and(|total| {
                    total > 0 && self.download.received < total && received >= total
                });
                transfer_changed |= self.download.total != total
                    || received < self.download.received
                    || (received == 0 && self.decompression.is_some());
                if received == 0 {
                    self.decompression = None;
                }
                self.download = Bytes { received, total };
            }
            SyncProgress::Decompressing { written, total } => {
                transfer_changed = self
                    .decompression
                    .is_none_or(|previous| previous.total != total);
                completed = total.is_some_and(|total| {
                    total > 0
                        && self
                            .decompression
                            .is_none_or(|previous| previous.received < total)
                        && written >= total
                });
                self.decompression = Some(Bytes {
                    received: written,
                    total,
                });
            }
            SyncProgress::Retrying { .. } => self.decompression = None,
            _ => {
                self.decompression = None;
                self.replaying_from = None;
            }
        }
        if let Some((phase, done, total)) = counted(progress) {
            if let Some((previous_phase, previous_done, previous_total)) =
                self.last_progress.and_then(counted)
            {
                transfer_changed =
                    phase != previous_phase || total != previous_total || done < previous_done;
                completed = total > 0 && previous_done < total && done >= total;
            } else {
                transfer_changed = true;
            }
        }
        let repeating_count = match (self.last_progress, progress) {
            (Some(previous), current) if transfer(previous) && transfer(current) => true,
            (Some(previous), current) => {
                matches!((counted(previous), counted(current)), (Some((left, _, _)), Some((right, _, _))) if left == right)
            }
            _ => false,
        };
        if !transfer_changed
            && !completed
            && repeating_count
            && self
                .last_post
                .is_some_and(|last| now.duration_since(last) < Duration::from_millis(200))
        {
            return None;
        }
        self.last_post = Some(now);
        self.last_progress = Some(progress);
        let stage = |zh: &str, en: &str| if chinese { zh } else { en }.to_owned();
        Some(match progress {
            SyncProgress::Checking => ProgressDisplay::stage(
                stage("正在检查服务器版本…", "Checking server revision…"),
                -1.0,
            ),
            SyncProgress::Downloading {
                received: 0,
                total: None,
            } => ProgressDisplay::stage(
                stage(
                    "服务器正在准备快照…",
                    "The server is preparing the snapshot…",
                ),
                -1.0,
            ),
            SyncProgress::Downloading { .. }
            | SyncProgress::Decompressing { .. }
            | SyncProgress::Replaying { .. } => ProgressDisplay {
                text: if self.replaying_from.is_some() {
                    format!(
                        "{} · {} {}",
                        stage("正在重放已下载快照", "Replaying saved snapshot"),
                        stage("已保留", "retained"),
                        self.download.amount(),
                    )
                } else {
                    self.download.label(chinese, false)
                },
                fraction: self.download.fraction(),
                decompression_text: self
                    .decompression
                    .map(|bytes| bytes.label(chinese, true))
                    .unwrap_or_default(),
                decompression_fraction: self.decompression.map_or(-1.0, Bytes::fraction),
            },
            SyncProgress::Retrying {
                attempt,
                wait_seconds,
            } => ProgressDisplay::stage(
                if chinese {
                    format!(
                        "连接中断，{wait_seconds} 秒后重试（第 {attempt} 次） · {}",
                        self.download.label(true, false)
                    )
                } else {
                    format!(
                        "Connection interrupted; retry {attempt} in {wait_seconds} s · {}",
                        self.download.label(false, false)
                    )
                },
                self.download.fraction(),
            ),
            SyncProgress::Restarting => ProgressDisplay::stage(
                stage("正在重新下载快照…", "Restarting snapshot download…"),
                -1.0,
            ),
            SyncProgress::Writing => {
                ProgressDisplay::stage(stage("写入磁盘中…", "Writing to disk…"), -1.0)
            }
            SyncProgress::Validating => ProgressDisplay::stage(
                stage("正在校验快照完整性…", "Validating snapshot integrity…"),
                -1.0,
            ),
            SyncProgress::Saving => {
                ProgressDisplay::stage(stage("正在更新本地缓存…", "Updating local cache…"), -1.0)
            }
            SyncProgress::Preparing(preparation) => {
                let preparation = crate::startup::StartupProgress::Remote(preparation);
                let fraction = preparation.fraction();
                let text = if fraction >= 0.0 {
                    format!("{} · {:.0}%", preparation.label(chinese), fraction * 100.0)
                } else {
                    preparation.label(chinese)
                };
                ProgressDisplay::stage(text, fraction)
            }
        })
    }
}

// Distinct identities keep SQLite-page copying separate from migration row copying.
fn counted(progress: SyncProgress) -> Option<(u8, u64, u64)> {
    match progress {
        SyncProgress::Preparing(CachePreparation::Copying { completed, total }) => {
            Some((0, completed, total))
        }
        SyncProgress::Preparing(CachePreparation::Tokenizing { completed, total }) => {
            Some((1, completed, total))
        }
        SyncProgress::Preparing(CachePreparation::Migrating(
            SnapshotUpgradeProgress::Copying { completed, total },
        )) => Some((2, completed, total)),
        SyncProgress::Preparing(CachePreparation::Migrating(
            SnapshotUpgradeProgress::Indexing { completed, total },
        )) => Some((3, completed, total)),
        SyncProgress::Preparing(CachePreparation::Migrating(
            SnapshotUpgradeProgress::Tokenizing { completed, total },
        )) => Some((4, completed, total)),
        _ => None,
    }
}

fn transfer(progress: SyncProgress) -> bool {
    matches!(
        progress,
        SyncProgress::Downloading { .. } | SyncProgress::Decompressing { .. }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_prefix_replay_resets_output_and_keeps_download_offset_until_new_bytes() {
        let now = Instant::now();
        let mut reporter = ProgressReporter::default();
        reporter.report(
            SyncProgress::Downloading {
                received: 600,
                total: Some(1000),
            },
            false,
            now,
        );
        reporter.report(
            SyncProgress::Decompressing {
                written: 4000,
                total: Some(8000),
            },
            false,
            now,
        );
        let replay = reporter
            .report(
                SyncProgress::Replaying {
                    received: 600,
                    total: Some(1000),
                },
                false,
                now,
            )
            .unwrap();
        assert!((replay.fraction - 0.6).abs() < 0.001);
        assert!(replay.text.contains("Replaying"));
        assert!(replay.decompression_text.is_empty());
        let output = reporter
            .report(
                SyncProgress::Decompressing {
                    written: 100,
                    total: Some(8000),
                },
                false,
                now,
            )
            .unwrap();
        assert!((output.fraction - 0.6).abs() < 0.001);
        assert!((output.decompression_fraction - 0.0125).abs() < 0.001);
        assert!(output.text.contains("Replaying"));
        let resumed = reporter
            .report(
                SyncProgress::Downloading {
                    received: 700,
                    total: Some(1000),
                },
                false,
                now,
            )
            .unwrap();
        assert!((resumed.fraction - 0.7).abs() < 0.001);
        assert!(!resumed.text.contains("Replaying"));
    }

    #[test]
    fn retry_wait_preserves_download_fraction_and_discards_abandoned_output() {
        let now = Instant::now();
        let mut reporter = ProgressReporter::default();
        reporter.report(
            SyncProgress::Downloading {
                received: 512,
                total: Some(1024),
            },
            true,
            now,
        );
        reporter.report(
            SyncProgress::Decompressing {
                written: 2048,
                total: Some(4096),
            },
            true,
            now,
        );
        let waiting = reporter
            .report(
                SyncProgress::Retrying {
                    attempt: 2,
                    wait_seconds: 5,
                },
                true,
                now,
            )
            .unwrap();
        assert_eq!(waiting.fraction, 0.5);
        assert!(waiting.text.contains("5 秒后重试"));
        assert!(waiting.decompression_text.is_empty());
        let restarted = reporter
            .report(SyncProgress::Restarting, true, now)
            .unwrap();
        assert_eq!(restarted.fraction, -1.0);
        assert!(restarted.decompression_text.is_empty());
    }

    #[test]
    fn migration_rows_flush_at_phase_changes_completion_and_resume_without_a_transfer_percentage() {
        let now = Instant::now();
        let mut reporter = ProgressReporter::default();
        for indexing in [false, true] {
            let event = |completed| {
                SyncProgress::Preparing(CachePreparation::Migrating(if indexing {
                    SnapshotUpgradeProgress::Indexing {
                        completed,
                        total: 10003,
                    }
                } else {
                    SnapshotUpgradeProgress::Copying {
                        completed,
                        total: 10003,
                    }
                }))
            };
            let display = reporter.report(event(5000), false, now).unwrap();
            assert!(display.text.contains("5000/10003 records saved"));
            assert!(display.fraction < 0.0);
            assert!(display.decompression_text.is_empty());
            assert!(reporter.report(event(7500), false, now).is_none());
            let display = reporter.report(event(10003), true, now).unwrap();
            assert!(display.text.contains("已保存 10003/10003 条"));
            assert!(display.fraction < 0.0);
            assert!(reporter.report(event(10003), true, now).is_none());
            // A resumed job can supply a lower committed cursor without being hidden.
            assert!(reporter.report(event(5000), true, now).is_some());
        }
        let display = reporter
            .report(
                SyncProgress::Preparing(CachePreparation::Migrating(
                    SnapshotUpgradeProgress::Publishing,
                )),
                false,
                now,
            )
            .unwrap();
        assert_eq!(display.text, "Applying the validated library upgrade");
        assert!(display.fraction < 0.0);
    }

    #[test]
    fn known_download_total_is_published_without_waiting_for_the_body() {
        let now = Instant::now();
        let mut reporter = ProgressReporter::default();
        reporter
            .report(
                SyncProgress::Downloading {
                    received: 0,
                    total: None,
                },
                false,
                now,
            )
            .unwrap();
        let display = reporter
            .report(
                SyncProgress::Downloading {
                    received: 0,
                    total: Some(1_048_576),
                },
                false,
                now,
            )
            .unwrap();
        assert_eq!(display.text, "Downloading snapshot 0.0 / 1.0 MiB");
        assert_eq!(display.fraction, 0.0);
    }

    #[test]
    fn crossing_a_declared_total_bypasses_throttling_only_once() {
        let now = Instant::now();
        let mut reporter = ProgressReporter::default();
        for progress in [
            SyncProgress::Downloading {
                received: 0,
                total: Some(1),
            },
            SyncProgress::Decompressing {
                written: 0,
                total: Some(1),
            },
            SyncProgress::Downloading {
                received: 2,
                total: Some(1),
            },
            SyncProgress::Decompressing {
                written: 262_144,
                total: Some(1),
            },
        ] {
            reporter.report(progress, false, now).unwrap();
        }
        for count in 2..10 {
            assert!(
                reporter
                    .report(
                        SyncProgress::Downloading {
                            received: count + 1,
                            total: Some(1),
                        },
                        false,
                        now,
                    )
                    .is_none()
            );
            assert!(
                reporter
                    .report(
                        SyncProgress::Decompressing {
                            written: count * 262_144,
                            total: Some(1),
                        },
                        false,
                        now,
                    )
                    .is_none()
            );
        }
        let display = reporter
            .report(
                SyncProgress::Downloading {
                    received: 11,
                    total: Some(1),
                },
                false,
                now + Duration::from_millis(200),
            )
            .unwrap();
        assert_eq!(display.fraction, 1.0);
        assert_eq!(display.decompression_fraction, 1.0);
        assert_eq!(
            display.decompression_text,
            "Decompressing snapshot 2.2 / 0.0 MiB"
        );
    }

    #[test]
    fn compressed_transfer_keeps_distinct_totals_and_coalesces_alternating_events() {
        let now = Instant::now();
        let mut reporter = ProgressReporter::default();
        reporter
            .report(
                SyncProgress::Downloading {
                    received: 0,
                    total: Some(1_500 * 1_048_576),
                },
                true,
                now,
            )
            .unwrap();
        // The decompression row appears immediately, even inside the throttle interval.
        let display = reporter
            .report(
                SyncProgress::Decompressing {
                    written: 0,
                    total: Some(19_495_989_248),
                },
                true,
                now,
            )
            .unwrap();
        assert_eq!(display.text, "正在下载快照 0.0 / 1500.0 MiB");
        assert_eq!(display.decompression_text, "正在解压快照 0.0 / 18592.8 MiB");
        assert!(
            reporter
                .report(
                    SyncProgress::Downloading {
                        received: 750 * 1_048_576,
                        total: Some(1_500 * 1_048_576)
                    },
                    true,
                    now
                )
                .is_none()
        );
        assert!(
            reporter
                .report(
                    SyncProgress::Decompressing {
                        written: 1_048_576_000,
                        total: Some(19_495_989_248)
                    },
                    true,
                    now
                )
                .is_none()
        );
        let display = reporter
            .report(
                SyncProgress::Downloading {
                    received: 750 * 1_048_576,
                    total: Some(1_500 * 1_048_576),
                },
                true,
                now + Duration::from_millis(200),
            )
            .unwrap();
        assert_eq!(display.text, "正在下载快照 750.0 / 1500.0 MiB");
        assert_eq!(
            display.decompression_text,
            "正在解压快照 1000.0 / 18592.8 MiB"
        );
        assert_eq!(display.fraction, 0.5);
        assert!((display.decompression_fraction - 0.05378).abs() < 0.0001);
        let display = reporter
            .report(
                SyncProgress::Downloading {
                    received: 1_500 * 1_048_576,
                    total: Some(1_500 * 1_048_576),
                },
                true,
                now + Duration::from_millis(201),
            )
            .unwrap();
        assert_eq!(display.fraction, 1.0);
        assert_eq!(
            display.decompression_text,
            "正在解压快照 1000.0 / 18592.8 MiB"
        );
        let display = reporter
            .report(
                SyncProgress::Decompressing {
                    written: 19_495_989_248,
                    total: Some(19_495_989_248),
                },
                true,
                now + Duration::from_millis(202),
            )
            .unwrap();
        assert_eq!(display.fraction, 1.0);
        assert_eq!(display.decompression_fraction, 1.0);
        let display = reporter
            .report(
                SyncProgress::Validating,
                true,
                now + Duration::from_millis(203),
            )
            .unwrap();
        assert_eq!(display.text, "正在校验快照完整性…");
        assert!(display.decompression_text.is_empty());
    }

    #[test]
    fn unknown_lengths_plain_responses_and_retries_never_invent_decompression() {
        let now = Instant::now();
        let mut reporter = ProgressReporter::default();
        let display = reporter
            .report(
                SyncProgress::Downloading {
                    received: 1_048_576,
                    total: None,
                },
                false,
                now,
            )
            .unwrap();
        assert_eq!(display.text, "Downloading snapshot 1.0 MiB");
        assert_eq!(display.fraction, -1.0);
        assert!(display.decompression_text.is_empty());
        let display = reporter
            .report(
                SyncProgress::Decompressing {
                    written: 3_145_728,
                    total: None,
                },
                false,
                now,
            )
            .unwrap();
        assert_eq!(display.text, "Downloading snapshot 1.0 MiB");
        assert_eq!(display.decompression_text, "Decompressing snapshot 3.0 MiB");
        assert_eq!(display.decompression_fraction, -1.0);
        let display = reporter
            .report(
                SyncProgress::Downloading {
                    received: 0,
                    total: None,
                },
                false,
                now + Duration::from_secs(1),
            )
            .unwrap();
        assert_eq!(display.text, "The server is preparing the snapshot…");
        assert!(display.decompression_text.is_empty());
        let display = reporter
            .report(
                SyncProgress::Downloading {
                    received: 2_097_152,
                    total: Some(1_048_576),
                },
                false,
                now + Duration::from_secs(2),
            )
            .unwrap();
        assert_eq!(display.fraction, 1.0);
        assert!(display.decompression_text.is_empty());
    }
}
