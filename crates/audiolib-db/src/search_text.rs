// crates/audiolib-db/src/search_text.rs
//! Shared Chinese segmentation and Unicode word boundaries for FTS5 input.
use audiolib_core::{Error, Result};
use jieba_rs::Jieba;
use unicode_segmentation::UnicodeSegmentation;

// Split ideographic runs before applying Unicode word boundaries: UAX #29 gives
// individual Han characters, whereas the audio dictionary must keep Chinese words.
fn is_han(character: char) -> bool {
    matches!(character,
        '\u{3400}'..='\u{4dbf}'
        | '\u{4e00}'..='\u{9fff}'
        | '\u{f900}'..='\u{faff}'
        | '\u{20000}'..='\u{2ffff}'
        | '\u{30000}'..='\u{323af}'
    )
}

fn words<'a>(jieba: &Jieba, value: &'a str, for_search: bool) -> Vec<&'a str> {
    let mut words = Vec::new();
    let mut start = 0;
    let mut han = value.chars().next().is_some_and(is_han);
    let mut append = |part: &'a str, han: bool| {
        if han {
            let tokens = if for_search {
                jieba.cut_for_search(part, true)
            } else {
                jieba.cut(part, true)
            };
            words.extend(tokens.into_iter().map(|token| token.word));
        } else {
            // Keep accented letters and combining marks in their Unicode word.
            // SQLite's unicode61 tokenizer performs the case/diacritic folding;
            // splitting café into caf + é would change both recall and AND semantics.
            words.extend(part.unicode_words());
        }
    };
    for (offset, character) in value.char_indices() {
        let next_han = is_han(character);
        if next_han != han {
            append(&value[start..offset], han);
            start = offset;
            han = next_han;
        }
    }
    append(&value[start..], han);
    words
}

/// Derived search text uses the same Unicode word boundaries as query compilation.
/// Chinese search-mode expansion retains the existing dictionary subword recall.
pub(crate) fn index_text(jieba: &Jieba, value: &str) -> String {
    words(jieba, value, true).join(" ")
}

fn query_word_parts(word: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    for (offset, grapheme) in word.grapheme_indices(true) {
        // UAX word boundaries keep internal colons, apostrophes and numeric
        // commas, while unicode61 splits them. Quoting the complete UAX word
        // would accidentally require adjacent FTS tokens instead of AND terms.
        // Keep letter/number graphemes intact, including their combining marks.
        // ASCII dot/underscore retain the existing compound/decimal phrases.
        if matches!(grapheme, "." | "_") || grapheme.chars().any(char::is_alphanumeric) {
            continue;
        }
        if start < offset {
            parts.push(&word[start..offset]);
        }
        start = offset + grapheme.len();
    }
    if start < word.len() {
        parts.push(&word[start..]);
    }
    parts
}

/// Compile a plain-text AND query; user punctuation never becomes an FTS operator.
pub(crate) fn compile_query(jieba: &Jieba, value: &str) -> Result<String> {
    let terms: Vec<_> = words(jieba, value, false)
        .into_iter()
        .flat_map(query_word_parts)
        .filter(|word| word.chars().any(char::is_alphanumeric))
        .map(|word| format!("\"{}\"", word.replace('"', "\"\"")))
        .collect();
    if terms.is_empty() {
        return Err(Error::Validation("Search requires a word or number".into()));
    }
    Ok(terms.join(" AND "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_words_keep_accents_and_combining_marks() {
        let jieba = Jieba::new();
        for word in ["café", "Cafe\u{301}", "naïve", "Äpfel", "Ελληνικά"] {
            assert_eq!(compile_query(&jieba, word).unwrap(), format!("\"{word}\""));
            assert_eq!(index_text(&jieba, word), word);
        }
    }

    #[test]
    fn punctuation_and_operator_names_remain_plain_and_terms() {
        let jieba = Jieba::new();
        assert_eq!(
            compile_query(&jieba, "metal (OR) \"wood\"*").unwrap(),
            "\"metal\" AND \"OR\" AND \"wood\""
        );
        assert!(compile_query(&jieba, "\" () *").is_err());
        assert_eq!(
            compile_query(&jieba, "door,slam").unwrap(),
            "\"door\" AND \"slam\""
        );
    }

    #[test]
    fn chinese_dictionary_words_and_search_expansions_are_retained() {
        let jieba = crate::load_tokenizer();
        assert_eq!(
            compile_query(&jieba, "café爆炸声音").unwrap(),
            "\"café\" AND \"爆炸\" AND \"声音\""
        );
        let expected = jieba
            .cut_for_search("数字音频工作站", true)
            .into_iter()
            .map(|token| token.word)
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(index_text(&jieba, "数字音频工作站"), expected);
    }

    #[test]
    fn internal_uax_punctuation_keeps_and_terms_and_legacy_compound_phrases() {
        let jieba = Jieba::new();
        for query in ["foo:bar", "foo·bar", "foo：bar", "foo‿bar"] {
            assert_eq!(compile_query(&jieba, query).unwrap(), "\"foo\" AND \"bar\"");
        }
        for query in ["don't", "don’t", "don‘t", "don＇t"] {
            assert_eq!(compile_query(&jieba, query).unwrap(), "\"don\" AND \"t\"");
        }
        assert_eq!(compile_query(&jieba, "1,2").unwrap(), "\"1\" AND \"2\"");
        assert_eq!(
            compile_query(&jieba, "Cafe\u{301}:door").unwrap(),
            "\"Cafe\u{301}\" AND \"door\""
        );
        for query in ["foo.bar", "foo_bar", "1.2"] {
            assert_eq!(
                compile_query(&jieba, query).unwrap(),
                format!("\"{query}\"")
            );
        }
        // This is query compilation only; the stored token revision is unchanged.
        assert_eq!(index_text(&jieba, "don't"), "don't");
        assert_eq!(index_text(&jieba, "Cafe\u{301}:door"), "Cafe\u{301}:door");
    }
}
