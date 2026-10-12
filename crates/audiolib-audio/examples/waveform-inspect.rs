// crates/audiolib-audio/examples/waveform-inspect.rs
//! Read-only spectral diagnostics: waveform-inspect <WAV> [display columns].
//! Writes JSON to stdout; never opens audio output or changes the source file.
use audiolib_audio::{WaveformRequest, probe, waveform};
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args_os().skip(1);
    let path = PathBuf::from(arguments.next().ok_or("Expected a WAV path")?);
    let columns = arguments
        .next()
        .map(|value| value.to_string_lossy().parse::<u64>())
        .transpose()?
        .unwrap_or(1600)
        .clamp(64, 4096);
    let metadata = probe(&path)?;
    let samples_per_pixel = u32::try_from(metadata.frames.div_ceil(columns).max(1))?;
    let peaks = waveform(
        &path,
        WaveformRequest {
            samples_per_pixel,
            ..Default::default()
        },
    )?;
    println!("{}", serde_json::to_string(&peaks)?);
    Ok(())
}
