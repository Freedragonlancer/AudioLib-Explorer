// crates/audiolib-ai/src/relay.rs
//! Bounded AudioLib server relay contract. Credentials are transient request data.
use crate::{
    AiError, Result,
    config::{ApiKey, Profile, Protocol, valid_model_id, validate_instruction},
    jev,
    provider::{self, Output},
};
use audiolib_core::ai_search::{AiQueryPlan, validate_intent};
use serde::{Deserialize, Serialize};

/// Provider coordinates only; local profile identities and key references stay local.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayProfile {
    pub protocol: Protocol,
    pub base_url: String,
    pub model: String,
    pub timeout_seconds: u64,
}

impl From<&Profile> for RelayProfile {
    fn from(profile: &Profile) -> Self {
        Self {
            protocol: profile.protocol,
            base_url: profile.base_url.clone(),
            model: profile.model.clone(),
            timeout_seconds: profile.timeout_seconds,
        }
    }
}

impl RelayProfile {
    /// Rebuild an ephemeral provider profile; no stored key can be selected by a caller.
    pub fn to_profile(&self) -> Profile {
        Profile {
            protocol: self.protocol,
            base_url: self.base_url.clone(),
            model: self.model.clone(),
            timeout_seconds: self.timeout_seconds,
            ..Default::default()
        }
    }
}

/// The only relay operations. Neither grants database, file or arbitrary HTTP access.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", from = "RelayOperationWire")]
pub enum RelayOperation {
    Models,
    Generate {
        intent: String,
        layers: bool,
        guidance: String,
        session: String,
    },
}

// Serde internally tagged unit variants ignore extra fields. Decode the unit
// operation as an empty struct variant so deny_unknown_fields actually applies.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum RelayOperationWire {
    Models {},
    Generate {
        intent: String,
        layers: bool,
        guidance: String,
        session: String,
    },
}

impl From<RelayOperationWire> for RelayOperation {
    fn from(value: RelayOperationWire) -> Self {
        match value {
            RelayOperationWire::Models {} => Self::Models,
            RelayOperationWire::Generate {
                intent,
                layers,
                guidance,
                session,
            } => Self::Generate {
                intent,
                layers,
                guidance,
                session,
            },
        }
    }
}

/// One explicit provider attempt. Debug deliberately omits the entire body.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayRequest {
    pub profile: RelayProfile,
    pub operation: RelayOperation,
    api_key: String,
}

impl std::fmt::Debug for RelayRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("RelayRequest([redacted])")
    }
}

impl RelayRequest {
    /// Construct model discovery without transmitting any app-local profile identity.
    pub fn models(profile: &Profile, key: &ApiKey) -> Result<Self> {
        let request = Self {
            profile: profile.into(),
            operation: RelayOperation::Models,
            api_key: key.expose().into(),
        };
        request.validate()?;
        Ok(request)
    }

    /// Construct one generation attempt; request strings remain bounded untrusted data.
    pub fn generate(
        profile: &Profile,
        key: &ApiKey,
        intent: &str,
        layers: bool,
        guidance: &str,
        session: &str,
    ) -> Result<Self> {
        let request = Self {
            profile: profile.into(),
            operation: RelayOperation::Generate {
                intent: intent.into(),
                layers,
                guidance: guidance.into(),
                session: session.into(),
            },
            api_key: key.expose().into(),
        };
        request.validate()?;
        Ok(request)
    }

    /// Validate again on the receiver before policy checks or provider I/O.
    pub fn validate(&self) -> Result<()> {
        self.key()?;
        upstream_identity(&self.profile)?;
        let profile = self.profile.to_profile();
        match &self.operation {
            RelayOperation::Models => profile.validate_discovery_request(),
            RelayOperation::Generate {
                intent,
                guidance,
                session,
                ..
            } => {
                profile.validate_request()?;
                validate_intent(intent).map_err(|_| AiError::Invalid)?;
                validate_instruction(guidance)?;
                if session.is_empty()
                    || session.len() > 80
                    || !session
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':' | '.'))
                {
                    return Err(AiError::Invalid);
                }
                Ok(())
            }
        }
    }

    /// Obtain a redacted non-serializable key for this attempt only.
    pub fn key(&self) -> Result<ApiKey> {
        ApiKey::new(self.api_key.clone())
    }
}

/// Exact upstream policy identity after the provider's documented base normalization.
/// Percent-encoded and dot-segment paths are refused to avoid proxy path ambiguity.
pub fn upstream_identity(profile: &RelayProfile) -> Result<String> {
    let provider = profile.to_profile();
    provider.validate_discovery_request()?;
    let uri: ureq::http::Uri = profile.base_url.parse().map_err(|_| AiError::Invalid)?;
    if uri.path().contains('%')
        || uri.path().contains("//")
        || uri.path().split('/').any(|part| matches!(part, "." | ".."))
    {
        return Err(AiError::Invalid);
    }
    let normalized = provider::request_url(&provider, "");
    let normalized: ureq::http::Uri = normalized.parse().map_err(|_| AiError::Invalid)?;
    let scheme = normalized.scheme_str().ok_or(AiError::Invalid)?;
    let authority = normalized.authority().ok_or(AiError::Invalid)?;
    let protocol = serde_json::to_string(&profile.protocol).map_err(|_| AiError::Invalid)?;
    Ok(format!(
        "{protocol}:{scheme}://{}{}",
        authority.as_str().to_ascii_lowercase(),
        normalized.path().trim_end_matches('/')
    ))
}

/// Provider failures carry fixed typed diagnostics, never response bodies or secrets.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", from = "RelayFailureWire")]
pub enum RelayFailure {
    ProviderInvalid,
    Cancelled,
    Transport,
    Timeout { seconds: u64 },
    Http { status: u16 },
    Response,
    Uncertain,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum RelayFailureWire {
    ProviderInvalid {},
    Cancelled {},
    Transport {},
    Timeout { seconds: u64 },
    Http { status: u16 },
    Response {},
    Uncertain {},
}

impl From<RelayFailureWire> for RelayFailure {
    fn from(value: RelayFailureWire) -> Self {
        match value {
            RelayFailureWire::ProviderInvalid {} => Self::ProviderInvalid,
            RelayFailureWire::Cancelled {} => Self::Cancelled,
            RelayFailureWire::Transport {} => Self::Transport,
            RelayFailureWire::Timeout { seconds } => Self::Timeout { seconds },
            RelayFailureWire::Http { status } => Self::Http { status },
            RelayFailureWire::Response {} => Self::Response,
            RelayFailureWire::Uncertain {} => Self::Uncertain,
        }
    }
}

impl From<AiError> for RelayFailure {
    fn from(error: AiError) -> Self {
        match error {
            AiError::Invalid => Self::ProviderInvalid,
            AiError::Cancelled => Self::Cancelled,
            AiError::Transport => Self::Transport,
            AiError::Timeout(seconds) => Self::Timeout { seconds },
            AiError::Http(status) => Self::Http { status },
            AiError::Uncertain => Self::Uncertain,
            _ => Self::Response,
        }
    }
}

impl RelayFailure {
    fn into_error(self) -> AiError {
        match self {
            Self::ProviderInvalid => AiError::Invalid,
            Self::Cancelled => AiError::Cancelled,
            Self::Transport => AiError::Transport,
            Self::Timeout { seconds }
                if (crate::config::MIN_TIMEOUT_SECONDS..=crate::config::MAX_TIMEOUT_SECONDS)
                    .contains(&seconds) =>
            {
                AiError::Timeout(seconds)
            }
            Self::Http { status } if (400..=599).contains(&status) => AiError::Http(status),
            Self::Uncertain => AiError::Uncertain,
            _ => AiError::Response,
        }
    }
}

/// Validated suggestions or a typed provider failure, separate from relay HTTP policy errors.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RelayResponse {
    Models {
        models: Vec<String>,
    },
    Generated {
        plan: AiQueryPlan,
        input_tokens: u64,
        output_tokens: u64,
        decision: Option<jev::Decision>,
    },
    Error {
        error: RelayFailure,
    },
}

impl RelayResponse {
    /// Encode model discovery without any upstream response body.
    pub fn from_models(result: Result<Vec<String>>) -> Self {
        match result {
            Ok(models) => Self::Models { models },
            Err(error) => Self::Error {
                error: error.into(),
            },
        }
    }

    /// Encode provider suggestions after the provider's strict response admission.
    pub fn from_output(result: Result<Output>) -> Self {
        match result {
            Ok(output) => Self::Generated {
                plan: output.plan,
                input_tokens: output.input_tokens,
                output_tokens: output.output_tokens,
                decision: output.decision,
            },
            Err(error) => Self::Error {
                error: error.into(),
            },
        }
    }

    /// Admit only the bounded model identifiers also accepted by direct discovery.
    pub fn into_models(self) -> Result<Vec<String>> {
        match self {
            Self::Models { mut models }
                if models.len() <= 200 && models.iter().all(|id| valid_model_id(id)) =>
            {
                models.sort();
                models.dedup();
                Ok(models)
            }
            Self::Error { error } => Err(error.into_error()),
            _ => Err(AiError::Response),
        }
    }

    /// Revalidate a returned plan locally before current-library hit verification.
    pub fn into_output(self, profile: &Profile, intent: &str, layers: bool) -> Result<Output> {
        profile.validate_request()?;
        validate_intent(intent).map_err(|_| AiError::Invalid)?;
        match self {
            Self::Generated {
                plan,
                input_tokens,
                output_tokens,
                decision,
            } if plan.intent == intent => provider::validate_output(
                Output {
                    plan,
                    input_tokens,
                    output_tokens,
                    decision,
                },
                profile.protocol,
                intent,
                layers,
            ),
            Self::Error { error } => Err(error.into_error()),
            _ => Err(AiError::Response),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::DEFAULT_PROMPT, reference::ReferenceIndex};

    fn profile() -> Profile {
        Profile {
            protocol: Protocol::Magpie,
            base_url: "http://127.0.0.1:3425/v1".into(),
            model: "group/owned-model".into(),
            ..Default::default()
        }
    }

    fn generated(plan: AiQueryPlan) -> RelayResponse {
        RelayResponse::Generated {
            plan,
            input_tokens: 13,
            output_tokens: 7,
            decision: None,
        }
    }

    #[test]
    fn requests_redact_the_whole_body_and_exclude_local_profile_identity() {
        let p = profile();
        let key = ApiKey::new("owned-secret".into()).unwrap();
        let request =
            RelayRequest::generate(&p, &key, "wind", false, DEFAULT_PROMPT, "owned-session")
                .unwrap();
        assert_eq!(format!("{request:?}"), "RelayRequest([redacted])");
        let value = serde_json::to_value(&request).unwrap();
        let encoded = value.to_string();
        assert!(!encoded.contains(&p.id));
        assert!(!encoded.contains(&p.credential_ref));
        assert_eq!(value["api_key"], "owned-secret");
        let decoded: RelayRequest = serde_json::from_value(value.clone()).unwrap();
        decoded.validate().unwrap();
        assert_eq!(decoded.key().unwrap().expose(), "owned-secret");
        let mut unknown = value;
        unknown["library_id"] = serde_json::json!("local-library");
        assert!(serde_json::from_value::<RelayRequest>(unknown).is_err());
    }

    #[test]
    fn requests_bound_strings_keys_timeouts_and_operation_fields() {
        let mut p = profile();
        let key = ApiKey::default();
        assert!(RelayRequest::models(&p, &key).is_ok());
        for (intent, guidance, session) in [
            ("", DEFAULT_PROMPT, "session"),
            ("wind", "", "session"),
            ("wind", DEFAULT_PROMPT, "bad\r\nheader"),
            ("wind", DEFAULT_PROMPT, ""),
        ] {
            assert!(RelayRequest::generate(&p, &key, intent, false, guidance, session).is_err());
        }
        p.timeout_seconds = 601;
        assert!(RelayRequest::models(&p, &key).is_err());
        p.timeout_seconds = 600;
        let mut value = serde_json::to_value(RelayRequest::models(&p, &key).unwrap()).unwrap();
        value["api_key"] = serde_json::json!("invalid\nkey");
        assert!(
            serde_json::from_value::<RelayRequest>(value)
                .unwrap()
                .validate()
                .is_err()
        );
        let unknown = serde_json::json!({"type":"models","intent":"wind"});
        assert!(serde_json::from_value::<RelayOperation>(unknown).is_err());
        let models = serde_json::json!({"type":"models"});
        assert!(matches!(
            serde_json::from_value::<RelayOperation>(models).unwrap(),
            RelayOperation::Models
        ));
    }

    #[test]
    fn upstream_policy_normalizes_documented_magpie_paths_only() {
        let p = profile();
        let identity = upstream_identity(&RelayProfile::from(&p)).unwrap();
        for base_url in [
            "http://127.0.0.1:3425",
            "http://127.0.0.1:3425/",
            "http://127.0.0.1:3425/v1/",
            "http://127.0.0.1:3425/v1/chat/completions",
        ] {
            let mut other = RelayProfile::from(&p);
            other.base_url = base_url.into();
            assert_eq!(upstream_identity(&other).unwrap(), identity);
        }
        for base_url in [
            "http://127.0.0.1:3425/a/../v1",
            "http://127.0.0.1:3425/%2ev1",
            "http://127.0.0.1:3425//v1",
        ] {
            let mut other = RelayProfile::from(&p);
            other.base_url = base_url.into();
            assert!(upstream_identity(&other).is_err());
        }
        for (protocol, base_url) in [
            (Protocol::Magpie, "http://127.0.0.1:3426/v1"),
            (Protocol::Magpie, "http://127.0.0.1:3425/prefix/v1"),
            (Protocol::ChatCompletions, "http://127.0.0.1:3425/v1"),
        ] {
            let mut other = RelayProfile::from(&p);
            other.protocol = protocol;
            other.base_url = base_url.into();
            assert_ne!(upstream_identity(&other).unwrap(), identity);
        }
    }

    #[test]
    fn model_responses_have_bounded_ids_and_typed_errors() {
        assert_eq!(
            RelayResponse::from_models(Ok(vec!["b".into(), "a".into(), "b".into()]))
                .into_models()
                .unwrap(),
            ["a", "b"]
        );
        for models in [vec!["invalid key".into()], vec!["mock".into(); 201]] {
            assert!(
                RelayResponse::from_models(Ok(models))
                    .into_models()
                    .is_err()
            );
        }
        assert!(matches!(
            RelayResponse::from_models(Err(AiError::Http(401))).into_models(),
            Err(AiError::Http(401))
        ));
        assert!(matches!(
            RelayResponse::from_output(Err(AiError::Timeout(600))).into_models(),
            Err(AiError::Timeout(600))
        ));
        let unknown = serde_json::json!({"type":"models","models":[],"raw_body":"secret"});
        assert!(serde_json::from_value::<RelayResponse>(unknown).is_err());
        for kind in [
            "provider_invalid",
            "cancelled",
            "transport",
            "response",
            "uncertain",
        ] {
            let valid = serde_json::json!({"type":"error","error":{"type":kind}});
            assert!(serde_json::from_value::<RelayResponse>(valid.clone()).is_ok());
            let mut unknown = valid;
            unknown["error"]["raw_body"] = serde_json::json!("secret");
            assert!(serde_json::from_value::<RelayResponse>(unknown).is_err());
        }
    }

    #[test]
    fn relayed_plans_retain_request_bounds_and_provenance_admission() {
        let p = profile();
        let plan = ReferenceIndex::embedded()
            .local_plan("wind", false)
            .unwrap();
        generated(plan.clone())
            .into_output(&p, "wind", false)
            .unwrap();
        let mut wrong_intent = plan.clone();
        wrong_intent.intent = "rain".into();
        assert!(
            generated(wrong_intent)
                .into_output(&p, "wind", false)
                .is_err()
        );
        let mut invalid = plan.clone();
        invalid.queries[0].source_ids = vec!["fabricated".into()];
        assert!(generated(invalid).into_output(&p, "wind", false).is_err());
        let mut invalid = plan;
        invalid.cautions = vec![String::new(); 6];
        assert!(generated(invalid).into_output(&p, "wind", false).is_err());
    }

    #[test]
    fn jev_relay_requires_local_candidate_plan_and_bounded_decision_evidence() {
        let mut p = profile();
        p.protocol = Protocol::Jev;
        p.base_url = Protocol::Jev.default_url().into();
        let candidate = ReferenceIndex::embedded()
            .decision_candidates("wind", false)
            .into_iter()
            .next()
            .unwrap();
        let plan = ReferenceIndex::embedded()
            .selection_plan("wind", &candidate.id, false)
            .unwrap();
        let decision = jev::Decision {
            recipe: candidate.id,
            confidence: 0.9,
            probability: 0.9,
            fit: 1.8,
            clarification: 0.1,
        };
        let response = |plan, decision| RelayResponse::Generated {
            plan,
            decision: Some(decision),
            input_tokens: 0,
            output_tokens: 0,
        };
        response(plan.clone(), decision.clone())
            .into_output(&p, "wind", false)
            .unwrap();
        let mut forged = plan.clone();
        forged.queries[0].text = "rain".into();
        assert!(
            response(forged, decision.clone())
                .into_output(&p, "wind", false)
                .is_err()
        );
        let mut bad = decision;
        bad.confidence = 1.1;
        assert!(response(plan, bad).into_output(&p, "wind", false).is_err());
    }
}
