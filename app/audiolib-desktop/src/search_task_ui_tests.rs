// app/audiolib-desktop/src/search_task_ui_tests.rs
//! Search-task controls are exercised with an isolated Slint backend and owned models.
use crate::{AppWindow, AudioRow, FolderRow, SavedSearchTaskRow, SearchTaskRow};
use i_slint_backend_testing::{ElementHandle, TestingBackend, TestingBackendOptions};
use slint::{
    ComponentHandle, ModelRc, VecModel,
    platform::{Key, PointerEventButton, WindowEvent},
};
use std::{cell::RefCell, rc::Rc, time::Duration};

fn tasks(count: usize) -> ModelRc<SearchTaskRow> {
    ModelRc::new(VecModel::from(
        (0..count)
            .map(|index| SearchTaskRow {
                id: index.to_string().into(),
                name: format!("Task {index}").into(),
                active: index == 0,
                favorite: false,
                modified: index == 1,
                ..Default::default()
            })
            .collect::<Vec<_>>(),
    ))
}

fn window() -> AppWindow {
    let ui = AppWindow::new().unwrap();
    ui.window().set_size(slint::LogicalSize::new(1440.0, 900.0));
    ui.set_search_tasks(tasks(2));
    ui.show().unwrap();
    settle();
    ui
}

fn click(ui: &AppWindow, label: &str) {
    ElementHandle::find_by_accessible_label(ui, label)
        .next()
        .unwrap_or_else(|| panic!("Missing task control: {label}"))
        .mock_single_click(PointerEventButton::Left);
    settle();
}

fn settle() {
    // Declarative Timer.running changes start their native timer in the change
    // handler pass, after the mock backend's timer activation pass. Flush that
    // pass before advancing time so one-shot view restoration can actually fire.
    i_slint_backend_testing::mock_elapsed_time(Duration::ZERO);
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(10));
}

fn snapshot_if_requested(ui: &AppWindow, name: &str) {
    let Some(directory) = std::env::var_os("AUDIOLIB_MULTI_SEARCH_SNAPSHOT_DIR") else {
        return;
    };
    let directory = std::path::Path::new(&directory);
    std::fs::create_dir_all(directory).unwrap();
    let pixels = ui.window().take_snapshot().unwrap();
    image::save_buffer(
        directory.join(format!("{name}.png")),
        pixels.as_bytes(),
        pixels.width(),
        pixels.height(),
        image::ColorType::Rgba8,
    )
    .unwrap();
}

// Optional visual evidence uses real rendered pixels, cropped around one tag.
fn snapshot_tag_button_states_if_requested(ui: &AppWindow, modern: bool, body: &str, plus: &str) {
    let Some(directory) = std::env::var_os("AUDIOLIB_MULTI_SEARCH_SNAPSHOT_DIR") else {
        return;
    };
    for scale in [1.0, 1.25, 2.0] {
        ui.window()
            .set_size(slint::LogicalSize::new(1600.0 * scale, 900.0 * scale));
        ui.global::<crate::Theme>().set_ui_scale(scale);
        click(ui, body);
        let tag = ElementHandle::find_by_accessible_label(ui, body)
            .next()
            .unwrap();
        let button = ElementHandle::find_by_accessible_label(ui, plus)
            .next()
            .unwrap();
        let center = slint::LogicalPosition::new(
            button.absolute_position().x + button.size().width / 2.0,
            button.absolute_position().y + button.size().height / 2.0,
        );
        for state in ["default", "hover", "pressed", "disabled"] {
            ui.set_ai_busy(state == "disabled");
            ui.window().dispatch_event(WindowEvent::PointerMoved {
                position: if state == "default" {
                    slint::LogicalPosition::new(0.0, 0.0)
                } else {
                    center
                },
            });
            if state == "pressed" {
                ui.window().dispatch_event(WindowEvent::PointerPressed {
                    position: center,
                    button: PointerEventButton::Left,
                });
            }
            settle();
            i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(120));
            let pixels = ui.window().take_snapshot().unwrap();
            let image = image::RgbaImage::from_raw(
                pixels.width(),
                pixels.height(),
                pixels.as_bytes().to_vec(),
            )
            .unwrap();
            let factor = ui.window().scale_factor();
            let x = ((tag.absolute_position().x - 6.0 * scale) * factor)
                .max(0.0)
                .floor() as u32;
            let y = ((tag.absolute_position().y - 6.0 * scale) * factor)
                .max(0.0)
                .floor() as u32;
            let right = ((button.absolute_position().x + button.size().width + 6.0 * scale)
                * factor)
                .ceil()
                .min(pixels.width() as f32) as u32;
            let bottom = ((tag.absolute_position().y + tag.size().height + 6.0 * scale) * factor)
                .ceil()
                .min(pixels.height() as f32) as u32;
            image::imageops::crop_imm(&image, x, y, right - x, bottom - y)
                .to_image()
                .save(
                    std::path::Path::new(&directory)
                        .join(format!("tag-button-{modern}-{scale}-{state}.png")),
                )
                .unwrap();
            if state == "pressed" {
                ui.window().dispatch_event(WindowEvent::PointerReleased {
                    position: center,
                    button: PointerEventButton::Left,
                });
            }
        }
        ui.set_ai_busy(false);
    }
}

#[test]
fn ai_panel_actions_and_provider_view_are_renderable() {
    slint::platform::set_platform(Box::new(TestingBackend::new(TestingBackendOptions {
        mock_time: true,
        threading: false,
        renderer_name: Some("software".into()),
    })))
    .unwrap();
    let ui = window();
    ui.window()
        .set_size(slint::LogicalSize::new(1920.0, 1080.0));
    ui.set_chinese(true);
    ui.set_data_source_label("本地音效库 · 验收示例".into());
    ui.set_ai_visible(true);
    ui.set_ai_prompt("我要设计科幻爆炸声".into());
    ui.set_ai_providers(ModelRc::new(VecModel::from(vec![
        "本地词表（离线）".into(),
    ])));
    ui.set_ai_rows(ModelRc::new(VecModel::from(vec![
        crate::AiSuggestion {
            text: "scifi explosion".into(),
            detail: "direct · 科幻爆炸".into(),
        },
        crate::AiSuggestion {
            text: "sub boom".into(),
            detail: "低频层".into(),
        },
    ])));
    ui.set_ai_query("explosion".into());
    ui.set_ai_selected(0);
    ui.set_ai_quick_rows(ModelRc::new(VecModel::from(
        [
            "explosion",
            "scifi explosion",
            "sub boom",
            "metal impact",
            "plasma hum",
        ]
        .into_iter()
        .map(|text| crate::AiSuggestion {
            text: text.into(),
            detail: "推荐素材 / suggested source".into(),
        })
        .collect::<Vec<_>>(),
    )));
    ui.set_ai_rows(ui.get_ai_quick_rows());
    ui.set_ai_detail("成品 / 科幻爆炸 · UCS: EXPLDsgn".into());
    let actions = Rc::new(RefCell::new(Vec::new()));
    let recorded = actions.clone();
    let weak = ui.as_weak();
    ui.on_ai_action(move |kind, index| {
        recorded.borrow_mut().push((kind, index));
        if kind == 23 {
            weak.upgrade().unwrap().set_ai_collapsed(index != 0);
        }
    });
    settle();
    snapshot_if_requested(&ui, "ai-search-panel");
    click(&ui, "生成查询建议");
    assert!(actions.borrow().contains(&(2, 0)));
    assert!(
        ElementHandle::find_by_accessible_label(&ui, "AI 生成进度")
            .next()
            .is_none()
    );
    ui.set_ai_busy(true);
    ui.set_ai_generating(true);
    settle();
    assert!(
        ElementHandle::find_by_accessible_label(&ui, "AI 生成进度")
            .next()
            .is_some()
    );
    let count = actions.borrow().len();
    click(&ui, "生成 AI 查询建议");
    click(&ui, "生成查询建议");
    assert_eq!(
        actions.borrow().len(),
        count,
        "Generation cannot be submitted twice"
    );
    snapshot_if_requested(&ui, "ai-generating");
    for modern in [false, true] {
        for scale in [1.0, 2.0] {
            ui.window().set_size(slint::LogicalSize::new(1080.0, 700.0));
            ui.global::<crate::Theme>().set_modern_style(modern);
            ui.global::<crate::Theme>().set_ui_scale(scale);
            settle();
            let field = ElementHandle::find_by_accessible_label(&ui, "AI 声音描述")
                .next()
                .unwrap();
            let submit = ElementHandle::find_by_accessible_label(&ui, "生成 AI 查询建议")
                .next()
                .unwrap();
            let tag = ElementHandle::find_by_accessible_label(&ui, "快捷搜索：explosion")
                .next()
                .unwrap();
            assert!(
                field.absolute_position().x + field.size().width <= submit.absolute_position().x
            );
            assert!(
                submit.absolute_position().x + submit.size().width <= tag.absolute_position().x,
                "Generating caption must not overlap the recommendations"
            );
            snapshot_if_requested(&ui, &format!("ai-generating-short-{modern}-{scale}"));
        }
    }
    ui.global::<crate::Theme>().set_modern_style(false);
    ui.global::<crate::Theme>().set_ui_scale(1.0);
    ui.window()
        .set_size(slint::LogicalSize::new(1920.0, 1080.0));
    settle();
    click(&ui, "取消");
    assert_eq!(actions.borrow().last(), Some(&(3, 0)));
    click(&ui, "退出 AI 搜索");
    assert_eq!(actions.borrow().last(), Some(&(1, 0)));
    ui.set_ai_generating(false);
    ui.set_ai_busy(false);
    settle();
    assert!(
        ElementHandle::find_by_accessible_label(&ui, "AI 生成进度")
            .next()
            .is_none()
    );
    for (index, text) in [
        "explosion",
        "scifi explosion",
        "sub boom",
        "metal impact",
        "plasma hum",
    ]
    .into_iter()
    .enumerate()
    {
        click(&ui, &format!("快捷搜索：{text}"));
        assert!(actions.borrow().contains(&(16, index as i32)));
    }
    assert!(!ui.get_ai_auto_search());
    click(&ui, "生成后自动搜索 3 个任务");
    assert!(ui.get_ai_auto_search());
    click(&ui, "查询全部推荐");
    assert!(actions.borrow().contains(&(20, 0)));
    ui.set_ai_batch_searching(true);
    ui.set_ai_batch_total(5);
    ui.set_ai_batch_completed(2);
    settle();
    let task_selections = Rc::new(RefCell::new(Vec::new()));
    let selected_tasks = task_selections.clone();
    ui.on_search_task_selected(move |id| selected_tasks.borrow_mut().push(id.to_string()));
    ui.set_search_workspace_loading(true);
    ui.set_task_restoring(true);
    settle();
    assert!(ui.get_background_blocked());
    assert!(!ui.get_ai_batch_blocked());
    let task = ElementHandle::find_by_accessible_label(&ui, "搜索任务：Task 1")
        .next()
        .unwrap();
    assert_eq!(task.accessible_enabled(), Some(true));
    click(&ui, "搜索任务：Task 1");
    assert_eq!(task_selections.borrow().as_slice(), &["1"]);
    assert_eq!(
        ElementHandle::find_by_accessible_label(&ui, "关闭搜索任务：Task 1")
            .next()
            .unwrap()
            .accessible_enabled(),
        Some(false)
    );
    assert!(
        ElementHandle::find_by_accessible_label(&ui, "取消")
            .next()
            .is_some()
    );
    click(&ui, "收起 AI 侧栏");
    assert!(ui.get_ai_collapsed());
    click(&ui, "展开 AI 侧栏");
    assert!(!ui.get_ai_collapsed());
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Escape.into(),
    });
    ui.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Escape.into(),
    });
    settle();
    assert_eq!(actions.borrow().last(), Some(&(1, 0)));
    ui.set_about_visible(true);
    settle();
    assert!(ui.get_ai_batch_blocked());
    assert_eq!(task.accessible_enabled(), Some(false));
    ui.set_about_visible(false);
    ui.set_search_workspace_loading(false);
    ui.set_task_restoring(false);
    settle();
    let count = actions.borrow().len();
    click(&ui, "查询全部推荐");
    assert_eq!(actions.borrow().len(), count);
    click(&ui, "取消");
    assert_eq!(actions.borrow().last(), Some(&(3, 0)));
    ui.set_ai_batch_searching(false);
    for modern in [false, true] {
        ui.global::<crate::Theme>().set_modern_style(modern);
        settle();
        let batch = ElementHandle::find_by_accessible_label(&ui, "查询全部推荐")
            .next()
            .unwrap();
        let tag = ElementHandle::find_by_accessible_label(&ui, "快捷搜索：scifi explosion")
            .next()
            .unwrap();
        let field = ElementHandle::find_by_accessible_label(&ui, "AI 声音描述")
            .next()
            .unwrap();
        let query = ElementHandle::find_by_accessible_label(&ui, "AI query")
            .next()
            .unwrap();
        assert!(tag.absolute_position().x >= field.absolute_position().x + field.size().width);
        assert!(
            (tag.absolute_position().y + tag.size().height / 2.0
                - field.absolute_position().y
                - field.size().height / 2.0)
                .abs()
                < 12.0,
            "Recommendations belong in the search toolbar"
        );
        assert!(batch.absolute_position().x >= query.absolute_position().x + query.size().width);
        assert!(
            (batch.absolute_position().y + batch.size().height / 2.0
                - query.absolute_position().y
                - query.size().height / 2.0)
                .abs()
                < 4.0
        );
        click(&ui, "收起 AI 侧栏");
        assert!(ui.get_ai_visible());
        assert!(ui.get_ai_collapsed());
        let reopen = ElementHandle::find_by_accessible_label(&ui, "展开 AI 侧栏")
            .next()
            .unwrap();
        let tabs = ElementHandle::find_by_accessible_label(&ui, "搜索任务")
            .next()
            .unwrap();
        assert!(
            reopen.size().width > reopen.size().height,
            "Restore is a compact horizontal control"
        );
        assert!(reopen.absolute_position().y >= field.absolute_position().y + field.size().height);
        assert!(
            reopen.absolute_position().y <= tabs.absolute_position().y + tabs.size().height,
            "Restore belongs in the top task band"
        );
        assert!(
            reopen.absolute_position().x + reopen.size().width
                <= ui
                    .window()
                    .size()
                    .to_logical(ui.window().scale_factor())
                    .width
        );
        for label in ["管理音效库", "声音详情页"] {
            let other = ElementHandle::find_by_accessible_label(&ui, label)
                .next()
                .unwrap();
            let a = reopen.absolute_position();
            let b = other.absolute_position();
            assert!(
                a.x + reopen.size().width <= b.x
                    || b.x + other.size().width <= a.x
                    || a.y + reopen.size().height <= b.y
                    || b.y + other.size().height <= a.y,
                "AI restore must not cover {label}"
            );
        }
        snapshot_if_requested(
            &ui,
            if modern {
                "ai-keyword-tags-studio"
            } else {
                "ai-keyword-tags-classic"
            },
        );
        ui.set_inspector_visible(false);
        settle();
        assert!(
            ElementHandle::find_by_accessible_label(&ui, "展开 AI 侧栏")
                .next()
                .is_some()
        );
        ui.set_inspector_visible(true);
        settle();
        click(&ui, "快捷搜索：metal impact");
        click(&ui, "展开 AI 侧栏");
        assert!(!ui.get_ai_collapsed());
        snapshot_if_requested(
            &ui,
            if modern {
                "ai-sidebar-studio"
            } else {
                "ai-sidebar-classic"
            },
        );
    }
    // The drawer stays in the native window even when the workbench scrolls
    // horizontally. Exercise enough tabs to place overflow/add controls nearby.
    ui.set_search_tasks(tasks(12));
    ui.window().set_size(slint::LogicalSize::new(1080.0, 700.0));
    ui.global::<crate::Theme>().set_ui_scale(2.0);
    for modern in [false, true] {
        ui.global::<crate::Theme>().set_modern_style(modern);
        click(&ui, "收起 AI 侧栏");
        let reopen = ElementHandle::find_by_accessible_label(&ui, "展开 AI 侧栏")
            .next()
            .unwrap();
        let tabs = ElementHandle::find_by_accessible_label(&ui, "搜索任务")
            .next()
            .unwrap();
        if !modern {
            assert!(
                tabs.absolute_position().x + tabs.size().width
                    <= ui
                        .window()
                        .size()
                        .to_logical(ui.window().scale_factor())
                        .width,
                "Classic tab reservation must fit the native window"
            );
        } else {
            assert!(
                reopen.absolute_position().y + reopen.size().height <= tabs.absolute_position().y,
                "Studio restore must stay above the task controls"
            );
        }
        snapshot_if_requested(&ui, &format!("ai-drawer-tabs-short-{modern}"));
        for label in ["新建搜索任务", "向右翻页搜索任务"] {
            let Some(other) = ElementHandle::find_by_accessible_label(&ui, label).next() else {
                // Studio's horizontally scrollable workbench can put the task
                // controls off-screen; its separate vertical lane still applies.
                assert!(modern, "Missing Classic task control: {label}");
                continue;
            };
            let a = reopen.absolute_position();
            let b = other.absolute_position();
            assert!(
                a.x + reopen.size().width <= b.x
                    || b.x + other.size().width <= a.x
                    || a.y + reopen.size().height <= b.y
                    || b.y + other.size().height <= a.y,
                "AI restore must not cover {label} at large UI scale"
            );
        }
        ui.invoke_scroll_main_to_end();
        settle();
        let band = tabs.absolute_position();
        let restore = ElementHandle::find_by_accessible_label(&ui, "展开 AI 侧栏").next();
        if band.y + tabs.size().height <= 0.0 {
            assert!(
                restore.is_none(),
                "A scrolled-away AI band must not leave an overlay over the content"
            );
        }
        if let Some(restore) = restore {
            if let Some(details) = ElementHandle::find_by_accessible_label(&ui, "声音详情页").next()
            {
                assert!(
                    restore.absolute_position().y + restore.size().height
                        <= details.absolute_position().y,
                    "Scrolling must preserve clearance above Metadata controls"
                );
            }
            click(&ui, "展开 AI 侧栏");
        } else {
            // The top-band control returns when the user scrolls back to it;
            // use the model here to continue fixed-panel compact acceptance.
            ui.set_ai_collapsed(false);
        }
        assert!(!ui.get_ai_collapsed());
    }
    ui.set_search_tasks(tasks(2));
    ui.global::<crate::Theme>().set_ui_scale(1.0);
    ui.window()
        .set_size(slint::LogicalSize::new(1920.0, 1080.0));
    ui.global::<crate::Theme>().set_modern_style(false);
    ui.invoke_scroll_main_to_end();
    settle();
    click(&ui, "编辑提示词");
    snapshot_if_requested(&ui, "ai-prompt-preset");
    // The editable prompt may outgrow a short or scaled sidebar. Its body can
    // scroll, while both search actions must remain reachable without scrolling.
    for scale in [1.0, 1.5, 2.0] {
        for chinese in [false, true] {
            ui.window().set_size(slint::LogicalSize::new(1080.0, 700.0));
            ui.global::<crate::Theme>().set_ui_scale(scale);
            ui.set_chinese(chinese);
            ui.set_ai_detail(
                "科幻爆炸素材来源和分层说明 / layered sound source details. "
                    .repeat(20)
                    .into(),
            );
            ui.set_ai_status(
                "保留目录与筛选条件 / Preserves the current directory and filters. "
                    .repeat(20)
                    .into(),
            );
            settle();
            let count = actions.borrow().len();
            click(
                &ui,
                if chinese {
                    "在当前任务搜索"
                } else {
                    "Search current task"
                },
            );
            click(
                &ui,
                if chinese {
                    "以此关键词新建搜索任务"
                } else {
                    "New search task with this keyword"
                },
            );
            click(
                &ui,
                if chinese {
                    "查询全部推荐"
                } else {
                    "Search all recommendations"
                },
            );
            click(
                &ui,
                if chinese {
                    "退出 AI 搜索"
                } else {
                    "Exit AI"
                },
            );
            assert_eq!(
                &actions.borrow()[count..],
                &[(6, 0), (7, 0), (20, 0), (1, 0)],
                "Expanded prompt must leave all search and exit actions clickable at scale {scale}"
            );
            snapshot_if_requested(&ui, &format!("ai-prompt-short-{scale}-{chinese}"));
        }
    }
    ui.global::<crate::Theme>().set_ui_scale(1.0);
    ui.window().set_size(slint::LogicalSize::new(1440.0, 900.0));
    ui.set_chinese(true);
    ui.set_ai_status("".into());
    ui.set_ai_detail("成品 / 科幻爆炸 · UCS: EXPLDsgn".into());
    settle();
    click(&ui, "保存提示词");
    assert!(actions.borrow().contains(&(15, 0)));
    click(&ui, "收起");
    click(&ui, "生成后自动搜索 3 个任务");
    assert!(!ui.get_ai_auto_search());
    click(&ui, "AI 搜索设置");
    assert!(ui.get_ai_settings());
    ui.set_ai_protocol(6);
    ui.set_ai_endpoint("https://api.deepseek.com".into());
    ui.set_ai_model("".into());
    ui.set_ai_profile_name("DeepSeek".into());
    assert!(ui.get_ai_profile_id().is_empty());
    click(&ui, "获取模型列表");
    assert!(actions.borrow().contains(&(11, 0)));
    ui.set_ai_model("deepseek-flash".into());
    click(&ui, "测试模型调用");
    assert!(actions.borrow().contains(&(17, 0)));
    let count = actions.borrow().len();
    ui.set_ai_busy(true);
    assert!(
        !ui.get_ai_generating(),
        "Provider operations do not generate recommendations"
    );
    click(&ui, "获取模型列表");
    assert_eq!(actions.borrow().len(), count);
    ui.set_ai_busy(false);
    ui.set_ai_profile_id("saved-deepseek".into());
    ui.set_ai_saved_key(true);
    ui.set_ai_saved_endpoint("https://api.deepseek.com".into());
    ui.set_ai_saved_protocol(6);
    settle();
    snapshot_if_requested(&ui, "ai-provider-panel");
    click(&ui, "Skill 与预设");
    assert_eq!(ui.get_ai_settings_page(), 1);
    assert!(
        ElementHandle::find_by_accessible_label(&ui, "AI Skill")
            .next()
            .is_some()
    );
    snapshot_if_requested(&ui, "ai-skill-preset-panel");
    click(&ui, "保存提示词");
    assert_eq!(actions.borrow().last(), Some(&(15, 0)));
    click(&ui, "供应商");
    assert_eq!(ui.get_ai_settings_page(), 0);
    assert_eq!(ui.get_ai_model(), "deepseek-flash");
    click(&ui, "退出 AI 搜索");
    assert_eq!(actions.borrow().last(), Some(&(1, 0)));
    click(&ui, "返回 AI 搜索");
    assert!(
        !ui.get_ai_auto_search(),
        "Provider navigation must preserve the auto-search choice"
    );
    click(&ui, "AI 搜索设置");
    // An existing modal takes Escape priority over the temporarily hidden AI panel.
    ui.set_about_visible(true);
    settle();
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Escape.into(),
    });
    ui.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Escape.into(),
    });
    settle();
    assert!(!ui.get_about_visible());
    assert!(ui.get_ai_visible());
    ui.hide().unwrap();
}

fn open_saved_menu(ui: &AppWindow) {
    let control = ElementHandle::find_by_accessible_label(ui, "Manage saved search: Forest")
        .next()
        .unwrap();
    control.mock_single_click(PointerEventButton::Left);
    settle();
}

#[test]
fn ai_provider_form_survives_page_recreation_and_panel_reopening() {
    i_slint_backend_testing::init_no_event_loop();
    let ui = window();
    ui.set_ai_visible(true);
    ui.set_ai_providers(ModelRc::new(VecModel::from(vec![
        "Local reference".into(),
        "Work".into(),
    ])));
    ui.set_ai_provider_index(1);
    ui.set_ai_profile_id("saved-profile".into());
    ui.set_ai_profile_name("Work".into());
    ui.set_ai_protocol(5);
    ui.set_ai_endpoint("https://example.test/go".into());
    ui.set_ai_model("saved-model".into());
    ui.set_ai_key("unsaved-draft-key".into());
    ui.set_ai_timeout(31);
    ui.set_ai_request_cap(123);
    ui.set_ai_subscription(true);
    ui.set_ai_preset_index(1);
    ui.set_ai_instruction("Preserved sound design instruction".into());
    let actions = Rc::new(RefCell::new(Vec::new()));
    let observed = actions.clone();
    ui.on_ai_action(move |kind, index| observed.borrow_mut().push((kind, index)));
    for _ in 0..3 {
        click(&ui, "AI search settings");
        assert_eq!(
            ui.get_ai_protocol(),
            5,
            "Recreating the protocol selector must not write its default index into the draft"
        );
        assert_eq!(ui.get_ai_provider_index(), 1);
        assert_eq!(ui.get_ai_profile_name(), "Work");
        assert_eq!(ui.get_ai_endpoint(), "https://example.test/go");
        assert_eq!(ui.get_ai_model(), "saved-model");
        assert_eq!(ui.get_ai_key(), "unsaved-draft-key");
        assert_eq!(ui.get_ai_timeout(), 31);
        assert_eq!(ui.get_ai_request_cap(), 123);
        assert!(ui.get_ai_subscription());
        click(&ui, "Skill and presets");
        assert_eq!(ui.get_ai_settings_page(), 1);
        assert_eq!(ui.get_ai_preset_index(), 1);
        assert_eq!(
            ui.get_ai_instruction(),
            "Preserved sound design instruction"
        );
        click(&ui, "Providers");
        assert_eq!(ui.get_ai_settings_page(), 0);
        assert_eq!(ui.get_ai_key(), "unsaved-draft-key");
        click(&ui, "Return to AI search");
        ui.set_ai_visible(false);
        settle();
        ui.set_ai_visible(true);
        settle();
    }
    assert!(
        actions.borrow().is_empty(),
        "Recreating a view must not dispatch user selections"
    );
    ui.hide().unwrap();
}

#[test]
fn ai_tag_edge_creates_a_task_without_triggering_the_tag_body() {
    slint::platform::set_platform(Box::new(TestingBackend::new(TestingBackendOptions {
        mock_time: true,
        threading: false,
        renderer_name: Some("software".into()),
    })))
    .unwrap();
    for modern in [false, true] {
        for chinese in [false, true] {
            let ui = window();
            ui.global::<crate::Theme>().set_modern_style(modern);
            ui.global::<crate::Theme>().set_ui_scale(1.0);
            ui.set_chinese(chinese);
            ui.set_ai_visible(true);
            ui.set_ai_collapsed(true);
            ui.set_ai_quick_rows(ModelRc::new(VecModel::from(vec![
                crate::AiSuggestion {
                    text: "wind".into(),
                    detail: "Air movement".into(),
                },
                crate::AiSuggestion {
                    text: "metal impact".into(),
                    detail: "Hard impact".into(),
                },
            ])));
            let actions = Rc::new(RefCell::new(Vec::new()));
            let observed = actions.clone();
            ui.on_ai_action(move |kind, index| observed.borrow_mut().push((kind, index)));
            let new_task = if chinese {
                "以推荐词新建搜索任务：metal impact"
            } else {
                "New search task: metal impact"
            };
            let body = if chinese {
                "快捷搜索：metal impact"
            } else {
                "Quick search: metal impact"
            };
            click(&ui, new_task);
            assert_eq!(actions.borrow().as_slice(), &[(24, 1)]);
            click(&ui, body);
            assert_eq!(actions.borrow().as_slice(), &[(24, 1), (16, 1)]);
            for (busy, restoring, loading) in [
                (true, false, false),
                (false, true, false),
                (false, false, true),
            ] {
                ui.set_ai_busy(busy);
                ui.set_task_restoring(restoring);
                ui.set_search_workspace_loading(loading);
                settle();
                for label in [new_task, body] {
                    let control = ElementHandle::find_by_accessible_label(&ui, label)
                        .next()
                        .unwrap();
                    assert_eq!(control.accessible_enabled(), Some(false));
                    control.mock_single_click(PointerEventButton::Left);
                    settle();
                }
                assert_eq!(actions.borrow().as_slice(), &[(24, 1), (16, 1)]);
            }
            ui.set_ai_busy(false);
            ui.set_task_restoring(false);
            ui.set_search_workspace_loading(false);
            click(&ui, new_task);
            assert_eq!(actions.borrow().as_slice(), &[(24, 1), (16, 1), (24, 1)]);
            snapshot_if_requested(&ui, &format!("ai-tag-new-task-{modern}-{chinese}"));
            if chinese {
                snapshot_tag_button_states_if_requested(&ui, modern, body, new_task);
            }
            ui.hide().unwrap();
        }
    }
}

#[test]
fn ai_switch_routes_the_shared_search_field_and_retains_both_drafts() {
    slint::platform::set_platform(Box::new(TestingBackend::new(TestingBackendOptions {
        mock_time: true,
        threading: false,
        renderer_name: Some("software".into()),
    })))
    .unwrap();
    for modern in [false, true] {
        let ui = window();
        ui.global::<crate::Theme>().set_modern_style(modern);
        ui.set_chinese(false);
        ui.set_query("wind".into());
        ui.set_ai_prompt("science fiction explosion".into());
        let ai = Rc::new(RefCell::new(Vec::new()));
        let seen = ai.clone();
        let weak = ui.as_weak();
        ui.on_ai_action(move |kind, _| {
            seen.borrow_mut().push(kind);
            let ui = weak.upgrade().unwrap();
            if kind == 0 {
                ui.set_ai_visible(true);
            } else if kind == 1 {
                ui.set_ai_visible(false);
            }
        });
        let searches = Rc::new(RefCell::new(Vec::new()));
        let seen = searches.clone();
        ui.on_search_requested(move |immediate| seen.borrow_mut().push(immediate));
        settle();
        assert_eq!(ui.get_search_entry_text(), "wind");
        click(&ui, "AI search mode");
        assert!(ui.get_ai_visible());
        assert_eq!(ui.get_search_entry_text(), "science fiction explosion");
        assert_eq!(
            ElementHandle::find_by_accessible_label(&ui, "AI search mode")
                .next()
                .unwrap()
                .accessible_checked(),
            Some(true)
        );
        ui.set_search_entry_text("layered alien explosion".into());
        ui.invoke_edit_search_entry();
        click(&ui, "Generate AI query suggestions");
        settle();
        assert_eq!(ui.get_ai_prompt(), "layered alien explosion");
        assert_eq!(ui.get_query(), "wind");
        assert!(searches.borrow().is_empty());
        assert!(ai.borrow().contains(&2));
        let count = ai.borrow().len();
        ui.set_ai_busy(true);
        click(&ui, "Generate AI query suggestions");
        assert_eq!(ai.borrow().len(), count);
        ui.set_ai_busy(false);
        snapshot_if_requested(
            &ui,
            if modern {
                "ai-integrated-studio"
            } else {
                "ai-integrated-classic"
            },
        );
        click(&ui, "AI search mode");
        assert!(!ui.get_ai_visible());
        assert_eq!(ui.get_search_entry_text(), "wind");
        ui.set_search_entry_text("metal".into());
        ui.invoke_edit_search_entry();
        click(&ui, "Run search");
        settle();
        assert_eq!(ui.get_query(), "metal");
        assert_eq!(ui.get_ai_prompt(), "layered alien explosion");
        assert_eq!(*searches.borrow(), vec![false, true]);
        for _ in 0..5 {
            click(&ui, "AI search mode");
            assert_eq!(ui.get_search_entry_text(), "layered alien explosion");
            click(&ui, "AI search mode");
            assert_eq!(ui.get_search_entry_text(), "metal");
            click(&ui, "Run search");
        }
        assert_eq!(searches.borrow().len(), 7);
        ui.hide().unwrap();
    }
}

fn activate_popup(ui: &AppWindow, label: &str, steps_from_end: usize) {
    ElementHandle::find_by_accessible_label(ui, label)
        .next()
        .unwrap_or_else(|| panic!("Missing saved-search menu item: {label}"));
    // Context popups use their own coordinate origin. Keyboard navigation drives
    // the actual menu without assuming MenuBar popup positioning conventions.
    for key in std::iter::repeat_n(Key::UpArrow, steps_from_end).chain([Key::Return]) {
        ui.window()
            .dispatch_event(WindowEvent::KeyPressed { text: key.into() });
        ui.window()
            .dispatch_event(WindowEvent::KeyReleased { text: key.into() });
    }
    settle();
}

#[test]
fn search_history_buttons_navigate_only_when_available_in_both_search_bars() {
    slint::platform::set_platform(Box::new(TestingBackend::new(TestingBackendOptions {
        mock_time: true,
        threading: false,
        renderer_name: Some("software".into()),
    })))
    .unwrap();
    for modern in [false, true] {
        let ui = window();
        ui.global::<crate::Theme>().set_modern_style(modern);
        ui.global::<crate::Theme>().set_modern_player(modern);
        ui.set_query("boom".into());
        settle();
        let events = Rc::new(RefCell::new(Vec::new()));
        let observed = events.clone();
        ui.on_search_history_requested(move |direction| observed.borrow_mut().push(direction));
        let assert_enabled = |label: &str, enabled| {
            assert_eq!(
                ElementHandle::find_by_accessible_label(&ui, label)
                    .next()
                    .unwrap_or_else(|| panic!("Missing search history control: {label}"))
                    .accessible_enabled(),
                Some(enabled),
                "Unexpected availability for {label}, modern={modern}"
            );
        };

        assert_enabled("Back in search history", false);
        assert_enabled("Forward in search history", false);
        click(&ui, "Back in search history");
        click(&ui, "Forward in search history");
        assert!(events.borrow().is_empty());

        ui.set_search_history_can_back(true);
        settle();
        assert_enabled("Back in search history", true);
        assert_enabled("Forward in search history", false);
        click(&ui, "Back in search history");
        click(&ui, "Forward in search history");
        assert_eq!(&*events.borrow(), &[-1]);

        ui.set_search_history_can_forward(true);
        ui.set_chinese(true);
        settle();
        assert_enabled("后退搜索历史", true);
        assert_enabled("前进搜索历史", true);
        click(&ui, "前进搜索历史");
        assert_eq!(&*events.borrow(), &[-1, 1]);
        snapshot_if_requested(
            &ui,
            if modern {
                "search-history-studio-cn"
            } else {
                "search-history-classic-cn"
            },
        );

        // A pending result may be superseded by history navigation. Workspace
        // restoration and application overlays must still block those actions.
        ui.set_searching(true);
        settle();
        assert_enabled("后退搜索历史", true);
        click(&ui, "后退搜索历史");
        assert_eq!(&*events.borrow(), &[-1, 1, -1]);
        events.borrow_mut().clear();
        ui.set_startup_busy(true);
        settle();
        assert_enabled("后退搜索历史", false);
        assert_enabled("前进搜索历史", false);
        click(&ui, "后退搜索历史");
        click(&ui, "前进搜索历史");
        ui.set_startup_busy(false);
        ui.set_search_workspace_loading(true);
        settle();
        assert_enabled("后退搜索历史", false);
        assert_enabled("前进搜索历史", false);
        click(&ui, "后退搜索历史");
        click(&ui, "前进搜索历史");
        ui.set_search_workspace_loading(false);
        ui.set_task_restoring(true);
        settle();
        assert_enabled("后退搜索历史", false);
        assert_enabled("前进搜索历史", false);
        click(&ui, "后退搜索历史");
        click(&ui, "前进搜索历史");
        assert!(events.borrow().is_empty());
        ui.set_task_restoring(false);
        settle();
        assert_enabled("后退搜索历史", true);
        assert_enabled("前进搜索历史", true);
        click(&ui, "前进搜索历史");
        assert_eq!(&*events.borrow(), &[1]);
        ui.hide().unwrap();
    }
}

#[test]
fn active_task_toolbar_favorite_and_background_close_do_not_select_a_task() {
    i_slint_backend_testing::init_no_event_loop();
    let ui = window();
    let events = Rc::new(RefCell::new(Vec::new()));
    let observed = events.clone();
    ui.on_search_task_selected(move |id| observed.borrow_mut().push(format!("select:{id}")));
    let observed = events.clone();
    ui.on_search_task_favorite_toggled(move |id| {
        observed.borrow_mut().push(format!("favorite:{id}"));
    });
    let observed = events.clone();
    ui.on_search_task_closed(move |id| observed.borrow_mut().push(format!("close:{id}")));
    click(&ui, "Favorite search task: Task 0");
    click(&ui, "Close search task: Task 1");
    assert_eq!(&*events.borrow(), &["favorite:0", "close:1"]);
    click(&ui, "Search task: Task 1");
    assert_eq!(&*events.borrow(), &["favorite:0", "close:1", "select:1"]);
    ui.set_active_search_task_index(1);
    settle();
    let toolbar_favorite = ElementHandle::find_by_accessible_label(
        &ui,
        "Favorite search task: Task 1 (changed; update from saved search menu)",
    )
    .next()
    .unwrap();
    toolbar_favorite.mock_single_click(PointerEventButton::Left);
    settle();
    assert_eq!(
        &*events.borrow(),
        &["favorite:0", "close:1", "select:1", "favorite:1"]
    );
    ui.hide().unwrap();
}

#[test]
fn new_task_control_follows_tabs_and_docks_when_resizing_both_themes() {
    slint::platform::set_platform(Box::new(TestingBackend::new(TestingBackendOptions {
        mock_time: true,
        threading: false,
        renderer_name: Some("software".into()),
    })))
    .unwrap();
    for theme in audiolib_core::config::THEME_IDS {
        for scale in [1.0, 2.0] {
            let ui = AppWindow::new().unwrap();
            crate::theme::apply_theme(&ui, theme);
            ui.global::<crate::Theme>().set_ui_scale(scale);
            ui.window()
                .set_size(slint::LogicalSize::new(2400.0 * scale, 900.0 * scale));
            ui.set_search_tasks(tasks(1));
            ui.on_search_task_selected(|_| panic!("Resizing and paging must not select a task"));
            let created = Rc::new(RefCell::new(0));
            let observed = created.clone();
            ui.on_search_task_created(move || *observed.borrow_mut() += 1);
            ui.show().unwrap();
            settle();

            let control = |label: &str| {
                ElementHandle::find_by_accessible_label(&ui, label)
                    .next()
                    .unwrap_or_else(|| {
                        panic!("Missing control: {label}, theme={theme}, scale={scale}")
                    })
            };
            let follows_last_tab = |count| {
                let last = control(&format!("Search task: Task {}", count - 1));
                let plus = control("New search task");
                let gap =
                    plus.absolute_position().x - (last.absolute_position().x + last.size().width);
                assert!(
                    (-1.0..plus.size().width / 4.0).contains(&gap),
                    "New must follow the last tab: gap={gap}, theme={theme}, scale={scale}"
                );
                assert!(
                    ElementHandle::find_by_accessible_label(&ui, "Scroll search tasks right")
                        .next()
                        .is_none()
                );
                plus.absolute_position().x
            };

            let first_x = follows_last_tab(1);
            ui.set_search_tasks(tasks(3));
            settle();
            assert!(follows_last_tab(3) > first_x);
            ui.set_search_tasks(tasks(1));
            settle();
            assert!((follows_last_tab(1) - first_x).abs() < 1.0);

            // Tabs first share the available width instead of immediately scrolling.
            ui.set_search_tasks(tasks(4));
            settle();
            follows_last_tab(4);
            let wide_tab_width = control("Search task: Task 0").size().width;
            let bar = control("Search tasks");
            // Studio's strip is inside the result pane. Include that theme's
            // surrounding panes when choosing a width that can fit four minimum tabs.
            let outside_strip = ui
                .window()
                .size()
                .to_logical(ui.window().scale_factor())
                .width
                - bar.size().width;
            let adaptive_window_width = (outside_strip + 712.0 * scale).max(1080.0 * scale);
            if scale == 1.0 {
                snapshot_if_requested(&ui, &format!("task-add-following-{theme}"));
            }
            ui.window().set_size(slint::LogicalSize::new(
                adaptive_window_width,
                900.0 * scale,
            ));
            settle();
            follows_last_tab(4);
            let narrow_tab_width = control("Search task: Task 0").size().width;
            assert!(
                narrow_tab_width < wide_tab_width - 1.0,
                "Tabs must shrink before scrolling: wide={wide_tab_width}, narrow={narrow_tab_width}, theme={theme}, scale={scale}"
            );
            assert!(narrow_tab_width >= 144.0 * scale - 1.0);
            for index in 1..4 {
                assert!(
                    (control(&format!("Search task: Task {index}")).size().width
                        - narrow_tab_width)
                        .abs()
                        < 1.0,
                    "Available width must be shared between tabs, theme={theme}, scale={scale}"
                );
            }
            if scale == 1.0 {
                snapshot_if_requested(&ui, &format!("task-tabs-adaptive-{theme}"));
            }

            // Only a strip that cannot fit minimum-width tabs gains scrolling.
            ui.set_search_tasks(tasks(12));
            ui.window()
                .set_size(slint::LogicalSize::new(1080.0 * scale, 700.0 * scale));
            settle();
            let first_tab = control("Search task: Task 0");
            assert!(
                (first_tab.size().width - 144.0 * scale).abs() < 1.0,
                "Overflow must retain readable minimum tabs, theme={theme}, scale={scale}"
            );
            let strip_id = if ui.global::<crate::Theme>().get_modern_style() {
                "AppWindow::studio-task-tabs-scroll"
            } else {
                "AppWindow::task-tabs-scroll"
            };
            let strip = ElementHandle::find_by_element_id(&ui, strip_id)
                .next()
                .unwrap();
            let scrollbar_lane = strip.absolute_position().y + strip.size().height
                - (first_tab.absolute_position().y + first_tab.size().height);
            assert!(
                scrollbar_lane >= 14.0 * scale - 1.0,
                "The horizontal scrollbar needs a lane below tab content: lane={scrollbar_lane}, theme={theme}, scale={scale}"
            );
            let plus = control("New search task");
            let bar = control("Search tasks");
            let right_gap = bar.absolute_position().x + bar.size().width
                - (plus.absolute_position().x + plus.size().width);
            assert!(
                (-1.0..plus.size().width / 4.0).contains(&right_gap),
                "Overflow must dock New at the bar's right edge: gap={right_gap}, theme={theme}, scale={scale}"
            );
            assert_eq!(plus.accessible_enabled(), Some(true));
            let fixed_x = plus.absolute_position().x;
            click(&ui, "Scroll search tasks right");
            assert!((control("New search task").absolute_position().x - fixed_x).abs() < 1.0);
            click(&ui, "New search task");
            assert_eq!(*created.borrow(), 1);
            if scale == 1.0 {
                snapshot_if_requested(&ui, &format!("task-add-docked-{theme}"));
            }

            ui.window()
                .set_size(slint::LogicalSize::new(2400.0 * scale, 900.0 * scale));
            ui.set_search_tasks(tasks(4));
            settle();
            follows_last_tab(4);
            assert!(
                (control("Search task: Task 0").size().width - wide_tab_width).abs() < 1.0,
                "Tabs must regain their preferred width when room returns, theme={theme}, scale={scale}"
            );
            ui.hide().unwrap();
        }
    }
}

#[test]
fn thirty_task_tabs_page_in_both_directions_with_a_fixed_new_task_control() {
    i_slint_backend_testing::init_no_event_loop();
    for modern in [false, true] {
        for scale in [1.0, 2.0] {
            let ui = AppWindow::new().unwrap();
            ui.global::<crate::Theme>().set_ui_scale(scale);
            ui.global::<crate::Theme>().set_modern_style(modern);
            ui.global::<crate::Theme>().set_modern_player(modern);
            ui.window()
                .set_size(slint::LogicalSize::new(1440.0 * scale, 900.0 * scale));
            ui.set_search_tasks(tasks(2));
            let created = Rc::new(RefCell::new(0));
            let observed = created.clone();
            ui.on_search_task_created(move || *observed.borrow_mut() += 1);
            let selected = Rc::new(RefCell::new(Vec::new()));
            let observed = selected.clone();
            ui.on_search_task_selected(move |id| observed.borrow_mut().push(id.to_string()));
            ui.show().unwrap();
            settle();

            assert!(
                ElementHandle::find_by_accessible_label(&ui, "Scroll search tasks right")
                    .next()
                    .is_none()
            );
            // New remains available at the start of a long strip without scrolling to its end.
            ui.window()
                .set_size(slint::LogicalSize::new(1080.0 * scale, 700.0 * scale));
            ui.set_search_tasks(tasks(29));
            settle();
            click(&ui, "New search task");
            assert_eq!(*created.borrow(), 1);
            ui.set_search_tasks(tasks(30));
            settle();
            let plus = ElementHandle::find_by_accessible_label(&ui, "New search task")
                .next()
                .unwrap();
            let fixed_x = plus.absolute_position().x;
            assert_eq!(plus.accessible_enabled(), Some(false));
            plus.mock_single_click(PointerEventButton::Left);
            assert_eq!(*created.borrow(), 1);
            assert!(selected.borrow().is_empty());
            let strip_id = if modern {
                "AppWindow::studio-task-tabs-scroll"
            } else {
                "AppWindow::task-tabs-scroll"
            };
            let strip = ElementHandle::find_by_element_id(&ui, strip_id)
                .next()
                .unwrap();
            assert!(
                plus.absolute_position().x
                    >= strip.absolute_position().x + strip.size().width - 1.0
            );
            let left = ElementHandle::find_by_accessible_label(&ui, "Scroll search tasks left")
                .next()
                .unwrap();
            assert_eq!(left.accessible_enabled(), Some(false));
            for _ in 0..30 {
                let right =
                    ElementHandle::find_by_accessible_label(&ui, "Scroll search tasks right")
                        .next()
                        .unwrap();
                if right.accessible_enabled() == Some(false) {
                    break;
                }
                right.mock_single_click(PointerEventButton::Left);
                settle();
            }
            assert_eq!(
                ElementHandle::find_by_accessible_label(&ui, "Scroll search tasks right")
                    .next()
                    .unwrap()
                    .accessible_enabled(),
                Some(false)
            );
            let last = ElementHandle::find_by_accessible_label(&ui, "Search task: Task 29")
                .next()
                .unwrap();
            assert!(last.absolute_position().x >= strip.absolute_position().x - 1.0);
            assert!(
                last.absolute_position().x + last.size().width
                    <= strip.absolute_position().x + strip.size().width + 1.0
            );
            assert_eq!(
                ElementHandle::find_by_accessible_label(&ui, "New search task")
                    .next()
                    .unwrap()
                    .absolute_position()
                    .x,
                fixed_x
            );
            for _ in 0..30 {
                let left = ElementHandle::find_by_accessible_label(&ui, "Scroll search tasks left")
                    .next()
                    .unwrap();
                if left.accessible_enabled() == Some(false) {
                    break;
                }
                left.mock_single_click(PointerEventButton::Left);
                settle();
            }
            assert_eq!(
                ElementHandle::find_by_accessible_label(&ui, "Scroll search tasks left")
                    .next()
                    .unwrap()
                    .accessible_enabled(),
                Some(false)
            );
            assert!(selected.borrow().is_empty(), "Paging must not switch tasks");
            // Native horizontal wheel input still reaches the end independently of the arrows.
            ui.window().dispatch_event(WindowEvent::PointerScrolled {
                position: slint::LogicalPosition::new(
                    strip.absolute_position().x + strip.size().width / 2.0,
                    strip.absolute_position().y + strip.size().height / 2.0,
                ),
                delta_x: -100_000.0,
                delta_y: 0.0,
            });
            settle();
            assert!(
                ElementHandle::find_by_accessible_label(&ui, "Search task: Task 29")
                    .next()
                    .is_some()
            );

            // Active-task restoration reveals the last tab even when the strip starts left.
            ui.invoke_reveal_search_task(0);
            settle();
            ui.invoke_reveal_search_task(29);
            settle();
            click(&ui, "Search task: Task 29");
            assert_eq!(&*selected.borrow(), &["29"]);
            ui.set_active_search_task_index(29);
            ui.window()
                .set_size(slint::LogicalSize::new(1440.0 * scale, 900.0 * scale));
            settle();
            ui.window()
                .set_size(slint::LogicalSize::new(1080.0 * scale, 700.0 * scale));
            settle();
            let last = ElementHandle::find_by_accessible_label(&ui, "Search task: Task 29")
                .next()
                .unwrap();
            assert!(last.absolute_position().x >= strip.absolute_position().x - 1.0);
            assert!(
                last.absolute_position().x + last.size().width
                    <= strip.absolute_position().x + strip.size().width + 1.0
            );
            ui.set_active_search_task_index(0);
            ui.set_search_tasks(tasks(2));
            settle();
            assert!(
                ElementHandle::find_by_accessible_label(&ui, "Scroll search tasks right")
                    .next()
                    .is_none()
            );
            assert!(
                ElementHandle::find_by_accessible_label(&ui, "Search task: Task 0")
                    .next()
                    .is_some()
            );
            ui.hide().unwrap();
        }
    }
}

#[test]
fn minimum_width_tabs_keep_keywords_ahead_of_auxiliary_labels_in_both_languages() {
    slint::platform::set_platform(Box::new(TestingBackend::new(TestingBackendOptions {
        mock_time: true,
        threading: false,
        renderer_name: Some("software".into()),
    })))
    .unwrap();
    for theme in audiolib_core::config::THEME_IDS {
        for chinese in [false, true] {
            let ui = AppWindow::new().unwrap();
            crate::theme::apply_theme(&ui, theme);
            ui.set_chinese(chinese);
            ui.window().set_size(slint::LogicalSize::new(1080.0, 700.0));
            let summary = "Search: distant thunderstorm rolling\nDirectory: D:/Sound Library/Weather/Atmospheres";
            ui.set_search_tasks(ModelRc::new(VecModel::from(
                (0..12)
                    .map(|index| SearchTaskRow {
                        id: index.to_string().into(),
                        name: format!("Task {index}").into(),
                        active: index == 0,
                        searched: index == 0,
                        keywords: if index == 0 {
                            "distant thunderstorm · rolling".into()
                        } else {
                            Default::default()
                        },
                        folder: "Atmospheres".into(),
                        summary: summary.into(),
                        ..Default::default()
                    })
                    .collect::<Vec<_>>(),
            )));
            ui.show().unwrap();
            settle();
            let tab_label = if chinese {
                "搜索任务：Task 0"
            } else {
                "Search task: Task 0"
            };
            let tab = ElementHandle::find_by_accessible_label(&ui, tab_label)
                .next()
                .unwrap();
            let keyword = ElementHandle::find_by_element_id(&ui, "SearchTaskTab::keyword-label")
                .next()
                .expect("Minimum tabs must keep their search keywords visible");
            let close_label = if chinese {
                "关闭搜索任务：Task 0"
            } else {
                "Close search task: Task 0"
            };
            let close = ElementHandle::find_by_accessible_label(&ui, close_label)
                .next()
                .unwrap();
            assert!((tab.size().width - 144.0).abs() < 1.0);
            assert!(keyword.size().width >= 90.0);
            assert!(keyword.absolute_position().x >= tab.absolute_position().x);
            assert!(
                keyword.absolute_position().x + keyword.size().width <= close.absolute_position().x,
                "Keyword text must stop before the close control: theme={theme}, chinese={chinese}"
            );
            for id in [
                "SearchTaskTab::directory-separator",
                "SearchTaskTab::directory-label",
            ] {
                assert!(
                    ElementHandle::find_by_element_id(&ui, id).next().is_none(),
                    "Auxiliary labels must yield space to long keywords: {id}, theme={theme}, chinese={chinese}"
                );
            }
            assert!(tab.accessible_description().unwrap().contains(summary));
            let language = if chinese { "cn" } else { "en" };
            snapshot_if_requested(&ui, &format!("task-keywords-minimum-{theme}-{language}"));

            // Short terms stay together instead of spreading across the tab.
            ui.set_search_tasks(ModelRc::new(VecModel::from(vec![SearchTaskRow {
                id: "0".into(),
                name: "Task 0".into(),
                active: true,
                searched: true,
                keywords: "wind".into(),
                folder: "Atmospheres".into(),
                summary: summary.into(),
                ..Default::default()
            }])));
            settle();
            let keyword = ElementHandle::find_by_element_id(&ui, "SearchTaskTab::keyword-label")
                .next()
                .unwrap();
            let separator =
                ElementHandle::find_by_element_id(&ui, "SearchTaskTab::directory-separator")
                    .next()
                    .expect("A fitting directory must have its separator");
            let directory =
                ElementHandle::find_by_element_id(&ui, "SearchTaskTab::directory-label")
                    .next()
                    .expect("A short query must retain its directory context");
            let keyword_end = keyword.absolute_position().x + keyword.size().width;
            let separator_end = separator.absolute_position().x + separator.size().width;
            assert!(
                (0.0..=8.0).contains(&(separator.absolute_position().x - keyword_end)),
                "The separator must follow the keyword closely: theme={theme}, chinese={chinese}"
            );
            assert!(
                (0.0..=8.0).contains(&(directory.absolute_position().x - separator_end)),
                "The directory must follow the separator closely: theme={theme}, chinese={chinese}"
            );
            assert!((keyword.absolute_position().y - directory.absolute_position().y).abs() < 1.0);
            snapshot_if_requested(&ui, &format!("task-keywords-compact-{theme}-{language}"));
            let keyword_start = keyword.absolute_position().x;

            // The directory follows the full query as more terms are added.
            // Wider tabs retain it; narrower tabs give the keywords that space.
            let mut previous_keyword_width = 0.0;
            let mut previous_directory_x = 0.0;
            for (stage, keywords) in [
                "water · runoff",
                "water · runoff · gutter · gush",
                "water · runoff · gutter · gush · torrential · storm · drainage",
            ]
            .into_iter()
            .enumerate()
            {
                ui.set_search_tasks(ModelRc::new(VecModel::from(vec![SearchTaskRow {
                    id: "0".into(),
                    name: "Task 0".into(),
                    active: true,
                    searched: true,
                    keywords: keywords.into(),
                    ..Default::default()
                }])));
                settle();
                let keyword =
                    ElementHandle::find_by_element_id(&ui, "SearchTaskTab::keyword-label")
                        .next()
                        .unwrap();
                let close = ElementHandle::find_by_accessible_label(&ui, close_label)
                    .next()
                    .unwrap();
                assert!(
                    keyword.absolute_position().x + keyword.size().width
                        <= close.absolute_position().x,
                    "Every query length must leave the close control clear: theme={theme}, chinese={chinese}"
                );
                if stage == 1 {
                    assert!(
                        keyword.size().width > previous_keyword_width,
                        "Third and fourth terms must use available tab space: theme={theme}, chinese={chinese}"
                    );
                }
                let directory =
                    ElementHandle::find_by_element_id(&ui, "SearchTaskTab::directory-label").next();
                if let Some(directory) = directory {
                    if stage == 1 {
                        assert!(
                            directory.absolute_position().x > previous_directory_x,
                            "The directory must move after the expanded query: theme={theme}, chinese={chinese}"
                        );
                    }
                    assert!(
                        directory.absolute_position().x + directory.size().width
                            <= close.absolute_position().x
                    );
                    previous_directory_x = directory.absolute_position().x;
                } else {
                    assert!(stage > 0);
                    assert!(
                        ElementHandle::find_by_element_id(
                            &ui,
                            "SearchTaskTab::directory-separator"
                        )
                        .next()
                        .is_none()
                    );
                }
                previous_keyword_width = keyword.size().width;
                snapshot_if_requested(
                    &ui,
                    &format!("task-keywords-expanded-{stage}-{theme}-{language}"),
                );
            }

            // A directory-only task keeps its marker and the same left alignment.
            ui.set_search_tasks(ModelRc::new(VecModel::from(vec![SearchTaskRow {
                id: "0".into(),
                name: "Task 0".into(),
                active: true,
                searched: true,
                folder: "Atmospheres".into(),
                ..Default::default()
            }])));
            settle();
            assert!(
                ElementHandle::find_by_element_id(&ui, "SearchTaskTab::keyword-label")
                    .next()
                    .is_none()
            );
            let separator =
                ElementHandle::find_by_element_id(&ui, "SearchTaskTab::directory-separator")
                    .next()
                    .unwrap();
            let directory =
                ElementHandle::find_by_element_id(&ui, "SearchTaskTab::directory-label")
                    .next()
                    .unwrap();
            assert!((separator.absolute_position().x - keyword_start).abs() < 1.0);
            assert!(directory.absolute_position().x > separator.absolute_position().x);

            // Filter-only conditions have no directory marker or reserved gap.
            ui.set_search_tasks(ModelRc::new(VecModel::from(vec![SearchTaskRow {
                id: "0".into(),
                name: "Task 0".into(),
                active: true,
                searched: true,
                ..Default::default()
            }])));
            settle();
            assert!(
                ElementHandle::find_by_element_id(&ui, "SearchTaskTab::directory-separator")
                    .next()
                    .is_none()
            );
            let filters = ElementHandle::find_by_element_id(&ui, "SearchTaskTab::directory-label")
                .next()
                .unwrap();
            assert!((filters.absolute_position().x - keyword_start).abs() < 1.0);
            ui.hide().unwrap();
        }
    }
}

#[test]
fn search_tab_hover_exposes_the_complete_query_and_directory_without_selecting_the_task() {
    slint::platform::set_platform(Box::new(TestingBackend::new(TestingBackendOptions {
        mock_time: true,
        threading: false,
        renderer_name: Some("software".into()),
    })))
    .unwrap();
    for chinese in [false, true] {
        let ui = window();
        ui.set_chinese(chinese);
        let summary = "Search: hard \"metal hit\" glass\nDirectory: D:/Sound Library/Aircraft/P-51 Mustang\nInclude subfolders: Yes";
        ui.set_search_tasks(ModelRc::new(VecModel::from(vec![SearchTaskRow {
            id: "hover-task".into(),
            name: "Mustang impacts".into(),
            keywords: "hard · metal hit · glass".into(),
            folder: "P-51 Mustang".into(),
            summary: summary.into(),
            searched: true,
            active: true,
            ..Default::default()
        }])));
        ui.on_search_task_selected(|_| panic!("Hover must not select a task"));
        settle();
        let label = if chinese {
            "搜索任务：Mustang impacts"
        } else {
            "Search task: Mustang impacts"
        };
        let tab = ElementHandle::find_by_accessible_label(&ui, label)
            .next()
            .unwrap();
        let accessible_summary = tab.accessible_description().unwrap();
        assert!(accessible_summary.contains("hard \"metal hit\" glass"));
        assert!(accessible_summary.contains("D:/Sound Library/Aircraft/P-51 Mustang"));
        assert!(
            ElementHandle::find_by_accessible_label(&ui, summary)
                .next()
                .is_none()
        );
        let language = if chinese { "cn" } else { "en" };
        snapshot_if_requested(&ui, &format!("task-before-hover-{language}"));
        ui.window().dispatch_event(WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(
                tab.absolute_position().x + tab.size().width / 2.0,
                tab.absolute_position().y + tab.size().height / 2.0,
            ),
        });
        settle();
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(600));
        settle();
        snapshot_if_requested(&ui, &format!("task-after-hover-{language}"));
        assert!(
            ElementHandle::find_by_accessible_label(&ui, summary)
                .next()
                .is_some(),
            "Hover should open the full task tooltip, chinese={chinese}"
        );
        ui.hide().unwrap();
    }
}

#[test]
fn task_creation_obeys_the_configured_limit_and_workspace_loading_guard() {
    i_slint_backend_testing::init_no_event_loop();
    let ui = window();
    let calls = Rc::new(RefCell::new(0));
    let observed = calls.clone();
    ui.on_search_task_created(move || *observed.borrow_mut() += 1);
    ui.set_max_search_tasks(2);
    click(&ui, "New search task");
    assert_eq!(*calls.borrow(), 0);
    ui.set_max_search_tasks(3);
    click(&ui, "New search task");
    assert_eq!(*calls.borrow(), 1);
    ui.set_search_workspace_loading(true);
    click(&ui, "New search task");
    assert_eq!(*calls.borrow(), 1);
    ui.set_search_workspace_loading(false);
    click(&ui, "New search task");
    assert_eq!(*calls.borrow(), 2);
    ui.hide().unwrap();
}

#[test]
fn rename_keeps_task_and_saved_search_targets_separate_and_blocks_background_actions() {
    i_slint_backend_testing::init_no_event_loop();
    let ui = window();
    let events = Rc::new(RefCell::new(Vec::new()));
    let observed = events.clone();
    ui.on_search_task_renamed(move |id, name| {
        observed.borrow_mut().push(format!("task:{id}:{name}"));
    });
    let observed = events.clone();
    ui.on_saved_search_renamed(move |id, name| {
        observed.borrow_mut().push(format!("saved:{id}:{name}"));
    });
    ui.on_search_task_created(|| panic!("Rename must block background task creation"));
    ui.invoke_begin_search_task_rename("1".into(), "Task 1".into(), false);
    settle();
    click(&ui, "New search task");
    ui.set_search_task_rename_name("Metal doors".into());
    click(&ui, "Save name");
    assert!(!ui.get_search_task_rename_visible());
    ui.invoke_begin_search_task_rename("saved-7".into(), "Old name".into(), true);
    settle();
    ui.set_search_task_rename_name("Interior rain".into());
    click(&ui, "Save name");
    assert_eq!(
        &*events.borrow(),
        &["task:1:Metal doors", "saved:saved-7:Interior rain"]
    );
    ui.hide().unwrap();
}

#[test]
fn favorites_categories_keep_audio_filtering_and_saved_search_opening_distinct() {
    i_slint_backend_testing::init_no_event_loop();
    let ui = window();
    ui.set_saved_search_tasks(ModelRc::new(VecModel::from(vec![SavedSearchTaskRow {
        id: "saved-9".into(),
        name: "Forest".into(),
        summary: "wind · Ambience".into(),
    }])));
    let searches = Rc::new(RefCell::new(0));
    let observed = searches.clone();
    ui.on_all_library_requested(move || *observed.borrow_mut() += 1);
    let opened = Rc::new(RefCell::new(Vec::new()));
    let observed = opened.clone();
    ui.on_saved_search_opened(move |id| observed.borrow_mut().push(id.to_string()));
    click(&ui, "Favorites page");
    assert_eq!(ui.get_library_sidebar_page(), 1);
    assert!(ui.get_favorites_only());
    assert_eq!(*searches.borrow(), 1);
    click(&ui, "Search task favorites category");
    assert_eq!(ui.get_favorites_category(), 1);
    assert_eq!(*searches.borrow(), 1);
    click(&ui, "Open saved search: Forest");
    assert_eq!(&*opened.borrow(), &["saved-9"]);
    click(&ui, "Audio favorites category");
    assert_eq!(ui.get_favorites_category(), 0);
    assert_eq!(*searches.borrow(), 2);
    ui.hide().unwrap();
}

#[test]
fn saved_search_update_and_delete_use_the_identity_captured_when_the_menu_opened() {
    i_slint_backend_testing::init_no_event_loop();
    let ui = window();
    ui.set_library_sidebar_page(1);
    ui.set_favorites_category(1);
    let saved = || {
        ModelRc::new(VecModel::from(vec![SavedSearchTaskRow {
            id: "saved-9".into(),
            name: "Forest".into(),
            summary: "wind".into(),
        }]))
    };
    ui.set_saved_search_tasks(saved());
    settle();
    let events = Rc::new(RefCell::new(Vec::new()));
    let observed = events.clone();
    ui.on_saved_search_updated(move |id| observed.borrow_mut().push(format!("update:{id}")));
    let observed = events.clone();
    ui.on_saved_search_deleted(move |id| observed.borrow_mut().push(format!("delete:{id}")));
    open_saved_menu(&ui);
    ui.set_saved_search_tasks(ModelRc::new(VecModel::default()));
    activate_popup(&ui, "Update from current task", 2);
    ui.set_saved_search_tasks(saved());
    settle();
    open_saved_menu(&ui);
    ui.set_saved_search_tasks(ModelRc::new(VecModel::default()));
    activate_popup(&ui, "Delete saved search", 1);
    assert_eq!(&*events.borrow(), &["update:saved-9", "delete:saved-9"]);
    ui.hide().unwrap();
}

#[test]
fn task_viewport_restore_survives_model_and_selection_replacement() {
    i_slint_backend_testing::init_no_event_loop();
    let ui = window();
    ui.set_rows(ModelRc::new(VecModel::from(
        (0..100)
            .map(|index| AudioRow {
                id: index.to_string().into(),
                name: format!("Sound {index}").into(),
                ..Default::default()
            })
            .collect::<Vec<_>>(),
    )));
    ui.set_folders(ModelRc::new(VecModel::from(
        (0..100)
            .map(|index| FolderRow {
                name: format!("Folder {index}").into(),
                ..Default::default()
            })
            .collect::<Vec<_>>(),
    )));
    settle();
    ui.set_task_restoring(true);
    ui.set_selected_row(90);
    ui.invoke_restore_search_task_viewport(-220.0, -80.0, -180.0, 0.0, 0.0);
    assert_eq!(ui.get_task_result_scroll_y(), -220.0);
    assert!(ui.get_task_restoring());
    // Restoration waits for the arriving virtual rows to be laid out on a frame.
    // Keep a bounded deadline while allowing that asynchronous measurement turn.
    for _ in 0..10 {
        settle();
        if !ui.get_task_restoring() {
            break;
        }
    }
    assert!(!ui.get_task_restoring());
    assert!((ui.get_task_result_scroll_y() + 220.0).abs() < 1.0);
    assert!((ui.get_task_folder_scroll_y() + 180.0).abs() < 1.0);
    assert!(ui.get_task_result_row_height() > 0.0);
    ui.hide().unwrap();
}

#[test]
fn thirty_task_tabs_and_saved_searches_render_at_all_supported_layout_extremes() {
    slint::platform::set_platform(Box::new(TestingBackend::new(TestingBackendOptions {
        mock_time: true,
        threading: false,
        renderer_name: Some("software".into()),
    })))
    .unwrap();
    for modern in [false, true] {
        for scale in [1.0, 1.5, 2.0] {
            for chinese in [false, true] {
                let ui = AppWindow::new().unwrap();
                ui.window().set_size(slint::LogicalSize::new(1080.0, 700.0));
                ui.global::<crate::Theme>().set_ui_scale(scale);
                ui.global::<crate::Theme>().set_modern_style(modern);
                ui.global::<crate::Theme>().set_modern_player(modern);
                ui.set_chinese(chinese);
                ui.set_search_tasks(tasks(30));
                ui.set_library_sidebar_page(1);
                ui.set_favorites_category(1);
                ui.set_saved_search_tasks(ModelRc::new(VecModel::from(vec![SavedSearchTaskRow {
                    id: "saved-1".into(),
                    name: "森林环境声音 / Forest ambience".into(),
                    summary: "wind -interior · Ambience/Forest".into(),
                }])));
                ui.show().unwrap();
                let pixels = ui.window().take_snapshot().unwrap();
                assert!(pixels.width() >= 1080 && pixels.height() >= 700);
                if let Some(directory) = std::env::var_os("AUDIOLIB_MULTI_SEARCH_SNAPSHOT_DIR") {
                    let directory = std::path::Path::new(&directory);
                    std::fs::create_dir_all(directory).unwrap();
                    let language = if chinese { "cn" } else { "en" };
                    let style = if modern { "studio" } else { "classic" };
                    image::save_buffer(
                        directory.join(format!("tasks-{style}-{language}-x{scale}.png")),
                        pixels.as_bytes(),
                        pixels.width(),
                        pixels.height(),
                        image::ColorType::Rgba8,
                    )
                    .unwrap();
                }
                ui.hide().unwrap();
            }
        }
    }
}
