// app/audiolib-desktop/src/waveform_view.rs
//! Waveform viewport and pixel rendering, independent of transport processing.
use audiolib_audio::{SPECTRAL_PEAK_BASE, WaveformData, spectral_color};

const SPECTRAL_BACKGROUND: [u8; 4] = [132, 144, 148, 255];
const NORMAL_BACKGROUND: [u8; 4] = [29, 35, 40, 255];

#[derive(Clone, Copy, Debug)]
pub(crate) struct WaveView {
    pub zoom: f64,
    pub scroll: f64,
    pub gain_db: f32,
    pub opacity: f32,
    pub noise: f32,
    pub variance: f32,
    pub spectral: bool,
}
impl Default for WaveView {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            scroll: 0.0,
            gain_db: 0.0,
            opacity: 0.85,
            noise: 1.0,
            variance: 0.38,
            spectral: true,
        }
    }
}
impl WaveView {
    pub fn range(self, duration: f64) -> (f64, f64) {
        let span = duration / self.zoom.clamp(1.0, 128.0);
        let start = (duration - span).max(0.0) * self.scroll.clamp(0.0, 1.0);
        (start, start + span)
    }
    pub fn zoom_at(&mut self, factor: f64, position: f64, duration: f64) {
        let (start, end) = self.range(duration);
        let anchor = if (start..=end).contains(&position) {
            position
        } else {
            (start + end) * 0.5
        };
        self.zoom = (self.zoom * factor).clamp(1.0, 128.0);
        let span = duration / self.zoom;
        self.scroll = if duration > span {
            (anchor - span * 0.5).clamp(0.0, duration - span) / (duration - span)
        } else {
            0.0
        };
    }

    /// Accumulates zoom around a source time from the displayed Waveform.
    /// The caller resolves `anchor` from the displayed range, which can differ
    /// from this pending view while rendering. The anchor keeps its pointer
    /// fraction unless a source boundary constrains it; invalid inputs are ignored.
    pub fn zoom_at_anchor(&mut self, factor: f64, anchor: f64, fraction: f64, duration: f64) {
        if !factor.is_finite()
            || factor <= 0.0
            || !anchor.is_finite()
            || !fraction.is_finite()
            || !duration.is_finite()
            || duration <= 0.0
            || !self.zoom.is_finite()
            || !self.scroll.is_finite()
        {
            return;
        }
        let anchor = anchor.clamp(0.0, duration);
        let fraction = fraction.clamp(0.0, 1.0);
        let zoom = (self.zoom.clamp(1.0, 128.0) * factor).clamp(1.0, 128.0);
        let span = duration / zoom;
        let travel = (duration - span).max(0.0);
        let scroll = if travel > 0.0 {
            (anchor - span * fraction).clamp(0.0, travel) / travel
        } else {
            0.0
        };
        self.zoom = zoom;
        self.scroll = scroll;
    }
}

pub(crate) fn render_waveform(
    data: &WaveformData,
    width: u32,
    height: u32,
    view: WaveView,
) -> Vec<u8> {
    let mut rgba = vec![0_u8; (width * height * 4) as usize];
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.copy_from_slice(if view.spectral {
            &SPECTRAL_BACKGROUND
        } else {
            &NORMAL_BACKGROUND
        });
    }
    if width == 0 || height == 0 {
        return rgba;
    }
    let channels = data.peaks.len().max(1);
    let lane = height as f32 / channels as f32;
    let gain = 10.0_f32.powf(view.gain_db.clamp(-12.0, 36.0) / 20.0);
    for (channel, peaks) in data.peaks.iter().enumerate() {
        let center = lane * (channel as f32 + 0.5);
        let amplitude = (lane * 0.44).max(1.0);
        let top = (channel as f32 * lane) as u32;
        let bottom = (((channel + 1) as f32 * lane) as u32)
            .saturating_sub(1)
            .min(height - 1);
        for x in 0..width {
            let put = |rgba: &mut Vec<u8>, y: u32, color: [u8; 3]| {
                let index = ((y * width + x) * 4) as usize;
                rgba[index..index + 4].copy_from_slice(&[color[0], color[1], color[2], 255]);
            };
            put(
                &mut rgba,
                center.min((height - 1) as f32) as u32,
                if view.spectral {
                    [98, 111, 116]
                } else {
                    [55, 66, 72]
                },
            );
            let Some(peak) = peaks.get(x as usize * peaks.len() / width as usize) else {
                continue;
            };
            if peak.max.abs().max(peak.min.abs()) * gain < 0.00001 {
                continue;
            }
            let high = (center - (peak.max * gain).clamp(-1.0, 1.0) * amplitude)
                .clamp(top as f32, bottom as f32) as u32;
            let low = (center - (peak.min * gain).clamp(-1.0, 1.0) * amplitude)
                .clamp(top as f32, bottom as f32) as u32;
            let rgb = if view.spectral {
                spectral_color(peak.dominant_hz, peak.tonality, view.noise, view.variance)
            } else {
                [143, 174, 166]
            };
            // Spectral opacity blends with the normal BLACK peak, not light
            // gray: neither low tonality nor opacity may brighten the noise.
            let color = if view.spectral {
                let opacity = view.opacity.clamp(0.0, 1.0);
                rgb.map(|v| {
                    (f32::from(v) * opacity + f32::from(SPECTRAL_PEAK_BASE[0]) * (1.0 - opacity))
                        .round() as u8
                })
            } else {
                rgb
            };
            for y in high.min(low)..=high.max(low) {
                // Slightly darker RMS core leaves transients readable without
                // manufacturing a spectrogram or changing amplitude geometry.
                let core = (y as f32 - center).abs() < peak.rms * gain * amplitude;
                put(
                    &mut rgba,
                    y,
                    if core {
                        color.map(|v| (v as f32 * 0.78) as u8)
                    } else {
                        color
                    },
                );
            }
        }
    }
    rgba
}

pub(crate) fn db_from_gain(gain: f32) -> f32 {
    if gain <= 0.001 {
        -60.0
    } else {
        (20.0 * gain.log10()).clamp(-60.0, 0.0)
    }
}
pub(crate) fn gain_from_db(db: f32) -> f32 {
    if db <= -60.0 {
        0.0
    } else {
        10.0_f32.powf(db.clamp(-60.0, 0.0) / 20.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mouse_zoom_keeps_source_anchor_when_zooming_in_and_out() {
        let duration = 137.5;
        for fraction in [0.1, 0.27, 0.5, 0.85] {
            let mut view = WaveView {
                zoom: 4.0,
                scroll: 0.43,
                ..Default::default()
            };
            let initial = view.range(duration);
            let anchor = initial.0 + (initial.1 - initial.0) * fraction;
            for factor in [2.0, 0.25, 2.0] {
                view.zoom_at_anchor(factor, anchor, fraction, duration);
                let (start, end) = view.range(duration);
                let actual_anchor = start + (end - start) * fraction;
                assert!((actual_anchor - anchor).abs() < 1e-10);
            }
            assert!((view.scroll - 0.43).abs() < 1e-10);
            assert_eq!(view.zoom, 4.0);
        }
    }

    #[test]
    fn mouse_zoom_anchors_to_displayed_range_while_prior_render_is_pending() {
        let duration = 100.0;
        let displayed = (0.0, 100.0);
        let source_at = |fraction| displayed.0 + (displayed.1 - displayed.0) * fraction;
        let mut pending = WaveView::default();
        pending.zoom_at_anchor(2.0, source_at(0.25), 0.25, duration);
        assert_eq!(pending.range(duration), (12.5, 62.5));

        // The first render is still pending, so moving the pointer to 75%
        // refers to 75 seconds in the displayed range, not 50 in the pending one.
        pending.zoom_at_anchor(2.0, source_at(0.75), 0.75, duration);
        assert_eq!(pending.zoom, 4.0);
        assert_eq!(pending.range(duration), (56.25, 81.25));
        let (start, end) = pending.range(duration);
        assert_eq!(start + (end - start) * 0.75, 75.0);

        // Reversing a wheel step also accumulates zoom while anchoring to what
        // remains visible, even when the pointer has moved to the other side.
        let mut pending = WaveView {
            zoom: 4.0,
            scroll: 0.5,
            ..Default::default()
        };
        let displayed = pending.range(duration);
        let source_at = |fraction| displayed.0 + (displayed.1 - displayed.0) * fraction;
        pending.zoom_at_anchor(2.0, source_at(0.25), 0.25, duration);
        assert_eq!(pending.range(duration), (40.625, 53.125));
        pending.zoom_at_anchor(0.5, source_at(0.75), 0.75, duration);
        assert_eq!(pending.zoom, 4.0);
        assert_eq!(pending.range(duration), displayed);
    }

    #[test]
    fn mouse_zoom_clamps_edges_and_restores_full_source() {
        for (fraction, scroll) in [(-20.0, 0.0), (0.0, 0.0), (1.0, 1.0), (20.0, 1.0)] {
            let mut view = WaveView::default();
            let anchor = if scroll == 0.0 { -20.0 } else { 120.0 };
            view.zoom_at_anchor(f64::MAX, anchor, fraction, 100.0);
            assert_eq!(view.zoom, 128.0);
            assert_eq!(view.scroll, scroll);
            let (start, end) = view.range(100.0);
            assert!(start >= 0.0 && end <= 100.0);
            view.zoom_at_anchor(1.0 / 4096.0, anchor, fraction, 100.0);
            assert_eq!(view.zoom, 1.0);
            assert_eq!(view.scroll, 0.0);
            assert_eq!(view.range(100.0), (0.0, 100.0));
        }

        let mut right = WaveView {
            zoom: 8.0,
            scroll: 1.0,
            ..Default::default()
        };
        right.zoom_at_anchor(0.5, 87.5, 0.0, 100.0);
        assert_eq!(right.range(100.0), (75.0, 100.0));
        assert_eq!(right.scroll, 1.0);
        let mut left = WaveView {
            zoom: 8.0,
            scroll: 0.0,
            ..Default::default()
        };
        left.zoom_at_anchor(0.5, 12.5, 1.0, 100.0);
        assert_eq!(left.range(100.0), (0.0, 25.0));
        assert_eq!(left.scroll, 0.0);
    }

    #[test]
    fn mouse_zoom_ignores_invalid_parameters() {
        let initial = WaveView {
            zoom: 8.0,
            scroll: 0.4,
            ..Default::default()
        };
        for (factor, anchor, fraction, duration) in [
            (0.0, 50.0, 0.5, 100.0),
            (-1.0, 50.0, 0.5, 100.0),
            (f64::NAN, 50.0, 0.5, 100.0),
            (f64::INFINITY, 50.0, 0.5, 100.0),
            (f64::NEG_INFINITY, 50.0, 0.5, 100.0),
            (2.0, f64::NAN, 0.5, 100.0),
            (2.0, f64::INFINITY, 0.5, 100.0),
            (2.0, f64::NEG_INFINITY, 0.5, 100.0),
            (2.0, 50.0, f64::NAN, 100.0),
            (2.0, 50.0, f64::INFINITY, 100.0),
            (2.0, 50.0, f64::NEG_INFINITY, 100.0),
            (2.0, 50.0, 0.5, 0.0),
            (2.0, 50.0, 0.5, -100.0),
            (2.0, 50.0, 0.5, f64::NAN),
            (2.0, 50.0, 0.5, f64::INFINITY),
            (2.0, 50.0, 0.5, f64::NEG_INFINITY),
        ] {
            let mut view = initial;
            view.zoom_at_anchor(factor, anchor, fraction, duration);
            assert_eq!(view.zoom, initial.zoom);
            assert_eq!(view.scroll, initial.scroll);
            assert_eq!(view.range(100.0), initial.range(100.0));
        }
    }

    #[test]
    fn zoom_and_pan_stay_within_source_and_gain_db_roundtrips() {
        let mut view = WaveView::default();
        view.zoom_at(8.0, 98.0, 100.0);
        let (start, end) = view.range(100.0);
        assert!(start >= 0.0 && end <= 100.0 && end == 100.0);
        view.zoom_at(1.0 / 128.0, 98.0, 100.0);
        assert_eq!(view.range(100.0), (0.0, 100.0));
        assert!((gain_from_db(-12.1) - 0.2483).abs() < 0.0001);
        assert!((db_from_gain(0.5) + 6.0206).abs() < 0.001);
        assert_eq!(gain_from_db(-60.0), 0.0);
    }
    #[test]
    fn noise_stays_black_through_opacity_and_display_gain_on_visible_background() {
        use audiolib_audio::WaveformPeak;
        let data = WaveformData {
            sample_rate: 48000,
            channels: 1,
            frames: 100,
            samples_per_pixel: 100,
            peaks: vec![vec![WaveformPeak {
                min: -0.5,
                max: 0.5,
                rms: 0.0,
                dominant_hz: 619.0,
                tonality: 0.003,
                ..Default::default()
            }]],
        };
        for opacity in [0.0, 0.5, 0.85, 1.0] {
            for gain_db in [-6.0, 0.0, 6.0] {
                let pixels = render_waveform(
                    &data,
                    1,
                    20,
                    WaveView {
                        opacity,
                        gain_db,
                        ..Default::default()
                    },
                );
                assert_eq!(&pixels[10 * 4..11 * 4], &[14, 14, 14, 255]);
                assert_eq!(&pixels[0..4], &SPECTRAL_BACKGROUND);
            }
        }
        let normal = render_waveform(
            &data,
            1,
            20,
            WaveView {
                spectral: false,
                ..Default::default()
            },
        );
        assert_eq!(&normal[0..4], &NORMAL_BACKGROUND);
        assert_eq!(&normal[10 * 4..11 * 4], &[143, 174, 166, 255]);
    }
}
