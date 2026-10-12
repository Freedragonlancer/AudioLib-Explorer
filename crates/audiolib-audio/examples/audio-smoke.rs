// crates/audiolib-audio/examples/audio-smoke.rs
//! Defaults to read-only enumeration. --open-silence explicitly opens a short all-zero stream.
use audiolib_audio::*;
use std::{
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |flag: &str| {
        args.iter()
            .position(|arg| arg == flag)
            .and_then(|index| args.get(index + 1))
            .cloned()
    };
    let backend = if value("--backend").as_deref() == Some("asio") {
        AudioBackend::Asio
    } else {
        AudioBackend::Wasapi
    };
    let devices = list_devices(backend)?;
    println!("{}", serde_json::to_string_pretty(&devices)?);
    if let Some(path) = value("--probe") {
        println!(
            "probe={}",
            serde_json::to_string_pretty(&probe(&PathBuf::from(path))?)?
        );
    }
    if !args.iter().any(|arg| arg == "--open-silence") {
        println!(
            "Enumeration only. Add --open-silence to open an all-zero output stream; no test tone is generated."
        );
        return Ok(());
    }
    let rate = value("--rate")
        .map(|value| value.parse::<u32>())
        .transpose()?
        .unwrap_or(192000);
    let buffer = value("--buffer")
        .map(|value| value.parse::<u32>())
        .transpose()?
        .unwrap_or(0);
    let pair = value("--output-pair")
        .map(|value| value.parse::<u16>())
        .transpose()?
        .unwrap_or(0);
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("silent.wav");
    let data_size = rate / 2 * 2 * 4;
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(data_size + 36).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&3_u16.to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&rate.to_le_bytes());
    wav.extend_from_slice(&(rate * 8).to_le_bytes());
    wav.extend_from_slice(&8_u16.to_le_bytes());
    wav.extend_from_slice(&32_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_size.to_le_bytes());
    wav.resize(wav.len() + data_size as usize, 0);
    fs::write(&path, wav)?;
    let config = PlaybackConfig {
        backend,
        device_id: value("--device"),
        buffer_frames: buffer,
        output_pair: pair,
        rate_policy: if args.iter().any(|arg| arg == "--native") {
            RatePolicy::Native
        } else {
            RatePolicy::DeviceDefault
        },
        ..Default::default()
    };
    let engine = AudioEngine::new(config)?;
    let handle = engine.handle();
    let start = Instant::now();
    let ready = handle.send_and_wait(AudioCommand::Load(path), Duration::from_secs(15))?;
    println!(
        "prepare_ms={:.3} status={}",
        start.elapsed().as_secs_f64() * 1000.0,
        serde_json::to_string(&ready)?
    );
    let start = Instant::now();
    let playing = handle.send_and_wait(AudioCommand::Play, Duration::from_secs(15))?;
    println!(
        "cold_open_start_ms={:.3} status={}",
        start.elapsed().as_secs_f64() * 1000.0,
        serde_json::to_string(&playing)?
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let status = handle.status();
        if matches!(
            status.state,
            PlaybackState::Ended | PlaybackState::Error | PlaybackState::Paused
        ) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    println!("final_status={}", serde_json::to_string(&handle.status())?);
    handle.send_and_wait(AudioCommand::Stop, Duration::from_secs(15))?;
    let start = Instant::now();
    let restarted = handle.send_and_wait(AudioCommand::Play, Duration::from_secs(15))?;
    println!(
        "warm_start_ms={:.3} status={}",
        start.elapsed().as_secs_f64() * 1000.0,
        serde_json::to_string(&restarted)?
    );
    std::thread::sleep(Duration::from_millis(50));
    handle.send_and_wait(AudioCommand::Stop, Duration::from_secs(15))?;
    Ok(())
}
