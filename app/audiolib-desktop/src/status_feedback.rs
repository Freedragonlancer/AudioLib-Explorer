// app/audiolib-desktop/src/status_feedback.rs
//! Scoped foreground audio progress and delivery-time guards for worker feedback.
use super::*;
use audiolib_audio::{CommandCompletion, PlaybackStatus};

/// The foreground operation still awaiting an actual engine acknowledgment.
#[derive(Clone, Copy)]
pub(super) enum AudioActivity {
    Loading,
    Previewing,
    Seeking,
}

impl AudioActivity {
    fn pending(self, chinese: bool) -> &'static str {
        match (self, chinese) {
            (Self::Loading, true) => "正在准备音频，请稍候…",
            (Self::Loading, false) => "Preparing audio…",
            (Self::Previewing, true) => "正在准备选区试听…",
            (Self::Previewing, false) => "Preparing the selected range…",
            (Self::Seeking, true) => "正在调整播放位置…",
            (Self::Seeking, false) => "Seeking…",
        }
    }
}

impl Controller {
    /// Wait on a worker without discarding a late acknowledgment. Pending is
    /// progress, not failure; Stop, source changes and newer selections abandon
    /// observation promptly without pretending to undo an admitted command.
    pub(super) fn wait_audio_confirmation(
        self: &Arc<Self>,
        completion: &CommandCompletion,
        cancel: &Arc<AtomicBool>,
        generation: u64,
        activity: AudioActivity,
        current: impl Fn(&Controller) -> bool,
    ) -> audiolib_core::Result<PlaybackStatus> {
        let library = self.library_generation.load(Ordering::Acquire);
        let source = self.source_generation.load(Ordering::Acquire);
        let wait = self
            .begin_remote_audition_wait(generation, cancel)
            .map_err(|_| audiolib_core::Error::Cancelled)?;
        let started = Instant::now();
        let mut shown = false;
        let result = (|| {
            loop {
                let valid = || {
                    self.available()
                        && !self.closing.load(Ordering::Acquire)
                        && !self.close_retains_status.load(Ordering::Acquire)
                        && !cancel.load(Ordering::Acquire)
                        && self.selection_generation.load(Ordering::Acquire) == generation
                        && self.library_generation.load(Ordering::Acquire) == library
                        && self.source_generation.load(Ordering::Acquire) == source
                        && current(self)
                };
                if !valid() {
                    return Err(audiolib_core::Error::Cancelled);
                }
                let result = completion.wait_slice(Duration::from_millis(100));
                if !valid() {
                    return Err(audiolib_core::Error::Cancelled);
                }
                match result {
                    Ok(Some(status)) => return Ok(status),
                    Err(error) => return Err(error),
                    Ok(None) => {}
                }
                if !shown && started.elapsed() >= Duration::from_millis(1500) {
                    shown = true;
                    self.publish_remote_audition_wait(
                        &wait,
                        activity
                            .pending(self.chinese.load(Ordering::Acquire))
                            .into(),
                    );
                }
            }
        })();
        self.finish_remote_audition_wait(&wait);
        result
    }

    /// Publish job-owned feedback only if its source and job are still current
    /// when Slint delivers it, including messages queued before close recovery.
    pub(super) fn status_when(
        self: &Arc<Self>,
        message: String,
        current: impl FnOnce(&Controller) -> bool + Send + 'static,
    ) {
        let controller = self.clone();
        let library = self.library_generation.load(Ordering::Acquire);
        let source = self.source_generation.load(Ordering::Acquire);
        post(self.ui.clone(), move |ui| {
            if controller.available()
                && !ui.get_closing()
                && !controller.close_retains_status.load(Ordering::Acquire)
                && controller.library_generation.load(Ordering::Acquire) == library
                && controller.source_generation.load(Ordering::Acquire) == source
                && current(&controller)
            {
                ui.set_status(message.into());
            }
        });
    }
}

#[cfg(test)]
#[path = "status_feedback_tests.rs"]
mod tests;
