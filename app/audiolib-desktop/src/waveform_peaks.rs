// app/audiolib-desktop/src/waveform_peaks.rs
//! Resident whole-source peaks. Navigation touches only bins behind visible pixels.
use audiolib_audio::{WaveformData, WaveformPeak, WaveformRequest};

/// Allows 1024 distinct columns at 128x for ordinary mono/stereo recordings.
pub(crate) const MAX_COLUMNS: usize = 128 * 1024;
const MAX_PEAKS: usize = 512 * 1024;
const FANOUT: usize = 4;

pub(crate) fn source_request(frames: u64, channels: u16) -> WaveformRequest {
    let columns = MAX_COLUMNS.min(MAX_PEAKS / usize::from(channels.max(1)));
    WaveformRequest {
        start_frame: 0,
        end_frame: Some(frames),
        samples_per_pixel: frames
            .div_ceil(columns as u64)
            .clamp(1, u64::from(u32::MAX)) as u32,
        include_colors: true,
    }
}

/// Each successive level summarizes four adjacent bins per channel. Use finer
/// bins at pixel boundaries so a transient cannot bleed across the viewport.
pub(crate) struct ResidentPeaks {
    base: WaveformData,
    levels: Vec<Vec<Vec<WaveformPeak>>>,
}

impl ResidentPeaks {
    pub(crate) fn new(base: WaveformData) -> Self {
        let mut levels = Vec::new();
        let mut previous = &base.peaks;
        let mut bin_frames = u64::from(base.samples_per_pixel.max(1));
        while previous.first().is_some_and(|lane| lane.len() >= FANOUT) {
            let next = previous
                .iter()
                .map(|lane| {
                    lane.chunks(FANOUT)
                        .enumerate()
                        .map(|(i, group)| {
                            let start = i as u64 * FANOUT as u64 * bin_frames;
                            let mut sum = PeakSum::default();
                            for (offset, peak) in group.iter().enumerate() {
                                let remaining = base
                                    .frames
                                    .saturating_sub(start + offset as u64 * bin_frames);
                                sum.add(peak, remaining.min(bin_frames));
                            }
                            sum.finish()
                        })
                        .collect()
                })
                .collect();
            levels.push(next);
            previous = levels.last().unwrap();
            bin_frames *= FANOUT as u64;
        }
        Self { base, levels }
    }

    pub(crate) fn viewport(&self, start: f64, end: f64, width: u32) -> WaveformData {
        let rate = f64::from(self.base.sample_rate);
        let start = ((start * rate).round() as u64).min(self.base.frames);
        let end = ((end * rate).round() as u64).clamp(start, self.base.frames);
        let span = end - start;
        let width = u64::from(width.max(1));
        let edge = |x| start + span / width * x + (span % width) * x / width;
        let spp = u64::from(self.base.samples_per_pixel.max(1));
        let peaks = self
            .base
            .peaks
            .iter()
            .enumerate()
            .map(|(channel, lane)| {
                (0..width)
                    .map(|x| {
                        let a = edge(x).min(self.base.frames.saturating_sub(1));
                        let b = edge(x + 1).max(a + 1).min(self.base.frames);
                        let mut i = (a / spp) as usize;
                        let limit = (b.div_ceil(spp) as usize).min(lane.len());
                        if i + 1 == limit {
                            return lane[i].clone();
                        }
                        let mut sum = PeakSum::default();
                        while i < limit {
                            let mut level = 0;
                            let mut step = 1;
                            while level < self.levels.len()
                                && step * FANOUT <= limit - i
                                && i.is_multiple_of(step * FANOUT)
                                && i as u64 * spp >= a
                                && (i + step * FANOUT) as u64 * spp <= b
                            {
                                step *= FANOUT;
                                level += 1;
                            }
                            let peak = if level == 0 {
                                &lane[i]
                            } else {
                                &self.levels[level - 1][channel][i / step]
                            };
                            let overlap = b.min((i + step) as u64 * spp) - a.max(i as u64 * spp);
                            sum.add(peak, overlap);
                            i += step;
                        }
                        sum.finish()
                    })
                    .collect()
            })
            .collect();
        WaveformData {
            frames: span,
            sample_rate: self.base.sample_rate,
            channels: self.base.channels,
            samples_per_pixel: span.div_ceil(width).clamp(1, u64::from(u32::MAX)) as u32,
            peaks,
        }
    }
}

#[derive(Default)]
struct PeakSum {
    peak: WaveformPeak,
    energy: f64,
    tone: f64,
    frames: u64,
    strongest: f32,
}

impl PeakSum {
    fn add(&mut self, value: &WaveformPeak, frames: u64) {
        if self.frames == 0 {
            self.peak = value.clone();
        } else {
            self.peak.min = self.peak.min.min(value.min);
            self.peak.max = self.peak.max.max(value.max);
        }
        let strength = value.min.abs().max(value.max.abs());
        if strength > self.strongest {
            self.strongest = strength;
            self.peak.dominant_hz = value.dominant_hz;
        }
        self.frames += frames;
        self.energy += f64::from(value.rms).powi(2) * frames as f64;
        self.tone += f64::from(value.tonality) * frames as f64;
    }

    fn finish(mut self) -> WaveformPeak {
        if self.frames > 0 {
            self.peak.rms = (self.energy / self.frames as f64).sqrt() as f32;
            self.peak.tonality = (self.tone / self.frames as f64) as f32;
        }
        self.peak
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::waveform_view::{WaveView, render_waveform_layers};
    use std::time::Instant;

    #[test]
    fn waveform_resident_levels_preserve_transients_at_edges_and_partial_tail() {
        let base = WaveformData {
            sample_rate: 1000,
            channels: 2,
            frames: 205,
            samples_per_pixel: 2,
            peaks: (0..2)
                .map(|ch| {
                    (0..103)
                        .map(|i| {
                            let v = if i == 19 || (i == 102 && ch == 1) {
                                1.0
                            } else {
                                0.1
                            };
                            WaveformPeak {
                                min: -v,
                                max: v,
                                rms: v,
                                ..Default::default()
                            }
                        })
                        .collect()
                })
                .collect(),
        };
        let resident = ResidentPeaks::new(base);
        let full = resident.viewport(0., 0.205, 1);
        assert_eq!(full.peaks[0][0].max, 1.0);
        assert!((full.peaks[1][0].rms - ((3.0_f32 + 202.0 * 0.01) / 205.0).sqrt()).abs() < 1e-5);
        let left = resident.viewport(0.0, 0.038, 5);
        assert!(left.peaks[0].iter().all(|p| p.max == 0.1));
        let right = resident.viewport(0.040, 0.205, 165);
        assert!(right.peaks[0].iter().all(|p| p.max == 0.1));
        assert_eq!(right.peaks[1][164].max, 1.0);
        let tail = resident.viewport(0.204, 0.205, 64);
        assert!(tail.peaks[1].iter().all(|p| p.max == 1.0));
        for channels in [1, 2, 6, 32, 256] {
            let frames = 4_000_000_017_u64;
            let req = source_request(frames, channels);
            let columns = frames.div_ceil(u64::from(req.samples_per_pixel));
            assert!(
                columns <= MAX_COLUMNS as u64 && columns * u64::from(channels) <= MAX_PEAKS as u64
            );
        }
    }

    #[test]
    fn waveform_continuous_zoom_renders_complete_stereo_views() {
        let columns = MAX_COLUMNS;
        let rate = 48000;
        let spp = 24;
        let frames = columns as u64 * u64::from(spp);
        let duration = frames as f64 / f64::from(rate);
        let lane = (0..columns)
            .map(|i| {
                let v = 0.25 + (i % 101) as f32 / 150.;
                WaveformPeak {
                    min: -v,
                    max: v,
                    rms: v * 0.6,
                    dominant_hz: 440.,
                    tonality: 0.7,
                    ..Default::default()
                }
            })
            .collect::<Vec<_>>();
        let resident = ResidentPeaks::new(WaveformData {
            sample_rate: rate,
            channels: 2,
            frames,
            samples_per_pixel: spp,
            peaks: vec![lane.clone(), lane],
        });
        for studio in [false, true] {
            let mut samples = Vec::new();
            for i in 0..96 {
                let view = WaveView {
                    zoom: 2_f64.powf(f64::from(i % 32) * 7.0 / 31.0),
                    scroll: f64::from(i) / 96.0,
                    ..Default::default()
                };
                let (a, b) = view.range(duration);
                let start = Instant::now();
                let data = resident.viewport(a, b, 1440);
                let (pixels, mask) = render_waveform_layers(&data, 1440, 96, view, studio);
                samples.push(start.elapsed());
                assert_eq!(pixels.as_bytes().len(), 1440 * 96 * 4);
                assert!(data.peaks.iter().flatten().all(|p| p.max >= 0.25));
                assert_eq!(mask.is_some(), studio);
            }
            samples.sort_unstable();
            eprintln!(
                "resident waveform, 1440x96 stereo, studio={studio}, 96 views: p50={:?}, p95={:?}",
                samples[48], samples[91]
            );
        }
    }
}
