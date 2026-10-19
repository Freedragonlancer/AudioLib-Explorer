// crates/audiolib-remote/examples/list_waveform_load.rs
//! Server-mode list Waveform load for the zero-latency acceptance: pages through ids with
//! `POST /waveforms` (200 ids, one request in flight, `retry_after` respected, like the
//! desktop worker) for a fixed time, printing a `BATCH` line per request. With
//! `--status-only`, it instead polls `GET /waveforms/status` every second and prints
//! `STATUS` lines. Read-only: it never calls PATCH, scans or removals.
//!
//! ```text
//! $env:ALX_SERVER_URL = '<server url>'      # never committed anywhere
//! cargo run --release -p audiolib-remote --example list_waveform_load -- --seconds 120 [--start-id 1]
//! cargo run --release -p audiolib-remote --example list_waveform_load -- --status-only 120
//! ```
//!
//! Run it next to `first-sound-loopback` or `stream_bench --quick` while the server's
//! pre-generation is running.
use audiolib_remote::{BatchStatus, RemoteClient};
use std::{
    collections::BTreeMap,
    sync::atomic::AtomicBool,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

fn epoch_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let value = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
    };
    let url = std::env::var("ALX_SERVER_URL").expect("set ALX_SERVER_URL to the server address");
    let token = std::env::var("ALX_SERVER_TOKEN").unwrap_or_default();
    let client = RemoteClient::new(&url, &token).expect("client");
    if let Some(seconds) = value("--status-only").and_then(|v| v.parse::<u64>().ok()) {
        let begin = Instant::now();
        while begin.elapsed() < Duration::from_secs(seconds) {
            let tick = Instant::now();
            match client.list_waveform_status() {
                Ok(status) => println!(
                    "STATUS {}",
                    serde_json::json!({"t": epoch_ms(), "state": status.state,
                        "yield_reason": status.yield_reason, "cached": status.cached,
                        "remaining": status.remaining, "queued_on_demand": status.queued_on_demand,
                        "failed": status.failed, "rate": status.rate})
                ),
                Err(error) => println!(
                    "STATUS {}",
                    serde_json::json!({"t": epoch_ms(), "error": error.to_string()})
                ),
            }
            std::thread::sleep(Duration::from_secs(1).saturating_sub(tick.elapsed()));
        }
        return;
    }
    let seconds: u64 = value("--seconds")
        .and_then(|v| v.parse().ok())
        .unwrap_or(120);
    let start: i64 = value("--start-id")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let caps = client.capabilities().expect("capabilities");
    assert!(
        caps.list_waveforms_usable(),
        "the server offers no list Waveforms"
    );
    let cancel = AtomicBool::new(false);
    let begin = Instant::now();
    let (mut next, mut requests, mut errors, mut ok_total) = (start, 0_u32, 0_u32, 0_usize);
    let mut latencies = Vec::new();
    while begin.elapsed() < Duration::from_secs(seconds) {
        let ids: Vec<i64> = (next..next + 200).collect();
        let at = Instant::now();
        match client.list_waveform_batch(&ids, &BTreeMap::new(), &cancel) {
            Ok(reply) => {
                let ms = at.elapsed().as_secs_f64() * 1000.0;
                latencies.push(ms);
                requests += 1;
                let count =
                    |status: BatchStatus| reply.items.iter().filter(|i| i.status == status).count();
                let (ok, pending) = (count(BatchStatus::Ok), count(BatchStatus::Pending));
                ok_total += ok;
                println!(
                    "BATCH {}",
                    serde_json::json!({"t": epoch_ms(), "first_id": next, "ms": ms, "ok": ok,
                        "pending": pending, "missing": count(BatchStatus::Missing),
                        "root_offline": count(BatchStatus::RootOffline),
                        "error": count(BatchStatus::Error), "retry_after": reply.retry_after})
                );
                if pending > 0 {
                    std::thread::sleep(Duration::from_secs(
                        reply.retry_after.unwrap_or(2).clamp(1, 30),
                    ));
                } else {
                    next += 200;
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
            Err(error) => {
                errors += 1;
                println!(
                    "BATCH {}",
                    serde_json::json!({"t": epoch_ms(), "first_id": next, "failure": error.to_string()})
                );
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
        "SUMMARY {}",
        serde_json::json!({"requests": requests, "errors": errors, "ok_total": ok_total,
            "post_ms_median": p(0.5), "post_ms_p95": p(0.95), "last_id": next})
    );
}
