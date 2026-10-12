// server/audiolib-server/src/pcm.rs
//! In-process PCM preview for `GET /audio/{id}/preview?codec=pcm` (doc/api/README.md §8.1):
//! WAV header parsing, integer-factor anti-alias decimation, channel folding and
//! deterministic TPDF dither, written as a canonical 44-byte-header WAV.
//!
//! Output frame `k` is computed only from source frames around `k * factor` with a fixed
//! linear-phase FIR filter, and the dither noise is a hash of `(output frame, channel)`, so
//! the output is a pure function of the source bytes and the plan: any byte range is
//! byte-identical to the same bytes of a full download (Range, resume and ETags work).
use crate::preview::Mix;
use std::{cell::RefCell, collections::VecDeque, io, sync::Arc, thread::JoinHandle};

/// Bumped whenever the output bytes for the same source and parameters change; part of
/// the preview `ETag`.
pub const ALGORITHM_VERSION: u32 = 2;
/// Size of the synthesized canonical WAV header (`RIFF` + 16-byte `fmt ` + `data`).
pub const HEADER_BYTES: u64 = 44;
/// Filter half-length per decimation step: `taps = 2 * HALF_TAPS_PER_STEP * factor + 1`.
const HALF_TAPS_PER_STEP: usize = 24;
/// Kaiser window shape (about 70 dB stop-band attenuation).
const KAISER_BETA: f64 = 7.0;
/// Low-pass cutoff (-6 dB point) as a fraction of the output sample rate. With the
/// filter length above the transition band is about 0.405..0.495 x the output rate, so
/// everything at or above the output Nyquist frequency is in the stop band.
const CUTOFF: f64 = 0.45;
/// Approximate output bytes rendered per chunk.
const CHUNK_BYTES: u64 = 64 * 1024;
/// Upper bound for the number of RIFF chunks walked while looking for `fmt `/`data`.
const MAX_RIFF_CHUNKS: usize = 4096;
/// Most channels accepted in a source file.
pub const MAX_CHANNELS: u16 = 64;
/// Highest source sample rate accepted (768 kHz, the highest rate PCM hardware uses);
/// anything above is a corrupt header.
pub const MAX_SOURCE_RATE: u32 = 768_000;
/// Largest integer decimation factor (768 kHz -> 8 kHz). The FIR length and the work per
/// output frame grow with the factor, so sources that need more (odd or prime rates)
/// are 415 for that `sr`.
pub const MAX_DECIMATION_FACTOR: u32 = 96;
/// Float samples are clamped to this magnitude (full scale; NaN counts as 0) before
/// filtering, so infinities and huge values from damaged files clip instead of
/// turning whole filter windows into NaN (silence).
const FLOAT_LIMIT: f32 = 1.0;
/// Source block size of [`ReadAhead`] (one positional read each).
pub const READ_AHEAD_BLOCK: usize = 1 << 20;
/// Blocks [`ReadAhead`] fetches in the background beyond the one being read.
const READ_AHEAD_BLOCKS: u64 = 2;
/// Fetched blocks kept for re-reads (the filter margin reaches back into the previous
/// block).
const READ_AHEAD_KEEP: usize = 3;
/// Fixed seed of the dither noise hash.
const DITHER_SEED: u64 = 0x6175_6469_6f6c_6962;
/// Tail of the `KSDATAFORMAT_SUBTYPE_*` GUIDs used by `WAVE_FORMAT_EXTENSIBLE`.
const SUBTYPE_TAIL: [u8; 14] = [
    0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71,
];

/// Sample encoding of a supported source WAV.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleFormat {
    /// 16-bit signed integer PCM.
    I16,
    /// 24-bit signed integer PCM (also lower valid bit depths in a 3-byte container).
    I24,
    /// 32-bit signed integer PCM.
    I32,
    /// 32-bit IEEE float.
    F32,
    /// 64-bit IEEE float.
    F64,
}

impl SampleFormat {
    /// Bytes per sample.
    pub fn bytes(self) -> usize {
        match self {
            Self::I16 => 2,
            Self::I24 => 3,
            Self::I32 | Self::F32 => 4,
            Self::F64 => 8,
        }
    }
    /// Container bit depth.
    pub fn bits(self) -> u16 {
        self.bytes() as u16 * 8
    }
    /// IEEE float samples.
    pub fn is_float(self) -> bool {
        matches!(self, Self::F32 | Self::F64)
    }
}

/// Layout of a supported source WAV file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WavInfo {
    /// Sample encoding.
    pub format: SampleFormat,
    /// Frames per second.
    pub sample_rate: u32,
    /// Interleaved channels.
    pub channels: u16,
    /// Byte offset of the first sample.
    pub data_offset: u64,
    /// Complete frames available in the file (a `data` size beyond the end of the file
    /// is cut to the file; a trailing partial frame is ignored).
    pub frames: u64,
}

impl WavInfo {
    /// Bytes per source frame.
    pub fn block_align(&self) -> usize {
        self.format.bytes() * usize::from(self.channels)
    }
    /// Duration in seconds.
    pub fn duration(&self) -> f64 {
        self.frames as f64 / f64::from(self.sample_rate)
    }
}

/// Why a file cannot be previewed in-process.
#[derive(Debug)]
pub enum WavError {
    /// Not a WAV file, or a WAV codec other than 16/24/32-bit integer or float PCM.
    Unsupported(String),
    /// Reading the file failed.
    Io(io::Error),
}

impl From<io::Error> for WavError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Positional reads (no shared cursor), so one open file can serve any byte range.
pub trait ReadAt {
    /// Read up to `buf.len()` bytes at `offset`; 0 means end of file.
    fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize>;

    /// Fill `buf` from `offset`; `UnexpectedEof` when the file is shorter.
    fn read_exact_at(&self, mut buf: &mut [u8], mut offset: u64) -> io::Result<()> {
        while !buf.is_empty() {
            match self.read_at(buf, offset) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "audio file ended early (truncated while streaming?)",
                    ));
                }
                Ok(n) => {
                    buf = &mut buf[n..];
                    offset += n as u64;
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}

impl ReadAt for std::fs::File {
    #[cfg(unix)]
    fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        std::os::unix::fs::FileExt::read_at(self, buf, offset)
    }
    #[cfg(windows)]
    fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        std::os::windows::fs::FileExt::seek_read(self, buf, offset)
    }
}

impl ReadAt for [u8] {
    fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        let start = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(self.len());
        let n = buf.len().min(self.len() - start);
        buf[..n].copy_from_slice(&self[start..start + n]);
        Ok(n)
    }
}

/// Read-ahead over a positional source for sequential rendering: reads aligned blocks
/// of [`READ_AHEAD_BLOCK`] bytes and keeps the next [`READ_AHEAD_BLOCKS`] blocks loading
/// on background threads, so a NAS latency spike on one read is hidden behind the
/// audio already buffered. Returns exactly the source bytes (output is unchanged). Not
/// `Sync`: one renderer thread uses it; abandoned prefetches finish on their own.
pub struct ReadAhead<R: ?Sized> {
    source: Arc<R>,
    len: u64,
    block: usize,
    state: RefCell<AheadState>,
}

#[derive(Default)]
struct AheadState {
    ready: VecDeque<(u64, Arc<Vec<u8>>)>,
    pending: Vec<(u64, JoinHandle<io::Result<Vec<u8>>>)>,
}

/// Blocking. Block `index` of `source` (shorter at the end of the file or if the file
/// shrank).
fn fetch_block<R: ReadAt + ?Sized>(
    source: &R,
    len: u64,
    block: usize,
    index: u64,
) -> io::Result<Vec<u8>> {
    let start = index * block as u64;
    let size = len.saturating_sub(start).min(block as u64) as usize;
    let mut buf = vec![0_u8; size];
    let mut filled = 0;
    while filled < size {
        match source.read_at(&mut buf[filled..], start + filled as u64) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    buf.truncate(filled);
    Ok(buf)
}

impl<R: ReadAt + Send + Sync + ?Sized + 'static> ReadAhead<R> {
    /// Read-ahead over the first `len` bytes of `source` with [`READ_AHEAD_BLOCK`] blocks.
    pub fn new(source: Arc<R>, len: u64) -> Self {
        Self::with_block(source, len, READ_AHEAD_BLOCK)
    }

    /// Read-ahead with another block size (tests).
    pub fn with_block(source: Arc<R>, len: u64, block: usize) -> Self {
        Self {
            source,
            len,
            block: block.max(1),
            state: RefCell::new(AheadState::default()),
        }
    }

    fn block(&self, index: u64) -> io::Result<Arc<Vec<u8>>> {
        let mut state = self.state.borrow_mut();
        let cached = state
            .ready
            .iter()
            .find(|(at, _)| *at == index)
            .map(|(_, data)| data.clone());
        let data = match cached {
            Some(data) => data,
            None => {
                let prefetched = state
                    .pending
                    .iter()
                    .position(|(at, _)| *at == index)
                    .map(|position| state.pending.swap_remove(position).1);
                // A failed or panicked prefetch is retried once in this thread.
                let data = match prefetched.map(JoinHandle::join) {
                    Some(Ok(Ok(data))) => data,
                    _ => fetch_block(&*self.source, self.len, self.block, index)?,
                };
                let data = Arc::new(data);
                state.ready.push_back((index, data.clone()));
                while state.ready.len() > READ_AHEAD_KEEP {
                    state.ready.pop_front();
                }
                data
            }
        };
        // Prefetches no longer ahead (a new range) are forgotten; they finish on their own.
        state
            .pending
            .retain(|(at, _)| *at > index && *at <= index + READ_AHEAD_BLOCKS);
        for next in index + 1..=index + READ_AHEAD_BLOCKS {
            if next.saturating_mul(self.block as u64) >= self.len {
                break;
            }
            if state.ready.iter().any(|(at, _)| *at == next)
                || state.pending.iter().any(|(at, _)| *at == next)
            {
                continue;
            }
            let (source, len, block) = (self.source.clone(), self.len, self.block);
            let spawned = std::thread::Builder::new()
                .name("pcm-read-ahead".into())
                .spawn(move || fetch_block(&*source, len, block, next));
            if let Ok(handle) = spawned {
                state.pending.push((next, handle));
            }
        }
        Ok(data)
    }
}

impl<R: ReadAt + Send + Sync + ?Sized + 'static> ReadAt for ReadAhead<R> {
    fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        if buf.is_empty() || offset >= self.len {
            return Ok(0);
        }
        let index = offset / self.block as u64;
        let data = self.block(index)?;
        let at = (offset - index * self.block as u64) as usize;
        if at >= data.len() {
            return Ok(0);
        }
        let n = buf.len().min(data.len() - at);
        buf[..n].copy_from_slice(&data[at..at + n]);
        Ok(n)
    }
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}
fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}
fn u64_at(bytes: &[u8], at: usize) -> u64 {
    u64::from(u32_at(bytes, at)) | (u64::from(u32_at(bytes, at + 4)) << 32)
}

fn codec_name(tag: u16) -> &'static str {
    match tag {
        0x0002 => "MS ADPCM",
        0x0006 => "A-law",
        0x0007 => "mu-law",
        0x0011 => "IMA ADPCM",
        0x0050 => "MPEG audio",
        0x0055 => "MP3",
        0x0161..=0x0163 => "WMA",
        0x2000 => "AC-3",
        _ => "compressed",
    }
}

/// Parse the `fmt ` chunk body (at least 16 bytes).
fn parse_fmt(fmt: &[u8]) -> Result<(SampleFormat, u32, u16), WavError> {
    let unsupported = |text: String| Err(WavError::Unsupported(text));
    let tag = u16_at(fmt, 0);
    let channels = u16_at(fmt, 2);
    let sample_rate = u32_at(fmt, 4);
    let block_align = u16_at(fmt, 12);
    let bits = u16_at(fmt, 14);
    let kind = if tag == 0xFFFE {
        if fmt.len() < 40 {
            return unsupported("WAVE_FORMAT_EXTENSIBLE fmt chunk is too short".into());
        }
        if fmt[26..40] != SUBTYPE_TAIL {
            return unsupported("WAVE_FORMAT_EXTENSIBLE with an unknown sub-format".into());
        }
        u16_at(fmt, 24)
    } else {
        tag
    };
    if kind != 1 && kind != 3 {
        return unsupported(format!("WAV codec 0x{kind:04X} ({})", codec_name(kind)));
    }
    if channels == 0 || channels > MAX_CHANNELS {
        return unsupported(format!(
            "{channels} channels (1..={MAX_CHANNELS} supported)"
        ));
    }
    if sample_rate == 0 || sample_rate > MAX_SOURCE_RATE {
        return unsupported(format!(
            "sample rate {sample_rate} Hz (1..={MAX_SOURCE_RATE} supported)"
        ));
    }
    if block_align == 0 || !block_align.is_multiple_of(channels) {
        return unsupported(format!(
            "block align {block_align} does not fit {channels} channels"
        ));
    }
    let container = block_align / channels;
    let format = match (kind, container) {
        (1, 2) => SampleFormat::I16,
        (1, 3) => SampleFormat::I24,
        (1, 4) => SampleFormat::I32,
        (3, 4) => SampleFormat::F32,
        (3, 8) => SampleFormat::F64,
        (1, _) => return unsupported(format!("{bits}-bit integer PCM")),
        (_, _) => return unsupported(format!("{bits}-bit float PCM")),
    };
    if u32::from(bits) > u32::from(container) * 8 {
        return unsupported(format!(
            "inconsistent fmt chunk: {bits}-bit samples in {container}-byte containers"
        ));
    }
    Ok((format, sample_rate, channels))
}

/// Blocking. Read the layout of a RIFF/RF64/BW64 WAV file of `len` bytes. Chunks before
/// and after `fmt `/`data` (`bext`, `iXML`, `LIST`, `JUNK`, ...) are skipped.
pub fn parse_wav<R: ReadAt + ?Sized>(source: &R, len: u64) -> Result<WavInfo, WavError> {
    let unsupported = |text: &str| Err(WavError::Unsupported(text.into()));
    if len < 12 {
        return unsupported("not a WAV file (too short)");
    }
    let mut head = [0_u8; 12];
    source.read_exact_at(&mut head, 0)?;
    let rf64 = match &head[0..4] {
        b"RIFF" => false,
        b"RF64" | b"BW64" => true,
        b"RIFX" => return unsupported("big-endian RIFX WAV"),
        _ => return unsupported("not a WAV file"),
    };
    if &head[8..12] != b"WAVE" {
        return unsupported("not a WAV file (RIFF form is not WAVE)");
    }
    let mut position = 12_u64;
    let mut ds64_data_size = None;
    let mut fmt = None;
    let mut data = None;
    // Read a chunk body that must lie inside the file (a cut-off header is 415, not I/O).
    let read_body = |buf: &mut [u8], at: u64, what: &str| -> Result<(), WavError> {
        if len.saturating_sub(at) < buf.len() as u64 {
            return Err(WavError::Unsupported(format!(
                "WAV header is truncated ({what} chunk ends after the end of the file)"
            )));
        }
        Ok(source.read_exact_at(buf, at)?)
    };
    for _ in 0..MAX_RIFF_CHUNKS {
        if len.saturating_sub(position) < 8 {
            break;
        }
        let mut header = [0_u8; 8];
        source.read_exact_at(&mut header, position)?;
        let body = position + 8;
        let mut size = u64::from(u32_at(&header, 4));
        match &header[0..4] {
            b"ds64" if rf64 => {
                if size >= 16 {
                    let mut ds64 = [0_u8; 16];
                    read_body(&mut ds64, body, "ds64")?;
                    ds64_data_size = Some(u64_at(&ds64, 8));
                }
            }
            b"fmt " => {
                if size < 16 {
                    return unsupported("fmt chunk is too short");
                }
                let mut buf = vec![0_u8; size.min(64) as usize];
                read_body(&mut buf, body, "fmt")?;
                fmt = Some(parse_fmt(&buf)?);
            }
            b"data" => {
                if rf64 && size == u64::from(u32::MAX) {
                    size = ds64_data_size
                        .ok_or_else(|| WavError::Unsupported("RF64 file without ds64".into()))?;
                }
                let available = len.saturating_sub(body);
                // 0 / 0xFFFFFFFF are placeholders of unfinished or streamed recordings.
                let usable = if size == 0 || size == u64::from(u32::MAX) {
                    available
                } else {
                    size.min(available)
                };
                data = Some((body, usable));
                if fmt.is_some() {
                    break;
                }
            }
            _ => {}
        }
        position = next_chunk(source, len, body, size)?;
    }
    let Some((format, sample_rate, channels)) = fmt else {
        return unsupported("WAV file without a fmt chunk");
    };
    let Some((data_offset, data_len)) = data else {
        return unsupported("WAV file without a data chunk");
    };
    let block_align = (format.bytes() * usize::from(channels)) as u64;
    Ok(WavInfo {
        format,
        sample_rate,
        channels,
        data_offset,
        frames: data_len / block_align,
    })
}

/// Position of the chunk after the one whose body starts at `body`: RIFF pads odd-sized
/// chunks with one byte, but some writers omit it; when the padded position holds no
/// chunk ID and the unpadded one does, the unpadded position is used.
fn next_chunk<R: ReadAt + ?Sized>(source: &R, len: u64, body: u64, size: u64) -> io::Result<u64> {
    let end = body.saturating_add(size);
    if size & 1 == 0 {
        return Ok(end);
    }
    if len.saturating_sub(end) >= 9 {
        let mut ids = [0_u8; 5];
        source.read_exact_at(&mut ids, end)?;
        let chunk_id = |id: &[u8]| id.iter().all(|b| (0x20..=0x7E).contains(b));
        if chunk_id(&ids[0..4]) && !chunk_id(&ids[1..5]) {
            return Ok(end);
        }
    }
    Ok(end.saturating_add(1))
}

/// Smallest integer factor `n >= 1` dividing `source_rate` with `source_rate / n <=
/// requested` (no request: 1). 96000 -> 48000 is 2, 192000 -> 48000 is 4, 88200 ->
/// 48000 is 2 (44100), 44100 -> 48000 is 1. `None` when that factor would exceed
/// [`MAX_DECIMATION_FACTOR`] (e.g. a prime source rate).
pub fn decimation_factor(source_rate: u32, requested: Option<u32>) -> Option<u32> {
    let Some(requested) = requested.filter(|&rate| rate > 0) else {
        return Some(1);
    };
    if requested >= source_rate {
        return Some(1);
    }
    (source_rate.div_ceil(requested)..=MAX_DECIMATION_FACTOR)
        .find(|&factor| source_rate.is_multiple_of(factor))
}

/// Resolved output of one PCM preview.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PcmPlan {
    /// The source layout.
    pub source: WavInfo,
    /// Integer decimation factor (1 = no rate change).
    pub factor: u32,
    /// Output rate: `source.sample_rate / factor`.
    pub sample_rate: u32,
    /// Output bit depth (16 or 24).
    pub bits: u16,
    /// Output channels.
    pub channels: u16,
    /// Channel folding.
    pub mix: Mix,
    /// Output frames: `floor(source.frames / factor)`.
    pub frames: u64,
    /// TPDF dither on 16-bit output whenever samples are not already exact 16-bit values.
    pub dither: bool,
}

impl PcmPlan {
    /// Plan the output for resolved parameters (see `crate::preview` for the rules).
    pub fn new(source: WavInfo, factor: u32, bits: u16, channels: u16, mix: Mix) -> Self {
        let identity = source.format == SampleFormat::I16 && factor == 1 && mix == Mix::Keep;
        Self {
            sample_rate: source.sample_rate / factor,
            frames: source.frames / u64::from(factor),
            dither: bits == 16 && !identity,
            source,
            factor,
            bits,
            channels,
            mix,
        }
    }
    /// Bytes per output frame.
    pub fn block_align(&self) -> u64 {
        u64::from(self.channels) * u64::from(self.bits / 8)
    }
    /// Bytes of sample data.
    pub fn data_len(&self) -> u64 {
        self.frames * self.block_align()
    }
    /// Size of the whole WAV (`Content-Length` of a full response).
    pub fn total_len(&self) -> u64 {
        HEADER_BYTES + self.data_len()
    }
    /// Canonical WAV header (`WAVE_FORMAT_PCM`). Sizes beyond 4 GiB are written as
    /// 0xFFFFFFFF; `Content-Length` stays exact.
    pub fn header(&self) -> [u8; HEADER_BYTES as usize] {
        let data = self.data_len();
        let data32 = u32::try_from(data).unwrap_or(u32::MAX);
        let riff32 = u32::try_from(data + 36).unwrap_or(u32::MAX);
        let block_align = self.block_align() as u16;
        let mut header = [0_u8; HEADER_BYTES as usize];
        header[0..4].copy_from_slice(b"RIFF");
        header[4..8].copy_from_slice(&riff32.to_le_bytes());
        header[8..16].copy_from_slice(b"WAVEfmt ");
        header[16..20].copy_from_slice(&16_u32.to_le_bytes());
        header[20..22].copy_from_slice(&1_u16.to_le_bytes());
        header[22..24].copy_from_slice(&self.channels.to_le_bytes());
        header[24..28].copy_from_slice(&self.sample_rate.to_le_bytes());
        let byte_rate = self.sample_rate.saturating_mul(u32::from(block_align));
        header[28..32].copy_from_slice(&byte_rate.to_le_bytes());
        header[32..34].copy_from_slice(&block_align.to_le_bytes());
        header[34..36].copy_from_slice(&self.bits.to_le_bytes());
        header[36..40].copy_from_slice(b"data");
        header[40..44].copy_from_slice(&data32.to_le_bytes());
        header
    }
    /// Strong `ETag`: the source file's stream ETag plus everything that shapes the
    /// output bytes (parameters and [`ALGORITHM_VERSION`]).
    pub fn etag(&self, source_etag: &str) -> String {
        let inner = source_etag.trim_start_matches("W/").trim_matches('"');
        format!(
            "\"{inner}-pcm{ALGORITHM_VERSION}-{}-{}-{}\"",
            self.sample_rate, self.bits, self.channels
        )
    }
}

/// Renders the bytes of a [`PcmPlan`] from the source file.
pub struct Renderer {
    plan: PcmPlan,
    /// Low-pass FIR (zero-padded to a multiple of 8); empty when `factor == 1`.
    taps: Vec<f32>,
    /// Frames before (and after) the centre tap.
    half: usize,
}

impl Renderer {
    /// Design the filter for `plan`.
    pub fn new(plan: PcmPlan) -> Self {
        let (taps, half) = if plan.factor > 1 {
            design_lowpass(plan.factor)
        } else {
            (Vec::new(), 0)
        };
        Self { plan, taps, half }
    }

    /// The plan being rendered.
    pub fn plan(&self) -> &PcmPlan {
        &self.plan
    }

    /// FIR length before zero padding (`2 * 24 * factor + 1`, 0 without decimation).
    pub fn filter_taps(&self) -> usize {
        if self.taps.is_empty() {
            0
        } else {
            2 * self.half + 1
        }
    }

    fn chunk_frames(&self) -> u64 {
        (CHUNK_BYTES / self.plan.block_align().max(1)).max(256)
    }

    /// Blocking. Append output frames `start..end` (little-endian samples) to `out`.
    pub fn render_frames<R: ReadAt + ?Sized>(
        &self,
        source: &R,
        start: u64,
        end: u64,
        out: &mut Vec<u8>,
    ) -> io::Result<()> {
        if start >= end {
            return Ok(());
        }
        let plan = &self.plan;
        let factor = i64::from(plan.factor);
        let window = if self.taps.is_empty() {
            1
        } else {
            self.taps.len()
        };
        // Source frame of plane index 0 and the number of frames the chunk needs.
        let first = start as i64 * factor - self.half as i64;
        let count = (end - start - 1) as usize * plan.factor as usize + window;
        let read_from = first.max(0);
        let read_to = (first + count as i64).min(plan.source.frames as i64);
        let block_align = plan.source.block_align();
        let mut raw = vec![0_u8; (read_to - read_from).max(0) as usize * block_align];
        if !raw.is_empty() {
            source.read_exact_at(
                &mut raw,
                plan.source.data_offset + read_from as u64 * block_align as u64,
            )?;
        }
        // Frames outside the file stay zero.
        let mut planes = vec![vec![0_f32; count]; usize::from(plan.channels)];
        decode_and_mix(
            &raw,
            plan.source.format,
            usize::from(plan.source.channels),
            plan.mix,
            &mut planes,
            (read_from - first) as usize,
        );
        let bytes_per_sample = usize::from(plan.bits / 8);
        out.reserve((end - start) as usize * usize::from(plan.channels) * bytes_per_sample);
        let scale = if plan.bits == 16 {
            32_768.0
        } else {
            8_388_608.0
        };
        for frame in start..end {
            let at = (frame - start) as usize * plan.factor as usize;
            for (channel, plane) in planes.iter().enumerate() {
                let value = if self.taps.is_empty() {
                    plane[at]
                } else {
                    dot(&self.taps, &plane[at..at + window])
                };
                let noise = if plan.dither {
                    tpdf(frame, channel as u16)
                } else {
                    0.0
                };
                let sample = (f64::from(value) * scale + noise)
                    .round()
                    .clamp(-scale, scale - 1.0) as i32;
                out.extend_from_slice(&sample.to_le_bytes()[..bytes_per_sample]);
            }
        }
        Ok(())
    }

    /// Blocking. Send bytes `first..=last` of the WAV (header included) to `sink` in
    /// chunks of about 64 KiB; stops early when `sink` returns `false` (client gone).
    pub fn render_range<R: ReadAt + ?Sized>(
        &self,
        source: &R,
        first: u64,
        last: u64,
        mut sink: impl FnMut(Vec<u8>) -> bool,
    ) -> io::Result<()> {
        let last = last.min(self.plan.total_len().saturating_sub(1));
        if first > last {
            return Ok(());
        }
        if first < HEADER_BYTES {
            let header = self.plan.header();
            let end = (last + 1).min(HEADER_BYTES) as usize;
            if !sink(header[first as usize..end].to_vec()) {
                return Ok(());
            }
        }
        if last < HEADER_BYTES {
            return Ok(());
        }
        let block_align = self.plan.block_align();
        let data_first = first.max(HEADER_BYTES) - HEADER_BYTES;
        let data_end = last + 1 - HEADER_BYTES;
        let mut frame = data_first / block_align;
        let end_frame = data_end.div_ceil(block_align);
        let step = self.chunk_frames();
        while frame < end_frame {
            let next = (frame + step).min(end_frame);
            let mut chunk = Vec::new();
            self.render_frames(source, frame, next, &mut chunk)?;
            let chunk_start = frame * block_align;
            let lo = data_first.saturating_sub(chunk_start) as usize;
            let hi = (data_end.min(next * block_align) - chunk_start) as usize;
            if hi < chunk.len() {
                chunk.truncate(hi);
            }
            if lo > 0 {
                chunk.drain(..lo);
            }
            if !sink(chunk) {
                return Ok(());
            }
            frame = next;
        }
        Ok(())
    }
}

/// Convert interleaved source samples to normalized `f32` and fold channels into
/// `planes`, starting at plane index `offset`.
fn decode_and_mix(
    raw: &[u8],
    format: SampleFormat,
    source_channels: usize,
    mix: Mix,
    planes: &mut [Vec<f32>],
    offset: usize,
) {
    let width = format.bytes();
    let decode = |bytes: &[u8]| -> f32 {
        match format {
            SampleFormat::I16 => f32::from(i16::from_le_bytes([bytes[0], bytes[1]])) / 32_768.0,
            SampleFormat::I24 => {
                let value = i32::from_le_bytes([0, bytes[0], bytes[1], bytes[2]]) >> 8;
                value as f32 / 8_388_608.0
            }
            SampleFormat::I32 => {
                i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f32
                    / 2_147_483_648.0
            }
            SampleFormat::F32 => {
                clamp_float(f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
            }
            SampleFormat::F64 => clamp_float(f64::from_le_bytes([
                bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
            ]) as f32),
        }
    };
    let evens = source_channels.div_ceil(2) as f32;
    let odds = (source_channels / 2).max(1) as f32;
    let all = source_channels as f32;
    for (index, frame) in raw.chunks_exact(width * source_channels).enumerate() {
        let at = offset + index;
        match mix {
            Mix::Keep => {
                for (plane, sample) in planes.iter_mut().zip(frame.chunks_exact(width)) {
                    plane[at] = decode(sample);
                }
            }
            Mix::Mono => {
                let sum: f32 = frame.chunks_exact(width).map(decode).sum();
                planes[0][at] = sum / all;
            }
            Mix::Pairs => {
                let (mut left, mut right) = (0.0_f32, 0.0_f32);
                for (channel, sample) in frame.chunks_exact(width).enumerate() {
                    if channel % 2 == 0 {
                        left += decode(sample);
                    } else {
                        right += decode(sample);
                    }
                }
                planes[0][at] = left / evens;
                planes[1][at] = right / odds;
            }
        }
    }
}

/// Float sample limited to [`FLOAT_LIMIT`]; NaN becomes 0.
fn clamp_float(value: f32) -> f32 {
    if value.is_nan() {
        0.0
    } else {
        value.clamp(-FLOAT_LIMIT, FLOAT_LIMIT)
    }
}

/// Dot product with eight independent accumulators (vectorizes; fixed summation order,
/// so results are deterministic). Both slices have the same length, a multiple of 8.
fn dot(taps: &[f32], samples: &[f32]) -> f32 {
    let mut acc = [0.0_f32; 8];
    for (t, s) in taps.chunks_exact(8).zip(samples.chunks_exact(8)) {
        for ((a, t), s) in acc.iter_mut().zip(t).zip(s) {
            *a += t * s;
        }
    }
    ((acc[0] + acc[4]) + (acc[1] + acc[5])) + ((acc[2] + acc[6]) + (acc[3] + acc[7]))
}

/// Kaiser-windowed sinc low-pass for decimation by `factor`, normalized to unity DC
/// gain; returns the taps (zero-padded to a multiple of 8) and the half-length.
fn design_lowpass(factor: u32) -> (Vec<f32>, usize) {
    let half = HALF_TAPS_PER_STEP * factor as usize;
    let len = 2 * half + 1;
    let cutoff = CUTOFF / f64::from(factor);
    let norm = bessel_i0(KAISER_BETA);
    let raw: Vec<f64> = (0..len)
        .map(|index| {
            let m = index as f64 - half as f64;
            let sinc = if m == 0.0 {
                2.0 * cutoff
            } else {
                (2.0 * std::f64::consts::PI * cutoff * m).sin() / (std::f64::consts::PI * m)
            };
            let r = m / half as f64;
            let window = bessel_i0(KAISER_BETA * (1.0 - r * r).max(0.0).sqrt()) / norm;
            sinc * window
        })
        .collect();
    let sum: f64 = raw.iter().sum();
    let mut taps: Vec<f32> = raw.iter().map(|tap| (tap / sum) as f32).collect();
    taps.resize(len.div_ceil(8) * 8, 0.0);
    (taps, half)
}

/// Modified Bessel function of the first kind, order 0 (power series).
fn bessel_i0(x: f64) -> f64 {
    let quarter = x * x / 4.0;
    let (mut sum, mut term) = (1.0, 1.0);
    for k in 1..64 {
        term *= quarter / f64::from(k * k);
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    sum
}

fn splitmix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Deterministic TPDF dither in LSB units (-1..1, triangular): the difference of two
/// uniform values derived from a hash of the output frame index and channel.
pub fn tpdf(frame: u64, channel: u16) -> f64 {
    let bits = splitmix64(frame ^ (u64::from(channel) << 48) ^ DITHER_SEED);
    let a = (bits >> 40) as f64;
    let b = ((bits >> 16) & 0xFF_FFFF) as f64;
    (a - b) / 16_777_216.0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A WAV file in memory: `header_extra` chunks go between `fmt ` and `data`.
    fn wav_bytes(tag: u16, channels: u16, rate: u32, container: u16, data: &[u8]) -> Vec<u8> {
        let block_align = channels * container / 8;
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&tag.to_le_bytes());
        fmt.extend_from_slice(&channels.to_le_bytes());
        fmt.extend_from_slice(&rate.to_le_bytes());
        fmt.extend_from_slice(&rate.wrapping_mul(u32::from(block_align)).to_le_bytes());
        fmt.extend_from_slice(&block_align.to_le_bytes());
        fmt.extend_from_slice(&container.to_le_bytes());
        let mut bytes = b"RIFF\0\0\0\0WAVE".to_vec();
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&fmt);
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(data);
        bytes
    }

    fn i24_bytes(samples: &[i32]) -> Vec<u8> {
        samples
            .iter()
            .flat_map(|s| s.to_le_bytes()[..3].to_vec())
            .collect()
    }

    fn render_all(plan: PcmPlan, file: &[u8]) -> Vec<u8> {
        let renderer = Renderer::new(plan);
        let mut out = Vec::new();
        let total = renderer.plan().total_len();
        renderer
            .render_range(file, 0, total - 1, |chunk| {
                out.extend_from_slice(&chunk);
                true
            })
            .unwrap();
        assert_eq!(out.len() as u64, total);
        out
    }

    fn samples_i16(wav: &[u8]) -> Vec<i16> {
        wav[44..]
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect()
    }

    /// Mono 24-bit sine at `freq` Hz (amplitude 0.5).
    fn sine_wav(rate: u32, freq: f64, frames: usize) -> Vec<u8> {
        let samples: Vec<i32> = (0..frames)
            .map(|i| {
                let x = (2.0 * std::f64::consts::PI * freq * i as f64 / f64::from(rate)).sin();
                (x * 0.5 * 8_388_607.0).round() as i32
            })
            .collect();
        wav_bytes(1, 1, rate, 24, &i24_bytes(&samples))
    }

    fn rms(values: &[f64]) -> f64 {
        (values.iter().map(|v| v * v).sum::<f64>() / values.len() as f64).sqrt()
    }

    #[test]
    fn decimation_factors_divide_the_source_rate() {
        assert_eq!(decimation_factor(96_000, Some(48_000)), Some(2));
        assert_eq!(decimation_factor(192_000, Some(48_000)), Some(4));
        assert_eq!(decimation_factor(88_200, Some(48_000)), Some(2));
        assert_eq!(decimation_factor(44_100, Some(48_000)), Some(1));
        assert_eq!(decimation_factor(48_000, Some(48_000)), Some(1));
        assert_eq!(decimation_factor(96_000, None), Some(1));
        assert_eq!(decimation_factor(96_000, Some(44_100)), Some(3)); // 32000
        assert_eq!(decimation_factor(96_000, Some(13_000)), Some(8)); // 12000
        assert_eq!(decimation_factor(44_100, Some(8_000)), Some(6)); // 7350
        assert_eq!(decimation_factor(96_000, Some(192_000)), Some(1));
        assert_eq!(decimation_factor(768_000, Some(8_000)), Some(96));
        assert_eq!(decimation_factor(352_800, Some(8_000)), Some(45)); // 7840
        // Prime or odd rates without a small enough factor (was: factor 96001, 1 Hz).
        assert_eq!(decimation_factor(96_001, Some(48_000)), None);
        assert_eq!(decimation_factor(768_000, Some(7_999)), None);
        assert_eq!(decimation_factor(96_001, None), Some(1));
    }

    #[test]
    fn parses_pcm_extensible_bext_and_rf64_layouts() {
        let data = i24_bytes(&[1, 2, 3, 4, 5, 6]);
        let plain = wav_bytes(1, 2, 96_000, 24, &data);
        let info = parse_wav(plain.as_slice(), plain.len() as u64).unwrap();
        assert_eq!(info.format, SampleFormat::I24);
        assert_eq!(
            (info.sample_rate, info.channels, info.frames),
            (96_000, 2, 3)
        );
        assert_eq!(info.data_offset, 44);

        // bext before fmt, odd-sized LIST chunk (padded) between fmt and data.
        let mut file = b"RIFF\0\0\0\0WAVEbext".to_vec();
        file.extend_from_slice(&10_u32.to_le_bytes());
        file.extend_from_slice(&[0; 10]);
        file.extend_from_slice(&plain[12..36]);
        file.extend_from_slice(b"LIST");
        file.extend_from_slice(&3_u32.to_le_bytes());
        file.extend_from_slice(&[1, 2, 3, 0]);
        file.extend_from_slice(&plain[36..]);
        let info = parse_wav(file.as_slice(), file.len() as u64).unwrap();
        assert_eq!(info.frames, 3);
        assert_eq!(info.data_offset, file.len() as u64 - data.len() as u64);

        // WAVE_FORMAT_EXTENSIBLE float32.
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&0xFFFE_u16.to_le_bytes());
        fmt.extend_from_slice(&1_u16.to_le_bytes());
        fmt.extend_from_slice(&48_000_u32.to_le_bytes());
        fmt.extend_from_slice(&192_000_u32.to_le_bytes());
        fmt.extend_from_slice(&4_u16.to_le_bytes());
        fmt.extend_from_slice(&32_u16.to_le_bytes());
        fmt.extend_from_slice(&22_u16.to_le_bytes());
        fmt.extend_from_slice(&32_u16.to_le_bytes());
        fmt.extend_from_slice(&4_u32.to_le_bytes());
        fmt.extend_from_slice(&3_u16.to_le_bytes());
        fmt.extend_from_slice(&SUBTYPE_TAIL);
        let mut file = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
        file.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
        file.extend_from_slice(&fmt);
        file.extend_from_slice(b"data");
        file.extend_from_slice(&8_u32.to_le_bytes());
        file.extend_from_slice(&0.5_f32.to_le_bytes());
        file.extend_from_slice(&(-0.25_f32).to_le_bytes());
        let info = parse_wav(file.as_slice(), file.len() as u64).unwrap();
        assert_eq!(info.format, SampleFormat::F32);
        assert_eq!(info.frames, 2);

        // RF64 with ds64 and a 0xFFFFFFFF data size.
        let mut file = b"RF64\xff\xff\xff\xffWAVEds64".to_vec();
        file.extend_from_slice(&28_u32.to_le_bytes());
        file.extend_from_slice(&0_u64.to_le_bytes());
        file.extend_from_slice(&(data.len() as u64).to_le_bytes());
        file.extend_from_slice(&3_u64.to_le_bytes());
        file.extend_from_slice(&0_u32.to_le_bytes());
        file.extend_from_slice(&plain[12..36]);
        file.extend_from_slice(b"data\xff\xff\xff\xff");
        file.extend_from_slice(&data);
        let info = parse_wav(file.as_slice(), file.len() as u64).unwrap();
        assert_eq!((info.format, info.frames), (SampleFormat::I24, 3));

        // A data size beyond the end of the file is cut to whole frames in the file.
        let mut truncated = plain.clone();
        truncated.truncate(plain.len() - 4);
        let info = parse_wav(truncated.as_slice(), truncated.len() as u64).unwrap();
        assert_eq!(info.frames, 2);
    }

    #[test]
    fn rejects_compressed_and_foreign_files() {
        let adpcm = wav_bytes(0x0011, 1, 48_000, 4, &[0; 8]);
        let reject = |bytes: &[u8]| match parse_wav(bytes, bytes.len() as u64) {
            Err(WavError::Unsupported(message)) => message,
            other => panic!("accepted: {other:?}"),
        };
        assert!(reject(&adpcm).contains("IMA ADPCM"), "{}", reject(&adpcm));
        assert!(reject(&wav_bytes(1, 1, 48_000, 8, &[0; 8])).contains("8-bit"));
        assert!(reject(b"OggS\0\x02\0\0\0\0\0\0\0\0").contains("not a WAV"));
        assert!(reject(b"ID3\x04\0\0\0\0\0\0\xff\xfb").contains("not a WAV"));
        assert!(reject(b"RIFF\0\0\0\0AVI LIST").contains("not WAVE"));
        assert!(reject(b"RIFF\0\0\0\0WAVEdata\0\0\0\0").contains("fmt"));
    }

    #[test]
    fn header_and_size_math_follow_the_plan() {
        let source = WavInfo {
            format: SampleFormat::I24,
            sample_rate: 96_000,
            channels: 2,
            data_offset: 100,
            frames: 960_001,
        };
        let plan = PcmPlan::new(source.clone(), 2, 16, 2, Mix::Keep);
        assert_eq!(
            (plan.sample_rate, plan.frames, plan.block_align()),
            (48_000, 480_000, 4)
        );
        assert_eq!(plan.total_len(), 44 + 480_000 * 4);
        assert!(plan.dither);
        let header = plan.header();
        assert_eq!(&header[0..4], b"RIFF");
        assert_eq!(u32_at(&header, 4), 36 + 1_920_000);
        assert_eq!(u16_at(&header, 20), 1);
        assert_eq!(u16_at(&header, 22), 2);
        assert_eq!(u32_at(&header, 24), 48_000);
        assert_eq!(u32_at(&header, 28), 192_000);
        assert_eq!(u16_at(&header, 32), 4);
        assert_eq!(u16_at(&header, 34), 16);
        assert_eq!(u32_at(&header, 40), 1_920_000);
        // The header parses back as the output layout.
        let info = parse_wav(header.as_slice(), header.len() as u64);
        assert!(matches!(info, Ok(WavInfo { frames: 0, .. })));
        let mono = PcmPlan::new(source.clone(), 1, 24, 1, Mix::Mono);
        assert_eq!(
            (mono.block_align(), mono.frames, mono.dither),
            (3, 960_001, false)
        );
        let etag = plan.etag("\"abc-1\"");
        assert_eq!(etag, "\"abc-1-pcm2-48000-16-2\"");
        assert_ne!(etag, mono.etag("\"abc-1\""));
        let plain16 = WavInfo {
            format: SampleFormat::I16,
            ..source
        };
        assert!(!PcmPlan::new(plain16.clone(), 1, 16, 2, Mix::Keep).dither);
        assert!(PcmPlan::new(plain16, 2, 16, 2, Mix::Keep).dither);
    }

    #[test]
    fn identity_plans_reproduce_the_source_samples() {
        let samples: Vec<i32> = (0..1000)
            .map(|i| (i * 7919 % 16_000_000) - 8_000_000)
            .collect();
        let file = wav_bytes(1, 2, 48_000, 24, &i24_bytes(&samples));
        let info = parse_wav(file.as_slice(), file.len() as u64).unwrap();
        let out = render_all(PcmPlan::new(info, 1, 24, 2, Mix::Keep), &file);
        assert_eq!(&out[44..], &file[44..]);
    }

    #[test]
    fn mono_and_pair_folds_average_channels() {
        // 16-bit, 4 channels: one frame (1000, 3000, -2000, 0).
        let frame: Vec<u8> = [1000_i16, 3000, -2000, 0]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();
        let file = wav_bytes(1, 4, 48_000, 16, &frame.repeat(4));
        let info = parse_wav(file.as_slice(), file.len() as u64).unwrap();
        let mono = render_all(PcmPlan::new(info.clone(), 1, 24, 1, Mix::Mono), &file);
        let first = i32::from_le_bytes([0, mono[44], mono[45], mono[46]]) >> 8;
        assert_eq!(first, 500 * 256); // (1000 + 3000 - 2000 + 0) / 4, in 24-bit units
        let pairs = render_all(PcmPlan::new(info, 1, 24, 2, Mix::Pairs), &file);
        let left = i32::from_le_bytes([0, pairs[44], pairs[45], pairs[46]]) >> 8;
        let right = i32::from_le_bytes([0, pairs[47], pairs[48], pairs[49]]) >> 8;
        assert_eq!((left, right), (-500 * 256, 1500 * 256));
    }

    #[test]
    fn decimator_keeps_the_pass_band_and_rejects_aliases() {
        let rate = 96_000;
        let frames = 96_000;
        // Output rate 48 kHz: 10 kHz stays, 19 kHz (near the pass-band edge) stays, and
        // tones at 26/30/40 kHz (which would alias to 22/18/8 kHz) disappear.
        for (freq, passes) in [
            (1_000.0, true),
            (10_000.0, true),
            (19_000.0, true),
            (26_000.0, false),
            (30_000.0, false),
            (40_000.0, false),
        ] {
            let file = sine_wav(rate, freq, frames);
            let info = parse_wav(file.as_slice(), file.len() as u64).unwrap();
            let plan = PcmPlan::new(info, 2, 24, 1, Mix::Keep);
            let out = render_all(plan, &file);
            let values: Vec<f64> = out[44..]
                .chunks_exact(3)
                .map(|b| f64::from(i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) / 8_388_608.0)
                .collect();
            // Skip the filter edges at both ends.
            let level = rms(&values[1000..values.len() - 1000]);
            let reference = 0.5 / 2.0_f64.sqrt();
            let db = 20.0 * (level / reference).log10();
            if passes {
                assert!(db.abs() < 0.05, "{freq} Hz: {db:.3} dB");
            } else {
                assert!(db < -60.0, "{freq} Hz alias only {db:.1} dB down");
            }
        }
        // 192 kHz -> 48 kHz (factor 4): a 50 kHz tone is gone, 15 kHz stays.
        for (freq, passes) in [(15_000.0, true), (50_000.0, false), (70_000.0, false)] {
            let file = sine_wav(192_000, freq, 192_000);
            let info = parse_wav(file.as_slice(), file.len() as u64).unwrap();
            let out = render_all(PcmPlan::new(info, 4, 24, 1, Mix::Keep), &file);
            let values: Vec<f64> = out[44..]
                .chunks_exact(3)
                .map(|b| f64::from(i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) / 8_388_608.0)
                .collect();
            let level = rms(&values[1000..values.len() - 1000]);
            let db = 20.0 * (level / (0.5 / 2.0_f64.sqrt())).log10();
            assert!(
                if passes { db.abs() < 0.05 } else { db < -60.0 },
                "{freq} Hz: {db:.1} dB"
            );
        }
    }

    #[test]
    fn any_byte_range_equals_the_same_bytes_of_the_full_render() {
        let samples: Vec<i32> = (0..200_011)
            .flat_map(|i| {
                let x = ((i * 37) % 1000) * 7000 - 3_500_000;
                [x, -x / 3]
            })
            .collect();
        let file = wav_bytes(1, 2, 96_000, 24, &i24_bytes(&samples));
        let info = parse_wav(file.as_slice(), file.len() as u64).unwrap();
        for (bits, mix, channels) in [(16, Mix::Keep, 2), (16, Mix::Mono, 1), (24, Mix::Keep, 2)] {
            let plan = PcmPlan::new(info.clone(), 2, bits, channels, mix);
            let full = render_all(plan.clone(), &file);
            let renderer = Renderer::new(plan);
            let total = full.len() as u64;
            for (first, last) in [
                (0, 0),
                (0, 43),
                (10, 50),
                (44, 44),
                (45, 46),
                (1001, 30_001),
                (total / 2 - 1, total / 2 + 70_000),
                (total - 3, total - 1),
                (65_573, total - 1),
            ] {
                let last = last.min(total - 1);
                assert!(first <= last && first < total);
                let mut part = Vec::new();
                renderer
                    .render_range(file.as_slice(), first, last, |chunk| {
                        part.extend_from_slice(&chunk);
                        true
                    })
                    .unwrap();
                assert_eq!(
                    part,
                    full[first as usize..=last as usize],
                    "{bits} bit {mix:?} range {first}-{last}"
                );
            }
        }
    }

    #[test]
    fn dither_is_deterministic_triangular_and_one_lsb() {
        assert_eq!(tpdf(12345, 1), tpdf(12345, 1));
        assert_ne!(tpdf(12345, 0), tpdf(12345, 1));
        assert_ne!(tpdf(12345, 0), tpdf(12346, 0));
        let values: Vec<f64> = (0..200_000).map(|frame| tpdf(frame, 0)).collect();
        assert!(values.iter().all(|v| v.abs() < 1.0));
        let mean = values.iter().sum::<f64>() / values.len() as f64;
        let variance = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / values.len() as f64;
        assert!(mean.abs() < 0.01, "{mean}");
        // TPDF over (-1, 1) has variance 1/6.
        assert!((variance - 1.0 / 6.0).abs() < 0.005, "{variance}");
        // A 24-bit source rendered twice to 16 bits gives identical bytes; the dither
        // decorrelates a constant low-level signal (not every sample equal).
        let samples = vec![100_i32; 4000];
        let file = wav_bytes(1, 1, 48_000, 24, &i24_bytes(&samples));
        let info = parse_wav(file.as_slice(), file.len() as u64).unwrap();
        let plan = PcmPlan::new(info, 1, 16, 1, Mix::Keep);
        let first = render_all(plan.clone(), &file);
        assert_eq!(first, render_all(plan, &file));
        let values = samples_i16(&first);
        assert!(values.iter().any(|&v| v != values[0]));
        assert!(values.iter().all(|&v| (-1..=2).contains(&v)), "{values:?}");
    }

    #[test]
    fn malformed_headers_are_rejected_without_panics() {
        let reject = |bytes: &[u8]| match parse_wav(bytes, bytes.len() as u64) {
            Err(WavError::Unsupported(message)) => message,
            other => panic!("not rejected as unsupported: {other:?}"),
        };
        // fmt and ds64 bodies cut off by the end of the file: 415, not an I/O error (500).
        let plain = wav_bytes(1, 2, 48_000, 16, &[0; 8]);
        assert!(
            reject(&plain[..28]).contains("truncated"),
            "{}",
            reject(&plain[..28])
        );
        let mut rf64 = b"RF64\xff\xff\xff\xffWAVEds64".to_vec();
        rf64.extend_from_slice(&28_u32.to_le_bytes());
        rf64.extend_from_slice(&[0; 6]);
        assert!(reject(&rf64).contains("truncated"), "{}", reject(&rf64));
        // RF64 whose ds64 claims a u64::MAX data size, data before fmt: no overflow.
        let mut rf64 = b"RF64\xff\xff\xff\xffWAVEds64".to_vec();
        rf64.extend_from_slice(&28_u32.to_le_bytes());
        rf64.extend_from_slice(&0_u64.to_le_bytes());
        rf64.extend_from_slice(&u64::MAX.to_le_bytes());
        rf64.extend_from_slice(&0_u64.to_le_bytes());
        rf64.extend_from_slice(&0_u32.to_le_bytes());
        rf64.extend_from_slice(b"data\xff\xff\xff\xff");
        rf64.extend_from_slice(&[0; 16]);
        assert!(reject(&rf64).contains("fmt"), "{}", reject(&rf64));
        // Sample rates from corrupt headers (4 GHz overflowed the output byte rate).
        let fast = wav_bytes(1, 1, 4_000_000_000, 16, &[0; 4]);
        assert!(reject(&fast).contains("sample rate"), "{}", reject(&fast));
        // A bit depth larger than the sample container (block_align / channels).
        let mut wide = wav_bytes(1, 2, 48_000, 16, &[0; 8]);
        wide[34..36].copy_from_slice(&24_u16.to_le_bytes());
        assert!(reject(&wide).contains("24-bit"), "{}", reject(&wide));
    }

    #[test]
    fn unpadded_odd_chunks_are_tolerated() {
        // Some writers omit the pad byte after an odd-sized chunk.
        let plain = wav_bytes(1, 1, 48_000, 16, &[1, 0, 2, 0]);
        let mut file = plain[..36].to_vec();
        file.extend_from_slice(b"LIST");
        file.extend_from_slice(&3_u32.to_le_bytes());
        file.extend_from_slice(&[1, 2, 3]);
        file.extend_from_slice(&plain[36..]);
        let info = parse_wav(file.as_slice(), file.len() as u64).unwrap();
        assert_eq!((info.frames, info.data_offset), (2, file.len() as u64 - 4));
        // A correctly padded odd chunk still parses the same way.
        let mut padded = plain[..36].to_vec();
        padded.extend_from_slice(b"LIST");
        padded.extend_from_slice(&3_u32.to_le_bytes());
        padded.extend_from_slice(&[1, 2, 3, 0]);
        padded.extend_from_slice(&plain[36..]);
        let info = parse_wav(padded.as_slice(), padded.len() as u64).unwrap();
        assert_eq!(
            (info.frames, info.data_offset),
            (2, padded.len() as u64 - 4)
        );
    }

    #[test]
    fn non_finite_float_samples_do_not_silence_the_decimated_output() {
        let mut samples = vec![0.5_f32; 4000];
        samples[2000] = f32::INFINITY;
        samples[3000] = f32::NAN;
        let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let file = wav_bytes(3, 1, 96_000, 32, &data);
        let info = parse_wav(file.as_slice(), file.len() as u64).unwrap();
        let out = render_all(PcmPlan::new(info, 2, 24, 1, Mix::Keep), &file);
        let values: Vec<i32> = out[44..]
            .chunks_exact(3)
            .map(|b| i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8)
            .collect();
        // Away from the file edges every output stays near the 0.5 input level: the
        // infinity counts as full scale and the NaN as one zero sample.
        for (k, &v) in values.iter().enumerate().take(values.len() - 100).skip(100) {
            assert!((2_000_000..=7_000_000).contains(&v), "frame {k}: {v}");
        }
    }

    /// A source that records the offset of every positional read.
    struct Recording {
        bytes: Vec<u8>,
        reads: std::sync::Mutex<Vec<u64>>,
    }

    impl ReadAt for Recording {
        fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
            self.reads.lock().unwrap().push(offset);
            self.bytes.as_slice().read_at(buf, offset)
        }
    }

    #[test]
    fn read_ahead_is_transparent_for_reads_and_renders() {
        let samples: Vec<i32> = (0..50_011)
            .flat_map(|i| {
                let x = ((i * 53) % 977) * 8000 - 3_900_000;
                [x, x / 2]
            })
            .collect();
        let file = wav_bytes(1, 2, 96_000, 24, &i24_bytes(&samples));
        let shared: Arc<[u8]> = Arc::from(file.clone());
        // Small blocks so every read crosses block boundaries.
        let ahead = ReadAhead::with_block(shared, file.len() as u64, 4096);
        for (offset, size) in [(0, 1), (5, 4096), (4095, 2), (100_000, 70_000), (12, 1)] {
            let mut expected = vec![0_u8; size];
            let mut actual = vec![0_u8; size];
            file.as_slice()
                .read_exact_at(&mut expected, offset)
                .unwrap();
            ahead.read_exact_at(&mut actual, offset).unwrap();
            assert_eq!(actual, expected, "{offset}+{size}");
        }
        let mut past = [0_u8; 4];
        assert!(
            ahead
                .read_exact_at(&mut past, file.len() as u64 - 2)
                .is_err()
        );
        let info = parse_wav(file.as_slice(), file.len() as u64).unwrap();
        let plan = PcmPlan::new(info, 2, 16, 2, Mix::Keep);
        let full = render_all(plan.clone(), &file);
        let renderer = Renderer::new(plan);
        let total = full.len() as u64;
        for (first, last) in [
            (0, total - 1),
            (45, 46),
            (total / 3 + 1, total * 2 / 3),
            (total - 5, total - 1),
        ] {
            let mut part = Vec::new();
            renderer
                .render_range(&ahead, first, last, |chunk| {
                    part.extend_from_slice(&chunk);
                    true
                })
                .unwrap();
            assert_eq!(part, full[first as usize..=last as usize], "{first}-{last}");
        }
    }

    #[test]
    fn read_ahead_fetches_the_next_blocks_in_the_background() {
        let source = Arc::new(Recording {
            bytes: vec![7_u8; 10 * 4096],
            reads: std::sync::Mutex::new(Vec::new()),
        });
        let ahead = ReadAhead::with_block(source.clone(), 10 * 4096, 4096);
        let mut byte = [0_u8; 1];
        ahead.read_exact_at(&mut byte, 0).unwrap();
        // Blocks 1 and 2 are requested without being asked for.
        let started = std::time::Instant::now();
        loop {
            let reads = source.reads.lock().unwrap().clone();
            if reads.contains(&4096) && reads.contains(&8192) {
                assert!(!reads.contains(&12_288), "{reads:?}");
                break;
            }
            assert!(
                started.elapsed() < std::time::Duration::from_secs(5),
                "{reads:?}"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        // Reading block 1 now takes the prefetched data and fetches block 3.
        let before = source.reads.lock().unwrap().len();
        ahead.read_exact_at(&mut byte, 4096 + 10).unwrap();
        let started = std::time::Instant::now();
        while !source.reads.lock().unwrap().contains(&12_288) {
            assert!(started.elapsed() < std::time::Duration::from_secs(5));
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let reads = source.reads.lock().unwrap().clone();
        assert!(
            !reads[before..].contains(&4096),
            "block 1 read twice: {reads:?}"
        );
    }

    /// CPU throughput on a real file held in memory:
    /// `AUDIOLIB_PCM_BENCH=<wav> cargo test --release -p audiolib-server --lib
    /// render_throughput -- --ignored --nocapture`
    #[test]
    #[ignore = "benchmark; set AUDIOLIB_PCM_BENCH to a WAV path"]
    fn render_throughput() {
        let Ok(path) = std::env::var("AUDIOLIB_PCM_BENCH") else {
            return;
        };
        let file = std::fs::read(path).unwrap();
        let info = parse_wav(file.as_slice(), file.len() as u64).unwrap();
        let source_bytes = info.frames as f64 * info.block_align() as f64;
        for (bits, channels) in [(16, info.channels), (16, 1), (24, info.channels)] {
            let factor = decimation_factor(info.sample_rate, Some(48_000)).unwrap();
            let mix = if channels < info.channels {
                Mix::Mono
            } else {
                Mix::Keep
            };
            let renderer = Renderer::new(PcmPlan::new(info.clone(), factor, bits, channels, mix));
            let total = renderer.plan().total_len();
            let started = std::time::Instant::now();
            let mut sent = 0;
            renderer
                .render_range(file.as_slice(), 0, total - 1, |chunk| {
                    sent += chunk.len();
                    true
                })
                .unwrap();
            let secs = started.elapsed().as_secs_f64();
            eprintln!(
                "{:?} {} Hz x{} -> {} Hz/{bits}/{channels}: {:.1} MB source in {:.3} s = {:.0} MB/s per core, {:.0}x real time, {sent} bytes out",
                info.format,
                info.sample_rate,
                info.channels,
                renderer.plan().sample_rate,
                source_bytes / 1e6,
                secs,
                source_bytes / 1e6 / secs,
                info.duration() / secs
            );
        }
    }

    #[test]
    fn float_sources_clip_instead_of_wrapping() {
        let data: Vec<u8> = [2.0_f32, -2.0, f32::NAN, 0.25]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();
        let file = wav_bytes(3, 1, 48_000, 32, &data);
        let info = parse_wav(file.as_slice(), file.len() as u64).unwrap();
        let out = render_all(PcmPlan::new(info, 1, 24, 1, Mix::Keep), &file);
        let values: Vec<i32> = out[44..]
            .chunks_exact(3)
            .map(|b| i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8)
            .collect();
        assert_eq!(values, [8_388_607, -8_388_608, 0, 2_097_152]);
    }
}
