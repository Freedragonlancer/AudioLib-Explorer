// crates/audiolib-audio/examples/first-sound-loopback/main.rs
//! Opt-in WASAPI endpoint loopback measurement of time to first audible frame (play
//! request to the first non-silent captured frame), seek discontinuity (second
//! difference against the tone's own curvature) and xruns/underruns, optionally while
//! list Waveforms are computed concurrently (`--load ondemand|backfill`). No physical
//! recording or listening claim. Defaults to the virtual "CABLE Input" endpoint and
//! never changes the system default.
//!
//! ```text
//! cargo run --release -p audiolib-audio --example first-sound-loopback -- \
//!     --make-fixtures DIR [--count 200]           # list Waveform load files (96 kHz/24-bit, 30 s)
//! cargo run --release -p audiolib-audio --example first-sound-loopback -- \
//!     [--device "CABLE Input"] [--runs 24] [--load none|ondemand|backfill] \
//!     [--policy app|stress] [--files DIR] [--tone-hz 60] [--volume 0.3] [--seed 1]
//! cargo run --release -p audiolib-audio --example first-sound-loopback -- \
//!     --load-only SECONDS --load ondemand|backfill --policy stress --files DIR
//! ```
//!
//! `--load-only` runs the load without audio, e.g. next to `seek-loopback`.

#[cfg(not(windows))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    Err("first-sound-loopback requires Windows WASAPI".into())
}

#[cfg(windows)]
mod load;

#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    windows::run()
}

#[cfg(windows)]
mod windows {
    use super::load::{Kind, Loader, Policy};
    use audiolib_audio::{AudioCommand, AudioEngine, PlaybackConfig};
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use serde_json::{Value, json};
    use std::{
        fs,
        io::Write,
        path::{Path, PathBuf},
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
        },
        thread,
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };

    type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
    /// A captured sample at or above this level counts as sound (about -80 dBFS).
    const AUDIBLE: f32 = 1.0e-4;
    const CALLBACKS: usize = 200_000;

    struct Capture {
        samples: Box<[AtomicU32]>,
        count: AtomicUsize,
        overflow: AtomicBool,
        failed: AtomicBool,
        xruns: AtomicUsize,
        errors: AtomicUsize,
        /// Per capture callback: sample count after it and its time (ns on `clock`).
        callback_end: Box<[AtomicUsize]>,
        callback_ns: Box<[AtomicU64]>,
        callbacks: AtomicUsize,
        clock: Instant,
    }

    impl Capture {
        /// Time (ms on `clock`) at which sample `index` was rendered: its callback's
        /// arrival minus the frames that followed it in that packet.
        fn time_of(&self, index: usize, frames_per_ms: f64, channels: usize) -> Option<f64> {
            let count = self.callbacks.load(Ordering::Acquire).min(CALLBACKS);
            let (mut low, mut high) = (0, count);
            while low < high {
                let mid = (low + high) / 2;
                if self.callback_end[mid].load(Ordering::Relaxed) > index {
                    high = mid;
                } else {
                    low = mid + 1;
                }
            }
            (low < count).then(|| {
                let end = self.callback_end[low].load(Ordering::Relaxed);
                let at = self.callback_ns[low].load(Ordering::Relaxed) as f64 / 1.0e6;
                at - ((end - index) / channels) as f64 / frames_per_ms
            })
        }

        fn sample(&self, index: usize) -> f32 {
            f32::from_bits(self.samples[index].load(Ordering::Relaxed))
        }
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

    /// `count` 96 kHz/24-bit stereo 30 s noise files (about 17 MB each).
    fn make_fixtures(dir: &Path, count: usize) -> Result<()> {
        fs::create_dir_all(dir)?;
        let (rate, channels, seconds) = (96_000_u32, 2_u16, 30_u32);
        let data_size = rate * seconds * u32::from(channels) * 3;
        let mut state = 0x1234_5678_u32;
        for index in 0..count {
            let path = dir.join(format!("load-{index:04}.wav"));
            if path.exists() {
                continue;
            }
            let mut file = std::io::BufWriter::new(fs::File::create_new(&path)?);
            file.write_all(b"RIFF")?;
            file.write_all(&(data_size + 36).to_le_bytes())?;
            file.write_all(b"WAVEfmt ")?;
            file.write_all(&16_u32.to_le_bytes())?;
            file.write_all(&1_u16.to_le_bytes())?;
            file.write_all(&channels.to_le_bytes())?;
            file.write_all(&rate.to_le_bytes())?;
            file.write_all(&(rate * u32::from(channels) * 3).to_le_bytes())?;
            file.write_all(&(channels * 3).to_le_bytes())?;
            file.write_all(&24_u16.to_le_bytes())?;
            file.write_all(b"data")?;
            file.write_all(&data_size.to_le_bytes())?;
            let mut block = Vec::with_capacity(rate as usize * 6);
            for _ in 0..seconds {
                block.clear();
                for _ in 0..rate * u32::from(channels) {
                    state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    // Quarter scale noise, 24-bit little endian.
                    let value = ((state >> 8) as i32 - (1 << 23)) / 4;
                    block.extend_from_slice(&value.to_le_bytes()[..3]);
                }
                file.write_all(&block)?;
            }
            file.flush()?;
        }
        println!("{count} fixture files in {}", dir.display());
        Ok(())
    }

    fn wav_files(dir: &Path) -> Result<Vec<PathBuf>> {
        let mut files: Vec<_> = fs::read_dir(dir)?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                path.extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("wav"))
            })
            .collect();
        files.sort();
        Ok(files)
    }

    /// Nearest-rank percentile of sorted values.
    fn percentile(sorted: &[f64], p: f64) -> Option<f64> {
        if sorted.is_empty() {
            return None;
        }
        let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
        Some(sorted[rank.clamp(1, sorted.len()) - 1])
    }

    fn summary(values: &[f64]) -> Value {
        let mut sorted = values.to_vec();
        sorted.sort_by(f64::total_cmp);
        json!({"n": sorted.len(), "min": sorted.first(), "median": percentile(&sorted, 50.0),
            "p95": percentile(&sorted, 95.0), "max": sorted.last()})
    }

    /// Largest |second difference| (and |first difference|) in samples `[begin, end)`.
    fn curvature(capture: &Capture, begin: usize, end: usize, channels: usize) -> (f32, f32) {
        let (mut second, mut step) = (0.0_f32, 0.0_f32);
        for index in begin.max(2 * channels)..end {
            let (a, b, c) = (
                capture.sample(index),
                capture.sample(index - channels),
                capture.sample(index - 2 * channels),
            );
            if a.is_finite() && b.is_finite() && c.is_finite() {
                step = step.max((a - b).abs());
                second = second.max((a - 2.0 * b + c).abs());
            }
        }
        (second, step)
    }

    fn parse_load(args: &[String]) -> Result<(Option<Kind>, Policy)> {
        let value = |flag: &str| {
            args.iter()
                .position(|arg| arg == flag)
                .and_then(|i| args.get(i + 1))
        };
        let kind = match value("--load").map_or("none", String::as_str) {
            "none" => None,
            "ondemand" => Some(Kind::OnDemand),
            "backfill" => Some(Kind::Backfill),
            _ => return Err("--load must be none, ondemand or backfill".into()),
        };
        let policy = match value("--policy").map_or("app", String::as_str) {
            "app" => Policy::App,
            "stress" => Policy::Stress,
            _ => return Err("--policy must be app or stress".into()),
        };
        Ok((kind, policy))
    }

    pub(super) fn run() -> Result<()> {
        let args: Vec<_> = std::env::args().skip(1).collect();
        let value = |flag: &str| {
            args.iter()
                .position(|arg| arg == flag)
                .and_then(|i| args.get(i + 1))
        };
        if let Some(dir) = value("--make-fixtures") {
            let count = value("--count")
                .map(|v| v.parse::<usize>())
                .transpose()?
                .unwrap_or(200);
            return make_fixtures(Path::new(dir), count);
        }
        let (kind, policy) = parse_load(&args)?;
        let files = match (kind, value("--files")) {
            (None, _) => Vec::new(),
            (Some(_), Some(dir)) => wav_files(Path::new(dir))?,
            (Some(_), None) => return Err("--load needs --files DIR".into()),
        };
        if let Some(seconds) = value("--load-only") {
            let seconds = seconds.parse::<u64>()?;
            let kind = kind.ok_or("--load-only needs --load ondemand|backfill")?;
            let clock = Instant::now();
            let loader = Loader::start(kind, policy, files, clock)?;
            thread::sleep(Duration::from_secs(seconds));
            let elapsed = clock.elapsed().as_secs_f64() * 1000.0;
            println!("{}", loader.finish(elapsed).1);
            return Ok(());
        }
        let needle = value("--device").map_or("CABLE Input", String::as_str);
        if needle.trim().is_empty() {
            return Err("--device requires an explicit nonempty endpoint substring".into());
        }
        let runs = value("--runs")
            .map(|v| v.parse::<usize>())
            .transpose()?
            .unwrap_or(24);
        if !(1..=200).contains(&runs) {
            return Err("--runs must be in 1..=200".into());
        }
        let volume = value("--volume")
            .map(|v| v.parse::<f32>())
            .transpose()?
            .unwrap_or(0.3);
        if !volume.is_finite() || !(0.0 < volume && volume <= 1.0) {
            return Err("--volume must be in (0,1]".into());
        }
        let tone_hz = value("--tone-hz")
            .map(|v| v.parse::<f32>())
            .transpose()?
            .unwrap_or(60.0);
        if !tone_hz.is_finite() || !(1.0..=2000.0).contains(&tone_hz) {
            return Err("--tone-hz must be in 1..=2000".into());
        }
        let mut seed = value("--seed")
            .map(|v| v.parse::<u32>())
            .transpose()?
            .unwrap_or(1);
        let mut random = move |range: u32| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) % range
        };

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
            return Err("Selected endpoint mix format is not f32".into());
        }
        let config = supported.config();
        let rate = config.sample_rate;
        let channels = usize::from(config.channels);
        let frames_per_ms = f64::from(rate) / 1000.0;
        // Bounded capture: up to 9 s per run plus margin.
        let capacity = rate as usize * channels * (runs * 9 + 10);
        if channels < 2 || capacity > 120_000_000 {
            return Err("Capture requires stereo or wider and a bounded run count".into());
        }
        let source_rate = value("--source-rate")
            .map(|v| v.parse::<u32>())
            .transpose()?
            .unwrap_or(rate);
        if !(8_000..=384_000).contains(&source_rate) {
            return Err("--source-rate must be in 8000..=384000".into());
        }
        let output = value("--output").map_or_else(
            || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.rust-validation/first-sound"),
            PathBuf::from,
        );
        fs::create_dir_all(&output)?;
        let fixture = tempfile::tempdir()?;
        let tone_path = fixture.path().join("tone.wav");
        let amplitude = 0.5_f32;
        let tone: Vec<_> = (0..source_rate * 20)
            .flat_map(|frame| {
                let value = amplitude
                    * (std::f64::consts::TAU * f64::from(tone_hz) * f64::from(frame)
                        / f64::from(source_rate))
                    .sin() as f32;
                [value, value]
            })
            .collect();
        wav(&tone_path, source_rate, 2, &tone)?;

        let capture = Arc::new(Capture {
            samples: (0..capacity).map(|_| AtomicU32::new(0)).collect(),
            count: AtomicUsize::new(0),
            overflow: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            xruns: AtomicUsize::new(0),
            errors: AtomicUsize::new(0),
            callback_end: (0..CALLBACKS).map(|_| AtomicUsize::new(0)).collect(),
            callback_ns: (0..CALLBACKS).map(|_| AtomicU64::new(0)).collect(),
            callbacks: AtomicUsize::new(0),
            clock: Instant::now(),
        });
        let (input, failure) = (capture.clone(), capture.clone());
        // CPAL WASAPI enables AUDCLNT_STREAMFLAGS_LOOPBACK for this render endpoint.
        // Callback work is bounded atomic stores into preallocated memory only.
        let stream = device.build_input_stream::<f32, _, _>(
            config,
            move |data, _| {
                let now = input.clock.elapsed().as_nanos() as u64;
                let begin = input.count.load(Ordering::Relaxed);
                let count = data.len().min(input.samples.len() - begin);
                for (slot, sample) in input.samples[begin..begin + count].iter().zip(data) {
                    slot.store(sample.to_bits(), Ordering::Relaxed);
                }
                let callback = input.callbacks.load(Ordering::Relaxed);
                if callback < CALLBACKS {
                    input.callback_end[callback].store(begin + count, Ordering::Relaxed);
                    input.callback_ns[callback].store(now, Ordering::Relaxed);
                }
                input.count.store(begin + count, Ordering::Release);
                input.callbacks.store(callback + 1, Ordering::Release);
                if count != data.len() {
                    input.overflow.store(true, Ordering::Relaxed);
                }
            },
            move |error| {
                failure.errors.fetch_add(1, Ordering::Relaxed);
                match error.kind() {
                    cpal::ErrorKind::Xrun => {
                        failure.xruns.fetch_add(1, Ordering::Relaxed);
                    }
                    cpal::ErrorKind::DeviceChanged | cpal::ErrorKind::RealtimeDenied => {}
                    _ => failure.failed.store(true, Ordering::Relaxed),
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
        let ms = |at: Instant| at.duration_since(clock).as_secs_f64() * 1000.0;
        thread::sleep(Duration::from_millis(300));
        // Warm-up (not counted): first device open and file cache.
        handle.send_and_wait(
            AudioCommand::Load(tone_path.clone()),
            Duration::from_secs(10),
        )?;
        handle.send_and_wait(AudioCommand::Play, Duration::from_secs(10))?;
        thread::sleep(Duration::from_millis(300));
        handle.send_and_wait(AudioCommand::Stop, Duration::from_secs(10))?;
        let loader = match kind {
            Some(kind) => Some(Loader::start(kind, policy, files, clock)?),
            None => None,
        };
        let natural = amplitude * volume * (std::f32::consts::TAU * tone_hz / rate as f32).powi(2);
        // Stream start (and the warm-up) may report a capture discontinuity; count
        // xruns during the measured runs separately.
        let xruns_before_runs = capture.xruns.load(Ordering::Relaxed);
        let mut results = Vec::new();
        for run in 0..runs {
            // Idle gap: lets an app-policy loader resume, so a decode can be in flight
            // when the next play request arrives.
            thread::sleep(Duration::from_millis(500 + u64::from(random(4000))));
            let begin = capture.count.load(Ordering::Acquire);
            let requested = Instant::now();
            if let Some(loader) = &loader {
                loader.audition_started();
            }
            handle.send_and_wait(
                AudioCommand::Load(tone_path.clone()),
                Duration::from_secs(10),
            )?;
            let loaded = Instant::now();
            handle.send_and_wait(AudioCommand::Play, Duration::from_secs(10))?;
            let played = Instant::now();
            let deadline = requested + Duration::from_secs(3);
            let mut scanned = begin;
            let mut first = None;
            while first.is_none() && Instant::now() < deadline {
                let count = capture.count.load(Ordering::Acquire);
                first = (scanned..count).find(|&i| capture.sample(i).abs() >= AUDIBLE);
                scanned = count;
                if first.is_none() {
                    thread::sleep(Duration::from_millis(1));
                }
            }
            let first_ms = first.and_then(|index| capture.time_of(index, frames_per_ms, channels));
            // Seek 600 ms after the first sound to a random position.
            thread::sleep(Duration::from_millis(600));
            let seek_begin = capture.count.load(Ordering::Acquire);
            let target = 1.0 + f64::from(random(15_000)) / 1000.0;
            let seek_requested = Instant::now();
            if let Some(loader) = &loader {
                loader.audition_started();
            }
            handle.send_and_wait(AudioCommand::SeekAndPlay(target), Duration::from_secs(10))?;
            let seek_ack = seek_requested.elapsed().as_secs_f64() * 1000.0;
            thread::sleep(Duration::from_millis(400));
            let seek_end = capture.count.load(Ordering::Acquire);
            let status = handle.status();
            let stop_begin = capture.count.load(Ordering::Acquire);
            handle.send_and_wait(AudioCommand::Stop, Duration::from_secs(10))?;
            if let Some(loader) = &loader {
                loader.audition_stopped();
            }
            let (seek_second, seek_step) = curvature(&capture, seek_begin, seek_end, channels);
            let (play_second, _) = match first {
                Some(first) => curvature(&capture, first, stop_begin, channels),
                None => (f32::NAN, f32::NAN),
            };
            results.push(json!({
                "run": run, "requested_ms": ms(requested),
                "load_ack_ms": loaded.duration_since(requested).as_secs_f64() * 1000.0,
                "play_ack_ms": played.duration_since(requested).as_secs_f64() * 1000.0,
                "first_sound_ms": first_ms.map(|at| at - ms(requested)),
                "seek_target": target, "seek_ack_ms": seek_ack,
                "seek_max_second_difference": seek_second, "seek_max_step": seek_step,
                "play_max_second_difference": play_second,
                "underruns": status.underruns, "state": format!("{:?}", status.state),
            }));
        }
        let elapsed = clock.elapsed().as_secs_f64() * 1000.0;
        let (busy, load_summary) = match loader {
            Some(loader) => loader.finish(elapsed),
            None => (Vec::new(), Value::Null),
        };
        stream.pause()?;
        drop(stream);
        drop(engine);
        // A decode running when the play request arrived.
        for result in &mut results {
            let at = result["requested_ms"].as_f64().unwrap();
            result["decode_in_flight"] = json!(busy.iter().any(|(b, e)| *b <= at && at <= *e));
        }
        let column =
            |key: &str| -> Vec<f64> { results.iter().filter_map(|r| r[key].as_f64()).collect() };
        let first_sound = column("first_sound_ms");
        let missing = results.len() - first_sound.len();
        let in_flight: Vec<f64> = results
            .iter()
            .filter(|r| r["decode_in_flight"] == json!(true))
            .filter_map(|r| r["first_sound_ms"].as_f64())
            .collect();
        let report = json!({
            "kind": "WASAPI render-endpoint loopback; not physical ADC or listening",
            "device": description, "sample_rate": rate, "channels": channels,
            "source_rate": source_rate, "tone_hz": tone_hz, "volume": volume,
            "natural_max_second_difference": natural,
            "runs": runs, "load": load_summary,
            "first_sound_ms": summary(&first_sound), "first_sound_missing": missing,
            "first_sound_ms_with_decode_in_flight": summary(&in_flight),
            "load_ack_ms": summary(&column("load_ack_ms")),
            "play_ack_ms": summary(&column("play_ack_ms")),
            "seek_ack_ms": summary(&column("seek_ack_ms")),
            "seek_max_second_difference": summary(&column("seek_max_second_difference")),
            "play_max_second_difference": summary(&column("play_max_second_difference")),
            "underruns_total": results.iter().filter_map(|r| r["underruns"].as_u64()).sum::<u64>(),
            "capture_xruns": capture.xruns.load(Ordering::Relaxed),
            "capture_xruns_during_runs": capture.xruns.load(Ordering::Relaxed) - xruns_before_runs,
            "capture_errors": capture.errors.load(Ordering::Relaxed),
            "capture_overflow": capture.overflow.load(Ordering::Relaxed),
            "capture_failed": capture.failed.load(Ordering::Relaxed),
            "elapsed_ms": elapsed, "results": results,
        });
        let path = output.join(format!(
            "first-sound-{}-{}.json",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis()
        ));
        fs::write(&path, serde_json::to_vec_pretty(&report)?)?;
        println!("{}", path.display());
        if missing > 0 || capture.failed.load(Ordering::Relaxed) {
            return Err("Some runs produced no sound or capture failed; inspect the report".into());
        }
        Ok(())
    }
}
