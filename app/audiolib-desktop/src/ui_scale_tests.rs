// app/audiolib-desktop/src/ui_scale_tests.rs
//! Headless software-rendered snapshots of synthetic UI states. With
//! `AUDIOLIB_UI_SCALE_SNAPSHOT_DIR` set, every rendered frame is also saved as
//! PNG so layouts can be compared pixel by pixel across revisions.
use crate::{AppWindow, AudioRow, FolderRow};
use i_slint_backend_testing::{TestingBackend, TestingBackendOptions};
use slint::{ComponentHandle, ModelRc, VecModel};

fn init_software_backend() {
    slint::platform::set_platform(Box::new(TestingBackend::new(TestingBackendOptions {
        mock_time: true,
        threading: false,
        renderer_name: Some("software".into()),
    })))
    .expect("platform already initialized");
}

fn synthetic_rows() -> ModelRc<AudioRow> {
    let rows = (0..12)
        .map(|index| AudioRow {
            id: format!("{index}").into(),
            file_path: format!("C:/Fixture/Sounds/Synthetic {index:02}.wav").into(),
            name: format!("Synthetic tone {index:02}.wav").into(),
            folder: "Fixture".into(),
            duration: format!("0:0{}", index % 10).into(),
            format: "WAV 48 kHz 24-bit".into(),
            channels: "2".into(),
            tags: "fixture, tone".into(),
            favorite: index % 3 == 0,
            missing: index == 5,
            checked: index % 4 == 1,
            ..Default::default()
        })
        .collect::<Vec<_>>();
    ModelRc::new(VecModel::from(rows))
}

fn synthetic_folders() -> ModelRc<FolderRow> {
    let folders = ["Fixture", "Ambience", "Foley", "Weapons"]
        .iter()
        .enumerate()
        .map(|(depth, name)| FolderRow {
            name: (*name).into(),
            count: "12".into(),
            path: format!("C:/Fixture/{name}").into(),
            depth: (depth.min(1)) as i32,
            expanded: true,
            has_children: depth == 0,
        })
        .collect::<Vec<_>>();
    ModelRc::new(VecModel::from(folders))
}

type Setup = fn(&AppWindow);

const VIEWS: &[(&str, Setup)] = &[
    ("main", |_| {}),
    ("settings", |ui| ui.set_settings_visible(true)),
    ("about", |ui| ui.set_about_visible(true)),
    ("help", |ui| ui.set_help_visible(true)),
    ("wide", |ui| ui.set_inspector_visible(false)),
];

fn render(scale: f32, chinese: bool, name: &str, setup: Setup) -> (u32, u32) {
    let ui = AppWindow::new().unwrap();
    ui.window()
        .set_size(slint::LogicalSize::new(1440.0, 900.0));
    if scale != 1.0 {
        ui.global::<crate::Theme>().set_ui_scale(scale);
    }
    ui.set_chinese(chinese);
    ui.set_rows(synthetic_rows());
    ui.set_folders(synthetic_folders());
    setup(&ui);
    ui.show().unwrap();
    let pixels = ui.window().take_snapshot().unwrap();
    assert!(pixels.width() > 0 && pixels.height() > 0);
    if let Some(directory) = std::env::var_os("AUDIOLIB_UI_SCALE_SNAPSHOT_DIR") {
        let directory = std::path::Path::new(&directory);
        std::fs::create_dir_all(directory).unwrap();
        let language = if chinese { "cn" } else { "en" };
        image::save_buffer(
            directory.join(format!("{name}-{language}-x{scale}.png")),
            pixels.as_bytes(),
            pixels.width(),
            pixels.height(),
            image::ColorType::Rgba8,
        )
        .unwrap();
    }
    ui.hide().unwrap();
    (pixels.width(), pixels.height())
}

#[test]
fn ui_scale_renders_without_panicking_at_1_0_1_5_and_2_0() {
    init_software_backend();
    for scale in [1.0, 1.5, 2.0] {
        for chinese in [false, true] {
            for (name, setup) in VIEWS {
                let (width, height) = render(scale, chinese, name, *setup);
                assert!(width >= 1440 && height >= 900, "{name} x{scale}");
            }
        }
    }
}
