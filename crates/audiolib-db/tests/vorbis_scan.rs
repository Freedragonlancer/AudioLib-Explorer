// crates/audiolib-db/tests/vorbis_scan.rs
//! Real FLAC/Ogg scanning, searchable source tags and lossless incremental re-extraction.
use audiolib_core::{DataProvider, MetadataPatch, SearchRequest};
use audiolib_db::{ScanCancellation, ScanOptions, ScanService, SqliteProvider};
use std::{fs, path::Path, sync::Arc};
mod support;

fn copy_sources(root: &Path) {
    fs::create_dir_all(root).unwrap();
    let fixtures =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../audiolib-audio/src/fixtures/vorbis");
    for name in ["comments.flac", "comments.ogg", "comments.oga"] {
        fs::copy(fixtures.join(name), root.join(name)).unwrap();
    }
}

#[test]
fn actual_vorbis_tags_scan_search_and_reextract_without_losing_annotations_or_vendor_data() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("sources");
    copy_sources(&root);
    let database = temp.path().join("library.db");
    let provider = Arc::new(SqliteProvider::open(&database).unwrap());
    let scan = ScanService::new(provider.clone())
        .scan(
            &root,
            &ScanOptions::default(),
            &ScanCancellation::default(),
            |_| {},
        )
        .unwrap();
    assert_eq!(scan.added, 3);
    assert_eq!(scan.errors, 0, "{scan:?}");
    let original = provider
        .search(&SearchRequest {
            query: "树林 微风".into(),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(original.total_count, 3);
    for file in &original.results {
        assert_eq!(
            file.attributes["wav_metadata"]["vorbis"]["DESCRIPTION"],
            "树林微风"
        );
        assert_eq!(
            file.attributes["wav_metadata"]["vorbis_version"],
            audiolib_audio::VORBIS_METADATA_VERSION
        );
        provider
            .update_metadata(
                file.id,
                &MetadataPatch {
                    notes: Some("user annotation".into()),
                    tags: Some("user-tag".into()),
                    favorite: Some(true),
                    ..Default::default()
                },
            )
            .unwrap();
    }
    let conn = support::connection(&database).unwrap();
    conn.execute("UPDATE audio_files SET metadata_json=json_set(metadata_json,'$.unknown_extension',json('{\"preserve\":true}'),'$.vendor_metadata',json('{\"fields\":{\"description\":\"Vendor controlled description\",\"keywords\":\"vendor-word\"}}'),'$.wav_metadata.vorbis_version',0)",[]).unwrap();
    drop(conn);
    let again = ScanService::new(provider.clone())
        .scan(
            &root,
            &ScanOptions::default(),
            &ScanCancellation::default(),
            |_| {},
        )
        .unwrap();
    assert_eq!(again.updated, 3);
    assert_eq!(again.errors, 0);
    for old in original.results {
        let file = provider.get_audio(old.id).unwrap();
        assert_eq!(file.notes, "user annotation");
        assert_eq!(file.tags, "user-tag");
        assert!(file.favorite);
        assert_eq!(file.attributes["unknown_extension"]["preserve"], true);
        assert_eq!(
            file.attributes["vendor_metadata"]["fields"]["description"],
            "Vendor controlled description"
        );
        assert_eq!(
            audiolib_core::source_description("", &file.attributes),
            "Vendor controlled description"
        );
        assert_eq!(
            file.attributes["wav_metadata"]["vorbis_version"],
            audiolib_audio::VORBIS_METADATA_VERSION
        );
    }
    for query in [
        "树林 微风",
        "Vendor controlled",
        "vendor-word",
        "user annotation",
    ] {
        assert_eq!(
            provider
                .search(&SearchRequest {
                    query: query.into(),
                    ..Default::default()
                })
                .unwrap()
                .total_count,
            3,
            "{query}"
        );
    }
    let unchanged = ScanService::new(provider)
        .scan(
            &root,
            &ScanOptions::default(),
            &ScanCancellation::default(),
            |_| {},
        )
        .unwrap();
    assert_eq!(unchanged.unchanged, 3);
    assert_eq!(unchanged.updated, 0);
}
