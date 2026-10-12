// app/audiolib-desktop/src/window_repaint.rs
//! Restore software surfaces without changing the retained component/model state.
use slint::winit_030::{EventResult, WinitWindowAccessor, winit::event::WindowEvent};

/// Install once, before the first show, on the event-loop-owned desktop window.
pub(crate) fn install(window: &slint::Window) {
    window.on_winit_window_event(|window, event| {
        handle_event(window, event);
        EventResult::Propagate
    });
}

fn handle_event(window: &slint::Window, event: &WindowEvent) {
    match event {
        WindowEvent::Focused(true) | WindowEvent::Occluded(false) => window.request_redraw(),
        WindowEvent::Resized(size) if size.width > 0 && size.height > 0 => {
            window.request_redraw();
        }
        // On Windows Occluded is not emitted for ordinary covering/uncovering.
        // WM_PAINT exposure arrives as RedrawRequested; softbuffer's age may
        // still say "reused" even though Windows cleared the presented surface.
        // Invalidate before rendering, even when no application property changed.
        // No timer or additional redraw is requested here, so idle stays idle.
        WindowEvent::RedrawRequested if cfg!(windows) => mark_full_damage(window),
        _ => {}
    }
}

fn mark_full_damage(window: &slint::Window) {
    // Slint 1.17.1 has a public request_redraw, but it does not invalidate its
    // partial-renderer cache. Keep this private version-pinned adapter boundary
    // narrow; revisit it when upgrading Slint. This touches rendering only.
    use slint::private_unstable_api::re_exports::{LogicalRect, WindowInner};
    let size = window.size().to_logical(window.scale_factor());
    let rectangle = LogicalRect::new([0.0, 0.0].into(), [size.width, size.height].into());
    WindowInner::from_pub(window)
        .window_adapter()
        .renderer()
        .mark_dirty_region(rectangle.into());
}

// This regression exercises the Windows-only native exposure hook.
#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use slint::ComponentHandle;
    use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
    use std::rc::Rc;

    struct Platform(Rc<MinimalSoftwareWindow>);
    impl slint::platform::Platform for Platform {
        fn create_window_adapter(
            &self,
        ) -> Result<Rc<dyn slint::platform::WindowAdapter>, slint::PlatformError> {
            Ok(self.0.clone())
        }
    }

    #[test]
    fn exposure_repaints_cleared_reused_buffer_and_keeps_form_state() {
        // Platform selection is thread-local; no user window/database/audio exists.
        let adapter = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        slint::platform::set_platform(Box::new(Platform(adapter.clone()))).unwrap();
        let ui = crate::AppWindow::new().unwrap();
        ui.window().set_size(slint::LogicalSize::new(1080.0, 700.0));
        ui.set_selected_id("42".into());
        ui.set_edit_notes("unsaved audition notes".into());
        ui.set_ds_server_url("https://example.invalid/audio".into());
        ui.set_ds_token("unsaved test credential".into());
        ui.set_library_visible(true);
        ui.set_sm_selected(-1);
        ui.show().unwrap();
        let render = |pixels: &mut [slint::Rgb8Pixel]| {
            let mut damage = slint::PhysicalSize::default();
            adapter.draw_if_needed(|renderer| {
                damage = renderer.render(pixels, 1080).bounding_box_size();
            });
            damage
        };
        let mut initial = vec![slint::Rgb8Pixel::default(); 1080 * 700];
        render(&mut initial);
        assert!(
            initial
                .iter()
                .any(|pixel| pixel.r != 0 || pixel.g != 0 || pixel.b != 0)
        );
        let mut exposed = vec![slint::Rgb8Pixel::default(); initial.len()];
        // A redraw alone correctly demonstrates the broken reused-surface case.
        ui.window().request_redraw();
        let partial = render(&mut exposed);
        assert!(partial.width < 1080 || partial.height < 700);
        assert_ne!(exposed, initial);
        for event in [
            WindowEvent::Focused(true),
            WindowEvent::Occluded(false),
            WindowEvent::Resized(slint::winit_030::winit::dpi::PhysicalSize::new(1080, 700)),
        ] {
            exposed.fill(slint::Rgb8Pixel::default());
            handle_event(ui.window(), &event);
            handle_event(ui.window(), &WindowEvent::RedrawRequested);
            let started = std::time::Instant::now();
            let full = render(&mut exposed);
            println!("1080x700 full software repaint: {:?}", started.elapsed());
            assert_eq!(full, slint::PhysicalSize::new(1080, 700));
            assert_eq!(exposed, initial);
            assert_eq!(ui.get_selected_id(), "42");
            assert_eq!(ui.get_edit_notes(), "unsaved audition notes");
            assert_eq!(ui.get_ds_server_url(), "https://example.invalid/audio");
            assert_eq!(ui.get_ds_token(), "unsaved test credential");
        }
        // Rendering the requested frame does not schedule another frame.
        assert!(!adapter.draw_if_needed(|_| panic!("unexpected idle repaint")));
    }
}
