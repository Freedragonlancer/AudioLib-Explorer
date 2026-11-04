// crates/audiolib-db/tests/vendor_import.rs
//! Real parser/provenance/FTS regressions on owned libraries, never working audio or settings.
use audiolib_core::{DataProvider, SearchRequest, vendor_import::*};
use audiolib_db::SqliteProvider;
use rusqlite::{Connection, params};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

fn connection(path: &Path) -> Connection {
    let conn = Connection::open(path).unwrap();
    audiolib_db::register_library_functions(&conn).unwrap();
    conn
}
fn fixture(duplicate: bool) -> (tempfile::TempDir, SqliteProvider, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let database = temp.path().join("library.db");
    let provider = SqliteProvider::open(&database).unwrap();
    let root = temp.path().join("audio");
    fs::create_dir(&root).unwrap();
    let conn = connection(&database);
    conn.execute(
        "INSERT INTO folders(id,folder_path,folder_name) VALUES(1,?1,'audio')",
        [root.to_string_lossy().as_ref()],
    )
    .unwrap();
    conn.execute("INSERT INTO folder_closure VALUES(1,1,0)", [])
        .unwrap();
    for (id, name) in [(1, "door.wav"), (2, "slam.wav")] {
        conn.execute("INSERT INTO audio_files(id,file_path,path_key,filename,folder_id,bwf_description,user_tags,user_notes,metadata_json) VALUES(?1,?2,?2,?3,1,'embeddedoldword','protectedtag','protectednote','{\"unknown\":{\"keep\":42}}')",params![id,root.join(name).to_string_lossy(),name]).unwrap();
    }
    if duplicate {
        conn.execute("INSERT INTO audio_files(id,file_path,path_key,filename,folder_id,metadata_json) VALUES(3,?1,?1,'door.wav',1,'{}')",[root.join("other/door.wav").to_string_lossy().as_ref()]).unwrap();
    }
    (temp, provider, database)
}
fn input(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/vendor_import")
        .join(name)
}
fn text(temp: &Path, name: &str, value: &str) -> PathBuf {
    let path = temp.join(name);
    fs::write(&path, value).unwrap();
    path
}
fn count(provider: &SqliteProvider, query: &str) -> u64 {
    provider
        .search(&SearchRequest {
            query: query.into(),
            ..Default::default()
        })
        .unwrap()
        .total_count
}
fn projection(path: &Path, id: i64) -> String {
    connection(path).query_row("SELECT audiolib_tokenize(audiolib_source_text(bwf_description,metadata_json)) FROM audio_files WHERE id=?1",[id],|r|r.get(0)).unwrap()
}

#[test]
fn preview_is_business_read_only_and_commit_rollback_preserve_metadata_and_fts() {
    let (_temp, provider, database) = fixture(false);
    let conn = connection(&database);
    let bytes = fs::read(&database).unwrap();
    let metadata: String = conn
        .query_row(
            "SELECT metadata_json FROM audio_files WHERE id=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let derived = projection(&database, 1);
    let preview = provider
        .vendor_import_preview_impl(&input("basic.csv"))
        .unwrap();
    assert_eq!(preview.rows, 2);
    assert_eq!(preview.matched, 2);
    assert!(preview.can_commit);
    assert_eq!(fs::read(&database).unwrap(), bytes);
    assert_eq!(
        conn.query_row(
            "SELECT metadata_json FROM audio_files WHERE id=1",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        metadata
    );
    assert!(preview.samples[0].user_conflict);
    let imported = provider
        .vendor_import_commit_impl(&preview.import_id, "request-a")
        .unwrap();
    assert_eq!(imported.written, 2);
    assert_eq!(imported.state.as_deref(), Some("committed"));
    assert_eq!(count(&provider, "vendoruniqueword"), 1);
    assert_eq!(count(&provider, "embeddedoldword"), 2);
    assert_eq!(
        provider
            .vendor_import_commit_impl(&preview.import_id, "request-a")
            .unwrap(),
        imported
    );
    assert!(
        provider
            .vendor_import_commit_impl(&preview.import_id, "different-key")
            .is_err()
    );
    let (tags, notes, raw): (String, String, String) = conn
        .query_row(
            "SELECT user_tags,user_notes,metadata_json FROM audio_files WHERE id=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        (tags.as_str(), notes.as_str()),
        ("protectedtag", "protectednote")
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&raw).unwrap()["unknown"]["keep"],
        42
    );
    let rollback = provider
        .vendor_import_rollback_impl(&preview.import_id)
        .unwrap();
    assert_eq!(rollback.state.as_deref(), Some("rolled_back"));
    assert_eq!(projection(&database, 1), derived);
    assert_eq!(count(&provider, "vendoruniqueword"), 0);
    assert_eq!(
        provider
            .vendor_import_rollback_impl(&preview.import_id)
            .unwrap(),
        rollback
    );
    assert_eq!(
        provider
            .vendor_import_commit_impl(&preview.import_id, "request-a")
            .unwrap(),
        imported,
        "idempotency receipt survives rollback without reimporting"
    );
    assert_eq!(
        conn.query_row("SELECT count(*) FROM vendor_metadata", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn real_biff_and_streaming_xlsx_use_magic_multiple_sheets_and_later_headers() {
    let (temp, provider, _) = fixture(false);
    for name in ["basic.xls", "basic.xlsx"] {
        let preview = provider.vendor_import_preview_impl(&input(name)).unwrap();
        assert!(
            preview.files.iter().all(|f| f.error.is_none()),
            "{preview:?}"
        );
        assert_eq!(preview.rows, 3);
        assert_eq!(preview.sheets.len(), 2);
        assert_eq!(
            preview
                .sheets
                .iter()
                .find(|s| s.sheet == "English")
                .unwrap()
                .header_row,
            2
        );
        assert_eq!(preview.matched, 3);
    }
    let renamed = temp.path().join("misleading.csv.xls");
    fs::copy(input("basic.csv"), &renamed).unwrap();
    let preview = provider.vendor_import_preview_impl(&renamed).unwrap();
    assert_eq!(preview.files[0].format, "text");
    assert_eq!(preview.rows, 2);
}

#[test]
fn text_encoding_rfc4180_and_separator_detection_preserve_decoded_values() {
    let (_temp, provider, _) = fixture(false);
    for (name, encoding) in [
        ("utf16le.csv", "UTF-16LE"),
        ("utf16be.csv", "UTF-16BE"),
        ("gb18030.csv", "gb18030"),
    ] {
        let preview = provider.vendor_import_preview_impl(&input(name)).unwrap();
        assert!(preview.files[0].error.is_none(), "{preview:?}");
        assert!(preview.files[0].encoding.eq_ignore_ascii_case(encoding));
        assert_eq!(preview.matched, 1);
        assert!(preview.samples[0].values["description"].contains('中'));
    }
    let preview = provider
        .vendor_import_preview_impl(&input("basic.csv"))
        .unwrap();
    assert_eq!(
        preview.samples[1].values["description"],
        "\"Quoted\" slam\nsecond line"
    );
}

#[test]
fn ambiguous_and_fuzzy_rows_require_explicit_existing_candidate_confirmation() {
    let (temp, provider, _) = fixture(true);
    let file = text(
        temp.path(),
        "ambiguous.csv",
        "Filename,Description\ndoor.wav,confirmedword\n",
    );
    let preview = provider.vendor_import_preview_impl(&file).unwrap();
    assert_eq!(preview.ambiguous, 1);
    assert_eq!(preview.samples[0].candidates.len(), 2);
    assert!(!preview.can_commit);
    let mut options = preview.options.clone();
    options
        .confirmations
        .insert(preview.samples[0].row_id.clone(), 1);
    let mapped = provider
        .vendor_import_mapping_impl(&preview.import_id, &options)
        .unwrap();
    assert!(mapped.can_commit);
    assert_eq!(mapped.matched, 1);
    options
        .confirmations
        .insert(preview.samples[0].row_id.clone(), 999);
    assert!(
        provider
            .vendor_import_mapping_impl(&preview.import_id, &options)
            .is_err()
    );
    let file = text(
        temp.path(),
        "fuzzy.csv",
        "Filename,Description\nd_o_o_r.wav,suggestionword\n",
    );
    let preview = provider
        .vendor_import_preview_with_options_impl(
            &file,
            &VendorImportOptions {
                fuzzy: true,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(preview.suggested, 1);
    assert!(!preview.can_commit);
    assert!(preview.samples[0].matched_id.is_none());
}

#[test]
fn large_ambiguities_share_disk_sets_and_any_page_can_be_confirmed() {
    let (temp, provider, database) = fixture(false);
    let conn = connection(&database);
    let root = temp.path().join("audio");
    for id in 3..=103i64 {
        let path = root.join(format!("duplicate-{id}/door.wav"));
        conn.execute("INSERT INTO audio_files(id,file_path,path_key,filename,folder_id,metadata_json) VALUES(?1,?2,?2,'door.wav',1,'{}')",params![id,path.to_string_lossy()]).unwrap();
    }
    let file = text(
        temp.path(),
        "many-candidates.csv",
        "Filename,Description\ndoor.wav,pagedcandidateword\ndoor.wav,secondrowword\n",
    );
    let preview = provider.vendor_import_preview_impl(&file).unwrap();
    assert_eq!(preview.ambiguous, 2);
    assert_eq!(preview.samples[0].candidate_count, 102);
    assert_eq!(preview.samples[0].candidates.len(), 32);
    assert!(preview.samples[0].candidates_truncated);
    let page = provider
        .vendor_import_candidates_impl(&preview.import_id, &preview.samples[0].row_id, 64, 1000)
        .unwrap();
    assert_eq!(page.offset, 64);
    assert_eq!(page.total, 102);
    assert_eq!(page.candidates.len(), 38);
    let chosen = page.candidates[0].id;
    assert!(!preview.samples[0].candidates.iter().any(|c| c.id == chosen));
    let stage = Connection::open(
        database
            .with_file_name("library.db.vendor-import")
            .join(&preview.import_id)
            .join("preview.db"),
    )
    .unwrap();
    assert_eq!(
        stage
            .query_row("SELECT count(*) FROM candidate_sets", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        stage
            .query_row("SELECT count(*) FROM candidate_set_members", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        102
    );
    let persisted: String = stage
        .query_row(
            "SELECT result FROM rows ORDER BY ordinal LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        serde_json::from_str::<VendorRowPreview>(&persisted)
            .unwrap()
            .candidates
            .is_empty()
    );
    let mut options = preview.options.clone();
    options
        .confirmations
        .insert(preview.samples[0].row_id.clone(), chosen);
    let mapped = provider
        .vendor_import_mapping_impl(&preview.import_id, &options)
        .unwrap();
    assert_eq!(mapped.samples[0].matched_id, Some(chosen));
    assert_eq!(mapped.samples[0].candidates.len(), 32);
    assert_eq!(
        provider
            .vendor_import_commit_impl(&preview.import_id, "any-page")
            .unwrap()
            .written,
        1
    );
    assert_eq!(count(&provider, "pagedcandidateword"), 1);
    assert_eq!(count(&provider, "secondrowword"), 0);
}

#[test]
fn row_pages_are_continuous_and_bounded_by_actual_serialized_bytes() {
    let (temp, provider, _database) = fixture(false);
    let value = "x".repeat(240_000);
    let mut csv = String::from("Filename,Description\n");
    for _ in 0..10 {
        csv.push_str(&format!("door.wav,{value}\n"));
    }
    let file = text(temp.path(), "large-rows.csv", &csv);
    let preview = provider.vendor_import_preview_impl(&file).unwrap();
    assert_eq!(preview.rows, 10);
    assert!(preview.samples.len() < 10);
    assert!(serde_json::to_vec(&preview.samples).unwrap().len() <= 2 * 1024 * 1024);
    let mut offset = 0u64;
    while offset < 10 {
        let page = provider
            .vendor_import_rows_impl(&preview.import_id, offset, 1000)
            .unwrap();
        assert!(!page.is_empty());
        assert!(serde_json::to_vec(&page).unwrap().len() <= 2 * 1024 * 1024);
        for (index, row) in page.iter().enumerate() {
            assert_eq!(row.row, offset + index as u64 + 2);
        }
        offset += page.len() as u64;
    }
    assert_eq!(offset, 10);
}

#[test]
fn mapping_and_templates_are_durable_and_missing_identity_blocks_commit() {
    let (temp, provider, database) = fixture(false);
    let file = text(
        temp.path(),
        "renamed.csv",
        "Sound,Custom\ndoor.wav,mappingword\n",
    );
    let preview = provider.vendor_import_preview_impl(&file).unwrap();
    assert!(!preview.can_commit);
    let mapping = VendorColumnMapping {
        filename: Some("Sound".into()),
        relative_path: None,
        fields: BTreeMap::from([("description".into(), "Custom".into())]),
    };
    let template = provider
        .vendor_import_save_template_impl(&VendorMappingTemplate {
            name: "Sound list".into(),
            mapping,
            ..Default::default()
        })
        .unwrap();
    drop(provider);
    let reopened = SqliteProvider::open(&database).unwrap();
    assert_eq!(
        reopened.vendor_import_templates_impl().unwrap(),
        vec![template.clone()]
    );
    let mapped = reopened
        .vendor_import_mapping_impl(
            &preview.import_id,
            &VendorImportOptions {
                template: Some(template.id),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(mapped.can_commit);
    assert_eq!(
        reopened
            .vendor_import_rows_impl(&preview.import_id, 0, 1)
            .unwrap()[0]
            .values["description"],
        "mappingword"
    );
    reopened
        .vendor_import_commit_impl(&preview.import_id, "mapped-key")
        .unwrap();
    assert_eq!(count(&reopened, "mappingword"), 1);
}

#[test]
fn duplicate_values_and_overlapping_rollbacks_do_not_erase_other_batches_or_new_embedded_text() {
    let (temp, provider, database) = fixture(false);
    let file = text(
        temp.path(),
        "a.csv",
        "Filename,Description,Keywords\ndoor.wav,sharedvendor,Wood;Door\n",
    );
    let a = provider.vendor_import_preview_impl(&file).unwrap();
    provider
        .vendor_import_commit_impl(&a.import_id, "key-a")
        .unwrap();
    let b = provider.vendor_import_preview_impl(&file).unwrap();
    provider
        .vendor_import_commit_impl(&b.import_id, "key-b")
        .unwrap();
    let conn = connection(&database);
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM vendor_metadata WHERE field='description'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    conn.execute("UPDATE audio_files SET bwf_description='embeddedchangedword',metadata_json=json_set(metadata_json,'$.unknown.rescanned',1) WHERE id=1",[]).unwrap();
    provider.vendor_import_rollback_impl(&a.import_id).unwrap();
    assert_eq!(count(&provider, "sharedvendor"), 1);
    assert_eq!(count(&provider, "embeddedchangedword"), 1);
    provider.vendor_import_rollback_impl(&b.import_id).unwrap();
    assert_eq!(count(&provider, "sharedvendor"), 0);
    assert_eq!(count(&provider, "embeddedchangedword"), 1);
    let raw: String = conn
        .query_row(
            "SELECT metadata_json FROM audio_files WHERE id=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&raw).unwrap()["unknown"]["rescanned"],
        1
    );
}

#[test]
fn explicit_overwrite_rollback_preserves_a_later_batch_and_uses_original_before_image() {
    let (temp, provider, _) = fixture(false);
    let a = text(
        temp.path(),
        "a.csv",
        "Filename,Description\ndoor.wav,firstvendor\n",
    );
    let a = provider.vendor_import_preview_impl(&a).unwrap();
    provider
        .vendor_import_commit_impl(&a.import_id, "overwrite-a")
        .unwrap();
    let b = text(
        temp.path(),
        "b.csv",
        "Filename,Description\ndoor.wav,secondvendor\n",
    );
    let b = provider
        .vendor_import_preview_with_options_impl(
            &b,
            &VendorImportOptions {
                overwrite: true,
                ..Default::default()
            },
        )
        .unwrap();
    provider
        .vendor_import_commit_impl(&b.import_id, "overwrite-b")
        .unwrap();
    assert_eq!(count(&provider, "firstvendor"), 0);
    assert_eq!(count(&provider, "secondvendor"), 1);
    provider.vendor_import_rollback_impl(&a.import_id).unwrap();
    assert_eq!(count(&provider, "secondvendor"), 1);
    provider.vendor_import_rollback_impl(&b.import_id).unwrap();
    assert_eq!(count(&provider, "firstvendor"), 0);
    assert_eq!(count(&provider, "secondvendor"), 0);
    assert_eq!(count(&provider, "embeddedoldword"), 2);
}

#[test]
fn multilang_merge_is_order_independent_and_keywords_are_a_union() {
    let (temp, provider, database) = fixture(false);
    let file = text(
        temp.path(),
        "language.csv",
        "Filename,Description,Keywords\ndoor.wav,English description,wood;door\ndoor.wav,中文描述,DOOR;slam\n",
    );
    let preview = provider.vendor_import_preview_impl(&file).unwrap();
    provider
        .vendor_import_commit_impl(&preview.import_id, "language-key")
        .unwrap();
    let raw: String = connection(&database)
        .query_row(
            "SELECT metadata_json FROM audio_files WHERE id=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(
        value["vendor_metadata"]["fields"]["description"][0],
        "中文描述"
    );
    assert_eq!(
        value["vendor_metadata"]["fields"]["keywords"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

#[test]
fn commit_pause_resume_is_atomic_and_status_exposes_the_original_key() {
    let (temp, provider, _) = fixture(false);
    let mut content = String::from("Filename,Description\n");
    for index in 0..5001 {
        content.push_str(&format!("door.wav,piece{index}\n"));
    }
    let file = text(temp.path(), "many.csv", &content);
    let preview = provider.vendor_import_preview_impl(&file).unwrap();
    let cancel = AtomicBool::new(false);
    let partial = provider
        .vendor_import_commit_cancellable_impl(
            &preview.import_id,
            "resumable-key",
            &cancel,
            &mut |status| {
                assert!(status.completed > 0);
                cancel.store(true, Ordering::Release);
            },
        )
        .unwrap();
    assert_eq!(partial.state.as_deref(), Some("paused"));
    assert!(partial.written > 0 && partial.written < 5001);
    let status = provider
        .vendor_import_status_impl(&preview.import_id)
        .unwrap();
    assert_eq!(status.idempotency_key.as_deref(), Some("resumable-key"));
    let complete = provider
        .vendor_import_commit_impl(&preview.import_id, "resumable-key")
        .unwrap();
    assert_eq!(complete.written, 5001);
    assert_eq!(
        provider
            .vendor_import_status_impl(&preview.import_id)
            .unwrap()
            .preview
            .state
            .as_deref(),
        Some("committed")
    );
}

#[test]
fn limits_refuse_entire_file_skip_system_names_and_truncate_only_at_utf8_boundaries() {
    let (temp, provider, _) = fixture(false);
    let large = temp.path().join("large.xls");
    let output = fs::File::create(&large).unwrap();
    output.set_len(52_428_801).unwrap();
    drop(output);
    let skipped = text(
        temp.path(),
        "._hidden.csv",
        "Filename,Description\ndoor.wav,unwanted\n",
    );
    let preview = provider
        .vendor_import_preview_files_impl(
            &[large, skipped, input("basic.csv")],
            &VendorImportOptions::default(),
        )
        .unwrap();
    assert_eq!(
        preview.files.iter().filter(|f| f.error.is_some()).count(),
        2
    );
    assert_eq!(preview.rows, 2);
    let file = text(
        temp.path(),
        "long.csv",
        &format!("Filename,Description\ndoor.wav,{}\n", "中".repeat(90_000)),
    );
    let preview = provider.vendor_import_preview_impl(&file).unwrap();
    assert_eq!(preview.files[0].truncated_rows, 1);
    assert!(preview.samples[0].values["description"].len() <= 262144);
}

#[test]
fn csv_over_row_limit_is_not_partially_importable() {
    let (temp, provider, _) = fixture(false);
    let mut content = String::from("Filename,Description\n");
    for _ in 0..200001 {
        content.push_str("door.wav,row\n");
    }
    let file = text(temp.path(), "too-many.csv", &content);
    let preview = provider.vendor_import_preview_impl(&file).unwrap();
    assert_eq!(preview.rows, 0);
    assert!(!preview.can_commit);
    assert!(
        preview.files[0]
            .error
            .as_deref()
            .unwrap()
            .contains("200000")
    );
}

#[test]
fn committed_rollback_history_survives_missing_preview_sidecars() {
    let (temp, provider, database) = fixture(false);
    let file = text(
        temp.path(),
        "history.csv",
        "Filename,Description\ndoor.wav,historyword\n",
    );
    let preview = provider.vendor_import_preview_impl(&file).unwrap();
    provider
        .vendor_import_commit_impl(&preview.import_id, "history-key")
        .unwrap();
    drop(provider);
    let sidecar = database.with_file_name("library.db.vendor-import");
    assert!(sidecar.starts_with(temp.path()));
    fs::remove_dir_all(&sidecar).unwrap();
    let reopened = SqliteProvider::open(&database).unwrap();
    let history = reopened.vendor_import_history_impl().unwrap();
    assert_eq!(history.imports.len(), 1);
    assert_eq!(history.imports[0].preview.import_id, preview.import_id);
    reopened
        .vendor_import_rollback_impl(&preview.import_id)
        .unwrap();
    assert_eq!(count(&reopened, "historyword"), 0);
}

fn workbook_with_first_sheet(path: &Path, xml: &str) {
    use std::io::{Read, Write};
    let mut original = zip::ZipArchive::new(fs::File::open(input("basic.xlsx")).unwrap()).unwrap();
    let mut output = zip::ZipWriter::new(fs::File::create(path).unwrap());
    for index in 0..original.len() {
        let mut entry = original.by_index(index).unwrap();
        let name = entry.name().to_owned();
        output
            .start_file(
                &name,
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated),
            )
            .unwrap();
        if name == "xl/worksheets/sheet1.xml" {
            output.write_all(xml.as_bytes()).unwrap();
        } else {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            output.write_all(&bytes).unwrap();
        }
    }
    output.finish().unwrap();
}

fn inline_sheet(text: &str) -> String {
    format!(
        "<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData><row r=\"1\"><c r=\"A1\" t=\"inlineStr\"><is><t>Filename</t></is></c><c r=\"B1\" t=\"inlineStr\"><is><t>Description</t></is></c></row><row r=\"2\"><c r=\"A2\" t=\"inlineStr\"><is><t>door.wav</t></is></c><c r=\"B2\" t=\"inlineStr\"><is><t>{text}</t></is></c></row></sheetData></worksheet>"
    )
}

#[test]
fn compressed_giant_inline_cell_is_refused_before_calamine_and_other_files_continue() {
    let (temp, provider, _) = fixture(false);
    let path = temp.path().join("bomb.xlsx");
    workbook_with_first_sheet(&path, &inline_sheet(&"x".repeat(1024 * 1024 + 1)));
    assert!(fs::metadata(&path).unwrap().len() < 50_000);
    let preview = provider
        .vendor_import_preview_files_impl(
            &[path, input("basic.csv")],
            &VendorImportOptions::default(),
        )
        .unwrap();
    assert!(
        preview
            .files
            .iter()
            .find(|f| f.name == "bomb.xlsx")
            .unwrap()
            .error
            .as_deref()
            .unwrap()
            .contains("safety")
    );
    assert_eq!(preview.rows, 2);
    assert_eq!(preview.matched, 2);
}

#[test]
fn ordinary_xlsx_row_just_above_text_limit_is_truncated_instead_of_refusing_the_file() {
    let (temp, provider, _) = fixture(false);
    let path = temp.path().join("row.xlsx");
    workbook_with_first_sheet(&path, &inline_sheet(&"x".repeat(262145)));
    let preview = provider.vendor_import_preview_impl(&path).unwrap();
    assert!(preview.files[0].error.is_none(), "{preview:?}");
    assert_eq!(preview.files[0].truncated_rows, 1);
    assert_eq!(preview.rows, 2);
}

#[test]
fn lying_zip_uncompressed_size_does_not_bypass_xml_safety_preflight() {
    let (temp, provider, _) = fixture(false);
    let path = temp.path().join("lying.xlsx");
    workbook_with_first_sheet(&path, &inline_sheet(&"x".repeat(1024 * 1024 + 1)));
    let mut bytes = fs::read(&path).unwrap();
    let mut position = 0;
    let mut patched = false;
    while position + 46 < bytes.len() {
        if &bytes[position..position + 4] == b"PK\x01\x02" {
            let length = u16::from_le_bytes(bytes[position + 28..position + 30].try_into().unwrap())
                as usize;
            if position + 46 + length <= bytes.len()
                && &bytes[position + 46..position + 46 + length] == b"xl/worksheets/sheet1.xml"
            {
                bytes[position + 24..position + 28].copy_from_slice(&1u32.to_le_bytes());
                patched = true;
                break;
            }
        }
        position += 1;
    }
    assert!(patched);
    fs::write(&path, bytes).unwrap();
    let preview = provider.vendor_import_preview_impl(&path).unwrap();
    assert!(preview.files[0].error.is_some());
    assert_eq!(preview.rows, 0);
}

#[test]
fn shared_string_text_budget_is_checked_streaming_with_a_small_compressed_fixture() {
    use std::io::Write;
    let (temp, provider, _) = fixture(false);
    let path = temp.path().join("strings.xlsx");
    let mut output = zip::ZipWriter::new(fs::File::create(&path).unwrap());
    output
        .start_file(
            "xl/sharedStrings.xml",
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated),
        )
        .unwrap();
    output.write_all(b"<sst>").unwrap();
    let chunk = "x".repeat(8192);
    for _ in 0..8193 {
        output.write_all(b"<si><t>").unwrap();
        output.write_all(chunk.as_bytes()).unwrap();
        output.write_all(b"</t></si>").unwrap();
    }
    output.write_all(b"</sst>").unwrap();
    output.finish().unwrap();
    assert!(fs::metadata(&path).unwrap().len() < 200_000);
    let preview = provider.vendor_import_preview_impl(&path).unwrap();
    assert!(
        preview.files[0]
            .error
            .as_deref()
            .unwrap()
            .contains("sharedStrings")
    );
}

#[test]
fn a_batch_can_include_the_449_real_world_table_count_up_to_512_files() {
    let (_temp, provider, _) = fixture(false);
    let files = vec![input("basic.csv"); 512];
    let preview = provider
        .vendor_import_preview_files_impl(&files, &VendorImportOptions::default())
        .unwrap();
    assert_eq!(preview.files.len(), 512);
    assert_eq!(preview.rows, 1024);
    assert!(
        provider
            .vendor_import_preview_files_impl(
                &vec![input("basic.csv"); 513],
                &VendorImportOptions::default()
            )
            .is_err()
    );
}
