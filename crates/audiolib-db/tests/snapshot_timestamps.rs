// crates/audiolib-db/tests/snapshot_timestamps.rs
//! Incremental Metadata replies preserve the server's timestamp and ordering.
mod support;
use audiolib_core::{DataProvider, SearchRequest, SortOrder};
use audiolib_db::SqliteProvider;
use std::path::Path;

fn fixture(path: &Path, version: i64) {
    if version < audiolib_db::SCHEMA_VERSION {
        let conn = support::connection(path).unwrap();
        conn.execute_batch(include_str!("fixtures/schema-v5.sql"))
            .unwrap();
        conn.pragma_update(None, "user_version", version).unwrap();
    } else {
        drop(SqliteProvider::open(path).unwrap());
    }
    support::connection(path).unwrap().execute_batch(
        "INSERT INTO audio_files(id,file_path,path_key,filename,modified_at,metadata_json)
         VALUES(1,'/remote/one.wav','/remote/one.wav','one.wav','2020-01-01 01:02:03','{\"private\":\"retain\"}'),
         (2,'/remote/two.wav','/remote/two.wav','two.wav','2024-01-01 01:02:03','{}');"
    ).unwrap();
}

#[test]
fn authoritative_timestamp_and_modified_sort_match_for_all_snapshot_schemas() {
    let root = tempfile::tempdir().unwrap();
    for version in 3..=audiolib_db::SCHEMA_VERSION {
        let path = root.path().join(format!("snapshot-v{version}.db"));
        fixture(&path, version);
        let provider = SqliteProvider::open_read_only(&path).unwrap();
        assert_eq!(provider.database_path(), path);
        let mut reply = provider.get_audio(1).unwrap();
        reply.tags = "updated tag".into();
        reply.notes = "server note".into();
        // Deliberately unlike the client's wall clock, and keep exact precision.
        reply.attributes["modified_at"] = "2031-04-05 06:07:08.123456".into();
        SqliteProvider::apply_snapshot_records(&path, &[reply]).unwrap();
        let row = provider.get_audio(1).unwrap();
        assert_eq!(row.attributes["modified_at"], "2031-04-05 06:07:08.123456");
        assert_eq!(row.attributes["private"], "retain");
        assert_eq!(row.notes, "server note");
        for (descending, expected) in [(true, vec![1, 2]), (false, vec![2, 1])] {
            let page = provider
                .search(&SearchRequest {
                    order_by: SortOrder::Modified,
                    descending,
                    ..Default::default()
                })
                .unwrap();
            assert_eq!(
                page.results.iter().map(|row| row.id).collect::<Vec<_>>(),
                expected
            );
        }
        assert_eq!(
            provider
                .search(&SearchRequest {
                    query: "updated".into(),
                    ..Default::default()
                })
                .unwrap()
                .total_count,
            1
        );
    }
}

#[test]
fn unsupported_or_missing_timestamps_require_full_sync_without_partial_writes() {
    let root = tempfile::tempdir().unwrap();
    for version in [5, audiolib_db::SCHEMA_VERSION] {
        let path = root.path().join(format!("invalid-v{version}.db"));
        fixture(&path, version);
        let provider = SqliteProvider::open_read_only(&path).unwrap();
        let original = provider.get_audio(1).unwrap();
        for bad in [
            None,
            Some(serde_json::Value::Null),
            Some(17.into()),
            Some("tomorrow".into()),
            Some("2026-02-30 12:00:00".into()),
            Some("2026-10-24T12:00:00Z".into()),
        ] {
            let mut first = original.clone();
            first.notes = "must roll back".into();
            let mut second = provider.get_audio(2).unwrap();
            if let Some(bad) = bad {
                second.attributes["modified_at"] = bad;
            } else {
                second
                    .attributes
                    .as_object_mut()
                    .unwrap()
                    .remove("modified_at");
            }
            assert!(SqliteProvider::apply_snapshot_records(&path, &[first, second]).is_err());
            assert_eq!(provider.get_audio(1).unwrap().notes, original.notes);
            assert_eq!(
                provider.get_audio(1).unwrap().attributes["modified_at"],
                original.attributes["modified_at"]
            );
        }
    }
}
