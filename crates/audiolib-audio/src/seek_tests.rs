// crates/audiolib-audio/src/seek_tests.rs
//! Hardware-free regressions for the seek boundary envelope and callback handshake.

use super::*;

fn realtime(volume: f32) -> Arc<Realtime> {
    Arc::new(Realtime {
        ring: Ring::new(4096),
        running: AtomicBool::new(true),
        gain: AtomicU32::new(volume.to_bits()),
        consumed: AtomicU64::new(0),
        underruns: AtomicU64::new(0),
        failed: AtomicBool::new(false),
        active: AtomicBool::new(false),
        callback_latency_nanos: AtomicU64::new(0),
        source_finished: AtomicBool::new(false),
        transition: SourceTransition {
            faded_out: AtomicU64::new(u64::MAX),
            ..Default::default()
        },
    })
}

fn callback(realtime: Arc<Realtime>, channels: usize, pair: usize, gain: f32) -> RenderCallback {
    RenderCallback {
        realtime,
        channels,
        pair,
        ramp_step: 1.0,
        gain,
        envelope: SeekEnvelope::default(),
    }
}

fn push_frames(realtime: &Realtime, frame: [f32; 2], count: usize) {
    let input: Vec<_> = (0..count).flat_map(|_| frame).collect();
    assert_eq!(realtime.ring.push_aligned(&input, 2), input.len());
}

fn assert_near(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 1e-6,
        "actual {actual} differs from expected {expected}"
    );
}

#[test]
fn seek_fades_opposite_sign_pcm_and_preserves_volume_and_hardware_routing() {
    let volume = 0.4;
    let realtime = realtime(volume);
    let mut callback = callback(realtime.clone(), 6, 1, volume);
    let frames = SEEK_FADE_FRAMES as usize;
    push_frames(&realtime, [0.75, -0.5], frames * 3);
    realtime.transition.fade_out.store(true, Ordering::Release);
    let mut old_output = vec![9.0_f32; frames * 6];
    callback.render(&mut old_output);
    assert_eq!(realtime.transition.faded_out.load(Ordering::Acquire), 0);
    for (index, frame) in old_output.chunks_exact(6).enumerate() {
        let envelope = (frames - 1 - index) as f32 / (frames - 1) as f32;
        assert_near(frame[2], 0.75 * volume * envelope);
        assert_near(frame[3], -0.5 * volume * envelope);
        for channel in [0, 1, 4, 5] {
            assert_eq!(frame[channel], 0.0);
        }
    }
    let consumed = realtime.consumed.load(Ordering::Relaxed);
    let remaining = realtime.ring.len();
    let mut muted = [9.0_f32; 42];
    callback.render(&mut muted);
    assert_eq!(muted, [0.0; 42]);
    assert_eq!(realtime.consumed.load(Ordering::Relaxed), consumed);
    assert_eq!(realtime.ring.len(), remaining);

    // Model the worker's quiescent source replacement: no pre-seek sample may escape.
    realtime.running.store(false, Ordering::SeqCst);
    assert!(!realtime.active.load(Ordering::SeqCst));
    realtime.ring.clear_suspended();
    realtime.consumed.store(0, Ordering::Relaxed);
    push_frames(&realtime, [-0.8, 0.6], frames * 2);
    realtime.transition.fade_out.store(false, Ordering::Release);
    realtime
        .transition
        .fade_in_revision
        .fetch_add(1, Ordering::Release);
    realtime.running.store(true, Ordering::SeqCst);
    let mut new_output = vec![9.0_f32; (frames + 1) * 6];
    callback.render(&mut new_output);
    for (index, frame) in new_output.chunks_exact(6).enumerate() {
        let envelope = index.min(frames - 1) as f32 / (frames - 1) as f32;
        assert_near(frame[2], -0.8 * volume * envelope);
        assert_near(frame[3], 0.6 * volume * envelope);
        for channel in [0, 1, 4, 5] {
            assert_eq!(frame[channel], 0.0);
        }
    }
    assert_eq!(
        f32::from_bits(realtime.gain.load(Ordering::Relaxed)),
        volume
    );
    assert_near(callback.gain, volume);
}

#[test]
fn seek_fade_spans_short_callbacks_and_acknowledges_only_after_the_zero_frame() {
    let realtime = realtime(1.0);
    let mut callback = callback(realtime.clone(), 2, 0, 1.0);
    let frames = SEEK_FADE_FRAMES as usize;
    push_frames(&realtime, [0.9, -0.3], frames * 3);
    realtime.transition.fade_out.store(true, Ordering::Release);
    let mut output = Vec::new();
    let mut rendered = 0;
    for chunk in [1, 3, 7, 2, 11, 7, 1] {
        let mut block = vec![9.0_f32; chunk * 2];
        callback.render(&mut block);
        output.extend(block);
        rendered += chunk;
        assert_eq!(
            realtime.transition.faded_out.load(Ordering::Acquire) == 0,
            rendered == frames,
            "fade acknowledgment after {rendered} frames"
        );
    }
    assert_eq!(rendered, frames);
    for (index, frame) in output.chunks_exact(2).enumerate() {
        let envelope = (frames - 1 - index) as f32 / (frames - 1) as f32;
        assert_near(frame[0], 0.9 * envelope);
        assert_near(frame[1], -0.3 * envelope);
    }
    let consumed = realtime.consumed.load(Ordering::Relaxed);
    let mut output = [9.0_f32; 128];
    callback.render(&mut output);
    assert_eq!(output, [0.0; 128]);
    assert_eq!(realtime.consumed.load(Ordering::Relaxed), consumed);
}

#[test]
fn paused_and_repeated_seek_revisions_restart_the_fade_at_the_first_audible_frame() {
    let volume = 0.7;
    let realtime = realtime(volume);
    let mut callback = callback(realtime.clone(), 2, 0, volume);
    let frames = SEEK_FADE_FRAMES as usize;
    realtime.running.store(false, Ordering::SeqCst);
    for revision in [1, 2] {
        realtime.ring.clear_suspended();
        push_frames(&realtime, [-0.6, 0.3], frames * 2);
        realtime
            .transition
            .fade_in_revision
            .store(revision, Ordering::Release);
        let mut silent = [9.0_f32; 18];
        callback.render(&mut silent);
        assert_eq!(silent, [0.0; 18]);
        assert_eq!(realtime.consumed.load(Ordering::Relaxed), 0);
    }
    realtime.running.store(true, Ordering::SeqCst);
    let mut partial = [9.0_f32; 16];
    callback.render(&mut partial);
    for (index, frame) in partial.chunks_exact(2).enumerate() {
        let envelope = index as f32 / (frames - 1) as f32;
        assert_near(frame[0], -0.6 * volume * envelope);
        assert_near(frame[1], 0.3 * volume * envelope);
    }

    // A second reset during fade-in must discard its partial envelope as well as its PCM.
    realtime.running.store(false, Ordering::SeqCst);
    realtime.ring.clear_suspended();
    realtime.consumed.store(0, Ordering::Relaxed);
    push_frames(&realtime, [0.2, -0.9], frames * 2);
    realtime
        .transition
        .fade_in_revision
        .store(3, Ordering::Release);
    let mut silent = [9.0_f32; 20];
    callback.render(&mut silent);
    assert_eq!(silent, [0.0; 20]);
    realtime.running.store(true, Ordering::SeqCst);
    let mut resumed = vec![9.0_f32; frames * 2];
    callback.render(&mut resumed);
    for (index, frame) in resumed.chunks_exact(2).enumerate() {
        let envelope = index as f32 / (frames - 1) as f32;
        assert_near(frame[0], 0.2 * volume * envelope);
        assert_near(frame[1], -0.9 * volume * envelope);
    }
    assert_eq!(realtime.consumed.load(Ordering::Relaxed), frames as u64);
}

#[test]
fn seek_worker_waits_for_real_callback_fade_acknowledgment() {
    let realtime = realtime(0.8);
    push_frames(&realtime, [0.5, -0.75], SEEK_FADE_FRAMES as usize * 3);
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let callback_realtime = realtime.clone();
    let callback_barrier = barrier.clone();
    let render_thread = thread::spawn(move || {
        let mut callback = callback(callback_realtime.clone(), 2, 0, 0.8);
        callback_barrier.wait();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !callback_realtime
            .transition
            .fade_out
            .load(Ordering::Acquire)
        {
            assert!(std::time::Instant::now() < deadline, "no fade request");
            thread::yield_now();
        }
        let mut rendered = Vec::new();
        while callback_realtime
            .transition
            .faded_out
            .load(Ordering::Acquire)
            != 0
        {
            let mut block = [9.0_f32; 6];
            callback.render(&mut block);
            rendered.extend(block);
            assert!(
                std::time::Instant::now() < deadline,
                "fade was not acknowledged"
            );
        }
        rendered
    });
    barrier.wait();
    let result = realtime.fade_out_before_reset();
    let rendered = render_thread.join().unwrap();
    result.unwrap();
    assert_eq!(realtime.transition.faded_out.load(Ordering::Acquire), 0);
    assert!(!realtime.active.load(Ordering::SeqCst));
    assert_near(rendered[0], 0.4);
    let last = SEEK_FADE_FRAMES as usize - 1;
    assert_eq!(rendered[last * 2], 0.0);
    assert_eq!(rendered[last * 2 + 1], 0.0);
}

#[test]
fn seek_worker_rejects_a_failed_output_even_with_a_stale_acknowledgment() {
    let realtime = realtime(0.5);
    // Default revision zero used to be indistinguishable from a previous completion.
    realtime.transition.faded_out.store(0, Ordering::Release);
    realtime.failed.store(true, Ordering::Release);
    assert!(realtime.fade_out_before_reset().is_err());
}

#[test]
fn seek_worker_ignores_an_acknowledgment_from_the_previous_source() {
    let realtime = realtime(0.6);
    let mut callback = callback(realtime.clone(), 2, 0, 0.6);
    let frames = SEEK_FADE_FRAMES as usize;
    push_frames(&realtime, [0.5, -0.5], frames * 4);
    realtime
        .transition
        .fade_in_revision
        .store(5, Ordering::Release);
    // Finish this source's initial ramp before exercising its fade-out handshake.
    callback.render(&mut vec![0.0_f32; frames * 2]);
    realtime
        .callback_latency_nanos
        .store(250_000_000, Ordering::Relaxed);
    let (sender, receiver) = std::sync::mpsc::channel();
    let worker_realtime = realtime.clone();
    let worker = thread::spawn(move || {
        sender
            .send(worker_realtime.fade_out_before_reset())
            .unwrap();
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while !realtime.transition.fade_out.load(Ordering::Acquire) {
        assert!(std::time::Instant::now() < deadline, "no fade request");
        thread::yield_now();
    }
    // A delayed callback from revision 4 cannot release a waiter for revision 5.
    realtime.transition.faded_out.store(4, Ordering::Release);
    assert!(matches!(
        receiver.recv_timeout(Duration::from_millis(10)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    ));
    let mut faded = vec![9.0_f32; frames * 2];
    callback.render(&mut faded);
    receiver
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    worker.join().unwrap();
    assert_eq!(realtime.transition.faded_out.load(Ordering::Acquire), 5);
    assert_near(faded[0], 0.3);
    assert_eq!(faded[(frames - 1) * 2], 0.0);
}

#[test]
fn seek_worker_times_out_without_a_callback_instead_of_hanging() {
    let realtime = realtime(0.5);
    let started = std::time::Instant::now();
    let error = realtime.fade_out_before_reset().unwrap_err();
    assert!(error.to_string().contains("did not finish seek fade"));
    assert!(started.elapsed() >= Duration::from_millis(100));
    assert!(!realtime.active.load(Ordering::SeqCst));
}

#[test]
fn seek_fade_finishes_a_short_or_empty_old_ring_without_advancing_synthetic_frames() {
    let frames = SEEK_FADE_FRAMES as usize;
    let volume = 0.6;
    let pcm = [0.8, -0.5];
    for remaining in [0, 1, 7, 31] {
        for finished in [false, true] {
            let realtime = realtime(volume);
            let mut callback = callback(realtime.clone(), 2, 0, volume);
            push_frames(&realtime, pcm, 1);
            let mut primed = [0.0_f32; 2];
            callback.render(&mut primed);
            assert_near(primed[0], pcm[0] * volume);
            assert_near(primed[1], pcm[1] * volume);
            push_frames(&realtime, pcm, remaining);
            realtime.source_finished.store(finished, Ordering::Release);
            let consumed = realtime.consumed.load(Ordering::Relaxed);
            realtime.transition.fade_out.store(true, Ordering::Release);
            let mut faded = vec![9.0_f32; frames * 2];
            callback.render(&mut faded);
            for (index, frame) in faded.chunks_exact(2).enumerate() {
                let envelope = (frames - 1 - index) as f32 / (frames - 1) as f32;
                assert_near(frame[0], pcm[0] * volume * envelope);
                assert_near(frame[1], pcm[1] * volume * envelope);
            }
            let actual = remaining.min(frames - 1);
            assert_eq!(
                realtime.consumed.load(Ordering::Relaxed),
                consumed + actual as u64,
                "synthetic frames advanced the cursor with remaining={remaining}, finished={finished}"
            );
            assert_eq!(realtime.ring.len(), (remaining - actual) * 2);
            assert_eq!(realtime.transition.faded_out.load(Ordering::Acquire), 0);
            let consumed = realtime.consumed.load(Ordering::Relaxed);
            let mut muted = [9.0_f32; 14];
            callback.render(&mut muted);
            assert_eq!(muted, [0.0; 14]);
            assert_eq!(realtime.consumed.load(Ordering::Relaxed), consumed);
        }
    }
}

#[test]
fn seek_fade_does_not_resurrect_pcm_after_an_ordinary_underrun_or_pause() {
    let frames = SEEK_FADE_FRAMES as usize;
    for pause in [false, true] {
        let realtime = realtime(0.8);
        let mut callback = callback(realtime.clone(), 2, 0, 0.8);
        push_frames(&realtime, [0.75, -0.5], 1);
        let mut primed = [0.0_f32; 2];
        callback.render(&mut primed);
        assert_near(primed[0], 0.6);
        assert_near(primed[1], -0.4);
        if pause {
            realtime.running.store(false, Ordering::SeqCst);
        }
        // The last submitted frame is silence, so the former nonzero sample is obsolete.
        let mut silent = [9.0_f32; 2];
        callback.render(&mut silent);
        assert_eq!(silent, [0.0; 2]);
        let consumed = realtime.consumed.load(Ordering::Relaxed);
        realtime.transition.fade_out.store(true, Ordering::Release);
        realtime.running.store(true, Ordering::SeqCst);
        let mut faded = vec![9.0_f32; frames * 2];
        callback.render(&mut faded);
        assert!(faded.iter().all(|sample| *sample == 0.0));
        assert_eq!(realtime.consumed.load(Ordering::Relaxed), consumed);
        assert_eq!(realtime.transition.faded_out.load(Ordering::Acquire), 0);
    }
}

#[test]
fn seek_source_revision_discards_the_previous_tail_before_any_new_pcm_is_available() {
    let frames = SEEK_FADE_FRAMES as usize;
    let realtime = realtime(1.0);
    let mut callback = callback(realtime.clone(), 2, 0, 1.0);
    push_frames(&realtime, [0.8, -0.6], 1);
    callback.render(&mut [0.0_f32; 2]);
    realtime.transition.fade_out.store(true, Ordering::Release);
    let mut partial = [9.0_f32; 12];
    callback.render(&mut partial);
    assert!(partial.chunks_exact(2).all(|frame| frame[0] > 0.0));

    // Replace the source while its tail is still nonzero, then seek again before new PCM.
    realtime.running.store(false, Ordering::SeqCst);
    realtime.ring.clear_suspended();
    realtime.consumed.store(0, Ordering::Relaxed);
    realtime.transition.fade_out.store(false, Ordering::Release);
    realtime
        .transition
        .fade_in_revision
        .store(1, Ordering::Release);
    realtime.transition.faded_out.store(0, Ordering::Release);
    realtime.transition.fade_out.store(true, Ordering::Release);
    realtime.running.store(true, Ordering::SeqCst);
    let mut new_source = vec![9.0_f32; frames * 2];
    callback.render(&mut new_source);
    assert!(new_source.iter().all(|sample| *sample == 0.0));
    assert_eq!(realtime.consumed.load(Ordering::Relaxed), 0);
    assert_eq!(realtime.transition.faded_out.load(Ordering::Acquire), 1);
}

fn seek_through_worker_with_simulated_callbacks(
    sample_rate: u32,
    stall_open: bool,
) -> Vec<[f32; 2]> {
    use crate::source::{
        ByteSource, SourceOpener, register_stream_source, unregister_stream_source,
    };

    struct Registration(std::path::PathBuf);
    impl Drop for Registration {
        fn drop(&mut self) {
            unregister_stream_source(&self.0);
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("source.wav");
    let virtual_path = temp.path().join("registered.wav");
    let pcm: Vec<u8> = (0..sample_rate)
        .flat_map(|frame| {
            // At either rate the requested 0.25 s starts exactly at the sign change.
            let pcm = if frame < sample_rate / 4 {
                [0.65_f32, -0.3]
            } else {
                [-0.7_f32, 0.4]
            };
            pcm.into_iter().flat_map(f32::to_le_bytes)
        })
        .collect();
    crate::tests::write_wav(&path, sample_rate, 2, 32, 3, &pcm, false, false);
    let bytes = Arc::new(std::fs::read(&path).unwrap());
    let realtime = realtime(0.8);
    realtime
        .callback_latency_nanos
        .store(50_000_000, Ordering::Relaxed);
    push_frames(&realtime, [0.75, -0.5], 1024);
    let output = Output {
        stream: None,
        callback_available: true,
        asio: false,
        realtime: realtime.clone(),
        decoder: AuditionDecoder::new(
            Decoder::open(&path).unwrap(),
            sample_rate,
            0,
            0,
            AuditionSettings::default(),
        )
        .unwrap(),
        sample_rate,
        volume: 0.8,
        buffering: Buffering::Off,
        pending: Vec::new(),
        pending_offset: 0,
        eof: false,
        pending_last: false,
        start_position: 0.0,
        drain_deadline: None,
    };
    let rendered_blocks = Arc::new(AtomicU64::new(0));
    let opens = Arc::new(AtomicU64::new(0));
    let opener: SourceOpener = {
        let realtime = realtime.clone();
        let rendered_blocks = rendered_blocks.clone();
        let opens = opens.clone();
        Arc::new(move || {
            if opens.fetch_add(1, Ordering::Relaxed) == 0 {
                if !realtime.transition.fade_out.load(Ordering::Acquire)
                    || realtime.transition.faded_out.load(Ordering::Acquire) != 0
                    || realtime.running.load(Ordering::SeqCst)
                {
                    return Err(std::io::Error::other(
                        "seek opened the decoder before the old callback faded and stopped consuming",
                    ));
                }
                if stall_open {
                    let consumed = realtime.consumed.load(Ordering::Relaxed);
                    let buffered = realtime.ring.len();
                    let target = rendered_blocks.load(Ordering::Acquire) + 20;
                    let deadline = std::time::Instant::now() + Duration::from_secs(2);
                    while rendered_blocks.load(Ordering::Acquire) < target {
                        if std::time::Instant::now() >= deadline {
                            return Err(std::io::Error::other(
                                "simulated callbacks stopped while the source opener was blocked",
                            ));
                        }
                        thread::yield_now();
                    }
                    if realtime.consumed.load(Ordering::Relaxed) != consumed
                        || realtime.ring.len() != buffered
                    {
                        return Err(std::io::Error::other(
                            "source I/O consumed the old ring after the fade acknowledgment",
                        ));
                    }
                }
            }
            let source: Box<dyn ByteSource> = Box::new(std::io::Cursor::new((*bytes).clone()));
            Ok((source, bytes.len() as u64))
        })
    };
    register_stream_source(&virtual_path, opener);
    let _registration = Registration(virtual_path.clone());
    let mut worker = Worker::new(
        PlaybackConfig::default(),
        Arc::new(Mutex::new(PlaybackStatus {
            generation: 7,
            play_session_id: 11,
            state: PlaybackState::Playing,
            file_path: Some(virtual_path),
            duration: 1.0,
            sample_rate,
            device_sample_rate: sample_rate,
            channels: 2,
            volume: 0.8,
            ..Default::default()
        })),
        Arc::new(Mutex::new(Vec::new())),
    );
    worker.output = Some(output);
    let stop = Arc::new(AtomicBool::new(false));
    let render_thread = {
        let realtime = realtime.clone();
        let stop = stop.clone();
        thread::spawn(move || {
            let mut callback = callback(realtime, 2, 0, 0.8);
            let mut rendered = Vec::new();
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while !stop.load(Ordering::Acquire) && std::time::Instant::now() < deadline {
                let mut block = [0.0_f32; 16];
                callback.render(&mut block);
                rendered.extend(block.chunks_exact(2).map(|frame| [frame[0], frame[1]]));
                rendered_blocks.fetch_add(1, Ordering::Release);
                thread::sleep(Duration::from_millis(1));
            }
            rendered
        })
    };
    let result = worker.execute_command(AudioCommand::Seek(0.25));
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while result.is_ok()
        && realtime.consumed.load(Ordering::Relaxed) < u64::from(SEEK_FADE_FRAMES) + 8
        && std::time::Instant::now() < deadline
    {
        thread::yield_now();
    }
    stop.store(true, Ordering::Release);
    let rendered = render_thread.join().unwrap();
    result.unwrap();
    assert!(opens.load(Ordering::Relaxed) >= 2);
    assert!(realtime.consumed.load(Ordering::Relaxed) >= u64::from(SEEK_FADE_FRAMES) + 8);
    let status = worker.snapshot();
    assert_eq!(status.state, PlaybackState::Playing);
    assert_eq!(status.generation, 7);
    assert_eq!(status.play_session_id, 11);
    assert_eq!(status.position, 0.25);
    assert_eq!(status.sample_rate, sample_rate);
    assert_eq!(status.device_sample_rate, sample_rate);
    let output = worker.output.as_ref().unwrap();
    assert_eq!(output.sample_rate, sample_rate);
    assert_eq!(output.start_position, 0.25);
    let expected_position =
        0.25 + realtime.consumed.load(Ordering::Relaxed) as f64 / f64::from(sample_rate);
    assert!((output.position() - expected_position).abs() < 1e-12);
    rendered
}

#[test]
fn seek_fades_and_stops_consumption_before_a_slow_decoder_open() {
    for sample_rate in [48_000, 96_000] {
        let rendered = seek_through_worker_with_simulated_callbacks(sample_rate, true);
        let last_old = rendered.iter().rposition(|frame| frame[0] > 0.0).unwrap();
        let first_new = rendered.iter().position(|frame| frame[0] < 0.0).unwrap();
        assert!(first_new > last_old + 20 * 8);
        assert!(
            rendered[last_old + 1..first_new]
                .iter()
                .all(|frame| *frame == [0.0; 2])
        );
    }
}

#[test]
fn seek_worker_replaces_real_pcm_through_the_actual_callback_handshake() {
    for sample_rate in [48_000, 96_000] {
        let rendered = seek_through_worker_with_simulated_callbacks(sample_rate, false);
        let first_new = rendered.iter().position(|frame| frame[0] < 0.0).unwrap();
        assert!(first_new > 0);
        assert_eq!(rendered[first_new - 1], [0.0; 2]);
        for index in 0..SEEK_FADE_FRAMES as usize - 1 {
            let envelope = (index + 1) as f32 / (SEEK_FADE_FRAMES - 1) as f32;
            assert_near(rendered[first_new + index][0], -0.7 * 0.8 * envelope);
            assert_near(rendered[first_new + index][1], 0.4 * 0.8 * envelope);
        }
        assert!(rendered[first_new..].iter().all(|frame| frame[0] <= 0.0));
    }
}
