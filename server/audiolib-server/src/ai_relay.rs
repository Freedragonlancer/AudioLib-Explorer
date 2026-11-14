// server/audiolib-server/src/ai_relay.rs
//! Explicitly approved AI upstreams, isolated from library readiness and writes.
use crate::{ApiError, ServerConfig, Service};
use audiolib_ai::{
    provider,
    relay::{RelayOperation, RelayProfile, RelayRequest, RelayResponse, upstream_identity},
};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State, rejection::JsonRejection},
    http::StatusCode,
    routing::post,
};
use std::{
    collections::HashSet,
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};
use tokio::sync::Semaphore;

/// Allow a maximum 600s upstream deadline plus response serialization overhead.
pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(620);
const MAX_BODY_BYTES: usize = 64 * 1024;
const MAX_UPSTREAMS: usize = 64;

/// Process-local, non-secret policy and permits shared by every AI route.
pub(crate) struct AiRelay {
    upstreams: HashSet<String>,
    slots: Arc<Semaphore>,
}

impl Default for AiRelay {
    fn default() -> Self {
        Self {
            upstreams: HashSet::new(),
            slots: Arc::new(Semaphore::new(crate::config::DEFAULT_MAX_AI_REQUESTS)),
        }
    }
}

pub(crate) fn validate_config(config: &ServerConfig) -> anyhow::Result<()> {
    if !(1..=64).contains(&config.max_ai_requests) {
        anyhow::bail!("max_ai_requests must be 1..=64");
    }
    if config.ai_upstreams.len() > MAX_UPSTREAMS {
        anyhow::bail!("ai_upstreams may contain at most 64 entries");
    }
    approved_identities(config)?;
    Ok(())
}

fn approved_identities(config: &ServerConfig) -> anyhow::Result<HashSet<String>> {
    let mut identities = HashSet::new();
    for upstream in &config.ai_upstreams {
        let identity = upstream_identity(&RelayProfile {
            protocol: upstream.protocol,
            base_url: upstream.base_url.clone(),
            model: String::new(),
            timeout_seconds: 180,
        })
        .map_err(|_| anyhow::anyhow!("ai_upstreams contains an invalid protocol or base URL"))?;
        if !identities.insert(identity) {
            anyhow::bail!("ai_upstreams contains duplicate normalized endpoints");
        }
    }
    Ok(identities)
}

impl AiRelay {
    pub(crate) fn from_config(config: &ServerConfig) -> anyhow::Result<Self> {
        validate_config(config)?;
        Ok(Self {
            upstreams: approved_identities(config)?,
            slots: Arc::new(Semaphore::new(config.max_ai_requests)),
        })
    }

    fn admit(&self, profile: &RelayProfile) -> Result<(), ApiError> {
        if self.upstreams.is_empty() {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "AI_DISABLED",
                "AI relay has no administrator-approved upstreams",
            ));
        }
        let identity = upstream_identity(profile).map_err(|_| invalid_request())?;
        if !self.upstreams.contains(&identity) {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "AI_UPSTREAM_FORBIDDEN",
                "The AI protocol and endpoint are not approved by the server administrator",
            ));
        }
        Ok(())
    }
}

pub(crate) fn router() -> Router<Service> {
    Router::new()
        .route("/ai/models", post(models))
        .route("/ai/generate", post(generate))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
}

async fn models(
    State(service): State<Service>,
    request: Result<Json<RelayRequest>, JsonRejection>,
) -> Result<Json<RelayResponse>, ApiError> {
    relay(service, request, true).await
}

async fn generate(
    State(service): State<Service>,
    request: Result<Json<RelayRequest>, JsonRejection>,
) -> Result<Json<RelayResponse>, ApiError> {
    relay(service, request, false).await
}

fn invalid_request() -> ApiError {
    ApiError::validation("Invalid AI relay request")
}

async fn relay(
    service: Service,
    request: Result<Json<RelayRequest>, JsonRejection>,
    models: bool,
) -> Result<Json<RelayResponse>, ApiError> {
    // Do not return serde diagnostics: an invalid field can contain a credential.
    let Json(request) = request.map_err(|error| {
        if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
            ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "AI_REQUEST_TOO_LARGE",
                "AI relay request exceeds the 64 KiB limit",
            )
        } else {
            invalid_request()
        }
    })?;
    if matches!(&request.operation, RelayOperation::Models) != models {
        return Err(invalid_request());
    }
    request.validate().map_err(|_| invalid_request())?;
    let relay = service.ai_relay();
    relay.admit(&request.profile)?;
    let permit =
        relay.slots.clone().try_acquire_owned().map_err(|_| {
            ApiError::fs_busy("All AI relay request slots are in use; retry shortly")
        })?;
    let response = tokio::task::spawn_blocking(move || {
        // Keep the permit in the worker: HTTP cancellation cannot stop a blocking
        // network call and must not release its concurrency slot prematurely.
        let _permit = permit;
        let profile = request.profile.to_profile();
        let key = request.key();
        let cancel = AtomicBool::new(false);
        match request.operation {
            RelayOperation::Models => RelayResponse::from_models(
                key.and_then(|key| provider::models(&profile, &key, &cancel)),
            ),
            RelayOperation::Generate {
                intent,
                layers,
                guidance,
                session,
            } => RelayResponse::from_output(key.and_then(|key| {
                provider::generate(
                    &profile, &key, &intent, layers, &guidance, &session, &cancel,
                )
            })),
        }
    })
    .await
    .map_err(|_| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "AI_RELAY_UNAVAILABLE",
            "The AI relay worker could not complete the request",
        )
    })?;
    Ok(Json(response))
}
