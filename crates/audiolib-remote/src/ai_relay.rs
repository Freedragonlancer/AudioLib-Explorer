// crates/audiolib-remote/src/ai_relay.rs
//! Bounded, cancellable AI forwarding through the configured AudioLib server.
//!
//! Provider credentials travel only in a single POST body. AI forwarding is independent
//! of the selected audio library: no snapshot, library identity or write lease is used.
use crate::{RemoteClient, abort::Timeouts, client::cancellable_io_with_stall};
use audiolib_ai::{
    AiError, Result, check_cancel,
    config::{ApiKey, Profile},
    provider,
    relay::{RelayRequest, RelayResponse},
};
use std::{io::Read, sync::atomic::AtomicBool, time::Duration};

const MAX_RELAY_REQUEST_BYTES: usize = 64 * 1024;
const MAX_RELAY_BYTES: u64 = 512 * 1024;
const RELAY_MARGIN: Duration = Duration::from_secs(5);

#[cfg(test)]
thread_local! {
    static TEST_DEADLINE: std::cell::Cell<Option<Duration>> = const {
        std::cell::Cell::new(None)
    };
}

fn deadline(timeout_seconds: u64) -> Duration {
    #[cfg(test)]
    if let Some(duration) = TEST_DEADLINE.with(std::cell::Cell::get) {
        return duration;
    }
    Duration::from_secs(timeout_seconds).saturating_add(RELAY_MARGIN)
}

fn transport_error(error: ureq::Error, timeout_seconds: u64) -> AiError {
    match error {
        ureq::Error::Timeout(_) => AiError::Timeout(timeout_seconds),
        ureq::Error::Io(error) if error.kind() == std::io::ErrorKind::TimedOut => {
            AiError::Timeout(timeout_seconds)
        }
        _ => AiError::RelayTransport,
    }
}

impl RemoteClient {
    /// Refresh provider models through AudioLib, without contacting the provider here.
    pub fn ai_models(
        &self,
        profile: &Profile,
        key: &ApiKey,
        cancel: &AtomicBool,
    ) -> Result<Vec<String>> {
        let request = RelayRequest::models(profile, key)?;
        self.ai_request("/ai/models", &request, profile.timeout_seconds, cancel)?
            .into_models()
    }

    /// Make one billable provider attempt through AudioLib; never retry or fall back.
    #[allow(clippy::too_many_arguments)]
    pub fn ai_generate(
        &self,
        profile: &Profile,
        key: &ApiKey,
        intent: &str,
        layers: bool,
        guidance: &str,
        session: &str,
        cancel: &AtomicBool,
    ) -> Result<provider::Output> {
        let request = RelayRequest::generate(profile, key, intent, layers, guidance, session)?;
        self.ai_request("/ai/generate", &request, profile.timeout_seconds, cancel)?
            .into_output(profile, intent, layers)
    }

    fn ai_request(
        &self,
        path: &str,
        request: &RelayRequest,
        timeout_seconds: u64,
        cancel: &AtomicBool,
    ) -> Result<RelayResponse> {
        check_cancel(cancel)?;
        let bytes = serde_json::to_vec(request).map_err(|_| AiError::Invalid)?;
        if bytes.len() > MAX_RELAY_REQUEST_BYTES {
            return Err(AiError::Invalid);
        }
        let duration = deadline(timeout_seconds);
        // Keep the typed AI/upstream result inside the remote cancellation wrapper.
        // The wrapper owns the abort scope; cancellation is checked again afterward
        // so socket shutdown errors cannot hide the user's cancellation.
        let result = cancellable_io_with_stall(cancel, duration, || {
            Ok(self.ai_exchange(path, &bytes, timeout_seconds, duration))
        });
        check_cancel(cancel)?;
        result.map_err(|_| AiError::RelayTransport)?
    }

    fn ai_exchange(
        &self,
        path: &str,
        body: &[u8],
        timeout_seconds: u64,
        duration: Duration,
    ) -> Result<RelayResponse> {
        let url = format!("{}{path}", self.api);
        let mut request = Timeouts::new(duration, Some(duration))
            .post(&url)
            .config()
            // One deadline covers the head and entire body, including relay overhead.
            .timeout_global(Some(duration))
            .max_redirects(0)
            .build()
            .header("Content-Type", "application/json");
        if !self.token.is_empty() {
            request = request.header("Authorization", format!("Bearer {}", self.token));
        }
        // Do not use call_fresh: a lost response must never repeat a billable request.
        let mut response = request
            .send(body)
            .map_err(|error| transport_error(error, timeout_seconds))?;
        if !response.status().is_success() {
            // The server status belongs to AudioLib, whereas typed upstream errors are
            // decoded from a successful RelayResponse. Never surface an HTTP body.
            return Err(AiError::RelayHttp(response.status().as_u16()));
        }
        let mut bytes = Vec::new();
        response
            .body_mut()
            .as_reader()
            .take(MAX_RELAY_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| transport_error(error.into(), timeout_seconds))?;
        if bytes.len() as u64 > MAX_RELAY_BYTES {
            return Err(AiError::Response);
        }
        serde_json::from_slice(&bytes).map_err(|_| AiError::Response)
    }
}

#[cfg(test)]
#[path = "ai_relay_tests.rs"]
mod tests;
