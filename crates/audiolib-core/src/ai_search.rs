// crates/audiolib-core/src/ai_search.rs
//! Bounded, owned AI suggestions. A plan never grants database or network authority.
use crate::{Error, Result, SearchRequest};
use serde::{Deserialize, Serialize};

/// Maximum raw candidates admitted before current-library hit verification.
pub const MAX_AI_QUERIES: usize = 12;

/// Maximum verified main recommendations displayed, with or without layers.
pub const MAX_AI_MAIN_RECOMMENDATIONS: usize = 6;

/// Maximum verified main and layer recommendations displayed together.
pub const MAX_AI_LAYERED_RECOMMENDATIONS: usize = 8;

/// Explicit recommendation purpose, independent of its human-readable layer label.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiQueryKind {
    /// A finished sound or ordinary keyword suggestion; also the legacy default.
    #[default]
    Main,
    /// A useful independent source layer for constructing the described sound.
    Layer,
}

/// One independently executable plain-text AND query, not an FTS expression.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiQuery {
    pub text: String,
    pub reason: String,
    /// Missing kinds in older responses remain main recommendations.
    #[serde(default)]
    pub kind: AiQueryKind,
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
                || (query.kind == AiQueryKind::Layer && query.layer.trim().is_empty())
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
    fn query_kind_round_trips_and_legacy_labels_do_not_infer_layers() {
        let legacy = serde_json::json!({
            "text":"metal impact","reason":"Useful impact","layer":"尾音",
            "source_ids":[],"cat_ids":[]
        });
        let main: AiQuery = serde_json::from_value(legacy.clone()).unwrap();
        assert_eq!(main.kind, AiQueryKind::Main);
        assert_eq!(serde_json::to_value(&main).unwrap()["kind"], "main");

        let mut explicit = legacy.clone();
        explicit["kind"] = serde_json::json!("layer");
        let layer: AiQuery = serde_json::from_value(explicit.clone()).unwrap();
        assert_eq!(layer.kind, AiQueryKind::Layer);
        assert_eq!(serde_json::to_value(&layer).unwrap(), explicit);

        for invalid in [serde_json::json!("tail"), serde_json::Value::Null] {
            let mut query = legacy.clone();
            query["kind"] = invalid;
            assert!(serde_json::from_value::<AiQuery>(query).is_err());
        }
    }

    #[test]
    fn explicit_layers_need_a_role_and_raw_candidates_keep_their_separate_bound() {
        let query: AiQuery = serde_json::from_value(serde_json::json!({
            "text":"wind","reason":"Wind recording","kind":"main","layer":"",
            "source_ids":[],"cat_ids":[]
        }))
        .unwrap();
        let mut plan = AiQueryPlan {
            schema_version: 1,
            intent: "wind".into(),
            queries: vec![query; MAX_AI_QUERIES],
            cautions: vec![],
        };
        // Display limits must not reject extra raw hit-verification candidates.
        plan.validate().unwrap();
        plan.queries[0].kind = AiQueryKind::Layer;
        for role in ["", " \t "] {
            plan.queries[0].layer = role.into();
            assert!(plan.validate().is_err());
        }
        plan.queries[0].layer = "air texture".into();
        plan.validate().unwrap();
        plan.queries.push(plan.queries[0].clone());
        assert!(plan.validate().is_err());
    }

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
