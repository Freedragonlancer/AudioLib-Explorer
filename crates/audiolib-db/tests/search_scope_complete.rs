// crates/audiolib-db/tests/search_scope_complete.rs
//! Provider regressions for scope membership, filtered directory search, ordering and facets.
use audiolib_core::{
    DataProvider, MetadataPatch, SearchFilters, SearchRequest, SearchScope, SortOrder,
};
use audiolib_db::SqliteProvider;
use rusqlite::{Connection, params};
use std::collections::{BTreeMap, BTreeSet};

fn fixture() -> (tempfile::TempDir, SqliteProvider) {
    let temp = tempfile::tempdir().unwrap();
    let database = temp.path().join("scope.db");
    let provider = SqliteProvider::open(&database).unwrap();
    let conn = Connection::open(database).unwrap();
    audiolib_db::register_library_functions(&conn).unwrap();
    for (id, path, name, parent) in [
        (1, "/library-root", "library-root", None),
        (2, "/library-root/Corridor", "Corridor", Some(1)),
        (3, "/library-root/Corridor/Metal", "Metal", Some(2)),
        (4, "/library-root/Forest", "Forest", Some(1)),
    ] {
        conn.execute(
            "INSERT INTO folders(id,folder_path,folder_name,parent_id) VALUES(?1,?2,?3,?4)",
            params![id, path, name, parent],
        )
        .unwrap();
    }
    conn.execute_batch("INSERT INTO folder_closure VALUES(1,1,0),(1,2,1),(1,3,2),(1,4,1),(2,2,0),(2,3,1),(3,3,0),(4,4,0);").unwrap();
    for id in 1..=12i64 {
        let (filename, notes) = match id {
            1 => ("z_shutter_中文42.wav".into(), "noteword"),
            2 => ("a_second.wav".into(), "shutter"),
            3 => ("m_third.wav".into(), ""),
            4 => ("corridor_shutter.wav".into(), ""),
            5 => ("shutter.wav".into(), ""),
            6 => ("quiet.wav".into(), "corridor metal"),
            7 => ("neutral.wav".into(), "metadataonly 中文42"),
            _ => (format!("filler{id}.wav"), ""),
        };
        let folder = if id <= 3 { 3 } else { 4 };
        let base = if folder == 3 {
            "/library-root/Corridor/Metal"
        } else {
            "/library-root/Forest"
        };
        let album = if matches!(id, 1 | 3 | 4) {
            "Album A"
        } else {
            "Album B"
        };
        let metadata = serde_json::json!({"wav_metadata":{"fields":{"library":album}}});
        let duration = match id {
            1 => 3.0,
            2 => 1.0,
            3 => 2.0,
            _ => id as f64,
        };
        conn.execute("INSERT INTO audio_files(id,file_path,path_key,filename,folder_id,user_notes,user_tags,favorite,sample_rate,bit_depth,channels,duration,metadata_json) VALUES(?1,?2,?2,?3,?4,?5,?6,?7,?8,24,2,?9,?10)",params![id,format!("{base}/{filename}"),filename,folder,notes,if matches!(id,1|3){"keep"}else{"other"},matches!(id,1|3|4),if id==2{96000}else{48000},duration,metadata.to_string()]).unwrap();
    }
    // Corridor/Metal contain only 3 of 12 files, so the deliberate >50% path fallback
    // does not mask any membership expectation in this fixture.
    provider.rebuild_directory_search().unwrap();
    (temp, provider)
}

fn request(scope: SearchScope, query: &str) -> SearchRequest {
    SearchRequest {
        scope,
        query: query.into(),
        ..Default::default()
    }
}
fn ids(provider: &SqliteProvider, request: &SearchRequest) -> Vec<i64> {
    provider
        .search(request)
        .unwrap()
        .results
        .into_iter()
        .map(|f| f.id)
        .collect()
}
fn set(provider: &SqliteProvider, request: &SearchRequest) -> BTreeSet<i64> {
    ids(provider, request).into_iter().collect()
}

#[test]
fn optimized_cursor_binds_query_scope_visibility_revision_and_provider_session() {
    let (temp, provider) = fixture();
    let mut original = request(SearchScope::Path, "corridor");
    original.limit = 1;
    let page = provider.search(&original).unwrap();
    let cursor = page.next_cursor.unwrap();
    assert!(cursor.len() <= 512);
    for mut changed in [
        SearchRequest {
            query: "metal".into(),
            ..original.clone()
        },
        SearchRequest {
            scope: SearchScope::All,
            ..original.clone()
        },
        SearchRequest {
            filters: SearchFilters {
                visibility: audiolib_core::library::FileVisibility::All,
                ..original.filters.clone()
            },
            ..original.clone()
        },
    ] {
        changed.cursor = Some(cursor.clone());
        let error = provider.search(&changed).unwrap_err().to_string();
        assert!(error.contains("CURSOR_STALE"), "{error}");
    }
    original.cursor = Some(cursor.clone());
    let other = SqliteProvider::open_existing(&temp.path().join("scope.db")).unwrap();
    assert!(
        other
            .search(&original)
            .unwrap_err()
            .to_string()
            .contains("CURSOR_STALE")
    );
    provider
        .update_metadata(
            1,
            &MetadataPatch {
                notes: Some("new annotation".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(
        provider
            .search(&original)
            .unwrap_err()
            .to_string()
            .contains("CURSOR_STALE")
    );
}

#[test]
fn optimized_cursor_preserves_global_position_and_reports_count_accuracy() {
    let (_temp, provider) = fixture();
    let mut selection = request(SearchScope::Path, "corridor");
    selection.limit = 1;
    let mut all = Vec::new();
    for index in 0..3 {
        let page = provider.search(&selection).unwrap();
        all.extend(page.results.iter().map(|file| file.id));
        assert_eq!(page.total_count_exact, index == 2);
        assert_eq!(page.total_count, if index == 0 { 2 } else { 3 });
        selection.cursor = page.next_cursor;
        // Cursor position wins over a caller's unrelated offset.
        selection.offset = 900;
    }
    assert_eq!(all, vec![1, 2, 3]);
    assert!(selection.cursor.is_none());
    let page = provider.search(&selection).unwrap();
    assert!(page.results.is_empty());
    assert!(!page.total_count_exact);
    assert_eq!(
        page.total_count, 0,
        "an unproven deep offset is not a lower bound"
    );
}

#[test]
fn filename_scope_excludes_notes_for_letters_han_and_numeric_terms() {
    let (_temp, provider) = fixture();
    assert_eq!(
        set(&provider, &request(SearchScope::Main, "noteword")),
        BTreeSet::from([1])
    );
    assert!(ids(&provider, &request(SearchScope::Filename, "noteword")).is_empty());
    for query in ["中文", "42"] {
        assert_eq!(
            set(&provider, &request(SearchScope::Main, query)),
            BTreeSet::from([1, 7]),
            "main: {query}"
        );
        assert_eq!(
            set(&provider, &request(SearchScope::Filename, query)),
            BTreeSet::from([1]),
            "filename: {query}"
        );
    }
    assert_eq!(
        set(&provider, &request(SearchScope::Main, "shutter")),
        BTreeSet::from([1, 2, 4, 5])
    );
    assert_eq!(
        set(&provider, &request(SearchScope::Filename, "shutter")),
        BTreeSet::from([1, 4, 5])
    );
}

#[test]
fn every_path_term_must_be_in_directories_while_all_supports_mixed_sources() {
    let (_temp, provider) = fixture();
    assert_eq!(
        set(&provider, &request(SearchScope::Path, "corridor metal")),
        BTreeSet::from([1, 2, 3])
    );
    assert!(ids(&provider, &request(SearchScope::Path, "metal shutter")).is_empty());
    assert_eq!(
        set(&provider, &request(SearchScope::All, "metal shutter")),
        BTreeSet::from([1, 2])
    );
    assert_eq!(
        set(&provider, &request(SearchScope::All, "corridor shutter")),
        BTreeSet::from([1, 2, 4])
    );
    assert!(
        ids(&provider, &request(SearchScope::Path, "library-root")).is_empty(),
        "the configured root name is excluded"
    );
}

#[test]
fn path_membership_accepts_folder_album_favorite_and_audio_filters_together() {
    let (_temp, provider) = fixture();
    let mut selected = request(SearchScope::Path, "corridor");
    selected.folder_id = Some(2);
    selected.include_subfolders = true;
    selected.filters = SearchFilters {
        favorites_only: true,
        sample_rate: Some(48000),
        album: Some("Album A".into()),
        include_tags: vec!["keep".into()],
        ..Default::default()
    };
    assert_eq!(set(&provider, &selected), BTreeSet::from([1, 3]));
    let album = provider
        .album_facets(&request(SearchScope::Path, "corridor"))
        .unwrap()
        .into_iter()
        .find(|f| f.album.name == "Album A")
        .unwrap()
        .album;
    selected.filters.album = None;
    selected.filters.album_id = Some(album.id);
    assert_eq!(set(&provider, &selected), BTreeSet::from([1, 3]));
    selected.filters.sample_rate = Some(96000);
    assert!(ids(&provider, &selected).is_empty());
    selected.filters.sample_rate = Some(48000);
    selected.include_subfolders = false;
    assert!(ids(&provider, &selected).is_empty());
}

#[test]
fn directory_search_honors_nonrelevance_sort_in_both_directions() {
    let (_temp, provider) = fixture();
    for order in [SortOrder::Filename, SortOrder::Duration] {
        let mut selected = request(SearchScope::Path, "corridor");
        selected.order_by = order;
        assert_eq!(ids(&provider, &selected), vec![2, 3, 1]);
        selected.descending = true;
        assert_eq!(ids(&provider, &selected), vec![1, 3, 2]);
    }
}

#[test]
fn filtered_path_keyset_pages_preserve_order_and_reject_changed_requests_and_revisions() {
    let (_temp, provider) = fixture();
    let mut selected = request(SearchScope::Path, "corridor");
    selected.filters.favorites_only = true;
    selected.order_by = SortOrder::Filename;
    selected.limit = 1;
    let first = provider.search(&selected).unwrap();
    assert_eq!(first.results[0].id, 3);
    assert_eq!(first.total_count, 2);
    selected.cursor = first.next_cursor.clone();
    selected.offset = 999;
    selected.page = Some(999);
    let second = provider.search(&selected).unwrap();
    assert_eq!(second.results[0].id, 1);
    assert!(second.next_cursor.is_none());
    let mut changed = selected.clone();
    changed.filters.sample_rate = Some(96000);
    assert!(
        provider
            .search(&changed)
            .unwrap_err()
            .to_string()
            .contains("CURSOR_STALE")
    );
    provider
        .update_metadata(
            1,
            &MetadataPatch {
                notes: Some("new revision".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(
        provider
            .search(&selected)
            .unwrap_err()
            .to_string()
            .contains("CURSOR_STALE")
    );
}

#[test]
fn folder_and_album_facets_use_the_same_scoped_membership_as_result_rows() {
    let (_temp, provider) = fixture();
    for scope in [
        SearchScope::Main,
        SearchScope::Filename,
        SearchScope::All,
        SearchScope::Path,
    ] {
        let query = if scope == SearchScope::Path {
            "corridor"
        } else {
            "shutter"
        };
        let mut selected = request(scope, query);
        selected.filters.sample_rate = Some(48000);
        let matches = provider.search(&selected).unwrap();
        let mut expected_folders = BTreeMap::<i64, u64>::new();
        let mut expected_albums = BTreeMap::<i64, u64>::new();
        for file in &matches.results {
            let leaf = file.folder_id.unwrap();
            *expected_folders.entry(leaf).or_default() += 1;
            *expected_albums.entry(file.album_id.unwrap()).or_default() += 1;
        }
        let folders = provider.search_folders(&selected).unwrap();
        for (folder, count) in expected_folders {
            assert_eq!(
                folders.iter().find(|f| f.id == folder).unwrap().file_count,
                count,
                "{scope:?}"
            );
        }
        let actual = provider
            .album_facets(&selected)
            .unwrap()
            .into_iter()
            .map(|facet| (facet.album.id, facet.matching_count))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(actual, expected_albums, "{scope:?}");
    }
}

#[test]
fn sparse_ids_null_fields_and_missing_rows_keep_fast_all_generic_and_facets_consistent() {
    let (temp, provider) = fixture();
    let conn = Connection::open(temp.path().join("scope.db")).unwrap();
    audiolib_db::register_library_functions(&conn).unwrap();
    for (id, folder, missing) in [
        (100, Some(3), false),
        (10000, Some(3), true),
        (100000, None, false),
    ] {
        conn.execute("INSERT INTO audio_files(id,file_path,path_key,filename,folder_id,missing,user_notes,user_tags,duration,sample_rate,bit_depth,channels,metadata_json) VALUES(?1,?2,?2,?3,?4,?5,NULL,NULL,NULL,NULL,24,2,?6)",params![id,format!("/library-root/Corridor/Metal/sparse{id}.wav"),format!("corridor_shutter_sparse{id}.wav"),folder,missing,serde_json::json!({"wav_metadata":{"fields":{"library":"Album A"}}}).to_string()]).unwrap();
    }
    provider.rebuild_directory_search().unwrap();
    let fast = request(SearchScope::All, "corridor shutter");
    let expected = set(&provider, &fast);
    assert_eq!(expected, BTreeSet::from([1, 2, 4, 100, 100000]));
    assert!(!expected.contains(&10000));
    let mut generic = fast.clone();
    generic.filters.bit_depth = Some(24);
    assert_eq!(
        set(&provider, &generic),
        expected,
        "a lossless filter must not change membership"
    );
    let mut sorted = fast.clone();
    sorted.order_by = SortOrder::Filename;
    assert_eq!(set(&provider, &sorted), expected);
    let expected_folders = provider
        .search(&fast)
        .unwrap()
        .results
        .iter()
        .filter_map(|file| file.folder_id)
        .fold(BTreeMap::<i64, u64>::new(), |mut counts, id| {
            *counts.entry(id).or_default() += 1;
            counts
        });
    for selection in [&fast, &generic, &sorted] {
        let folders = provider.search_folders(selection).unwrap();
        for (id, count) in &expected_folders {
            assert_eq!(
                folders.iter().find(|f| f.id == *id).unwrap().file_count,
                *count
            );
        }
        let from_rows = provider
            .search(selection)
            .unwrap()
            .results
            .into_iter()
            .fold(BTreeMap::<i64, u64>::new(), |mut counts, file| {
                *counts.entry(file.album_id.unwrap()).or_default() += 1;
                counts
            });
        let facets = provider
            .album_facets(selection)
            .unwrap()
            .into_iter()
            .map(|facet| (facet.album.id, facet.matching_count))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(facets, from_rows);
    }
}
