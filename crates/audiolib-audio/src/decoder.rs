// crates/audiolib-audio/src/decoder.rs
use crate::{
    source::{
        ByteSource, MediaAdapter, SourceBuffer, Tracked, is_stream_source, open_source,
        source_buffer,
    },
    wav::{WavInfo, read_info},
};
use audioadapter_buffers::direct::InterleavedSlice;
use audiolib_core::{Error, Result};
use rubato::{Fft, FixedSync, Resampler};
use std::{
    io::{BufReader, Read, Seek, SeekFrom},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use symphonia::core::{
    codecs::audio::AudioDecoder,
    formats::{FormatReader, SeekMode, SeekTo, TrackType, probe::Hint},
    io::MediaSourceStream,
    packet::Packet,
    units::{Duration, Timestamp},
};

pub(crate) struct Decoder {
    pub info: WavInfo,
    source: Source,
    frame: u64,
    /// For a network source: what is in memory after the decoder's read position.
    buffer: Option<BufferProbe>,
}

/// Bytes past the estimate a buffered check also requires when Symphonia's format reader
/// (which reads ahead of the frames it returns) owns the source.
const READ_SLACK_BYTES: u64 = 64 * 1024;

struct BufferProbe {
    buffer: Arc<dyn SourceBuffer>,
    position: Arc<AtomicU64>,
    bytes_per_frame: f64,
    slack: u64,
}

/// Wrap `file` so its position is visible to the source's [`SourceBuffer`], if any.
fn tracked(
    file: Box<dyn ByteSource>,
    buffer: &Option<Arc<dyn SourceBuffer>>,
    (bytes_per_frame, slack): (f64, u64),
    probe: &mut Option<BufferProbe>,
) -> Box<dyn ByteSource> {
    let Some(buffer) = buffer else {
        return file;
    };
    let position = Arc::new(AtomicU64::new(0));
    *probe = Some(BufferProbe {
        buffer: buffer.clone(),
        position: position.clone(),
        bytes_per_frame,
        slack,
    });
    Box::new(Tracked::new(file, position))
}
enum Source {
    Pcm(Box<dyn ByteSource>),
    Symphonia {
        reader: Box<dyn FormatReader>,
        decoder: Box<dyn AudioDecoder>,
        track: u32,
        pending: Vec<f32>,
        offset: usize,
    },
    Blocks {
        file: Box<dyn ByteSource>,
        decoder: Box<dyn AudioDecoder>,
        track: u32,
        frames_per_block: u64,
        max_blocks_per_packet: u64,
        next_frame: u64,
        remaining_bytes: u64,
        pending: Vec<f32>,
        offset: usize,
    },
}
fn audio_error(error: impl std::fmt::Display) -> Error {
    Error::Audio(error.to_string())
}

/// Open `path`; with `read_buffer > 0` a plain file is read through a buffer of that many
/// bytes, so small decoder reads become large sequential reads (registered stream sources
/// keep their own buffering and are never wrapped).
fn open_bytes(path: &Path, read_buffer: usize) -> Result<(Box<dyn ByteSource>, u64)> {
    let (file, len) = open_source(path)?;
    if read_buffer == 0 || is_stream_source(path) {
        return Ok((file, len));
    }
    Ok((Box::new(BufReader::with_capacity(read_buffer, file)), len))
}

impl Decoder {
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_with_read_buffer(path, 0)
    }

    /// [`Decoder::open`] reading a plain file through a `read_buffer`-byte buffer
    /// (0 = unbuffered). Decoded samples are identical either way.
    pub fn open_with_read_buffer(path: &Path, read_buffer: usize) -> Result<Self> {
        let info = read_info(path)?;
        let buffer = source_buffer(path);
        let mut probe = None;
        let source = if matches!(info.format_tag, 1 | 3) {
            if !matches!(
                (info.format_tag, info.metadata.bit_depth),
                (1, 8 | 16 | 24 | 32) | (3, 32 | 64)
            ) {
                return Err(Error::Audio(
                    "Unsupported PCM/float WAV sample width".into(),
                ));
            }
            let (file, _) = open_bytes(path, read_buffer)?;
            let mut file = tracked(file, &buffer, (f64::from(info.block_align), 0), &mut probe);
            file.seek(SeekFrom::Start(info.data_offset))?;
            Source::Pcm(file)
        } else {
            let (inner, len) = open_bytes(path, read_buffer)?;
            let bytes_per_frame = if info.metadata.frames > 0 {
                info.data_bytes as f64 / info.metadata.frames as f64
            } else {
                f64::from(info.block_align)
            };
            let inner = tracked(
                inner,
                &buffer,
                (bytes_per_frame, READ_SLACK_BYTES),
                &mut probe,
            );
            let stream =
                MediaSourceStream::new(Box::new(MediaAdapter::new(inner, len)), Default::default());
            let mut hint = Hint::new();
            hint.with_extension("wav");
            let reader = symphonia::default::get_probe()
                .probe(&hint, stream, Default::default(), Default::default())
                .map_err(audio_error)?;
            let track = reader
                .default_track(TrackType::Audio)
                .ok_or_else(|| Error::Audio("No WAV audio track".into()))?;
            let params = track
                .codec_params
                .as_ref()
                .and_then(|p| p.audio())
                .ok_or_else(|| Error::Audio("Missing audio codec parameters".into()))?;
            let decoder = symphonia::default::get_codecs()
                .make_audio_decoder(params, &Default::default())
                .map_err(audio_error)?;
            let frames_per_block = params.frames_per_block.unwrap_or(1);
            if frames_per_block > 1 {
                // Symphonia 0.6.1's RIFF seek uses frame_ts * block_size for
                // ADPCM; byte positions actually require block_index * block_size.
                // Packetize complete WAV blocks here, preserving the codec decoder
                // and allowing bounded, sample-accurate seeks without a linear scan.
                // The format reader's own source is no longer read: track the block reader.
                probe = None;
                let (file, _) = open_bytes(path, read_buffer)?;
                // Whole blocks are read at a time: one block of slack.
                let mut file = tracked(
                    file,
                    &buffer,
                    (
                        f64::from(info.block_align) / frames_per_block as f64,
                        u64::from(info.block_align),
                    ),
                    &mut probe,
                );
                file.seek(SeekFrom::Start(info.data_offset))?;
                Source::Blocks {
                    file,
                    track: track.id,
                    decoder,
                    frames_per_block,
                    max_blocks_per_packet: params
                        .max_frames_per_packet
                        .unwrap_or(frames_per_block)
                        .div_euclid(frames_per_block)
                        .max(1),
                    next_frame: 0,
                    remaining_bytes: info.data_bytes,
                    pending: Vec::new(),
                    offset: 0,
                }
            } else {
                Source::Symphonia {
                    track: track.id,
                    reader,
                    decoder,
                    pending: Vec::new(),
                    offset: 0,
                }
            }
        };
        Ok(Self {
            info,
            source,
            frame: 0,
            buffer: probe,
        })
    }

    /// Whether the next `frames` source frames can be decoded without waiting for network
    /// data. Always true for files and sources without a [`SourceBuffer`].
    pub fn is_buffered(&self, frames: u64) -> bool {
        let Some(probe) = &self.buffer else {
            return true;
        };
        let frames = frames.min(self.info.metadata.frames.saturating_sub(self.frame));
        if frames == 0 {
            return true;
        }
        let bytes = (frames as f64 * probe.bytes_per_frame).ceil() as u64 + probe.slack;
        probe
            .buffer
            .is_buffered(probe.position.load(Ordering::Acquire), bytes)
    }

    pub fn seek(&mut self, frame: u64) -> Result<()> {
        let frame = frame.min(self.info.metadata.frames);
        let actual_frame = match &mut self.source {
            Source::Pcm(file) => {
                file.seek(SeekFrom::Start(
                    self.info.data_offset + frame * u64::from(self.info.block_align),
                ))?;
                frame
            }
            Source::Symphonia {
                reader,
                decoder,
                track,
                pending,
                offset,
            } => {
                let seeked = reader
                    .seek(
                        SeekMode::Accurate,
                        SeekTo::Timestamp {
                            ts: Timestamp::try_from(frame).map_err(audio_error)?,
                            track_id: *track,
                        },
                    )
                    .map_err(audio_error)?;
                decoder.reset();
                pending.clear();
                *offset = 0;
                u64::try_from(seeked.actual_ts.get()).map_err(audio_error)?
            }
            Source::Blocks {
                file,
                decoder,
                frames_per_block,
                next_frame,
                remaining_bytes,
                pending,
                offset,
                ..
            } => {
                let block = frame / *frames_per_block;
                let byte_offset = block * u64::from(self.info.block_align);
                file.seek(SeekFrom::Start(self.info.data_offset + byte_offset))?;
                *remaining_bytes = self.info.data_bytes.saturating_sub(byte_offset);
                *next_frame = block * *frames_per_block;
                decoder.reset();
                pending.clear();
                *offset = 0;
                *next_frame
            }
        };
        if actual_frame > frame {
            return Err(Error::Audio(
                "Codec seek landed after the requested frame".into(),
            ));
        }
        self.frame = actual_frame;
        let channels = usize::from(self.info.metadata.channels);
        while self.frame < frame {
            let discard = (frame - self.frame).min(2048) as usize;
            if self.read(discard)?.len() < channels {
                return Err(Error::Audio("Unexpected end of WAV during seek".into()));
            }
        }
        Ok(())
    }

    /// Decode at most a bounded number of interleaved frames; never used in an output callback.
    pub fn read(&mut self, frames: usize) -> Result<Vec<f32>> {
        let channels = usize::from(self.info.metadata.channels);
        let frames = frames.min(self.info.metadata.frames.saturating_sub(self.frame) as usize);
        if frames == 0 {
            return Ok(Vec::new());
        }
        let mut output = Vec::with_capacity(frames * channels);
        match &mut self.source {
            Source::Pcm(file) => {
                let width = usize::from(self.info.metadata.bit_depth / 8);
                let align = usize::from(self.info.block_align);
                let mut bytes = vec![0_u8; frames * align];
                file.read_exact(&mut bytes)?;
                for frame in bytes.chunks_exact(align) {
                    for bytes in frame[..width * channels].chunks_exact(width) {
                        let value = match (self.info.format_tag, width) {
                            (1, 1) => (f32::from(bytes[0]) - 128.0) / 128.0,
                            (1, 2) => {
                                f32::from(i16::from_le_bytes(bytes.try_into().unwrap())) / 32768.0
                            }
                            (1, 3) => {
                                let value = i32::from_le_bytes([
                                    bytes[0],
                                    bytes[1],
                                    bytes[2],
                                    if bytes[2] & 128 != 0 { 255 } else { 0 },
                                ]);
                                value as f32 / 8_388_608.0
                            }
                            (1, 4) => {
                                i32::from_le_bytes(bytes.try_into().unwrap()) as f32
                                    / 2_147_483_648.0
                            }
                            (3, 4) => f32::from_le_bytes(bytes.try_into().unwrap()),
                            (3, 8) => f64::from_le_bytes(bytes.try_into().unwrap()) as f32,
                            _ => unreachable!(),
                        };
                        output.push(if value.is_finite() { value } else { 0.0 });
                    }
                }
            }
            Source::Symphonia {
                reader,
                decoder,
                track,
                pending,
                offset,
            } => {
                while output.len() < frames * channels {
                    if *offset < pending.len() {
                        let count = (frames * channels - output.len()).min(pending.len() - *offset);
                        output.extend_from_slice(&pending[*offset..*offset + count]);
                        *offset += count;
                    } else {
                        let Some(packet) = reader.next_packet().map_err(audio_error)? else {
                            break;
                        };
                        if packet.track_id != *track {
                            continue;
                        }
                        let audio = decoder.decode(&packet).map_err(audio_error)?;
                        pending.resize(audio.samples_interleaved(), 0.0);
                        audio.copy_to_slice_interleaved(&mut *pending);
                        *offset = 0;
                    }
                }
            }
            Source::Blocks {
                file,
                decoder,
                track,
                frames_per_block,
                max_blocks_per_packet,
                next_frame,
                remaining_bytes,
                pending,
                offset,
            } => {
                while output.len() < frames * channels {
                    if *offset < pending.len() {
                        let count = (frames * channels - output.len()).min(pending.len() - *offset);
                        output.extend_from_slice(&pending[*offset..*offset + count]);
                        *offset += count;
                    } else {
                        let block_size = u64::from(self.info.block_align);
                        let blocks = (*max_blocks_per_packet).min(*remaining_bytes / block_size);
                        if blocks == 0 {
                            break;
                        }
                        let packet_frames = blocks * *frames_per_block;
                        let mut bytes = vec![0; (blocks * block_size) as usize];
                        file.read_exact(&mut bytes)?;
                        *remaining_bytes -= bytes.len() as u64;
                        let packet = Packet::new(
                            *track,
                            Timestamp::try_from(*next_frame).map_err(audio_error)?,
                            Duration::from(packet_frames),
                            bytes,
                        );
                        *next_frame += packet_frames;
                        let audio = decoder.decode(&packet).map_err(audio_error)?;
                        pending.resize(audio.samples_interleaved(), 0.0);
                        audio.copy_to_slice_interleaved(&mut *pending);
                        *offset = 0;
                    }
                }
            }
        }
        self.frame += (output.len() / channels) as u64;
        Ok(output)
    }
}

/// Bounded FFT sample rate conversion, trimming its startup delay and padded tail.
pub(crate) struct ResampledDecoder {
    decoder: Decoder,
    resampler: Option<Fft<f32>>,
    output_remaining: u64,
    delay_remaining: usize,
    channels: usize,
    /// Source frames consumed per output frame.
    input_per_output: f64,
}
impl ResampledDecoder {
    pub fn is_finished(&self) -> bool {
        self.output_remaining == 0
    }
    /// [`Decoder::is_buffered`] for `frames` output frames (plus one resampler block).
    pub fn is_buffered(&self, frames: u64) -> bool {
        let block = self
            .resampler
            .as_ref()
            .map_or(0, |resampler| resampler.input_frames_max() as u64);
        self.decoder
            .is_buffered((frames as f64 * self.input_per_output).ceil() as u64 + block)
    }
    pub fn new(mut decoder: Decoder, output_rate: u32, start_frame: u64) -> Result<Self> {
        decoder.seek(start_frame)?;
        let metadata = &decoder.info.metadata;
        let channels = usize::from(metadata.channels);
        let output_remaining = ((metadata.frames.saturating_sub(start_frame)) as f64
            * f64::from(output_rate)
            / f64::from(metadata.sample_rate))
        .round() as u64;
        let resampler = if output_rate == metadata.sample_rate {
            None
        } else {
            Some(
                Fft::<f32>::new(
                    metadata.sample_rate as usize,
                    output_rate as usize,
                    1024,
                    channels,
                    FixedSync::Input,
                )
                .map_err(audio_error)?,
            )
        };
        let delay_remaining = resampler
            .as_ref()
            .map_or(0, |resampler| resampler.output_delay());
        let input_per_output = f64::from(metadata.sample_rate) / f64::from(output_rate.max(1));
        Ok(Self {
            decoder,
            resampler,
            output_remaining,
            delay_remaining,
            channels,
            input_per_output,
        })
    }
    pub fn next(&mut self) -> Result<Vec<f32>> {
        if self.output_remaining == 0 {
            return Ok(Vec::new());
        }
        let Some(resampler) = &mut self.resampler else {
            let samples = self.decoder.read(2048)?;
            self.output_remaining = self
                .output_remaining
                .saturating_sub((samples.len() / self.channels) as u64);
            return Ok(samples);
        };
        let frames = resampler.input_frames_next();
        let mut input = self.decoder.read(frames)?;
        input.resize(frames * self.channels, 0.0);
        let input = InterleavedSlice::new(&input, self.channels, frames).map_err(audio_error)?;
        let mut output = resampler
            .process(&input, None)
            .map_err(audio_error)?
            .take_data();
        let skip = self.delay_remaining.min(output.len() / self.channels);
        self.delay_remaining -= skip;
        output.drain(..skip * self.channels);
        output.truncate(
            (self
                .output_remaining
                .min((output.len() / self.channels) as u64) as usize)
                * self.channels,
        );
        self.output_remaining -= (output.len() / self.channels) as u64;
        if output.is_empty() && self.output_remaining > 0 {
            return self.next();
        }
        Ok(output)
    }
}
