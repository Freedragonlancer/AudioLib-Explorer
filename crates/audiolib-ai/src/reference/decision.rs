// crates/audiolib-ai/src/reference/decision.rs
//! Bounded local records for decision models; selected text is always reconstructed locally.
use super::{ReferenceIndex, validate_intent};
use crate::{AiError, Result};
use audiolib_core::ai_search::{AiQueryKind, AiQueryPlan};
use serde::Serialize;
use std::collections::HashSet;

const MAX_KEYWORDS: usize = 24;
const MAX_CATEGORIES: usize = 24;
const MAX_CANDIDATES: usize = 254;

/// A finite, locally owned option. Its ID never authorizes model-generated queries.
#[derive(Clone, Debug, Serialize)]
pub struct DecisionCandidate {
    pub id: String,
    pub kind: &'static str,
    pub title: String,
    pub queries: Vec<String>,
    pub note: String,
}

fn chinese(c: char) -> bool {
    ('\u{3400}'..='\u{9fff}').contains(&c)
}

fn generic_pair(value: &str) -> bool {
    matches!(
        value,
        "我要"
            | "想要"
            | "我想"
            | "需要"
            | "寻找"
            | "找个"
            | "声音"
            | "音效"
            | "素材"
            | "一个"
            | "一些"
            | "设计"
            | "类型"
            | "一般"
            | "这里"
            | "比如"
            | "用于"
            | "可以"
            | "表示"
            | "常用"
            | "常见"
            | "其他"
            | "这种"
            | "或者"
            | "以及"
            | "效果"
            | "细节"
            | "部分"
            | "声源"
    )
}

/// These single-character source names carry meaning; arbitrary Chinese characters do not.
fn source_character(value: &str) -> bool {
    matches!(
        value,
        "风" | "雨"
            | "雷"
            | "水"
            | "火"
            | "冰"
            | "雪"
            | "狗"
            | "猫"
            | "鸟"
            | "牛"
            | "马"
            | "羊"
            | "猪"
            | "蛙"
            | "虫"
            | "鼓"
            | "钟"
            | "铃"
            | "枪"
            | "剑"
            | "刀"
            | "车"
            | "船"
            | "门"
            | "木"
            | "铁"
    )
}

fn contains_phrase(haystack: &str, needle: &str) -> bool {
    haystack.match_indices(needle).any(|(start, _)| {
        let end = start + needle.len();
        let left = !needle.as_bytes()[0].is_ascii_alphanumeric()
            || start == 0
            || !haystack.as_bytes()[start - 1].is_ascii_alphanumeric();
        let right = !needle.as_bytes()[needle.len() - 1].is_ascii_alphanumeric()
            || end == haystack.len()
            || !haystack.as_bytes()[end].is_ascii_alphanumeric();
        left && right
    })
}

struct IntentTerms {
    query: String,
    words: Vec<String>,
    chinese_pairs: HashSet<String>,
}

impl IntentTerms {
    fn new(intent: &str) -> Self {
        let query = intent.trim().to_lowercase();
        let words = query
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|word| word.len() >= 2)
            .map(str::to_owned)
            .collect();
        let chinese_pairs = query
            .split(|c: char| !chinese(c))
            .flat_map(|run| {
                run.chars()
                    .collect::<Vec<_>>()
                    .windows(2)
                    .map(|pair| pair.iter().collect::<String>())
                    .filter(|pair| !generic_pair(pair))
                    .collect::<Vec<_>>()
            })
            .collect();
        Self {
            query,
            words,
            chinese_pairs,
        }
    }

    fn weight(&self, candidate: &str) -> usize {
        let candidate = candidate.trim().to_lowercase();
        if candidate.is_empty() || generic_pair(&candidate) {
            return 0;
        }
        if candidate.chars().count() == 1 && !source_character(&candidate) {
            return 0;
        }
        if self.query == candidate {
            return 8;
        }
        if contains_phrase(&self.query, &candidate) {
            return 4;
        }
        if self
            .words
            .iter()
            .any(|word| contains_phrase(&candidate, word))
            || self
                .chinese_pairs
                .iter()
                .any(|pair| candidate.contains(pair))
        {
            return 1;
        }
        0
    }
}

impl ReferenceIndex {
    /// Supply every recipe plus bounded lexical keyword/UCS matches, with stable local IDs.
    /// The current corpus admits at most 67 candidates, leaving room for an uncertainty option.
    /// Query text and ordering match the plan admitted for the requested layer mode.
    pub fn decision_candidates(&self, intent: &str, layers: bool) -> Vec<DecisionCandidate> {
        if validate_intent(intent).is_err() {
            return Vec::new();
        }
        let terms = IntentTerms::new(intent);
        let mut recipes: Vec<_> = self.recipes.iter().collect();
        recipes.sort_by(|a, b| a.id.cmp(&b.id));
        let mut candidates: Vec<_> = recipes
            .into_iter()
            .map(|recipe| DecisionCandidate {
                id: recipe.id.clone(),
                kind: "recipe",
                title: recipe.title.clone(),
                queries: self
                    .selection_plan(intent, &recipe.id, layers)
                    .map(|plan| plan.queries.into_iter().map(|query| query.text).collect())
                    .unwrap_or_default(),
                note: format!("{}；{}", recipe.title, recipe.note),
            })
            .collect();
        let mut words: Vec<_> = self
            .words
            .iter()
            .map(|word| {
                let score = terms.weight(&word.term) * 8
                    + word
                        .alternatives
                        .iter()
                        .map(|alternative| terms.weight(alternative) * 4)
                        .sum::<usize>()
                    + word
                        .zh
                        .split(['，', ',', '、', '；', ';', '。'])
                        .map(|part| terms.weight(part) * 2)
                        .sum::<usize>()
                    + terms.weight(&word.domain);
                (score, word)
            })
            .filter(|(score, _)| *score > 0)
            .collect();
        words.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.id.cmp(&b.1.id)));
        for (_, word) in words.into_iter().take(MAX_KEYWORDS) {
            candidates.push(DecisionCandidate {
                id: word.id.clone(),
                kind: "keyword",
                title: format!("{} — {}", word.term, word.zh),
                queries: self
                    .selection_plan(intent, &word.id, layers)
                    .map(|plan| plan.queries.into_iter().map(|query| query.text).collect())
                    .unwrap_or_default(),
                note: format!("{}；领域：{}；{}", word.zh, word.domain, word.note),
            });
        }
        let mut categories: Vec<_> = self
            .categories
            .iter()
            .map(|category| {
                let score = terms.weight(&category.id) * 10
                    + terms.weight(&category.category) * 3
                    + terms.weight(&category.subcategory) * 4
                    + terms.weight(&category.category_zh) * 3
                    + terms.weight(&category.subcategory_zh) * 4
                    + category
                        .synonyms
                        .split(',')
                        .chain(category.synonyms_zh.split('、'))
                        .map(|synonym| terms.weight(synonym))
                        .sum::<usize>();
                (score, category)
            })
            .filter(|(score, _)| *score > 0)
            .collect();
        categories.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.id.cmp(&b.1.id)));
        for (_, category) in categories.into_iter().take(MAX_CATEGORIES) {
            candidates.push(DecisionCandidate {
                id: category.id.clone(),
                kind: "ucs",
                title: format!(
                    "{} / {}（{} / {}）",
                    category.category,
                    category.subcategory,
                    category.category_zh,
                    category.subcategory_zh
                ),
                queries: self
                    .selection_plan(intent, &category.id, layers)
                    .map(|plan| plan.queries.into_iter().map(|query| query.text).collect())
                    .unwrap_or_default(),
                note: format!(
                    "{}；{}；{}；UCS CatID 仅适合带有此命名或 Metadata 的素材",
                    category.description, category.synonyms, category.synonyms_zh
                ),
            });
        }
        let mut ids = HashSet::new();
        candidates
            .retain(|candidate| !candidate.queries.is_empty() && ids.insert(candidate.id.clone()));
        candidates.truncate(MAX_CANDIDATES);
        candidates
    }

    /// Reconstruct an existing recipe, keyword or CatID without admitting model-written text.
    pub fn selection_plan(&self, intent: &str, id: &str, layers: bool) -> Result<AiQueryPlan> {
        validate_intent(intent).map_err(|_| AiError::Invalid)?;
        if self.recipes.iter().any(|recipe| recipe.id == id) {
            return self.recipe_plan(intent, id, layers);
        }
        let mut queries = Vec::new();
        if let Some(word) = self.words.iter().find(|word| word.id == id) {
            for text in std::iter::once(&word.term).chain(&word.alternatives) {
                Self::add(
                    &mut queries,
                    text,
                    AiQueryKind::Main,
                    "keyword",
                    &word.zh,
                    &word.id,
                    &word.ucs_catids,
                );
            }
        } else if let Some(category) = self.categories.iter().find(|category| category.id == id) {
            Self::add(
                &mut queries,
                &category.id,
                AiQueryKind::Main,
                "UCS",
                &format!("{} / {}", category.category, category.subcategory),
                &category.id,
                std::slice::from_ref(&category.id),
            );
        } else {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use audiolib_core::ai_search::MAX_AI_QUERIES;

    #[test]
    fn decisions_recall_keywords_in_english_and_unsegmented_chinese() {
        let index = ReferenceIndex::embedded();
        for (intent, id) in [
            ("wind", "P0300"),
            ("我要找风吹过树林的声音", "P0300"),
            ("我要找脚步", "P0120"),
            ("汽车发动机怠速", "P0282"),
        ] {
            assert!(
                index
                    .decision_candidates(intent, false)
                    .iter()
                    .any(|candidate| { candidate.kind == "keyword" && candidate.id == id }),
                "missing {id} for {intent}"
            );
        }
        let wood = index.decision_candidates("木地板上的脚步", false);
        assert!(wood.iter().any(|candidate| candidate.id == "P0120"));
        assert!(wood.iter().any(|candidate| {
            candidate.id == "footsteps"
                && candidate.note.contains("碎石")
                && candidate
                    .queries
                    .iter()
                    .any(|query| query.contains("gravel"))
        }));
    }

    #[test]
    fn decisions_recall_catids_and_chinese_source_names() {
        let index = ReferenceIndex::embedded();
        for (intent, id) in [
            ("EXPLDsgn", "EXPLDsgn"),
            ("我要找狗叫的声音", "ANMLDog"),
            ("风", "WIND"),
        ] {
            assert!(
                index
                    .decision_candidates(intent, false)
                    .iter()
                    .any(|candidate| { candidate.kind == "ucs" && candidate.id == id }),
                "missing {id} for {intent}"
            );
        }
    }

    #[test]
    fn decision_options_are_bounded_unique_stable_and_keep_all_recipes() {
        let index = ReferenceIndex::embedded();
        let intent = "风雨雷电 火焰 冰 狗猫鸟 水流 脚步 木材 金属 发动机 爆炸";
        let candidates = index.decision_candidates(intent, true);
        assert_eq!(
            candidates
                .iter()
                .filter(|item| item.kind == "recipe")
                .count(),
            19
        );
        assert!(
            candidates
                .iter()
                .filter(|item| item.kind == "keyword")
                .count()
                <= MAX_KEYWORDS
        );
        assert!(candidates.iter().filter(|item| item.kind == "ucs").count() <= MAX_CATEGORIES);
        assert!(candidates.len() <= 67 && candidates.len() < 255);
        let ids: Vec<_> = candidates.iter().map(|candidate| &candidate.id).collect();
        assert_eq!(ids.iter().collect::<HashSet<_>>().len(), ids.len());
        assert_eq!(
            serde_json::to_value(&candidates).unwrap(),
            serde_json::to_value(index.decision_candidates(intent, true)).unwrap()
        );
        for candidate in candidates {
            let plan = index.selection_plan(intent, &candidate.id, true).unwrap();
            assert!(
                plan.queries
                    .iter()
                    .all(|query| query.source_ids == [candidate.id.clone()])
            );
        }
        assert!(
            index
                .decision_candidates("声音", true)
                .iter()
                .all(|candidate| candidate.kind == "recipe")
        );
        assert!(index.decision_candidates("", true).is_empty());
        assert!(
            index
                .decision_candidates(&"风".repeat(501), true)
                .is_empty()
        );
        assert!(index.selection_plan("wind", "fabricated", false).is_err());
        assert!(index.selection_plan("", "P0300", false).is_err());
    }

    #[test]
    fn candidate_queries_match_selected_plans_in_both_layer_modes() {
        let index = ReferenceIndex::embedded();
        for layers in [false, true] {
            for intent in [
                "科幻爆炸",
                "wind",
                "木地板上的脚步",
                "风雨雷电 火焰 冰 狗猫鸟 水流 脚步 木材 金属 发动机 爆炸",
            ] {
                for candidate in index.decision_candidates(intent, layers) {
                    let expected = index
                        .selection_plan(intent, &candidate.id, layers)
                        .unwrap()
                        .queries
                        .into_iter()
                        .map(|query| query.text)
                        .collect::<Vec<_>>();
                    assert_eq!(
                        candidate.queries, expected,
                        "{}; layers={layers}",
                        candidate.id
                    );
                    assert!(candidate.queries.len() <= MAX_AI_QUERIES);
                }
            }
        }

        let direct = index
            .decision_candidates("sub boom", false)
            .into_iter()
            .find(|candidate| candidate.id == "scifi-explosion")
            .unwrap();
        assert_eq!(direct.queries.len(), 3);
        assert!(!direct.queries.iter().any(|query| query == "sub boom"));
        assert!(!direct.note.contains("sub boom"));

        let layered = index
            .decision_candidates("reverb tail", true)
            .into_iter()
            .find(|candidate| candidate.id == "scifi-explosion")
            .unwrap();
        assert_eq!(layered.queries.len(), MAX_AI_QUERIES);
        assert!(layered.queries.iter().any(|query| query == "sub boom"));
        assert!(!layered.queries.iter().any(|query| query == "reverb tail"));
        assert!(!layered.note.contains("reverb tail"));
    }

    #[test]
    fn selected_queries_keep_local_sources_and_every_recipe_layer() {
        let index = ReferenceIndex::embedded();
        let plan = index.selection_plan("风", "P0300", false).unwrap();
        assert_eq!(plan.queries[0].text, "wind");
        assert_eq!(plan.queries[0].source_ids, ["P0300"]);
        let plan = index.selection_plan("狗叫", "ANMLDog", false).unwrap();
        assert_eq!(plan.queries[0].text, "ANMLDog");
        assert_eq!(plan.queries[0].source_ids, ["ANMLDog"]);
        assert_eq!(plan.queries[0].cat_ids, ["ANMLDog"]);
        for recipe in &index.recipes {
            let plan = index
                .selection_plan(&recipe.title, &recipe.id, true)
                .unwrap();
            assert!(plan.queries.len() <= MAX_AI_QUERIES);
            for layer in &recipe.layers {
                assert!(plan.queries.iter().any(|query| query.layer == layer.role));
            }
        }
        let plan = index
            .selection_plan("科幻爆炸", "scifi-explosion", true)
            .unwrap();
        assert_eq!(plan.queries.len(), MAX_AI_QUERIES);
        assert!(
            plan.queries
                .iter()
                .any(|query| query.text == "rumble" && query.layer == "尾音")
        );
    }
}
