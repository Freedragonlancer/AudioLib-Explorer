// crates/audiolib-db/tests/path_case_identity.rs
//! Legacy folded keys are buckets, not identities for case-sensitive source paths.
mod support;
use audiolib_core::DataProvider;
#[cfg(any(unix, windows))]
use audiolib_core::MetadataPatch;
use audiolib_db::SqliteProvider;
use std::{fs, path::Path};

#[test]
fn foreign_posix_snapshots_and_annotation_imports_distinguish_case_on_every_host() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("library.db");
    let local = SqliteProvider::open(&path).unwrap();
    let conn = support::connection(&path).unwrap();
    conn.execute_batch(
        "INSERT INTO audio_files(id,file_path,path_key,filename,user_notes) VALUES
        (1,'/Volumes/SFX/Foo.wav','/volumes/sfx/foo.wav','Foo.wav','uppercase'),
        (2,'/Volumes/SFX/foo.wav','/volumes/sfx/foo.wav','foo.wav','lowercase'),
        (3,'C:/Sounds/Boom.wav','c:/sounds/boom.wav','Boom.wav','Windows alias'),
        (4,'C:/Sensitive/Foo.wav','c:/sensitive/foo.wav','Foo.wav','Windows uppercase'),
        (5,'C:/Sensitive/foo.wav','c:/sensitive/foo.wav','foo.wav','Windows lowercase');",
    )
    .unwrap();
    drop(conn);
    let snapshot = SqliteProvider::open_read_only(&path).unwrap();
    for (path, id) in [
        ("/Volumes/SFX/Foo.wav", 1),
        ("/Volumes/SFX/foo.wav", 2),
        ("c:/sounds/BOOM.wav", 3),
        ("c:/Sensitive/Foo.wav", 4),
        ("C:/Sensitive/foo.wav", 5),
    ] {
        assert_eq!(
            snapshot
                .find_audio_by_path(Path::new(path))
                .unwrap()
                .unwrap()
                .id,
            id
        );
    }
    assert!(
        snapshot
            .find_audio_by_path(Path::new("/volumes/sfx/foo.wav"))
            .unwrap()
            .is_none()
    );
    assert!(
        snapshot
            .find_audio_by_path(Path::new("c:/sensitive/FOO.wav"))
            .is_err(),
        "an offline ambiguous alias must not select either case-distinct record"
    );
    drop(snapshot);
    let annotations = root.path().join("annotations.jsonl");
    fs::write(
        &annotations,
        "{\"file_path\":\"/Volumes/SFX/foo.wav\",\"notes\":\"only lowercase\"}\n",
    )
    .unwrap();
    assert_eq!(local.import_annotations(&annotations).unwrap().records, 1);
    assert_eq!(local.get_audio(1).unwrap().notes, "uppercase");
    assert_eq!(local.get_audio(2).unwrap().notes, "only lowercase");
    fs::write(
        &annotations,
        "{\"file_path\":\"c:/sounds/BOOM.wav\",\"notes\":\"alias updated\"}\n",
    )
    .unwrap();
    assert_eq!(local.import_annotations(&annotations).unwrap().records, 1);
    assert_eq!(local.get_audio(3).unwrap().notes, "alias updated");
    fs::write(
        &annotations,
        "{\"file_path\":\"c:/Sensitive/foo.wav\",\"notes\":\"only Windows lowercase\"}\n",
    )
    .unwrap();
    assert_eq!(local.import_annotations(&annotations).unwrap().records, 1);
    assert_eq!(local.get_audio(4).unwrap().notes, "Windows uppercase");
    assert_eq!(local.get_audio(5).unwrap().notes, "only Windows lowercase");
}

#[test]
fn case_distinct_posix_folder_identities_keep_legacy_keys() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("library.db");
    let local = SqliteProvider::open(&path).unwrap();
    for folder in ["/offline/SFX/Audio", "/offline/SFX/audio"] {
        local
            .ensure_scan_folder(Path::new(folder), Path::new(folder))
            .unwrap();
    }
    let folders = local.folders(None).unwrap();
    assert_eq!(folders.len(), 2);
    assert!(
        folders
            .iter()
            .any(|folder| folder.path == "/offline/SFX/Audio")
    );
    assert!(
        folders
            .iter()
            .any(|folder| folder.path == "/offline/SFX/audio")
    );
}

#[cfg(any(unix, windows))]
fn wav(path: &Path) {
    let mut bytes = b"RIFF".to_vec();
    bytes.extend(38u32.to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16u32.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(48000u32.to_le_bytes());
    bytes.extend(96000u32.to_le_bytes());
    bytes.extend(2u16.to_le_bytes());
    bytes.extend(16u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend(2u32.to_le_bytes());
    bytes.extend(0i16.to_le_bytes());
    fs::write(path, bytes).unwrap();
}

#[cfg(windows)]
#[test]
fn native_windows_case_insensitive_aliases_keep_ids_and_unicode_folders() {
    use audiolib_db::{ScanCancellation, ScanOptions, ScanService, display_path};
    use std::sync::Arc;
    let root = tempfile::tempdir().unwrap();
    let sounds = root.path().join("Äudio");
    let alias = root.path().join("äudio");
    fs::create_dir(&sounds).unwrap();
    if !alias.exists() {
        eprintln!("case-insensitive Windows filesystem unavailable");
        return;
    }
    wav(&sounds.join("Boom.wav"));
    let provider = Arc::new(SqliteProvider::open(&root.path().join("library.db")).unwrap());
    let scanner = ScanService::new(provider.clone());
    let scan = || {
        scanner
            .scan(
                &sounds,
                &ScanOptions::default(),
                &ScanCancellation::default(),
                |_| {},
            )
            .unwrap()
    };
    assert_eq!(scan().added, 1);
    let id = provider
        .find_audio_by_path(&sounds.join("Boom.wav"))
        .unwrap()
        .unwrap()
        .id;
    provider
        .update_metadata(
            id,
            &MetadataPatch {
                notes: Some("retain ordinary Windows alias".into()),
                ..Default::default()
            },
        )
        .unwrap();
    // Emulate a historical stored spelling while both names still open one entry.
    support::connection(provider.database_path())
        .unwrap()
        .execute(
            "UPDATE audio_files SET file_path=?1 WHERE id=?2",
            rusqlite::params![display_path(&alias.join("boom.wav")), id],
        )
        .unwrap();
    provider.ensure_scan_folder(&alias, &alias).unwrap();
    assert_eq!(provider.folders(None).unwrap().len(), 1);
    let again = scan();
    assert_eq!((again.added, again.unchanged), (0, 1));
    assert_eq!(
        provider.get_audio(id).unwrap().notes,
        "retain ordinary Windows alias"
    );
}

#[cfg(windows)]
#[test]
fn native_windows_existing_case_sensitive_root_preserves_distinct_entries() {
    use audiolib_db::{ScanCancellation, ScanOptions, ScanService};
    use std::sync::Arc;
    let Some(base) = std::env::var_os("AUDIOLIB_CASE_SENSITIVE_TEST_ROOT") else {
        eprintln!(
            "AUDIOLIB_CASE_SENSITIVE_TEST_ROOT absent; existing sensitive-volume test not run"
        );
        return;
    };
    // Use only an owned child of an explicitly supplied existing directory. Never
    // change a directory's case-sensitivity or any other filesystem flags.
    let root = tempfile::Builder::new()
        .prefix("audiolib-case-")
        .tempdir_in(base)
        .unwrap();
    let sounds = root.path().join("Sounds");
    fs::create_dir(&sounds).unwrap();
    let upper = sounds.join("Foo.wav");
    let lower = sounds.join("foo.wav");
    wav(&upper);
    if lower.exists() {
        eprintln!("provided root does not expose distinct case-sensitive entries; not run");
        return;
    }
    let provider = Arc::new(SqliteProvider::open(&root.path().join("library.db")).unwrap());
    let scanner = ScanService::new(provider.clone());
    let scan = |incremental| {
        scanner
            .scan(
                &sounds,
                &ScanOptions {
                    incremental,
                    ..Default::default()
                },
                &ScanCancellation::default(),
                |_| {},
            )
            .unwrap()
    };
    assert_eq!(scan(false).added, 1);
    let upper_id = provider.find_audio_by_path(&upper).unwrap().unwrap().id;
    provider
        .update_metadata(
            upper_id,
            &MetadataPatch {
                notes: Some("retain uppercase entry".into()),
                ..Default::default()
            },
        )
        .unwrap();
    wav(&lower);
    assert_eq!(scan(false).added, 1);
    let lower_id = provider.find_audio_by_path(&lower).unwrap().unwrap().id;
    assert_ne!(upper_id, lower_id);
    assert_eq!(scan(true).unchanged, 2);
    fs::remove_file(&upper).unwrap();
    assert_eq!(scan(false).removed, 1);
    assert_eq!(
        provider.get_audio(upper_id).unwrap().notes,
        "retain uppercase entry"
    );
    assert!(!provider.get_audio(lower_id).unwrap().missing);
}

#[cfg(unix)]
#[test]
fn native_case_sensitive_scans_preserve_both_files_annotations_and_missing_scope() {
    use audiolib_db::{ScanCancellation, ScanOptions, ScanService};
    use std::sync::Arc;
    let root = tempfile::tempdir().unwrap();
    let upper = root.path().join("Audio");
    let lower = root.path().join("audio");
    fs::create_dir(&upper).unwrap();
    if lower.exists() {
        eprintln!("case-sensitive filesystem unavailable");
        return;
    }
    fs::create_dir(&lower).unwrap();
    wav(&upper.join("Foo.wav"));
    wav(&upper.join("foo.wav"));
    wav(&lower.join("foo.wav"));
    let provider = Arc::new(SqliteProvider::open(&root.path().join("library.db")).unwrap());
    let scanner = ScanService::new(provider.clone());
    let scan = |folder: &Path, incremental| {
        scanner
            .scan(
                folder,
                &ScanOptions {
                    incremental,
                    ..Default::default()
                },
                &ScanCancellation::default(),
                |_| {},
            )
            .unwrap()
    };
    assert_eq!(scan(&upper, false).added, 2);
    assert_eq!(scan(&lower, false).added, 1);
    let upper_file = provider
        .find_audio_by_path(&upper.join("Foo.wav"))
        .unwrap()
        .unwrap();
    let lower_file = provider
        .find_audio_by_path(&upper.join("foo.wav"))
        .unwrap()
        .unwrap();
    assert_ne!(upper_file.id, lower_file.id);
    provider
        .update_metadata(
            upper_file.id,
            &MetadataPatch {
                notes: Some("retain uppercase".into()),
                ..Default::default()
            },
        )
        .unwrap();
    provider
        .update_metadata(
            lower_file.id,
            &MetadataPatch {
                notes: Some("retain lowercase".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(scan(&upper, true).unchanged, 2);
    fs::remove_file(upper.join("Foo.wav")).unwrap();
    assert_eq!(scan(&upper, false).removed, 1);
    assert!(provider.get_audio(upper_file.id).unwrap().missing);
    assert_eq!(
        provider.get_audio(upper_file.id).unwrap().notes,
        "retain uppercase"
    );
    assert!(!provider.get_audio(lower_file.id).unwrap().missing);
    assert_eq!(
        provider.get_audio(lower_file.id).unwrap().notes,
        "retain lowercase"
    );
    assert!(
        !provider
            .find_audio_by_path(&lower.join("foo.wav"))
            .unwrap()
            .unwrap()
            .missing
    );
    wav(&upper.join("Foo.wav"));
    scan(&upper, true);
    assert_eq!(
        provider
            .find_audio_by_path(&upper.join("Foo.wav"))
            .unwrap()
            .unwrap()
            .id,
        upper_file.id
    );
}

#[cfg(unix)]
#[test]
fn native_case_insensitive_unicode_rename_keeps_identity_and_reconciles_new_spelling() {
    use audiolib_db::{ScanCancellation, ScanOptions, ScanService};
    use std::sync::Arc;
    let root = tempfile::tempdir().unwrap();
    let upper = root.path().join("Äudio");
    let lower = root.path().join("äudio");
    fs::create_dir(&upper).unwrap();
    if !lower.exists() {
        eprintln!("case-insensitive filesystem unavailable");
        return;
    }
    wav(&upper.join("old.wav"));
    let provider = Arc::new(SqliteProvider::open(&root.path().join("library.db")).unwrap());
    let scanner = ScanService::new(provider.clone());
    let scan = |folder: &Path| {
        scanner
            .scan(
                folder,
                &ScanOptions {
                    incremental: false,
                    ..Default::default()
                },
                &ScanCancellation::default(),
                |_| {},
            )
            .unwrap()
    };
    scan(&upper);
    let original = provider
        .find_audio_by_path(&upper.join("old.wav"))
        .unwrap()
        .unwrap();
    provider
        .update_metadata(
            original.id,
            &MetadataPatch {
                notes: Some("retain rename".into()),
                ..Default::default()
            },
        )
        .unwrap();
    fs::rename(&upper, &lower).unwrap();
    wav(&lower.join("new.wav"));
    assert_eq!(scan(&lower).added, 1);
    assert_eq!(provider.folders(None).unwrap().len(), 1);
    assert_eq!(
        provider
            .find_audio_by_path(&lower.join("old.wav"))
            .unwrap()
            .unwrap()
            .id,
        original.id
    );
    let new_id = provider
        .find_audio_by_path(&lower.join("new.wav"))
        .unwrap()
        .unwrap()
        .id;
    fs::remove_file(lower.join("new.wav")).unwrap();
    assert_eq!(scan(&lower).removed, 1);
    assert!(provider.get_audio(new_id).unwrap().missing);
    assert_eq!(
        provider.get_audio(original.id).unwrap().notes,
        "retain rename"
    );
}

#[cfg(unix)]
#[test]
fn native_case_distinct_hardlinks_remain_separate_library_entries() {
    use audiolib_db::{ScanCancellation, ScanOptions, ScanService};
    use std::sync::Arc;
    let root = tempfile::tempdir().unwrap();
    let sounds = root.path().join("sounds");
    fs::create_dir(&sounds).unwrap();
    let upper = sounds.join("Foo.wav");
    let lower = sounds.join("foo.wav");
    wav(&upper);
    if lower.exists() {
        eprintln!("case-sensitive filesystem unavailable");
        return;
    }
    fs::hard_link(&upper, &lower).unwrap();
    let provider = Arc::new(SqliteProvider::open(&root.path().join("library.db")).unwrap());
    let scanner = ScanService::new(provider.clone());
    let scan = |incremental| {
        scanner
            .scan(
                &sounds,
                &ScanOptions {
                    incremental,
                    ..Default::default()
                },
                &ScanCancellation::default(),
                |_| {},
            )
            .unwrap()
    };
    assert_eq!(scan(false).added, 2);
    let upper_id = provider.find_audio_by_path(&upper).unwrap().unwrap().id;
    let lower_id = provider.find_audio_by_path(&lower).unwrap().unwrap().id;
    assert_ne!(upper_id, lower_id);
    provider
        .update_metadata(
            upper_id,
            &MetadataPatch {
                notes: Some("uppercase entry".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(scan(true).unchanged, 2);
    assert_eq!(provider.get_audio(lower_id).unwrap().notes, "");
    fs::remove_file(&upper).unwrap();
    assert_eq!(scan(false).removed, 1);
    assert_eq!(
        provider.get_audio(upper_id).unwrap().notes,
        "uppercase entry"
    );
    assert!(!provider.get_audio(lower_id).unwrap().missing);
}

#[cfg(unix)]
#[test]
fn native_posix_backslashes_remain_literal_folder_and_file_characters() {
    use audiolib_db::{ScanCancellation, ScanOptions, ScanService, folder_path_text};
    use std::sync::Arc;
    let root = tempfile::tempdir().unwrap();
    let sounds = root.path().join(r"Audio\Raw");
    fs::create_dir(&sounds).unwrap();
    let file = sounds.join(r"Foo\Bar.wav");
    wav(&file);
    assert!(folder_path_text(&sounds).ends_with(r"Audio\Raw"));
    let provider = Arc::new(SqliteProvider::open(&root.path().join("library.db")).unwrap());
    let scanner = ScanService::new(provider.clone());
    let scan = |incremental| {
        scanner
            .scan(
                &sounds,
                &ScanOptions {
                    incremental,
                    ..Default::default()
                },
                &ScanCancellation::default(),
                |_| {},
            )
            .unwrap()
    };
    assert_eq!(scan(false).added, 1);
    assert_eq!(
        provider.folders(None).unwrap()[0].path,
        folder_path_text(&sounds.canonicalize().unwrap())
    );
    let id = provider.find_audio_by_path(&file).unwrap().unwrap().id;
    assert_eq!(provider.get_audio(id).unwrap().filename, r"Foo\Bar.wav");
    assert!(!sounds.join("Foo").exists());
    assert_eq!(scan(true).unchanged, 1);
    fs::remove_file(&file).unwrap();
    // An empty scan root must not mark files missing: an unmounted volume looks
    // the same. A non-audio sibling keeps this deletion inside reconciliation.
    fs::write(sounds.join("keep.txt"), b"keep").unwrap();
    assert_eq!(scan(false).removed, 1);
    assert!(provider.get_audio(id).unwrap().missing);
}
