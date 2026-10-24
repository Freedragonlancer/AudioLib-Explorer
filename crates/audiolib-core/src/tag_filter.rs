// crates/audiolib-core/src/tag_filter.rs
//! Include and exclude tag terms as typed into a filter box: `foo` or `+foo` includes,
//! `-foo` excludes. Terms map onto the `SearchFilters` tag fields; matching itself stays
//! in the database (case-insensitive literal substrings of library and extracted tags).
use crate::SearchFilters;

/// One include or exclude term, in the spelling it was entered with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagTerm {
    pub text: String,
    pub exclude: bool,
}

fn is_separator(character: char) -> bool {
    character.is_whitespace() || matches!(character, ',' | ';' | '|' | '，' | '；' | '、')
}

/// `Some(true)` for an exclude sign, `Some(false)` for an include sign.
fn side_of(character: char) -> Option<bool> {
    match character {
        '+' | '＋' => Some(false),
        '-' | '－' | '−' => Some(true),
        _ => None,
    }
}

fn closing_quote(character: char) -> Option<char> {
    match character {
        '"' => Some('"'),
        '“' => Some('”'),
        _ => None,
    }
}

fn push_term(terms: &mut Vec<TagTerm>, text: &str, exclude: bool) {
    let text = text.trim();
    if !text.is_empty() {
        terms.push(TagTerm {
            text: text.to_owned(),
            exclude,
        });
    }
}

/// Split typed text into terms. Whitespace, commas, semicolons (also full-width), `、`
/// and `|` separate terms. A leading `+` or `-` (also full-width, or `−`) chooses the
/// side, and a sign followed by a separator applies to the next term. A term that starts
/// with a quote keeps separators, e.g. `-"pass by"`; signs and quotes inside a term
/// (`pass-by`) are literal.
pub fn parse_tag_terms(input: &str) -> Vec<TagTerm> {
    let mut terms = Vec::new();
    let mut text = String::new();
    let mut exclude = false;
    let mut quoted = false;
    let mut quote = None;
    for character in input.chars() {
        if let Some(close) = quote {
            if character == close {
                quote = None;
            } else {
                text.push(character);
            }
            continue;
        }
        let fresh = text.is_empty() && !quoted;
        if is_separator(character) {
            if !fresh {
                push_term(&mut terms, &text, exclude);
                text.clear();
                exclude = false;
                quoted = false;
            }
        } else if fresh && let Some(side) = side_of(character) {
            exclude = side;
        } else if fresh && let Some(close) = closing_quote(character) {
            quote = Some(close);
            quoted = true;
        } else {
            text.push(character);
        }
    }
    push_term(&mut terms, &text, exclude);
    terms
}

/// Add terms, keeping one term per case-insensitive text: a repeated term keeps its
/// position and spelling and takes the side it was entered with last.
pub fn merge_tag_terms(terms: &mut Vec<TagTerm>, added: impl IntoIterator<Item = TagTerm>) {
    for term in added {
        let key = term.text.to_lowercase();
        match terms
            .iter_mut()
            .find(|existing| existing.text.to_lowercase() == key)
        {
            Some(existing) => existing.exclude = term.exclude,
            None => terms.push(term),
        }
    }
}

/// Canonical text that `parse_tag_terms` reads back as the same terms. A term containing
/// both `"` and `”` loses its `”` characters, the only spelling the syntax cannot quote.
pub fn format_tag_terms(terms: &[TagTerm]) -> String {
    terms
        .iter()
        .map(|term| {
            let sign = if term.exclude { "-" } else { "" };
            let plain = !term.text.chars().any(is_separator)
                && term.text.chars().next().is_some_and(|first| {
                    side_of(first).is_none() && closing_quote(first).is_none()
                });
            if plain {
                format!("{sign}{}", term.text)
            } else if !term.text.contains('"') {
                format!("{sign}\"{}\"", term.text)
            } else {
                format!("{sign}“{}”", term.text.replace('”', ""))
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Store terms in `filters`. The first include term goes to `tag_contains`, so a
/// single-term filter is exactly the request earlier versions send; further include
/// terms go to `include_tags` and exclude terms to `exclude_tags`.
pub fn apply_tag_terms(filters: &mut SearchFilters, terms: &[TagTerm]) {
    let mut include = terms
        .iter()
        .filter(|term| !term.exclude)
        .map(|term| term.text.clone());
    filters.tag_contains = include.next();
    filters.include_tags = include.collect();
    filters.exclude_tags = terms
        .iter()
        .filter(|term| term.exclude)
        .map(|term| term.text.clone())
        .collect();
}

/// The tag terms of `filters`: include terms first, then exclude terms.
pub fn tag_terms_of(filters: &SearchFilters) -> Vec<TagTerm> {
    let include = filters
        .tag_contains
        .iter()
        .chain(&filters.include_tags)
        .map(|text| TagTerm {
            text: text.clone(),
            exclude: false,
        });
    let exclude = filters.exclude_tags.iter().map(|text| TagTerm {
        text: text.clone(),
        exclude: true,
    });
    include.chain(exclude).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn term(text: &str, exclude: bool) -> TagTerm {
        TagTerm {
            text: text.into(),
            exclude,
        }
    }

    #[test]
    fn signs_choose_the_side_and_separators_split_terms() {
        assert_eq!(
            parse_tag_terms("engine +exterior -interior"),
            [
                term("engine", false),
                term("exterior", false),
                term("interior", true)
            ]
        );
        assert_eq!(
            parse_tag_terms(" a,b；c、d|e;f，g\th "),
            ["a", "b", "c", "d", "e", "f", "g", "h"].map(|text| term(text, false))
        );
        assert_eq!(
            parse_tag_terms("－室内 ＋室外 −雨"),
            [term("室内", true), term("室外", false), term("雨", true)]
        );
        // A sign alone applies to the next term; the last sign wins.
        assert_eq!(parse_tag_terms("- door"), [term("door", true)]);
        assert_eq!(parse_tag_terms("+ -door"), [term("door", true)]);
        assert!(parse_tag_terms("").is_empty());
        assert!(parse_tag_terms(" , ; - + \"\" ").is_empty());
    }

    #[test]
    fn quotes_keep_separators_and_inner_signs_stay_literal() {
        assert_eq!(
            parse_tag_terms("-\"pass by\", “heavy metal”, pass-by, a+b, x\"y"),
            [
                term("pass by", true),
                term("heavy metal", false),
                term("pass-by", false),
                term("a+b", false),
                term("x\"y", false)
            ]
        );
        // An unterminated quote keeps the rest of the input in one term.
        assert_eq!(
            parse_tag_terms("\"open end, still"),
            [term("open end, still", false)]
        );
    }

    #[test]
    fn repeated_terms_keep_position_and_take_the_last_side() {
        let mut terms = parse_tag_terms("Engine, door");
        merge_tag_terms(&mut terms, parse_tag_terms("-ENGINE rain door"));
        assert_eq!(
            terms,
            [
                term("Engine", true),
                term("door", false),
                term("rain", false)
            ]
        );
        merge_tag_terms(&mut terms, parse_tag_terms("+engine"));
        assert_eq!(terms[0], term("Engine", false));
    }

    #[test]
    fn formatted_terms_parse_back_unchanged() {
        let terms = vec![
            term("engine", false),
            term("pass by", true),
            term("-literal", false),
            term("+plus", true),
            term("\"quoted\" text", false),
            term("a,b", true),
            term("木门、脚步", false),
            term("x\"y", true),
        ];
        let text = format_tag_terms(&terms);
        assert_eq!(parse_tag_terms(&text), terms, "{text}");
        assert_eq!(format_tag_terms(&[]), "");
    }

    #[test]
    fn terms_map_onto_search_filters_and_back() {
        let terms = parse_tag_terms("metal -interior door -rain");
        let mut filters = SearchFilters::default();
        apply_tag_terms(&mut filters, &terms);
        assert_eq!(filters.tag_contains.as_deref(), Some("metal"));
        assert_eq!(filters.include_tags, ["door"]);
        assert_eq!(filters.exclude_tags, ["interior", "rain"]);
        assert_eq!(
            tag_terms_of(&filters),
            [
                term("metal", false),
                term("door", false),
                term("interior", true),
                term("rain", true)
            ]
        );
        // A single include term is the request earlier versions build.
        apply_tag_terms(&mut filters, &parse_tag_terms("metal"));
        assert_eq!(
            filters,
            SearchFilters {
                tag_contains: Some("metal".into()),
                ..Default::default()
            }
        );
        apply_tag_terms(&mut filters, &[]);
        assert_eq!(filters, SearchFilters::default());
    }

    #[test]
    fn new_tag_fields_are_optional_on_the_wire() {
        let legacy: SearchFilters = serde_json::from_value(serde_json::json!({
            "sample_rate": null, "bit_depth": null, "channels": null,
            "duration_min": null, "duration_max": null, "favorites_only": false,
            "tagged_only": false, "tag_contains": "metal", "color": null
        }))
        .unwrap();
        assert!(legacy.include_tags.is_empty() && legacy.exclude_tags.is_empty());
        let encoded = serde_json::to_value(&legacy).unwrap();
        assert!(encoded.get("include_tags").is_none() && encoded.get("exclude_tags").is_none());
        let mut filters = legacy;
        filters.exclude_tags = vec!["rain".into()];
        assert_eq!(
            serde_json::to_value(&filters).unwrap()["exclude_tags"],
            serde_json::json!(["rain"])
        );
    }

    #[test]
    fn tag_terms_are_validated_with_the_request() {
        let request = |include: Vec<&str>, exclude: Vec<&str>| crate::SearchRequest {
            filters: SearchFilters {
                include_tags: include.into_iter().map(Into::into).collect(),
                exclude_tags: exclude.into_iter().map(Into::into).collect(),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(request(vec!["door"], vec!["rain"]).validate().is_ok());
        assert!(request(vec![], vec![" "]).validate().is_err());
        assert!(request(vec![""], vec![]).validate().is_err());
        assert!(request(vec!["a\0"], vec![]).validate().is_err());
        assert!(
            request(vec!["x".repeat(501).as_str()], vec![])
                .validate()
                .is_err()
        );
        let many: Vec<_> = (0..=crate::MAX_TAG_TERMS).map(|_| "t").collect();
        assert!(request(many, vec![]).validate().is_err());
    }
}
