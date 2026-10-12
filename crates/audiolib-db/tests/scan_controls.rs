// crates/audiolib-db/tests/scan_controls.rs
//! Cooperative controls keep the same session and lease while releasing database writers.
use audiolib_core::{DataProvider, Error, MetadataPatch, ScanProgress, ScanState, SearchRequest};
use audiolib_db::{ScanCancellation, ScanOptions, ScanService, SqliteProvider};
use std::{
    fs,
    sync::{Arc, mpsc},
    time::Duration,
};

fn wait_paused(receiver: &mpsc::Receiver<ScanProgress>, after: u64) -> ScanProgress {
    loop {
        let progress = receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("worker progress");
        if progress.paused && progress.processed >= after {
            return progress;
        }
    }
}

#[test]
fn pause_resume_and_cancel_preserve_session_lease_annotations_and_sources() {
    for cancel_while_paused in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("audio");
        fs::create_dir(&root).unwrap();
        // Tiny valid WAV files keep the regression independent of real assets.
        let bytes = [
            b"RIFF".as_slice(),
            &38u32.to_le_bytes(),
            b"WAVEfmt ",
            &16u32.to_le_bytes(),
            &1u16.to_le_bytes(),
            &1u16.to_le_bytes(),
            &48000u32.to_le_bytes(),
            &96000u32.to_le_bytes(),
            &2u16.to_le_bytes(),
            &16u16.to_le_bytes(),
            b"data",
            &2u32.to_le_bytes(),
            &[0, 0],
        ]
        .concat();
        for index in 0..24 {
            fs::write(root.join(format!("{index}.wav")), &bytes).unwrap();
        }
        let provider = Arc::new(SqliteProvider::open(&temp.path().join("library.db")).unwrap());
        let control = ScanCancellation::default();
        let worker_control = control.clone();
        let worker_provider = provider.clone();
        let worker_root = root.clone();
        let (updates, receiver) = mpsc::channel();
        let (done, completion) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut pauses = 0;
            let result = ScanService::new(worker_provider).scan(
                &worker_root,
                &ScanOptions {
                    incremental: false,
                    batch_size: 1,
                    ..Default::default()
                },
                &worker_control,
                |progress| {
                    if !progress.paused
                        && !progress.finalizing
                        && progress.processed >= (pauses + 1) * 2
                        && pauses < 2
                    {
                        assert!(worker_control.pause());
                        pauses += 1;
                    }
                    updates.send(progress).unwrap();
                },
            );
            done.send(result).unwrap();
        });
        let first = wait_paused(&receiver, 2);
        assert_eq!(first.processed, 2);
        let second = wait_paused(&receiver, 2);
        assert_eq!(
            first.processed, second.processed,
            "paused work must stop advancing"
        );
        assert!(completion.try_recv().is_err());
        assert_eq!(provider.stats().unwrap().audio_files, 2);
        // A paused scan retains process exclusion but permits ordinary annotations.
        assert!(matches!(
            ScanService::new(provider.clone()).scan(
                &root,
                &ScanOptions::default(),
                &ScanCancellation::default(),
                |_| {}
            ),
            Err(Error::Busy(_))
        ));
        let id = provider.search(&SearchRequest::default()).unwrap().results[0].id;
        provider
            .update_metadata(
                id,
                &MetadataPatch {
                    notes: Some("kept during pause".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        control.resume();
        let next = wait_paused(&receiver, 4);
        assert_eq!(next.session_id, first.session_id);
        assert_eq!(next.processed, 4);
        if cancel_while_paused {
            control.cancel();
        } else {
            control.resume();
        }
        let result = completion
            .recv_timeout(Duration::from_secs(10))
            .expect("paused worker must drain")
            .unwrap();
        worker.join().unwrap();
        assert_eq!(result.session_id, first.session_id);
        assert_eq!(
            result.state,
            if cancel_while_paused {
                ScanState::Cancelled
            } else {
                ScanState::Completed
            }
        );
        assert_eq!(result.processed, if cancel_while_paused { 4 } else { 24 });
        assert_eq!(provider.get_audio(id).unwrap().notes, "kept during pause");
        assert_eq!(provider.scan_history(10, 0).unwrap().len(), 1);
        assert_eq!(result.removed, 0);
        assert!(!control.pause(), "a terminal scan cannot be paused again");
        for index in 0..24 {
            assert_eq!(fs::read(root.join(format!("{index}.wav"))).unwrap(), bytes);
        }
        // Cancellation must release the lease so a subsequent scan can complete.
        assert_eq!(
            ScanService::new(provider.clone())
                .scan(
                    &root,
                    &ScanOptions::default(),
                    &ScanCancellation::default(),
                    |_| {}
                )
                .unwrap()
                .state,
            ScanState::Completed
        );
    }
}

#[test]
fn prepaused_scan_can_be_cancelled_without_resume_or_missing_reconciliation() {
    let temp = tempfile::tempdir().unwrap();
    let provider = Arc::new(SqliteProvider::open(&temp.path().join("library.db")).unwrap());
    let root = temp.path().join("empty");
    fs::create_dir(&root).unwrap();
    let control = ScanCancellation::default();
    assert!(control.pause());
    let worker_control = control.clone();
    let (updates, receiver) = mpsc::channel();
    let (done, completion) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        done.send(ScanService::new(provider).scan(
            &root,
            &ScanOptions {
                incremental: false,
                ..Default::default()
            },
            &worker_control,
            |progress| {
                updates.send(progress).unwrap();
            },
        ))
        .unwrap();
    });
    assert_eq!(wait_paused(&receiver, 0).processed, 0);
    control.cancel();
    control.resume(); // Continue cannot undo cancellation.
    let result = completion
        .recv_timeout(Duration::from_secs(10))
        .unwrap()
        .unwrap();
    worker.join().unwrap();
    assert_eq!(result.state, ScanState::Cancelled);
    assert_eq!(result.removed, 0);
}
