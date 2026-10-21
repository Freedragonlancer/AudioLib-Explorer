// crates/audiolib-db/tests/search_sorted_tags.rs
//! Broad tag ordering agrees with native keys across optional-index fallbacks.
use audiolib_core::{DataProvider, FileVisibility, SearchFilters, SearchRequest, SortOrder};
use audiolib_db::{SqliteProvider, import_legacy, register_library_functions};
use rusqlite::{Connection, params};

const RECORDS: usize = 16_384;

fn fixture(nullable_source_tags: bool) -> (tempfile::TempDir, SqliteProvider, Connection) {
    fixture_with_source_column(if nullable_source_tags {
        ",source_tags_text TEXT"
    } else {
        ""
    })
}

fn fixture_with_source_column(
    source_column: &str,
) -> (tempfile::TempDir, SqliteProvider, Connection) {
    let directory = tempfile::tempdir().unwrap();
    let legacy = directory.path().join("legacy.db");
    let path = directory.path().join("tags.db");
    // A supported import retains nullable/nonbinary legacy visibility. The
    // normal source-tags column is added with its current NOT NULL contract.
    let source = Connection::open(&legacy).unwrap();
    source
        .execute_batch(&format!(
            "CREATE TABLE audio_files(id INTEGER PRIMARY KEY,file_path TEXT NOT NULL UNIQUE,
         filename TEXT NOT NULL,user_tags TEXT,missing INTEGER{source_column});
         CREATE TABLE folders(id INTEGER PRIMARY KEY,folder_path TEXT NOT NULL UNIQUE,
         folder_name TEXT NOT NULL,parent_id INTEGER);"
        ))
        .unwrap();
    drop(source);
    import_legacy(&legacy, &path).unwrap();
    let provider = SqliteProvider::open_existing(&path).unwrap();
    let conn = Connection::open(path).unwrap();
    register_library_functions(&conn).unwrap();
    let mut insert = conn.prepare(
        "INSERT INTO audio_files(id,file_path,filename,user_tags,missing,metadata_json,source_tags_text)
         VALUES(?1,?2,'metal.wav',?3,0,?4,audiolib_source_tags_key(?4))"
    ).unwrap();
    conn.execute_batch("BEGIN").unwrap();
    let annotations = [
        None,
        Some(""),
        Some("   "),
        Some("\u{3000}\u{a0}\t\r\n"),
        Some("  Straße\u{3000}"),
        Some("STRASSE"),
        Some("Écho"),
        Some("E\u{301}CHO"),
        Some("Zulu"),
        None,
        Some(" \t "),
        Some("\u{a0}\u{2028}"),
    ];
    let source_tags = [
        "", "Alpha", "Zulu", "Beta", "ignored", "ignored", "Z", "Z", "aardvark", "café", "omega",
        "",
    ];
    let ids = (1..=RECORDS as i64).chain([i64::MIN, -7, 0, i64::MAX]);
    for (index, id) in ids.enumerate() {
        let kind = if id <= 0 || id == i64::MAX {
            0
        } else {
            index % annotations.len()
        };
        let metadata = serde_json::json!({
            "private": {"retained_id":id},
            "wav_metadata":{"fields":{"category":source_tags[kind]}}
        })
        .to_string();
        insert
            .execute(params![
                id,
                format!("root/{id}.wav"),
                annotations[kind],
                metadata
            ])
            .unwrap();
    }
    drop(insert);
    conn.execute_batch(
        "COMMIT;
        INSERT INTO audio_files(id,file_path,filename,missing,source_tags_text)
        VALUES(20000,'root/unmatched.wav','wind.wav',0,'not in MATCH');",
    )
    .unwrap();
    (directory, provider, conn)
}

fn assert_page(provider: &SqliteProvider, conn: &Connection, request: &SearchRequest) {
    let visibility = match request.filters.visibility {
        FileVisibility::Present => "af.missing=0",
        FileVisibility::Missing => "af.missing=1",
        FileVisibility::All => "1=1",
    };
    // Every fixture match has the complete raw token metal. Bypass both the
    // fragment compiler and the optimized page strategy in this native oracle.
    let from = format!(
        "FROM audio_search CROSS JOIN audio_files af ON af.id=audio_search.rowid
        WHERE audio_search MATCH 'metal' AND {visibility}"
    );
    let total = u64::try_from(
        conn.query_row(&format!("SELECT COUNT(*) {from}"), [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap(),
    )
    .unwrap();
    let ids = if request.offset as u64 >= total {
        Vec::new()
    } else {
        let direction = if request.descending { "DESC" } else { "ASC" };
        conn.prepare(&format!(
            "SELECT af.id {from}
            ORDER BY COALESCE(audiolib_user_tags_key(af.user_tags),af.source_tags_text)
            COLLATE NOCASE {direction},af.id ASC LIMIT ?1 OFFSET ?2"
        ))
        .unwrap()
        .query_map(params![request.limit as i64, request.offset as i64], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
    };
    let page = provider.search(request).unwrap();
    assert_eq!(page.total_count, total, "{request:?}");
    assert_eq!(page.returned_count, ids.len(), "{request:?}");
    assert_eq!(
        page.results.iter().map(|f| f.id).collect::<Vec<_>>(),
        ids,
        "{request:?}"
    );
    for file in page.results {
        assert_eq!(file.attributes["private"]["retained_id"], file.id);
    }
}

fn request(descending: bool, offset: usize, visibility: FileVisibility) -> SearchRequest {
    SearchRequest {
        query: "metal".into(),
        order_by: SortOrder::Tags,
        descending,
        offset,
        limit: 7,
        filters: SearchFilters {
            visibility,
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn broad_tag_pages_preserve_native_unicode_blank_fallback_ties_and_visibility() {
    let (_directory, provider, conn) = fixture(false);
    for descending in [false, true] {
        for offset in [0, 3, 200, RECORDS - 3, usize::MAX] {
            assert_page(
                &provider,
                &conn,
                &request(descending, offset, FileVisibility::Present),
            );
        }
    }
    let snapshot = SqliteProvider::open_read_only(provider.database_path()).unwrap();
    for descending in [false, true] {
        assert_page(
            &snapshot,
            &conn,
            &request(descending, 200, FileVisibility::Present),
        );
    }
    // NULL and nonbinary flags must invalidate the all-present proof; only an
    // exact zero is Present, while Missing retains the existing exact-one rule.
    conn.execute_batch(
        "UPDATE audio_files SET missing=CASE id
        WHEN 1 THEN NULL WHEN 2 THEN 2 WHEN 3 THEN -1 ELSE 1 END WHERE id IN(1,2,3,4)",
    )
    .unwrap();
    for visibility in [
        FileVisibility::Present,
        FileVisibility::Missing,
        FileVisibility::All,
    ] {
        for descending in [false, true] {
            for offset in [0, 3, 200] {
                assert_page(&provider, &conn, &request(descending, offset, visibility));
            }
        }
    }
}

#[test]
fn tag_pages_reject_missing_or_incomplete_covering_indexes() {
    let (_directory, provider, conn) = fixture(false);
    for replacement in [
        "DROP INDEX idx_audio_visible_tagged_folders",
        "CREATE INDEX idx_audio_visible_tagged_folders ON audio_files(folder_id,id,user_tags,source_tags_text)
         WHERE missing=0 AND source_tags_text<>''",
        "DROP INDEX idx_audio_visible_tagged_folders;
         CREATE INDEX idx_audio_visible_tagged_folders ON audio_files(folder_id,id,user_tags)
         WHERE missing=0 AND (COALESCE(trim(user_tags),'')<>'' OR source_tags_text<>'')",
    ] {
        conn.execute_batch(replacement).unwrap();
        for descending in [false,true] {
            for offset in [0,3,200,RECORDS-3] {
                assert_page(&provider,&conn,&request(descending,offset,FileVisibility::Present));
            }
        }
    }
}

#[test]
fn tag_pages_keep_nullable_source_fallback_as_null_in_supported_snapshots() {
    let (_directory, provider, conn) = fixture(true);
    assert_eq!(
        conn.query_row(
            "SELECT [notnull] FROM pragma_table_info('audio_files')
        WHERE name='source_tags_text'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    // A NULL fallback sorts differently from the inferred empty-string key.
    conn.execute(
        "UPDATE audio_files SET user_tags=NULL,source_tags_text=NULL WHERE id=?1",
        [i64::MAX],
    )
    .unwrap();
    let snapshot = SqliteProvider::open_read_only(provider.database_path()).unwrap();
    for descending in [false, true] {
        for offset in [0, 3, 200, RECORDS - 3] {
            let request = request(descending, offset, FileVisibility::Present);
            assert_page(&provider, &conn, &request);
            assert_page(&snapshot, &conn, &request);
        }
    }
}

#[test]
fn tag_pages_keep_nonempty_source_keys_when_partial_index_uses_rtrim_collation() {
    let (_directory, provider, conn) =
        fixture_with_source_column(",source_tags_text TEXT NOT NULL DEFAULT '' COLLATE RTRIM");
    assert_eq!(
        conn.query_row(
            "SELECT [notnull] FROM pragma_table_info('audio_files')
        WHERE name='source_tags_text'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    conn.execute(
        "UPDATE audio_files SET user_tags=NULL,source_tags_text=' ' WHERE id=?1",
        [i64::MAX],
    )
    .unwrap();
    // RTRIM makes the partial predicate exclude a nonempty source key. The
    // actual tag expression still orders this space after an empty key.
    let stored_collation: String = conn
        .query_row(
            "SELECT coll FROM pragma_index_xinfo('idx_audio_visible_tagged_folders')
         WHERE name='source_tags_text' AND key=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stored_collation, "RTRIM");
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM audio_files INDEXED BY idx_audio_visible_tagged_folders
        WHERE missing=0 AND (COALESCE(trim(user_tags),'')<>'' OR source_tags_text<>'') AND id=?1",
            [i64::MAX],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    let expected = serde_json::to_value(provider.get_audio(i64::MAX).unwrap()).unwrap();
    for descending in [false, true] {
        let direction = if descending { "DESC" } else { "ASC" };
        let position = usize::try_from(
            conn.query_row(
                &format!(
                    "SELECT position FROM (
            SELECT af.id,ROW_NUMBER() OVER(ORDER BY
                COALESCE(audiolib_user_tags_key(af.user_tags),af.source_tags_text)
                COLLATE NOCASE {direction},af.id ASC)-1 AS position
            FROM audio_search CROSS JOIN audio_files af ON af.id=audio_search.rowid
            WHERE audio_search MATCH 'metal' AND af.missing=0) WHERE id=?1"
                ),
                [i64::MAX],
                |r| r.get::<_, i64>(0),
            )
            .unwrap(),
        )
        .unwrap();
        let request = request(
            descending,
            position.saturating_sub(3),
            FileVisibility::Present,
        );
        assert_page(&provider, &conn, &request);
        let page = provider.search(&request).unwrap();
        let actual = page
            .results
            .iter()
            .find(|file| file.id == i64::MAX)
            .unwrap();
        assert_eq!(serde_json::to_value(actual).unwrap(), expected);
    }
}
