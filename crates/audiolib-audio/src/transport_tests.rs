// crates/audiolib-audio/src/transport_tests.rs
//! Transport regressions with a retained device FIFO beyond the application PCM ring.

use super::*;
use std::{collections::VecDeque, path::Path};

const RATE: u32 = 48_000;
const DEVICE_FRAMES: usize = 64;
const DEVICE_BUFFERS: usize = 2;

/// A stopped device retains submitted PCM. Only running callbacks can replace it;
/// clearing the application's ring cannot change either of these device buffers.
struct RetainedDevice {
    callback: RenderCallback,
    queued: VecDeque<Vec<f32>>,
    submitted: Vec<f32>,
}

impl RetainedDevice {
    fn new(realtime: Arc<Realtime>) -> Self {
        let mut device = Self {
            callback: RenderCallback {
                realtime,
                channels: 2,
                pair: 0,
                ramp_step: 1.0,
                gain: 0.8,
                envelope: SeekEnvelope::default(),
            },
            queued: VecDeque::new(),
            submitted: Vec::new(),
        };
        for _ in 0..DEVICE_BUFFERS {
            device.submit();
        }
        assert!(device.queued.iter().all(|block| block[0] > 0.0));
        device
    }

    fn submit(&mut self) -> Vec<f32> {
        let mut block = vec![9.0; DEVICE_FRAMES * 2];
        self.callback.render(&mut block);
        self.submitted.extend_from_slice(&block);
        let played = if self.queued.len() == DEVICE_BUFFERS {
            self.queued.pop_front().unwrap()
        } else {
            vec![0.0; DEVICE_FRAMES * 2]
        };
        self.queued.push_back(block);
        played
    }

    /// Run real control commands while callbacks advance a separate device queue.
    /// On acknowledgment, freeze callbacks as a hardware Pause does. A command
    /// which merely clears the application ring leaves old PCM in `queued`.
    fn command(&mut self, worker: &mut Worker, command: AudioCommand) {
        self.command_result(worker, command).unwrap();
    }

    fn command_result(&mut self, worker: &mut Worker, command: AudioCommand) -> Result<()> {
        thread::scope(|scope| {
            let (sender, receiver) = mpsc::sync_channel(1);
            scope.spawn(move || {
                sender.send(worker.execute_command(command)).unwrap();
            });
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            loop {
                match receiver.recv_timeout(Duration::from_millis(1)) {
                    Ok(result) => break result,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        assert!(std::time::Instant::now() < deadline, "transport timed out");
                        self.submit();
                    }
                    Err(error) => panic!("transport thread disconnected: {error}"),
                }
            }
        })
    }

    fn assert_retained_silence(&self) {
        assert_eq!(self.queued.len(), DEVICE_BUFFERS);
        assert!(
            self.queued.iter().flatten().all(|sample| *sample == 0.0),
            "paused device retained nonzero audio from the previous position"
        );
    }

    fn assert_resumes_from_zero(&mut self, negative: bool) {
        let first_new = self.submitted.len();
        let mut played = Vec::new();
        // Cover the full envelope and both retained device buffers at 48 kHz.
        for _ in 0..32 {
            played.extend(self.submit());
        }
        let new_pcm = &self.submitted[first_new..];
        assert_eq!(&new_pcm[..2], &[0.0, 0.0]);
        assert_eq!(
            &played[..DEVICE_BUFFERS * DEVICE_FRAMES * 2],
            vec![0.0; DEVICE_BUFFERS * DEVICE_FRAMES * 2]
        );
        let audible: Vec<_> = played
            .chunks_exact(2)
            .filter(|frame| frame[0] != 0.0)
            .collect();
        assert!(!audible.is_empty(), "resumed output stayed muted");
        for frame in audible {
            assert_eq!(frame[0] < 0.0, negative, "old-source PCM escaped on resume");
            assert_eq!(frame[1] > 0.0, negative, "stereo source identity changed");
        }
    }
}

fn write_source(path: &Path, replacement: bool) {
    let mut bytes = Vec::with_capacity(RATE as usize * 4);
    for frame in 0..RATE {
        let pcm = if replacement || frame >= RATE / 2 {
            [-20_480_i16, 8_192_i16]
        } else {
            [24_576_i16, -16_384_i16]
        };
        for sample in pcm {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
    }
    crate::tests::write_wav(path, RATE, 2, 16, 1, &bytes, false, false);
}

fn playing_worker(path: &Path) -> Worker {
    write_source(path, false);
    let mut output = Output {
        stream: None,
        callback_available: true,
        asio: false,
        realtime: Arc::new(Realtime {
            ring: Ring::new(16_384),
            running: AtomicBool::new(true),
            gain: AtomicU32::new(0.8_f32.to_bits()),
            consumed: AtomicU64::new(0),
            underruns: AtomicU64::new(0),
            failed: AtomicBool::new(false),
            active: AtomicBool::new(false),
            // Deliberately conservative queued latency gives the simulated
            // callback thread room for Windows scheduler jitter before Pause.
            callback_latency_nanos: AtomicU64::new(100_000_000),
            source_finished: AtomicBool::new(false),
            transition: SourceTransition {
                faded_out: AtomicU64::new(u64::MAX),
                ..Default::default()
            },
        }),
        decoder: AuditionDecoder::new(
            Decoder::open(path).unwrap(),
            RATE,
            0,
            0,
            AuditionSettings::default(),
        )
        .unwrap(),
        sample_rate: RATE,
        volume: 0.8,
        buffering: Buffering::Off,
        pending: Vec::new(),
        pending_offset: 0,
        eof: false,
        pending_last: false,
        start_position: 0.0,
        drain_deadline: None,
    };
    output.fill().unwrap();
    let mut worker = Worker::new(
        PlaybackConfig::default(),
        Arc::new(Mutex::new(PlaybackStatus {
            generation: 11,
            play_session_id: 7,
            state: PlaybackState::Playing,
            file_path: Some(path.to_owned()),
            duration: 1.0,
            sample_rate: RATE,
            channels: 2,
            volume: 0.8,
            ..Default::default()
        })),
        Arc::new(Mutex::new(Vec::new())),
    );
    worker.output = Some(output);
    worker
}

#[test]
fn pause_then_resume_or_seek_drains_retained_device_pcm_before_new_audio() {
    for seek in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("paused-retained-device.wav");
        let mut worker = playing_worker(&path);
        let realtime = worker.output.as_ref().unwrap().realtime.clone();
        let mut device = RetainedDevice::new(realtime);
        let before = worker.snapshot();
        device.command(&mut worker, AudioCommand::Pause);
        assert_eq!(worker.snapshot().state, PlaybackState::Paused);
        device.assert_retained_silence();
        // While paused there are no callback submissions: queued audio survives
        // unchanged, exactly as a WASAPI Stop or skipped ASIO callback does.
        if seek {
            worker
                .execute_command(AudioCommand::SeekAndPlay(0.75))
                .unwrap();
            assert_eq!(worker.snapshot().position, 0.75);
        } else {
            worker.execute_command(AudioCommand::Play).unwrap();
        }
        let resumed = worker.snapshot();
        assert_eq!(resumed.state, PlaybackState::Playing);
        assert_eq!(resumed.generation, before.generation);
        assert_eq!(resumed.play_session_id, before.play_session_id);
        device.assert_resumes_from_zero(seek);
    }
}

#[test]
fn loading_a_reused_output_drains_the_old_device_queue_before_play() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("old-retained-device.wav");
    let replacement = temp.path().join("new-retained-device.wav");
    let mut worker = playing_worker(&path);
    write_source(&replacement, true);
    let realtime = worker.output.as_ref().unwrap().realtime.clone();
    let mut device = RetainedDevice::new(realtime.clone());
    let before = worker.snapshot();
    device.command(&mut worker, AudioCommand::Load(replacement.clone()));
    assert_eq!(worker.snapshot().state, PlaybackState::Ready);
    assert_eq!(worker.snapshot().file_path, Some(replacement));
    assert_eq!(worker.snapshot().generation, before.generation + 1);
    assert!(Arc::ptr_eq(
        &worker.output.as_ref().unwrap().realtime,
        &realtime
    ));
    device.assert_retained_silence();
    worker.execute_command(AudioCommand::Play).unwrap();
    device.assert_resumes_from_zero(true);
}

#[test]
fn silent_callback_count_resets_on_audio_and_excludes_a_partial_fade_buffer() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("silent-device-buffer-count.wav");
    let worker = playing_worker(&path);
    let realtime = worker.output.as_ref().unwrap().realtime.clone();
    let mut device = RetainedDevice::new(realtime.clone());
    assert_eq!(
        realtime.transition.silent_callbacks.load(Ordering::Acquire),
        0
    );

    realtime.running.store(false, Ordering::SeqCst);
    for expected in 1..=DEVICE_BUFFERS {
        device.submit();
        assert_eq!(
            realtime.transition.silent_callbacks.load(Ordering::Acquire),
            expected
        );
    }
    realtime.running.store(true, Ordering::SeqCst);
    device.submit();
    assert_eq!(
        realtime.transition.silent_callbacks.load(Ordering::Acquire),
        0,
        "audible output must invalidate earlier silent device buffers"
    );

    let revision = realtime.transition.fade_in_revision.load(Ordering::Acquire);
    realtime.transition.fade_out.store(true, Ordering::Release);
    let mut partial_fade_buffer = false;
    for _ in 0..128 {
        let start = device.submitted.len();
        device.submit();
        let block = &device.submitted[start..];
        let audible = block.iter().any(|sample| *sample != 0.0);
        if audible {
            assert_eq!(
                realtime.transition.silent_callbacks.load(Ordering::Acquire),
                0,
                "a buffer containing fade audio must not count as a silent device buffer"
            );
            partial_fade_buffer |= block[block.len() - 2..].iter().all(|sample| *sample == 0.0);
        }
        if realtime.transition.faded_out.load(Ordering::Acquire) == revision {
            break;
        }
    }
    assert!(
        partial_fade_buffer,
        "regression must cover a callback containing both fade audio and silence"
    );
    assert_eq!(
        realtime.transition.faded_out.load(Ordering::Acquire),
        revision
    );
    assert_eq!(
        realtime.transition.silent_callbacks.load(Ordering::Acquire),
        0,
        "the fade acknowledgment alone does not clear a whole device buffer"
    );
    for expected in 1..=DEVICE_BUFFERS {
        device.submit();
        assert_eq!(
            realtime.transition.silent_callbacks.load(Ordering::Acquire),
            expected,
            "only subsequent wholly silent callbacks clear the two hardware buffers"
        );
    }
}

#[test]
fn failed_seek_reopen_drains_the_old_device_queue_before_closing_output() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("removed-during-preview.wav");
    let mut worker = playing_worker(&path);
    let realtime = worker.output.as_ref().unwrap().realtime.clone();
    let mut device = RetainedDevice::new(realtime);
    // This is an owned generated fixture, not a source library WAV. The current
    // decoder and device still hold valid old PCM while reopening must fail.
    std::fs::remove_file(&path).unwrap();
    let error = device
        .command_result(&mut worker, AudioCommand::SeekAndPlay(0.75))
        .unwrap_err();
    assert!(
        matches!(&error, Error::Io(cause) if cause.kind() == std::io::ErrorKind::NotFound),
        "draining must preserve the original reopen error: {error}"
    );
    assert_eq!(worker.snapshot().state, PlaybackState::Error);
    assert_eq!(worker.snapshot().error, Some(error.to_string()));
    assert!(worker.output.is_none());
    device.assert_retained_silence();
}

#[test]
fn pause_during_network_waiting_drains_a_live_device_with_pcm_consumption_stopped() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("buffering-retained-device.wav");
    let mut worker = playing_worker(&path);
    let realtime = worker.output.as_ref().unwrap().realtime.clone();
    let mut device = RetainedDevice::new(realtime.clone());
    // Model the exact transport boundary after network buffering suspends PCM
    // consumption: hardware remains live and still owns two nonzero buffers.
    // The source itself need not perform a network request to exercise this state.
    worker.output.as_mut().unwrap().buffering = Buffering::Waiting(u64::from(RATE));
    realtime.running.store(false, Ordering::SeqCst);
    worker.update(|status| status.buffering = true);
    device.command(&mut worker, AudioCommand::Pause);
    assert_eq!(worker.snapshot().state, PlaybackState::Paused);
    assert!(!worker.snapshot().buffering);
    assert!(!realtime.running.load(Ordering::SeqCst));
    device.assert_retained_silence();
}

#[test]
fn asio_pause_waits_for_two_wholly_silent_callbacks_even_after_time_drain_expires() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("asio-silent-device-buffers.wav");
    let mut worker = playing_worker(&path);
    let realtime = worker.output.as_ref().unwrap().realtime.clone();
    let mut device = RetainedDevice::new(realtime.clone());
    realtime.running.store(false, Ordering::SeqCst);
    let output = worker.output.as_mut().unwrap();
    // Select only the ASIO drain policy; the fixture has no CPAL stream or driver.
    // Restore this flag before assertions so Drop cannot change the real driver count.
    output.asio = true;
    let (waited_before_silence, waited_after_one, result) = thread::scope(|scope| {
        let (sender, receiver) = mpsc::sync_channel(1);
        let asio_output = &mut *output;
        scope.spawn(move || {
            let _ = sender.send(asio_output.pause_after_drain(Some(std::time::Instant::now())));
        });
        let early = receiver.recv_timeout(Duration::from_millis(10));
        if !matches!(&early, Err(mpsc::RecvTimeoutError::Timeout)) {
            return (false, false, early);
        }
        device.submit();
        let after_one = receiver.recv_timeout(Duration::from_millis(10));
        if !matches!(&after_one, Err(mpsc::RecvTimeoutError::Timeout)) {
            return (true, false, after_one);
        }
        device.submit();
        (true, true, receiver.recv_timeout(Duration::from_secs(1)))
    });
    output.asio = false;
    assert!(
        waited_before_silence,
        "ASIO paused with stale device buffers"
    );
    assert!(
        waited_after_one,
        "one silent callback cannot clear both ASIO device buffers"
    );
    result.unwrap().unwrap();
    assert_eq!(
        realtime.transition.silent_callbacks.load(Ordering::Acquire),
        DEVICE_BUFFERS
    );
    device.assert_retained_silence();
}
