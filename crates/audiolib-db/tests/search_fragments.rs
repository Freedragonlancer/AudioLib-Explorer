// crates/audiolib-db/tests/search_fragments.rs
//! Letter-fragment searches use the same predicates for pages, caches and facets.
use audiolib_core::{
    DataProvider, FileVisibility, MaintenanceAction, MetadataPatch, SearchFilters, SearchRequest,
    SortOrder,
};
use audiolib_db::SqliteProvider;
use rusqlite::{Connection, params};
use std::collections::BTreeSet;

fn fixture() -> (tempfile::TempDir, SqliteProvider) {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("fragments.db");
    let provider = SqliteProvider::open(&database).unwrap();
    let conn = Connection::open(&database).unwrap();
    conn.execute_batch(
        "INSERT INTO folders(id,folder_path,folder_name) VALUES(1,'root','root');
         INSERT INTO folder_closure VALUES(1,1,0);",
    )
    .unwrap();
    for (id, name, tags, notes, description, scene, take, missing) in [
        (1, "explosion.wav", "metal", "", "", "", "", 0),
        (2, "tags.wav", "explosion", "", "", "", "", 0),
        (3, "notes.wav", "", "explosion", "", "", "", 0),
        (4, "source.wav", "", "", "explosion", "", "", 0),
        (5, "scene.wav", "", "", "", "explosion", "", 0),
        (6, "take.wav", "", "", "", "", "explosion", 0),
        (7, "missing.wav", "explosion", "", "", "", "", 1),
        (8, "unrelated.wav", "wood", "", "", "", "", 0),
    ] {
        conn.execute(
            "INSERT INTO audio_files(id,file_path,path_key,filename,user_tags,user_notes,
             bwf_description,ixml_scene,ixml_take,folder_id,missing)
             VALUES(?1,?2,?2,?3,?4,?5,?6,?7,?8,1,?9)",
            params![
                id,
                format!("root/{name}"),
                name,
                tags,
                notes,
                description,
                scene,
                take,
                missing
            ],
        )
        .unwrap();
    }
    provider
        .maintain_library(MaintenanceAction::RebuildSearch)
        .unwrap();
    (directory, provider)
}

fn ids(provider: &SqliteProvider, request: &SearchRequest) -> BTreeSet<i64> {
    provider
        .search(request)
        .unwrap()
        .results
        .into_iter()
        .map(|file| file.id)
        .collect()
}

#[test]
fn every_letter_fragment_matches_filename_and_all_metadata_fields() {
    let (_directory, provider) = fixture();
    let expected = BTreeSet::from([1, 2, 3, 4, 5, 6]);
    for (start, end) in (0..8).flat_map(|start| (start + 2..=9).map(move |end| (start, end))) {
        for query in [&"explosion"[start..end], &"EXPLOSION"[start..end]] {
            let request = SearchRequest {
                query: query.into(),
                ..Default::default()
            };
            // Repeated searches exercise the ranking cache as well as initial SQL.
            for _ in 0..2 {
                let page = provider.search(&request).unwrap();
                assert_eq!(page.total_count, 6, "{query}");
                assert_eq!(ids(&provider, &request), expected, "{query}");
            }
            let folders = provider.search_folders(&request).unwrap();
            assert_eq!(folders.len(), 1);
            assert_eq!(folders[0].file_count, 6, "{query}");
        }
    }
    for query in ["explosi*", "\"expl\"", "(expl)"] {
        assert_eq!(
            ids(
                &provider,
                &SearchRequest {
                    query: query.into(),
                    ..Default::default()
                }
            ),
            expected,
            "{query}"
        );
    }
    for query in [
        "losp",
        "explosia",
        "g000065000078",
        "expl OR wood",
        "expl NOT wood",
        "expl nonexistent",
    ] {
        assert!(
            ids(
                &provider,
                &SearchRequest {
                    query: query.into(),
                    ..Default::default()
                }
            )
            .is_empty(),
            "{query}"
        );
    }
}

#[test]
fn fragment_filters_sorting_pagination_and_metadata_invalidation_agree() {
    let (_directory, provider) = fixture();
    let request = SearchRequest {
        query: "plos eta".into(),
        ..Default::default()
    };
    assert_eq!(ids(&provider, &request), BTreeSet::from([1]));
    provider
        .update_metadata(
            2,
            &MetadataPatch {
                tags: Some("explosion metallic".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(ids(&provider, &request), BTreeSet::from([1, 2]));
    provider
        .update_metadata(
            2,
            &MetadataPatch {
                tags: Some("wood".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(ids(&provider, &request), BTreeSet::from([1]));
    let request = SearchRequest {
        query: "sion".into(),
        folder_id: Some(1),
        filters: SearchFilters {
            visibility: FileVisibility::All,
            ..Default::default()
        },
        order_by: SortOrder::Filename,
        limit: 2,
        offset: 1,
        ..Default::default()
    };
    let page = provider.search(&request).unwrap();
    assert_eq!(page.total_count, 6);
    assert_eq!(
        page.results.iter().map(|file| file.id).collect::<Vec<_>>(),
        vec![7, 3]
    );
    assert_eq!(provider.search_folders(&request).unwrap()[0].file_count, 6);
    let readonly = SqliteProvider::open_read_only(provider.database_path()).unwrap();
    assert_eq!(ids(&readonly, &request), BTreeSet::from([7, 3]));
}

#[test]
fn numeric_queries_keep_exact_tokens_while_letter_number_fragments_match() {
    let (_directory, provider) = fixture();
    provider
        .update_metadata(
            1,
            &MetadataPatch {
                tags: Some("123 A123".into()),
                ..Default::default()
            },
        )
        .unwrap();
    for query in ["1", "12"] {
        assert!(
            ids(
                &provider,
                &SearchRequest {
                    query: query.into(),
                    ..Default::default()
                }
            )
            .is_empty()
        );
    }
    for query in ["123", "a1", "A12"] {
        assert_eq!(
            ids(
                &provider,
                &SearchRequest {
                    query: query.into(),
                    ..Default::default()
                }
            ),
            BTreeSet::from([1])
        );
    }
}

#[test]
fn single_letter_codes_in_multi_term_queries_include_extracted_source_metadata() {
    let (_directory, provider) = fixture();
    let conn = Connection::open(provider.database_path()).unwrap();
    conn.execute(
        "UPDATE audio_files SET metadata_json=?1 WHERE id=1",
        [r#"{"wav_metadata":{"fields":{"keywords":"Z"}}}"#],
    )
    .unwrap();
    drop(conn);
    provider
        .maintain_library(MaintenanceAction::RebuildSearch)
        .unwrap();
    for query in ["z explosion", "Z plos"] {
        assert_eq!(
            ids(
                &provider,
                &SearchRequest {
                    query: query.into(),
                    ..Default::default()
                }
            ),
            BTreeSet::from([1])
        );
    }
    assert!(
        provider
            .search(&SearchRequest {
                query: "z".into(),
                ..Default::default()
            })
            .is_err()
    );
}

#[test]
fn single_grapheme_fold_expansions_keep_fragments_exact_terms_and_word_boundaries() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("fold-expansions.db");
    let provider = SqliteProvider::open(&database).unwrap();
    let conn = Connection::open(&database).unwrap();
    conn.execute_batch(
        "INSERT INTO folders(id,folder_path,folder_name) VALUES(1,'root','root');
         INSERT INTO folder_closure VALUES(1,1,0);",
    )
    .unwrap();
    for (id, name, tags, source) in [
        (1, "a.wav", "ß", ""),
        (2, "b.wav", "ﬃ", ""),
        (3, "c.wav", "ss ffi", ""),
        (4, "d.wav", "f f i s s", ""),
        (5, "e.wav", "", "ß ﬃ"),
        (6, "h.wav", "ﬃ ß", ""),
    ] {
        let metadata = serde_json::json!({
            "wav_metadata": {"fields": {"keywords": source}},
            "private": {"retain": true},
        });
        conn.execute(
            "INSERT INTO audio_files(id,file_path,filename,folder_id,user_tags,
             user_notes,metadata_json) VALUES(?1,?2,?3,1,?4,'anchor',?5)",
            params![id, format!("root/{name}"), name, tags, metadata.to_string()],
        )
        .unwrap();
    }
    drop(conn);
    provider
        .maintain_library(MaintenanceAction::RebuildSearch)
        .unwrap();
    for rebuild in [false, true] {
        if rebuild {
            provider
                .maintain_library(MaintenanceAction::RebuildSearch)
                .unwrap();
        }
        for (queries, expected) in [
            (vec!["ss", "SS"], BTreeSet::from([1, 3, 5, 6])),
            (
                vec!["ff", "FF", "fi", "ffi", "FFI"],
                BTreeSet::from([2, 3, 5, 6]),
            ),
            (vec!["ß anchor", "ẞ anchor"], BTreeSet::from([1, 5, 6])),
            (vec!["ﬃ anchor"], BTreeSet::from([2, 5, 6])),
            (vec!["s anchor"], BTreeSet::from([4])),
        ] {
            for query in queries {
                let request = SearchRequest {
                    query: query.into(),
                    ..Default::default()
                };
                for _ in 0..2 {
                    assert_eq!(ids(&provider, &request), expected, "{query}");
                }
                assert_eq!(
                    provider.search_folders(&request).unwrap()[0].file_count,
                    expected.len() as u64,
                    "{query}"
                );
                assert_eq!(
                    ids(
                        &provider,
                        &SearchRequest {
                            order_by: SortOrder::Filename,
                            ..request
                        }
                    ),
                    expected,
                    "{query}"
                );
            }
        }
        // These would exist only if the folded letters or their secondary exact
        // tokens could concatenate across the original source word boundaries.
        for query in ["ssffi", "ffiss", "sf", "is"] {
            assert!(
                ids(
                    &provider,
                    &SearchRequest {
                        query: query.into(),
                        ..Default::default()
                    }
                )
                .is_empty(),
                "{query}"
            );
        }
        for query in ["ß", "ẞ", "ﬃ", "f"] {
            assert!(
                provider
                    .search(&SearchRequest {
                        query: query.into(),
                        ..Default::default()
                    })
                    .is_err(),
                "{query}"
            );
        }
        let readonly = SqliteProvider::open_read_only(&database).unwrap();
        assert_eq!(
            ids(
                &readonly,
                &SearchRequest {
                    query: "fi".into(),
                    ..Default::default()
                }
            ),
            BTreeSet::from([2, 3, 5, 6])
        );
    }
    let source = provider.get_audio(5).unwrap();
    assert!(source.tags.is_empty());
    assert_eq!(
        source.attributes["wav_metadata"]["fields"]["keywords"],
        "ß ﬃ"
    );
    assert_eq!(source.attributes["private"]["retain"], true);
}
