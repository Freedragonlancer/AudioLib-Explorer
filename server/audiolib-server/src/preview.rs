// server/audiolib-server/src/preview.rs
//! `GET /audio/{id}/preview` (doc/api/README.md §8.1): query parsing, the output format
//! rules shared by both codecs, and the ffmpeg FLAC transcode (`codec=flac`). The
//! in-process WAV path (`codec=pcm`) lives in [`crate::pcm`].
use crate::{ApiError, pcm::WavInfo};
use axum::body::Bytes;
use futures_core::Stream;
use std::{
    collections::HashSet,
    future::Future,
    io,
    path::Path,
    pin::Pin,
    process::Stdio,
    sync::{Arc, Mutex},
    task::{Context, Poll, ready},
    time::{Duration, Instant},
};
use tokio::{
    io::AsyncReadExt,
    process::{Child, ChildStdout, Command},
    sync::OwnedSemaphorePermit,
    task::JoinHandle,
};
use tokio_util::io::ReaderStream;

/// Lowest accepted `sr`.
pub const MIN_SAMPLE_RATE: u32 = 8_000;
/// Highest accepted `sr`.
pub const MAX_SAMPLE_RATE: u32 = 192_000;
/// Highest accepted `ch`.
pub const MAX_CHANNELS: u16 = crate::pcm::MAX_CHANNELS;
/// ffmpeg FLAC compression level. On production 96 kHz/24-bit material decimated to
/// 48 kHz/16-bit, level 5 is about 14% smaller than levels 0-2 at the same speed
/// (several hundred times real time); its 4096-sample frames keep latency low.
pub const FLAC_COMPRESSION_LEVEL: u8 = 5;
/// Bumped whenever the ffmpeg arguments (and so the FLAC output for the same source and
/// parameters) change; part of the FLAC preview `ETag`.
pub const FLAC_ALGORITHM_VERSION: u32 = 1;
/// How long a `codec=flac` request waits for a transcode slot before 503 `BUSY`.
pub const TRANSCODE_SLOT_WAIT: Duration = Duration::from_millis(500);
/// ffmpeg stderr kept for the log (the rest is read and discarded).
const STDERR_LIMIT: usize = 16 * 1024;
/// Largest body chunk read from ffmpeg's stdout.
const STDOUT_CHUNK: usize = 64 * 1024;

/// Preview encoding chosen with `codec=`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    /// In-process WAV (PCM) with integer decimation; sized, rangeable.
    Pcm,
    /// Live ffmpeg FLAC transcode; chunked, seek with `t=`.
    Flac,
}

/// How source channels become output channels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mix {
    /// Channels as in the source.
    Keep,
    /// Average of all channels.
    Mono,
    /// Left = average of even-numbered channels (0, 2, ...), right = average of the odd
    /// ones (1, 3, ...); only for sources with more than two channels.
    Pairs,
}

/// Parsed and validated query of `/audio/{id}/preview` (source-independent checks only).
#[derive(Clone, Debug, PartialEq)]
pub struct PreviewRequest {
    /// `codec=pcm|flac` (required).
    pub codec: Codec,
    /// `sr=` (8000..=192000).
    pub sample_rate: Option<u32>,
    /// `bits=16|24`.
    pub bits: Option<u16>,
    /// `ch=` (1..=64; resolved against the source by [`resolve_channels`]).
    pub channels: Option<u16>,
    /// `t=` start seconds (flac only).
    pub start: Option<f64>,
}

impl PreviewRequest {
    /// Validate raw query values; any problem is 400 `VALIDATION`.
    pub fn parse(
        codec: Option<&str>,
        sample_rate: Option<&str>,
        bits: Option<&str>,
        channels: Option<&str>,
        start: Option<&str>,
    ) -> Result<Self, ApiError> {
        let codec = match codec.map(str::to_ascii_lowercase).as_deref() {
            Some("pcm") => Codec::Pcm,
            Some("flac") => Codec::Flac,
            Some(other) => {
                return Err(ApiError::validation(format!(
                    "codec must be pcm or flac, got {other:?}"
                )));
            }
            None => return Err(ApiError::validation("codec is required: pcm or flac")),
        };
        let sample_rate = sample_rate
            .map(|value| {
                let rate = integer("sr", value)?;
                if !(u64::from(MIN_SAMPLE_RATE)..=u64::from(MAX_SAMPLE_RATE)).contains(&rate) {
                    return Err(ApiError::validation(format!(
                        "sr must be {MIN_SAMPLE_RATE}..={MAX_SAMPLE_RATE}, got {value}"
                    )));
                }
                Ok(rate as u32)
            })
            .transpose()?;
        let bits = bits
            .map(|value| match value {
                "16" => Ok(16),
                "24" => Ok(24),
                _ => Err(ApiError::validation(format!(
                    "bits must be 16 or 24, got {value:?}"
                ))),
            })
            .transpose()?;
        let channels = channels
            .map(|value| {
                let channels = integer("ch", value)?;
                if !(1..=u64::from(MAX_CHANNELS)).contains(&channels) {
                    return Err(ApiError::validation(format!(
                        "ch must be 1..={MAX_CHANNELS}, got {value}"
                    )));
                }
                Ok(channels as u16)
            })
            .transpose()?;
        let start = start.map(parse_seconds).transpose()?;
        if codec == Codec::Pcm && start.is_some() {
            return Err(ApiError::validation(
                "t is only supported with codec=flac; seek a pcm preview with a byte Range",
            ));
        }
        Ok(Self {
            codec,
            sample_rate,
            bits,
            channels,
            start,
        })
    }
}

/// Decimal digits only (at most 9), so `+1`, `1e3` or `0x10` are rejected.
fn integer(name: &str, value: &str) -> Result<u64, ApiError> {
    if value.is_empty() || value.len() > 9 || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ApiError::validation(format!(
            "{name} must be a decimal integer, got {value:?}"
        )));
    }
    Ok(value.parse().unwrap_or_default())
}

/// `t=`: non-negative decimal seconds (`12`, `12.5`, `.5`), no sign or exponent.
fn parse_seconds(value: &str) -> Result<f64, ApiError> {
    let invalid = || {
        ApiError::validation(format!(
            "t must be non-negative decimal seconds, got {value:?}"
        ))
    };
    let well_formed = !value.is_empty()
        && value.len() <= 32
        && value.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        && value.bytes().filter(|&b| b == b'.').count() <= 1
        && value.bytes().any(|b| b.is_ascii_digit());
    if !well_formed {
        return Err(invalid());
    }
    value
        .parse::<f64>()
        .ok()
        .filter(|seconds| seconds.is_finite())
        .ok_or_else(invalid)
}

/// Output bit depth: the request (16 or 24), else 16 for integer sources of at most 16
/// bits and 24 for everything else. Never more than an integer source has: 24 from a
/// 16-bit source gives 16 (float sources may get 24).
pub fn resolve_bits(requested: Option<u16>, source_bits: u16, float: bool) -> u16 {
    let narrow = !float && source_bits > 0 && source_bits <= 16;
    match requested {
        Some(_) if narrow => 16,
        Some(bits) => bits,
        None if narrow => 16,
        None => 24,
    }
}

/// Output channels: absent or at least the source count keeps the source channels (no
/// upmix), `1` is the average of all channels, `2` folds a source with more than two
/// channels into stereo ([`Mix::Pairs`]); anything else is 400 `VALIDATION`.
pub fn resolve_channels(requested: Option<u16>, source: u16) -> Result<(u16, Mix), ApiError> {
    match requested {
        None => Ok((source, Mix::Keep)),
        Some(channels) if channels >= source => Ok((source, Mix::Keep)),
        Some(1) => Ok((1, Mix::Mono)),
        Some(2) => Ok((2, Mix::Pairs)),
        Some(channels) => Err(ApiError::validation(format!(
            "ch must be 1, 2 or the source channel count ({source}), got {channels}"
        ))),
    }
}

/// Source layout used to plan a FLAC preview: from the WAV header when the file is a
/// supported WAV, otherwise from the library record.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceFormat {
    /// Frames per second (0 = unknown).
    pub sample_rate: u32,
    /// Bit depth (0 = unknown).
    pub bits: u16,
    /// IEEE float samples.
    pub float: bool,
    /// Channels (0 = unknown).
    pub channels: u16,
    /// Exact frame count when known.
    pub frames: Option<u64>,
    /// Duration in seconds (0 = unknown).
    pub duration: f64,
}

impl SourceFormat {
    /// From a parsed WAV header.
    pub fn from_wav(info: &WavInfo) -> Self {
        Self {
            sample_rate: info.sample_rate,
            bits: info.format.bits(),
            float: info.format.is_float(),
            channels: info.channels,
            frames: Some(info.frames),
            duration: info.duration(),
        }
    }
    /// From library metadata.
    pub fn from_record(audio: &audiolib_core::AudioFile) -> Self {
        Self {
            sample_rate: audio.sample_rate,
            bits: audio.bit_depth,
            float: false,
            channels: audio.channels,
            frames: None,
            duration: audio.duration,
        }
    }
}

/// Resolved output of one FLAC preview.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlacPlan {
    /// Source rate.
    pub source_rate: u32,
    /// Source channels.
    pub source_channels: u16,
    /// Output rate: `min(sr, source rate)` (ffmpeg resamples to any rate).
    pub sample_rate: u32,
    /// Output bits (16 or 24).
    pub bits: u16,
    /// Output channels.
    pub channels: u16,
    /// Channel folding.
    pub mix: Mix,
    /// Start position in source frames (`round(t * source rate)`).
    pub start_source_frame: u64,
    /// Start position in output frames (`X-Start-Frame`).
    pub start_frame: u64,
    /// Source length in frames (exact for WAV, else from the library duration; 0 =
    /// unknown).
    pub source_frames: u64,
}

/// `frames` at rate `from` expressed at rate `to`, rounded to the nearest frame.
fn rescale(frames: u64, to: u32, from: u32) -> u64 {
    ((u128::from(frames) * u128::from(to) * 2 + u128::from(from)) / (2 * u128::from(from))) as u64
}

impl FlacPlan {
    /// Plan the transcode. Unknown source rate or channels is 415
    /// `PREVIEW_UNSUPPORTED_FORMAT`; `t` at or beyond the end is 416.
    pub fn new(source: &SourceFormat, request: &PreviewRequest) -> Result<Self, ApiError> {
        if source.sample_rate == 0 || source.channels == 0 {
            return Err(ApiError::preview_unsupported(
                "The source format of this file is unknown; play it with /stream",
            ));
        }
        let source_rate = source.sample_rate;
        let sample_rate = request
            .sample_rate
            .map_or(source_rate, |rate| rate.min(source_rate));
        let bits = resolve_bits(request.bits, source.bits, source.float);
        let (channels, mix) = resolve_channels(request.channels, source.channels)?;
        let start = request.start.unwrap_or(0.0);
        let start_source_frame = (start * f64::from(source_rate)).round() as u64;
        let total = source
            .frames
            .unwrap_or_else(|| (source.duration.max(0.0) * f64::from(source_rate)).round() as u64);
        if start > 0.0 && total > 0 && start_source_frame >= total {
            return Err(ApiError::range_not_satisfiable(format!(
                "t={start} is at or beyond the end of the audio ({:.6} s)",
                total as f64 / f64::from(source_rate)
            )));
        }
        let start_frame = rescale(start_source_frame, sample_rate, source_rate);
        Ok(Self {
            source_rate,
            source_channels: source.channels,
            sample_rate,
            bits,
            channels,
            mix,
            start_source_frame,
            start_frame,
            source_frames: total,
        })
    }

    /// `X-Total-Frames`: output frames from the start to the end, at the output rate
    /// (`round(source_frames x sr / source rate) - X-Start-Frame`); `None` when the
    /// source length is unknown.
    pub fn total_frames(&self) -> Option<u64> {
        (self.source_frames > 0).then(|| {
            rescale(self.source_frames, self.sample_rate, self.source_rate)
                .saturating_sub(self.start_frame)
        })
    }

    /// Weak `ETag` (ffmpeg output is not promised to be byte-stable, and FLAC previews
    /// have no Range): the source's stream ETag, [`FLAC_ALGORITHM_VERSION`], the ffmpeg
    /// binary ([`ffmpeg_tag`]) and every normalized parameter, start frame included.
    pub fn etag(&self, source_etag: &str, ffmpeg_tag: &str) -> String {
        let inner = source_etag.trim_start_matches("W/").trim_matches('"');
        format!(
            "W/\"{inner}-flac{FLAC_ALGORITHM_VERSION}-{ffmpeg_tag}-{}-{}-{}-{}\"",
            self.sample_rate, self.bits, self.channels, self.start_source_frame
        )
    }

    /// `X-Start-Time` and ffmpeg `-ss`: the start source frame in seconds, rounded to
    /// microseconds (ffmpeg's time unit; it lands on exactly that frame).
    pub fn start_time(&self) -> String {
        format!(
            "{:.6}",
            self.start_source_frame as f64 / f64::from(self.source_rate)
        )
    }

    /// ffmpeg arguments reading `input`, writing FLAC to stdout.
    pub fn ffmpeg_args(&self, input: &str) -> Vec<String> {
        let mut args: Vec<String> = ["-nostdin", "-hide_banner", "-loglevel", "error"]
            .map(String::from)
            .to_vec();
        if self.start_source_frame > 0 {
            // Input seeking: a direct jump on PCM WAV, then sample-accurate trimming.
            args.extend(["-ss".into(), self.start_time()]);
        }
        args.extend(
            [
                "-i",
                input,
                "-map",
                "0:a:0",
                "-vn",
                "-sn",
                "-dn",
                "-map_metadata",
                "-1",
                "-map_chapters",
                "-1",
            ]
            .map(String::from),
        );
        let channel_sum = |range: &mut dyn Iterator<Item = u16>| {
            range.map(|c| format!("c{c}")).collect::<Vec<_>>().join("+")
        };
        let mut filters = Vec::new();
        match self.mix {
            Mix::Keep => {}
            Mix::Mono => filters.push(format!(
                "pan=mono|c0<{}",
                channel_sum(&mut (0..self.source_channels))
            )),
            Mix::Pairs => filters.push(format!(
                "pan=stereo|c0<{}|c1<{}",
                channel_sum(&mut (0..self.source_channels).step_by(2)),
                channel_sum(&mut (1..self.source_channels).step_by(2))
            )),
        }
        let sample_format = if self.bits == 16 { "s16" } else { "s32" };
        filters.push(format!(
            "aresample=osr={}:osf={sample_format}:dither_method=triangular",
            self.sample_rate
        ));
        args.extend(["-af".into(), filters.join(",")]);
        args.extend(
            [
                "-c:a",
                "flac",
                "-compression_level",
                &FLAC_COMPRESSION_LEVEL.to_string(),
                "-sample_fmt",
                sample_format,
            ]
            .map(String::from),
        );
        if self.bits == 24 {
            args.extend(["-bits_per_raw_sample".into(), "24".into()]);
        }
        // Flush every packet: a few KiB at a time instead of a 32 KiB buffer.
        args.extend(["-flush_packets", "1", "-f", "flac", "pipe:1"].map(String::from));
        args
    }
}

/// Whether `path` is an executable regular file (missing ffmpeg is 503
/// `TRANSCODER_UNAVAILABLE`, never a startup failure).
pub fn ffmpeg_available(path: &Path) -> bool {
    match std::fs::metadata(path) {
        #[cfg(unix)]
        Ok(metadata) => {
            use std::os::unix::fs::PermissionsExt;
            metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
        }
        #[cfg(not(unix))]
        Ok(metadata) => metadata.is_file(),
        Err(_) => false,
    }
}

/// Identity of the ffmpeg executable for the FLAC `ETag`: size and modification time of
/// the binary `path` resolves to (a Homebrew upgrade changes it).
pub fn ffmpeg_tag(path: &Path) -> String {
    match std::fs::metadata(path) {
        Ok(metadata) => {
            let modified = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |since| since.as_secs());
            format!("{:x}.{modified:x}", metadata.len())
        }
        Err(_) => "0".into(),
    }
}

/// Process ids of running ffmpeg previews (diagnostics and tests).
#[derive(Clone, Debug, Default)]
pub struct TranscodeRegistry(Arc<Mutex<HashSet<u32>>>);

impl TranscodeRegistry {
    fn insert(&self, pid: u32) {
        if let Ok(mut pids) = self.0.lock() {
            pids.insert(pid);
        }
    }
    fn remove(&self, pid: u32) {
        if let Ok(mut pids) = self.0.lock() {
            pids.remove(&pid);
        }
    }
    /// Running (not yet reaped) ffmpeg processes.
    pub fn pids(&self) -> Vec<u32> {
        self.0
            .lock()
            .map(|pids| pids.iter().copied().collect())
            .unwrap_or_default()
    }
}

/// One running ffmpeg. Dropping it kills the process (`kill_on_drop`), stops the stderr
/// reader and releases the preview and transcode slots.
struct Transcode {
    child: Child,
    pid: u32,
    stderr: JoinHandle<Vec<u8>>,
    registry: TranscodeRegistry,
    _permits: Vec<OwnedSemaphorePermit>,
    id: i64,
    started: Instant,
}

impl Drop for Transcode {
    fn drop(&mut self) {
        self.registry.remove(self.pid);
        self.stderr.abort();
    }
}

impl Transcode {
    /// stdout ended: reap ffmpeg; a failure exit becomes an error so the response is
    /// aborted instead of ending like a complete stream.
    async fn finish(&mut self, sent: u64) -> io::Result<()> {
        let status = self.child.wait().await?;
        let stderr = (&mut self.stderr).await.unwrap_or_default();
        let millis = self.started.elapsed().as_millis() as u64;
        if status.success() {
            tracing::info!(id = self.id, bytes = sent, millis, "FLAC preview finished");
            return Ok(());
        }
        let text = stderr_summary(&stderr);
        tracing::error!(id = self.id, %status, bytes = sent, stderr = %text, "ffmpeg preview failed");
        Err(io::Error::other(format!("ffmpeg {status}: {text}")))
    }

    /// The client went away (or reading failed): kill ffmpeg now and reap it in the
    /// background; the slot is released once the process is gone.
    fn abort(mut self, sent: u64) {
        let _ = self.child.start_kill();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let _ = self.child.wait().await;
                tracing::info!(
                    id = self.id,
                    bytes = sent,
                    millis = self.started.elapsed().as_millis() as u64,
                    "FLAC preview stopped early (client disconnected); ffmpeg killed"
                );
                drop(self);
            });
        }
    }
}

fn stderr_summary(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let text = text.trim();
    if text.is_empty() {
        "(no error output)".into()
    } else {
        text.lines().take(20).collect::<Vec<_>>().join(" | ")
    }
}

/// Read a child's stderr to the end, keeping the first [`STDERR_LIMIT`] bytes, so the
/// pipe never fills up and blocks ffmpeg.
async fn read_bounded(mut stderr: tokio::process::ChildStderr) -> Vec<u8> {
    let mut kept = Vec::new();
    let mut buf = [0_u8; 4096];
    loop {
        match stderr.read(&mut buf).await {
            Ok(0) | Err(_) => return kept,
            Ok(n) => {
                let room = STDERR_LIMIT - kept.len();
                kept.extend_from_slice(&buf[..n.min(room)]);
            }
        }
    }
}

/// Response body of a FLAC preview: ffmpeg's stdout. Dropping it (client disconnect)
/// kills ffmpeg immediately; a non-zero exit at the end aborts the response.
pub struct FlacStream {
    stdout: ReaderStream<ChildStdout>,
    pending: Option<Bytes>,
    process: Option<Transcode>,
    ending: Option<Pin<Box<dyn Future<Output = io::Result<()>> + Send>>>,
    sent: u64,
}

impl FlacStream {
    /// Spawn `ffmpeg args` with `stdin` (the opened source on Unix); `permits` (preview
    /// and transcode slots) are held until ffmpeg is gone. Missing or non-executable
    /// ffmpeg is 503 `TRANSCODER_UNAVAILABLE`.
    pub fn spawn(
        ffmpeg: &Path,
        args: &[String],
        stdin: Stdio,
        permits: Vec<OwnedSemaphorePermit>,
        registry: &TranscodeRegistry,
        id: i64,
    ) -> Result<Self, ApiError> {
        let mut child = Command::new(ffmpeg)
            .args(args)
            .stdin(stdin)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| match error.kind() {
                io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied => {
                    tracing::warn!(path = %ffmpeg.display(), %error, "Cannot start ffmpeg");
                    ApiError::transcoder_unavailable(format!(
                        "ffmpeg cannot be started on the server ({error}); use codec=pcm or /stream"
                    ))
                }
                _ => ApiError::internal(format!("Cannot start ffmpeg: {error}")),
            })?;
        let pid = child.id().unwrap_or_default();
        let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
            return Err(ApiError::internal("ffmpeg pipes are missing"));
        };
        registry.insert(pid);
        tracing::debug!(id, pid, "Started ffmpeg preview");
        Ok(Self {
            stdout: ReaderStream::with_capacity(stdout, STDOUT_CHUNK),
            pending: None,
            process: Some(Transcode {
                child,
                pid,
                stderr: tokio::spawn(read_bounded(stderr)),
                registry: registry.clone(),
                _permits: permits,
                id,
                started: Instant::now(),
            }),
            ending: None,
            sent: 0,
        })
    }

    /// Wait for the first chunk (headers are sent only once ffmpeg produced output, so
    /// an early ffmpeg failure is a JSON error instead of a broken 200). Errors and an
    /// empty output are 500 `AUDIO`.
    pub async fn first_chunk(&mut self) -> Result<(), ApiError> {
        let failed = |message: String| {
            ApiError::new(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "AUDIO",
                message,
            )
        };
        match std::future::poll_fn(|cx| Pin::new(&mut *self).poll_next(cx)).await {
            Some(Ok(bytes)) => {
                self.pending = Some(bytes);
                Ok(())
            }
            Some(Err(error)) => Err(failed(format!("Cannot transcode the audio: {error}"))),
            None => Err(failed("ffmpeg produced no output".into())),
        }
    }
}

impl Stream for FlacStream {
    type Item = io::Result<Bytes>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if let Some(bytes) = this.pending.take() {
            return Poll::Ready(Some(Ok(bytes)));
        }
        loop {
            if let Some(ending) = this.ending.as_mut() {
                let result = ready!(ending.as_mut().poll(cx));
                this.ending = None;
                return Poll::Ready(result.err().map(Err));
            }
            if this.process.is_none() {
                return Poll::Ready(None);
            }
            match ready!(Pin::new(&mut this.stdout).poll_next(cx)) {
                Some(Ok(bytes)) => {
                    this.sent += bytes.len() as u64;
                    return Poll::Ready(Some(Ok(bytes)));
                }
                Some(Err(error)) => {
                    if let Some(process) = this.process.take() {
                        process.abort(this.sent);
                    }
                    return Poll::Ready(Some(Err(error)));
                }
                None => {
                    if let Some(mut process) = this.process.take() {
                        let sent = this.sent;
                        this.ending = Some(Box::pin(async move { process.finish(sent).await }));
                    }
                }
            }
        }
    }
}

impl Drop for FlacStream {
    fn drop(&mut self) {
        if let Some(process) = self.process.take() {
            process.abort(self.sent);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pcm::SampleFormat;

    fn parse(query: [Option<&str>; 5]) -> Result<PreviewRequest, ApiError> {
        PreviewRequest::parse(query[0], query[1], query[2], query[3], query[4])
    }

    fn wav_source(rate: u32, format: SampleFormat, channels: u16, frames: u64) -> SourceFormat {
        SourceFormat::from_wav(&WavInfo {
            format,
            sample_rate: rate,
            channels,
            data_offset: 44,
            frames,
        })
    }

    #[test]
    fn query_values_are_validated() {
        let ok = parse([
            Some("FLAC"),
            Some("48000"),
            Some("16"),
            Some("1"),
            Some("12.5"),
        ])
        .unwrap();
        assert_eq!(
            ok,
            PreviewRequest {
                codec: Codec::Flac,
                sample_rate: Some(48_000),
                bits: Some(16),
                channels: Some(1),
                start: Some(12.5),
            }
        );
        let pcm = parse([Some("pcm"), None, None, None, None]).unwrap();
        assert_eq!(pcm.codec, Codec::Pcm);
        assert_eq!(
            parse([Some("flac"), None, None, None, Some(".5")])
                .unwrap()
                .start,
            Some(0.5)
        );
        assert_eq!(
            parse([Some("flac"), None, None, None, Some("3")])
                .unwrap()
                .start,
            Some(3.0)
        );
        for bad in [
            [None, None, None, None, None],
            [Some("opus"), None, None, None, None],
            [Some("pcm"), Some("7999"), None, None, None],
            [Some("pcm"), Some("192001"), None, None, None],
            [Some("pcm"), Some("48k"), None, None, None],
            [Some("pcm"), Some("+48000"), None, None, None],
            [Some("pcm"), None, Some("32"), None, None],
            [Some("pcm"), None, Some("8"), None, None],
            [Some("pcm"), None, None, Some("0"), None],
            [Some("pcm"), None, None, Some("65"), None],
            [Some("pcm"), None, None, None, Some("1")],
            [Some("flac"), None, None, None, Some("-1")],
            [Some("flac"), None, None, None, Some("1e3")],
            [Some("flac"), None, None, None, Some("inf")],
            [Some("flac"), None, None, None, Some("NaN")],
            [Some("flac"), None, None, None, Some("1.2.3")],
            [Some("flac"), None, None, None, Some(".")],
            [Some("flac"), None, None, None, Some("")],
        ] {
            let error = parse(bad).unwrap_err();
            assert_eq!(
                (error.status().as_u16(), error.code()),
                (400, "VALIDATION"),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn bits_default_to_the_source_and_never_grow() {
        assert_eq!(resolve_bits(None, 24, false), 24);
        assert_eq!(resolve_bits(None, 32, false), 24);
        assert_eq!(resolve_bits(None, 32, true), 24);
        assert_eq!(resolve_bits(None, 16, false), 16);
        assert_eq!(resolve_bits(None, 0, false), 24);
        assert_eq!(resolve_bits(Some(16), 24, false), 16);
        assert_eq!(resolve_bits(Some(24), 16, false), 16);
        assert_eq!(resolve_bits(Some(24), 32, true), 24);
        assert_eq!(resolve_bits(Some(24), 32, false), 24);
    }

    #[test]
    fn channels_fold_but_never_upmix() {
        assert_eq!(resolve_channels(None, 2).unwrap(), (2, Mix::Keep));
        assert_eq!(resolve_channels(Some(1), 2).unwrap(), (1, Mix::Mono));
        assert_eq!(resolve_channels(Some(2), 2).unwrap(), (2, Mix::Keep));
        assert_eq!(resolve_channels(Some(2), 1).unwrap(), (1, Mix::Keep));
        assert_eq!(resolve_channels(Some(1), 1).unwrap(), (1, Mix::Keep));
        assert_eq!(resolve_channels(Some(2), 6).unwrap(), (2, Mix::Pairs));
        assert_eq!(resolve_channels(Some(6), 6).unwrap(), (6, Mix::Keep));
        assert_eq!(resolve_channels(Some(8), 6).unwrap(), (6, Mix::Keep));
        let error = resolve_channels(Some(4), 6).unwrap_err();
        assert_eq!(error.code(), "VALIDATION");
    }

    #[test]
    fn flac_plans_clamp_the_rate_and_report_the_start() {
        let source = wav_source(96_000, SampleFormat::I24, 2, 96_000 * 60);
        let request = |sr, t| PreviewRequest {
            codec: Codec::Flac,
            sample_rate: sr,
            bits: Some(16),
            channels: None,
            start: t,
        };
        let plan = FlacPlan::new(&source, &request(Some(48_000), Some(12.345_678))).unwrap();
        assert_eq!(
            (plan.sample_rate, plan.bits, plan.channels),
            (48_000, 16, 2)
        );
        assert_eq!(plan.start_source_frame, 1_185_185); // round(12.345678 * 96000)
        assert_eq!(plan.start_frame, 592_593); // round(1185185 / 2)
        assert_eq!(plan.start_time(), "12.345677");
        // 60 s at 48 kHz from frame 592593 on.
        assert_eq!(plan.total_frames(), Some(2_880_000 - 592_593));
        let etag = plan.etag("\"abc-1\"", "ff.1");
        assert_eq!(etag, "W/\"abc-1-flac1-ff.1-48000-16-2-1185185\"");
        assert_ne!(etag, plan.etag("\"abc-1\"", "ff.2"));
        // Never upsampled: 192000 requested from 96000 stays 96000.
        let plan = FlacPlan::new(&source, &request(Some(192_000), None)).unwrap();
        assert_eq!((plan.sample_rate, plan.start_frame), (96_000, 0));
        assert_eq!(plan.start_time(), "0.000000");
        // 44.1 kHz is allowed (ffmpeg resamples to any rate).
        let plan = FlacPlan::new(&source, &request(Some(44_100), Some(1.0))).unwrap();
        assert_eq!((plan.sample_rate, plan.start_frame), (44_100, 44_100));
        // At or beyond the end: 416.
        for t in [60.0, 61.5] {
            let error = FlacPlan::new(&source, &request(None, Some(t))).unwrap_err();
            assert_eq!(
                (error.status().as_u16(), error.code()),
                (416, "RANGE_NOT_SATISFIABLE")
            );
        }
        assert!(FlacPlan::new(&source, &request(None, Some(59.99))).is_ok());
        // Length from the library duration when there is no WAV header; unknown -> None.
        let record = SourceFormat {
            frames: None,
            duration: 2.5,
            ..source.clone()
        };
        let plan = FlacPlan::new(&record, &request(Some(44_100), Some(1.0))).unwrap();
        assert_eq!(plan.total_frames(), Some(66_150)); // 2.5 s - 1 s at 44.1 kHz
        let unknown_length = SourceFormat {
            duration: 0.0,
            ..record
        };
        let plan = FlacPlan::new(&unknown_length, &request(None, None)).unwrap();
        assert_eq!(plan.total_frames(), None);
        let unknown = SourceFormat {
            sample_rate: 0,
            ..source.clone()
        };
        assert_eq!(
            FlacPlan::new(&unknown, &request(None, None))
                .unwrap_err()
                .code(),
            "PREVIEW_UNSUPPORTED_FORMAT"
        );
    }

    #[test]
    fn ffmpeg_arguments_seek_on_input_and_stream_flac() {
        let source = wav_source(96_000, SampleFormat::I24, 6, 96_000 * 60);
        let request = PreviewRequest {
            codec: Codec::Flac,
            sample_rate: Some(48_000),
            bits: Some(16),
            channels: Some(2),
            start: Some(2.5),
        };
        let args = FlacPlan::new(&source, &request)
            .unwrap()
            .ffmpeg_args("/dev/fd/0");
        let text = args.join(" ");
        assert!(
            text.starts_with("-nostdin -hide_banner -loglevel error -ss 2.500000 -i /dev/fd/0 "),
            "{text}"
        );
        assert!(
            text.contains("-map 0:a:0 -vn -sn -dn -map_metadata -1 -map_chapters -1"),
            "{text}"
        );
        assert!(text.contains(
            "-af pan=stereo|c0<c0+c2+c4|c1<c1+c3+c5,aresample=osr=48000:osf=s16:dither_method=triangular"
        ), "{text}");
        assert!(
            text.contains("-c:a flac -compression_level 5 -sample_fmt s16"),
            "{text}"
        );
        assert!(!text.contains("bits_per_raw_sample"));
        assert!(text.ends_with("-flush_packets 1 -f flac pipe:1"), "{text}");
        let mono = PreviewRequest {
            bits: None,
            channels: Some(1),
            start: None,
            ..request
        };
        let text = FlacPlan::new(&source, &mono)
            .unwrap()
            .ffmpeg_args("x")
            .join(" ");
        assert!(!text.contains("-ss"));
        assert!(
            text.contains("-af pan=mono|c0<c0+c1+c2+c3+c4+c5,aresample=osr=48000:osf=s32:"),
            "{text}"
        );
        assert!(
            text.contains("-sample_fmt s32 -bits_per_raw_sample 24"),
            "{text}"
        );
    }
}
