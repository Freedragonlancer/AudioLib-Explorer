// crates/audiolib-ai/src/reference.rs
//! Embedded reference retrieval. Never reads a working library or source audio.
use crate::{AiError, Result};
use audiolib_core::ai_search::{
    AiQuery, AiQueryKind, AiQueryPlan, MAX_AI_QUERIES, validate_ai_query, validate_intent,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::sync::OnceLock;

mod decision;
pub use decision::DecisionCandidate;

const WORDS: &str =
    include_str!("../../../.agents/skills/audio-keyword-search/data/keywords.jsonl");
const UCS: &str = include_str!("../../../.agents/skills/audio-keyword-search/data/ucs.jsonl");
const RECIPES: &str =
    include_str!("../../../.agents/skills/audio-keyword-search/data/recipes.jsonl");
/// Changes when the embedded reference content changes.
pub const REFERENCE_VERSION: &str = "1.0.0-UCS8.2.1";

#[derive(Deserialize)]
struct Keyword {
    id: String,
    term: String,
    alternatives: Vec<String>,
    zh: String,
    domain: String,
    ucs_catids: Vec<String>,
    note: String,
}
#[derive(Deserialize)]
struct Category {
    id: String,
    category: String,
    subcategory: String,
    category_zh: String,
    subcategory_zh: String,
    synonyms: String,
    synonyms_zh: String,
    description: String,
}
/// One read-only design recipe supplied to the decision provider as a candidate.
#[derive(Clone, Debug, Deserialize)]
pub struct Recipe {
    pub id: String,
    pub title: String,
    pub triggers: Vec<String>,
    pub queries: Vec<String>,
    pub layers: Vec<Layer>,
    pub ucs_catids: Vec<String>,
    pub broader_queries: Vec<String>,
    pub note: String,
}
/// Independent search alternatives for a sound-design layer.
#[derive(Clone, Debug, Deserialize)]
pub struct Layer {
    pub role: String,
    pub queries: Vec<String>,
}

/// Immutable embedded lookup data, initialized once outside audio callbacks.
pub struct ReferenceIndex {
    words: Vec<Keyword>,
    categories: Vec<Category>,
    recipes: Vec<Recipe>,
    ids: HashSet<String>,
    cat_ids: HashSet<String>,
}

fn parse<T: serde::de::DeserializeOwned>(text: &str) -> Vec<T> {
    text.lines()
        .map(|line| serde_json::from_str(line).expect("validated embedded AI reference"))
        .collect()
}

fn weight(query: &str, candidate: &str) -> usize {
    let candidate = candidate.trim().to_lowercase();
    if candidate.is_empty() {
        return 0;
    }
    if query == candidate {
        return 8;
    }
    if query.contains(&candidate) {
        return 4;
    }
    if query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .filter(|word| {
            word.chars().count() >= 2
                || word.chars().any(|c| ('\u{3400}'..='\u{9fff}').contains(&c))
        })
        .any(|word| candidate.contains(word))
    {
        1
    } else {
        0
    }
}

impl ReferenceIndex {
    /// Shared offline data; no file or network access on subsequent lookups.
    pub fn embedded() -> &'static Self {
        static INDEX: OnceLock<ReferenceIndex> = OnceLock::new();
        INDEX.get_or_init(|| {
            let words: Vec<Keyword> = parse(WORDS);
            let categories: Vec<Category> = parse(UCS);
            let recipes: Vec<Recipe> = parse(RECIPES);
            let ids = words
                .iter()
                .map(|x| x.id.clone())
                .chain(recipes.iter().map(|x| x.id.clone()))
                .chain(categories.iter().map(|x| x.id.clone()))
                .collect();
            let cat_ids = categories.iter().map(|x| x.id.clone()).collect();
            Self {
                words,
                categories,
                recipes,
                ids,
                cat_ids,
            }
        })
    }

    /// At most six ranked candidates; lexical score is not a model probability.
    pub fn recipes(&self, intent: &str) -> Vec<&Recipe> {
        let query = intent.to_lowercase();
        let mut ranked: Vec<_> = self
            .recipes
            .iter()
            .map(|r| {
                (
                    r.triggers
                        .iter()
                        .map(|t| weight(&query, t) * 6)
                        .sum::<usize>()
                        + weight(&query, &r.title) * 5,
                    r,
                )
            })
            .filter(|(s, _)| *s > 0)
            .collect();
        ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.id.cmp(&b.1.id)));
        ranked.into_iter().take(6).map(|(_, r)| r).collect()
    }

    fn add(
        queries: &mut Vec<AiQuery>,
        text: &str,
        kind: AiQueryKind,
        layer: &str,
        reason: &str,
        id: &str,
        cats: &[String],
    ) {
        if queries.len() == MAX_AI_QUERIES
            || validate_ai_query(text).is_err()
            || queries.iter().any(|q| q.text.eq_ignore_ascii_case(text))
        {
            return;
        }
        queries.push(AiQuery {
            text: text.into(),
            kind,
            layer: layer.into(),
            reason: reason.chars().take(300).collect(),
            source_ids: vec![id.into()],
            cat_ids: cats.iter().take(6).cloned().collect(),
        });
    }

    /// Convert an already validated recipe selection to deterministic query strings.
    pub fn recipe_plan(&self, intent: &str, id: &str, layers: bool) -> Result<AiQueryPlan> {
        let recipe = self
            .recipes
            .iter()
            .find(|r| r.id == id)
            .ok_or(AiError::Response)?;
        let mut queries = Vec::new();
        for text in &recipe.queries {
            Self::add(
                &mut queries,
                text,
                AiQueryKind::Main,
                "direct",
                &recipe.title,
                &recipe.id,
                &recipe.ucs_catids,
            );
        }
        if layers {
            // Visit every layer before adding its second alternative, so a bounded
            // plan retains the tail and other late layers in larger recipes.
            let alternatives = recipe
                .layers
                .iter()
                .map(|layer| layer.queries.len())
                .max()
                .unwrap_or(0);
            for alternative in 0..alternatives {
                for layer in &recipe.layers {
                    let Some(text) = layer.queries.get(alternative) else {
                        continue;
                    };
                    Self::add(
                        &mut queries,
                        text,
                        AiQueryKind::Layer,
                        &layer.role,
                        &recipe.title,
                        &recipe.id,
                        &[],
                    );
                }
            }
        }
        let plan = AiQueryPlan {
            schema_version: 1,
            intent: intent.into(),
            queries,
            cautions: vec![recipe.note.clone()],
        };
        self.validate(&plan)?;
        Ok(plan)
    }

    /// Offline suggestions. Ambiguous drone retains both meanings in direct mode.
    pub fn local_plan(&self, intent: &str, layers: bool) -> Result<AiQueryPlan> {
        validate_intent(intent).map_err(|_| AiError::Invalid)?;
        let recipes = self.recipes(intent);
        if let Some(recipe) = recipes.first() {
            let mut plan = self.recipe_plan(intent, &recipe.id, layers)?;
            if intent.trim().eq_ignore_ascii_case("drone") {
                for r in &recipes {
                    for q in &r.queries {
                        Self::add(
                            &mut plan.queries,
                            q,
                            AiQueryKind::Main,
                            "direct",
                            &r.title,
                            &r.id,
                            &r.ucs_catids,
                        );
                    }
                }
            }
            return Ok(plan);
        }
        let query = intent.to_lowercase();
        let mut ranked: Vec<_> = self
            .words
            .iter()
            .map(|k| {
                (
                    weight(&query, &k.term) * 8
                        + k.alternatives
                            .iter()
                            .map(|s| weight(&query, s) * 4)
                            .sum::<usize>()
                        + k.zh
                            .split(['，', ',', '、', '；', ';', '。'])
                            .map(|s| weight(&query, s) * 2)
                            .sum::<usize>()
                        + weight(&query, &k.domain),
                    k,
                )
            })
            .filter(|(s, _)| *s > 0)
            .collect();
        ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.id.cmp(&b.1.id)));
        let mut queries = Vec::new();
        for (_, k) in ranked.into_iter().take(4) {
            Self::add(
                &mut queries,
                &k.term,
                AiQueryKind::Main,
                "keyword",
                &k.zh,
                &k.id,
                &k.ucs_catids,
            );
            for alt in k.alternatives.iter().take(2) {
                Self::add(
                    &mut queries,
                    alt,
                    AiQueryKind::Main,
                    "keyword",
                    &k.zh,
                    &k.id,
                    &k.ucs_catids,
                );
            }
        }
        if queries.len() < 5 {
            let mut categories: Vec<_> = self
                .categories
                .iter()
                .map(|c| {
                    (
                        weight(&query, &c.id) * 10
                            + weight(&query, &c.category)
                            + weight(&query, &c.subcategory)
                            + weight(&query, &c.category_zh)
                            + weight(&query, &c.subcategory_zh)
                            + c.synonyms
                                .split(',')
                                .chain(c.synonyms_zh.split('、'))
                                .map(|s| weight(&query, s))
                                .sum::<usize>(),
                        c,
                    )
                })
                .filter(|(s, _)| *s > 0)
                .collect();
            categories.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.id.cmp(&b.1.id)));
            for (_, c) in categories.into_iter().take(3) {
                Self::add(
                    &mut queries,
                    &c.id,
                    AiQueryKind::Main,
                    "UCS",
                    &format!("{} / {}", c.category, c.subcategory),
                    &c.id,
                    std::slice::from_ref(&c.id),
                );
            }
        }
        if queries.is_empty() {
            return Err(AiError::Response);
        }
        let plan = AiQueryPlan {
            schema_version: 1,
            intent: intent.into(),
            queries,
            cautions: vec![],
        };
        self.validate(&plan)?;
        Ok(plan)
    }

    /// Reject fabricated CatIDs and provenance even from valid structured JSON.
    pub fn validate(&self, plan: &AiQueryPlan) -> Result<()> {
        plan.validate().map_err(|_| AiError::Response)?;
        if plan.queries.iter().any(|q| {
            q.source_ids.iter().any(|id| !self.ids.contains(id))
                || q.cat_ids.iter().any(|id| !self.cat_ids.contains(id))
        }) {
            return Err(AiError::Response);
        }
        Ok(())
    }

    /// Bounded public reference context; does not include user library metadata.
    pub fn context(&self, intent: &str, layers: bool) -> Value {
        json!({"reference_version":REFERENCE_VERSION,"suggestions":self.local_plan(intent,layers).ok(),"recipes":self.recipes(intent).iter().take(4).map(|r|json!({"id":r.id,"title":r.title,"queries":r.queries,"cat_ids":r.ucs_catids})).collect::<Vec<_>>()})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_kinds_distinguish_recipe_layers_without_inventing_keyword_layers() {
        let index = ReferenceIndex::embedded();
        let direct = index.local_plan("科幻爆炸", false).unwrap();
        assert!(
            direct
                .queries
                .iter()
                .all(|query| query.kind == AiQueryKind::Main)
        );
        let layered = index.local_plan("科幻爆炸", true).unwrap();
        assert_eq!(&layered.queries[..direct.queries.len()], &direct.queries);
        let layers = &layered.queries[direct.queries.len()..];
        assert!(!layers.is_empty());
        assert!(layers.iter().all(|query| {
            query.kind == AiQueryKind::Layer
                && query.source_ids == ["scifi-explosion"]
                && !query.layer.is_empty()
        }));
        let impact = layers
            .iter()
            .find(|query| query.text == "metal impact")
            .unwrap();
        assert_eq!(impact.layer, "音头");
        assert_eq!(impact.reason, direct.queries[0].reason);

        // Permission to consider layers does not turn keyword alternatives into
        // layers, and neither the local word nor its provenance is changed.
        let wind = index.local_plan("wind", false).unwrap();
        assert_eq!(index.local_plan("wind", true).unwrap(), wind);
        assert!(
            wind.queries
                .iter()
                .all(|query| query.kind == AiQueryKind::Main)
        );
        let category = index.selection_plan("狗叫", "ANMLDog", true).unwrap();
        assert!(
            category
                .queries
                .iter()
                .all(|query| query.kind == AiQueryKind::Main)
        );
        assert_eq!(category.queries[0].source_ids, ["ANMLDog"]);
        assert_eq!(category.queries[0].cat_ids, ["ANMLDog"]);
    }

    #[test]
    fn corpus_and_chinese_recipes_remain_executable() {
        let index = ReferenceIndex::embedded();
        assert_eq!(
            (
                index.words.len(),
                index.categories.len(),
                index.recipes.len()
            ),
            (431, 753, 19)
        );
        for recipe in &index.recipes {
            index.recipe_plan(&recipe.title, &recipe.id, true).unwrap();
        }
        let p = index.local_plan("我要设计科幻爆炸声", true).unwrap();
        assert_eq!(p.queries[0].text, "scifi explosion");
        assert!(p.queries.iter().any(|q| q.text == "sub boom"));
        assert!(
            index
                .local_plan("风", false)
                .unwrap()
                .queries
                .iter()
                .any(|q| q.text == "wind")
        );
        assert!(index.local_plan("qzxqzxqzxqzxq", false).is_err());
    }
    #[test]
    fn ambiguity_and_provenance_are_not_silently_discarded() {
        let index = ReferenceIndex::embedded();
        let p = index.local_plan("drone", false).unwrap();
        assert!(p.queries.iter().any(|q| q.text == "quadcopter"));
        assert!(p.queries.iter().any(|q| q.text == "dark drone"));
        let mut p = index.local_plan("爆炸", false).unwrap();
        p.queries[0].cat_ids.push("EXPLInvented".into());
        assert!(index.validate(&p).is_err());
    }
}
