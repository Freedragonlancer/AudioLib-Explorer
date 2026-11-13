// crates/audiolib-core/src/ai_search.rs
//! Bounded, owned AI suggestions. A plan never grants database or network authority.
use crate::{Error, Result, SearchRequest};
use serde::{Deserialize, Serialize};

/// Maximum suggestions shown or admitted from a model response.
pub const MAX_AI_QUERIES: usize = 12;

/// One independently executable plain-text AND query, not an FTS expression.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiQuery {
    pub text: String,
    pub reason: String,
    pub layer: String,
    pub source_ids: Vec<String>,
    pub cat_ids: Vec<String>,
}

/// Suggestions only; the desktop supplies library identity, filters and scope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiQueryPlan {
    pub schema_version: u32,
    pub intent: String,
    pub queries: Vec<AiQuery>,
    pub cautions: Vec<String>,
}

/// Validate natural-language input independently of the executable query grammar.
pub fn validate_intent(value: &str) -> Result<()> {
    if value.trim().is_empty() || value.chars().count() > 500 || value.contains('\0') {
        return Err(Error::Validation(
            "AI input must contain 1–500 characters and no NUL".into(),
        ));
    }
    Ok(())
}

/// Admit only syntax understood by the ordinary search field.
pub fn validate_ai_query(value: &str) -> Result<()> {
    if value.trim().is_empty()
        || value.contains([';', '\0', '*', '"', '(', ')', '\n', '\r'])
        || value
            .split_whitespace()
            .any(|word| matches!(word, "AND" | "OR" | "NOT"))
    {
        return Err(Error::Validation(
            "AI queries must be independent plain-text searches".into(),
        ));
    }
    SearchRequest {
        query: value.into(),
        ..Default::default()
    }
    .validate()
}

impl AiQueryPlan {
    /// Check structural bounds; the AI service additionally checks reference IDs.
    pub fn validate(&self) -> Result<()> {
        validate_intent(&self.intent)?;
        if self.schema_version != 1
            || self.queries.is_empty()
            || self.queries.len() > MAX_AI_QUERIES
            || self.cautions.len() > 5
            || self.cautions.iter().any(|s| s.chars().count() > 300)
        {
            return Err(Error::Validation("Invalid AI plan version or size".into()));
        }
        for query in &self.queries {
            validate_ai_query(&query.text)?;
            if query.reason.chars().count() > 300
                || query.layer.chars().count() > 40
                || query.source_ids.len() > 6
                || query.cat_ids.len() > 6
                || query
                    .source_ids
                    .iter()
                    .chain(&query.cat_ids)
                    .any(|id| id.len() > 80)
            {
                return Err(Error::Validation("AI suggestion exceeds its bounds".into()));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn executable_queries_cannot_smuggle_boolean_or_shell_syntax() {
        for value in [
            "explosion OR blast",
            "metal;DELETE",
            "\"plasma\"*",
            "",
            "metal\0",
        ] {
            assert!(validate_ai_query(value).is_err(), "{value:?}");
        }
        assert!(validate_ai_query("plasma explosion").is_ok());
        assert!(validate_ai_query("风").is_ok());
        assert!(validate_intent(&"声".repeat(501)).is_err());
    }
}
