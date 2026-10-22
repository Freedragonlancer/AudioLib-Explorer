// crates/audiolib-update/src/tests.rs
//! Regression coverage for release selection and update data boundaries.
use super::*;
use std::io::Write;

const NATIVE_PRODUCT: Product = if cfg!(windows) {
    Product::Desktop
} else {
    Product::Server
};

fn release(tag: &str, draft: bool, prerelease: bool) -> Release {
    release_for_platform(
        NATIVE_PRODUCT,
        tag,
        draft,
        prerelease,
        native_platform().unwrap(),
    )
}

fn release_for_platform(
    product: Product,
    tag: &str,
    draft: bool,
    prerelease: bool,
    platform: Platform,
) -> Release {
    let name = asset_names_for_platform(product, tag, platform).unwrap()[0].clone();
    Release {
        tag_name: tag.into(),
        body: Some("notes".into()),
        draft,
        prerelease,
        assets: [name.clone(), format!("{name}.sha256")]
            .into_iter()
            .map(|name| Asset {
                browser_download_url: format!(
                    "https://github.com/{REPOSITORY}/releases/download/{tag}/{name}"
                ),
                name,
                size: 128,
                digest: None,
            })
            .collect(),
    }
}

#[test]
fn releases_obey_numeric_beta_order_and_stable_channel() {
    let beta = UpdateClient::new(NATIVE_PRODUCT, Channel::Beta, "0.6.2-beta.2").unwrap();
    assert_eq!(
        beta.select(vec![
            release("v0.6.2-beta.9", false, true),
            release("v0.6.2-beta.10", false, true)
        ])
        .unwrap()
        .unwrap()
        .version,
        "0.6.2-beta.10"
    );
    assert_eq!(
        beta.select(vec![
            release("v0.6.2-beta.10", false, true),
            release("v0.6.2", false, false)
        ])
        .unwrap()
        .unwrap()
        .version,
        "0.6.2"
    );
    let stable = UpdateClient::new(NATIVE_PRODUCT, Channel::Stable, "0.6.1").unwrap();
    assert_eq!(
        stable
            .select(vec![
                release("v0.7.0-beta.1", false, true),
                release("v0.6.2", false, false)
            ])
            .unwrap()
            .unwrap()
            .version,
        "0.6.2"
    );
    assert!(
        stable
            .select(vec![release("v0.7.0", true, false)])
            .unwrap()
            .is_none()
    );
    assert!(
        stable
            .select(vec![release("v0.6.0", false, false)])
            .unwrap()
            .is_none()
    );
}

#[test]
fn wrong_repository_duplicate_and_incomplete_assets_are_ignored() {
    let client = UpdateClient::new(NATIVE_PRODUCT, Channel::Stable, "0.6.1").unwrap();
    let mut wrong = release("v0.6.2", false, false);
    wrong.assets[0].browser_download_url = wrong.assets[0]
        .browser_download_url
        .replace("Freedragonlancer", "attacker");
    assert!(client.select(vec![wrong]).unwrap().is_none());
    let mut duplicate = release("v0.6.2", false, false);
    duplicate.assets.push(duplicate.assets[0].clone());
    assert!(client.select(vec![duplicate]).unwrap().is_none());
    let mut incomplete = release("v0.6.2", false, false);
    incomplete.assets.pop();
    assert!(client.select(vec![incomplete]).unwrap().is_none());
}

#[test]
fn historical_release_names_without_v_and_lowercase_server_are_supported() {
    for product in [Product::Desktop, Product::Server] {
        let mut release =
            release_for_platform(product, "v0.6.2", false, false, Platform::WindowsX64);
        let historical =
            asset_names_for_platform(product, "v0.6.2", Platform::WindowsX64).unwrap()[1].clone();
        for (index, asset) in release.assets.iter_mut().enumerate() {
            asset.name = if index == 0 {
                historical.clone()
            } else {
                format!("{historical}.sha256")
            };
            asset.browser_download_url = format!(
                "https://github.com/{REPOSITORY}/releases/download/v0.6.2/{}",
                asset.name
            );
        }
        let client = UpdateClient::new(product, Channel::Stable, "0.6.1").unwrap();
        let update = client
            .select_for_platform(vec![release], Platform::WindowsX64)
            .unwrap()
            .unwrap();
        assert_eq!(update.asset.name, historical);
    }
}

#[test]
fn known_binary_only_mac_archives_are_not_offered_as_installable_updates() {
    let client = UpdateClient::new(Product::Server, Channel::Beta, "0.5.0-0").unwrap();
    for tag in ["v0.5.0", "v0.5.1", "v0.6.0", "v0.6.1", "v0.6.2"] {
        let mut legacy =
            release_for_platform(Product::Server, tag, false, false, Platform::MacArm64);
        let archive_name = format!("audiolib-server-{}-macos-arm64.tar.gz", &tag[1..]);
        for (index, asset) in legacy.assets.iter_mut().enumerate() {
            asset.name = if index == 0 {
                archive_name.clone()
            } else {
                format!("{archive_name}.sha256")
            };
            asset.browser_download_url = format!(
                "https://github.com/{REPOSITORY}/releases/download/{tag}/{}",
                asset.name
            );
        }
        assert!(
            client
                .select_for_platform(vec![legacy], Platform::MacArm64)
                .unwrap()
                .is_none(),
            "offered known binary-only archive {tag}"
        );
        let flat = release_for_platform(Product::Server, tag, false, false, Platform::MacArm64);
        assert_eq!(
            client
                .select_for_platform(vec![flat], Platform::MacArm64)
                .unwrap()
                .unwrap()
                .tag,
            tag,
            "canonical full package must remain eligible"
        );
    }
    let mut future =
        release_for_platform(Product::Server, "v0.6.3", false, false, Platform::MacArm64);
    for asset in &mut future.assets {
        asset.name = asset.name.replace("AudioLibServer-v", "audiolib-server-");
        asset.browser_download_url = format!(
            "https://github.com/{REPOSITORY}/releases/download/v0.6.3/{}",
            asset.name
        );
    }
    assert!(
        client
            .select_for_platform(vec![future], Platform::MacArm64)
            .unwrap()
            .is_some(),
        "do not ban future lowercase flat packages"
    );
}

#[test]
fn historical_binary_only_mac_layout_cannot_bypass_public_package_validation() {
    let root = tempfile::tempdir().unwrap();
    let archive = root.path().join("legacy.tar.gz");
    let encoder = flate2::write::GzEncoder::new(
        std::fs::File::create(&archive).unwrap(),
        flate2::Compression::default(),
    );
    let mut tar = tar::Builder::new(encoder);
    for name in [
        "._audiolib-server-0.6.2-macos-arm64",
        "audiolib-server-0.6.2-macos-arm64/audiolib-server",
    ] {
        let mut header = tar::Header::new_ustar();
        header.set_path(name).unwrap();
        header.set_size(3);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append(&header, b"old".as_slice()).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap();
    let payload = root.path().join("payload");
    std::fs::create_dir(&payload).unwrap();
    assert!(package::extract(&archive, &payload, Product::Server, "legacy.tar.gz").is_err());
    assert!(!payload.join("audiolib-server").exists());
    assert!(!payload.join("audiolib-server.exe").exists());
}

#[test]
fn staged_updates_exclude_another_updater_for_the_same_installation() {
    let root = tempfile::tempdir().unwrap();
    let first_stage = tempfile::Builder::new()
        .prefix(".audiolib-update-")
        .tempdir_in(root.path())
        .unwrap();
    let first = PreparedUpdate::new(first_stage, root.path(), NATIVE_PRODUCT).unwrap();
    let second_stage = tempfile::Builder::new()
        .prefix(".audiolib-update-")
        .tempdir_in(root.path())
        .unwrap();
    assert!(PreparedUpdate::new(second_stage, root.path(), NATIVE_PRODUCT).is_err());
    drop(first);
    let third_stage = tempfile::Builder::new()
        .prefix(".audiolib-update-")
        .tempdir_in(root.path())
        .unwrap();
    assert!(PreparedUpdate::new(third_stage, root.path(), NATIVE_PRODUCT).is_ok());
    assert!(root.path().join(".audiolib-update.lock").is_file());
}

#[test]
fn tar_pax_paths_are_validated_and_bounded() {
    for malicious in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("update.tar.gz");
        let encoder = flate2::write::GzEncoder::new(
            std::fs::File::create(&archive).unwrap(),
            flate2::Compression::default(),
        );
        let mut tar = tar::Builder::new(encoder);
        if malicious {
            tar.append_pax_extensions([("path", b"../LICENSE".as_slice())])
                .unwrap();
        } else {
            tar.append_pax_extensions([("mtime", b"1700000000.12345".as_slice())])
                .unwrap();
        }
        for name in package::required(NATIVE_PRODUCT) {
            let mut header = tar::Header::new_ustar();
            header.set_path(name).unwrap();
            header.set_size(3);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append(&header, b"new".as_slice()).unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap();
        let payload = root.path().join("payload");
        std::fs::create_dir(&payload).unwrap();
        assert_eq!(
            package::extract(&archive, &payload, NATIVE_PRODUCT, "update.tar.gz").is_ok(),
            !malicious
        );
        assert!(!root.path().join("LICENSE").exists());
    }
}

#[test]
fn checksum_requires_exact_filename_and_single_line() {
    let good = format!("{}  expected.zip\n", "Ab".repeat(32));
    assert_eq!(
        package::parse_checksum(good.as_bytes(), "expected.zip").unwrap(),
        "ab".repeat(32)
    );
    assert!(package::parse_checksum(good.as_bytes(), "other.zip").is_err());
    assert!(package::parse_checksum(format!("{good}{good}").as_bytes(), "expected.zip").is_err());
    assert!(package::parse_checksum(b"abc expected.zip", "expected.zip").is_err());
}

#[test]
fn update_paths_cannot_target_runtime_data_or_traverse() {
    for name in [
        "../LICENSE",
        "/LICENSE",
        "C:/LICENSE",
        "doc/../LICENSE",
        "doc\\LICENSE",
        "settings.json",
        "library.db",
        "config/rust_settings.json",
        "licenses/evil.exe",
        "licenses/library.db-wal",
        "licenses/library.sqlite3-shm",
        "licenses/tokens.json",
        "licenses/NUL",
        "licenses/NUL.txt",
        "licenses/com1/LICENSE",
        "licenses/LPt9.md",
        "licenses/COM¹.txt",
        "licenses/notice?.txt",
        "LICENSE.",
        "doc//INDEX.md",
        "doc/INDEX.md:stream",
    ] {
        assert!(!package::allowed(Product::Desktop, name), "accepted {name}");
    }
    assert!(package::allowed(
        Product::Desktop,
        "config/rust_settings.example.json"
    ));
    assert!(package::allowed(
        Product::Desktop,
        "licenses/serde-1.0/LICENSE-MIT"
    ));
}

#[test]
fn installed_beta_marker_is_bound_to_binary_core_version() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("update-channel.txt"), "beta\n").unwrap();
    std::fs::write(root.path().join("update-version.txt"), "0.6.2-beta.12\n").unwrap();
    let client = UpdateClient::for_install(Product::Desktop, "0.6.2", root.path()).unwrap();
    assert_eq!(client.channel, Channel::Beta);
    assert_eq!(client.current.to_string(), "0.6.2-beta.12");
    assert!(UpdateClient::for_install(Product::Desktop, "0.6.1", root.path()).is_err());
}

#[test]
fn zip_rejects_private_and_symlink_entries_before_deployment() {
    for (name, symlink) in [
        ("../LICENSE", false),
        ("settings.json", false),
        ("LICENSE", true),
    ] {
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("update.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
        if symlink {
            zip.add_symlink(name, "../secret", zip::write::SimpleFileOptions::default())
                .unwrap();
        } else {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"bad").unwrap();
        }
        zip.finish().unwrap();
        let payload = root.path().join("payload");
        std::fs::create_dir(&payload).unwrap();
        assert!(package::extract(&archive, &payload, Product::Desktop, "update.zip").is_err());
        assert!(!root.path().join("LICENSE").exists());
    }
}

#[test]
fn deployment_and_rollback_preserve_private_files() {
    for rollback in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let install = root.path().join("install");
        let stage = root.path().join("stage");
        std::fs::create_dir(&install).unwrap();
        std::fs::create_dir_all(stage.join("payload")).unwrap();
        for name in package::required(Product::Desktop) {
            std::fs::write(install.join(name), b"old").unwrap();
            std::fs::write(stage.join("payload").join(name), b"new").unwrap();
        }
        std::fs::write(install.join("settings.json"), b"user settings").unwrap();
        std::fs::write(install.join("library.db"), b"user database").unwrap();
        deploy::exercise_deploy(&install, &stage, rollback).unwrap();
        assert_eq!(
            std::fs::read(install.join("AudioLibExplorer.exe")).unwrap(),
            if rollback { b"old" } else { b"new" }
        );
        assert_eq!(
            std::fs::read(install.join("settings.json")).unwrap(),
            b"user settings"
        );
        assert_eq!(
            std::fs::read(install.join("library.db")).unwrap(),
            b"user database"
        );
    }
}

fn fixture_response(body: Vec<u8>) -> (String, std::thread::JoinHandle<()>) {
    use std::io::Read;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let thread = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut header = Vec::new();
        let mut byte = [0];
        while !header.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).unwrap();
            header.push(byte[0]);
        }
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(response.as_bytes()).unwrap();
        let _ = stream.write_all(&body);
    });
    (format!("http://{address}/fixture"), thread)
}

#[test]
fn http_download_enforces_checksum_declared_size_and_response_bounds() {
    use sha2::{Digest, Sha256};
    let body = b"verified payload".to_vec();
    let good = format!("{:x}", Sha256::digest(&body));
    for (size, checksum, succeeds) in [
        (body.len() as u64, good.clone(), true),
        (body.len() as u64, "0".repeat(64), false),
        (body.len() as u64 - 1, good.clone(), false),
        (body.len() as u64 + 1, good, false),
    ] {
        let root = tempfile::tempdir().unwrap();
        let (url, fixture) = fixture_response(body.clone());
        let mut client = UpdateClient::new(NATIVE_PRODUCT, Channel::Stable, "0.0.0").unwrap();
        // Only this private fixture replaces the TLS-only production agent.
        client.agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(5)))
            .build()
            .into();
        let asset = Asset {
            name: "fixture.zip".into(),
            browser_download_url: url,
            size,
            digest: None,
        };
        assert_eq!(
            client
                .download(&asset, &root.path().join("archive"), &checksum)
                .is_ok(),
            succeeds
        );
        fixture.join().unwrap();
    }
    let (url, fixture) = fixture_response(vec![b'x'; 4097]);
    let mut client = UpdateClient::new(NATIVE_PRODUCT, Channel::Stable, "0.0.0").unwrap();
    client.agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(5)))
        .build()
        .into();
    assert!(client.fetch(&url, 4096, Duration::from_secs(5)).is_err());
    fixture.join().unwrap();
}
