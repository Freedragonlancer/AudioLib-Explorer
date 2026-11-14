// app/audiolib-desktop/src/ai_search_tests.rs
//! Real controller callbacks with an owned library and no external provider traffic.
use super::*;

#[test]
fn new_ai_profiles_stay_unsaved_until_saved_and_preserve_their_drafts() {
    isolated("ai_search::child_unsaved_provider");
}

#[test]
#[ignore = "runs in an owned isolated child"]
fn child_unsaved_provider() {
    use audiolib_ai::{config::Profile, storage::ProfileStore};
    use i_slint_backend_testing::ElementHandle;
    let fixture = fixture();
    let ui = &fixture.ui;
    let preferences = fixture.root.join("preferences");
    let mut store = ProfileStore::open(&preferences).unwrap();
    store
        .save(
            Profile {
                name: "Existing profile".into(),
                model: "saved-model".into(),
                ..Default::default()
            },
            None,
        )
        .unwrap();
    let retained = store.selected().unwrap().clone();
    drop(store);
    ui.show().unwrap();
    ui.invoke_ai_action(22, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    assert_eq!(ui.get_ai_provider_index(), 1);
    let new = ElementHandle::find_by_accessible_label(ui, "New provider preset")
        .next()
        .unwrap();
    // This fixture uses the real clock; mock_single_click advances only Slint's
    // clock and would make the next real-time timer appear to move backwards.
    let origin = new.absolute_position();
    let size = new.size();
    let position =
        slint::LogicalPosition::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0);
    ui.window()
        .dispatch_event(slint::platform::WindowEvent::PointerMoved { position });
    ui.window()
        .dispatch_event(slint::platform::WindowEvent::PointerPressed {
            position,
            button: slint::platform::PointerEventButton::Left,
        });
    ui.window()
        .dispatch_event(slint::platform::WindowEvent::PointerReleased {
            position,
            button: slint::platform::PointerEventButton::Left,
        });
    wait_for(ui, |ui| ui.get_ai_provider_index() == 2);
    let selector = ElementHandle::find_by_accessible_label(ui, "AI provider preset")
        .next()
        .unwrap();
    assert_eq!(selector.accessible_value().as_deref(), Some("Unsaved"));
    assert!(ui.get_ai_profile_id().is_empty());
    assert!(ui.get_ai_selected_model().is_empty());
    assert!(ui.get_ai_dirty());
    ui.set_ai_profile_name("Retained draft".into());
    ui.set_ai_model("draft-model".into());
    ui.set_ai_key("owned-unsaved-key".into());
    // Real ComboBox binding writes the requested index before its callback.
    ui.set_ai_provider_index(0);
    ui.invoke_ai_action(4, 0);
    assert!(ui.get_ai_model().is_empty());
    assert!(ui.get_ai_key().is_empty());
    ui.set_ai_provider_index(2);
    ui.invoke_ai_action(4, 2);
    assert_eq!(ui.get_ai_profile_name(), "Retained draft");
    assert_eq!(ui.get_ai_model(), "draft-model");
    assert_eq!(ui.get_ai_key(), "owned-unsaved-key");
    ui.invoke_ai_action(1, 0);
    ui.invoke_ai_action(22, 0);
    assert_eq!(ui.get_ai_provider_index(), 2);
    assert_eq!(ui.get_ai_model(), "draft-model");
    ui.set_chinese(true);
    ui.invoke_language_requested(true);
    let ready = fixture.controller.clone();
    wait_for(ui, move |ui| {
        !ui.get_searching()
            && fully_published(&ready)
            && ui.get_ai_providers().row_data(2).as_deref() == Some("未保存")
    });
    assert_eq!(ui.get_ai_key(), "owned-unsaved-key");
    ui.set_ai_prompt("wind".into());
    ui.invoke_ai_action(2, 0);
    assert!(ui.get_ai_error());
    assert!(ui.get_ai_status().contains("请先保存"));
    assert!(!ui.get_ai_busy());
    assert_eq!(
        ProfileStore::open(&preferences).unwrap().profiles(),
        std::slice::from_ref(&retained)
    );
    ui.set_ai_settings(true);
    ui.invoke_ai_action(18, 0);
    assert_eq!(ui.get_ai_provider_index(), 2);
    assert!(ui.get_ai_model().is_empty());
    assert!(ui.get_ai_key().is_empty());
    ui.set_ai_profile_name("New saved profile".into());
    ui.set_ai_model("new-model".into());
    ui.invoke_ai_action(8, 0);
    wait_for(ui, |ui| {
        !ui.get_ai_busy() && !ui.get_ai_profile_id().is_empty()
    });
    assert_eq!(ui.get_ai_provider_index(), 2);
    assert_eq!(
        ui.get_ai_providers().row_data(2).as_deref(),
        Some("New saved profile")
    );
    assert_eq!(ui.get_ai_selected_model(), "new-model");
    assert_eq!(
        ProfileStore::open(&preferences).unwrap().profiles()[0],
        retained
    );
    ui.invoke_ai_action(10, 0);
    wait_for(ui, |ui| {
        !ui.get_ai_busy() && ui.get_ai_provider_index() == 0
    });
    assert_eq!(ui.get_ai_providers().row_count(), 3);
    assert_eq!(
        ui.get_ai_providers().row_data(0).as_deref(),
        Some("本地词表（离线）")
    );
    ui.set_ai_model("implicit-new-draft".into());
    ui.invoke_ai_action(26, 0);
    assert_eq!(ui.get_ai_provider_index(), 2);
    assert_eq!(ui.get_ai_model(), "implicit-new-draft");
    ui.set_ai_provider_index(1);
    ui.invoke_ai_action(4, 1);
    ui.set_ai_model("edited-saved-model".into());
    ui.invoke_ai_action(26, 0);
    assert_eq!(ui.get_ai_provider_index(), 1);
    assert_eq!(ui.get_ai_selected_model(), "saved-model");
    finish(&fixture);
}

#[test]
fn provider_timeouts_show_the_configured_deadline_and_how_to_raise_it() {
    isolated("ai_search::child_provider_timeout_notice");
}

#[test]
#[ignore = "runs in an owned isolated child"]
fn child_provider_timeout_notice() {
    use std::io::Read;
    let fixture = fixture();
    let ui = &fixture.ui;
    ui.invoke_ai_action(22, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    ui.invoke_ai_action(9, 0);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    ui.set_ai_endpoint(format!("http://{}", listener.local_addr().unwrap()).into());
    ui.set_ai_model("owned-slow-model".into());
    ui.set_ai_timeout(2);
    let worker = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut request = Vec::new();
        let mut byte = [0];
        while !request.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
        }
        std::thread::sleep(Duration::from_millis(2200));
    });
    ui.invoke_ai_action(17, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    assert!(ui.get_ai_error());
    assert!(ui.get_ai_status().contains("after 2 seconds"));
    assert!(ui.get_ai_status().contains("advanced options"));
    assert!(ui.get_ai_status().contains("600"));
    assert!(
        ui.get_ai_providers()
            .row_data(ui.get_ai_provider_index().max(0) as usize)
            .as_deref()
            == Some("Unsaved")
    );
    assert!(!fixture.root.join("preferences/ai/profiles.json").exists());
    worker.join().unwrap();
    finish(&fixture);
}

#[test]
fn ai_server_mode_forwards_models_probes_and_generation_without_switching_library() {
    isolated("ai_search::child_ai_server_mode");
}

#[test]
#[ignore = "runs in an owned isolated child"]
fn child_ai_server_mode() {
    use audiolib_ai::relay::{RelayRequest, RelayResponse};
    use audiolib_ai::{
        AiError,
        config::{Profile, RequestMode},
        provider,
        reference::ReferenceIndex,
        storage::ProfileStore,
    };
    use std::io::{Read, Write};
    let fixture = fixture();
    let ui = &fixture.ui;
    let relay = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let provider = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    provider.set_nonblocking(true).unwrap();
    let relay_url = format!("http://{}", relay.local_addr().unwrap());
    let provider_url = format!("http://{}/v1", provider.local_addr().unwrap());
    let (root, settings) = {
        use audiolib_core::profiles::ActiveSource;
        let mut config = fixture.controller.config.lock().unwrap();
        let settings = &mut config.data_source;
        let id = settings
            .add_profile("AI relay fixture", &relay_url)
            .unwrap();
        settings
            .set_active(&ActiveSource::Profile(id.clone()))
            .unwrap();
        settings.set_active(&ActiveSource::Local).unwrap();
        let settings = settings.clone();
        let root = config.data_root();
        let mut secrets = audiolib_remote::RemoteSecrets::default();
        secrets.set_token(&id, "owned-server-token");
        audiolib_remote::save_secrets(&root, &secrets).unwrap();
        (root, settings)
    };
    let before = serde_json::to_value(&settings).unwrap();
    let retained_provider = fixture.controller.provider();
    let (sent, received) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        relay.set_nonblocking(true).unwrap();
        for index in 0..4 {
            let started = Instant::now();
            let mut stream = loop {
                match relay.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(started.elapsed() < Duration::from_secs(10));
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("owned relay accept: {error}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut head = Vec::new();
            let mut byte = [0];
            while !head.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                head.push(byte[0]);
            }
            let head = String::from_utf8(head).unwrap();
            assert!(
                head.to_ascii_lowercase()
                    .contains("authorization: bearer owned-server-token")
            );
            let length = head
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|value| value.trim().parse::<usize>().unwrap())
                })
                .unwrap();
            let mut body = vec![0; length];
            stream.read_exact(&mut body).unwrap();
            let request: RelayRequest = serde_json::from_slice(&body).unwrap();
            request.validate().unwrap();
            assert_eq!(request.profile.timeout_seconds, 180);
            let response = if index == 0 {
                assert!(head.starts_with("POST /api/v1/ai/models "));
                RelayResponse::from_models(Ok(vec!["owned-model".into()]))
            } else {
                if index < 3 {
                    assert!(head.starts_with("POST /api/v1/ai/generate "));
                } else {
                    assert!(head.starts_with("POST /api/v1/ai/models "));
                }
                RelayResponse::from_output(Ok(provider::Output {
                    plan: ReferenceIndex::embedded()
                        .local_plan("wind", false)
                        .unwrap(),
                    input_tokens: 7,
                    output_tokens: 3,
                    decision: None,
                }))
            };
            let body = serde_json::to_vec(&response).unwrap();
            let status = if index == 3 {
                "404 Not Found"
            } else {
                "200 OK"
            };
            write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
            stream.write_all(&body).unwrap();
            sent.send(()).unwrap();
        }
    });
    ui.invoke_ai_action(22, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    ui.invoke_ai_action(9, 0);
    assert_eq!(ui.get_ai_request_mode(), 0);
    assert_eq!(ui.get_ai_timeout(), 180);
    ui.set_ai_request_mode(1);
    ui.set_ai_endpoint(provider_url.into());
    ui.set_ai_key("owned-provider-key".into());
    ui.invoke_ai_action(11, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    assert!(!ui.get_ai_error(), "{}", ui.get_ai_status());
    assert_eq!(
        ui.get_ai_models().row_data(0).as_deref(),
        Some("owned-model")
    );
    ui.set_ai_model("owned-model".into());
    ui.invoke_ai_action(17, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    assert!(!ui.get_ai_error(), "{}", ui.get_ai_status());
    ui.invoke_ai_action(8, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    let stored = ProfileStore::open(&root).unwrap();
    assert_eq!(stored.selected().unwrap().request_mode, RequestMode::Server);
    drop(stored);
    ui.set_ai_auto_search(false);
    ui.set_ai_prompt("wind".into());
    ui.invoke_ai_action(2, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    assert!(!ui.get_ai_error(), "{}", ui.get_ai_status());
    assert!(ui.get_ai_rows().row_count() > 0);
    // An older server must show its own failure, never contact the provider here.
    ui.invoke_ai_action(11, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    assert!(ui.get_ai_error());
    assert!(ui.get_ai_status().contains("upgrade the server"));
    worker.join().unwrap();
    assert_eq!(received.try_iter().count(), 4);
    assert_eq!(
        provider.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    assert!(Arc::ptr_eq(
        &retained_provider,
        &fixture.controller.provider()
    ));
    assert_eq!(
        serde_json::to_value(&fixture.controller.config.lock().unwrap().data_source).unwrap(),
        before
    );
    assert!(!fixture.controller.remote_mode.load(Ordering::Acquire));
    // A queued admission must not combine its old endpoint with credentials
    // saved for the same profile's replacement server.
    {
        let mut config = fixture.controller.config.lock().unwrap();
        let id = config.data_source.legacy_profile_id().unwrap();
        config.data_source.profile_mut(&id).unwrap().server_url =
            format!("http://{}", provider.local_addr().unwrap());
        config.data_source.server_url = config.data_source.profile(&id).unwrap().server_url.clone();
        let mut secrets = audiolib_remote::RemoteSecrets::default();
        secrets.set_token(&id, "owned-replacement-token");
        audiolib_remote::save_secrets(&root, &secrets).unwrap();
    }
    let admitted = Profile {
        request_mode: RequestMode::Server,
        ..Default::default()
    };
    assert!(matches!(
        fixture.controller.ai_transport(&admitted, &root, &settings),
        Err(AiError::Cancelled)
    ));
    assert_eq!(
        provider.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    finish(&fixture);
}

#[test]
fn ai_server_mode_requires_a_configured_server_without_direct_fallback() {
    isolated("ai_search::child_ai_server_missing");
}

#[test]
#[ignore = "runs in an owned isolated child"]
fn child_ai_server_missing() {
    let fixture = fixture();
    let ui = &fixture.ui;
    let provider = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    provider.set_nonblocking(true).unwrap();
    ui.invoke_ai_action(22, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    ui.invoke_ai_action(9, 0);
    ui.set_ai_request_mode(1);
    ui.set_ai_endpoint(format!("http://{}/v1", provider.local_addr().unwrap()).into());
    ui.set_ai_model("owned-model".into());
    for action in [11, 17, 8, 2] {
        ui.set_ai_prompt("wind".into());
        ui.invoke_ai_action(action, 0);
        wait_for(ui, |ui| !ui.get_ai_busy());
        if action == 8 {
            assert!(!ui.get_ai_error(), "{}", ui.get_ai_status());
        } else {
            assert!(ui.get_ai_error());
            assert!(ui.get_ai_status().contains("Configure an AudioLib server"));
        }
    }
    assert_eq!(
        provider.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    finish(&fixture);
}

#[test]
fn ai_relay_never_forwards_a_token_from_a_failed_server_settings_save() {
    isolated("ai_search::child_ai_relay_failed_settings_save");
}

#[test]
#[ignore = "runs in an owned isolated child"]
fn child_ai_relay_failed_settings_save() {
    use audiolib_core::config::ActiveSource;
    use audiolib_remote::{RemoteSecrets, load_profile_secrets, save_profile_secrets};
    let fixture = fixture();
    let ui = &fixture.ui;
    let old_server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let replacement_server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    old_server.set_nonblocking(true).unwrap();
    replacement_server.set_nonblocking(true).unwrap();
    let (root, committed, id, unrelated) = {
        let mut config = fixture.controller.config.lock().unwrap();
        let id = config
            .data_source
            .add_profile(
                "Committed server",
                &format!("http://{}", old_server.local_addr().unwrap()),
            )
            .unwrap();
        config
            .data_source
            .set_active(&ActiveSource::Profile(id.clone()))
            .unwrap();
        config.data_source.set_active(&ActiveSource::Local).unwrap();
        let unrelated = config
            .data_source
            .add_profile("Unrelated server", "https://unrelated.example.test/Audio")
            .unwrap();
        config.save().unwrap();
        (
            config.data_root(),
            config.data_source.clone(),
            id,
            unrelated,
        )
    };
    let mut secrets = RemoteSecrets::default();
    secrets.set_token(&id, "owned-committed-token");
    secrets.set_token(&unrelated, "owned-unrelated-token");
    save_profile_secrets(&root, &committed, &mut secrets).unwrap();
    let mut edited = committed.clone();
    let replacement_url = format!("http://{}", replacement_server.local_addr().unwrap());
    edited.profile_mut(&id).unwrap().server_url = replacement_url.clone();
    edited.server_url = replacement_url;
    secrets.set_token(&id, "owned-replacement-token");
    // Both paths are in this fixture's preferences directory. A directory in
    // place of settings.json deterministically rejects its atomic replacement,
    // while remote/secrets.bin remains writable on Windows and macOS.
    let settings_path = root.join("settings.json");
    let retained = root.join("settings-before-failure.json");
    std::fs::rename(&settings_path, &retained).unwrap();
    std::fs::create_dir(&settings_path).unwrap();
    let result = fixture
        .controller
        .save_data_source(edited.clone(), secrets, false, true);
    std::fs::remove_dir(&settings_path).unwrap();
    std::fs::rename(&retained, &settings_path).unwrap();
    assert!(result.is_err());
    assert_eq!(
        fixture.controller.config.lock().unwrap().data_source,
        committed
    );
    assert_eq!(
        load_profile_secrets(&root, &edited).unwrap().token_for(&id),
        "owned-replacement-token"
    );
    let masked = load_profile_secrets(&root, &committed).unwrap();
    assert!(masked.token_for(&id).is_empty());
    assert_eq!(masked.token_for(&unrelated), "owned-unrelated-token");
    // Reloading the committed settings models a fresh process: it must reject
    // the mismatched persisted token just like the still-open controller.
    let restarted = AppConfig::load_in(root.clone()).unwrap();
    assert_eq!(restarted.data_source, committed);
    let mut restarted_secrets = load_profile_secrets(&root, &restarted.data_source).unwrap();
    assert!(restarted_secrets.token_for(&id).is_empty());
    assert_eq!(
        restarted_secrets.token_for(&unrelated),
        "owned-unrelated-token"
    );
    ui.invoke_ai_action(22, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    ui.invoke_ai_action(9, 0);
    ui.set_ai_request_mode(1);
    ui.set_ai_timeout(2);
    ui.invoke_ai_action(11, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    assert!(ui.get_ai_error());
    assert!(ui.get_ai_status().contains("Configure an AudioLib server"));
    assert_eq!(
        old_server.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    assert_eq!(
        replacement_server.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    // Repairing the failed profile must retain other valid profiles' keys.
    restarted_secrets.set_token(&id, "owned-recovered-token");
    fixture
        .controller
        .save_data_source(committed.clone(), restarted_secrets, false, true)
        .unwrap();
    let recovered = load_profile_secrets(&root, &committed).unwrap();
    assert_eq!(recovered.token_for(&id), "owned-recovered-token");
    assert_eq!(recovered.token_for(&unrelated), "owned-unrelated-token");
    finish(&fixture);
}

fn owned_ai_plan(
    queries: &[(&str, audiolib_core::ai_search::AiQueryKind)],
) -> audiolib_core::ai_search::AiQueryPlan {
    audiolib_core::ai_search::AiQueryPlan {
        schema_version: 1,
        intent: "owned layered sound".into(),
        queries: queries
            .iter()
            .map(|(text, kind)| audiolib_core::ai_search::AiQuery {
                text: (*text).into(),
                reason: format!("Owned recommendation for {text}"),
                kind: *kind,
                // The presentation label deliberately cannot distinguish roles.
                layer: "owned shared label".into(),
                source_ids: Vec::new(),
                cat_ids: Vec::new(),
            })
            .collect(),
        cautions: Vec::new(),
    }
}

fn seed_owned_ai_terms(fixture: &Fixture, terms: &[&str]) {
    let file = fixture
        .provider
        .search(&SearchRequest {
            query: "gore one".into(),
            ..Default::default()
        })
        .unwrap()
        .results
        .remove(0);
    fixture
        .provider
        .update_metadata(
            file.id,
            &MetadataPatch {
                notes: Some(terms.join(" ")),
                ..Default::default()
            },
        )
        .unwrap();
}

type AiResponseGate = (
    std::sync::mpsc::SyncSender<()>,
    std::sync::mpsc::Receiver<()>,
);

// A finite owned provider exercises parsing, actual-library verification and UI
// publication together. The optional gate holds a reply after receiving its POST.
fn install_owned_ai_provider(
    fixture: &Fixture,
    plans: Vec<audiolib_core::ai_search::AiQueryPlan>,
    gate: Option<AiResponseGate>,
) -> std::thread::JoinHandle<()> {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let worker = std::thread::spawn(move || {
        for plan in plans {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut head = Vec::new();
            let mut byte = [0];
            while !head.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                head.push(byte[0]);
            }
            let header = String::from_utf8(head).unwrap();
            assert!(header.starts_with("POST /chat/completions HTTP/1.1"));
            let length = header
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|value| value.trim().parse::<usize>().unwrap())
                })
                .unwrap();
            let mut body = vec![0; length];
            stream.read_exact(&mut body).unwrap();
            let _: serde_json::Value = serde_json::from_slice(&body).unwrap();
            if let Some((accepted, released)) = &gate {
                accepted.send(()).unwrap();
                released.recv_timeout(Duration::from_secs(5)).unwrap();
            }
            let reply = serde_json::json!({
                "choices": [{"finish_reason": "stop", "message": {
                    "content": serde_json::to_string(&plan).unwrap()
                }}]
            })
            .to_string();
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}", reply.len()).unwrap();
        }
    });
    let ui = &fixture.ui;
    ui.invoke_ai_action(0, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    ui.invoke_ai_action(9, 0);
    ui.set_ai_protocol(0);
    ui.invoke_ai_action(12, 0);
    ui.set_ai_endpoint(endpoint.into());
    ui.set_ai_model("owned-layered-model".into());
    ui.set_ai_key("owned-layered-key".into());
    ui.invoke_ai_action(8, 0);
    wait_for(ui, |ui| {
        !ui.get_ai_busy() && ui.get_ai_provider_index() == 1
    });
    worker
}

// Fixed owned replies exercise current errors and cancellation without exposing
// raw provider bodies or contacting an external service.
fn owned_ai_error_server(
    replies: Vec<(u16, &'static str, Option<AiResponseGate>)>,
) -> (
    String,
    std::sync::mpsc::Receiver<serde_json::Value>,
    std::thread::JoinHandle<()>,
) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (observed, requests) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        for (status, body, gate) in replies {
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
            let header = String::from_utf8(header).unwrap();
            let length = header
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|value| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or_default();
            let mut request = vec![0; length];
            stream.read_exact(&mut request).unwrap();
            observed
                .send(serde_json::from_slice(&request).unwrap_or(serde_json::Value::Null))
                .unwrap();
            if let Some((accepted, released)) = gate {
                accepted.send(()).unwrap();
                released.recv_timeout(Duration::from_secs(5)).unwrap();
            }
            if status != 0 {
                // A cancelled request may already have closed its socket.
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} Owned\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        }
    });
    (endpoint, requests, worker)
}

fn scifi_queries() -> Vec<String> {
    audiolib_ai::reference::ReferenceIndex::embedded()
        .local_plan("科幻爆炸", true)
        .unwrap()
        .queries
        .into_iter()
        .map(|query| query.text)
        .collect()
}

// Metadata-only matches retain the fixture's existing gore/wind result counts.
fn seed_ai_notes(fixture: &Fixture, scifi_count: usize) {
    let scifi = scifi_queries()
        .into_iter()
        .take(scifi_count)
        .collect::<Vec<_>>()
        .join(" ");
    let wind = audiolib_ai::reference::ReferenceIndex::embedded()
        .local_plan("风", true)
        .unwrap()
        .queries
        .into_iter()
        .map(|query| query.text)
        .collect::<Vec<_>>()
        .join(" ");
    for file in fixture
        .provider
        .search(&SearchRequest::default())
        .unwrap()
        .results
    {
        let notes = if file.filename == "gore one.wav" {
            Some(scifi.clone())
        } else if file.filename == "wind.wav" {
            Some(wind.clone())
        } else {
            None
        };
        if let Some(notes) = notes {
            fixture
                .provider
                .update_metadata(
                    file.id,
                    &MetadataPatch {
                        notes: Some(notes),
                        ..Default::default()
                    },
                )
                .unwrap();
        }
    }
}

// Real owned WAVs make Filename-only probes meaningful. The first candidate
// deliberately has no file. A later single-word plasma also matches its pair.
fn seed_ai_filename_hits(fixture: &Fixture) -> Vec<String> {
    let queries = scifi_queries()
        .into_iter()
        .skip(1)
        .take(3)
        .collect::<Vec<_>>();
    let sounds = fixture.root.join("sounds");
    let directory = sounds.join("gore");
    for query in &queries {
        let destination = directory.join(format!("{query}.wav"));
        assert_eq!(destination.parent(), Some(directory.as_path()));
        std::fs::copy(directory.join("gore one.wav"), destination).unwrap();
    }
    ScanService::new(fixture.provider.clone())
        .scan(
            &sounds,
            &ScanOptions::default(),
            &ScanCancellation::default(),
            |_| {},
        )
        .unwrap();
    for file in fixture
        .provider
        .search(&SearchRequest::default())
        .unwrap()
        .results
    {
        if queries
            .iter()
            .any(|query| file.filename == format!("{query}.wav"))
        {
            fixture
                .provider
                .update_metadata(
                    file.id,
                    &MetadataPatch {
                        tags: Some("gore".into()),
                        ..Default::default()
                    },
                )
                .unwrap();
            fixture
                .provider
                .compute_list_waveform(file.id, Arc::new(AtomicBool::new(false)))
                .unwrap();
        }
    }
    fixture.provider.rebuild_directory_search().unwrap();
    vec![
        "plasma".into(),
        queries[0].clone(),
        queries[1].clone(),
        queries[2].clone(),
    ]
}

fn assert_cached_ai_queries(fixture: &Fixture, offset: usize, queries: &[String]) {
    let cached = {
        let state = fixture.controller.task_workspace.lock().unwrap();
        state.workspace.session.tasks[offset..offset + queries.len()]
            .iter()
            .zip(queries)
            .map(|(task, query)| {
                assert_eq!(&task.definition.query, query);
                let cache = state
                    .caches
                    .get(&task.id)
                    .expect("each query must finish before the batch completes");
                assert!(!cache.records.is_empty());
                (
                    task.definition.clone(),
                    cache.limit,
                    cache.records.iter().map(|file| file.id).collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>()
    };
    for (definition, limit, actual) in cached {
        assert!(definition.query.split_whitespace().count() <= 2);
        let expected = fixture
            .provider
            .search(&definition.to_request(limit, 0))
            .unwrap()
            .results
            .into_iter()
            .map(|file| file.id)
            .collect::<Vec<_>>();
        assert_eq!(actual, expected);
    }
}

#[test]
fn ai_zero_hit_tag_is_removed_without_erasing_custom_queries_or_tasks() {
    isolated("ai_search::child_zero_hit_tag_pruning");
}

#[test]
fn ai_recommendations_survive_folder_filters_and_search_in_the_selected_directory() {
    isolated("ai_search::child_folder_filter_recommendations");
}

#[test]
fn ai_recommendations_replace_query_drafts_but_reject_invalid_filters() {
    isolated("ai_search::child_replace_query_drafts");
}

#[test]
#[ignore = "runs in an owned isolated child"]
fn child_replace_query_drafts() {
    let fixture = fixture();
    let ui = &fixture.ui;
    seed_ai_notes(&fixture, 5);
    search(&fixture, "", 4);
    fixture
        .controller
        .max_search_tasks
        .store(12, Ordering::Release);
    ui.set_ai_auto_search(false);
    ui.invoke_ai_action(0, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    ui.set_ai_prompt("科幻爆炸".into());
    ui.invoke_ai_action(2, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    let queries = ui
        .get_ai_quick_rows()
        .iter()
        .map(|row| row.text.to_string())
        .collect::<Vec<_>>();
    assert_eq!(queries.len(), 6);
    ui.invoke_ai_action(1, 0);
    let initial = fixture.controller.request.lock().unwrap().clone();

    // Neither an invalid filter nor a pending valid filter can be discarded
    // when a recommendation replaces the unrelated ordinary query draft.
    ui.set_query("x".repeat(501).into());
    for minimum in ["unfinished", "999"] {
        ui.set_duration_min(minimum.into());
        ui.invoke_ai_action(16, 0);
        assert_eq!(ui.get_query().len(), 501);
        assert_eq!(ui.get_duration_min(), minimum);
        assert_eq!(
            fixture.controller.request.lock().unwrap().query,
            initial.query
        );
        assert_eq!(ui.get_ai_quick_rows().row_count(), 6);
    }
    ui.set_duration_min("".into());
    for (kind, index, draft) in [
        (16, 1, "x".repeat(501)),
        (6, 2, "pending ordinary query".into()),
        (7, 3, "x".repeat(501)),
    ] {
        ui.set_query(draft.into());
        if kind != 16 {
            ui.set_ai_query(queries[index].clone().into());
        }
        ui.invoke_ai_action(kind, index as i32);
        let ready = fixture.controller.clone();
        let expected = queries[index].clone();
        wait_for(ui, move |ui| {
            !ui.get_searching()
                && !ui.get_task_restoring()
                && ui.get_query() == expected
                && fully_published(&ready)
        });
        let request = fixture.controller.request.lock().unwrap().clone();
        assert_eq!(request.filters, initial.filters);
        assert_eq!(request.folder_id, initial.folder_id);
        assert_eq!(request.scope, initial.scope);
        assert!(ui.get_rows().row_count() > 0);
    }
    assert_eq!(ui.get_search_tasks().row_count(), 2);
    ui.set_query("x".repeat(501).into());
    ui.invoke_ai_action(20, 0);
    wait_for(ui, |ui| {
        ui.get_search_tasks().row_count() == 8
            && !ui.get_ai_batch_searching()
            && !ui.get_task_restoring()
            && !ui.get_searching()
    });
    assert_cached_ai_queries(&fixture, 2, &queries);
    finish(&fixture);
}

#[test]
#[ignore = "runs in an owned isolated child"]
fn child_folder_filter_recommendations() {
    let fixture = fixture();
    let ui = &fixture.ui;
    let intent = "科幻爆炸";
    let terms = scifi_queries().join(" ");
    let files = fixture
        .provider
        .search(&SearchRequest::default())
        .unwrap()
        .results;
    for file in files
        .iter()
        .filter(|file| matches!(file.filename.as_str(), "gore one.wav" | "wind.wav"))
    {
        fixture
            .provider
            .update_metadata(
                file.id,
                &MetadataPatch {
                    notes: Some(terms.clone()),
                    ..Default::default()
                },
            )
            .unwrap();
    }
    search(&fixture, "", 4);
    ui.set_ai_auto_search(false);
    ui.invoke_ai_action(0, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    ui.set_ai_prompt(intent.into());
    ui.invoke_ai_action(2, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    let recommendations = ui
        .get_ai_quick_rows()
        .iter()
        .map(|row| row.text.to_string())
        .collect::<Vec<_>>();
    assert_eq!(recommendations.len(), 8);
    ui.invoke_ai_action(16, 0);
    let ready = fixture.controller.clone();
    wait_for(ui, move |ui| {
        !ui.get_searching() && ui.get_rows().row_count() == 2 && fully_published(&ready)
    });
    let assert_plan = |selected| {
        assert_eq!(
            ui.get_ai_quick_rows()
                .iter()
                .map(|row| row.text.to_string())
                .collect::<Vec<_>>(),
            recommendations
        );
        assert_eq!(ui.get_ai_rows().row_count(), recommendations.len());
        assert_eq!(ui.get_ai_selected(), selected);
        assert_eq!(ui.get_ai_prompt(), intent);
    };
    assert_plan(0);
    // Use the bound left-tree callbacks, not a reconstructed SearchRequest.
    for name in ["gore", "wind"] {
        if !ui.get_folders().iter().any(|folder| folder.name == name) {
            let root = ui
                .get_folders()
                .iter()
                .position(|folder| folder.name == "sounds")
                .unwrap();
            ui.invoke_folder_toggle_requested(root as i32);
            let name = name.to_owned();
            wait_for(ui, move |ui| {
                ui.get_folders().iter().any(|folder| folder.name == name)
            });
        }
        let index = ui
            .get_folders()
            .iter()
            .position(|folder| folder.name == name)
            .unwrap();
        ui.invoke_folder_requested(index as i32);
        let ready = fixture.controller.clone();
        wait_for(ui, move |ui| {
            !ui.get_searching() && ui.get_rows().row_count() == 1 && fully_published(&ready)
        });
        let folder = fixture
            .provider
            .find_folder_by_path(&fixture.root.join("sounds").join(name))
            .unwrap()
            .unwrap();
        assert_eq!(
            fixture.controller.request.lock().unwrap().folder_id,
            Some(folder.id)
        );
        assert_eq!(ui.get_query(), recommendations[0]);
        assert_plan(0);
    }
    ui.set_tag_filter("wind".into());
    ui.invoke_filter_requested();
    let ready = fixture.controller.clone();
    wait_for(ui, move |ui| !ui.get_searching() && fully_published(&ready));
    assert_eq!(ui.get_rows().row_count(), 1);
    assert_plan(0);
    // An unfinished filter draft blocks execution without erasing the plan.
    ui.set_duration_min("unfinished".into());
    ui.invoke_ai_action(16, 1);
    assert_eq!(ui.get_query(), recommendations[0]);
    assert_plan(0);
    ui.set_duration_min("".into());
    let wind = files
        .iter()
        .find(|file| file.filename == "wind.wav")
        .unwrap();
    fixture
        .provider
        .update_metadata(
            wind.id,
            &MetadataPatch {
                notes: Some(String::new()),
                ..Default::default()
            },
        )
        .unwrap();
    fixture.controller.refresh_result_window(ui);
    let ready = fixture.controller.clone();
    wait_for(ui, move |ui| !ui.get_searching() && fully_published(&ready));
    assert_eq!(ui.get_rows().row_count(), 0);
    assert_plan(0);
    // Zero in a child directory is not evidence that the library keyword died.
    ui.invoke_ai_action(16, 1);
    let ready = fixture.controller.clone();
    wait_for(ui, move |ui| !ui.get_searching() && fully_published(&ready));
    assert_eq!(ui.get_rows().row_count(), 0);
    assert_eq!(ui.get_query(), recommendations[1]);
    assert_plan(1);
    let current = fixture.controller.request.lock().unwrap().clone();
    let expected_tasks = 1 + recommendations.len();
    ui.invoke_ai_action(20, 0);
    wait_for(ui, move |ui| {
        ui.get_search_tasks().row_count() == expected_tasks
            && !ui.get_ai_batch_searching()
            && !ui.get_task_restoring()
            && !ui.get_searching()
    });
    assert_plan(1);
    {
        let state = fixture.controller.task_workspace.lock().unwrap();
        for task in &state.workspace.session.tasks[1..] {
            assert_eq!(
                task.definition.folder.as_ref().map(|folder| folder.id),
                current.folder_id
            );
            assert_eq!(task.definition.filters, current.filters);
            let cache = state
                .caches
                .get(&task.id)
                .expect("filtered batch must complete every task");
            assert!(cache.records.is_empty());
        }
    }
    ui.invoke_ai_action(3, 0);
    assert_eq!(ui.get_ai_quick_rows().row_count(), 0);
    assert_eq!(ui.get_ai_rows().row_count(), 0);
    finish(&fixture);
}

#[test]
fn ai_pending_generation_is_rejected_after_folder_navigation() {
    isolated("ai_search::child_pending_generation_folder_change");
}

#[test]
#[ignore = "runs in an owned isolated child"]
fn child_pending_generation_folder_change() {
    let fixture = fixture();
    let ui = &fixture.ui;
    seed_ai_notes(&fixture, 3);
    search(&fixture, "", 4);
    ui.set_ai_auto_search(false);
    ui.invoke_ai_action(0, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    if !ui.get_folders().iter().any(|folder| folder.name == "wind") {
        let root = ui
            .get_folders()
            .iter()
            .position(|folder| folder.name == "sounds")
            .unwrap();
        ui.invoke_folder_toggle_requested(root as i32);
        wait_for(ui, |ui| {
            ui.get_folders().iter().any(|folder| folder.name == "wind")
        });
    }
    let wind_index = ui
        .get_folders()
        .iter()
        .position(|folder| folder.name == "wind")
        .unwrap();
    ui.set_ai_prompt("科幻爆炸".into());
    ui.invoke_ai_action(2, 0);
    // The worker captured the previous whole-library request synchronously;
    // its queued reply must not publish into this new displayed folder.
    ui.invoke_folder_requested(wind_index as i32);
    let ready = fixture.controller.clone();
    wait_for(ui, move |ui| {
        !ui.get_ai_busy() && !ui.get_searching() && fully_published(&ready)
    });
    let wind = fixture
        .provider
        .find_folder_by_path(&fixture.root.join("sounds/wind"))
        .unwrap()
        .unwrap();
    assert_eq!(
        fixture.controller.request.lock().unwrap().folder_id,
        Some(wind.id)
    );
    assert_eq!(ui.get_rows().row_count(), 1);
    assert_eq!(ui.get_ai_quick_rows().row_count(), 0);
    assert_eq!(ui.get_ai_query(), "");
    assert_eq!(ui.get_ai_prompt(), "科幻爆炸");
    assert_eq!(ui.get_search_tasks().row_count(), 1);
    finish(&fixture);
}

#[test]
#[ignore = "runs in an owned isolated child"]
fn child_zero_hit_tag_pruning() {
    let fixture = fixture();
    let ui = &fixture.ui;
    seed_ai_notes(&fixture, 2);
    fixture.controller.capture_search_task(ui);
    let unrelated = {
        let state = fixture.controller.task_workspace.lock().unwrap();
        state.workspace.session.tasks[0].clone()
    };
    ui.invoke_search_task_created();
    wait_for(ui, |ui| !ui.get_task_restoring() && !ui.get_searching());
    ui.invoke_ai_action(0, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    ui.set_ai_prompt("科幻爆炸".into());
    ui.invoke_ai_action(2, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    assert_eq!(ui.get_ai_quick_rows().row_count(), 3);
    // An unrelated custom query may return zero without invalidating the plan.
    ui.set_ai_query("unrelated missing".into());
    ui.invoke_ai_action(6, 0);
    wait_for(ui, |ui| !ui.get_searching());
    assert_eq!(ui.get_rows().row_count(), 0);
    assert_eq!(ui.get_ai_quick_rows().row_count(), 3);
    assert_eq!(ui.get_ai_query(), "unrelated missing");
    for file in fixture
        .provider
        .search(&SearchRequest::default())
        .unwrap()
        .results
    {
        if file.filename == "gore one.wav" {
            fixture
                .provider
                .update_metadata(
                    file.id,
                    &MetadataPatch {
                        notes: Some(String::new()),
                        ..Default::default()
                    },
                )
                .unwrap();
        }
    }
    let index = ui
        .get_ai_quick_rows()
        .iter()
        .position(|row| row.text == "energy burst")
        .unwrap();
    ui.invoke_ai_action(16, index as i32);
    ui.set_ai_query("user custom draft".into());
    wait_for(ui, |ui| !ui.get_searching());
    assert_eq!(ui.get_rows().row_count(), 0);
    assert_eq!(ui.get_query(), "energy burst");
    assert_eq!(ui.get_ai_query(), "user custom draft");
    assert_eq!(ui.get_ai_selected(), -1);
    assert_eq!(ui.get_ai_quick_rows().row_count(), 2);
    assert!(
        !ui.get_ai_quick_rows()
            .iter()
            .any(|row| row.text == "energy burst")
    );
    // If the only single keyword also loses its hit, regenerate rather than
    // retaining a pair-only plan under the old verified-hit claim.
    ui.invoke_ai_action(16, 0);
    wait_for(ui, |ui| !ui.get_searching());
    assert_eq!(ui.get_query(), "explosion");
    assert_eq!(ui.get_ai_quick_rows().row_count(), 0);
    assert_eq!(ui.get_ai_rows().row_count(), 0);
    assert_eq!(ui.get_search_tasks().row_count(), 2);
    {
        let state = fixture.controller.task_workspace.lock().unwrap();
        assert_eq!(state.workspace.session.tasks[0].id, unrelated.id);
        assert_eq!(
            state.workspace.session.tasks[0].definition,
            unrelated.definition
        );
    }
    // Pruning must reapply role-dependent admission rather than leaving a
    // layer-only plan or seven main recommendations under an old eight-tag cap.
    use audiolib_core::ai_search::AiQueryKind::{Layer, Main};
    let layer_terms = ["spark", "debris", "metal", "bass", "rattle", "tail", "ring"];
    let sole_main = std::iter::once(("anchor", Main))
        .chain(layer_terms.iter().map(|term| (*term, Layer)))
        .collect::<Vec<_>>();
    let main_pairs = [
        "deep impact",
        "metal hit",
        "air whoosh",
        "noise burst",
        "glass break",
        "bell strike",
    ];
    let sole_layer = std::iter::once(("glow", Layer))
        .chain(main_pairs.iter().map(|term| (*term, Main)))
        .chain([("rumble", Main)])
        .collect::<Vec<_>>();
    seed_owned_ai_terms(
        &fixture,
        &std::iter::once("anchor")
            .chain(layer_terms)
            .collect::<Vec<_>>(),
    );
    search(&fixture, "", 4);
    let server = install_owned_ai_provider(
        &fixture,
        vec![owned_ai_plan(&sole_main), owned_ai_plan(&sole_layer)],
        None,
    );
    ui.set_ai_layers(true);
    ui.set_ai_auto_search(false);
    ui.set_ai_prompt("owned layered sound".into());
    ui.invoke_ai_action(2, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    assert_eq!(ui.get_ai_quick_rows().row_count(), 8);
    assert_eq!(
        ui.get_ai_quick_rows()
            .iter()
            .filter(|row| !row.is_layer)
            .count(),
        1
    );
    seed_owned_ai_terms(&fixture, &layer_terms);
    ui.invoke_ai_action(16, 0);
    ui.set_ai_query("custom draft after main pruning".into());
    wait_for(ui, |ui| !ui.get_searching());
    assert_eq!(ui.get_query(), "anchor");
    assert_eq!(ui.get_rows().row_count(), 0);
    assert_eq!(ui.get_ai_quick_rows().row_count(), 0);
    assert_eq!(ui.get_ai_rows().row_count(), 0);
    assert_eq!(ui.get_ai_selected(), -1);
    assert_eq!(ui.get_ai_query(), "custom draft after main pruning");

    seed_owned_ai_terms(
        &fixture,
        &sole_layer.iter().map(|(text, _)| *text).collect::<Vec<_>>(),
    );
    search(&fixture, "", 4);
    ui.invoke_ai_action(2, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    assert_eq!(ui.get_ai_quick_rows().row_count(), 8);
    assert_eq!(
        ui.get_ai_quick_rows()
            .iter()
            .filter(|row| row.is_layer)
            .count(),
        1
    );
    assert_eq!(ui.get_ai_quick_rows().row_data(0).unwrap().text, "glow");
    seed_owned_ai_terms(
        &fixture,
        &main_pairs
            .iter()
            .copied()
            .chain(["rumble"])
            .collect::<Vec<_>>(),
    );
    ui.invoke_ai_action(16, 0);
    ui.set_ai_query("custom draft after layer pruning".into());
    wait_for(ui, |ui| !ui.get_searching());
    let rows = ui.get_ai_quick_rows().iter().collect::<Vec<_>>();
    assert_eq!(ui.get_query(), "glow");
    assert_eq!(ui.get_rows().row_count(), 0);
    assert_eq!(rows.len(), 6);
    assert_eq!(ui.get_ai_rows().row_count(), 6);
    assert!(rows.iter().all(|row| !row.is_layer));
    assert_eq!(rows[0].text, "rumble");
    assert_eq!(
        rows.iter()
            .map(|row| row.text.to_string())
            .collect::<Vec<_>>(),
        [
            "rumble",
            "deep impact",
            "metal hit",
            "air whoosh",
            "noise burst",
            "glass break"
        ]
    );
    assert_eq!(ui.get_ai_selected(), -1);
    assert_eq!(ui.get_ai_query(), "custom draft after layer pruning");
    assert_eq!(ui.get_search_tasks().row_count(), 2);
    let state = fixture.controller.task_workspace.lock().unwrap();
    assert_eq!(
        state.workspace.session.tasks[0].definition,
        unrelated.definition
    );
    drop(state);
    server.join().unwrap();
    finish(&fixture);
}

#[test]
fn ai_single_keyword_survives_when_all_pairs_miss() {
    isolated("ai_search::child_single_keyword_without_pair_hits");
}

#[test]
#[ignore = "runs in an owned isolated child"]
fn child_single_keyword_without_pair_hits() {
    let fixture = fixture();
    let ui = &fixture.ui;
    assert!(!ui.get_ai_auto_search());
    for file in fixture
        .provider
        .search(&SearchRequest::default())
        .unwrap()
        .results
    {
        if file.filename == "gore one.wav" {
            fixture
                .provider
                .update_metadata(
                    file.id,
                    &MetadataPatch {
                        notes: Some("explosion".into()),
                        ..Default::default()
                    },
                )
                .unwrap();
        }
    }
    choose_gore(&fixture);
    search(&fixture, "explosion", 1);
    let request = fixture.controller.request.lock().unwrap().clone();
    let plan = audiolib_ai::reference::ReferenceIndex::embedded()
        .local_plan("科幻爆炸", false)
        .unwrap();
    assert!(
        plan.queries
            .iter()
            .all(|query| query.text.split_whitespace().count() == 2)
    );
    for query in &plan.queries {
        let mut pair_request = request.clone();
        pair_request.query = query.text.clone();
        assert!(
            fixture
                .provider
                .search(&pair_request)
                .unwrap()
                .results
                .is_empty()
        );
    }
    ui.invoke_ai_action(0, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    ui.set_ai_prompt("科幻爆炸".into());
    ui.set_ai_layers(false);
    ui.invoke_ai_action(2, 0);
    assert!(ui.get_ai_generating());
    wait_for(ui, |ui| !ui.get_ai_busy());
    assert!(!ui.get_ai_generating());
    assert_eq!(ui.get_ai_rows().row_count(), 1);
    assert_eq!(ui.get_ai_quick_rows().row_count(), 1);
    assert_eq!(ui.get_search_tasks().row_count(), 1);
    let row = ui.get_ai_quick_rows().row_data(0).unwrap();
    assert_eq!(row.text, "explosion");
    assert!(row.detail.contains(&plan.queries[0].reason));
    for source in &plan.queries[0].source_ids {
        assert!(row.detail.contains(source));
    }
    ui.invoke_ai_action(20, 0);
    wait_for(ui, |ui| {
        !ui.get_ai_batch_searching() && !ui.get_task_restoring() && !ui.get_searching()
    });
    assert_eq!(ui.get_search_tasks().row_count(), 2);
    assert_cached_ai_queries(&fixture, 1, &["explosion".into()]);
    ui.set_ai_auto_search(true);
    ui.invoke_ai_action(2, 0);
    wait_for(ui, |ui| {
        !ui.get_ai_busy()
            && !ui.get_ai_batch_searching()
            && !ui.get_task_restoring()
            && !ui.get_searching()
    });
    assert_eq!(ui.get_ai_quick_rows().row_count(), 1);
    assert_eq!(ui.get_search_tasks().row_count(), 3);
    assert_cached_ai_queries(&fixture, 2, &["explosion".into()]);
    finish(&fixture);
}

#[test]
fn ai_layered_limit_requires_verified_layers_and_has_no_fixed_role_quota() {
    isolated("ai_search::child_layered_limit_admission");
}

#[test]
#[ignore = "runs in an owned isolated child"]
fn child_layered_limit_admission() {
    use audiolib_core::ai_search::AiQueryKind::{Layer, Main};
    let fixture = fixture();
    let ui = &fixture.ui;
    let mains = [
        "explosion",
        "blast",
        "plasma",
        "energy",
        "impact",
        "whoosh",
        "rumble",
        "crackle",
    ];
    let layers = ["spark", "debris", "metal", "bass", "rattle", "tail"];
    let terms = mains
        .iter()
        .chain(&layers)
        .copied()
        .chain(["ring", "splash"])
        .collect::<Vec<_>>();
    seed_owned_ai_terms(&fixture, &terms);
    search(&fixture, "", 4);
    let main_queries = mains.iter().map(|text| (*text, Main)).collect::<Vec<_>>();
    let main_only = owned_ai_plan(&main_queries);
    let mut zero_hit_queries = main_queries.clone();
    zero_hit_queries.extend([("layerzeroalpha", Layer), ("layerzerobeta", Layer)]);
    let mut disabled_queries = main_queries;
    disabled_queries.extend([("spark", Layer), ("debris", Layer)]);
    let layer_heavy_queries = mains[..2]
        .iter()
        .map(|text| (*text, Main))
        .chain(layers.iter().map(|text| (*text, Layer)))
        .collect::<Vec<_>>();
    // Ranking across roles matters: six useful layers must survive even when
    // six lower-ranked main alternatives would otherwise fill a main quota.
    let layers_ranked_first = layers
        .iter()
        .map(|text| (*text, Layer))
        .chain(mains[..6].iter().map(|text| (*text, Main)))
        .collect::<Vec<_>>();
    let main_heavy_queries = mains[..7]
        .iter()
        .map(|text| (*text, Main))
        .chain([("spark", Layer)])
        .collect::<Vec<_>>();
    let main_ranked_last = layers
        .iter()
        .map(|text| (*text, Layer))
        .chain([("ring", Layer), ("splash", Layer), ("explosion", Main)])
        .collect::<Vec<_>>();
    let duplicate_main_later = [
        ("spark", Layer),
        ("explosion", Layer),
        ("explosion", Main),
        ("debris", Layer),
    ];
    let server = install_owned_ai_provider(
        &fixture,
        vec![
            main_only,
            owned_ai_plan(&zero_hit_queries),
            owned_ai_plan(&disabled_queries),
            owned_ai_plan(&layer_heavy_queries),
            owned_ai_plan(&layers_ranked_first),
            owned_ai_plan(&main_heavy_queries),
            owned_ai_plan(&main_ranked_last),
            owned_ai_plan(&duplicate_main_later),
        ],
        None,
    );
    ui.set_ai_auto_search(false);
    ui.set_ai_prompt("owned layered sound".into());
    for (allow_layers, expected, expected_layers, first) in [
        (true, 6, 0, "explosion"),
        (true, 6, 0, "explosion"),
        (false, 6, 0, "explosion"),
        (true, 8, 6, "explosion"),
        (true, 8, 6, "spark"),
        (true, 8, 1, "explosion"),
        (true, 8, 7, "spark"),
        (true, 3, 2, "spark"),
    ] {
        ui.set_ai_layers(allow_layers);
        ui.invoke_ai_action(2, 0);
        wait_for(ui, |ui| !ui.get_ai_busy());
        assert!(!ui.get_ai_error(), "{}", ui.get_ai_status());
        let rows = ui.get_ai_quick_rows().iter().collect::<Vec<_>>();
        assert_eq!(rows.len(), expected);
        assert_eq!(ui.get_ai_rows().row_count(), expected);
        assert_eq!(
            rows.iter().filter(|row| row.is_layer).count(),
            expected_layers
        );
        assert!(rows.iter().any(|row| !row.is_layer));
        assert_eq!(rows[0].text, first);
        assert_eq!(
            rows.iter()
                .map(|row| row.text.to_string())
                .collect::<std::collections::HashSet<_>>()
                .len(),
            expected
        );
        assert_eq!(ui.get_search_tasks().row_count(), 1);
        assert!(!ui.get_ai_batch_searching());
        if expected_layers == 0 {
            assert_eq!(
                rows.iter()
                    .map(|row| row.text.to_string())
                    .collect::<Vec<_>>(),
                mains[..6]
            );
        } else if expected == 3 {
            let main = rows.iter().find(|row| row.text == "explosion").unwrap();
            assert!(
                !main.is_layer,
                "A later Main owns the duplicate layer's slot"
            );
            assert_eq!(rows[1].text, "explosion");
        }
    }
    server.join().unwrap();
    finish(&fixture);
}

#[test]
fn ai_eight_typed_tags_support_last_actions_and_atomic_all_tasks() {
    isolated("ai_search::child_eight_typed_tags");
}

#[test]
#[ignore = "runs in an owned isolated child"]
fn child_eight_typed_tags() {
    use audiolib_core::ai_search::AiQueryKind::{Layer, Main};
    let fixture = fixture();
    let ui = &fixture.ui;
    let mains = [
        "explosion",
        "blast",
        "plasma",
        "energy",
        "impact",
        "whoosh",
        "rumble",
        "crackle",
    ];
    let mut queries = mains[..6]
        .iter()
        .map(|text| (*text, Main))
        .collect::<Vec<_>>();
    queries.extend([
        ("spark", Layer),
        ("debris", Layer),
        ("rumble", Main),
        ("crackle", Main),
        // A duplicate layer must not recolor a verified main recommendation.
        ("explosion", Layer),
        ("blast", Main),
    ]);
    let terms = mains
        .iter()
        .copied()
        .chain(["spark", "debris"])
        .collect::<Vec<_>>();
    seed_owned_ai_terms(&fixture, &terms);
    choose_gore(&fixture);
    ui.set_tag_filter("gore".into());
    ui.set_search_scope_index(1);
    search(&fixture, "gore", 3);
    let server = install_owned_ai_provider(&fixture, vec![owned_ai_plan(&queries)], None);
    ui.set_ai_auto_search(false);
    ui.set_ai_layers(true);
    ui.set_ai_prompt("owned layered sound".into());
    ui.invoke_ai_action(2, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    assert!(!ui.get_ai_error(), "{}", ui.get_ai_status());
    let rows = ui.get_ai_quick_rows().iter().collect::<Vec<_>>();
    let texts = rows
        .iter()
        .map(|row| row.text.to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        texts,
        [
            "explosion",
            "blast",
            "plasma",
            "energy",
            "impact",
            "whoosh",
            "spark",
            "debris"
        ]
    );
    assert_eq!(ui.get_ai_rows().row_count(), 8);
    for (index, row) in rows.iter().enumerate() {
        assert_eq!(row.is_layer, index >= 6);
        assert_eq!(
            ui.get_ai_rows().row_data(index).unwrap().is_layer,
            row.is_layer
        );
    }
    assert_eq!(
        texts.iter().collect::<std::collections::HashSet<_>>().len(),
        8
    );
    assert_eq!(ui.get_search_tasks().row_count(), 1);
    // The eighth body is fully actionable, preserving admitted conditions.
    let before = fixture.controller.request.lock().unwrap().clone();
    ui.invoke_ai_action(16, 7);
    let ready = fixture.controller.clone();
    wait_for(ui, move |ui| !ui.get_searching() && fully_published(&ready));
    assert_eq!(ui.get_query(), "debris");
    assert_eq!(ui.get_ai_selected(), 7);
    assert_eq!(ui.get_search_tasks().row_count(), 1);
    let after = fixture.controller.request.lock().unwrap().clone();
    assert_eq!(after.folder_id, before.folder_id);
    assert_eq!(after.filters, before.filters);
    assert_eq!(after.scope, before.scope);
    assert_eq!(ui.get_rows().row_count(), 1);
    // The matching + action creates exactly one independently warmed task.
    fixture
        .controller
        .max_search_tasks
        .store(2, Ordering::Release);
    ui.invoke_ai_action(24, 7);
    wait_for(ui, |ui| {
        ui.get_search_tasks().row_count() == 2 && !ui.get_task_restoring() && !ui.get_searching()
    });
    assert_eq!(ui.get_query(), "debris");
    assert_cached_ai_queries(&fixture, 1, &texts[7..]);
    let source = {
        fixture.controller.capture_search_task(ui);
        let state = fixture.controller.task_workspace.lock().unwrap();
        state.workspace.session.tasks[1].clone()
    };
    // Seven slots cannot admit an eight-tag batch; zero tasks must be created.
    fixture
        .controller
        .max_search_tasks
        .store(9, Ordering::Release);
    ui.invoke_ai_action(20, 0);
    assert_eq!(ui.get_search_tasks().row_count(), 2);
    assert_eq!(active_id(ui), source.id);
    assert_eq!(ui.get_query(), source.definition.query);
    assert!(!ui.get_ai_batch_searching());
    assert!(ui.get_ai_status().contains("No new tasks"));
    fixture
        .controller
        .max_search_tasks
        .store(10, Ordering::Release);
    ui.invoke_ai_action(20, 0);
    wait_for(ui, |ui| {
        ui.get_search_tasks().row_count() == 10
            && !ui.get_ai_batch_searching()
            && !ui.get_task_restoring()
            && !ui.get_searching()
    });
    assert_cached_ai_queries(&fixture, 2, &texts);
    {
        let state = fixture.controller.task_workspace.lock().unwrap();
        let batch = &state.workspace.session.tasks[2..];
        assert_eq!(batch.len(), 8);
        assert_eq!(
            batch
                .iter()
                .map(|task| &task.id)
                .collect::<std::collections::HashSet<_>>()
                .len(),
            8
        );
        assert_eq!(
            batch
                .iter()
                .map(|task| &task.definition.query)
                .collect::<std::collections::HashSet<_>>()
                .len(),
            8
        );
        for task in batch {
            let mut expected = source.definition.clone();
            expected.query = task.definition.query.clone();
            assert_eq!(task.definition, expected);
        }
    }
    server.join().unwrap();
    finish(&fixture);
}

#[test]
fn ai_layer_permission_change_rejects_pending_generation_and_auto_tasks() {
    isolated("ai_search::child_layer_permission_changed");
}

#[test]
#[ignore = "runs in an owned isolated child"]
fn child_layer_permission_changed() {
    use audiolib_core::ai_search::AiQueryKind::{Layer, Main};
    use std::sync::mpsc;
    let fixture = fixture();
    let ui = &fixture.ui;
    seed_owned_ai_terms(
        &fixture,
        &[
            "explosion",
            "blast",
            "plasma",
            "energy",
            "impact",
            "whoosh",
            "spark",
            "debris",
        ],
    );
    search(&fixture, "", 4);
    let plan = owned_ai_plan(&[
        ("explosion", Main),
        ("blast", Main),
        ("plasma", Main),
        ("energy", Main),
        ("impact", Main),
        ("whoosh", Main),
        ("spark", Layer),
        ("debris", Layer),
    ]);
    let (accepted, received) = mpsc::sync_channel(1);
    let (release, released) = mpsc::sync_channel(1);
    let server = install_owned_ai_provider(&fixture, vec![plan], Some((accepted, released)));
    ui.set_ai_layers(true);
    ui.set_ai_auto_search(true);
    ui.set_ai_prompt("owned layered sound".into());
    ui.invoke_ai_action(2, 0);
    received.recv_timeout(Duration::from_secs(5)).unwrap();
    // This also covers an unsent property change, without relying on the widget's
    // explicit cancellation callback to invalidate the old layer permission.
    ui.set_ai_layers(false);
    release.send(()).unwrap();
    wait_for(ui, |ui| {
        !ui.get_ai_busy()
            && !ui.get_ai_batch_searching()
            && !ui.get_task_restoring()
            && !ui.get_searching()
    });
    assert_eq!(ui.get_ai_rows().row_count(), 0);
    assert_eq!(ui.get_ai_quick_rows().row_count(), 0);
    assert_eq!(ui.get_search_tasks().row_count(), 1);
    assert_eq!(ui.get_ai_batch_total(), 0);
    assert!(ui.get_ai_status().contains("context changed"));
    server.join().unwrap();
    finish(&fixture);
}

#[test]
fn ai_six_verified_recommendations_prefer_single_and_default_to_manual() {
    isolated("ai_search::child_six_verified_manual_default");
}

#[test]
#[ignore = "runs in an owned isolated child"]
fn child_six_verified_manual_default() {
    use audiolib_core::ai_search::AiQueryKind::Main;
    let fixture = fixture();
    let ui = &fixture.ui;
    assert!(!ui.get_ai_auto_search());
    for file in fixture
        .provider
        .search(&SearchRequest::default())
        .unwrap()
        .results
    {
        if file.filename == "gore one.wav" {
            fixture
                .provider
                .update_metadata(
                    file.id,
                    &MetadataPatch {
                        // Six pairs match, while the first valid raw single
                        // (plasma) occurs after those pairs in the plan.
                        notes: Some("scifi explosion energy burst plasma explosion metal impact sub boom bass drop".into()),
                        ..Default::default()
                    },
                )
                .unwrap();
        }
    }
    ui.invoke_ai_action(0, 0);
    assert!(!ui.get_ai_generating());
    wait_for(ui, |ui| !ui.get_ai_busy());
    ui.set_ai_prompt("科幻爆炸".into());
    ui.set_ai_layers(false);
    ui.invoke_ai_action(2, 0);
    assert!(ui.get_ai_generating());
    wait_for(ui, |ui| !ui.get_ai_busy());
    assert!(!ui.get_ai_generating());
    assert_eq!(ui.get_search_tasks().row_count(), 1);
    // Pair-only input derives a real-hit core word, skipping the genre modifier.
    assert_eq!(
        ui.get_ai_quick_rows().row_data(0).unwrap().text,
        "explosion"
    );
    assert_eq!(ui.get_ai_quick_rows().row_count(), 4);
    let fallback = ui.get_ai_quick_rows().row_data(0).unwrap();
    let original = ui.get_ai_quick_rows().row_data(1).unwrap();
    assert_eq!(fallback.detail, original.detail);

    // This regression owns the ordinary six-tag contract, independent of how
    // many explicitly typed layers an offline recipe now supplies.
    let plan = owned_ai_plan(&[
        ("scifi explosion", Main),
        ("energy burst", Main),
        ("plasma explosion", Main),
        ("metal impact", Main),
        ("sub boom", Main),
        ("bass drop", Main),
        ("plasma", Main),
    ]);
    let server = install_owned_ai_provider(&fixture, vec![plan.clone(), plan], None);
    ui.set_ai_layers(true);
    ui.invoke_ai_action(2, 0);
    assert!(ui.get_ai_generating());
    wait_for(ui, |ui| !ui.get_ai_busy());
    assert!(!ui.get_ai_generating());
    assert_eq!(ui.get_search_tasks().row_count(), 1);
    let recommendations = ui
        .get_ai_quick_rows()
        .iter()
        .map(|row| row.text.to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        recommendations,
        [
            "plasma",
            "scifi explosion",
            "energy burst",
            "plasma explosion",
            "metal impact",
            "sub boom"
        ]
    );
    assert_eq!(ui.get_ai_rows().row_count(), 6);
    assert!(scifi_queries().len() > recommendations.len());
    // A seventh candidate also has real hits; it must stay outside the six-tag
    // display cap rather than being filtered out as an invalid recommendation.
    for query in recommendations
        .iter()
        .map(String::as_str)
        .chain(std::iter::once("bass drop"))
    {
        assert!(query.split_whitespace().count() <= 2);
        let mut request = fixture.controller.request.lock().unwrap().clone();
        request.query = query.to_owned();
        request.limit = 1;
        request.offset = 0;
        request.cursor = None;
        request.page = None;
        assert!(
            !fixture
                .provider
                .search(&request)
                .unwrap()
                .results
                .is_empty()
        );
    }
    // The sixth toolbar choice is independently actionable, not just displayed.
    ui.invoke_ai_action(16, 5);
    wait_for(ui, |ui| !ui.get_searching());
    assert_eq!(ui.get_query(), "sub boom");
    assert_eq!(ui.get_ai_selected(), 5);
    assert_eq!(ui.get_search_tasks().row_count(), 1);

    fixture.controller.capture_search_task(ui);
    let original = {
        let state = fixture.controller.task_workspace.lock().unwrap();
        state.workspace.session.tasks[0].clone()
    };
    // Three free slots admit opted-in automation, but All must refuse the same
    // capacity atomically rather than silently searching only three of six.
    fixture
        .controller
        .max_search_tasks
        .store(4, Ordering::Release);
    ui.invoke_ai_action(20, 0);
    assert_eq!(ui.get_search_tasks().row_count(), 1);
    assert_eq!(active_id(ui), original.id);
    assert_eq!(ui.get_query(), original.definition.query);
    assert!(!ui.get_ai_batch_searching());
    assert!(ui.get_ai_status().contains("No new tasks"));

    ui.set_ai_auto_search(true);
    ui.invoke_ai_action(2, 0);
    assert!(ui.get_ai_generating());
    wait_for(ui, |ui| {
        !ui.get_ai_busy()
            && !ui.get_ai_batch_searching()
            && !ui.get_task_restoring()
            && !ui.get_searching()
    });
    assert!(!ui.get_ai_generating());
    assert_eq!(ui.get_search_tasks().row_count(), 4);
    assert_eq!(ui.get_ai_quick_rows().row_count(), 6);
    assert_cached_ai_queries(&fixture, 1, &recommendations[..3]);
    // Manual All searches every tag independently even with automation off.
    ui.set_ai_auto_search(false);
    fixture
        .controller
        .max_search_tasks
        .store(12, Ordering::Release);
    ui.invoke_ai_action(20, 0);
    wait_for(ui, |ui| {
        !ui.get_ai_batch_searching() && !ui.get_task_restoring() && !ui.get_searching()
    });
    assert_eq!(ui.get_search_tasks().row_count(), 10);
    assert_cached_ai_queries(&fixture, 4, &recommendations);
    {
        let state = fixture.controller.task_workspace.lock().unwrap();
        assert_eq!(
            state.workspace.session.active_task_id.as_ref(),
            Some(&state.workspace.session.tasks[4].id)
        );
        for task in &state.workspace.session.tasks[1..] {
            let mut expected = original.definition.clone();
            expected.query = task.definition.query.clone();
            assert_eq!(task.definition, expected);
        }
    }
    // The sixth tag's + action creates exactly one task using the same scope.
    ui.invoke_ai_action(24, 5);
    wait_for(ui, |ui| {
        ui.get_search_tasks().row_count() == 11 && !ui.get_task_restoring() && !ui.get_searching()
    });
    assert_eq!(ui.get_query(), "sub boom");
    assert_cached_ai_queries(&fixture, 10, &recommendations[5..]);
    server.join().unwrap();
    finish(&fixture);
}

#[test]
fn ai_long_model_queries_keep_sound_anchor_and_verify_two_term_hits() {
    isolated("ai_search::child_long_model_queries");
}

#[test]
#[ignore = "runs in an owned isolated child"]
fn child_long_model_queries() {
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };
    let fixture = fixture();
    let ui = &fixture.ui;
    ui.set_ai_auto_search(true);
    for file in fixture
        .provider
        .search(&SearchRequest::default())
        .unwrap()
        .results
    {
        if file.filename == "wind.wav" {
            fixture
                .provider
                .update_metadata(
                    file.id,
                    &MetadataPatch {
                        notes: Some("heavy wind roar distant wind".into()),
                        ..Default::default()
                    },
                )
                .unwrap();
        }
    }
    let mut plan = audiolib_ai::reference::ReferenceIndex::embedded()
        .local_plan("wind", false)
        .unwrap();
    plan.queries.truncate(1);
    let provenance = plan.queries[0].source_ids.clone();
    let reason = plan.queries[0].reason.clone();
    plan.queries[0].text = "heavy strong wind roar".into();
    let mut modifier = plan.queries[0].clone();
    modifier.text = "heavy".into();
    plan.queries.insert(0, modifier);
    let mut thunder = audiolib_ai::reference::ReferenceIndex::embedded()
        .local_plan("thunder", false)
        .unwrap()
        .queries
        .remove(0);
    // A spatial descriptor alone must not admit an unrelated distant wind hit.
    thunder.text = "distant thunder rumble".into();
    // This is an independently generated alternative, not a supplied wind
    // reference. It must not claim the separate thunder lookup's provenance.
    thunder.source_ids.clear();
    thunder.cat_ids.clear();
    plan.queries.push(thunder);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut head = Vec::new();
        let mut byte = [0];
        while !head.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).unwrap();
            head.push(byte[0]);
        }
        let header = String::from_utf8(head).unwrap();
        assert!(header.starts_with("POST /chat/completions HTTP/1.1"));
        let length = header
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .map(|length| length.trim().parse::<usize>().unwrap())
            })
            .unwrap();
        let mut body = vec![0; length];
        stream.read_exact(&mut body).unwrap();
        let reply = serde_json::json!({"choices":[{"finish_reason":"stop","message":{"content":serde_json::to_string(&plan).unwrap()}}]}).to_string();
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}", reply.len()).unwrap();
    });
    ui.invoke_ai_action(0, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    ui.invoke_ai_action(9, 0);
    ui.set_ai_protocol(0);
    ui.invoke_ai_action(12, 0);
    ui.set_ai_endpoint(endpoint.into());
    ui.set_ai_model("owned-model".into());
    ui.set_ai_key("owned-key".into());
    ui.invoke_ai_action(8, 0);
    wait_for(ui, |ui| {
        !ui.get_ai_busy() && ui.get_ai_provider_index() == 1
    });
    ui.set_ai_prompt("wind".into());
    ui.invoke_ai_action(2, 0);
    wait_for(ui, |ui| {
        !ui.get_ai_busy()
            && !ui.get_ai_batch_searching()
            && !ui.get_task_restoring()
            && !ui.get_searching()
    });
    assert_eq!(ui.get_search_tasks().row_count(), 3);
    assert_eq!(ui.get_ai_quick_rows().row_count(), 2);
    assert_eq!(ui.get_ai_quick_rows().row_data(0).unwrap().text, "wind");
    let row = ui.get_ai_quick_rows().row_data(1).unwrap();
    assert_eq!(row.text, "wind roar");
    assert!(row.text.split_whitespace().count() <= 2);
    assert!(row.detail.contains(&reason));
    for source in provenance {
        assert!(row.detail.contains(&source));
    }
    assert_cached_ai_queries(&fixture, 1, &["wind".into(), "wind roar".into()]);
    let saved_usage = || {
        audiolib_ai::storage::ProfileStore::open(&fixture.root.join("preferences"))
            .unwrap()
            .usage(ui.get_ai_profile_id().as_str())
            .requests
    };
    let requests = saved_usage();
    assert_eq!(requests, 1);
    // The provider accepted exactly one generation. The edge action uses the
    // second verified tag directly, without generating or charging another call.
    ui.invoke_ai_action(24, 1);
    wait_for(ui, |ui| {
        ui.get_search_tasks().row_count() == 4 && !ui.get_task_restoring() && !ui.get_searching()
    });
    assert_eq!(ui.get_query(), "wind roar");
    assert_eq!(saved_usage(), requests);
    // The two-term limit belongs to recommendations, never ordinary user searches.
    ui.invoke_ai_action(1, 0);
    search(&fixture, "heavy strong wind roar", 0);
    assert_eq!(
        fixture.controller.request.lock().unwrap().query,
        "heavy strong wind roar"
    );
    server.join().unwrap();
    finish(&fixture);
}

#[test]
fn ai_batch_distinguishes_owned_restore_from_real_modal_cancellation() {
    isolated("ai_search::child_batch_modal_cancellation");
}

#[test]
#[ignore = "runs in an owned isolated child"]
fn child_batch_modal_cancellation() {
    let fixture = fixture();
    let ui = &fixture.ui;
    ui.set_ai_auto_search(true);
    seed_ai_notes(&fixture, 3);
    ui.invoke_ai_action(0, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    ui.set_ai_prompt("科幻爆炸".into());
    ui.invoke_ai_action(2, 0);
    wait_for(ui, |ui| ui.get_ai_batch_searching());
    ui.set_about_visible(true);
    assert!(ui.get_ai_batch_blocked());
    let count = ui.get_search_tasks().row_count();
    ui.invoke_ai_action(20, 0);
    wait_for(ui, |ui| !ui.get_ai_batch_searching());
    assert_eq!(ui.get_ai_batch_completed(), 0);
    assert_eq!(ui.get_ai_batch_total(), 0);
    ui.set_about_visible(false);
    wait_for(ui, |ui| !ui.get_task_restoring() && !ui.get_searching());
    let selected = active_id(ui);
    ui.invoke_ai_action(0, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    assert_eq!(active_id(ui), selected);
    assert_eq!(ui.get_search_tasks().row_count(), count);
    finish(&fixture);
}

#[test]
fn ai_auto_three_verifies_hits_primes_caches_and_yields_to_navigation() {
    isolated("ai_search::child_auto_three");
}

#[test]
#[ignore = "runs in an owned isolated child"]
fn child_auto_three() {
    let fixture = fixture();
    let ui = &fixture.ui;
    assert!(!ui.get_ai_auto_search());
    ui.set_ai_auto_search(true);
    let recommendations = seed_ai_filename_hits(&fixture);
    let queries = recommendations[..3].to_vec();
    choose_gore(&fixture);
    ui.set_tag_filter("gore".into());
    ui.set_search_scope_index(3);
    search(&fixture, "gore", 3);
    fixture.controller.capture_search_task(ui);
    let original = {
        let state = fixture.controller.task_workspace.lock().unwrap();
        state.workspace.session.tasks[0].clone()
    };
    ui.invoke_ai_action(0, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    assert_eq!(ui.get_search_tasks().row_count(), 1);
    ui.set_ai_prompt("科幻爆炸".into());
    ui.invoke_ai_action(2, 0);
    // A submission owns its preference even if it changes before completion.
    ui.set_ai_auto_search(false);
    wait_for(ui, |ui| {
        ui.get_search_tasks().row_count() == 4
            && !ui.get_ai_busy()
            && !ui.get_ai_batch_searching()
            && !ui.get_task_restoring()
            && !ui.get_searching()
    });
    assert_eq!(
        ui.get_ai_quick_rows()
            .iter()
            .map(|row| row.text.to_string())
            .collect::<Vec<_>>(),
        recommendations
    );
    assert_cached_ai_queries(&fixture, 1, &queries);
    {
        let state = fixture.controller.task_workspace.lock().unwrap();
        assert_eq!(
            state.workspace.session.active_task_id.as_ref(),
            Some(&state.workspace.session.tasks[1].id)
        );
        for (task, query) in state.workspace.session.tasks[1..].iter().zip(&queries) {
            let mut expected = original.definition.clone();
            expected.query = query.clone();
            assert_eq!(task.definition, expected);
        }
    }
    assert_eq!(ui.get_query(), queries[0]);
    fixture.controller.refresh_result_window(ui);
    wait_for(ui, |ui| !ui.get_searching());
    assert_eq!(ui.get_search_tasks().row_count(), 4);
    // Unchecked submission stays manual even if rechecked while it is pending.
    ui.invoke_ai_action(2, 0);
    ui.set_ai_auto_search(true);
    wait_for(ui, |ui| {
        !ui.get_ai_busy() && ui.get_ai_quick_rows().row_count() == 4
    });
    assert_eq!(ui.get_search_tasks().row_count(), 4);
    assert!(!ui.get_ai_batch_searching());
    // Manual navigation may interrupt All during its first asynchronous restore.
    ui.invoke_ai_action(20, 0);
    wait_for(ui, |ui| ui.get_ai_batch_searching());
    fixture.controller.switch_search_task(&original.id);
    wait_for(ui, |ui| {
        !ui.get_ai_batch_searching() && !ui.get_task_restoring() && !ui.get_searching()
    });
    assert_eq!(active_id(ui), original.id);
    assert_eq!(ui.get_query(), original.definition.query);
    assert_eq!(ui.get_ai_batch_completed(), 0);
    assert_eq!(ui.get_ai_batch_total(), 0);
    // A later event-loop turn cannot resume the superseded queue over that choice.
    ui.invoke_ai_action(1, 0);
    ui.invoke_ai_action(0, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    assert_eq!(active_id(ui), original.id);
    finish(&fixture);
}

#[test]
fn ai_verified_three_handles_empty_fewer_capacity_and_stale_work() {
    isolated("ai_search::child_verified_three_boundaries");
}

#[test]
#[ignore = "runs in an owned isolated child"]
fn child_verified_three_boundaries() {
    let fixture = fixture();
    let ui = &fixture.ui;
    ui.set_ai_auto_search(true);
    ui.invoke_ai_action(0, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    ui.set_ai_prompt("科幻爆炸".into());
    ui.invoke_ai_action(2, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    assert_eq!(ui.get_ai_rows().row_count(), 0);
    assert!(ui.get_ai_query().is_empty());
    assert_eq!(ui.get_search_tasks().row_count(), 1);
    assert!(!ui.get_ai_error());
    ui.invoke_ai_action(20, 0);
    assert_eq!(ui.get_search_tasks().row_count(), 1);
    assert!(!ui.get_ai_batch_searching());
    seed_ai_notes(&fixture, 2);
    fixture
        .controller
        .max_search_tasks
        .store(2, Ordering::Release);
    ui.invoke_ai_action(2, 0);
    wait_for(ui, |ui| {
        !ui.get_ai_busy() && ui.get_ai_rows().row_count() == 3
    });
    assert_eq!(ui.get_search_tasks().row_count(), 1);
    assert!(!ui.get_ai_batch_searching());
    assert!(ui.get_ai_status().contains("No new tasks"));
    fixture
        .controller
        .max_search_tasks
        .store(12, Ordering::Release);
    ui.invoke_ai_action(20, 0);
    wait_for(ui, |ui| {
        ui.get_search_tasks().row_count() == 4
            && !ui.get_ai_batch_searching()
            && !ui.get_task_restoring()
            && !ui.get_searching()
    });
    let queries = ui
        .get_ai_quick_rows()
        .iter()
        .map(|row| row.text.to_string())
        .collect::<Vec<_>>();
    assert_eq!(queries.len(), 3);
    assert_cached_ai_queries(&fixture, 1, &queries);
    // Cancellation and a new admitted search must never create another batch.
    ui.invoke_ai_action(2, 0);
    assert!(ui.get_ai_generating());
    ui.invoke_ai_action(3, 0);
    assert!(!ui.get_ai_generating());
    wait_for(ui, |ui| !ui.get_ai_busy());
    assert_eq!(ui.get_search_tasks().row_count(), 4);
    assert_eq!(ui.get_ai_quick_rows().row_count(), 0);
    assert_eq!(ui.get_ai_batch_total(), 0);
    ui.invoke_ai_action(2, 0);
    search(&fixture, "wind", 1);
    wait_for(ui, |ui| !ui.get_ai_busy());
    assert_eq!(ui.get_search_tasks().row_count(), 4);
    assert_eq!(ui.get_ai_quick_rows().row_count(), 0);
    // An invalid unsent editor draft changes no generation, but still rejects publication.
    ui.invoke_ai_action(2, 0);
    ui.set_duration_min("unfinished".into());
    wait_for(ui, |ui| !ui.get_ai_busy());
    assert_eq!(ui.get_search_tasks().row_count(), 4);
    assert_eq!(ui.get_ai_quick_rows().row_count(), 0);
    ui.set_duration_min("".into());
    finish(&fixture);
}

#[test]
fn ai_batch_removes_a_query_that_loses_hits_after_preflight() {
    isolated("ai_search::child_batch_loses_hits");
}

#[test]
#[ignore = "runs in an owned isolated child"]
fn child_batch_loses_hits() {
    let fixture = fixture();
    let ui = &fixture.ui;
    ui.set_ai_auto_search(true);
    seed_ai_notes(&fixture, 3);
    let original = active_id(ui);
    ui.invoke_ai_action(0, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    ui.set_ai_prompt("科幻爆炸".into());
    ui.invoke_ai_action(2, 0);
    wait_for(ui, |ui| ui.get_ai_batch_searching());
    for file in fixture
        .provider
        .search(&SearchRequest::default())
        .unwrap()
        .results
    {
        if file.filename == "gore one.wav" {
            fixture
                .provider
                .update_metadata(
                    file.id,
                    &MetadataPatch {
                        notes: Some(String::new()),
                        ..Default::default()
                    },
                )
                .unwrap();
        }
    }
    wait_for(ui, |ui| {
        !ui.get_ai_batch_searching() && !ui.get_task_restoring() && !ui.get_searching()
    });
    assert_eq!(ui.get_ai_quick_rows().row_count(), 0);
    assert_eq!(ui.get_ai_rows().row_count(), 0);
    assert_eq!(ui.get_search_tasks().row_count(), 3);
    assert!(ui.get_search_tasks().iter().any(|task| task.id == original));
    assert_eq!(ui.get_ai_batch_total(), 0);
    finish(&fixture);
}

#[test]
fn ai_admitted_main_fallback_keeps_verified_all_mode_queries() {
    isolated("ai_search::child_admitted_main_fallback");
}

#[test]
#[ignore = "runs in an owned isolated child"]
fn child_admitted_main_fallback() {
    let fixture = fixture();
    let ui = &fixture.ui;
    ui.set_ai_auto_search(true);
    seed_ai_notes(&fixture, 3);
    // Represent the canonical All -> Main pair admitted while indexing is absent.
    // The index may become ready again before asynchronous task restore executes.
    ui.set_search_scope_index(0);
    fixture.controller.request.lock().unwrap().scope = audiolib_core::SearchScope::Main;
    assert_eq!(
        fixture
            .controller
            .ai_search_probe_request(ui)
            .unwrap()
            .scope,
        audiolib_core::SearchScope::Main
    );
    ui.invoke_ai_action(0, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    ui.set_ai_prompt("科幻爆炸".into());
    ui.invoke_ai_action(2, 0);
    wait_for(ui, |ui| {
        ui.get_search_tasks().row_count() == 4
            && !ui.get_ai_busy()
            && !ui.get_ai_batch_searching()
            && !ui.get_task_restoring()
            && !ui.get_searching()
    });
    let queries = ui
        .get_ai_quick_rows()
        .iter()
        .take(3)
        .map(|row| row.text.to_string())
        .collect::<Vec<_>>();
    assert_eq!(queries.len(), 3);
    assert_cached_ai_queries(&fixture, 1, &queries);
    finish(&fixture);
}

#[test]
fn ai_selected_model_tracks_saved_generation_profile_without_draft_leak() {
    isolated("ai_search::child_provider_drafts");
}

fn seed_saved_ai_profile(fixture: &Fixture, custom_preset: bool) {
    use audiolib_ai::{
        config::{Profile, Protocol},
        storage::ProfileStore,
    };
    let mut store = ProfileStore::open(&fixture.root.join("preferences")).unwrap();
    store
        .save(
            Profile {
                id: "sidebar-provider".into(),
                name: "Sound design".into(),
                protocol: Protocol::ChatCompletions,
                base_url: "https://example.test/v1".into(),
                model: "saved-model".into(),
                ..Default::default()
            },
            None,
        )
        .unwrap();
    if custom_preset {
        store
            .save_prompt(true, "Prefer raw material for layered sound design")
            .unwrap();
    }
}

#[test]
fn ai_sidebar_opens_setup_without_a_saved_provider() {
    isolated("ai_search::child_sidebar_missing_provider");
}

#[test]
#[ignore = "runs in an owned isolated child"]
fn child_sidebar_missing_provider() {
    let fixture = fixture();
    let ui = &fixture.ui;
    ui.invoke_ai_action(0, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    assert!(ui.get_ai_visible());
    assert!(!ui.get_ai_collapsed());
    assert!(ui.get_ai_settings());
    assert_eq!(ui.get_ai_providers().row_count(), 2);
    ui.set_ai_profile_name("Unsaved provider".into());
    ui.set_ai_model("draft-model".into());
    ui.set_ai_dirty(true);
    ui.invoke_ai_action(1, 0);
    ui.invoke_ai_action(0, 0);
    assert!(!ui.get_ai_collapsed());
    assert!(ui.get_ai_settings());
    assert_eq!(ui.get_ai_profile_name(), "Unsaved provider");
    assert_eq!(ui.get_ai_model(), "draft-model");
    finish(&fixture);
}

#[test]
fn ai_sidebar_stays_collapsed_with_a_saved_provider_and_default_preset() {
    isolated("ai_search::child_sidebar_default_preset");
}

#[test]
#[ignore = "runs in an owned isolated child"]
fn child_sidebar_default_preset() {
    saved_ai_sidebar_preset(false);
}

#[test]
fn ai_sidebar_stays_collapsed_with_a_saved_provider_and_custom_preset() {
    isolated("ai_search::child_sidebar_custom_preset");
}

#[test]
#[ignore = "runs in an owned isolated child"]
fn child_sidebar_custom_preset() {
    saved_ai_sidebar_preset(true);
}

fn saved_ai_sidebar_preset(custom_preset: bool) {
    let fixture = fixture();
    let ui = &fixture.ui;
    seed_saved_ai_profile(&fixture, custom_preset);
    ui.invoke_ai_action(0, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    assert!(ui.get_ai_visible());
    assert!(ui.get_ai_collapsed());
    assert_eq!(ui.get_ai_provider_index(), 1);
    assert_eq!(ui.get_ai_preset_index(), i32::from(custom_preset));
    // Saved configuration is sufficient; opening the mode does not call or
    // authenticate a provider, and a key is not required to hide the sidebar.
    assert!(!ui.get_ai_saved_key());
    ui.set_ai_collapsed(false);
    ui.invoke_ai_action(1, 0);
    ui.invoke_ai_action(0, 0);
    assert!(ui.get_ai_collapsed());
    finish(&fixture);
}

#[test]
fn ai_settings_open_explicitly_during_loading_and_keep_unsaved_drafts() {
    isolated("ai_search::child_sidebar_explicit_setup");
}

#[test]
#[ignore = "runs in an owned isolated child"]
fn child_sidebar_explicit_setup() {
    let fixture = fixture();
    let ui = &fixture.ui;
    seed_saved_ai_profile(&fixture, true);
    ui.invoke_ai_action(0, 0);
    assert!(ui.get_ai_busy());
    ui.invoke_ai_action(22, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    assert!(ui.get_ai_visible());
    assert!(!ui.get_ai_collapsed());
    assert!(ui.get_ai_settings());
    ui.set_ai_model("draft-model".into());
    ui.set_ai_key("unsaved-draft-key".into());
    ui.set_ai_dirty(true);
    ui.set_ai_collapsed(true);
    ui.invoke_ai_action(22, 0);
    assert!(!ui.get_ai_collapsed());
    assert!(ui.get_ai_settings());
    assert_eq!(ui.get_ai_model(), "draft-model");
    assert_eq!(ui.get_ai_key(), "unsaved-draft-key");
    assert!(ui.get_ai_dirty());
    ui.invoke_ai_action(1, 0);
    ui.invoke_ai_action(22, 0);
    assert!(ui.get_ai_visible());
    assert!(!ui.get_ai_collapsed());
    assert_eq!(ui.get_ai_model(), "draft-model");
    assert_eq!(ui.get_ai_key(), "unsaved-draft-key");
    let persisted =
        std::fs::read_to_string(fixture.root.join("preferences/ai/profiles.json")).unwrap();
    assert!(!persisted.contains("unsaved-draft-key"));
    assert!(!persisted.contains("draft-model"));
    finish(&fixture);
}

#[test]
fn ai_initial_configuration_load_does_not_reopen_an_exited_mode() {
    isolated("ai_search::child_sidebar_exit_during_load");
}

#[test]
#[ignore = "runs in an owned isolated child"]
fn child_sidebar_exit_during_load() {
    let fixture = fixture();
    let ui = &fixture.ui;
    ui.invoke_ai_action(0, 0);
    assert!(ui.get_ai_busy());
    ui.invoke_ai_action(1, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    assert!(!ui.get_ai_visible());
    ui.invoke_ai_action(0, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    assert!(ui.get_ai_visible());
    assert!(!ui.get_ai_collapsed());
    assert!(ui.get_ai_settings());
    finish(&fixture);
}

#[test]
fn ai_loading_respects_manual_sidebar_expansion_with_a_saved_provider() {
    isolated("ai_search::child_sidebar_expand_during_load");
}

#[test]
#[ignore = "runs in an owned isolated child"]
fn child_sidebar_expand_during_load() {
    let fixture = fixture();
    let ui = &fixture.ui;
    seed_saved_ai_profile(&fixture, false);
    ui.invoke_ai_action(0, 0);
    assert!(ui.get_ai_busy());
    ui.invoke_ai_action(23, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    assert!(ui.get_ai_visible());
    assert!(!ui.get_ai_collapsed());
    assert_eq!(ui.get_ai_provider_index(), 1);
    finish(&fixture);
}

#[test]
fn ai_loading_respects_manual_sidebar_collapse_without_a_saved_provider() {
    isolated("ai_search::child_sidebar_collapse_during_load");
}

#[test]
#[ignore = "runs in an owned isolated child"]
fn child_sidebar_collapse_during_load() {
    let fixture = fixture();
    let ui = &fixture.ui;
    ui.invoke_ai_action(0, 0);
    assert!(ui.get_ai_busy());
    ui.invoke_ai_action(23, 1);
    wait_for(ui, |ui| !ui.get_ai_busy());
    assert!(ui.get_ai_visible());
    assert!(ui.get_ai_collapsed());
    assert_eq!(ui.get_ai_providers().row_count(), 2);
    ui.invoke_ai_action(22, 0);
    assert!(!ui.get_ai_collapsed());
    assert!(ui.get_ai_settings());
    finish(&fixture);
}

#[test]
#[ignore = "runs in an owned isolated child"]
fn child_provider_drafts() {
    use audiolib_ai::{
        config::{ApiKey, Profile, Protocol},
        storage::ProfileStore,
    };
    let fixture = fixture();
    let ui = &fixture.ui;
    let preferences = fixture.root.join("preferences");
    let mut store = ProfileStore::open(&preferences).unwrap();
    for (id, protocol) in [("deep", Protocol::DeepSeek), ("go", Protocol::OpenCodeGo)] {
        store
            .save(
                Profile {
                    id: id.into(),
                    name: format!("{} 1", protocol.display_name()),
                    protocol,
                    base_url: protocol.default_url().into(),
                    model: format!("{id}-saved-model"),
                    ..Default::default()
                },
                Some(ApiKey::new("owned-fixture-key".into()).unwrap()),
            )
            .unwrap();
    }
    drop(store);
    ui.invoke_ai_action(0, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    assert_eq!(ui.get_ai_provider_index(), 2);
    assert_eq!(ui.get_ai_profile_name(), "OpenCode Go 1");
    assert_eq!(ui.get_ai_model(), "go-saved-model");
    assert_eq!(ui.get_ai_selected_model(), "go-saved-model");
    assert_eq!(ui.get_ai_saved_key(), cfg!(windows));
    assert!(ui.get_ai_key().is_empty());
    ui.set_ai_settings(true);
    ui.set_ai_model("draft-model".into());
    ui.set_ai_endpoint("https://example.test/custom".into());
    ui.set_ai_key("unsaved-private-key".into());
    ui.set_ai_dirty(true);
    for _ in 0..3 {
        ui.invoke_ai_action(1, 0);
        ui.invoke_ai_action(0, 0);
        assert!(ui.get_ai_settings());
        assert_eq!(ui.get_ai_model(), "draft-model");
        assert_eq!(ui.get_ai_selected_model(), "go-saved-model");
        assert_eq!(ui.get_ai_key(), "unsaved-private-key");
        // Re-selecting the current protocol must not apply defaults again.
        ui.invoke_ai_action(12, 5);
        assert_eq!(ui.get_ai_endpoint(), "https://example.test/custom");
        assert_eq!(ui.get_ai_key(), "unsaved-private-key");
        ui.invoke_ai_action(4, 1);
        assert_eq!(ui.get_ai_profile_name(), "DeepSeek 1");
        assert_eq!(ui.get_ai_selected_model(), "deep-saved-model");
        ui.invoke_ai_action(4, 2);
        assert_eq!(ui.get_ai_model(), "draft-model");
        assert_eq!(ui.get_ai_selected_model(), "go-saved-model");
        assert_eq!(ui.get_ai_key(), "unsaved-private-key");
        assert!(ui.get_ai_dirty());
    }
    let persisted = std::fs::read_to_string(preferences.join("ai/profiles.json")).unwrap();
    assert!(!persisted.contains("unsaved-private-key"));
    assert!(!persisted.contains("draft-model"));
    ui.invoke_ai_action(18, 0);
    assert_eq!(ui.get_ai_model(), "go-saved-model");
    assert_eq!(ui.get_ai_endpoint(), Protocol::OpenCodeGo.default_url());
    assert!(ui.get_ai_key().is_empty());
    assert_eq!(ui.get_ai_saved_key(), cfg!(windows));
    assert!(!ui.get_ai_dirty());
    for number in 2..=3 {
        ui.invoke_ai_action(9, 0);
        assert!(ui.get_ai_selected_model().is_empty());
        ui.set_ai_protocol(5);
        ui.invoke_ai_action(12, 5);
        assert!(ui.get_ai_profile_name().is_empty());
        assert_eq!(ui.get_ai_auto_name(), format!("OpenCode Go {number}"));
        ui.set_ai_model("new-model".into());
        assert!(ui.get_ai_selected_model().is_empty());
        ui.set_ai_key("new-fixture-key".into());
        ui.invoke_ai_action(8, 0);
        wait_for(ui, |ui| !ui.get_ai_busy());
        assert_eq!(ui.get_ai_profile_name(), format!("OpenCode Go {number}"));
        assert_eq!(ui.get_ai_selected_model(), "new-model");
        assert!(ui.get_ai_key().is_empty());
        assert!(ui.get_ai_saved_key());
    }
    ui.set_ai_profile_name("My sound design".into());
    ui.invoke_ai_action(8, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    let saved = ProfileStore::open(&preferences).unwrap();
    assert_eq!(saved.profiles().len(), 4);
    assert_eq!(saved.selected().unwrap().name, "My sound design");
    assert_eq!(saved.has_key(saved.selected().unwrap()), cfg!(windows));
    drop(saved);
    ui.invoke_ai_action(10, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    assert_eq!(ui.get_ai_provider_index(), 0);
    assert!(ui.get_ai_selected_model().is_empty());
    finish(&fixture);
}

#[test]
fn ai_failures_publish_notices_but_cancelled_and_stale_replies_do_not() {
    isolated("ai_search::child_failure_notices");
}

#[test]
#[ignore = "runs in an owned isolated child"]
fn child_failure_notices() {
    let fixture = fixture();
    let ui = &fixture.ui;
    let profile_file = fixture.root.join("preferences/ai/profiles.json");
    std::fs::create_dir_all(profile_file.parent().unwrap()).unwrap();
    std::fs::write(&profile_file, b"{owned-invalid-profile").unwrap();
    let mut revision = ui.get_ai_notice_revision();
    ui.invoke_ai_action(0, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    revision += 1;
    assert_eq!(ui.get_ai_notice_revision(), revision);
    assert_eq!(ui.get_ai_notice_message(), ui.get_ai_status());
    assert!(ui.get_ai_error());
    assert!(!ui.get_ai_notice_message().contains("owned-invalid-profile"));
    std::fs::remove_file(&profile_file).unwrap();
    ui.invoke_ai_action(0, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    assert!(ui.get_ai_selected_model().is_empty());

    // Identical synchronous validation failures must each restart the notice.
    ui.set_ai_instruction("".into());
    for _ in 0..2 {
        ui.invoke_ai_action(15, 0);
        revision += 1;
        assert_eq!(ui.get_ai_notice_revision(), revision);
        assert_eq!(ui.get_ai_notice_message(), ui.get_ai_status());
        assert!(ui.get_ai_error());
    }
    ui.set_ai_instruction(audiolib_ai::config::DEFAULT_PROMPT.into());
    ui.invoke_ai_action(9, 0);
    ui.invoke_ai_action(8, 0);
    revision += 1;
    assert_eq!(ui.get_ai_notice_revision(), revision);
    assert_eq!(ui.get_ai_notice_message(), ui.get_ai_status());

    let (cancel_accepted, cancel_received) = std::sync::mpsc::sync_channel(1);
    let (cancel_released, cancel_release) = std::sync::mpsc::sync_channel(1);
    let (stale_accepted, stale_received) = std::sync::mpsc::sync_channel(1);
    let (stale_released, stale_release) = std::sync::mpsc::sync_channel(1);
    let (endpoint, requests, server) = owned_ai_error_server(vec![
        (401, "{\"error\":\"owned-private-server-body\"}", None),
        (200, "{\"unexpected\":\"owned-private-server-body\"}", None),
        (403, "{\"error\":\"owned-private-server-body\"}", None),
        (0, "", None),
        (403, "{}", Some((cancel_accepted, cancel_release))),
        (500, "{}", Some((stale_accepted, stale_release))),
    ]);
    ui.set_ai_endpoint(endpoint.into());
    ui.set_ai_model("saved-generation-model".into());
    ui.set_ai_key("owned-private-key".into());
    ui.invoke_ai_action(8, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    assert_eq!(ui.get_ai_selected_model(), "saved-generation-model");
    ui.set_ai_model("unsaved-draft-model".into());
    ui.set_ai_dirty(true);
    ui.set_ai_prompt("wind".into());
    for (action, expected) in [
        (2, "401"),
        (11, "model list"),
        (17, "403"),
        (17, "Connection failed"),
    ] {
        ui.invoke_ai_action(action, 0);
        wait_for(ui, |ui| !ui.get_ai_busy());
        revision += 1;
        assert_eq!(ui.get_ai_notice_revision(), revision);
        assert_eq!(ui.get_ai_notice_message(), ui.get_ai_status());
        assert!(
            ui.get_ai_notice_message().contains(expected),
            "{}",
            ui.get_ai_notice_message()
        );
        assert!(ui.get_ai_error());
        assert!(!ui.get_ai_notice_message().contains("owned-private"));
        assert_eq!(ui.get_ai_selected_model(), "saved-generation-model");
        let request = requests.recv_timeout(Duration::from_secs(5)).unwrap();
        if action == 2 {
            // The header model is the actual generation model, not the editor draft.
            assert_eq!(request["model"], "saved-generation-model");
        }
    }
    let before_cancel = ui.get_ai_notice_message();
    ui.invoke_ai_action(2, 0);
    cancel_received
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    ui.invoke_ai_action(3, 0);
    cancel_released.send(()).unwrap();
    wait_for(ui, |ui| !ui.get_ai_busy());
    assert_eq!(ui.get_ai_notice_revision(), revision);
    assert_eq!(ui.get_ai_notice_message(), before_cancel);

    ui.invoke_ai_action(2, 0);
    stale_received.recv_timeout(Duration::from_secs(5)).unwrap();
    // An unsent filter edit changes the generating request's displayed context
    // without setting its explicit cancellation flag or advancing search state.
    ui.set_duration_min("invalid-draft".into());
    stale_released.send(()).unwrap();
    wait_for(ui, |ui| !ui.get_ai_busy());
    assert_eq!(ui.get_ai_notice_revision(), revision);
    assert_eq!(ui.get_ai_notice_message(), before_cancel);
    assert!(!ui.get_ai_error());
    server.join().unwrap();
    finish(&fixture);
}

#[test]
fn ai_all_recommendations_keep_scope_and_reject_partial_creation() {
    isolated("ai_search::child_all_tasks");
}

#[test]
fn ai_tag_new_task_preserves_its_source_and_rejects_invalid_admission() {
    isolated("ai_search::child_tag_new_task");
}

#[test]
#[ignore = "runs in an owned isolated child"]
fn child_tag_new_task() {
    let fixture = fixture();
    let ui = &fixture.ui;
    ui.set_ai_auto_search(false);
    seed_ai_filename_hits(&fixture);
    choose_gore(&fixture);
    ui.set_tag_filter("gore".into());
    ui.set_search_scope_index(3);
    search(&fixture, "gore", 3);
    ui.invoke_ai_action(0, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    ui.set_ai_prompt("科幻爆炸".into());
    ui.invoke_ai_action(2, 0);
    wait_for(ui, |ui| {
        !ui.get_ai_busy() && ui.get_ai_quick_rows().row_count() == 4
    });
    fixture.controller.capture_search_task(ui);
    let (source_id, before, original_results) = {
        let state = fixture.controller.task_workspace.lock().unwrap();
        let source = &state.workspace.session.tasks[0];
        (
            source.id.clone(),
            source.definition.clone(),
            state.caches[&source.id]
                .records
                .iter()
                .map(|file| file.id)
                .collect::<Vec<_>>(),
        )
    };
    assert!(before.folder.is_some());
    assert_ne!(before.filters, audiolib_core::SearchFilters::default());
    let tag = ui.get_ai_quick_rows().row_data(1).unwrap().text;
    assert_ne!(tag, ui.get_ai_query());
    for index in [-1, 5, i32::MAX] {
        ui.invoke_ai_action(24, index);
        assert_eq!(ui.get_search_tasks().row_count(), 1);
        assert_eq!(ui.get_query(), "gore");
    }
    ui.set_duration_min("unfinished".into());
    ui.invoke_ai_action(24, 1);
    assert_eq!(ui.get_search_tasks().row_count(), 1);
    assert_eq!(ui.get_duration_min(), "unfinished");
    assert_eq!(ui.get_query(), "gore");
    ui.set_duration_min("".into());
    fixture
        .controller
        .max_search_tasks
        .store(1, Ordering::Release);
    ui.invoke_ai_action(24, 1);
    assert_eq!(ui.get_search_tasks().row_count(), 1);
    assert!(ui.get_ai_status().contains("No new tasks"));
    assert_eq!(ui.get_query(), "gore");
    fixture
        .controller
        .max_search_tasks
        .store(12, Ordering::Release);
    ui.invoke_ai_action(24, 1);
    let ready = fixture.controller.clone();
    wait_for(ui, move |ui| {
        ui.get_search_tasks().row_count() == 2
            && !ui.get_task_restoring()
            && !ui.get_searching()
            && fully_published(&ready)
    });
    assert_eq!(ui.get_query(), tag);
    {
        let state = fixture.controller.task_workspace.lock().unwrap();
        let source = &state.workspace.session.tasks[0];
        assert_eq!(source.id, source_id);
        assert_eq!(source.definition, before);
        assert_eq!(
            state.caches[&source_id]
                .records
                .iter()
                .map(|file| file.id)
                .collect::<Vec<_>>(),
            original_results
        );
        let created = &state.workspace.session.tasks[1];
        let mut expected = before.clone();
        expected.query = tag.to_string();
        assert_eq!(created.definition, expected);
        assert_eq!(
            state.workspace.session.active_task_id.as_ref(),
            Some(&created.id)
        );
        assert_eq!(state.caches[&created.id].records.len(), 1);
    }
    fixture.controller.switch_search_task(&source_id);
    wait_for(ui, |ui| !ui.get_task_restoring() && !ui.get_searching());
    assert_eq!(ui.get_query(), "gore");
    assert_eq!(ui.get_tag_filter(), "gore");
    ui.invoke_ai_action(3, 0);
    assert_eq!(ui.get_ai_quick_rows().row_count(), 0);
    ui.invoke_ai_action(24, 1);
    assert_eq!(ui.get_search_tasks().row_count(), 2);
    assert_eq!(ui.get_query(), "gore");
    finish(&fixture);
}

#[test]
#[ignore = "runs in an owned isolated child"]
fn child_all_tasks() {
    let fixture = fixture();
    let ui = &fixture.ui;
    ui.set_ai_auto_search(false);
    let expected_recommendations = seed_ai_filename_hits(&fixture);
    choose_gore(&fixture);
    ui.set_tag_filter("gore".into());
    ui.set_search_scope_index(3);
    search(&fixture, "gore", 3);
    ui.invoke_ai_action(0, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    ui.set_ai_prompt("科幻爆炸".into());
    ui.invoke_ai_action(2, 0);
    wait_for(ui, |ui| {
        !ui.get_ai_busy() && ui.get_ai_quick_rows().row_count() == 4
    });
    assert_eq!(ui.get_search_tasks().row_count(), 1);
    fixture.controller.capture_search_task(ui);
    let before = fixture
        .controller
        .task_workspace
        .lock()
        .unwrap()
        .workspace
        .session
        .tasks[0]
        .definition
        .clone();
    assert!(before.folder.is_some());
    assert_ne!(before.scope, audiolib_core::SearchScope::Main);
    assert_ne!(before.filters, audiolib_core::SearchFilters::default());
    let recommendations = ui
        .get_ai_quick_rows()
        .iter()
        .map(|row| row.text.to_string())
        .collect::<Vec<_>>();
    assert_eq!(recommendations, expected_recommendations);
    let queries = recommendations;
    fixture
        .controller
        .max_search_tasks
        .store(3, Ordering::Release);
    ui.invoke_ai_action(20, 0);
    assert_eq!(ui.get_search_tasks().row_count(), 1);
    assert!(ui.get_ai_status().contains("No new tasks"));
    fixture
        .controller
        .max_search_tasks
        .store(12, Ordering::Release);
    ui.invoke_ai_action(20, 0);
    wait_for(ui, |ui| {
        ui.get_search_tasks().row_count() == 5
            && !ui.get_ai_batch_searching()
            && !ui.get_task_restoring()
            && !ui.get_searching()
    });
    let ids = {
        let state = fixture.controller.task_workspace.lock().unwrap();
        assert_eq!(
            state.workspace.session.active_task_id.as_ref(),
            Some(&state.workspace.session.tasks[1].id)
        );
        state.workspace.session.tasks[1..]
            .iter()
            .zip(&queries)
            .map(|(task, query)| {
                let mut expected = before.clone();
                expected.query = query.clone();
                assert_eq!(task.definition, expected);
                let cache = state
                    .caches
                    .get(&task.id)
                    .expect("all recommended searches must be cached before switching");
                assert_eq!(cache.records.len(), 1);
                task.id.clone()
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(ui.get_query(), queries[0]);
    for (id, query) in ids.iter().zip(&queries) {
        fixture.controller.switch_search_task(id);
        wait_for(ui, |ui| !ui.get_task_restoring() && !ui.get_searching());
        assert_eq!(ui.get_query(), query.as_str());
    }
    ui.invoke_ai_action(3, 0);
    ui.invoke_ai_action(20, 0);
    assert_eq!(ui.get_search_tasks().row_count(), 5);
    finish(&fixture);
}

#[test]
fn ai_provider_discovery_works_before_save_and_recovers_from_auth_failure() {
    isolated("ai_search::child_provider_discovery");
}

#[test]
#[ignore = "runs in an owned isolated child"]
fn child_provider_discovery() {
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };
    let fixture = fixture();
    let ui = &fixture.ui;
    ui.invoke_ai_action(0, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    ui.invoke_ai_action(9, 0);
    ui.set_ai_protocol(6);
    ui.invoke_ai_action(12, 6);
    assert_eq!(ui.get_ai_endpoint(), "https://api.deepseek.com");
    assert!(ui.get_ai_model().is_empty());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    ui.set_ai_endpoint(format!("http://{}", listener.local_addr().unwrap()).into());
    ui.set_ai_key("draft-key".into());
    let server = std::thread::spawn(move || {
        for (step, status) in [401, 200, 200, 200].into_iter().enumerate() {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut head = vec![];
            let mut byte = [0];
            while !head.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                head.push(byte[0]);
            }
            let request = String::from_utf8(head).unwrap();
            assert!(request.starts_with(if step < 2 {
                "GET /models HTTP/1.1"
            } else {
                "POST /chat/completions HTTP/1.1"
            }));
            assert!(
                request
                    .to_lowercase()
                    .contains("authorization: bearer draft-key")
            );
            let body = if step >= 2 {
                let length: usize = request
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|s| s.trim().parse().unwrap())
                    })
                    .unwrap();
                let mut bytes = vec![0; length];
                stream.read_exact(&mut bytes).unwrap();
                let plan = audiolib_ai::reference::ReferenceIndex::embedded()
                    .local_plan("wind", false)
                    .unwrap();
                serde_json::json!({"choices":[{"finish_reason":"stop","message":{"content":serde_json::to_string(&plan).unwrap()}}],"usage":{"prompt_tokens":10,"completion_tokens":20}}).to_string()
            } else if status == 200 {
                r#"{"data":[{"id":"deepseek-v4-pro"},{"id":"deepseek-flash"}]}"#.to_owned()
            } else {
                "{}".to_owned()
            };
            write!(stream, "HTTP/1.1 {status} Response\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        }
    });
    ui.invoke_ai_action(11, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    assert!(ui.get_ai_status().contains("401"));
    assert!(ui.get_ai_profile_id().is_empty());
    assert_eq!(ui.get_ai_key(), "draft-key");
    ui.invoke_ai_action(11, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    assert_eq!(ui.get_ai_models().row_count(), 2);
    assert_eq!(ui.get_ai_model(), "deepseek-flash");
    assert!(ui.get_ai_profile_id().is_empty());
    assert!(!fixture.root.join("preferences/ai/profiles.json").exists());
    ui.invoke_ai_action(17, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    assert!(ui.get_ai_status().contains("Model test passed"));
    assert!(!fixture.root.join("preferences/ai/profiles.json").exists());
    ui.invoke_ai_action(8, 0);
    wait_for(ui, |ui| {
        !ui.get_ai_busy() && ui.get_ai_provider_index() == 1
    });
    let store =
        audiolib_ai::storage::ProfileStore::open(&fixture.root.join("preferences")).unwrap();
    assert_eq!(
        store.profiles()[0].protocol,
        audiolib_ai::config::Protocol::DeepSeek
    );
    assert_eq!(store.profiles()[0].model, "deepseek-flash");
    assert!(ui.get_ai_key().is_empty());
    ui.invoke_ai_action(17, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    assert!(ui.get_ai_status().contains("Model test passed"));
    let usage = audiolib_ai::storage::ProfileStore::open(&fixture.root.join("preferences"))
        .unwrap()
        .usage(ui.get_ai_profile_id().as_str());
    assert_eq!(usage.requests, 1);
    assert_eq!(usage.output_tokens, 20);
    // A stored key must never follow an unsaved endpoint edit.
    let other = TcpListener::bind("127.0.0.1:0").unwrap();
    other.set_nonblocking(true).unwrap();
    ui.set_ai_endpoint(format!("http://{}", other.local_addr().unwrap()).into());
    ui.invoke_ai_action(11, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    assert_eq!(ui.get_ai_models().row_count(), 0);
    assert_eq!(
        other.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    server.join().unwrap();
    finish(&fixture);
}

#[test]
fn jev_provider_probe_uses_keyword_choices_and_preserves_unsaved_drafts() {
    isolated("ai_search::child_jev_provider_probe");
}

#[test]
#[ignore = "runs in an owned isolated child"]
fn child_jev_provider_probe() {
    use serde_json::{Value, json};
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };

    let fixture = fixture();
    let ui = &fixture.ui;
    ui.invoke_ai_action(0, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    ui.invoke_ai_action(9, 0);
    ui.set_ai_protocol(4);
    ui.invoke_ai_action(12, 4);
    ui.set_ai_profile_name("Jev test".into());
    ui.set_ai_model("jev-latest".into());
    ui.set_ai_key("draft-jev-key".into());
    ui.set_ai_prompt("private-current-sound-description".into());
    ui.set_ai_instruction("private-custom-instruction".into());
    ui.set_ai_layers(true);
    ui.set_ai_auto_search(true);
    let original_task_count = ui.get_search_tasks().row_count();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    ui.set_ai_endpoint(format!("http://{}", listener.local_addr().unwrap()).into());
    let private_root = fixture.root.to_string_lossy().into_owned();
    let server = std::thread::spawn(move || {
        for step in 0..4 {
            // A missing network request must fail this regression promptly,
            // rather than leave a server blocked in accept after a local error.
            let started = Instant::now();
            let (mut stream, _) = loop {
                match listener.accept() {
                    Ok(connection) => break connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(started.elapsed() < Duration::from_secs(8));
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("owned probe listener failed: {error}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut head = Vec::new();
            let mut byte = [0];
            while !head.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                head.push(byte[0]);
            }
            let head = String::from_utf8(head).unwrap();
            assert!(head.starts_with("POST /systemone HTTP/1.1"));
            assert!(
                head.to_ascii_lowercase()
                    .contains("authorization: bearer draft-jev-key")
            );
            let length: usize = head
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|value| value.trim().parse().unwrap())
                })
                .unwrap();
            let mut bytes = vec![0; length];
            stream.read_exact(&mut bytes).unwrap();
            let body = String::from_utf8(bytes).unwrap();
            assert!(!body.contains("private-current-sound-description"));
            assert!(!body.contains("private-custom-instruction"));
            assert!(!body.contains(&private_root));
            let request: Value = serde_json::from_str(&body).unwrap();
            assert_eq!(request["model"], "jev-latest");
            assert_eq!(request["state"]["intent"], "wind");
            assert_eq!(request["state"]["mode"], "direct");
            let questions = request["questions"].as_object().unwrap();
            let criteria = questions["recipe"]["criteria"].as_object().unwrap();
            let keyword = request["state"]["candidates"]
                .as_object()
                .unwrap()
                .iter()
                .find_map(|(id, option)| {
                    (option["kind"] == "keyword"
                        && option["queries"]
                            .as_array()
                            .is_some_and(|queries| queries.iter().any(|query| query == "wind")))
                    .then(|| id.clone())
                })
                .expect("wind must be submitted as a structured keyword choice");
            let chosen = if step == 1 {
                "other_or_unclear"
            } else {
                &keyword
            };
            let probability = if step == 2 { 0.55 } else { 0.95 };
            let alternative = if step == 1 {
                &keyword
            } else {
                "other_or_unclear"
            };
            let probabilities = criteria
                .keys()
                .map(|id| {
                    (
                        id.clone(),
                        json!(if id == chosen {
                            probability
                        } else if id == alternative {
                            1.0 - probability
                        } else {
                            0.0
                        }),
                    )
                })
                .collect::<serde_json::Map<_, _>>();
            let uniform = 1.0 / criteria.len() as f64;
            let confidence = (probability - uniform) / (1.0 - uniform);
            let answers = questions.iter().map(|(id, question)| {
                let answer = match question["type"].as_str().unwrap() {
                    "choice" => json!({"type":"choice", "choice":chosen, "confidence":confidence, "probabilities":probabilities}),
                    "score" => {
                        let selected = id == &format!("fit_{keyword}");
                        json!({"type":"score", "score":if selected {2.0} else {0.0}, "confidence":1.0,
                            "probabilities":{"0":if selected {0.0} else {1.0},"1":0.0,"2":if selected {1.0} else {0.0}}})
                    }
                    "noul" => json!({"type":"noul", "noul":0.1}),
                    other => panic!("unexpected Jev question type: {other}"),
                };
                (id.clone(), answer)
            }).collect::<serde_json::Map<_, _>>();
            let response =
                json!({"answers":answers,"usage":{"input_tokens":40,"output_tokens":20}})
                    .to_string();
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).unwrap();
        }
    });

    for step in 0..3 {
        ui.invoke_ai_action(17, 0);
        wait_for(ui, |ui| !ui.get_ai_busy());
        if step == 0 {
            assert!(!ui.get_ai_error());
            assert!(ui.get_ai_status().contains("Model test passed"));
        } else {
            assert!(ui.get_ai_error());
            assert!(ui.get_ai_status().contains("Jev decision uncertain"));
            assert!(!ui.get_ai_status().contains("Model test passed"));
        }
        assert!(ui.get_ai_profile_id().is_empty());
        assert_eq!(ui.get_ai_key(), "draft-jev-key");
        assert_eq!(ui.get_ai_prompt(), "private-current-sound-description");
        assert_eq!(ui.get_search_tasks().row_count(), original_task_count);
        assert_eq!(ui.get_ai_quick_rows().row_count(), 0);
        assert!(!fixture.root.join("preferences/ai/profiles.json").exists());
    }
    ui.invoke_ai_action(8, 0);
    wait_for(ui, |ui| {
        !ui.get_ai_busy() && ui.get_ai_provider_index() == 1
    });
    assert!(ui.get_ai_key().is_empty());
    ui.invoke_ai_action(17, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    assert!(!ui.get_ai_error());
    assert!(ui.get_ai_status().contains("Model test passed"));
    let usage = audiolib_ai::storage::ProfileStore::open(&fixture.root.join("preferences"))
        .unwrap()
        .usage(ui.get_ai_profile_id().as_str());
    assert_eq!(usage.requests, 1);
    assert_eq!(usage.output_tokens, 20);
    assert_eq!(ui.get_search_tasks().row_count(), original_task_count);
    server.join().unwrap();
    finish(&fixture);
}

#[test]
fn ai_magpie_discovers_probes_and_reopens_without_replacing_saved_providers() {
    isolated("ai_search::child_magpie_provider");
}

#[test]
#[ignore = "runs in an owned isolated child"]
fn child_magpie_provider() {
    use audiolib_ai::{
        config::{ApiKey, Profile, Protocol},
        storage::ProfileStore,
    };
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };
    let fixture = fixture();
    let ui = &fixture.ui;
    let preferences = fixture.root.join("preferences");
    let mut store = ProfileStore::open(&preferences).unwrap();
    let existing = Profile {
        name: "Existing DeepSeek".into(),
        protocol: Protocol::DeepSeek,
        base_url: Protocol::DeepSeek.default_url().into(),
        model: "saved-model".into(),
        ..Default::default()
    };
    store
        .save(
            existing.clone(),
            Some(ApiKey::new("existing-fixture-key".into()).unwrap()),
        )
        .unwrap();
    let retained = store.profiles()[0].clone();
    drop(store);
    ui.invoke_ai_action(0, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    ui.invoke_ai_action(9, 0);
    ui.set_ai_settings(true);
    ui.set_ai_protocol(7);
    ui.invoke_ai_action(12, 7);
    assert_eq!(ui.get_ai_endpoint(), "http://127.0.0.1:3425/v1");
    assert_eq!(ui.get_ai_auto_name(), "Magpie 1");
    assert!(ui.get_ai_model().is_empty());
    assert!(ui.get_ai_key().is_empty());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
    ui.set_ai_endpoint(endpoint.clone().into());
    let server = std::thread::spawn(move || {
        for step in 0..3 {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut head = vec![];
            let mut byte = [0];
            while !head.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                head.push(byte[0]);
            }
            let request = String::from_utf8(head).unwrap();
            assert!(request.starts_with(if step == 0 {
                "GET /v1/models HTTP/1.1"
            } else {
                "POST /v1/chat/completions HTTP/1.1"
            }));
            let headers = request.to_ascii_lowercase();
            assert!(headers.contains("user-agent: audiolibexplorer/"));
            assert!(!headers.contains("existing-fixture-key"));
            let body = if step == 0 {
                serde_json::json!({"data":[{"id":"vendor/model"},{"id":"group/sound-design"},{"id":"models/retained-prefix"}]}).to_string()
            } else {
                let length: usize = request
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|s| s.trim().parse().unwrap())
                    })
                    .unwrap();
                let mut bytes = vec![0; length];
                stream.read_exact(&mut bytes).unwrap();
                let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(body["model"], "models/retained-prefix");
                let plan = audiolib_ai::reference::ReferenceIndex::embedded()
                    .local_plan("wind", false)
                    .unwrap();
                serde_json::json!({"choices":[{"finish_reason":"stop","message":{"content":serde_json::to_string(&plan).unwrap()}}],"usage":{"prompt_tokens":10,"completion_tokens":20}}).to_string()
            };
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        }
    });
    ui.invoke_ai_action(11, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    assert_eq!(
        ui.get_ai_models()
            .iter()
            .map(|model| model.to_string())
            .collect::<Vec<_>>(),
        [
            "group/sound-design",
            "models/retained-prefix",
            "vendor/model"
        ]
    );
    assert_eq!(ui.get_ai_model(), "group/sound-design");
    assert!(ui.get_ai_profile_id().is_empty());
    ui.set_ai_model("models/retained-prefix".into());
    // Re-selecting Magpie preserves a customized port and the chosen raw model ID.
    ui.invoke_ai_action(12, 7);
    assert_eq!(ui.get_ai_endpoint(), endpoint.as_str());
    assert_eq!(ui.get_ai_model(), "models/retained-prefix");
    ui.invoke_ai_action(17, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    assert!(ui.get_ai_status().contains("Model test passed"));
    assert_eq!(
        ProfileStore::open(&preferences).unwrap().profiles().len(),
        1
    );
    ui.invoke_ai_action(8, 0);
    wait_for(ui, |ui| {
        !ui.get_ai_busy() && ui.get_ai_provider_index() == 2
    });
    assert_eq!(ui.get_ai_profile_name(), "Magpie 1");
    let reopened = ProfileStore::open(&preferences).unwrap();
    assert_eq!(reopened.profiles().len(), 2);
    assert_eq!(reopened.profiles()[0], retained);
    assert_eq!(reopened.selected().unwrap().protocol, Protocol::Magpie);
    assert_eq!(reopened.selected().unwrap().model, "models/retained-prefix");
    assert_eq!(reopened.selected().unwrap().base_url, endpoint);
    assert!(
        reopened
            .key(reopened.selected().unwrap())
            .unwrap()
            .is_empty()
    );
    ui.invoke_ai_action(1, 0);
    ui.invoke_ai_action(0, 0);
    assert_eq!(ui.get_ai_protocol(), 7);
    assert_eq!(ui.get_ai_model(), "models/retained-prefix");
    ui.invoke_ai_action(17, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    assert!(ui.get_ai_status().contains("Model test passed"));
    server.join().unwrap();
    finish(&fixture);
}

#[test]
fn repeated_ai_search_and_quick_queries_survive_result_refresh() {
    isolated("ai_search::child_repeated_ai_search");
}

#[test]
#[ignore = "runs in an owned isolated child"]
fn child_repeated_ai_search() {
    let fixture = fixture();
    let ui = &fixture.ui;
    ui.set_ai_auto_search(false);
    seed_ai_notes(&fixture, 3);
    ui.invoke_ai_action(0, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    ui.set_ai_prompt("科幻爆炸".into());
    ui.invoke_ai_action(2, 0);
    wait_for(ui, |ui| {
        !ui.get_ai_busy() && ui.get_ai_rows().row_count() > 0
    });
    assert_eq!(ui.get_ai_quick_rows().row_count(), 4);
    ui.set_ai_query("wind".into());
    ui.invoke_ai_action(6, 0);
    wait_for(ui, |ui| {
        !ui.get_searching() && ui.get_rows().row_count() == 1
    });
    let file = fixture.controller.records.lock().unwrap()[0].clone();
    fixture
        .engine
        .handle()
        .send_and_wait(
            AudioCommand::Load(PathBuf::from(&file.file_path)),
            Duration::from_secs(4),
        )
        .unwrap();
    wait_for(ui, |ui| ui.get_audio_ready() && ui.get_waveform_ready());
    let preview = fixture.engine.handle().status();
    // Preview and result-window refreshes may advance read admission while the
    // completed keyword plan is unchanged. Reusing it must still start a search.
    for query in ["gore", "wind", "gore", "wind"] {
        fixture.controller.refresh_result_window(ui);
        wait_for(ui, |ui| !ui.get_searching());
        ui.set_ai_query(query.into());
        ui.invoke_ai_action(6, 0);
        let expected = if query == "gore" { 3 } else { 1 };
        wait_for(ui, move |ui| {
            !ui.get_searching() && ui.get_rows().row_count() == expected
        });
        assert_eq!(ui.get_query(), query);
        assert_eq!(
            fixture.engine.handle().status().generation,
            preview.generation
        );
        assert_eq!(
            fixture.engine.handle().status().play_session_id,
            preview.play_session_id
        );
    }
    // Closing AI returns the editor to ordinary keywords, without losing shortcuts.
    ui.invoke_ai_action(1, 0);
    assert!(!ui.get_ai_visible());
    for index in 0..3 {
        let query = ui.get_ai_quick_rows().row_data(index).unwrap().text;
        ui.invoke_ai_action(16, index as i32);
        wait_for(ui, |ui| !ui.get_searching());
        assert_eq!(ui.get_query(), query);
        assert_eq!(
            fixture.controller.request.lock().unwrap().query,
            query.as_str()
        );
        assert_eq!(ui.get_search_tasks().row_count(), 1);
    }
    ui.invoke_ai_action(3, 0);
    assert_eq!(ui.get_ai_quick_rows().row_count(), 0);
    let last = ui.get_query();
    ui.invoke_ai_action(16, 0);
    assert_eq!(ui.get_query(), last);
    finish(&fixture);
}

#[test]
fn local_ai_queries_preserve_scope_and_reject_stale_results() {
    isolated("ai_search::child_local_ai");
}

#[test]
#[ignore = "runs in an owned isolated child"]
fn child_local_ai() {
    let fixture = fixture();
    let ui = &fixture.ui;
    ui.set_ai_auto_search(false);
    seed_ai_notes(&fixture, 3);
    ui.invoke_ai_action(0, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    assert_eq!(ui.get_ai_instruction(), audiolib_ai::config::DEFAULT_PROMPT);
    ui.set_ai_prompt("我要设计科幻爆炸声".into());
    ui.invoke_ai_action(2, 0);
    wait_for(ui, |ui| {
        !ui.get_ai_busy() && ui.get_ai_rows().row_count() > 0
    });
    assert_eq!(ui.get_ai_query(), "plasma");
    ui.set_ai_query("wind".into());
    ui.invoke_ai_action(6, 0);
    wait_for(ui, |ui| {
        !ui.get_searching() && ui.get_rows().row_count() == 1
    });
    assert_eq!(ui.get_query(), "wind");
    // A cancelled result never changes the displayed query.
    ui.invoke_ai_action(3, 0);
    ui.set_ai_query("gore".into());
    ui.invoke_ai_action(6, 0);
    assert_eq!(ui.get_query(), "wind");
    // New tasks retain the exact current definition except the query.
    ui.set_ai_prompt("风".into());
    ui.invoke_ai_action(2, 0);
    wait_for(ui, |ui| {
        !ui.get_ai_busy() && ui.get_ai_rows().row_count() > 0
    });
    fixture.controller.capture_search_task(ui);
    let before = {
        let state = fixture.controller.task_workspace.lock().unwrap();
        state
            .workspace
            .session
            .tasks
            .iter()
            .find(|t| Some(&t.id) == state.workspace.session.active_task_id.as_ref())
            .unwrap()
            .definition
            .clone()
    };
    ui.set_ai_query("gore".into());
    ui.invoke_ai_action(7, 0);
    wait_for(ui, |ui| {
        ui.get_search_tasks().row_count() == 2 && !ui.get_task_restoring() && !ui.get_searching()
    });
    fixture.controller.capture_search_task(ui);
    let after = {
        let state = fixture.controller.task_workspace.lock().unwrap();
        state
            .workspace
            .session
            .tasks
            .iter()
            .find(|t| Some(&t.id) == state.workspace.session.active_task_id.as_ref())
            .unwrap()
            .definition
            .clone()
    };
    let mut expected = before;
    expected.query = "gore".into();
    assert_eq!(after, expected);
    // Saving resolves DPAPI outside the event loop and retains the selected profile.
    ui.invoke_ai_action(9, 0);
    ui.set_ai_profile_name("Test".into());
    ui.set_ai_model("test-model".into());
    ui.set_ai_key("isolated-test-key".into());
    ui.invoke_ai_action(8, 0);
    wait_for(ui, |ui| {
        !ui.get_ai_busy() && ui.get_ai_provider_index() == 1
    });
    assert!(ui.get_ai_key().is_empty());
    let stored =
        std::fs::read_to_string(fixture.root.join("preferences/ai/profiles.json")).unwrap();
    assert!(!stored.contains("isolated-test-key"));
    ui.invoke_ai_action(10, 0);
    wait_for(ui, |ui| {
        !ui.get_ai_busy() && ui.get_ai_providers().row_count() == 2
    });
    ui.set_ai_instruction("优先推荐可用于分层的原始声音素材".into());
    ui.set_ai_preset_index(1);
    ui.invoke_ai_action(15, 0);
    wait_for(ui, |ui| !ui.get_ai_busy());
    let saved: audiolib_ai::storage::ProfileStore =
        audiolib_ai::storage::ProfileStore::open(&fixture.root.join("preferences")).unwrap();
    assert_eq!(
        saved.prompt().effective(),
        "优先推荐可用于分层的原始声音素材"
    );
    finish(&fixture);
}
