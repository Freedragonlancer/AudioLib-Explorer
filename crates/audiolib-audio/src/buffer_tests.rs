// crates/audiolib-audio/src/buffer_tests.rs
//! Device-period regressions run real decoders and callbacks without a CPAL device.
use super::*;
use std::path::Path;

fn source(path: &Path, rate: u32, frames: usize, negative_from: usize) {
    let pcm = (0..frames)
        .flat_map(|frame| {
            let left = if frame < negative_from {
                0.5_f32
            } else {
                -0.5_f32
            };
            [left, -left].into_iter().flat_map(f32::to_le_bytes)
        })
        .collect::<Vec<_>>();
    crate::tests::write_wav(path, rate, 2, 32, 3, &pcm, false, false);
}

fn output(path: &Path, rate: u32, callback_frames: u32) -> Output {
    Output {
        stream: None,
        callback_available: false,
        asio: false,
        realtime: Arc::new(Realtime {
            ring: Ring::new(output_ring_capacity(rate)),
            running: AtomicBool::new(false),
            gain: AtomicU32::new(1.0_f32.to_bits()),
            consumed: AtomicU64::new(0),
            underruns: AtomicU64::new(0),
            failed: AtomicBool::new(false),
            active: AtomicBool::new(false),
            callback_latency_nanos: AtomicU64::new(0),
            source_finished: AtomicBool::new(false),
            transition: SourceTransition::default(),
        }),
        decoder: AuditionDecoder::new(
            Decoder::open(path).unwrap(),
            rate,
            0,
            0,
            AuditionSettings::default(),
        )
        .unwrap(),
        sample_rate: rate,
        prefill_frames: output_prefill_frames(rate, callback_frames),
        volume: 1.0,
        buffering: Buffering::Off,
        pending: Vec::new(),
        pending_offset: 0,
        eof: false,
        pending_last: false,
        start_position: 0.0,
        drain_deadline: None,
    }
}

fn callback(realtime: Arc<Realtime>) -> RenderCallback {
    RenderCallback {
        realtime,
        channels: 2,
        pair: 0,
        ramp_step: 1.0,
        gain: 1.0,
        envelope: SeekEnvelope::default(),
    }
}

#[test]
fn legal_large_callbacks_have_complete_pcm_and_exact_eof_without_underruns() {
    let temp = tempfile::tempdir().unwrap();
    for rate in [44_100, 48_000] {
        for period in [64, 8192, 16_384, MAX_OUTPUT_BUFFER_FRAMES] {
            let path = temp.path().join(format!("{rate}-{period}.wav"));
            let frames = period as usize * 3 + 123;
            source(&path, rate, frames, usize::MAX);
            let mut output = output(&path, rate, period);
            output.start_running().unwrap();
            assert!(output.realtime.ring.len() >= period as usize * 2);
            if period == 64 {
                assert!(output.realtime.ring.len() < output.realtime.ring.capacity() / 2);
            }
            let mut callback = callback(output.realtime.clone());
            let mut rendered = 0;
            while rendered < frames {
                output.fill().unwrap();
                let mut block = vec![9.0_f32; period as usize * 2];
                callback.render(&mut block);
                for (offset, frame) in block.chunks_exact(2).enumerate() {
                    assert_eq!(
                        frame,
                        if rendered + offset < frames {
                            &[0.5, -0.5]
                        } else {
                            &[0.0, 0.0]
                        },
                        "rate={rate}, period={period}, frame={}",
                        rendered + offset
                    );
                }
                rendered += period as usize;
            }
            assert!(output.eof);
            assert_eq!(output.realtime.ring.len(), 0);
            assert_eq!(
                output.realtime.consumed.load(Ordering::Relaxed),
                frames as u64
            );
            assert_eq!(output.realtime.underruns.load(Ordering::Relaxed), 0);
            assert!((output.position() - frames as f64 / f64::from(rate)).abs() < 1e-9);
        }
    }
}

#[test]
fn large_callback_seek_discards_old_pcm_and_prefills_the_new_position() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("seek.wav");
    let rate = 48_000;
    let period = MAX_OUTPUT_BUFFER_FRAMES;
    let seek_frame = period as usize + 123;
    source(&path, rate, period as usize * 4, seek_frame);
    let mut output = output(&path, rate, period);
    output.start_running().unwrap();
    let realtime = output.realtime.clone();
    let mut callback = callback(realtime.clone());
    let mut block = vec![0.0_f32; period as usize * 2];
    callback.render(&mut block);
    assert!(block.chunks_exact(2).all(|frame| frame == [0.5, -0.5]));
    let mut worker = tests::worker(PlaybackStatus {
        generation: 7,
        play_session_id: 1,
        state: PlaybackState::Playing,
        file_path: Some(path),
        duration: f64::from(period * 4) / f64::from(rate),
        sample_rate: rate,
        channels: 2,
        ..Default::default()
    });
    worker.output = Some(output);
    worker
        .execute_command(AudioCommand::Seek(seek_frame as f64 / f64::from(rate)))
        .unwrap();
    assert!(realtime.ring.len() >= period as usize * 2);
    callback.render(&mut block);
    assert_eq!(&block[..2], &[0.0, 0.0]);
    assert!(
        block[2..]
            .chunks_exact(2)
            .all(|frame| frame[0] < 0.0 && frame[1] > 0.0)
    );
    assert_eq!(realtime.underruns.load(Ordering::Relaxed), 0);
    assert_eq!(realtime.consumed.load(Ordering::Relaxed), u64::from(period));
}

#[test]
fn callbacks_exceeding_the_supported_limit_fail_without_consuming_pcm() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("oversized.wav");
    source(&path, 48_000, 1000, usize::MAX);
    let mut output = output(&path, 48_000, 64);
    output.start_running().unwrap();
    let realtime = output.realtime.clone();
    let before = realtime.ring.len();
    let mut callback = callback(realtime.clone());
    let mut block = vec![9.0_f32; (MAX_OUTPUT_BUFFER_FRAMES as usize + 1) * 2];
    callback.render(&mut block);
    assert!(block.iter().all(|sample| *sample == 0.0));
    assert!(realtime.failed.load(Ordering::Acquire));
    assert!(!realtime.active.load(Ordering::SeqCst));
    assert_eq!(realtime.ring.len(), before);
    assert_eq!(realtime.consumed.load(Ordering::Relaxed), 0);
}

#[test]
fn a_large_network_period_waits_for_its_whole_first_callback() {
    use crate::{ByteSource, SourceBuffer, SourceOpener};
    struct Available {
        total: u64,
        bytes: AtomicU64,
    }
    impl SourceBuffer for Available {
        fn is_buffered(&self, offset: u64, len: u64) -> bool {
            offset.saturating_add(len).min(self.total) <= self.bytes.load(Ordering::Acquire)
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("source.wav");
    let rate = 48_000;
    let period = MAX_OUTPUT_BUFFER_FRAMES;
    source(&path, rate, period as usize * 3, usize::MAX);
    let bytes = Arc::new(std::fs::read(&path).unwrap());
    let buffer = Arc::new(Available {
        total: bytes.len() as u64,
        bytes: AtomicU64::new(44 + 40_000 * 8),
    });
    let opener: SourceOpener = {
        let bytes = bytes.clone();
        Arc::new(move || {
            let reader: Box<dyn ByteSource> = Box::new(std::io::Cursor::new(bytes.to_vec()));
            Ok((reader, bytes.len() as u64))
        })
    };
    let virtual_path = temp.path().join("remote.wav");
    crate::register_buffered_stream_source(&virtual_path, opener, buffer.clone());
    let mut output = output(&virtual_path, rate, period);
    output.start_running().unwrap();
    assert!(output.is_waiting());
    assert!(!output.realtime.running.load(Ordering::Acquire));
    assert!(output.realtime.ring.len() < period as usize * 2);
    buffer.bytes.store(buffer.total, Ordering::Release);
    output.regulate(false).unwrap();
    assert!(!output.is_waiting());
    let mut callback = callback(output.realtime.clone());
    let mut block = vec![9.0_f32; period as usize * 2];
    callback.render(&mut block);
    assert!(block.chunks_exact(2).all(|frame| frame == [0.5, -0.5]));
    assert_eq!(output.realtime.underruns.load(Ordering::Relaxed), 0);
    crate::unregister_stream_source(&virtual_path);
}
