// crates/audiolib-ai/src/lib.rs
//! Local sound-design references and bounded optional AI provider calls.
pub mod config;
pub mod jev;
pub mod provider;
pub mod reference;
pub mod relay;
pub mod storage;

use std::sync::atomic::{AtomicBool, Ordering};

/// Errors contain fixed diagnostics, never HTTP bodies, URLs, prompts or secrets.
#[derive(Debug, thiserror::Error)]
pub enum AiError {
    /// A valid decision that should not be automatically accepted.
    #[error("Decision uncertain; refine the description or use local suggestions")]
    Uncertain,
    #[error("Invalid AI input or configuration")]
    Invalid,
    #[error("AI request cancelled")]
    Cancelled,
    #[error("AI network request failed")]
    Transport,
    /// The configured global deadline elapsed before a complete response arrived.
    #[error("AI request timed out after {0} seconds")]
    Timeout(u64),
    #[error("AI service returned HTTP {0}")]
    Http(u16),
    #[error("AI response failed structured validation")]
    Response,
    #[error("AI configuration could not be read or saved; the original file is retained")]
    Storage,
    #[error("This platform supports session-only AI keys")]
    SessionOnly,
    #[error("No AudioLib server is configured for AI requests")]
    RelayUnavailable,
    #[error("The AudioLib AI relay could not be reached")]
    RelayTransport,
    #[error("The AudioLib AI relay returned HTTP {0}")]
    RelayHttp(u16),
}

/// Result with redacted AI service errors.
pub type Result<T> = std::result::Result<T, AiError>;

/// Check cancellation before I/O and before publishing or applying a response.
pub fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Acquire) {
        Err(AiError::Cancelled)
    } else {
        Ok(())
    }
}
