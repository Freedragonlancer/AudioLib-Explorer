// crates/audiolib-ai/src/config.rs
//! Provider identity and configuration; API keys are deliberately separate.
use crate::{AiError, Result};
use serde::{Deserialize, Serialize};

/// User-requested default role prompt; fixed query/response contracts are appended separately.
pub const DEFAULT_PROMPT: &str =
    "作为资深声音设计师，请根据输入内容，合理匹配声音设计需要用的专业Keyword";

/// Minimum user-selected provider request timeout.
pub const MIN_TIMEOUT_SECONDS: u64 = 2;
/// Initial timeout for new profiles; saved choices retain their original value.
pub const DEFAULT_TIMEOUT_SECONDS: u64 = 180;
/// Bounded allowance for providers with slow reasoning or gateway queuing.
pub const MAX_TIMEOUT_SECONDS: u64 = 600;

/// Bounded optional role guidance, independent from the concrete search intent.
pub fn validate_instruction(value: &str) -> Result<()> {
    if value.trim().is_empty() || value.chars().count() > 2000 || value.contains('\0') {
        return Err(AiError::Invalid);
    }
    Ok(())
}

/// Global prompt choice. The last custom draft survives switching back to the default.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptSettings {
    pub use_custom: bool,
    pub custom: String,
}
impl Default for PromptSettings {
    fn default() -> Self {
        Self {
            use_custom: false,
            custom: DEFAULT_PROMPT.into(),
        }
    }
}
impl PromptSettings {
    /// Effective prompt passed to the selected remote provider.
    pub fn effective(&self) -> &str {
        if self.use_custom {
            &self.custom
        } else {
            DEFAULT_PROMPT
        }
    }
}

/// Wire protocols are explicit; decision models never masquerade as chat models.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    #[default]
    ChatCompletions,
    Responses,
    Anthropic,
    Gemini,
    Jev,
    OpenCodeGo,
    DeepSeek,
    Magpie,
}

impl Protocol {
    /// Human-readable provider label, also used for unnamed profile suggestions.
    pub fn display_name(self) -> &'static str {
        match self {
            Self::ChatCompletions => "OpenAI Compatible",
            Self::Responses => "OpenAI Responses",
            Self::Anthropic => "Anthropic",
            Self::Gemini => "Gemini",
            Self::Jev => "Jev TypeSafe",
            Self::OpenCodeGo => "OpenCode Go",
            Self::DeepSeek => "DeepSeek",
            Self::Magpie => "Magpie",
        }
    }

    /// Stable index used by the desktop protocol selector.
    pub fn index(self) -> i32 {
        match self {
            Self::ChatCompletions => 0,
            Self::Responses => 1,
            Self::Anthropic => 2,
            Self::Gemini => 3,
            Self::Jev => 4,
            Self::OpenCodeGo => 5,
            Self::DeepSeek => 6,
            Self::Magpie => 7,
        }
    }
    /// Unknown UI indices never silently choose a network protocol.
    pub fn from_index(index: i32) -> Result<Self> {
        match index {
            0 => Ok(Self::ChatCompletions),
            1 => Ok(Self::Responses),
            2 => Ok(Self::Anthropic),
            3 => Ok(Self::Gemini),
            4 => Ok(Self::Jev),
            5 => Ok(Self::OpenCodeGo),
            6 => Ok(Self::DeepSeek),
            7 => Ok(Self::Magpie),
            _ => Err(AiError::Invalid),
        }
    }
    /// Public initial endpoint; users explicitly configure custom services.
    pub fn default_url(self) -> &'static str {
        match self {
            Self::ChatCompletions | Self::Responses => "https://api.openai.com/v1",
            Self::Anthropic => "https://api.anthropic.com/v1",
            Self::Gemini => "https://generativelanguage.googleapis.com/v1beta",
            Self::Jev => "https://api.typesafe.ai/v1",
            Self::OpenCodeGo => "https://opencode.ai/inference/go",
            Self::DeepSeek => "https://api.deepseek.com",
            Self::Magpie => "http://127.0.0.1:3425/v1",
        }
    }
}

/// Where an optional provider request is executed. Existing profiles use the client.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestMode {
    #[default]
    Client,
    Server,
}

impl RequestMode {
    /// Stable index used by the advanced-settings selector.
    pub fn index(self) -> i32 {
        match self {
            Self::Client => 0,
            Self::Server => 1,
        }
    }

    /// Invalid UI choices cannot silently redirect credentials or requests.
    pub fn from_index(index: i32) -> Result<Self> {
        match index {
            0 => Ok(Self::Client),
            1 => Ok(Self::Server),
            _ => Err(AiError::Invalid),
        }
    }
}

/// Non-secret provider configuration; credentials are stored separately.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub protocol: Protocol,
    pub base_url: String,
    pub model: String,
    pub credential_ref: String,
    pub timeout_seconds: u64,
    #[serde(default)]
    pub request_mode: RequestMode,
    /// Accepted only when reading older profiles; no local quota is enforced.
    #[serde(default, skip_serializing)]
    pub max_requests_per_month: u64,
    /// Accepted only when reading older profiles; provider plans are not managed here.
    #[serde(default, skip_serializing)]
    pub subscription: bool,
    /// Legacy preference retained for backwards compatibility; not an API permission.
    #[serde(default)]
    pub go_use_authorized: bool,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: "AI".into(),
            protocol: Protocol::default(),
            base_url: Protocol::default().default_url().into(),
            model: String::new(),
            credential_ref: uuid::Uuid::new_v4().to_string(),
            timeout_seconds: DEFAULT_TIMEOUT_SECONDS,
            request_mode: RequestMode::Client,
            max_requests_per_month: 0,
            subscription: false,
            go_use_authorized: false,
        }
    }
}

impl Profile {
    /// Validate URL authority and bounded request controls before storing or sending.
    pub fn validate(&self) -> Result<()> {
        self.validate_discovery()?;
        if !valid_model_id(&self.model) {
            return Err(AiError::Invalid);
        }
        Ok(())
    }
    /// Model discovery only needs a safe endpoint and bounded controls, not a model ID.
    pub fn validate_discovery(&self) -> Result<()> {
        if self.name.trim().is_empty()
            || self.name.chars().count() > 80
            || self.id.len() > 80
            || self.credential_ref.len() > 80
            || self.id.is_empty()
            || self.credential_ref.is_empty()
            || !(MIN_TIMEOUT_SECONDS..=MAX_TIMEOUT_SECONDS).contains(&self.timeout_seconds)
        {
            return Err(AiError::Invalid);
        }
        validate_endpoint_url(&self.base_url, self.protocol == Protocol::Magpie)?;
        Ok(())
    }
    /// Validate a generation request; provider account controls belong to the provider.
    pub fn validate_request(&self) -> Result<()> {
        self.validate()
    }
    /// Discovery does not require a selected model or a local eligibility checkbox.
    pub fn validate_discovery_request(&self) -> Result<()> {
        self.validate_discovery()
    }
}

/// The same model identifiers are accepted by discovery, persistence and generation.
pub fn valid_model_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 120
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | ':'))
}

/// HTTPS for remote services; unencrypted HTTP only for an explicit loopback host.
pub fn validate_url(value: &str) -> Result<()> {
    validate_endpoint_url(value, false)
}

// Magpie explicitly supports user-selected LAN gateways. Keep this exception
// scoped to that profile protocol and literal private IPs; never resolve DNS
// names to decide whether an otherwise remote HTTP endpoint is trusted.
fn validate_endpoint_url(value: &str, allow_private_http: bool) -> Result<()> {
    if value.len() > 500
        || value.contains(['@', '?', '#', '\\'])
        || value.chars().any(char::is_whitespace)
    {
        return Err(AiError::Invalid);
    }
    let uri: ureq::http::Uri = value.parse().map_err(|_| AiError::Invalid)?;
    let host = uri.host().ok_or(AiError::Invalid)?;
    if host.is_empty() || !matches!(uri.scheme_str(), Some("https") | Some("http")) {
        return Err(AiError::Invalid);
    }
    if uri.scheme_str() == Some("http")
        && !matches!(host, "127.0.0.1" | "localhost" | "[::1]" | "::1")
        && !(allow_private_http
            && host
                .trim_start_matches('[')
                .trim_end_matches(']')
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| match ip {
                    std::net::IpAddr::V4(ip) => ip.is_private(),
                    std::net::IpAddr::V6(ip) => ip.is_unique_local(),
                }))
    {
        return Err(AiError::Invalid);
    }
    Ok(())
}

/// Secret bytes have a redacted Debug implementation and are never serializable.
#[derive(Clone, Default)]
pub struct ApiKey(String);
impl std::fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ApiKey([redacted])")
    }
}
impl ApiKey {
    /// Reject values that cannot be safely placed in an HTTP header.
    pub fn new(value: String) -> Result<Self> {
        if value.len() > 4096 || value.chars().any(char::is_control) {
            Err(AiError::Invalid)
        } else {
            Ok(Self(value))
        }
    }
    /// Whether the profile has a session or stored credential.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn provider_timeouts_accept_slow_requests_and_remain_bounded() {
        assert_eq!(Profile::default().timeout_seconds, 180);
        for timeout_seconds in [2, 15, 60, 120, 180, 300, 600] {
            let profile = Profile {
                model: "owned-model".into(),
                timeout_seconds,
                ..Default::default()
            };
            profile.validate_discovery_request().unwrap();
            profile.validate_request().unwrap();
        }
        for timeout_seconds in [0, 1, 601, u64::MAX] {
            let profile = Profile {
                model: "owned-model".into(),
                timeout_seconds,
                ..Default::default()
            };
            assert!(profile.validate_discovery_request().is_err());
            assert!(profile.validate_request().is_err());
        }
    }

    #[test]
    fn old_profiles_default_to_client_and_retired_controls_are_not_written() {
        let original = Profile {
            model: "owned-model".into(),
            ..Default::default()
        };
        let mut json = serde_json::to_value(&original).unwrap();
        json.as_object_mut().unwrap().remove("request_mode");
        json["max_requests_per_month"] = serde_json::json!(0);
        json["subscription"] = serde_json::json!(true);
        let mut loaded: Profile = serde_json::from_value(json).unwrap();
        assert_eq!(loaded.request_mode, RequestMode::Client);
        loaded.validate().unwrap();
        loaded.request_mode = RequestMode::Server;
        let encoded = serde_json::to_value(&loaded).unwrap();
        assert_eq!(encoded["request_mode"], "server");
        assert!(encoded.get("max_requests_per_month").is_none());
        assert!(encoded.get("subscription").is_none());
        assert_eq!(RequestMode::from_index(0).unwrap(), RequestMode::Client);
        assert_eq!(RequestMode::from_index(1).unwrap().index(), 1);
        assert!(RequestMode::from_index(2).is_err());
        assert!(RequestMode::from_index(-1).is_err());
    }

    #[test]
    fn saved_timeouts_survive_new_defaults_and_explicit_longer_edits() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = crate::storage::ProfileStore::open(dir.path()).unwrap();
        for timeout_seconds in [15, 45, 60] {
            store
                .save(
                    Profile {
                        protocol: Protocol::Magpie,
                        base_url: Protocol::Magpie.default_url().into(),
                        model: "owned-model".into(),
                        timeout_seconds,
                        ..Default::default()
                    },
                    None,
                )
                .unwrap();
        }
        drop(store);
        let mut reopened = crate::storage::ProfileStore::open(dir.path()).unwrap();
        assert_eq!(
            reopened
                .profiles()
                .iter()
                .map(|profile| profile.timeout_seconds)
                .collect::<Vec<_>>(),
            [15, 45, 60]
        );
        let mut selected = reopened.selected().unwrap().clone();
        selected.name = "Renamed profile".into();
        reopened.save(selected, None).unwrap();
        assert_eq!(reopened.selected().unwrap().timeout_seconds, 60);
        let mut selected = reopened.selected().unwrap().clone();
        selected.timeout_seconds = 300;
        reopened.save(selected, None).unwrap();
        drop(reopened);
        let reopened = crate::storage::ProfileStore::open(dir.path()).unwrap();
        assert_eq!(
            reopened
                .profiles()
                .iter()
                .map(|profile| profile.timeout_seconds)
                .collect::<Vec<_>>(),
            [15, 45, 300]
        );
    }

    #[test]
    fn only_magpie_profiles_admit_literal_private_http_gateways() {
        for url in [
            "http://192.168.31.3:3425/v1",
            "http://10.0.0.1:3425/v1",
            "http://172.16.0.1/v1",
            "http://172.31.255.254/v1",
            "http://[fd00::3]:3425/v1",
            "http://[fc00::3]/v1",
        ] {
            assert!(validate_url(url).is_err(), "{url}");
            for protocol in [
                Protocol::Magpie,
                Protocol::ChatCompletions,
                Protocol::Responses,
                Protocol::Anthropic,
                Protocol::Gemini,
                Protocol::Jev,
                Protocol::OpenCodeGo,
                Protocol::DeepSeek,
            ] {
                let profile = Profile {
                    protocol,
                    base_url: url.into(),
                    ..Default::default()
                };
                assert_eq!(
                    profile.validate_discovery_request().is_ok(),
                    protocol == Protocol::Magpie,
                    "{protocol:?}: {url}"
                );
            }
        }
        for url in [
            "http://8.8.8.8/v1",
            "http://172.15.255.255/v1",
            "http://172.32.0.1/v1",
            "http://192.169.0.1/v1",
            "http://100.64.0.1/v1",
            "http://169.254.0.1/v1",
            "http://0.0.0.0/v1",
            "http://224.0.0.1/v1",
            "http://[fe80::3]/v1",
            "http://[2001:db8::3]/v1",
            "http://[::ffff:192.168.31.3]/v1",
            "http://192.168.31.3.example/v1",
            "http://magpie.local/v1",
            "http://user@192.168.31.3/v1",
            "http://192.168.31.3/v1?key=secret",
        ] {
            let profile = Profile {
                protocol: Protocol::Magpie,
                base_url: url.into(),
                ..Default::default()
            };
            assert!(profile.validate_discovery_request().is_err(), "{url}");
        }
    }

    #[test]
    fn magpie_appends_a_stable_protocol_and_round_trips_gateway_profiles() {
        for (index, protocol, wire) in [
            (0, Protocol::ChatCompletions, "chat_completions"),
            (1, Protocol::Responses, "responses"),
            (2, Protocol::Anthropic, "anthropic"),
            (3, Protocol::Gemini, "gemini"),
            (4, Protocol::Jev, "jev"),
            (5, Protocol::OpenCodeGo, "open_code_go"),
            (6, Protocol::DeepSeek, "deep_seek"),
            (7, Protocol::Magpie, "magpie"),
        ] {
            assert_eq!(protocol.index(), index);
            assert_eq!(Protocol::from_index(index).unwrap(), protocol);
            let serialized = serde_json::to_string(&protocol).unwrap();
            assert_eq!(serialized, format!("\"{wire}\""));
            assert_eq!(
                serde_json::from_str::<Protocol>(&serialized).unwrap(),
                protocol
            );
        }
        assert!(Protocol::from_index(8).is_err());
        assert!(Protocol::from_index(-1).is_err());
        let profile = Profile {
            protocol: Protocol::Magpie,
            base_url: Protocol::Magpie.default_url().into(),
            model: "group/sound-design".into(),
            ..Default::default()
        };
        assert_eq!(profile.base_url, "http://127.0.0.1:3425/v1");
        profile.validate_request().unwrap();
        let serialized = serde_json::to_string(&profile).unwrap();
        assert_eq!(
            serde_json::from_str::<Profile>(&serialized).unwrap(),
            profile
        );
    }

    #[test]
    fn endpoint_boundaries_and_go_requests_without_legacy_checkbox() {
        for url in [
            "https://a.example/v1?key=secret",
            "https://user:pass@a.example/v1",
            "http://192.168.1.2/v1",
            "file:///secret",
            "https://a.example/#x",
        ] {
            assert!(validate_url(url).is_err());
        }
        for url in [
            "https://api.openai.com/v1",
            "http://127.0.0.1:9000/v1",
            "http://[::1]:9000",
        ] {
            assert!(validate_url(url).is_ok(), "{url}");
        }
        let p = Profile {
            protocol: Protocol::OpenCodeGo,
            model: "configured-model".into(),
            ..Default::default()
        };
        assert!(p.validate_request().is_ok());
        assert!(!format!("{:?}", ApiKey::new("SECRET".into()).unwrap()).contains("SECRET"));
        let mut draft = Profile {
            protocol: Protocol::DeepSeek,
            base_url: Protocol::DeepSeek.default_url().into(),
            ..Default::default()
        };
        assert_eq!(draft.base_url, "https://api.deepseek.com");
        assert!(draft.validate_discovery_request().is_ok());
        assert!(draft.validate().is_err());
        draft.protocol = Protocol::OpenCodeGo;
        assert!(draft.validate_discovery_request().is_ok());
        assert_eq!(Protocol::from_index(6).unwrap(), Protocol::DeepSeek);
    }
}
