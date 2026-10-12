// app/audiolib-desktop/src/progressive_waveform.rs
//! Bounded remote Waveform ranges and partial images on a fixed source timeline.
use audiolib_audio::{WaveformData, WaveformPeak, WaveformRequest};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

const UPDATE_INTERVAL: Duration = Duration::from_millis(500);

/// Fetch a small first range, then grow batches to bound round trips for long files.
/// Publication happens on this worker; the caller marshals images to the UI thread.
pub(crate) fn load(
    request: WaveformRequest,
    sample_rate: u32,
    cancel: &AtomicBool,
    mut fetch: impl FnMut(WaveformRequest, u32) -> Result<WaveformData, String>,
    mut publish: impl FnMut(&WaveformData, f64),
) -> Result<WaveformData, String> {
    let frames = request
        .end_frame
        .unwrap_or(0)
        .saturating_sub(request.start_frame);
    let spp = u64::from(request.samples_per_pixel.max(1));
    let bins = frames.div_ceil(spp) as usize;
    if bins == 0 || bins > 8192 || sample_rate == 0 {
        return Err("Invalid progressive Waveform range".into());
    }
    let mut data: Option<WaveformData> = None;
    let mut offset = 0;
    let mut batch = u64::from(sample_rate).div_ceil(2 * spp).max(1) as usize;
    let max_batch = bins.div_ceil(16).max(batch);
    let mut last_update = None;
    while offset < bins {
        check_cancel(cancel)?;
        let count = batch.min(bins - offset);
        let start = offset as u64 * spp;
        let end = ((offset + count) as u64 * spp).min(frames);
        let part = fetch(
            WaveformRequest {
                start_frame: request.start_frame + start,
                end_frame: Some(request.start_frame + end),
                ..request.clone()
            },
            (count as u32).clamp(64, 8192),
        )?;
        check_cancel(cancel)?;
        validate_part(&part, end - start, sample_rate)?;
        if bins.saturating_mul(usize::from(part.channels)) > 4_000_000 {
            return Err("Waveform request exceeds 4 million peaks".into());
        }
        let output = data.get_or_insert_with(|| WaveformData {
            sample_rate,
            channels: part.channels,
            frames,
            samples_per_pixel: spp as u32,
            peaks: vec![vec![WaveformPeak::default(); bins]; usize::from(part.channels)],
        });
        if output.channels != part.channels {
            return Err("Waveform channels changed between ranges".into());
        }
        merge(output, &part, offset, count, end - start);
        offset += count;
        // First usable range immediately, then at most twice per second. The caller
        // publishes the completed result once, including wholly cached requests.
        if offset < bins
            && last_update.is_none_or(|time: Instant| time.elapsed() >= UPDATE_INTERVAL)
        {
            publish(output, end as f64 / frames as f64);
            last_update = Some(Instant::now());
        }
        batch = batch.saturating_mul(2).min(max_batch);
    }
    Ok(data.expect("nonempty validated range"))
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Acquire) {
        Err("Waveform cancelled".into())
    } else {
        Ok(())
    }
}

/// Validate remote/cache segments before admitting them to the progressive image.
pub(crate) fn validate_part(part: &WaveformData, frames: u64, rate: u32) -> Result<(), String> {
    if rate == 0 || frames == 0 {
        return Err("Invalid remote Waveform segment".into());
    }
    // A completed audition fallback can have a different preview sample rate.
    let expected = (u128::from(frames) * u128::from(part.sample_rate) + u128::from(rate) / 2)
        / u128::from(rate);
    let Ok(expected) = u64::try_from(expected) else {
        return Err("Invalid remote Waveform segment".into());
    };
    // Independently rounded preview endpoints can coincide for a final source
    // fragment shorter than one preview frame. It has no samples to draw.
    let empty_preview = part.sample_rate < rate && expected <= 1;
    if part.sample_rate == 0
        || part.channels == 0
        || (part.frames == 0 && !empty_preview)
        || part.frames.abs_diff(expected) > 1
        || part.samples_per_pixel == 0
        || part.peaks.len() != usize::from(part.channels)
        || part
            .frames
            .div_ceil(u64::from(part.samples_per_pixel))
            .saturating_mul(u64::from(part.channels))
            > 4_000_000
        || part.peaks.iter().any(|peaks| {
            peaks.len() as u64 != part.frames.div_ceil(u64::from(part.samples_per_pixel))
                || peaks.iter().any(|p| {
                    !p.min.is_finite()
                        || !p.max.is_finite()
                        || !p.rms.is_finite()
                        || !p.dominant_hz.is_finite()
                        || !p.tonality.is_finite()
                        || p.min > p.max
                        || p.rms < 0.0
                        || p.dominant_hz < 0.0
                        || !(0.0..=1.0).contains(&p.tonality)
                })
        })
    {
        return Err("Invalid remote Waveform segment".into());
    }
    Ok(())
}

/// Rebin smaller server buckets without losing transients, weighting RMS by overlap.
/// Unloaded columns remain empty; the renderer always sees the entire viewport.
fn merge(output: &mut WaveformData, part: &WaveformData, offset: usize, count: usize, frames: u64) {
    if part.frames == 0 {
        return;
    }
    let scale = part.frames as f64 / frames as f64;
    let source_spp = f64::from(part.samples_per_pixel);
    for (target, source) in output.peaks.iter_mut().zip(&part.peaks) {
        for index in 0..count {
            let start = index as f64 * f64::from(output.samples_per_pixel) * scale;
            let end = (((index + 1) as u64 * u64::from(output.samples_per_pixel)).min(frames))
                as f64
                * scale;
            let first = (start / source_spp).floor() as usize;
            let last = ((end / source_spp).ceil() as usize).min(source.len());
            let mut peak = WaveformPeak {
                min: f32::INFINITY,
                max: f32::NEG_INFINITY,
                ..Default::default()
            };
            let (mut energy, mut tonal, mut hz, mut weight) = (0.0, 0.0, 0.0, 0.0);
            for (i, value) in source.iter().enumerate().take(last).skip(first) {
                let overlap = (end.min((i + 1) as f64 * source_spp)
                    - start.max(i as f64 * source_spp))
                .max(0.0);
                peak.min = peak.min.min(value.min);
                peak.max = peak.max.max(value.max);
                energy += f64::from(value.rms).powi(2) * overlap;
                tonal += f64::from(value.tonality) * overlap;
                let tone_weight = f64::from(value.tonality).powi(2) * overlap;
                hz += f64::from(value.dominant_hz.max(1.0)).ln() * tone_weight;
                weight += tone_weight;
            }
            peak.rms = (energy / (end - start)).sqrt() as f32;
            peak.tonality = (tonal / (end - start)) as f32;
            peak.dominant_hz = if weight > 1e-12 {
                (hz / weight).exp() as f32
            } else {
                0.0
            };
            target[offset + index] = peak;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(frames: u64) -> WaveformRequest {
        WaveformRequest {
            start_frame: 1234,
            end_frame: Some(1234 + frames),
            samples_per_pixel: frames.div_ceil(1024) as u32,
            include_colors: true,
        }
    }

    fn response(request: &WaveformRequest, buckets: u32) -> WaveformData {
        let frames = request.end_frame.unwrap() - request.start_frame;
        let spp = frames.div_ceil(u64::from(buckets)) as u32;
        WaveformData {
            sample_rate: 48000,
            channels: 2,
            frames,
            samples_per_pixel: spp,
            peaks: vec![
                vec![
                    WaveformPeak {
                        min: -0.8,
                        max: 0.9,
                        rms: 0.5,
                        dominant_hz: 619.0,
                        tonality: 0.8,
                        color: None
                    };
                    frames.div_ceil(u64::from(spp)) as usize
                ];
                2
            ],
        }
    }

    #[test]
    fn partial_timeline_and_final_tail_keep_stereo_peaks_and_zoom_offset() {
        let request = request(48000 * 10 + 17);
        let mut next = request.start_frame;
        let mut partials = 0;
        let data = load(
            request.clone(),
            48000,
            &AtomicBool::new(false),
            |part, buckets| {
                assert_eq!(part.start_frame, next);
                next = part.end_frame.unwrap();
                Ok(response(&part, buckets))
            },
            |data, progress| {
                partials += 1;
                assert!(progress > 0.0 && progress < 1.0);
                assert_eq!(data.frames, 480017);
                assert_eq!(data.peaks[0].last().unwrap().max, 0.0);
                assert_eq!(data.peaks[0][0].max, 0.9);
            },
        )
        .unwrap();
        assert!(partials > 0);
        assert_eq!(Some(next), request.end_frame);
        for peaks in data.peaks {
            for peak in peaks {
                assert_eq!((peak.min, peak.max), (-0.8, 0.9));
                assert!((peak.rms - 0.5).abs() < 1e-5);
                assert!((peak.dominant_hz - 619.0).abs() < 0.01);
            }
        }
    }

    #[test]
    fn long_files_have_bounded_requests_and_cancellation_stops_publication() {
        let mut calls = 0;
        load(
            request(48000 * 3600),
            48000,
            &AtomicBool::new(false),
            |part, buckets| {
                calls += 1;
                Ok(response(&part, buckets))
            },
            |_, _| {},
        )
        .unwrap();
        assert!(calls <= 24, "{calls}");
        let cancel = AtomicBool::new(false);
        let result = load(
            request(480000),
            48000,
            &cancel,
            |part, buckets| {
                cancel.store(true, Ordering::Release);
                Ok(response(&part, buckets))
            },
            |_, _| panic!("cancelled result published"),
        );
        assert!(result.is_err());
    }

    #[test]
    fn malformed_and_incomplete_segments_are_rejected() {
        let part = request(24000);
        let mut data = response(&part, 64);
        data.peaks[1].pop();
        assert!(validate_part(&data, 24000, 48000).is_err());
        let mut data = response(&part, 64);
        data.peaks[0][0].rms = f32::NAN;
        assert!(validate_part(&data, 24000, 48000).is_err());
        assert!(validate_part(&response(&part, 64), 48000, 48000).is_err());
        for (min, max, rms, hz, tonality) in [
            (0.9, -0.8, 0.5, 619.0, 0.8),
            (-0.8, 0.9, -0.5, 619.0, 0.8),
            (-0.8, 0.9, 0.5, -619.0, 0.8),
            (-0.8, 0.9, 0.5, 619.0, -0.1),
            (-0.8, 0.9, 0.5, 619.0, 1.1),
        ] {
            let mut data = response(&part, 64);
            data.peaks[0][0] = WaveformPeak {
                min,
                max,
                rms,
                dominant_hz: hz,
                tonality,
                color: None,
            };
            assert!(validate_part(&data, 24000, 48000).is_err());
        }
        let mut data = response(&part, 64);
        data.samples_per_pixel = 0;
        assert!(validate_part(&data, 24000, 48000).is_err());
        assert!(validate_part(&response(&part, 64), 24000, 0).is_err());
    }

    #[test]
    fn downsampled_fallback_keeps_source_timeline_and_accepts_a_subframe_tail() {
        // A server response for the first range can be followed by the complete
        // 48 kHz audition fallback for the remaining 192 kHz source ranges.
        let request = WaveformRequest {
            start_frame: 0,
            end_frame: Some(480001),
            samples_per_pixel: 1000,
            include_colors: true,
        };
        let mut calls = 0;
        let mut empty_tail = false;
        let data = load(
            request,
            192000,
            &AtomicBool::new(false),
            |part, buckets| {
                calls += 1;
                if calls == 1 {
                    let mut data = response(&part, buckets);
                    data.sample_rate = 192000;
                    return Ok(data);
                }
                let scale = |frame: u64| (frame + 2) / 4;
                let frames = scale(part.end_frame.unwrap()) - scale(part.start_frame);
                if frames == 0 {
                    empty_tail = true;
                    assert_eq!(part.end_frame.unwrap() - part.start_frame, 1);
                    return Ok(WaveformData {
                        sample_rate: 48000,
                        channels: 2,
                        frames: 0,
                        samples_per_pixel: 1,
                        peaks: vec![Vec::new(); 2],
                    });
                }
                Ok(response(
                    &WaveformRequest {
                        start_frame: scale(part.start_frame),
                        end_frame: Some(scale(part.end_frame.unwrap())),
                        ..part
                    },
                    buckets,
                ))
            },
            |_, _| {},
        )
        .unwrap();
        assert!(empty_tail);
        assert_eq!((data.sample_rate, data.frames), (192000, 480001));
        assert_eq!(data.peaks[0].len(), 481);
        for peaks in data.peaks {
            for peak in &peaks[..480] {
                assert_eq!((peak.min, peak.max), (-0.8, 0.9));
                assert!((peak.rms - 0.5).abs() < 1e-5);
                assert!((peak.dominant_hz - 619.0).abs() < 0.01);
            }
            assert_eq!((peaks[480].min, peaks[480].max), (0.0, 0.0));
        }
        let empty = WaveformData {
            sample_rate: 48000,
            channels: 2,
            frames: 0,
            samples_per_pixel: 1,
            peaks: vec![Vec::new(); 2],
        };
        assert!(validate_part(&empty, 1, 48000).is_err());
        assert!(validate_part(&empty, 1000, 192000).is_err());
    }

    #[test]
    fn preview_endpoint_rounding_preserves_fractional_rate_segments() {
        let original_rate = 44100;
        let preview_rate = 32000;
        let scale = |frame: u64| {
            ((u128::from(frame) * u128::from(preview_rate) + u128::from(original_rate) / 2)
                / u128::from(original_rate)) as u64
        };
        let source = WaveformRequest {
            start_frame: 1235,
            end_frame: Some(442254),
            samples_per_pixel: 431,
            include_colors: true,
        };
        let data = load(
            source.clone(),
            original_rate,
            &AtomicBool::new(false),
            |part, buckets| {
                let mut data = response(
                    &WaveformRequest {
                        start_frame: scale(part.start_frame),
                        end_frame: part.end_frame.map(scale),
                        ..part
                    },
                    buckets,
                );
                data.sample_rate = preview_rate;
                Ok(data)
            },
            |_, _| {},
        )
        .unwrap();
        assert_eq!(data.sample_rate, original_rate);
        assert_eq!(data.frames, source.end_frame.unwrap() - source.start_frame);
        for peak in data.peaks.iter().flatten() {
            assert_eq!((peak.min, peak.max), (-0.8, 0.9));
            assert!((peak.rms - 0.5).abs() < 1e-5);
            assert!((peak.tonality - 0.8).abs() < 1e-5);
        }
    }

    #[test]
    fn range_http_replies_publish_before_the_next_request_and_preserve_transients() {
        use audiolib_remote::{RemoteClient, WaveformQuery, WaveformReply};
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (published, first_visible) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            for (start, end) in [(1234, 25234), (25234, 49234)] {
                if start != 1234 {
                    first_visible.recv_timeout(Duration::from_secs(5)).unwrap();
                }
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(socket.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                assert!(
                    line.contains(&format!("start_frame={start}&end_frame={end}")),
                    "{line}"
                );
                loop {
                    line.clear();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" {
                        break;
                    }
                }
                let mut part = response(
                    &WaveformRequest {
                        start_frame: start,
                        end_frame: Some(end),
                        ..Default::default()
                    },
                    64,
                );
                part.peaks[0][13].max = 1.0;
                let json = serde_json::to_vec(&part).unwrap();
                write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", json.len()).unwrap();
                socket.write_all(&json).unwrap();
            }
        });
        let client = RemoteClient::new(&format!("http://{address}"), "").unwrap();
        let cancel = AtomicBool::new(false);
        let data = load(
            WaveformRequest {
                start_frame: 1234,
                end_frame: Some(49234),
                samples_per_pixel: 1000,
                include_colors: true,
            },
            48000,
            &cancel,
            |part, buckets| {
                let reply = client
                    .waveform(
                        7,
                        &WaveformQuery {
                            buckets,
                            colors: true,
                            start_frame: Some(part.start_frame),
                            end_frame: part.end_frame,
                        },
                        None,
                        &cancel,
                    )
                    .unwrap();
                let WaveformReply::Data { json, .. } = reply else {
                    panic!("expected data")
                };
                Ok(serde_json::from_slice(&json).unwrap())
            },
            |data, progress| {
                assert_eq!(progress, 0.5);
                assert_eq!(data.peaks[0].len(), 48);
                assert!(data.peaks[0][..24].iter().any(|p| p.max == 1.0));
                assert!(data.peaks[0][24..].iter().all(|p| p.max == 0.0));
                published.send(()).unwrap();
            },
        )
        .unwrap();
        assert_eq!(data.peaks[1].last().unwrap().max, 0.9);
        assert_eq!(data.peaks[0].iter().filter(|p| p.max == 1.0).count(), 4);
        server.join().unwrap();
    }

    #[test]
    fn cancellation_after_partial_keeps_later_requests_off_the_wire() {
        let cancel = AtomicBool::new(false);
        let mut calls = 0;
        let result = load(
            request(480000),
            48000,
            &cancel,
            |part, buckets| {
                calls += 1;
                Ok(response(&part, buckets))
            },
            |_, _| cancel.store(true, Ordering::Release),
        );
        assert!(result.is_err());
        assert_eq!(calls, 1);
    }
}
