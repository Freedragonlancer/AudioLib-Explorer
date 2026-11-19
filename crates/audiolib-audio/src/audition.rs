// crates/audiolib-audio/src/audition.rs
//! Streaming audition transforms; all SoundTouch work stays on the control worker.
use crate::decoder::{Decoder, ResampledDecoder};
use audiolib_core::{Error, Result};
use serde::{Deserialize, Serialize};
#[path = "soundtouch_processor.rs"]
mod soundtouch_processor;
use soundtouch_processor::MonitorProcessor;

/// Independent semitone pitch and pitch-preserving speed, applied to the monitored channels.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AuditionSettings {
    pub pitch_semitones: f32,
    pub rate: f32,
    pub mono: bool,
    /// Explicitly monitor the selected pair even when discrete multichannel output is available.
    pub stereo_pair: bool,
    pub loop_enabled: bool,
}
impl Default for AuditionSettings {
    fn default() -> Self {
        Self {
            pitch_semitones: 0.0,
            rate: 1.0,
            mono: false,
            stereo_pair: false,
            loop_enabled: false,
        }
    }
}
impl AuditionSettings {
    /// Reject non-finite or unsupported transform values before altering playback.
    pub fn validate(self) -> Result<()> {
        if !self.pitch_semitones.is_finite()
            || !(-24.0..=24.0).contains(&self.pitch_semitones)
            || !self.rate.is_finite()
            || !(0.25..=4.0).contains(&self.rate)
        {
            return Err(Error::Validation(
                "Pitch must be -24..24 semitones; Rate must be 0.25..4.0".into(),
            ));
        }
        Ok(())
    }
}

/// Hardware layout negotiated for the stream. Pair monitoring uses the selected outputs;
/// discrete monitoring starts at output one and preserves source channel order.
#[derive(Clone, Copy, Debug)]
pub(crate) struct MonitorOutput {
    pub channels: u16,
    pub pair: u16,
}
impl MonitorOutput {
    #[cfg(test)]
    pub const STEREO: Self = Self {
        channels: 2,
        pair: 0,
    };
    pub fn lanes(self) -> usize {
        usize::from(self.channels.clamp(2, 256))
    }
}

pub(crate) struct AuditionDecoder {
    source: ResampledDecoder,
    channels: usize,
    pair: usize,
    pcm_channels: usize,
    output: MonitorOutput,
    pub settings: AuditionSettings,
    processor: Option<MonitorProcessor>,
    flushed: bool,
    remaining: u64,
}
impl AuditionDecoder {
    #[cfg(test)]
    pub fn new(
        decoder: Decoder,
        output_rate: u32,
        start_frame: u64,
        pair: u16,
        settings: AuditionSettings,
    ) -> Result<Self> {
        let end_frame = decoder.info.metadata.frames;
        Self::new_bounded(decoder, output_rate, start_frame, end_frame, pair, settings)
    }
    /// Only selected source frames reach the resampler and SoundTouch input.
    #[cfg(test)]
    pub fn new_bounded(
        decoder: Decoder,
        output_rate: u32,
        start_frame: u64,
        end_frame: u64,
        pair: u16,
        settings: AuditionSettings,
    ) -> Result<Self> {
        Self::routed(
            decoder,
            output_rate,
            start_frame..end_frame,
            pair,
            settings,
            MonitorOutput::STEREO,
        )
    }
    pub fn routed(
        decoder: Decoder,
        output_rate: u32,
        range: std::ops::Range<u64>,
        pair: u16,
        mut settings: AuditionSettings,
        output: MonitorOutput,
    ) -> Result<Self> {
        let (start_frame, end_frame) = (range.start, range.end);
        settings.validate()?;
        let metadata = &decoder.info.metadata;
        let channels = usize::from(metadata.channels);
        if channels == 2 && output.channels > 2 {
            settings.mono = false;
        }
        let multi = channels > 2 && output.channels > 2 && !settings.mono && !settings.stereo_pair;
        let pcm_channels = if multi {
            channels.min(output.lanes())
        } else {
            2
        };
        if !multi && usize::from(pair) * 2 >= channels {
            return Err(Error::Validation(
                "Source channel pair is outside the loaded WAV channels".into(),
            ));
        }
        if start_frame > end_frame || end_frame > metadata.frames {
            return Err(Error::Validation("Invalid audition frame range".into()));
        }
        let remaining = ((end_frame - start_frame) as f64 * f64::from(output_rate)
            / f64::from(metadata.sample_rate)
            / f64::from(settings.rate))
        .round() as u64;
        let processor = if settings.pitch_semitones != 0.0 || settings.rate != 1.0 {
            Some(MonitorProcessor::new(
                pcm_channels,
                output_rate,
                settings.rate,
                settings.pitch_semitones,
            )?)
        } else {
            None
        };
        Ok(Self {
            source: if end_frame == metadata.frames {
                ResampledDecoder::new(decoder, output_rate, start_frame)?
            } else {
                ResampledDecoder::new_bounded(decoder, output_rate, start_frame, end_frame)?
            },
            channels,
            pcm_channels,
            output,
            pair: usize::from(pair),
            settings,
            processor,
            flushed: false,
            remaining,
        })
    }
    /// Expand compact decoded frames on the worker. Unassigned device outputs stay silent.
    pub fn next_output(&mut self) -> Result<Vec<f32>> {
        let pcm = self.next()?;
        let lanes = self.output.lanes();
        if lanes == self.pcm_channels {
            return Ok(pcm);
        }
        let offset = if self.pcm_channels == 2 {
            usize::from(self.output.pair) * 2
        } else {
            0
        };
        let mut result = vec![0.0; pcm.len() / self.pcm_channels * lanes];
        for (input, output) in pcm
            .chunks_exact(self.pcm_channels)
            .zip(result.chunks_exact_mut(lanes))
        {
            output[offset..offset + input.len()].copy_from_slice(input);
        }
        Ok(result)
    }
    pub fn is_finished(&self) -> bool {
        self.remaining == 0
    }
    /// Whether producing `frames` more output frames will not wait for network data
    /// (Rate scales the source consumed; SoundTouch keeps a few thousand frames queued).
    pub fn is_buffered(&self, frames: u64) -> bool {
        let source = if self.processor.is_some() {
            (frames as f64 * f64::from(self.settings.rate)).ceil() as u64 + 8192
        } else {
            frames
        };
        self.source.is_buffered(source)
    }
    pub fn next(&mut self) -> Result<Vec<f32>> {
        if self.remaining == 0 {
            return Ok(Vec::new());
        }
        loop {
            if let Some(processor) = &mut self.processor {
                let frames = self.remaining.min(2048) as usize;
                let mut output = vec![0.0; frames * self.pcm_channels];
                let received = processor.receive_samples(&mut output);
                if received > 0 {
                    output.truncate(received * self.pcm_channels);
                    self.remaining -= received as u64;
                    return Ok(output);
                }
                if self.flushed {
                    // SoundTouch may round its duration by a frame. Pad only the
                    // bounded final block, retaining the exact source-time contract.
                    self.remaining -= frames as u64;
                    return Ok(output);
                }
            }
            let source = if self.source.is_finished() {
                Vec::new()
            } else {
                self.source.next()?
            };
            if source.is_empty() {
                if let Some(processor) = &mut self.processor {
                    processor.flush();
                    self.flushed = true;
                    continue;
                }
                self.remaining = 0;
                return Ok(Vec::new());
            }
            let mut pcm = if self.pcm_channels > 2 && self.pcm_channels == self.channels {
                source
            } else {
                let mut pcm = Vec::with_capacity(source.len() / self.channels * self.pcm_channels);
                for frame in source.chunks_exact(self.channels) {
                    if self.pcm_channels > 2 {
                        pcm.extend_from_slice(&frame[..self.pcm_channels]);
                    } else {
                        let (left, right) = crate::engine::monitor_pair(frame, self.pair);
                        if self.settings.mono || self.output.channels == 1 {
                            pcm.extend_from_slice(&[(left + right) * 0.5; 2]);
                        } else {
                            pcm.extend_from_slice(&[left, right]);
                        }
                    }
                }
                pcm
            };
            if let Some(processor) = &mut self.processor {
                processor.put_samples(&pcm)?;
            } else {
                let frames = self.remaining.min((pcm.len() / self.pcm_channels) as u64);
                pcm.truncate(frames as usize * self.pcm_channels);
                self.remaining -= frames;
                return Ok(pcm);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tone(path: &std::path::Path) {
        let mut pcm = Vec::new();
        for frame in 0..96000 {
            let value = (std::f32::consts::TAU * 440.0 * frame as f32 / 48000.0).sin() * 0.4;
            pcm.extend_from_slice(&value.to_le_bytes());
            pcm.extend_from_slice(&(-value).to_le_bytes());
        }
        crate::tests::write_wav(path, 48000, 2, 32, 3, &pcm, false, false);
    }
    #[test]
    fn monitor_routes_discrete_lanes_and_the_selected_stereo_pair() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("surround.wav");
        let frame = [0.125_f32, 0.25, 0.375, 0.5, 0.625, 0.75];
        let pcm = frame
            .repeat(32)
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect::<Vec<_>>();
        crate::tests::write_wav(&path, 48000, 6, 32, 3, &pcm, false, false);
        for (outputs, pair, mono, stereo_pair, expected) in [
            (
                8,
                1,
                false,
                false,
                vec![0.125, 0.25, 0.375, 0.5, 0.625, 0.75, 0.0, 0.0],
            ),
            (3, 0, false, false, vec![0.125, 0.25, 0.375]),
            (
                8,
                1,
                false,
                true,
                vec![0.0, 0.0, 0.375, 0.5, 0.0, 0.0, 0.0, 0.0],
            ),
            (4, 1, true, false, vec![0.0, 0.0, 0.4375, 0.4375]),
            (2, 0, false, false, vec![0.375, 0.5]),
            (1, 0, false, false, vec![0.4375, 0.4375]),
        ] {
            let output = MonitorOutput {
                channels: outputs,
                pair,
            };
            let settings = AuditionSettings {
                mono,
                stereo_pair,
                ..Default::default()
            };
            let mut decoder = AuditionDecoder::routed(
                Decoder::open(&path).unwrap(),
                48000,
                5..12,
                1,
                settings,
                output,
            )
            .unwrap();
            assert_eq!(decoder.next_output().unwrap(), expected.repeat(7));
            assert!(decoder.next_output().unwrap().is_empty());
        }
    }

    #[test]
    fn stereo_material_keeps_its_pair_on_a_surround_device() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stereo.wav");
        let frame = [0.25_f32, -0.5];
        let pcm: Vec<_> = frame
            .repeat(4)
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect();
        crate::tests::write_wav(&path, 48000, 2, 32, 3, &pcm, false, false);
        let mut decoder = AuditionDecoder::routed(
            Decoder::open(&path).unwrap(),
            48000,
            0..4,
            0,
            AuditionSettings {
                mono: true,
                ..Default::default()
            },
            MonitorOutput {
                channels: 6,
                pair: 1,
            },
        )
        .unwrap();
        assert_eq!(
            decoder.next_output().unwrap(),
            [0., 0., 0.25, -0.5, 0., 0.].repeat(4)
        );
    }

    #[test]
    fn multichannel_pitch_and_rate_preserve_silent_lanes_and_duration() {
        let dir = tempfile::tempdir().unwrap();
        for channels in [6, 18] {
            let path = dir.path().join(format!("monitor-{channels}.wav"));
            let mut pcm = Vec::new();
            for n in 0..24000 {
                for channel in 0..channels {
                    let sample = if channel + 1 == channels {
                        (n as f32 * 0.04).sin() * 0.4
                    } else {
                        0.0
                    };
                    pcm.extend_from_slice(&sample.to_le_bytes());
                }
            }
            crate::tests::write_wav(&path, 48000, channels, 32, 3, &pcm, false, false);
            let mut decoder = AuditionDecoder::routed(
                Decoder::open(&path).unwrap(),
                48000,
                0..24000,
                0,
                AuditionSettings {
                    pitch_semitones: 3.0,
                    rate: 1.5,
                    ..Default::default()
                },
                MonitorOutput { channels, pair: 0 },
            )
            .unwrap();
            let mut rendered = Vec::new();
            while !decoder.is_finished() {
                rendered.extend(decoder.next_output().unwrap());
            }
            assert_eq!(rendered.len(), 16000 * usize::from(channels));
            assert!(rendered.iter().any(|v| v.abs() > 0.05));
            assert!(
                rendered
                    .chunks_exact(channels.into())
                    .all(|f| f[..f.len() - 1].iter().all(|v| *v == 0.0))
            );
        }
    }

    #[test]
    fn pitch_and_speed_are_independent_with_exact_tail_and_bounded_blocks() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("tone.wav");
        tone(&path);
        for (pitch, rate, frequency) in [
            (0.0, 0.5, 440.0),
            (12.0, 1.0, 880.0),
            (-12.0, 2.0, 220.0),
            (7.0, 1.5, 659.26),
            (24.0, 4.0, 1760.0),
            (-24.0, 0.25, 110.0),
        ] {
            let settings = AuditionSettings {
                pitch_semitones: pitch,
                rate,
                ..Default::default()
            };
            let mut decoder =
                AuditionDecoder::new(Decoder::open(&path).unwrap(), 48000, 12000, 0, settings)
                    .unwrap();
            let mut result = Vec::new();
            while !decoder.is_finished() {
                let block = decoder.next().unwrap();
                assert!(block.len() <= 4096);
                result.extend(block);
            }
            assert_eq!(result.len() / 2, (84000.0 / rate).round() as usize);
            let mono: Vec<_> = result
                .chunks_exact(2)
                .skip(8000)
                .take(16000)
                .map(|p| p[0])
                .collect();
            let crossings = mono
                .windows(2)
                .filter(|p| p[0] <= 0.0 && p[1] > 0.0)
                .count();
            let measured = crossings as f32 * 48000.0 / mono.len() as f32;
            assert!(
                (measured - frequency).abs() < 8.0,
                "pitch {pitch} rate {rate}: {measured}"
            );
        }
        let mut mono = AuditionDecoder::new(
            Decoder::open(&path).unwrap(),
            48000,
            0,
            0,
            AuditionSettings {
                mono: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(mono.next().unwrap().iter().all(|v| v.abs() < 1e-6));
    }
}
