// crates/audiolib-audio/src/waveform.rs
use crate::decoder::Decoder;
use crate::spectral::{SpectralAnalyzer, spectral_color};
use audiolib_core::{Error, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    hash::{Hash, Hasher},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

/// Waveform algorithm/cache version; part of every cache key (and of the server's
/// waveform ETag), so a change invalidates cached peaks everywhere.
// Version 4 subtracts broadband energy and fades non-tonal peaks to black.
// Earlier concentration values and precomputed gray-noise colors are stale.
pub const CACHE_VERSION: u32 = 4;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct WaveformRequest {
    pub samples_per_pixel: u32,
    pub start_frame: u64,
    pub end_frame: Option<u64>,
    pub include_colors: bool,
}
impl Default for WaveformRequest {
    fn default() -> Self {
        Self {
            samples_per_pixel: 512,
            start_frame: 0,
            end_frame: None,
            include_colors: true,
        }
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WaveformPeak {
    pub min: f32,
    pub max: f32,
    pub rms: f32,
    pub color: Option<[u8; 3]>,
    /// Dominant source frequency, independent of audition Pitch and Rate.
    #[serde(default)]
    pub dominant_hz: f32,
    /// Dominant tonal energy after broadband-floor subtraction (0 = noise, 1 = tone).
    #[serde(default)]
    pub tonality: f32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WaveformData {
    pub sample_rate: u32,
    pub channels: u16,
    pub frames: u64,
    pub samples_per_pixel: u32,
    pub peaks: Vec<Vec<WaveformPeak>>,
}
#[derive(Serialize, Deserialize)]
struct Cache {
    version: u32,
    fingerprint: u64,
    waveform: WaveformData,
}

/// Stream per-channel peaks, retaining the final incomplete bucket and no full-file PCM.
pub fn waveform(path: &Path, request: WaveformRequest) -> Result<WaveformData> {
    waveform_with_cancel(path, request, &AtomicBool::new(false))
}
/// Cancellable variant for a latest-selection Waveform worker.
pub fn waveform_with_cancel(
    path: &Path,
    request: WaveformRequest,
    cancel: &AtomicBool,
) -> Result<WaveformData> {
    waveform_with_read_buffer(path, request, cancel, 0)
}
/// [`waveform_with_cancel`] reading a plain file through a `read_buffer`-byte buffer
/// (0 = unbuffered), for whole-file passes over slow network storage. The output is
/// identical to the unbuffered path.
pub fn waveform_with_read_buffer(
    path: &Path,
    request: WaveformRequest,
    cancel: &AtomicBool,
    read_buffer: usize,
) -> Result<WaveformData> {
    if request.samples_per_pixel == 0 {
        return Err(Error::Validation(
            "Waveform samples_per_pixel must be positive".into(),
        ));
    }
    waveform_from_decoder(
        Decoder::open_with_read_buffer(path, read_buffer)?,
        request,
        cancel,
    )
}
/// Whole-file summary of `buckets` columns (`samples_per_pixel = ceil(frames / buckets)`,
/// at least 1) from a single open: the frame count comes from the decoder's own header
/// parse, which stops at the `data` chunk, so trailing chunks (ID3, SMED, iXML, ...) are
/// never read. Identical to probing first and calling [`waveform_with_read_buffer`].
pub fn summary_waveform_with_read_buffer(
    path: &Path,
    buckets: u32,
    cancel: &AtomicBool,
    read_buffer: usize,
) -> Result<WaveformData> {
    let decoder = Decoder::open_with_read_buffer(path, read_buffer)?;
    let samples_per_pixel = u32::try_from(
        decoder
            .info
            .metadata
            .frames
            .div_ceil(u64::from(buckets.max(1))),
    )
    .unwrap_or(u32::MAX)
    .max(1);
    waveform_from_decoder(
        decoder,
        WaveformRequest {
            samples_per_pixel,
            start_frame: 0,
            end_frame: None,
            include_colors: false,
        },
        cancel,
    )
}
fn waveform_from_decoder(
    mut decoder: Decoder,
    request: WaveformRequest,
    cancel: &AtomicBool,
) -> Result<WaveformData> {
    let metadata = decoder.info.metadata.clone();
    let end = request
        .end_frame
        .unwrap_or(metadata.frames)
        .min(metadata.frames);
    let start = request.start_frame.min(end);
    let frames = end - start;
    let bins = frames.div_ceil(u64::from(request.samples_per_pixel));
    if bins.saturating_mul(u64::from(metadata.channels)) > 4_000_000 {
        return Err(Error::Validation(
            "Waveform request exceeds 4 million peaks; increase samples_per_pixel".into(),
        ));
    }
    decoder.seek(start)?;
    let channels = usize::from(metadata.channels);
    let mut peaks = vec![Vec::with_capacity(bins as usize); channels];
    let mut min = vec![f32::INFINITY; channels];
    let mut max = vec![f32::NEG_INFINITY; channels];
    let mut sum = vec![0.0_f64; channels];
    let mut analyzer = request
        .include_colors
        .then(|| SpectralAnalyzer::new(metadata.sample_rate, channels));
    let mut features = vec![vec![FeatureAccumulator::default(); bins as usize]; channels];
    let mut processed = 0_u64;
    let mut count = 0_u32;
    let mut remaining = frames;
    while remaining > 0 {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let samples = decoder.read(remaining.min(4096) as usize)?;
        if samples.is_empty() {
            return Err(Error::Audio(
                "Unexpected end of WAV while computing Waveform".into(),
            ));
        }
        remaining -= (samples.len() / channels) as u64;
        for frame in samples.chunks_exact(channels) {
            for channel in 0..channels {
                let sample = frame[channel];
                min[channel] = min[channel].min(sample);
                max[channel] = max[channel].max(sample);
                sum[channel] += f64::from(sample) * f64::from(sample);
            }
            processed += 1;
            if let Some(analyzer) = &mut analyzer {
                analyzer.push(frame);
                collect_features(
                    analyzer,
                    processed,
                    request.samples_per_pixel,
                    &mut features,
                );
            }
            count += 1;
            if count == request.samples_per_pixel {
                flush(&mut peaks, &mut min, &mut max, &mut sum, count);
                count = 0;
            }
        }
    }
    if count > 0 {
        flush(&mut peaks, &mut min, &mut max, &mut sum, count);
    }
    if let Some(analyzer) = &mut analyzer {
        let zero = vec![0.0; channels];
        for _ in 0..analyzer.size() / 2 + analyzer.hop() {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            processed += 1;
            analyzer.push(&zero);
            collect_features(
                analyzer,
                processed,
                request.samples_per_pixel,
                &mut features,
            );
        }
        for (channel_peaks, channel_features) in peaks.iter_mut().zip(features) {
            let known: Vec<_> = channel_features
                .iter()
                .enumerate()
                .filter(|(_, value)| value.count > 0)
                .collect();
            let mut cursor = 0;
            for (index, peak) in channel_peaks.iter_mut().enumerate() {
                if known.is_empty() {
                    continue;
                }
                while cursor + 1 < known.len()
                    && known[cursor + 1].0.abs_diff(index) <= known[cursor].0.abs_diff(index)
                {
                    cursor += 1;
                }
                let (_, feature) = known[cursor];
                peak.dominant_hz = if feature.weight > 1e-12 {
                    (feature.log_hz / feature.weight).exp() as f32
                } else {
                    0.0
                };
                peak.tonality = feature.tonality / feature.count as f32;
                peak.color = Some(spectral_color(peak.dominant_hz, peak.tonality, 1.0, 0.38));
            }
        }
    }
    Ok(WaveformData {
        sample_rate: metadata.sample_rate,
        channels: metadata.channels,
        frames,
        samples_per_pixel: request.samples_per_pixel,
        peaks,
    })
}

#[derive(Clone, Copy, Default)]
struct FeatureAccumulator {
    log_hz: f64,
    weight: f64,
    tonality: f32,
    count: u32,
}

fn collect_features(
    analyzer: &mut SpectralAnalyzer,
    processed: u64,
    samples_per_pixel: u32,
    features: &mut [Vec<FeatureAccumulator>],
) {
    if !processed.is_multiple_of(analyzer.hop() as u64) || processed < analyzer.size() as u64 / 2 {
        return;
    }
    let index = ((processed - analyzer.size() as u64 / 2) / u64::from(samples_per_pixel)) as usize;
    for (channel, features) in features.iter_mut().enumerate() {
        if let Some(value) = features.get_mut(index) {
            let feature = analyzer.analyze(channel);
            // Random noise frequencies must not pull a brief tone's hue away
            // from its actual pitch when multiple windows share one pixel.
            let weight = f64::from(feature.tonality).powi(2);
            value.log_hz += f64::from(feature.frequency.max(1.0)).ln() * weight;
            value.weight += weight;
            value.tonality += feature.tonality;
            value.count += 1;
        }
    }
}

fn flush(
    peaks: &mut [Vec<WaveformPeak>],
    min: &mut [f32],
    max: &mut [f32],
    sum: &mut [f64],
    count: u32,
) {
    for channel in 0..peaks.len() {
        peaks[channel].push(WaveformPeak {
            min: min[channel],
            max: max[channel],
            rms: (sum[channel] / f64::from(count)).sqrt() as f32,
            ..Default::default()
        });
        min[channel] = f32::INFINITY;
        max[channel] = f32::NEG_INFINITY;
        sum[channel] = 0.0;
    }
}

/// Disk cache keyed by source identity/size/mtime, zoom request, and algorithm version.
pub fn cached_waveform(
    path: &Path,
    cache_dir: &Path,
    request: WaveformRequest,
) -> Result<WaveformData> {
    cached_waveform_with_cancel(path, cache_dir, request, &AtomicBool::new(false))
}
/// Cancellable fingerprinted cache lookup/generation.
pub fn cached_waveform_with_cancel(
    path: &Path,
    cache_dir: &Path,
    request: WaveformRequest,
    cancel: &AtomicBool,
) -> Result<WaveformData> {
    if cancel.load(Ordering::Relaxed) {
        return Err(Error::Cancelled);
    }
    let metadata = fs::metadata(path)?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.canonicalize()?.hash(&mut hasher);
    metadata.len().hash(&mut hasher);
    metadata
        .modified()?
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .hash(&mut hasher);
    CACHE_VERSION.hash(&mut hasher);
    request.samples_per_pixel.hash(&mut hasher);
    request.start_frame.hash(&mut hasher);
    request.end_frame.hash(&mut hasher);
    request.include_colors.hash(&mut hasher);
    let fingerprint = hasher.finish();
    let cache_path = cache_dir.join(format!("{fingerprint:016x}.json"));
    if let Ok(bytes) = fs::read(&cache_path)
        && let Ok(mut cache) = serde_json::from_slice::<Cache>(&bytes)
        && cache.version == CACHE_VERSION
        && cache.fingerprint == fingerprint
    {
        // Frequency/tonality remain valid when only the display palette changes.
        // Refresh derived RGB without decoding the source or rebuilding its FFT.
        for peak in cache.waveform.peaks.iter_mut().flatten() {
            if peak.color.is_some() {
                peak.color = Some(spectral_color(peak.dominant_hz, peak.tonality, 1.0, 0.38));
            }
        }
        return Ok(cache.waveform);
    }
    let waveform = waveform_with_cancel(path, request, cancel)?;
    fs::create_dir_all(cache_dir)?;
    let temp = cache_dir.join(format!(
        "{fingerprint:016x}.{}.tmp",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let data = serde_json::to_vec(&Cache {
        version: CACHE_VERSION,
        fingerprint,
        waveform: waveform.clone(),
    })
    .map_err(|error| Error::Audio(error.to_string()))?;
    fs::write(&temp, data)?;
    if fs::rename(&temp, &cache_path).is_err() {
        let _ = fs::remove_file(&temp);
    }
    Ok(waveform)
}
