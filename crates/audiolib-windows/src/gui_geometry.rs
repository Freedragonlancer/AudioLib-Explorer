// crates/audiolib-windows/src/gui_geometry.rs
//! Explicit diagnostic geometry history; normal application mode never installs it.
//! Own-HWND/UI-thread geometry history before winit can buffer WM_SIZE events.
//! Windows 0.62.2 projection: SetWindowSubclass/RemoveWindowSubclass/
//! DefSubclassProc are in Win32::UI::Shell (already enabled in audiolib-windows).
//! Read-only geometry queries preserve the existing thread/window DPI contexts.
//! No input injection, window mutation, menu modification, pixels or titles.
//!
//! Integration: arm after initial WGC calibration immediately before measured
//! input; keep this guard on the HWND's UI thread; poll history outside application
//! state locks; accept an ALL frame only after frame_after_restore() returns true AND
//! the existing decoder/visibility/generation/overflow checks pass. Explicitly
//! stop before normal controller close/hide; do not clear fatal history or replace
//! samples. A RejectedFrame remains invalid under the existing capture policy.
//!
//! Limits: this establishes synchronous native geometry history since arm, not
//! compositor metadata for a historical frame. Frame.ContentSize plus the pixel
//! marker remains required. Frame-side before/after geometry snapshots are useful
//! additional evidence, but they occur at dequeue/read time, not at frame time.
//! A matching current snapshot alone cannot excuse an older buffered event.

#![cfg(windows)]

use crate::gui_capture::{PixelRect, QpcStamp, input_to_frame_ms, qpc_now};
use std::{
    cell::{Cell, RefCell},
    ffi::c_void,
    rc::Rc,
};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute},
    System::Threading::GetCurrentThreadId,
    UI::{
        HiDpi::{
            DPI_AWARENESS_PER_MONITOR_AWARE, GetAwarenessFromDpiAwarenessContext, GetDpiForWindow,
            GetThreadDpiAwarenessContext, GetWindowDpiAwarenessContext,
        },
        Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::{
            GetClientRect, GetWindowRect, GetWindowThreadProcessId, SWP_NOMOVE, SWP_NOSIZE,
            WINDOWPOS, WM_DPICHANGED, WM_NCDESTROY, WM_SIZE, WM_WINDOWPOSCHANGED,
        },
    },
};

/// Physical rectangles: window_rect includes invisible borders, frame_rect does
/// not. client_screen fixes both client dimensions AND its placement in the frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Geometry {
    /// Physical GetWindowRect, including invisible resize borders.
    pub window_rect: PixelRect,
    /// Physical DWM extended frame, excluding invisible resize borders.
    pub frame_rect: PixelRect,
    /// Physical client bounds translated to screen coordinates.
    pub client_screen: PixelRect,
    /// DPI reported by GetDpiForWindow without changing any DPI context.
    pub dpi: u32,
}

/// Copied WM_WINDOWPOSCHANGED scalars; no HWND or message pointer is retained.
#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct WindowPosEvidence {
    /// Native SWP flags; NOMOVE and NOSIZE make their corresponding fields unused.
    pub flags: u32,
    /// Requested outer x, meaningful only without SWP_NOMOVE.
    pub x: i32,
    /// Requested outer y, meaningful only without SWP_NOMOVE.
    pub y: i32,
    /// Requested outer width, meaningful only without SWP_NOSIZE.
    pub width: i32,
    /// Requested outer height, meaningful only without SWP_NOSIZE.
    pub height: i32,
}

/// Native geometry messages observed synchronously before forwarding to winit.
#[derive(Clone, Copy, Debug, serde::Serialize)]
pub enum NativeEvent {
    /// Carries outer-change evidence even if a later current query is restored.
    WindowPosChanged(WindowPosEvidence),
    /// Original unsigned 16-bit WM_SIZE client-width/client-height payload.
    ClientSize { payload: [u32; 2] },
    /// Any DPI transition invalidates the armed sample.
    DpiChanged,
    /// Unexpected native destruction invalidates the armed sample.
    NcDestroy,
}

/// One bounded native event and the adjacent owned-window observation.
#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct Record {
    /// Timestamp after the synchronous OS geometry query, before forwarding.
    pub observed_at: QpcStamp,
    /// Original message classification and copied scalar payload.
    pub event: NativeEvent,
    /// None when geometry cannot be established or the event is terminal/DPI.
    pub geometry: Option<Geometry>,
    /// This observation established return to the complete client baseline.
    pub client_restored: bool,
}

/// First failure is sticky. Callback code stores scalars, never formats/logs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub enum Failure {
    /// Own-window identity, DPI awareness or geometry could not be established.
    NativeQuery,
    /// The native monotonic clock was unavailable.
    ClockQuery,
    /// Full outer bounds or DWM frame changed, including changes later restored.
    OuterChanged,
    /// Native DPI changed, including a change later restored.
    DpiChanged,
    /// WM_NCDESTROY arrived before explicit measurement stop.
    NativeDestroyed,
    /// The fixed event budget was exhausted; missing history is never accepted.
    QueueOverflow,
    /// Reentrancy prevented a complete history observation.
    ReentrantBorrow,
    /// Rust code unwound inside the FFI panic boundary.
    CallbackPanic,
    /// A caller or callback used a thread other than the HWND owner.
    WrongThread,
    /// Subclass removal failed; callback ownership is conservatively retained.
    DetachFailed,
    /// WINDOWPOS was absent or named another window.
    MalformedWindowPos,
    /// WM_SIZE payload disagreed with the immediate client-geometry observation.
    NativeSizeMismatch,
}

/// Bounded sample history. A fatal result is sticky even after baseline recovery.
#[derive(Clone, Debug, serde::Serialize)]
pub struct History {
    /// Stable, adjacent own-window reads captured immediately before installation.
    pub baseline: Geometry,
    /// Native timestamp sampled before installing the subclass.
    pub armed_at: QpcStamp,
    /// Last observed complete client recovery, or armed_at when none was needed.
    pub last_client_restore: QpcStamp,
    /// Client bounds currently differ from the armed baseline.
    pub client_layout_pending: bool,
    /// Number of observed entries into a nonbaseline client-layout epoch.
    pub client_transitions: u64,
    /// Events omitted after the fixed history capacity was exhausted.
    pub overflow: u64,
    /// First failure; returning to baseline never clears it.
    pub fatal: Option<Failure>,
    /// Native events in callback order, bounded by the arm event_limit.
    pub events: Vec<Record>,
}

struct Inner {
    history: History,
    capacity: usize,
}

struct State {
    hwnd: HWND,
    ui_thread: u32,
    // Same UI thread only. No borrow may survive any call to DefSubclassProc.
    inner: RefCell<Inner>,
    fatal: Cell<Option<Failure>>,
    installation_ref_live: Cell<bool>,
}

/// Rc deliberately makes the guard !Send/!Sync. One extra raw Rc belongs to the
/// installed subclass, so failed removal or reentrant destruction cannot free
/// dwRefData while a callback is still live.
pub struct NativeGeometryGuard {
    state: Rc<State>,
}

fn pixel_rect(value: RECT) -> PixelRect {
    PixelRect {
        left: value.left,
        top: value.top,
        right: value.right,
        bottom: value.bottom,
    }
}

fn valid_rect(rectangle: PixelRect) -> bool {
    rectangle.left < rectangle.right && rectangle.top < rectangle.bottom
}

fn geometry_once(hwnd: HWND) -> Result<Geometry, Failure> {
    let mut owner = 0;
    let mut outer = RECT::default();
    let mut frame = RECT::default();
    let mut client = RECT::default();
    let mut origin = POINT::default();
    // SAFETY: read-only own-window metadata into initialized local outputs. DPI
    // contexts must be Per-Monitor aware because GetWindowRect is DPI virtualized
    // while DWM extended-frame bounds are physical. Never change a DPI context.
    unsafe {
        if GetWindowThreadProcessId(hwnd, Some(&mut owner)) == 0
            || owner != std::process::id()
            || GetAwarenessFromDpiAwarenessContext(GetThreadDpiAwarenessContext())
                != DPI_AWARENESS_PER_MONITOR_AWARE
            || GetAwarenessFromDpiAwarenessContext(GetWindowDpiAwarenessContext(hwnd))
                != DPI_AWARENESS_PER_MONITOR_AWARE
        {
            return Err(Failure::NativeQuery);
        }
        GetWindowRect(hwnd, &mut outer).map_err(|_| Failure::NativeQuery)?;
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&mut frame as *mut RECT).cast(),
            std::mem::size_of::<RECT>() as u32,
        )
        .map_err(|_| Failure::NativeQuery)?;
        GetClientRect(hwnd, &mut client).map_err(|_| Failure::NativeQuery)?;
        if !ClientToScreen(hwnd, &mut origin).as_bool() {
            return Err(Failure::NativeQuery);
        }
        let dpi = GetDpiForWindow(hwnd);
        let result = Geometry {
            window_rect: pixel_rect(outer),
            frame_rect: pixel_rect(frame),
            dpi,
            client_screen: PixelRect {
                left: origin.x,
                top: origin.y,
                right: origin
                    .x
                    .checked_add(client.right)
                    .ok_or(Failure::NativeQuery)?,
                bottom: origin
                    .y
                    .checked_add(client.bottom)
                    .ok_or(Failure::NativeQuery)?,
            },
        };
        if dpi == 0
            || client.left != 0
            || client.top != 0
            || !valid_rect(result.window_rect)
            || !valid_rect(result.frame_rect)
            || !valid_rect(result.client_screen)
        {
            return Err(Failure::NativeQuery);
        }
        Ok(result)
    }
}

/// Read only this process's own HWND under existing per-monitor DPI awareness.
/// Two adjacent reads reject a mixed snapshot rather than retry/spin. This is not
/// atomic and does not establish geometry at an older compositor timestamp.
pub fn own_window_geometry(hwnd: usize) -> Result<Geometry, Failure> {
    if hwnd == 0 {
        return Err(Failure::NativeQuery);
    }
    let hwnd = HWND(hwnd as *mut c_void);
    let before = geometry_once(hwnd)?;
    let after = geometry_once(hwnd)?;
    if before != after {
        return Err(Failure::NativeQuery);
    }
    Ok(after)
}

fn explicit_outer_change(baseline: Geometry, evidence: WindowPosEvidence) -> bool {
    let moved = evidence.flags & SWP_NOMOVE.0 == 0
        && (evidence.x != baseline.window_rect.left || evidence.y != baseline.window_rect.top);
    let resized = evidence.flags & SWP_NOSIZE.0 == 0
        && (i64::from(evidence.width)
            != i64::from(baseline.window_rect.right) - i64::from(baseline.window_rect.left)
            || i64::from(evidence.height)
                != i64::from(baseline.window_rect.bottom) - i64::from(baseline.window_rect.top));
    moved || resized
}

impl History {
    fn fail(&mut self, failure: Failure) {
        if self.fatal.is_none() {
            self.fatal = Some(failure);
        }
    }

    /// Pure state machine, shared by the native callback and sequence tests.
    fn observe(
        &mut self,
        event: NativeEvent,
        geometry: Option<Geometry>,
        clock: QpcStamp,
        capacity: usize,
    ) {
        if let NativeEvent::WindowPosChanged(evidence) = event
            && explicit_outer_change(self.baseline, evidence)
        {
            self.fail(Failure::OuterChanged);
        }
        match event {
            NativeEvent::DpiChanged => self.fail(Failure::DpiChanged),
            NativeEvent::NcDestroy => self.fail(Failure::NativeDestroyed),
            _ if geometry.is_none() => self.fail(Failure::NativeQuery),
            _ => {}
        }
        let mut client_restored = false;
        if let Some(geometry) = geometry {
            if let NativeEvent::ClientSize { payload } = event {
                let width = i64::from(geometry.client_screen.right)
                    - i64::from(geometry.client_screen.left);
                let height = i64::from(geometry.client_screen.bottom)
                    - i64::from(geometry.client_screen.top);
                // WM_SIZE carries only the low 16 bits. A disagreement means the
                // current query cannot explain the synchronous message history.
                if payload != [(width as u32) & 0xffff, (height as u32) & 0xffff] {
                    self.fail(Failure::NativeSizeMismatch);
                }
            }
            if geometry.dpi != self.baseline.dpi {
                self.fail(Failure::DpiChanged);
            }
            if geometry.window_rect != self.baseline.window_rect
                || geometry.frame_rect != self.baseline.frame_rect
            {
                self.fail(Failure::OuterChanged);
            }
            if geometry.client_screen != self.baseline.client_screen {
                if !self.client_layout_pending {
                    self.client_transitions += 1;
                }
                self.client_layout_pending = true;
            } else if self.client_layout_pending {
                self.client_layout_pending = false;
                self.last_client_restore = clock;
                client_restored = true;
            }
        }
        if self.events.len() >= capacity {
            self.overflow += 1;
            self.fail(Failure::QueueOverflow);
        } else {
            self.events.push(Record {
                observed_at: clock,
                event,
                geometry,
                client_restored,
            });
        }
    }

    /// Pure history gate. False means pending recovery or a frame older than it;
    /// an incomplete/invalid native history is an error. Capture checks still apply.
    pub fn frame_after_restore(&self, frame_time_100ns: i64) -> Result<bool, Failure> {
        if let Some(failure) = self.fatal {
            return Err(failure);
        }
        Ok(!self.client_layout_pending
            && input_to_frame_ms(
                self.last_client_restore.ticks,
                self.last_client_restore.frequency,
                frame_time_100ns,
            )
            .is_ok())
    }
}

impl State {
    fn fail(&self, failure: Failure) {
        if self.fatal.get().is_none() {
            self.fatal.set(Some(failure));
        }
    }

    fn record(&self, event: NativeEvent) {
        // All OS queries precede the RefCell borrow, and forwarding follows it.
        // Nested WM_SIZE dispatched by DefWindowProc thus never sees a held borrow.
        let geometry = if matches!(event, NativeEvent::NcDestroy | NativeEvent::DpiChanged) {
            None
        } else {
            match own_window_geometry(self.hwnd.0 as usize) {
                Ok(value) => Some(value),
                Err(failure) => {
                    self.fail(failure);
                    None
                }
            }
        };
        let clock = match qpc_now() {
            Ok(value) => value,
            Err(_) => {
                self.fail(Failure::ClockQuery);
                return;
            }
        };
        let Ok(mut inner) = self.inner.try_borrow_mut() else {
            self.fail(Failure::ReentrantBorrow);
            return;
        };
        let capacity = inner.capacity;
        // Capacity was reserved before install; the state machine cannot grow
        // beyond it. No borrow survives the later DefSubclassProc call.
        inner.history.observe(event, geometry, clock, capacity);
        if let Some(failure) = inner.history.fatal {
            self.fail(failure);
        }
    }
}

fn release_installation_ref(state: &Rc<State>) {
    if state.installation_ref_live.replace(false) {
        // SAFETY: balances exactly the Rc::into_raw used for dwRefData. The caller
        // holds another Rc, including callback-local ownership during NCDESTROY.
        unsafe {
            drop(Rc::from_raw(Rc::as_ptr(state)));
        }
    }
}

fn detach(state: &Rc<State>) -> Result<(), Failure> {
    if !state.installation_ref_live.get() {
        return Ok(());
    }
    // SAFETY: query current thread; subclass helpers cannot cross UI threads.
    if unsafe { GetCurrentThreadId() } != state.ui_thread {
        return Err(Failure::WrongThread);
    }
    let pointer = Rc::as_ptr(state) as usize;
    // SAFETY: exact own HWND/procedure/ID tuple installed by arm(). No RefCell
    // borrow or mutex is held. FALSE retains the raw ownership to avoid UAF.
    if !unsafe { RemoveWindowSubclass(state.hwnd, Some(subclass_proc), pointer) }.as_bool() {
        return Err(Failure::DetachFailed);
    }
    release_installation_ref(state);
    Ok(())
}

unsafe extern "system" fn subclass_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    id: usize,
    data: usize,
) -> LRESULT {
    // SAFETY: arm() installed this exact callback/ID and one live raw Rc. All
    // invocations/removal occur on the same UI thread. A callback-local Rc keeps
    // state alive if a nested close drops the external guard during forwarding.
    let state = unsafe {
        let pointer = data as *const State;
        Rc::increment_strong_count(pointer);
        Rc::from_raw(pointer)
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if id != data || hwnd != state.hwnd || unsafe { GetCurrentThreadId() } != state.ui_thread {
            state.fail(Failure::WrongThread);
            return;
        }
        let event = match message {
            WM_WINDOWPOSCHANGED => {
                let pointer = lparam.0 as *const WINDOWPOS;
                if pointer.is_null() {
                    state.fail(Failure::MalformedWindowPos);
                    return;
                }
                // SAFETY: documented WM_WINDOWPOSCHANGED LPARAM is a live
                // WINDOWPOS for this call only. Copy scalars before forwarding;
                // never retain a pointer or inspect hwndInsertAfter/foreign state.
                let position = unsafe { std::ptr::read_unaligned(pointer) };
                if position.hwnd != hwnd {
                    state.fail(Failure::MalformedWindowPos);
                    return;
                }
                Some(NativeEvent::WindowPosChanged(WindowPosEvidence {
                    flags: position.flags.0,
                    x: position.x,
                    y: position.y,
                    width: position.cx,
                    height: position.cy,
                }))
            }
            WM_SIZE => Some(NativeEvent::ClientSize {
                payload: [(lparam.0 as u32 & 0xffff), (lparam.0 as u32 >> 16) & 0xffff],
            }),
            WM_DPICHANGED => Some(NativeEvent::DpiChanged),
            WM_NCDESTROY => Some(NativeEvent::NcDestroy),
            _ => None,
        };
        if let Some(event) = event {
            state.record(event);
        }
    }));
    if result.is_err() {
        state.fail(Failure::CallbackPanic);
    }
    if message == WM_NCDESTROY {
        // Standard terminal-message cleanup. The local Rc outlives forwarding,
        // including any nested final messages; no HWND survives NCDESTROY.
        let _ = detach(&state);
        release_installation_ref(&state);
    }
    // SAFETY: forward each untouched native message exactly once. Absolutely no
    // RefCell borrow/mutex survives this call; WM_WINDOWPOSCHANGED's default
    // handling may synchronously dispatch nested WM_SIZE before returning.
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

impl NativeGeometryGuard {
    /// Arm on the owned HWND's UI thread, after startup placement/calibration.
    /// A fresh guard starts a new sample; never rearm/reset an invalid sample.
    pub fn arm(hwnd: usize, event_limit: usize) -> Result<Self, String> {
        if !(1..=4096).contains(&event_limit) {
            return Err("invalid native history capacity".into());
        }
        let hwnd = HWND(hwnd as *mut c_void);
        let mut process_id = 0;
        // SAFETY: scalar identity only; refuse a foreign HWND or owner thread.
        let (thread_id, current_thread) = unsafe {
            (
                GetWindowThreadProcessId(hwnd, Some(&mut process_id)),
                GetCurrentThreadId(),
            )
        };
        if hwnd.0.is_null()
            || process_id != std::process::id()
            || thread_id == 0
            || thread_id != current_thread
        {
            return Err("native geometry guard requires its own HWND/UI thread".into());
        }
        let baseline =
            own_window_geometry(hwnd.0 as usize).map_err(|e| format!("native baseline: {e:?}"))?;
        let armed_at = qpc_now()?;
        let mut events = Vec::new();
        events
            .try_reserve_exact(event_limit)
            .map_err(|_| "native history allocation refused")?;
        let state = Rc::new(State {
            hwnd,
            ui_thread: thread_id,
            fatal: Cell::new(None),
            installation_ref_live: Cell::new(true),
            inner: RefCell::new(Inner {
                capacity: event_limit,
                history: History {
                    baseline,
                    armed_at,
                    last_client_restore: armed_at,
                    client_layout_pending: false,
                    client_transitions: 0,
                    overflow: 0,
                    fatal: None,
                    events,
                },
            }),
        });
        let pointer = Rc::into_raw(state.clone()) as usize;
        // SAFETY: exact UI-thread HWND, stable raw Rc and unique live allocation
        // address used as ID. On failure no callback owns the additional reference.
        if !unsafe { SetWindowSubclass(hwnd, Some(subclass_proc), pointer, pointer) }.as_bool() {
            release_installation_ref(&state);
            return Err("SetWindowSubclass failed".into());
        }
        let mut guard = Self { state };
        if own_window_geometry(hwnd.0 as usize).ok() != Some(baseline) {
            let _ = guard.stop();
            return Err("native geometry changed while arming".into());
        }
        Ok(guard)
    }

    /// Clone the bounded history on its HWND owner thread without resetting it.
    /// No native message forwarding occurs while the internal state is borrowed.
    pub fn history(&self) -> Result<History, String> {
        // No Windows call occurs while a RefCell borrow is held.
        // SAFETY: read-only scalar identity; Rc prevents safe cross-thread moves.
        if unsafe { GetCurrentThreadId() } != self.state.ui_thread {
            return Err("wrong UI thread".into());
        }
        let inner = self
            .state
            .inner
            .try_borrow()
            .map_err(|_| "native history is borrowed")?;
        let mut result = inner.history.clone();
        result.fatal = self.state.fatal.get();
        Ok(result)
    }

    /// Only a supplemental gate: decoder/ALL generation/visibility/occlusion and
    /// every existing capture failure/overflow check remain mandatory.
    pub fn frame_after_restore(&self, frame_time_100ns: i64) -> Result<bool, String> {
        let history = self.history()?;
        if !history
            .frame_after_restore(frame_time_100ns)
            .map_err(|failure| format!("native geometry invalid: {failure:?}"))?
        {
            return Ok(false);
        }
        let current = own_window_geometry(self.state.hwnd.0 as usize)
            .map_err(|e| format!("native frame gate: {e:?}"))?;
        if current != history.baseline {
            return Err("native geometry no longer matches armed baseline".into());
        }
        // Read-only OS queries can still reenter native message dispatch. Re-read
        // history after querying so a newer restore stamp cannot be missed by an
        // otherwise matching current snapshot.
        self.history()?
            .frame_after_restore(frame_time_100ns)
            .map_err(|failure| format!("native geometry invalid after query: {failure:?}"))
    }

    /// Call before hide/close/NCDESTROY, then preserve the returned history in the
    /// report. Failure is invalid; Drop must never claim successful detachment.
    pub fn stop(&mut self) -> Result<History, String> {
        if let Err(failure) = detach(&self.state) {
            self.state.fail(failure);
            return Err(format!("native geometry stop: {failure:?}"));
        }
        self.history()
    }
}

impl Drop for NativeGeometryGuard {
    fn drop(&mut self) {
        if let Err(failure) = detach(&self.state) {
            self.state.fail(failure);
            // Keep installation's raw Rc on failed removal: bounded retention
            // until NCDESTROY is preferable to freeing future callback state.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clock(ticks: i64) -> QpcStamp {
        QpcStamp {
            ticks,
            frequency: 10_000_000,
        }
    }
    fn baseline() -> Geometry {
        Geometry {
            window_rect: PixelRect {
                left: 10,
                top: 10,
                right: 1010,
                bottom: 810,
            },
            frame_rect: PixelRect {
                left: 18,
                top: 10,
                right: 1002,
                bottom: 802,
            },
            client_screen: PixelRect {
                left: 18,
                top: 110,
                right: 1002,
                bottom: 802,
            },
            dpi: 192,
        }
    }
    fn history(capacity: usize) -> History {
        History {
            baseline: baseline(),
            armed_at: clock(100),
            last_client_restore: clock(100),
            client_layout_pending: false,
            client_transitions: 0,
            overflow: 0,
            fatal: None,
            events: Vec::with_capacity(capacity),
        }
    }
    fn size() -> NativeEvent {
        NativeEvent::ClientSize {
            payload: [984, 692],
        }
    }

    #[test]
    fn outer_one_pixel_then_restored_remains_fatal_in_native_history() {
        let mut history = history(8);
        let evidence = WindowPosEvidence {
            flags: SWP_NOMOVE.0,
            x: -999,
            y: -999,
            width: 1001,
            height: 800,
        };
        // Explicit WINDOWPOS evidence cannot be erased even if the current query
        // already reads the baseline (the exact late buffered-event problem).
        history.observe(
            NativeEvent::WindowPosChanged(evidence),
            Some(baseline()),
            clock(110),
            8,
        );
        history.observe(size(), Some(baseline()), clock(120), 8);
        assert_eq!(history.fatal, Some(Failure::OuterChanged));
        assert_eq!(history.frame_after_restore(150), Err(Failure::OuterChanged));
    }

    #[test]
    fn client_plus_39_must_restore_and_frame_must_not_predate_restore() {
        let mut history = history(8);
        let mut changed = baseline();
        changed.client_screen.top -= 39;
        history.observe(
            NativeEvent::ClientSize {
                payload: [984, 731],
            },
            Some(changed),
            clock(110),
            8,
        );
        assert_eq!(history.fatal, None);
        assert_eq!(history.frame_after_restore(999), Ok(false));
        history.observe(size(), Some(baseline()), clock(120), 8);
        assert_eq!(history.client_transitions, 1);
        assert_eq!(history.last_client_restore.ticks, 120);
        assert!(history.events.last().unwrap().client_restored);
        assert_eq!(history.frame_after_restore(119), Ok(false));
        assert_eq!(history.frame_after_restore(120), Ok(true));
    }

    #[test]
    fn dpi_overflow_missing_geometry_and_destroy_are_sticky_failures() {
        for (event, geometry, expected) in [
            (NativeEvent::DpiChanged, None, Failure::DpiChanged),
            (size(), None, Failure::NativeQuery),
            (NativeEvent::NcDestroy, None, Failure::NativeDestroyed),
        ] {
            let mut history = history(8);
            history.observe(event, geometry, clock(110), 8);
            history.observe(size(), Some(baseline()), clock(120), 8);
            assert_eq!(history.frame_after_restore(150), Err(expected));
        }
        let mut history = history(1);
        history.observe(size(), Some(baseline()), clock(110), 1);
        history.observe(size(), Some(baseline()), clock(120), 1);
        assert_eq!(history.events.len(), 1);
        assert_eq!(history.overflow, 1);
        assert_eq!(
            history.frame_after_restore(150),
            Err(Failure::QueueOverflow)
        );
    }

    #[test]
    fn windowpos_ignored_fields_never_create_a_false_outer_change() {
        let ignored = WindowPosEvidence {
            flags: SWP_NOMOVE.0 | SWP_NOSIZE.0,
            x: i32::MIN,
            y: i32::MAX,
            width: -1,
            height: -1,
        };
        assert!(!explicit_outer_change(baseline(), ignored));
        let actual_move = WindowPosEvidence {
            flags: SWP_NOSIZE.0,
            x: 11,
            y: 10,
            ..ignored
        };
        assert!(explicit_outer_change(baseline(), actual_move));
    }

    #[test]
    fn contradictory_native_size_cannot_disappear_into_a_restored_snapshot() {
        let mut history = history(8);
        history.observe(
            NativeEvent::ClientSize {
                payload: [984, 731],
            },
            Some(baseline()),
            clock(110),
            8,
        );
        history.observe(size(), Some(baseline()), clock(120), 8);
        assert_eq!(history.fatal, Some(Failure::NativeSizeMismatch));
        assert_eq!(
            history.frame_after_restore(150),
            Err(Failure::NativeSizeMismatch)
        );
    }

    #[test]
    fn later_recovery_requires_rechecking_latest_history() {
        let mut history = history(8);
        assert_eq!(history.frame_after_restore(150), Ok(true));
        let mut changed = baseline();
        changed.client_screen.top -= 39;
        history.observe(
            NativeEvent::ClientSize {
                payload: [984, 731],
            },
            Some(changed),
            clock(160),
            8,
        );
        history.observe(size(), Some(baseline()), clock(170), 8);
        assert_eq!(history.frame_after_restore(150), Ok(false));
        assert_eq!(history.frame_after_restore(170), Ok(true));
    }
}

// Suggested isolated native validation after explicit compile ownership:
// 1. Own window client-only SetMenu/DrawMenuBar transition: no outer failure,
//    pending true then restore stamp; pre-restore WGC frame rejected by gate.
// 2. Real outer resize then restore before winit callbacks: explicit WINDOWPOS
//    history still marks OuterChanged permanently; late baseline payload cannot
//    clear it. SWP_NOMOVE/NOSIZE garbage fields are ignored exactly as documented.
// 3. WM_DPICHANGED, queue overflow, failed API/clock, destroy, failed detach all
//    remain fatal. stop() before normal hide avoids postmeasurement invalidation.
// 4. Keep the existing armed RejectedFrame policy; do not turn GPU/ROI/CRC errors
//    into a client-layout exception. Native tests must run only with an explicit owned-window validation.
