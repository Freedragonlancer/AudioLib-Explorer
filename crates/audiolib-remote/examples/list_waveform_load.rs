// crates/audiolib-remote/examples/list_waveform_load.rs
//! Server-mode list Waveform load for the zero-latency acceptance: pages through ids with
//! `POST /waveforms` (200 ids, one request in flight, `retry_after` respected, like the
//! desktop worker) for a fixed time. Read-only: it never calls PATCH, scans or removals.
//! Run it next to `first-sound-loopback` (audio only, local tone) while the server's
//! pre-generation is running:
//!
//! ```text
//! $env:ALX_SERVER_URL = '<server url>'      # never committed anywhere
//! Start-Process cargo -ArgumentList 'run --release -p audiolib-remote --example list_waveform_load -- --seconds 120'
//! cargo run --release -p audiolib-audio --example first-sound-loopback -- --runs 24 --output OUT
//! ```
use audiolib_remote::RemoteClient;
use std::{
    collections::BTreeMap,
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let value = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
    };
    let seconds: u64 = value("--seconds")
        .and_then(|v| v.parse().ok())
        .unwrap_or(120);
    let start: i64 = value("--start-id")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let url = std::env::var("ALX_SERVER_URL").expect("set ALX_SERVER_URL to the server address");
    let token = std::env::var("ALX_SERVER_TOKEN").unwrap_or_default();
    let client = RemoteClient::new(&url, &token).expect("client");
    let caps = client.capabilities().expect("capabilities");
    assert!(
        caps.list_waveforms_usable(),
        "the server offers no list Waveforms"
    );
    let cancel = AtomicBool::new(false);
    let begin = Instant::now();
    let (mut next, mut requests, mut busy) = (start, 0_u32, 0_u32);
    let mut latencies = Vec::new();
    while begin.elapsed() < Duration::from_secs(seconds) {
        let ids: Vec<i64> = (next..next + 200).collect();
        let at = Instant::now();
        match client.list_waveform_batch(&ids, &BTreeMap::new(), &cancel) {
            Ok(reply) => {
                latencies.push(at.elapsed().as_secs_f64() * 1000.0);
                requests += 1;
                let pending = reply
                    .items
                    .iter()
                    .any(|item| format!("{:?}", item.status) == "Pending");
                if pending {
                    std::thread::sleep(Duration::from_secs(
                        reply.retry_after.unwrap_or(2).clamp(1, 30),
                    ));
                } else {
                    next += 200;
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
            Err(error) => {
                busy += 1;
                eprintln!("request failed: {error}");
                std::thread::sleep(Duration::from_secs(2));
            }
        }
    }
    latencies.sort_by(f64::total_cmp);
    let p = |q: f64| {
        latencies
            .get(((latencies.len() as f64 * q) as usize).min(latencies.len().saturating_sub(1)))
            .copied()
    };
    println!(
        "{{\"requests\":{requests},\"errors\":{busy},\"post_ms_median\":{:?},\"post_ms_p95\":{:?},\"last_id\":{next}}}",
        p(0.5),
        p(0.95)
    );
}
