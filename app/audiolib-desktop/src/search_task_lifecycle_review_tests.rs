// app/audiolib-desktop/src/search_task_lifecycle_review_tests.rs
//! Failure, close and loaded-audio regressions for reviewed task transitions.
use super::super::{SearchTask, WorkspaceResume, resolve_task};
use super::*;

fn wait_task(fixture: &Fixture, id: &str) {
    let wanted = id.to_owned();
    wait_for(&fixture.ui, move |ui| {
        active_id(ui) == wanted && !ui.get_search_workspace_loading() && !ui.get_task_restoring()
    });
}

fn two_scoped_tasks(fixture: &Fixture) -> (String, String, SearchTask) {
    search(fixture, "gore", 3);
    choose_gore(fixture);
    let first = active_id(&fixture.ui);
    fixture.ui.invoke_search_task_created();
    wait_for(&fixture.ui, |ui| {
        ui.get_search_tasks().row_count() == 2
            && !ui.get_search_workspace_loading()
            && !ui.get_task_restoring()
            && !ui.get_searching()
    });
    search(fixture, "wind", 1);
    let second = active_id(&fixture.ui);
    let wind = fixture.controller.records.lock().unwrap()[0].clone();
    let folder = fixture
        .provider
        .get_folder(wind.folder_id.unwrap())
        .unwrap();
    fixture.controller.capture_search_task(&fixture.ui);
    fixture.ui.invoke_search_task_selected(first.clone().into());
    wait_task(fixture, &first);
    let task = {
        let mut state = fixture.controller.task_workspace.lock().unwrap();
        let task = state
            .workspace
            .session
            .tasks
            .iter_mut()
            .find(|task| task.id == second)
            .unwrap();
        task.definition.folder = Some(super::super::folder_ref(&folder));
        task.view.selected_audio = Some(super::super::audio_ref(&wind));
        task.view.checked_audio = vec![super::super::audio_ref(&wind)];
        task.view.selection_anchor = Some(super::super::audio_ref(&wind));
        task.clone()
    };
    (first, second, task)
}

#[test]
fn failed_restore_keeps_the_target_scope_and_owned_state_until_f5_retries() {
    isolated("lifecycle_review::child_failed_restore");
}

#[test]
#[ignore = "isolated task restore error and retry helper"]
fn child_failed_restore() {
    let fixture = fixture();
    let ui = &fixture.ui;
    let controller = &fixture.controller;
    let (_, second, target) = two_scoped_tasks(&fixture);
    let old_folder = controller.request.lock().unwrap().folder_id;
    assert_ne!(
        old_folder,
        target.definition.folder.as_ref().map(|folder| folder.id)
    );
    controller.capture_search_task(ui);
    let (_, _, epoch) = controller.prepare_search_task_restore(ui, &second).unwrap();
    // The real completion boundary receives the same error as a failed SQLite
    // identity/revision read, without damaging or locking even the owned library.
    controller.finish_search_task_restore(
        ui,
        &second,
        epoch,
        Err("injected identity read failure".into()),
    );
    assert_eq!(active_id(ui), second);
    assert_eq!(
        controller.request.lock().unwrap().folder_id,
        target.definition.folder.as_ref().map(|folder| folder.id)
    );
    assert!(controller.tree.lock().unwrap().selected.is_none());
    assert!(controller.multi_selection.lock().unwrap().is_empty());
    assert!(controller.selected.lock().unwrap().is_none());
    assert_eq!(ui.get_checked_count(), 0);
    assert_eq!(ui.get_selected_folder_path(), "");
    ui.set_query("gore".into());
    controller.request_navigation_search(ui, true, false);
    controller.capture_search_task(ui);
    assert!(controller.search_job.lock().unwrap().is_none());
    assert_eq!(
        controller
            .task_workspace
            .lock()
            .unwrap()
            .workspace
            .session
            .tasks
            .iter()
            .find(|task| task.id == second)
            .unwrap(),
        &target
    );
    ui.invoke_refresh_requested();
    let wanted = second.clone();
    wait_for(ui, move |ui| {
        active_id(ui) == wanted
            && !ui.get_search_workspace_loading()
            && !ui.get_task_restoring()
            && !ui.get_searching()
            && ui.get_query() == "wind"
            && ui.get_rows().row_count() == 1
    });
    assert_eq!(
        controller.request.lock().unwrap().folder_id,
        target.definition.folder.as_ref().map(|folder| folder.id)
    );
    assert_eq!(
        ui.get_selected_id().as_str(),
        target.view.selected_audio.as_ref().unwrap().id.to_string()
    );
    assert_eq!(fixture.engine.handle().status().generation, 0);
    finish(&fixture);
}

#[test]
fn failed_close_resumes_pending_task_and_initial_workspace_reads() {
    isolated("lifecycle_review::child_close_during_restore");
}

#[test]
#[ignore = "isolated task restore and failed-close ordering helper"]
fn child_close_during_restore() {
    let fixture = fixture();
    let controller = &fixture.controller;
    let ui = &fixture.ui;
    let (_, second, _) = two_scoped_tasks(&fixture);
    let (task, cache, epoch) = controller.prepare_search_task_restore(ui, &second).unwrap();
    controller.closing.store(true, Ordering::Release);
    controller.cancel_pending_task_restore_for_close(ui);
    assert!(!ui.get_search_workspace_loading());
    controller.finish_search_task_restore(
        ui,
        &second,
        epoch,
        resolve_task(&fixture.provider, task, cache),
    );
    assert!(controller.task_workspace.lock().unwrap().restore_failed);
    controller.finish_close(ui, Err("injected close failure".into()));
    wait_task(&fixture, &second);
    assert_eq!(ui.get_query(), "wind");
    controller.persist_search_task(ui);
    controller.wait_workspace_writes().unwrap();
    // A native title-bar close may interrupt the first workspace read before it
    // has supplied any active task. Resume must load that scope rather than look
    // for an absent task ID and leave the surviving window blocked.
    *controller.task_workspace.lock().unwrap() = Default::default();
    controller.task_workspace.lock().unwrap().loading = true;
    ui.set_search_workspace_loading(true);
    controller.closing.store(true, Ordering::Release);
    controller.cancel_pending_task_restore_for_close(ui);
    assert!(matches!(
        controller.task_workspace.lock().unwrap().resume_after_close,
        Some(WorkspaceResume::Load)
    ));
    controller.finish_close(ui, Err("injected startup close failure".into()));
    wait_task(&fixture, &second);
    assert_eq!(ui.get_query(), "wind");
    assert!(!controller.task_workspace.lock().unwrap().restore_failed);
    finish(&fixture);
}

#[test]
fn damaged_workspace_is_explicitly_read_only_and_cannot_accept_new_favorites() {
    isolated("lifecycle_review::child_protected_workspace");
}

#[test]
#[ignore = "isolated protected workspace read-only interaction helper"]
fn child_protected_workspace() {
    let fixture = fixture();
    let controller = &fixture.controller;
    let ui = &fixture.ui;
    controller.persist_search_task(ui);
    controller.wait_workspace_writes().unwrap();
    let target = controller
        .task_workspace
        .lock()
        .unwrap()
        .target
        .clone()
        .unwrap();
    let path = target
        .data_root
        .join("search-workspaces")
        .join(audiolib_db::local_play_history::safe_library_id(&target.library_scope).unwrap())
        .join("workspace.json");
    assert!(path.starts_with(&fixture.root));
    let damaged = b"{\"format_version\":999,\"library_scope\":\"future\"}";
    std::fs::write(&path, damaged).unwrap();
    controller.init_search_workspace();
    wait_for(ui, |ui| {
        !ui.get_search_workspace_loading() && ui.get_search_workspace_read_only()
    });
    let initial = active_id(ui);
    let title = ui.get_search_tasks().row_data(0).unwrap().name;
    ui.invoke_search_task_created();
    ui.invoke_search_task_closed(initial.clone().into());
    ui.invoke_search_task_renamed(initial.clone().into(), "should not persist".into());
    ui.invoke_search_task_favorite_toggled(initial.clone().into());
    assert_eq!(ui.get_search_tasks().row_count(), 1);
    assert_eq!(active_id(ui), initial);
    assert_eq!(ui.get_search_tasks().row_data(0).unwrap().name, title);
    assert_eq!(ui.get_saved_search_tasks().row_count(), 0);
    search(&fixture, "wind", 1);
    assert!(ui.get_search_workspace_read_only());
    assert!(!ui.get_search_workspace_error().is_empty());
    assert_eq!(std::fs::read(&path).unwrap(), damaged);
    finish(&fixture);
    assert_eq!(std::fs::read(&path).unwrap(), damaged);
}

fn add_sustained_sound(fixture: &Fixture) -> AudioFile {
    let path = fixture.root.join("sounds/gore/sustain.wav");
    let bytes = 48_000u32 * 2 * 4;
    let mut wav = Vec::new();
    wav.extend(b"RIFF");
    wav.extend((bytes + 36).to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16u32.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(48_000u32.to_le_bytes());
    wav.extend(96_000u32.to_le_bytes());
    wav.extend(2u16.to_le_bytes());
    wav.extend(16u16.to_le_bytes());
    wav.extend(b"data");
    wav.extend(bytes.to_le_bytes());
    wav.resize(44 + bytes as usize, 0);
    std::fs::write(&path, wav).unwrap();
    ScanService::new(fixture.provider.clone())
        .scan(
            &fixture.root.join("sounds"),
            &ScanOptions::default(),
            &ScanCancellation::default(),
            |_| {},
        )
        .unwrap();
    fixture.provider.find_audio_by_path(&path).unwrap().unwrap()
}

#[test]
fn returning_to_the_loaded_sound_preserves_pause_resume_generation_and_session() {
    isolated("lifecycle_review::child_loaded_playback");
}

#[test]
#[ignore = "isolated muted real-engine task pause/resume helper"]
fn child_loaded_playback() {
    let fixture = fixture();
    let controller = &fixture.controller;
    let ui = &fixture.ui;
    let sound = add_sustained_sound(&fixture);
    search(&fixture, "sustain", 1);
    let first = active_id(ui);
    controller.select_id(sound.id, true);
    let handle = fixture.engine.handle();
    let ready = handle.clone();
    wait_for(ui, move |ui| {
        ui.get_audio_ready() && ready.status().state == PlaybackState::Playing
    });
    handle
        .send_and_wait(AudioCommand::Pause, Duration::from_secs(4))
        .unwrap();
    let before = handle.status();
    assert!(before.play_session_id > 0);
    ui.invoke_search_task_created();
    wait_for(ui, |ui| {
        ui.get_search_tasks().row_count() == 2
            && !ui.get_search_workspace_loading()
            && !ui.get_task_restoring()
    });
    ui.invoke_search_task_selected(first.clone().into());
    wait_task(&fixture, &first);
    assert!(controller.audio_reload_target().is_none());
    ui.invoke_start_requested();
    let resumed = handle.clone();
    wait_for(ui, move |_| {
        resumed.status().state == PlaybackState::Playing
    });
    handle
        .send_and_wait(AudioCommand::Pause, Duration::from_secs(4))
        .unwrap();
    let after = handle.status();
    assert_eq!(after.generation, before.generation);
    assert_eq!(after.play_session_id, before.play_session_id);
    assert!(after.position >= before.position);
    // A current failed explicit Load marker is still authoritative even when
    // an older copy of the same record remains loaded in the engine.
    controller.mark_audio_reload(sound.id);
    controller.restore_task_audio_selection(&sound);
    assert_eq!(controller.audio_reload_target(), Some(sound.id));
    finish(&fixture);
}
