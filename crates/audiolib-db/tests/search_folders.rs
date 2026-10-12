// crates/audiolib-db/tests/search_folders.rs
//! Directory facets share search predicates and roll up every matching descendant.
use audiolib_core::{
    DataProvider, FileVisibility, MetadataPatch, SearchFilters, SearchRequest, SortOrder,
};
use audiolib_db::SqliteProvider;
use rusqlite::{Connection, params};
use std::collections::BTreeMap;

fn fixture() -> (tempfile::TempDir, SqliteProvider) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("facets.db");
    let provider = SqliteProvider::open(&path).unwrap();
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "INSERT INTO folders(id,folder_path,folder_name,parent_id,total_file_count) VALUES
         (1,'root','root',NULL,999),(2,'root/child','child',1,999),
         (3,'root/child/deep','deep',2,999),(4,'root/wind','wind',1,999),
         (5,'other','other',NULL,999),(6,'empty','empty',NULL,0);
         INSERT INTO folder_closure VALUES
         (1,1,0),(2,2,0),(3,3,0),(4,4,0),(5,5,0),(6,6,0),
         (1,2,1),(1,3,2),(1,4,1),(2,3,1);",
    )
    .unwrap();
    for (id, folder, name, missing) in [
        (1, Some(1), "gore root.wav", false),
        (2, Some(2), "gore child.wav", false),
        (3, Some(2), "gore second.wav", false),
        (4, Some(3), "gore deep.wav", false),
        (5, Some(4), "wind.wav", false),
        (6, Some(5), "gore missing.wav", true),
        (7, None, "gore unfiled.wav", false),
    ] {
        conn.execute(
            "INSERT INTO audio_files(id,file_path,path_key,filename,filename_tokenized,folder_id,
             missing,sample_rate,bit_depth,channels,duration,user_rating,favorite,user_tags,user_color)
             VALUES(?1,?2,?2,?2,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            params![id, name, folder, missing, if id % 2 == 0 { 48000 } else { 96000 },
                if id % 2 == 0 { 24 } else { 16 }, if id % 2 == 0 { 2 } else { 1 },
                f64::from(id), id % 6, id % 2 == 0,
                if id % 2 == 0 { "Metal %_" } else { "" },
                if id % 2 == 0 { "#AABBCC" } else { "" }],
        ).unwrap();
    }
    (temp, provider)
}

fn counts(provider: &SqliteProvider, request: &SearchRequest) -> BTreeMap<i64, u64> {
    provider
        .search_folders(request)
        .unwrap()
        .into_iter()
        .map(|folder| (folder.id, folder.file_count))
        .collect()
}

#[test]
fn directory_counts_ignore_selected_scope_and_pagination_and_hide_zero_matches() {
    let (_temp, provider) = fixture();
    let mut request = SearchRequest {
        query: "gore".into(),
        ..Default::default()
    };
    assert_eq!(
        counts(&provider, &request),
        BTreeMap::from([(1, 4), (2, 3), (3, 1)])
    );
    request.folder_id = Some(4); // A selected zero-hit directory must not hide other choices.
    request.include_subfolders = false;
    request.limit = 1;
    request.offset = 5000;
    request.order_by = SortOrder::SampleRate;
    request.descending = true;
    assert_eq!(provider.search(&request).unwrap().total_count, 0);
    assert_eq!(
        counts(&provider, &request),
        BTreeMap::from([(1, 4), (2, 3), (3, 1)])
    );
    request.query = "no_such_sound".into();
    assert!(counts(&provider, &request).is_empty());
    request.query.clear();
    assert_eq!(
        counts(&provider, &request),
        BTreeMap::from([(1, 5), (2, 3), (3, 1), (4, 1)])
    );
    // Querying facets never rewrites the persisted scan statistics.
    assert_eq!(
        provider
            .folders(None)
            .unwrap()
            .iter()
            .find(|f| f.id == 1)
            .unwrap()
            .file_count,
        999
    );
}

#[test]
fn directory_facets_match_result_counts_for_every_filter_and_visibility() {
    let (_temp, provider) = fixture();
    let filters = [
        SearchFilters::default(),
        SearchFilters {
            visibility: FileVisibility::Missing,
            ..Default::default()
        },
        SearchFilters {
            visibility: FileVisibility::All,
            ..Default::default()
        },
        SearchFilters {
            sample_rate: Some(48000),
            ..Default::default()
        },
        SearchFilters {
            bit_depth: Some(24),
            ..Default::default()
        },
        SearchFilters {
            channels: Some(2),
            ..Default::default()
        },
        SearchFilters {
            duration_min: Some(2.0),
            duration_max: Some(4.0),
            ..Default::default()
        },
        SearchFilters {
            favorites_only: true,
            ..Default::default()
        },
        SearchFilters {
            tagged_only: true,
            ..Default::default()
        },
        SearchFilters {
            tag_contains: Some("METAL %_".into()),
            ..Default::default()
        },
        SearchFilters {
            color: Some("#aabbcc".into()),
            ..Default::default()
        },
        SearchFilters {
            sample_rate: Some(48000),
            bit_depth: Some(24),
            channels: Some(2),
            duration_min: Some(0.0),
            duration_max: Some(4.0),
            favorites_only: true,
            tagged_only: true,
            tag_contains: Some("%_".into()),
            color: Some("#aabbcc".into()),
            ..Default::default()
        },
    ];
    for query in ["", "gore", "go*", "gore deep"] {
        for filter in &filters {
            let mut request = SearchRequest {
                query: query.into(),
                filters: filter.clone(),
                ..Default::default()
            };
            let actual = counts(&provider, &request);
            let mut expected = BTreeMap::new();
            for folder_id in 1..=6 {
                request.folder_id = Some(folder_id);
                let total = provider.search(&request).unwrap().total_count;
                if total > 0 {
                    expected.insert(folder_id, total);
                }
            }
            assert_eq!(actual, expected, "{request:?}");
        }
    }
}

#[test]
fn directory_facets_refresh_chinese_annotations_and_validate_requests() {
    let (_temp, provider) = fixture();
    let mut request = SearchRequest {
        query: "爆炸".into(),
        ..Default::default()
    };
    assert!(counts(&provider, &request).is_empty());
    provider
        .update_metadata(
            4,
            &MetadataPatch {
                tags: Some("爆炸".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        counts(&provider, &request),
        BTreeMap::from([(1, 1), (2, 1), (3, 1)])
    );
    provider
        .update_metadata(
            4,
            &MetadataPatch {
                tags: Some("风声".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(counts(&provider, &request).is_empty());
    request.query = "x".repeat(501);
    assert!(provider.search_folders(&request).is_err());
    request.query.clear();
    request.filters.duration_min = Some(f64::NAN);
    assert!(provider.search_folders(&request).is_err());
}
