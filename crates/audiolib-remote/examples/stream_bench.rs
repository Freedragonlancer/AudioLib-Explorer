//! Audition streaming bench against a server (reads only: `GET /audio/{id}`, `/stream`,
//! `/preview`, `/waveform`). Run it through `tools/throttle_proxy.py` to emulate a link.
//!
//! ```text
//! cargo run --release -p audiolib-remote --example stream_bench -- \
//!     --server http://127.0.0.1:18790 --label 5M --play 698,4,16926 --arrow 1000-1019
//! ```
//!
//! It drives the same path as the desktop (route choice with the moving bandwidth
//! estimate, the 415/503 fallbacks, the in-memory stream) and plays each record with a
//! simulated engine: a 0.5 s ring fed in 2048-frame blocks only while the next block is in
//! memory, 0.5 s prebuffer at start and after a seek, pause when the ring runs dry and
//! resume once 2 s are buffered (the engine's rules). Arrowing follows the desktop: a
//! selection starts its stream after resting 180 ms, its waveform after the prebuffer.
//! Results are `RESULT` JSON lines. `--quick` measures only the first sound, `--seconds`
//! of play and one seek per record (no fresh-seek passes, no waveform).
use audiolib_core::AudioFile;
use audiolib_remote::{
    AudioRoute, MemoryCache, MemoryReader, MemoryStream, RemoteClient, RemoteError, RouteOptions,
    WaveformQuery, WaveformReply, bandwidth, choose_route,
};
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const PREBUFFER: f64 = 0.5;
const REBUFFER: f64 = 2.0;
/// The desktop starts a selection's network audio only after it rests this long.
const SETTLE: Duration = Duration::from_millis(180);
const RING: f64 = 0.5;
const BLOCK_FRAMES: u64 = 2048;
const TICK: Duration = Duration::from_millis(5);
/// `--transcode on|off` (default on): the server profile's "transcode auditions" choice.
static TRANSCODE: AtomicBool = AtomicBool::new(true);
/// `--quick`: first sound, play and one seek only (no fresh-seek passes, no waveform).
static QUICK: AtomicBool = AtomicBool::new(false);

fn route_options() -> RouteOptions {
    RouteOptions {
        transcode: TRANSCODE.load(Ordering::Relaxed),
        ..RouteOptions::default()
    }
}

struct Args {
    server: String,
    label: String,
    play: Vec<i64>,
    arrow: Vec<i64>,
    seconds: f64,
    arrow_ms: Vec<u64>,
}

fn args() -> Args {
    let mut args = Args {
        server: "http://127.0.0.1:18790".into(),
        label: "run".into(),
        play: Vec::new(),
        arrow: Vec::new(),
        seconds: 20.0,
        arrow_ms: vec![120, 300],
    };
    let mut items = std::env::args().skip(1);
    while let Some(flag) = items.next() {
        if flag == "--quick" {
            QUICK.store(true, Ordering::Relaxed);
            continue;
        }
        let value = items.next().unwrap_or_default();
        match flag.as_str() {
            "--server" => args.server = value,
            "--label" => args.label = value,
            "--play" => args.play = value.split(',').filter_map(|v| v.parse().ok()).collect(),
            "--arrow" => {
                let (a, b) = value.split_once('-').expect("--arrow first-last");
                args.arrow = (a.parse().unwrap()..=b.parse().unwrap()).collect();
            }
            "--seconds" => args.seconds = value.parse().unwrap(),
            "--arrow-ms" => {
                args.arrow_ms = value.split(',').filter_map(|v| v.parse().ok()).collect()
            }
            "--transcode" => TRANSCODE.store(value != "off", Ordering::Relaxed),
            other => panic!("unknown flag {other}"),
        }
    }
    args
}

fn epoch_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis()
}

fn get_audio(server: &str, id: i64) -> Option<AudioFile> {
    let body = ureq::get(&format!("{server}/api/v1/audio/{id}"))
        .call()
        .ok()?
        .body_mut()
        .read_to_string()
        .ok()?;
    serde_json::from_str(&body).ok()
}

/// Open the record's audition stream like the desktop: route, then the fallbacks.
fn open(
    client: &RemoteClient,
    file: &AudioFile,
) -> Result<(MemoryStream, AudioRoute), RemoteError> {
    let mut options = route_options();
    let mut route = choose_route(file, bandwidth().estimate_bps(), options);
    loop {
        let source = client.audio_source(file, &route);
        let key = format!("bench/{}/{}/{}", file.id, file.file_size, route.tag());
        let stream = MemoryStream::start(MemoryCache::global(), &key, source)
            .map_err(|error| RemoteError::Protocol(error.to_string()))?;
        match stream.wait_ready(&AtomicBool::new(false), Duration::from_secs(100)) {
            Ok(_) => return Ok((stream, route)),
            Err(RemoteError::PreviewUnavailable { code, .. }) if route.is_preview() => {
                println!("NOTE {} {} refused: {code}", file.id, route.tag());
                match route {
                    AudioRoute::Pcm(_) => options.pcm_unsupported = true,
                    _ => options.flac_unavailable = true,
                }
                let next = choose_route(file, bandwidth().estimate_bps(), options);
                route = if next == route {
                    AudioRoute::Stream
                } else {
                    next
                };
            }
            Err(error) => return Err(error),
        }
    }
}

struct Wav {
    rate: u32,
    channels: u16,
    align: u64,
    data_offset: u64,
    data_len: u64,
}

fn parse_wav(reader: &mut MemoryReader, total: u64) -> io::Result<Wav> {
    let mut riff = [0_u8; 12];
    reader.read_exact(&mut riff)?;
    let (mut rate, mut channels, mut align) = (0, 0, 0);
    loop {
        let mut head = [0_u8; 8];
        reader.read_exact(&mut head)?;
        let size = u64::from(u32::from_le_bytes(head[4..8].try_into().unwrap()));
        match &head[..4] {
            b"fmt " => {
                let mut fmt = vec![0_u8; size as usize];
                reader.read_exact(&mut fmt)?;
                channels = u16::from_le_bytes([fmt[2], fmt[3]]);
                rate = u32::from_le_bytes(fmt[4..8].try_into().unwrap());
                align = u64::from(u16::from_le_bytes([fmt[12], fmt[13]]));
            }
            b"data" => {
                let data_offset = reader.stream_position()?;
                let data_len = if size == 0 || size == 0xFFFF_FFFF {
                    total - data_offset
                } else {
                    size.min(total - data_offset)
                };
                return Ok(Wav {
                    rate,
                    channels,
                    align,
                    data_offset,
                    data_len,
                });
            }
            _ => {
                reader.seek(SeekFrom::Current((size + (size & 1)) as i64))?;
            }
        }
    }
}

/// The simulated engine for one record.
struct Player<'a> {
    stream: &'a MemoryStream,
    reader: MemoryReader,
    wav: Wav,
    /// Byte the output has played up to / the decoder has read up to.
    played: f64,
    decoded: u64,
    end: u64,
    waiting: Option<u64>,
    stalls: u32,
    stalled: Duration,
    stall_since: Option<Instant>,
    block: Vec<u8>,
}

impl<'a> Player<'a> {
    fn bytes_for(&self, seconds: f64) -> u64 {
        ((seconds * f64::from(self.wav.rate)).ceil() as u64).max(BLOCK_FRAMES) * self.wav.align
    }

    fn seek(&mut self, byte: u64) {
        let byte =
            self.wav.data_offset + (byte - self.wav.data_offset) / self.wav.align * self.wav.align;
        self.played = byte as f64;
        self.decoded = byte;
        self.reader.seek(SeekFrom::Start(byte)).unwrap();
        self.waiting = Some(self.bytes_for(PREBUFFER));
    }

    fn buffered(&self, len: u64) -> bool {
        let len = len.min(self.end - self.decoded);
        len == 0 || self.stream.is_buffered(self.decoded, len)
    }

    /// Decoder side: top the ring up while the next block is in memory. True if starved.
    fn fill(&mut self) -> bool {
        let block = BLOCK_FRAMES * self.wav.align;
        let ring = self.bytes_for(RING);
        while (self.decoded as f64) < self.played + ring as f64 / 2.0 && self.decoded < self.end {
            if !self.buffered(block) {
                return true;
            }
            let len = block.min(self.end - self.decoded) as usize;
            self.block.resize(len, 0);
            self.reader.read_exact(&mut self.block).unwrap();
            self.decoded += len as u64;
        }
        false
    }

    /// Wait for the prebuffer (after start or seek); returns how long it took.
    fn prebuffer(&mut self, deadline: Duration) -> Option<Duration> {
        let started = Instant::now();
        let need = self.bytes_for(PREBUFFER);
        while !self.buffered(need) {
            if started.elapsed() > deadline || self.stream.is_failed() {
                return None;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        self.waiting = None;
        self.fill();
        Some(started.elapsed())
    }

    /// Play `seconds` of audio in real time (or to the end).
    fn play(&mut self, seconds: f64) {
        let byte_rate = f64::from(self.wav.rate) * self.wav.align as f64;
        let low = (self.wav.rate as f64 / 16.0) * self.wav.align as f64;
        let target = (self.played + seconds * byte_rate).min(self.end as f64);
        let mut last = Instant::now();
        while self.played < target {
            std::thread::sleep(TICK);
            let now = Instant::now();
            let dt = now - last;
            last = now;
            if let Some(need) = self.waiting {
                if self.decoded >= self.end || self.buffered(need) {
                    self.waiting = None;
                    if let Some(since) = self.stall_since.take() {
                        self.stalled += since.elapsed();
                    }
                }
                continue;
            }
            self.played = (self.played + dt.as_secs_f64() * byte_rate).min(self.decoded as f64);
            let starved = self.fill();
            if starved && (self.decoded as f64 - self.played) < low && self.decoded < self.end {
                self.stalls += 1;
                self.stall_since = Some(Instant::now());
                self.waiting = Some(self.bytes_for(REBUFFER));
            }
        }
    }
}

/// Select the record afresh (nothing cached) and click to 70 % `listen` seconds after the
/// first sound (0: at once): the seek latency of data that has not arrived, then 4 s of
/// play.
fn fresh_seek(client: &RemoteClient, file: &AudioFile, listen: f64) -> (Option<Duration>, u32) {
    MemoryCache::global().clear();
    let Ok((stream, _)) = open(client, file) else {
        return (None, 0);
    };
    let total = stream.len().unwrap();
    let mut reader = stream.reader().unwrap();
    let wav = parse_wav(&mut reader, total).unwrap();
    let end = wav.data_offset + wav.data_len;
    let data_offset = wav.data_offset;
    let data_len = wav.data_len;
    let mut player = Player {
        stream: &stream,
        reader,
        wav,
        played: 0.0,
        decoded: 0,
        end,
        waiting: None,
        stalls: 0,
        stalled: Duration::ZERO,
        stall_since: None,
        block: Vec::new(),
    };
    player.seek(data_offset);
    if player.prebuffer(Duration::from_secs(60)).is_none() {
        return (None, 0);
    }
    if listen > 0.0 {
        player.play(listen);
    }
    player.seek(data_offset + (data_len as f64 * 0.7) as u64);
    let seek = player.prebuffer(Duration::from_secs(60));
    if seek.is_some() {
        player.play(4.0);
    }
    (seek, player.stalls)
}

fn play_record(client: &RemoteClient, args: &Args, file: &AudioFile) {
    let started = Instant::now();
    let started_epoch_ms = epoch_ms();
    let (stream, route) = match open(client, file) {
        Ok(opened) => opened,
        Err(error) => {
            println!(
                "RESULT {}",
                serde_json::json!({"label": args.label, "id": file.id, "error": error.to_string()})
            );
            return;
        }
    };
    let total = stream.len().unwrap();
    let mut reader = stream.reader().unwrap();
    let wav = parse_wav(&mut reader, total).unwrap();
    let end = wav.data_offset + wav.data_len;
    let mut player = Player {
        stream: &stream,
        reader,
        wav,
        played: 0.0,
        decoded: 0,
        end,
        waiting: None,
        stalls: 0,
        stalled: Duration::ZERO,
        stall_since: None,
        block: Vec::new(),
    };
    let data_offset = player.wav.data_offset;
    player.seek(data_offset);
    let first = player.prebuffer(Duration::from_secs(60));
    let ttfa = started.elapsed();
    if first.is_some() {
        player.play(args.seconds);
    }
    let (stalls_play, stalled_play) = (player.stalls, player.stalled);
    // Seek to 60 % of the record, then play 4 s.
    let seek_byte = data_offset + (player.wav.data_len as f64 * 0.6) as u64;
    player.seek(seek_byte);
    let seek = player.prebuffer(Duration::from_secs(60));
    if seek.is_some() {
        player.play(4.0);
    }
    let stalls_after_seek = player.stalls - stalls_play;
    let played_format = format!("{}/{}ch", player.wav.rate, player.wav.channels);
    let bitrate_mbps = f64::from(player.wav.rate) * player.wav.align as f64 * 8.0 / 1e6;
    if QUICK.load(Ordering::Relaxed) {
        let stalled_after_seek = player.stalled - stalled_play;
        println!(
            "RESULT {}",
            serde_json::json!({
                "label": args.label,
                "id": file.id,
                "source": format!("{}k/{}/{}ch", file.sample_rate / 1000, file.bit_depth, file.channels),
                "route": route.tag(),
                "played_format": played_format,
                "played_mbps": (bitrate_mbps * 100.0).round() / 100.0,
                "started_epoch_ms": started_epoch_ms,
                "ended_epoch_ms": epoch_ms(),
                "first_sound": first.is_some(),
                "ttfa_ms": ttfa.as_millis(),
                "stalls": stalls_play,
                "stalled_ms": stalled_play.as_millis(),
                "seek_ms": seek.map(|s| s.as_millis()),
                "stalls_after_seek": stalls_after_seek,
                "stalled_after_seek_ms": stalled_after_seek.as_millis(),
            })
        );
        return;
    }
    let (seek_late, stalls_late) = {
        drop(player);
        drop(stream);
        fresh_seek(client, file, 0.0)
    };
    // The same after listening for 1 s (a spare connection has had time to open).
    let (seek_1s, stalls_1s) = fresh_seek(client, file, 1.0);
    // Waveform: fresh, then revalidated (304).
    let query = WaveformQuery {
        buckets: 1600,
        colors: true,
        start_frame: None,
        end_frame: None,
    };
    let cancel = AtomicBool::new(false);
    let wave_started = Instant::now();
    let wave = client.waveform(file.id, &query, None, &cancel);
    let wave_time = wave_started.elapsed();
    let (wave_bytes, etag) = match &wave {
        Ok(WaveformReply::Data { json, etag }) => (json.len(), etag.clone()),
        _ => (0, None),
    };
    let revalidated_started = Instant::now();
    let revalidated = client.waveform(file.id, &query, etag.as_deref(), &cancel);
    let revalidate_time = revalidated_started.elapsed();
    println!(
        "RESULT {}",
        serde_json::json!({
            "label": args.label,
            "id": file.id,
            "source": format!("{}k/{}/{}ch", file.sample_rate / 1000, file.bit_depth, file.channels),
            "route": route.tag(),
            "played_format": played_format,
            "played_mbps": (bitrate_mbps * 100.0).round() / 100.0,
            "started_epoch_ms": started_epoch_ms,
            "ttfa_ms": ttfa.as_millis(),
            "stalls": stalls_play,
            "stalled_ms": stalled_play.as_millis(),
            "seek_ms": seek.map(|s| s.as_millis()),
            "stalls_after_seek": stalls_after_seek,
            "fresh_seek_ms": seek_late.map(|s| s.as_millis()),
            "stalls_after_fresh_seek": stalls_late,
            "seek_after_1s_ms": seek_1s.map(|s| s.as_millis()),
            "stalls_after_1s_seek": stalls_1s,
            "waveform_ms": wave_time.as_millis(),
            "waveform_bytes": wave_bytes,
            "waveform_error": wave.as_ref().err().map(|e| e.to_string()),
            "waveform_304": matches!(revalidated, Ok(WaveformReply::NotModified)),
            "waveform_304_ms": revalidate_time.as_millis(),
            "bandwidth_mbps": bandwidth().estimate_bps().map(|b| (b / 1e4).round() / 100.0),
            "memory_mib": MemoryCache::global().used_bytes() / (1 << 20),
        })
    );
}

/// The selected record's stream and its waveform cancel flag.
type Selected = (MemoryStream, Arc<AtomicBool>);

fn arrow(client: &RemoteClient, args: &Args, files: &[AudioFile], interval: u64) {
    println!("PHASE arrow-{interval}-start {}", epoch_ms());
    let started = Instant::now();
    let generation = Arc::new(AtomicU64::new(0));
    let current: Arc<std::sync::Mutex<Option<Selected>>> = Arc::new(std::sync::Mutex::new(None));
    let mut started_streams = 0;
    let mut threads = Vec::new();
    let mut last_key = Instant::now();
    for file in files {
        last_key = Instant::now();
        let mine = generation.fetch_add(1, Ordering::AcqRel) + 1;
        let (client, file, generation, current) = (
            client.clone(),
            file.clone(),
            generation.clone(),
            current.clone(),
        );
        threads.push(std::thread::spawn(move || {
            std::thread::sleep(SETTLE);
            if generation.load(Ordering::Acquire) != mine {
                return false;
            }
            let route = choose_route(&file, bandwidth().estimate_bps(), route_options());
            let source = client.audio_source(&file, &route);
            let key = format!("bench/{}/{}/{}", file.id, file.file_size, route.tag());
            let stream = MemoryStream::start(MemoryCache::global(), &key, source).unwrap();
            let cancel = Arc::new(AtomicBool::new(false));
            if let Some((old, old_cancel)) = current
                .lock()
                .unwrap()
                .replace((stream.clone(), cancel.clone()))
            {
                // Superseded: the desktop cancels the stream and the waveform job.
                old.cancel();
                old_cancel.store(true, Ordering::Release);
            }
            // The waveform follows the prebuffer (at most 3 s), unless superseded.
            let byte_rate = route.output_bytes_per_second(&file);
            let deadline = Instant::now() + Duration::from_secs(3);
            while !cancel.load(Ordering::Acquire) && Instant::now() < deadline {
                let need = 64 * 1024 + byte_rate * 6 / 10;
                if stream.is_complete()
                    || stream.is_failed()
                    || stream
                        .len()
                        .is_some_and(|total| stream.contiguous_from(0) >= need.min(total))
                {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            if !cancel.load(Ordering::Acquire) {
                let query = WaveformQuery {
                    buckets: 1600,
                    colors: true,
                    start_frame: None,
                    end_frame: None,
                };
                let _ = client.waveform(file.id, &query, None, &cancel);
            }
            true
        }));
        if threads.len() < files.len() {
            std::thread::sleep(Duration::from_millis(interval));
        }
    }
    // Measured from the last key press (the rest of its interval is idle).
    std::thread::sleep((SETTLE + Duration::from_millis(20)).saturating_sub(last_key.elapsed()));
    let stream = loop {
        if let Some((stream, _)) = current.lock().unwrap().as_ref()
            && stream
                .key()
                .contains(&format!("bench/{}/", files.last().unwrap().id))
        {
            break stream.clone();
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let ready = stream.wait_ready(&AtomicBool::new(false), Duration::from_secs(100));
    let mut reader = stream.reader().unwrap();
    let wav = ready
        .ok()
        .and_then(|total| parse_wav(&mut reader, total).ok());
    let prebuffered = wav.map(|wav| {
        let need = (PREBUFFER * f64::from(wav.rate)) as u64 * wav.align;
        while !stream.is_buffered(wav.data_offset, need.min(wav.data_len)) && !stream.is_failed() {
            std::thread::sleep(Duration::from_millis(2));
        }
        last_key.elapsed()
    });
    for thread in threads {
        if thread.join().unwrap_or(false) {
            started_streams += 1;
        }
    }
    println!("PHASE arrow-{interval}-end {}", epoch_ms());
    println!(
        "RESULT {}",
        serde_json::json!({
            "label": args.label,
            "arrow_interval_ms": interval,
            "arrow_records": files.len(),
            "streams_started": started_streams,
            "arrow_total_ms": started.elapsed().as_millis(),
            "last_ttfa_after_last_key_ms": prebuffered.map(|d| d.as_millis()),
            "last_failed": stream.is_failed(),
            "memory_mib": MemoryCache::global().used_bytes() / (1 << 20),
        })
    );
    if let Some((old, cancel)) = current.lock().unwrap().take() {
        old.cancel();
        cancel.store(true, Ordering::Release);
    }
}

fn main() {
    let args = args();
    let client = RemoteClient::new(&args.server, "").unwrap();
    // Like the desktop: a spare keep-alive connection after a cancel.
    audiolib_remote::set_connection_warming(true);
    let peak = Arc::new(AtomicU64::new(0));
    {
        let peak = peak.clone();
        std::thread::spawn(move || {
            loop {
                peak.fetch_max(MemoryCache::global().used_bytes(), Ordering::Relaxed);
                std::thread::sleep(Duration::from_millis(20));
            }
        });
    }
    println!("PHASE start {}", epoch_ms());
    let files: Vec<AudioFile> = args
        .play
        .iter()
        .filter_map(|id| get_audio(&args.server, *id))
        .collect();
    for file in &files {
        play_record(&client, &args, file);
    }
    if !args.arrow.is_empty() {
        let files: Vec<AudioFile> = args
            .arrow
            .iter()
            .filter_map(|id| get_audio(&args.server, *id))
            .collect();
        // Each interval arrows through its own records (nothing cached from a previous pass).
        let per = (files.len() / args.arrow_ms.len().max(1)).max(1);
        for (interval, files) in args.arrow_ms.clone().into_iter().zip(files.chunks(per)) {
            arrow(&client, &args, files, interval);
            std::thread::sleep(Duration::from_secs(2));
        }
    }
    std::thread::sleep(Duration::from_millis(500));
    println!(
        "RESULT {}",
        serde_json::json!({
            "label": args.label,
            "peak_memory_mib": peak.load(Ordering::Relaxed) / (1 << 20),
            "budget_mib": MemoryCache::global().budget() / (1 << 20),
        })
    );
    println!("PHASE end {}", epoch_ms());
}
