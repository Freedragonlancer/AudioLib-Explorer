// crates/audiolib-audio/examples/seek-loopback.rs
//! Opt-in WASAPI endpoint loopback diagnostics; no physical recording or listening claim.
//! Defaults to the virtual "CABLE Input" endpoint and never changes the system default.

#[cfg(not(windows))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    Err("seek-loopback requires Windows WASAPI".into())
}

#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    windows::run()
}

#[cfg(windows)]
mod windows {
    use audiolib_audio::{
        AudioCommand, AudioEngine, AudioHandle, PlaybackConfig, SourceOpener,
        register_stream_source, unregister_stream_source,
    };
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use serde_json::{Value, json};
    use std::{
        fs,
        io::{Cursor, Write},
        path::{Path, PathBuf},
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering},
        },
        thread,
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };

    type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
    const ERROR_NAMES: [&str; 15] = [
        "device_busy",
        "device_changed",
        "device_not_available",
        "host_unavailable",
        "invalid_input",
        "permission_denied",
        "realtime_denied",
        "resource_exhausted",
        "stream_invalidated",
        "unsupported_config",
        "unsupported_operation",
        "xrun",
        "backend_error",
        "other",
        "unknown",
    ];

    fn error_index(kind: cpal::ErrorKind) -> usize {
        match kind {
            cpal::ErrorKind::DeviceBusy => 0,
            cpal::ErrorKind::DeviceChanged => 1,
            cpal::ErrorKind::DeviceNotAvailable => 2,
            cpal::ErrorKind::HostUnavailable => 3,
            cpal::ErrorKind::InvalidInput => 4,
            cpal::ErrorKind::PermissionDenied => 5,
            cpal::ErrorKind::RealtimeDenied => 6,
            cpal::ErrorKind::ResourceExhausted => 7,
            cpal::ErrorKind::StreamInvalidated => 8,
            cpal::ErrorKind::UnsupportedConfig => 9,
            cpal::ErrorKind::UnsupportedOperation => 10,
            cpal::ErrorKind::Xrun => 11,
            cpal::ErrorKind::BackendError => 12,
            cpal::ErrorKind::Other => 13,
            _ => 14,
        }
    }

    struct Registration(PathBuf);
    impl Drop for Registration {
        fn drop(&mut self) {
            unregister_stream_source(&self.0);
        }
    }

    struct Capture {
        samples: Box<[AtomicU32]>,
        count: AtomicUsize,
        overflow: AtomicBool,
        failed: AtomicBool,
        errors: [AtomicUsize; 15],
        first_error_sample: [AtomicUsize; 15],
        clock: Instant,
    }

    fn wav(path: &Path, rate: u32, channels: u16, samples: &[f32]) -> Result<()> {
        let data_size = u32::try_from(samples.len() * 4)?;
        let mut file = std::io::BufWriter::new(fs::File::create_new(path)?);
        file.write_all(b"RIFF")?;
        file.write_all(&(data_size + 36).to_le_bytes())?;
        file.write_all(b"WAVEfmt ")?;
        file.write_all(&16_u32.to_le_bytes())?;
        file.write_all(&3_u16.to_le_bytes())?;
        file.write_all(&channels.to_le_bytes())?;
        file.write_all(&rate.to_le_bytes())?;
        file.write_all(&(rate * u32::from(channels) * 4).to_le_bytes())?;
        file.write_all(&(channels * 4).to_le_bytes())?;
        file.write_all(&32_u16.to_le_bytes())?;
        file.write_all(b"data")?;
        file.write_all(&data_size.to_le_bytes())?;
        for sample in samples {
            file.write_all(&sample.to_le_bytes())?;
        }
        file.flush()?;
        Ok(())
    }

    fn metrics(samples: &[f32], channels: usize) -> Value {
        let mut peak = 0.0_f32;
        let mut step = 0.0_f32;
        let mut clipped = 0_usize;
        let mut nonfinite = 0_usize;
        for (index, &sample) in samples.iter().enumerate() {
            if !sample.is_finite() {
                nonfinite += 1;
                continue;
            }
            peak = peak.max(sample.abs());
            clipped += usize::from(sample.abs() >= 1.0);
            if index >= channels {
                step = step.max((sample - samples[index - channels]).abs());
            }
        }
        json!({"sample_count":samples.len(),"peak":peak,"max_adjacent_step":step,
            "clipped_samples":clipped,"nonfinite_samples":nonfinite})
    }

    fn command(
        handle: &AudioHandle,
        capture: &Capture,
        events: &mut Vec<Value>,
        name: &str,
        command: AudioCommand,
        settle_ms: u64,
    ) -> Result<()> {
        let started = Instant::now();
        let begin = capture.count.load(Ordering::Acquire);
        let status = handle.send_and_wait(command, Duration::from_secs(10))?;
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        thread::sleep(Duration::from_millis(settle_ms));
        events.push(json!({"command":name,"started_ms":started.duration_since(capture.clock).as_secs_f64()*1000.0,
            "acknowledgment_ms":elapsed,"capture_begin":begin,
            "capture_end":capture.count.load(Ordering::Acquire),"status":status}));
        Ok(())
    }

    /// Capture the explicitly selected endpoint and write only owned diagnostic artifacts.
    pub(super) fn run() -> Result<()> {
        let args: Vec<_> = std::env::args().skip(1).collect();
        let value = |flag| {
            args.iter()
                .position(|arg| arg == flag)
                .and_then(|i| args.get(i + 1))
        };
        let needle = value("--device").map_or("CABLE Input", String::as_str);
        if needle.trim().is_empty() {
            return Err("--device requires an explicit nonempty endpoint substring".into());
        }
        let delay_ms = value("--delay-ms")
            .map(|v| v.parse::<u64>())
            .transpose()?
            .unwrap_or(200);
        if delay_ms > 500 {
            return Err("--delay-ms must be in 0..=500 to keep capture bounded".into());
        }
        let volume = value("--volume")
            .map(|v| v.parse::<f32>())
            .transpose()?
            .unwrap_or(0.3);
        if !volume.is_finite() || !(0.0 < volume && volume <= 1.0) {
            return Err("--volume must be in (0,1]".into());
        }
        let host = cpal::host_from_id(cpal::HostId::Wasapi)?;
        let mut matches = host.output_devices()?.filter(|device| {
            device
                .description()
                .is_ok_and(|d| d.name().to_lowercase().contains(&needle.to_lowercase()))
        });
        let device = matches
            .next()
            .ok_or("No matching output endpoint; no default-device fallback")?;
        if matches.next().is_some() {
            return Err(
                "Multiple matching endpoints; use a more specific --device substring".into(),
            );
        }
        let description = device.description()?.name().to_owned();
        let device_id = device.id()?.to_string();
        let supported = device.default_output_config()?;
        if supported.sample_format() != cpal::SampleFormat::F32 {
            return Err(
                "Selected endpoint mix format is not f32; diagnostic does not convert capture"
                    .into(),
            );
        }
        let config = supported.config();
        let rate = config.sample_rate;
        if value("--expected-rate")
            .map(|v| v.parse::<u32>())
            .transpose()?
            .is_some_and(|expected| expected != rate)
        {
            return Err(
                "Endpoint mix rate differs from --expected-rate; no settings changed".into(),
            );
        }
        let channels = usize::from(config.channels);
        let capacity = rate as usize * channels * 12;
        if channels < 2 || capacity > 16_000_000 {
            return Err(
                "Capture requires stereo or wider and at most 16M samples in 12 seconds".into(),
            );
        }
        let output = value("--output").map_or_else(
            || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.rust-validation/seek-device"),
            PathBuf::from,
        );
        fs::create_dir_all(&output)?;
        let result_dir = output.join(format!(
            "loopback-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
        ));
        fs::create_dir(&result_dir)?;
        let fixture = tempfile::tempdir()?;
        let fixture_path = fixture.path().join("dc.wav");
        // Two long constant regions avoid natural edges during the short test windows.
        let pcm: Vec<_> = (0..rate * 4)
            .flat_map(|frame| {
                let value = if frame < rate * 2 { 0.15_f32 } else { -0.15 };
                [value, value]
            })
            .collect();
        wav(&fixture_path, rate, 2, &pcm)?;
        let bytes: Arc<[u8]> = fs::read(&fixture_path)?.into();
        let virtual_path = fixture.path().join("streamed.wav");
        let opens = Arc::new(AtomicUsize::new(0));
        let delayed = Arc::new(AtomicUsize::new(0));
        let arm = Arc::new(AtomicBool::new(false));
        let opener: SourceOpener = {
            let (opens, delayed, arm) = (opens.clone(), delayed.clone(), arm.clone());
            Arc::new(move || {
                opens.fetch_add(1, Ordering::Relaxed);
                if arm.load(Ordering::Acquire) {
                    delayed.fetch_add(1, Ordering::Relaxed);
                    thread::sleep(Duration::from_millis(delay_ms));
                }
                Ok((Box::new(Cursor::new(bytes.clone())), bytes.len() as u64))
            })
        };
        register_stream_source(&virtual_path, opener);
        let _registration = Registration(virtual_path.clone());
        let capture = Arc::new(Capture {
            samples: (0..capacity).map(|_| AtomicU32::new(0)).collect(),
            count: AtomicUsize::new(0),
            overflow: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            errors: std::array::from_fn(|_| AtomicUsize::new(0)),
            first_error_sample: std::array::from_fn(|_| AtomicUsize::new(usize::MAX)),
            clock: Instant::now(),
        });
        let (input, failure) = (capture.clone(), capture.clone());
        // CPAL WASAPI enables AUDCLNT_STREAMFLAGS_LOOPBACK for this render endpoint.
        // Callback work is bounded atomic stores into preallocated memory only.
        let stream = device.build_input_stream::<f32, _, _>(
            config,
            move |data, _| {
                let begin = input.count.load(Ordering::Relaxed);
                let count = data.len().min(input.samples.len() - begin);
                for (slot, sample) in input.samples[begin..begin + count].iter().zip(data) {
                    slot.store(sample.to_bits(), Ordering::Relaxed);
                }
                input.count.store(begin + count, Ordering::Release);
                if count != data.len() {
                    input.overflow.store(true, Ordering::Relaxed);
                }
            },
            move |error| {
                let index = error_index(error.kind());
                failure.errors[index].fetch_add(1, Ordering::Relaxed);
                let _ = failure.first_error_sample[index].compare_exchange(
                    usize::MAX,
                    failure.count.load(Ordering::Relaxed),
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                );
                // CPAL documents these three as recoverable stream notifications.
                // Xrun remains visible in the report and is not a clean capture.
                if !matches!(
                    error.kind(),
                    cpal::ErrorKind::DeviceChanged
                        | cpal::ErrorKind::RealtimeDenied
                        | cpal::ErrorKind::Xrun
                ) {
                    failure.failed.store(true, Ordering::Relaxed);
                }
            },
            None,
        )?;
        let engine = AudioEngine::new(PlaybackConfig {
            device_id: Some(device_id.clone()),
            volume,
            ..Default::default()
        })?;
        let handle = engine.handle();
        stream.play()?;
        let clock = capture.clock;
        let mut events = Vec::new();
        command(
            &handle,
            &capture,
            &mut events,
            "Load initial",
            AudioCommand::Load(virtual_path.clone()),
            0,
        )?;
        command(
            &handle,
            &capture,
            &mut events,
            "Play",
            AudioCommand::Play,
            200,
        )?;
        arm.store(true, Ordering::Release);
        for (name, command_value, settle) in [
            ("Seek playing negative", AudioCommand::SeekAndPlay(2.5), 120),
            ("Pause", AudioCommand::Pause, 80),
            ("Seek paused positive", AudioCommand::SeekAndPlay(0.5), 100),
        ] {
            command(&handle, &capture, &mut events, name, command_value, settle)?;
        }
        let mut pending = Vec::new();
        for target in [2.5, 0.25, 2.5] {
            pending.push((
                Instant::now(),
                capture.count.load(Ordering::Acquire),
                target,
                handle.enqueue_with_ack(AudioCommand::SeekAndPlay(target))?,
            ));
        }
        for (started, begin, target, completion) in pending {
            let status = completion.wait(Duration::from_secs(10))?;
            events.push(json!({"command":"Queued rapid SeekAndPlay","target":target,
                "started_ms":started.duration_since(clock).as_secs_f64()*1000.0,
                "acknowledgment_ms":started.elapsed().as_secs_f64()*1000.0,
                "capture_begin":begin,"capture_end":capture.count.load(Ordering::Acquire),"status":status}));
        }
        thread::sleep(Duration::from_millis(100));
        for (name, command_value, settle) in [
            ("Load during playback", AudioCommand::Load(virtual_path), 50),
            ("Play after load", AudioCommand::Play, 100),
            ("Stop", AudioCommand::Stop, 150),
        ] {
            command(&handle, &capture, &mut events, name, command_value, settle)?;
        }
        let final_status = handle.status();
        // Keep the render endpoint alive until capture has stopped, so dropping
        // the last output stream cannot create a spurious capture teardown error.
        stream.pause()?;
        drop(stream);
        drop(engine);
        let count = capture.count.load(Ordering::Acquire);
        let samples: Vec<_> = capture.samples[..count]
            .iter()
            .map(|v| f32::from_bits(v.load(Ordering::Relaxed)))
            .collect();
        for event in &mut events {
            let begin = event["capture_begin"].as_u64().unwrap() as usize;
            let end = event["capture_end"].as_u64().unwrap() as usize;
            event["pcm"] = metrics(&samples[begin.saturating_sub(channels)..end], channels);
        }
        let errors: serde_json::Map<_, _> = ERROR_NAMES
            .into_iter()
            .enumerate()
            .map(|(i, name)| {
                let first = capture.first_error_sample[i].load(Ordering::Relaxed);
                (
                    name.to_owned(),
                    json!({"count":capture.errors[i].load(Ordering::Relaxed),
                "first_sample":(first != usize::MAX).then_some(first)}),
                )
            })
            .collect();
        let report = json!({"kind":"WASAPI render-endpoint loopback; not physical ADC or listening",
            "device":description,"device_id":device_id,"sample_rate":rate,"channels":channels,
            "fixture_seconds":4,"fixture_dc":[0.15,-0.15],"volume":volume,"expected_peak":0.15 * volume,
            "opener_delay_ms":delay_ms,"source_opens":opens.load(Ordering::Relaxed),
            "delayed_opens":delayed.load(Ordering::Relaxed),"elapsed_ms":clock.elapsed().as_secs_f64()*1000.0,
            "capture_overflow":capture.overflow.load(Ordering::Relaxed),"capture_failed":capture.failed.load(Ordering::Relaxed),
            "capture_error_kinds":errors,"capture_had_xrun":capture.errors[11].load(Ordering::Relaxed)>0,
            "pcm":metrics(&samples,channels),"final_status":final_status,"commands":events});
        wav(
            &result_dir.join("capture.wav"),
            rate,
            config.channels,
            &samples,
        )?;
        fs::write(
            result_dir.join("report.json"),
            serde_json::to_vec_pretty(&report)?,
        )?;
        println!("{}", result_dir.display());
        if count == 0
            || !samples.iter().any(|v| v.abs() > 0.15 * volume * 0.1)
            || capture.failed.load(Ordering::Relaxed)
            || capture.overflow.load(Ordering::Relaxed)
        {
            return Err(
                "Capture was silent, failed, or exceeded its bound; inspect report.json".into(),
            );
        }
        Ok(())
    }
}
