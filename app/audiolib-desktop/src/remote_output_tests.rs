// app/audiolib-desktop/src/remote_output_tests.rs
//! Owned four-channel fixtures exercise remote route replacement without audio output.
use super::*;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};

const ROOT_ENV: &str = "AUDIOLIB_REMOTE_OUTPUT_TEST_ROOT";

#[test]
fn remote_source_pair_reloads_preview_and_stop_cancels_preparation() {
    let root = std::env::temp_dir().join(format!(
        "audiolib-remote-output-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "controller::remote_output_tests::child_remote_source_pair",
            "--ignored",
            "--test-threads=1",
        ])
        .env(ROOT_ENV, &root)
        .output()
        .unwrap();
    assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
    if root.exists() {
        std::fs::remove_dir_all(root).unwrap();
    }
    assert!(
        output.status.success() && String::from_utf8_lossy(&output.stdout).contains("1 passed"),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn wav(channels: u16) -> Vec<u8> {
    wav_frames(channels, 1000)
}

fn wav_frames(channels: u16, frames: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    let data_len = u32::from(channels) * 2 * frames;
    bytes.extend(b"RIFF");
    bytes.extend((36 + data_len).to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16u32.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(channels.to_le_bytes());
    bytes.extend(1000u32.to_le_bytes());
    bytes.extend((u32::from(channels) * 2000).to_le_bytes());
    bytes.extend((channels * 2).to_le_bytes());
    bytes.extend(16u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend(data_len.to_le_bytes());
    for _ in 0..frames {
        for channel in 0..channels {
            bytes.extend(((channel + 1) as i16 * 1000).to_le_bytes());
        }
    }
    bytes
}

fn server(requests: Arc<Mutex<Vec<String>>>, delayed: bool) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut head = String::new();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                    break;
                }
                head.push_str(&line);
            }
            requests.lock().unwrap().push(head.clone());
            let stereo = head.lines().next().unwrap_or_default().contains("ch=2");
            if delayed && !stereo {
                std::thread::sleep(Duration::from_millis(500));
            }
            let body = wav(if stereo { 2 } else { 4 });
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(reply.as_bytes());
            let _ = stream.write_all(&body);
        }
    });
    url
}

#[test]
#[ignore = "isolated child of the remote output regression"]
fn child_remote_source_pair() {
    i_slint_backend_testing::init_no_event_loop();
    let root = PathBuf::from(std::env::var_os(ROOT_ENV).unwrap());
    assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
    std::fs::create_dir_all(root.join("sounds")).unwrap();
    std::fs::write(root.join("sounds/four-channel.wav"), wav(4)).unwrap();
    let provider = Arc::new(SqliteProvider::open(&root.join("library.db")).unwrap());
    ScanService::new(provider.clone())
        .scan(
            &root.join("sounds"),
            &ScanOptions::default(),
            &ScanCancellation::default(),
            |_| {},
        )
        .unwrap();
    let file = provider.search(&SearchRequest::default()).unwrap().results[0].clone();
    for delayed in [false, true] {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let url = server(requests.clone(), delayed);
        let mut config =
            AppConfig::load_in(root.join(if delayed { "cancel" } else { "replace" })).unwrap();
        config.data_source.server_url = url.clone();
        config.data_source.active = Some("fixture".into());
        config
            .data_source
            .profiles
            .push(audiolib_core::config::ServerProfile {
                id: "fixture".into(),
                server_url: url,
                transcode: Some(true),
                ..Default::default()
            });
        let playback = PlaybackConfig::default();
        let engine = audiolib_audio::AudioEngine::new(playback.clone()).unwrap();
        let ui = AppWindow::new().unwrap();
        let controller = Controller::new(
            &ui,
            provider.clone(),
            engine.handle(),
            config,
            playback.clone(),
        );
        controller.remote_mode.store(true, Ordering::Release);
        controller.remote.lock().unwrap().state = Some(audiolib_remote::CacheState {
            library_id: format!("fixture-{delayed}"),
            database_file: "fixture.db".into(),
            ..Default::default()
        });
        let first = controller
            .playable_path(&file, &AtomicBool::new(false), 0)
            .unwrap();
        engine
            .handle()
            .send_and_wait(AudioCommand::Load(first.clone()), Duration::from_secs(4))
            .unwrap();
        engine
            .handle()
            .send_and_wait(AudioCommand::Seek(0.25), Duration::from_secs(4))
            .unwrap();
        let audition = AuditionSettings {
            pitch_semitones: 3.0,
            rate: 0.5,
            mono: true,
            loop_enabled: true,
        };
        engine
            .handle()
            .send_and_wait(AudioCommand::SetAudition(audition), Duration::from_secs(4))
            .unwrap();
        let before = engine.handle().status();
        assert_eq!(before.channels, 2);
        let output = PlaybackConfig {
            source_pair: 1,
            audition,
            ..playback
        };
        if delayed {
            let c = controller.clone();
            let pending = std::thread::spawn(move || c.configure_output(output));
            let deadline = Instant::now() + Duration::from_secs(3);
            while requests.lock().unwrap().len() < 2 && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            assert_eq!(requests.lock().unwrap().len(), 2);
            controller
                .enqueue_stop()
                .unwrap()
                .wait(Duration::from_secs(4))
                .unwrap();
            assert!(pending.join().unwrap().is_err());
            let stopped = engine.handle().status();
            assert_eq!(stopped.file_path, Some(first));
            assert_eq!(stopped.state, PlaybackState::Stopped);
            assert_eq!(controller.playback.lock().unwrap().source_pair, 0);
        } else {
            controller.configure_output(output.clone()).unwrap();
            let after = engine.handle().status();
            assert_eq!(after.channels, 4);
            assert_eq!(after.generation, before.generation);
            assert_eq!(after.play_session_id, before.play_session_id);
            assert_eq!(after.position, before.position);
            assert_eq!(after.state, before.state);
            assert_eq!(after.audition, output.audition);
            assert_ne!(after.file_path.as_ref(), Some(&first));
            assert_eq!(
                controller.remote_audio_id(after.file_path.as_ref().unwrap()),
                Some(file.id)
            );
            assert_eq!(
                controller.playback_display_name(after.file_path.as_ref().unwrap()),
                file.filename
            );
            assert!(!audiolib_audio::is_stream_source(&first));
            let heads = requests.lock().unwrap();
            assert!(heads[0].contains("ch=2"));
            assert!(!heads[1].lines().next().unwrap().contains("ch="));
        }
        controller.shutdown();
        drop(controller);
        drop(ui);
        drop(engine);
    }
    drop(provider);
}

#[test]
fn stop_interrupts_the_loaded_stream_while_a_route_handoff_is_queued() {
    let root = std::env::temp_dir().join(format!("audiolib-route-stop-{}", std::process::id()));
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "controller::remote_output_tests::child_stop_queued_route",
            "--ignored",
            "--test-threads=1",
        ])
        .env(ROOT_ENV, &root)
        .output()
        .unwrap();
    assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
    if root.exists() {
        std::fs::remove_dir_all(root).unwrap();
    }
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "isolated loaded-stream Stop/handoff regression"]
fn child_stop_queued_route() {
    i_slint_backend_testing::init_no_event_loop();
    let root = PathBuf::from(std::env::var_os(ROOT_ENV).unwrap());
    std::fs::create_dir_all(root.join("sounds")).unwrap();
    std::fs::write(root.join("sounds/four-channel.wav"), wav(4)).unwrap();
    let provider = Arc::new(SqliteProvider::open(&root.join("library.db")).unwrap());
    ScanService::new(provider.clone())
        .scan(
            &root.join("sounds"),
            &ScanOptions::default(),
            &ScanCancellation::default(),
            |_| {},
        )
        .unwrap();
    let file = provider
        .search(&SearchRequest::default())
        .unwrap()
        .results
        .remove(0);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let release_rx = Arc::new(Mutex::new(release_rx));
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let release_rx = release_rx.clone();
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut head = String::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                        break;
                    }
                    head.push_str(&line);
                }
                let stereo = head.lines().next().unwrap_or_default().contains("ch=2");
                let body = if stereo {
                    wav_frames(2, 100_000)
                } else {
                    wav(4)
                };
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(header.as_bytes());
                if stereo {
                    let _ = stream.write_all(&body[..32_768]);
                    let _ = release_rx
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(10));
                } else {
                    let _ = stream.write_all(&body);
                }
            });
        }
    });
    let mut config = AppConfig::load_in(root.join("preferences")).unwrap();
    config.data_source.server_url = url.clone();
    config.data_source.active = Some("fixture".into());
    config
        .data_source
        .profiles
        .push(audiolib_core::config::ServerProfile {
            id: "fixture".into(),
            server_url: url,
            transcode: Some(true),
            ..Default::default()
        });
    let playback = PlaybackConfig::default();
    let engine = audiolib_audio::AudioEngine::new(playback.clone()).unwrap();
    let ui = AppWindow::new().unwrap();
    let controller = Controller::new(&ui, provider, engine.handle(), config, playback.clone());
    controller.remote_mode.store(true, Ordering::Release);
    controller.remote.lock().unwrap().state = Some(audiolib_remote::CacheState {
        library_id: "fixture-stalled".into(),
        database_file: "fixture.db".into(),
        ..Default::default()
    });
    let first = controller
        .playable_path(&file, &AtomicBool::new(false), 0)
        .unwrap();
    engine
        .handle()
        .send_and_wait(AudioCommand::Load(first.clone()), Duration::from_secs(4))
        .unwrap();
    let old = controller
        .remote_stream
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .clone();
    assert!(!old.stream.is_complete());
    // Force the worker's next decoder open to wait on bytes absent from the loaded
    // stream. ConfigureSource is queued behind that real network read, without a
    // hardware output stream or user audio. Stop must still reach the old reader.
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let source = old.stream.clone();
    let opener: audiolib_audio::SourceOpener = Arc::new(move || {
        let mut reader = source.reader()?;
        reader.seek(SeekFrom::Start(60_000))?;
        entered_tx.send(()).unwrap();
        reader.read_exact(&mut [0; 16])?;
        reader.seek(SeekFrom::Start(0))?;
        Ok((Box::new(reader), 400_044))
    });
    audiolib_audio::register_stream_source(&first, opener);
    let loading = engine
        .handle()
        .enqueue_with_ack(AudioCommand::Load(first.clone()))
        .unwrap();
    entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    let c = controller.clone();
    let pending = std::thread::spawn(move || {
        c.configure_output(PlaybackConfig {
            source_pair: 1,
            ..playback
        })
    });
    let deadline = Instant::now() + Duration::from_secs(3);
    let prepared = loop {
        if let Some(current) = controller
            .remote_stream
            .lock()
            .unwrap()
            .as_ref()
            .filter(|s| s.path != first)
            .cloned()
        {
            break current;
        }
        assert!(
            Instant::now() < deadline,
            "route preparation did not finish"
        );
        std::thread::sleep(Duration::from_millis(5));
    };
    assert!(controller.remote_retained_stream.lock().unwrap().is_some());
    // A registration transaction must await the definitive ack even beyond the
    // ordinary four-second command timeout. It cannot restore/unregister a
    // representation while a queued ConfigureSource may still execute later.
    std::thread::sleep(Duration::from_millis(4200));
    assert!(
        !pending.is_finished(),
        "route handoff rolled back before worker acknowledgment"
    );
    let started = Instant::now();
    let stopped = controller
        .enqueue_stop()
        .unwrap()
        .wait(Duration::from_secs(2))
        .unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "Stop waited for the stalled download"
    );
    // An interrupted Load discards its half-loaded source before Stop; Empty is
    // the existing engine contract and it must never become a late Ready/Play.
    assert_eq!(stopped.state, PlaybackState::Empty);
    assert_eq!(stopped.file_path, None);
    assert!(loading.wait_until_completed().is_err());
    assert!(pending.join().unwrap().is_err());
    assert!(controller.remote_retained_stream.lock().unwrap().is_none());
    assert!(!audiolib_audio::is_stream_source(&prepared.path));
    controller.shutdown();
    assert!(!audiolib_audio::is_stream_source(&old.path));
    let _ = release_tx.send(());
}
