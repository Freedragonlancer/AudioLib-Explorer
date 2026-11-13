// crates/audiolib-ai/src/config.rs
//! Provider identity and configuration; API keys are deliberately separate.
use crate::{AiError, Result};
use serde::{Deserialize, Serialize};

/// User-requested default role prompt; fixed query/response contracts are appended separately.
pub const DEFAULT_PROMPT: &str =
    "作为资深声音设计师，请根据输入内容，合理匹配声音设计需要用的专业Keyword";

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
        }
    }
}

/// Non-secret profile. Limits are local request caps, not provider balances.
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
    pub max_requests_per_month: u64,
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
            timeout_seconds: 15,
            max_requests_per_month: 1000,
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
            || !(2..=60).contains(&self.timeout_seconds)
            || !(1..=100_000).contains(&self.max_requests_per_month)
        {
            return Err(AiError::Invalid);
        }
        validate_url(&self.base_url)?;
        Ok(())
    }
    /// Validate a generation request. Subscription authorization belongs to the service.
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
