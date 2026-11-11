// app/audiolib-desktop/src/window_layout.rs
//! Restore and capture the desktop window geometry and pane layout.
//!
//! All functions run on the UI thread and only touch Slint/winit window state;
//! persistence happens on the preferences worker or during shutdown, never in
//! audio callbacks.

use audiolib_core::window_layout::{ScreenRect, WindowLayoutPreferences, fit_window_to_monitors};
use slint::ComponentHandle;

use crate::AppWindow;

/// Apply the saved window size, position, maximized state and pane visibility
/// before the window is first shown, so it does not visibly jump on startup.
/// The right side panel always opens on details, so the saved page is not applied.
pub(crate) fn restore_before_show(ui: &AppWindow, layout: &WindowLayoutPreferences) {
    let window = ui.window();
    if let (Some(width), Some(height)) = (layout.width, layout.height) {
        window.set_size(slint::LogicalSize::new(width, height));
    }
    if let (Some(x), Some(y)) = (layout.x, layout.y) {
        window.set_position(slint::PhysicalPosition::new(x, y));
    }
    if let Some(visible) = layout.sidebar_visible {
        ui.set_sidebar_visible(visible);
    }
    if let Some(visible) = layout.inspector_visible {
        ui.set_inspector_visible(visible);
    }
    if layout.maximized {
        window.set_maximized(true);
    }
}

/// Apply saved pane widths and sidebar section layout; call after the interface
/// scale is applied because widths are stored at interface scale 100%.
pub(crate) fn restore_panes(ui: &AppWindow, layout: &WindowLayoutPreferences) {
    let scale = interface_scale(ui);
    if let Some(width) = layout.sidebar_width {
        ui.set_sidebar_preferred_width(width * scale);
    }
    if let Some(width) = layout.inspector_width {
        ui.set_inspector_preferred_width(width * scale);
    }
    if let Some(expanded) = layout.folders_section_expanded {
        ui.set_folders_section_expanded(expanded);
    }
    if let Some(expanded) = layout.libraries_section_expanded {
        ui.set_libraries_section_expanded(expanded);
    }
    if let Some(ratio) = layout.folders_section_ratio {
        ui.set_folders_section_ratio(ratio);
    }
}

/// After the window is shown, move a restored window back onto a connected
/// monitor if its saved position is no longer reachable (for example after a
/// monitor was disconnected), shrinking it to fit when needed.
pub(crate) fn keep_on_screen(ui: &AppWindow) {
    use slint::winit_030::WinitWindowAccessor;
    let window = ui.window();
    if window.is_maximized() || window.is_minimized() || window.is_fullscreen() {
        return;
    }
    let monitors = window
        .with_winit_window(|winit| {
            winit
                .available_monitors()
                .map(|monitor| {
                    let position = monitor.position();
                    let size = monitor.size();
                    ScreenRect {
                        x: position.x,
                        y: position.y,
                        width: size.width,
                        height: size.height,
                    }
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let position = window.position();
    let size = window.size();
    let saved = ScreenRect {
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
    };
    let (fitted, fitted_size) = fit_window_to_monitors(saved, &monitors);
    if fitted_size != (size.width, size.height) {
        window.set_size(slint::PhysicalSize::new(fitted_size.0, fitted_size.1));
    }
    if let Some((x, y)) = fitted
        && (x, y) != (position.x, position.y)
    {
        window.set_position(slint::PhysicalPosition::new(x, y));
    }
}

/// Capture the current layout. While maximized or minimized the last normal
/// window size and position are kept, so restoring un-maximizes to them.
pub(crate) fn capture(
    ui: &AppWindow,
    previous: &WindowLayoutPreferences,
) -> WindowLayoutPreferences {
    let window = ui.window();
    let mut layout = previous.clone();
    if !window.is_minimized() {
        layout.maximized = window.is_maximized();
        if !layout.maximized && !window.is_fullscreen() {
            let size = window.size().to_logical(window.scale_factor());
            if size.width > 0.0 && size.height > 0.0 {
                layout.width = Some(size.width);
                layout.height = Some(size.height);
            }
            let position = window.position();
            layout.x = Some(position.x);
            layout.y = Some(position.y);
        }
    }
    let scale = interface_scale(ui);
    layout.sidebar_width = Some(ui.get_sidebar_preferred_width() / scale);
    layout.sidebar_visible = Some(ui.get_sidebar_visible());
    layout.inspector_width = Some(ui.get_inspector_preferred_width() / scale);
    layout.inspector_visible = Some(ui.get_inspector_visible());
    layout.folders_section_expanded = Some(ui.get_folders_section_expanded());
    layout.libraries_section_expanded = Some(ui.get_libraries_section_expanded());
    layout.folders_section_ratio = Some(ui.get_folders_section_ratio());
    // `filters_visible` stays as saved: the page is not restored (see restore_before_show)
    // and older builds that still read it keep their previous value.
    layout.normalized()
}

fn interface_scale(ui: &AppWindow) -> f32 {
    let scale = ui.global::<crate::Theme>().get_ui_scale();
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_layout_round_trips_through_the_window_at_any_interface_scale() {
        i_slint_backend_testing::init_no_event_loop();
        for scale in [1.0_f32, 1.5] {
            let ui = AppWindow::new().unwrap();
            ui.global::<crate::Theme>().set_ui_scale(scale);
            let saved = WindowLayoutPreferences {
                sidebar_width: Some(260.0),
                sidebar_visible: Some(false),
                inspector_width: Some(340.0),
                inspector_visible: Some(false),
                filters_visible: Some(true),
                folders_section_expanded: Some(false),
                libraries_section_expanded: Some(true),
                folders_section_ratio: Some(0.7),
                ..Default::default()
            };
            restore_before_show(&ui, &saved);
            restore_panes(&ui, &saved);
            assert_eq!(ui.get_sidebar_preferred_width(), 260.0 * scale);
            assert!(!ui.get_sidebar_visible());
            // A saved filter page is not restored: every start shows sound details.
            assert!(!ui.get_inspector_visible() && !ui.get_filters_visible());
            let captured = capture(&ui, &saved);
            assert_eq!(captured.sidebar_width, Some(260.0));
            assert_eq!(captured.sidebar_visible, Some(false));
            assert_eq!(captured.inspector_width, Some(340.0));
            assert_eq!(captured.inspector_visible, Some(false));
            assert_eq!(captured.filters_visible, Some(true));
            assert_eq!(captured.folders_section_expanded, Some(false));
            assert_eq!(captured.libraries_section_expanded, Some(true));
            assert_eq!(captured.folders_section_ratio, Some(0.7));
        }
    }

    #[test]
    fn startup_opens_the_sidebar_on_the_details_page() {
        i_slint_backend_testing::init_no_event_loop();
        let ui = AppWindow::new().unwrap();
        restore_before_show(
            &ui,
            &WindowLayoutPreferences {
                inspector_visible: Some(true),
                filters_visible: Some(true),
                ..Default::default()
            },
        );
        assert!(ui.get_inspector_visible() && !ui.get_filters_visible());
        // The page chosen during the session is left for older builds, never re-applied.
        ui.set_filters_visible(true);
        let captured = capture(&ui, &WindowLayoutPreferences::default());
        assert_eq!(captured.filters_visible, None);
        assert_eq!(captured.inspector_visible, Some(true));
    }

    #[test]
    fn missing_layout_keeps_the_design_defaults() {
        i_slint_backend_testing::init_no_event_loop();
        let ui = AppWindow::new().unwrap();
        let sidebar = ui.get_sidebar_preferred_width();
        let inspector = ui.get_inspector_preferred_width();
        let visible = (
            ui.get_sidebar_visible(),
            ui.get_inspector_visible(),
            ui.get_filters_visible(),
        );
        let sections = (
            ui.get_folders_section_expanded(),
            ui.get_libraries_section_expanded(),
            ui.get_folders_section_ratio(),
        );
        let empty = WindowLayoutPreferences::default();
        restore_before_show(&ui, &empty);
        restore_panes(&ui, &empty);
        assert_eq!(ui.get_sidebar_preferred_width(), sidebar);
        assert_eq!(ui.get_inspector_preferred_width(), inspector);
        assert_eq!(
            (
                ui.get_sidebar_visible(),
                ui.get_inspector_visible(),
                ui.get_filters_visible(),
            ),
            visible
        );
        assert_eq!(
            (
                ui.get_folders_section_expanded(),
                ui.get_libraries_section_expanded(),
                ui.get_folders_section_ratio(),
            ),
            sections
        );
    }

    #[test]
    fn library_sidebar_visibility_survives_restart_and_scale_changes() {
        i_slint_backend_testing::init_no_event_loop();
        for theme in audiolib_core::config::THEME_IDS {
            for percent in [100, 150, 200] {
                let older: WindowLayoutPreferences = serde_json::from_value(serde_json::json!({
                    "width": 1280,
                    "height": 800,
                    "sidebar_width": 275,
                    "inspector_width": 340,
                    "inspector_visible": false,
                    "filters_visible": true,
                    "folders_section_expanded": false,
                    "libraries_section_expanded": true,
                    "folders_section_ratio": 0.7
                }))
                .unwrap();
                let ui = AppWindow::new().unwrap();
                crate::theme::apply_theme(&ui, theme);
                restore_before_show(&ui, &older);
                crate::controller::apply_ui_scale(&ui, percent);
                restore_panes(&ui, &older);
                assert!(ui.get_sidebar_visible());

                ui.set_sidebar_visible(false);
                let captured = capture(&ui, &older);
                assert_eq!(captured.sidebar_visible, Some(false));
                assert_eq!(captured.sidebar_width, Some(275.0));
                assert_eq!(captured.inspector_width, Some(340.0));
                assert_eq!(captured.inspector_visible, Some(false));
                assert_eq!(captured.filters_visible, Some(true));
                assert_eq!(captured.folders_section_expanded, Some(false));
                assert_eq!(captured.libraries_section_expanded, Some(true));
                assert_eq!(captured.folders_section_ratio, Some(0.7));

                let saved = serde_json::to_string(&captured).unwrap();
                let restored: WindowLayoutPreferences = serde_json::from_str(&saved).unwrap();
                let restarted = AppWindow::new().unwrap();
                crate::theme::apply_theme(&restarted, theme);
                restore_before_show(&restarted, &restored);
                let next_percent = if percent == 200 { 100 } else { 200 };
                crate::controller::apply_ui_scale(&restarted, next_percent);
                restore_panes(&restarted, &restored);
                assert!(!restarted.get_sidebar_visible());
                assert_eq!(
                    restarted.get_sidebar_preferred_width(),
                    275.0 * next_percent as f32 / 100.0
                );
                assert!(!restarted.get_inspector_visible());
                assert!(!restarted.get_filters_visible());

                restarted.set_sidebar_visible(true);
                let shown = capture(&restarted, &restored);
                assert_eq!(shown.sidebar_visible, Some(true));
                assert_eq!(shown.sidebar_width, Some(275.0));
                let reopened = AppWindow::new().unwrap();
                reopened.set_sidebar_visible(false);
                restore_before_show(&reopened, &shown);
                assert!(reopened.get_sidebar_visible());
            }
        }
    }

    #[test]
    fn restored_window_size_is_captured_back() {
        i_slint_backend_testing::init_no_event_loop();
        let ui = AppWindow::new().unwrap();
        let saved = WindowLayoutPreferences {
            width: Some(1200.0),
            height: Some(760.0),
            ..Default::default()
        };
        restore_before_show(&ui, &saved);
        ui.show().unwrap();
        let captured = capture(&ui, &WindowLayoutPreferences::default());
        assert_eq!(
            (captured.width, captured.height),
            (Some(1200.0), Some(760.0))
        );
        assert!(!captured.maximized);
        ui.hide().unwrap();
    }
}
