// crates/audiolib-windows/src/gui_capture.rs
//! Opt-in GUI diagnostics: the application's own HWND -> compositor-frame marker.
//! No input injection, UI callback, full-frame storage, database or audio access.
//! Windows API projection is pinned to windows 0.62.2.
//!
//! This observes a compositor frame, not panel scanout. The first observed matching
//! marker is a conservative completion upper bound: an earlier frame may be lost.
//! Visibility snapshots do not prove absence of occluding windows or continuous
//! foreground state. The integration MUST independently invalidate focus/resize,
//! input-desktop changes, occlusion and stale generations throughout each sample.

#[cfg(windows)]
use std::sync::mpsc;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc::{Receiver, SyncSender},
};
use std::thread::JoinHandle;

/// Sixteen-byte marker, displayed MSB-first as 64 columns by two rows.
pub const MARKER_BITS: usize = 128;
/// The fixed barcode width in cells; not its physical pixel width.
pub const MARKER_COLUMNS: u32 = 64;
/// The fixed barcode height in cells.
pub const MARKER_ROWS: u32 = 2;
/// Input accepted, final models not published yet.
pub const PHASE_PENDING: u8 = 0;
/// The result-page model is actually assigned on the UI thread.
pub const PHASE_PAGE: u8 = 1;
/// Page and matching directory-tree models are both assigned on the UI thread.
pub const PHASE_ALL: u8 = 2;

/// Native input-handler timestamp with a known conversion to WGC's QPC domain.
#[derive(Clone, Copy, Debug)]
pub struct QpcStamp {
    pub ticks: i64,
    pub frequency: i64,
}

/// Read only the native monotonic clock; safe to call on the existing UI thread.
pub fn qpc_now() -> Result<QpcStamp, String> {
    #[cfg(windows)]
    {
        native::qpc_now()
    }
    #[cfg(not(windows))]
    {
        Err("native QPC is available only on Windows".into())
    }
}

/// Current-process resource observation only. The peak working set covers the
/// whole process lifetime, including startup. Private usage is the current sample,
/// not a native peak, resident-only figure or whole-machine memory measurement.
#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct ProcessMemoryUsage {
    /// OS-reported peak resident working set for this process's entire lifetime.
    pub peak_working_set_bytes: u64,
    /// OS-reported current resident working set at this sample.
    pub current_working_set_bytes: u64,
    /// Current private committed usage; callers may separately aggregate samples.
    pub private_usage_bytes: u64,
}

/// Read the current process's counters without reading process memory contents.
/// Call outside audio callbacks; periodic UI diagnostics must label sample maxima
/// separately from the OS-reported process-lifetime working-set peak.
pub fn process_memory_usage() -> Result<ProcessMemoryUsage, String> {
    #[cfg(windows)]
    {
        native::process_memory_usage()
    }
    #[cfg(not(windows))]
    {
        Err("native process memory counters require Windows".into())
    }
}

fn crc16(bytes: &[u8]) -> u16 {
    let mut crc = 0xffff_u16;
    for byte in bytes {
        crc ^= u16::from(*byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// Make a marker with no query text, file names or Metadata. A per-run nonce and
/// monotonic request generation prevent stale frames from completing a new sample.
/// Draw only un-antialiased black/white cell interiors, without text/glyph OCR.
pub fn encode_marker(nonce: u32, generation: u64, phase: u8) -> Result<[u8; 16], String> {
    if phase > PHASE_ALL {
        return Err("invalid completion phase".into());
    }
    let mut token = [0_u8; 16];
    token[0] = 0xa7;
    token[1] = 0x10 | phase;
    token[2..6].copy_from_slice(&nonce.to_be_bytes());
    token[6..14].copy_from_slice(&generation.to_be_bytes());
    let crc = crc16(&token[..14]).to_be_bytes();
    token[14..].copy_from_slice(&crc);
    Ok(token)
}

/// Bool model for the Slint diagnostic marker: index / 64 gives its row.
pub fn marker_bits(token: &[u8; 16]) -> [bool; MARKER_BITS] {
    std::array::from_fn(|index| token[index / 8] & (0x80 >> (index % 8)) != 0)
}

#[cfg(any(windows, test))]
fn decode_marker(token: [u8; 16]) -> Result<(u32, u64, u8), String> {
    if token[0] != 0xa7 || token[1] >> 4 != 1 || token[1] & 0xf > PHASE_ALL {
        return Err("marker magic/version/phase invalid".into());
    }
    if crc16(&token[..14]) != u16::from_be_bytes([token[14], token[15]]) {
        return Err("marker CRC invalid".into());
    }
    let nonce = u32::from_be_bytes(token[2..6].try_into().map_err(|_| "nonce")?);
    let generation = u64::from_be_bytes(token[6..14].try_into().map_err(|_| "generation")?);
    Ok((nonce, generation, token[1] & 0xf))
}

#[cfg(any(windows, test))]
fn decode_rows<'a>(
    roi: Roi,
    mut row: impl FnMut(usize) -> &'a [u8],
) -> Result<([u8; 16], u64), String> {
    let (width, height) = roi.dimensions()?;
    let pixel_bytes = width as usize * 4;
    let mut digest = 0xcbf29ce484222325_u64;
    for y in 0..height as usize {
        let pixels = row(y).get(..pixel_bytes).ok_or("ROI row is truncated")?;
        for byte in pixels {
            digest ^= u64::from(*byte);
            digest = digest.wrapping_mul(0x100000001b3);
        }
    }
    let mut token = [0_u8; 16];
    for bit in 0..MARKER_BITS {
        let x = ((bit as u32 % MARKER_COLUMNS) * roi.cell_width + roi.cell_width / 2) as usize;
        let y = ((bit as u32 / MARKER_COLUMNS) * roi.cell_height + roi.cell_height / 2) as usize;
        let pixels = row(y);
        let rgb = pixels
            .get(x * 4..x * 4 + 3)
            .ok_or("ROI sample is truncated")?;
        if rgb.iter().all(|channel| *channel >= 224) {
            token[bit / 8] |= 0x80 >> (bit % 8);
        } else if !rgb.iter().all(|channel| *channel <= 31) {
            return Err("marker has ambiguous color: check ROI/DPI/SDR/cell alignment".into());
        }
    }
    Ok((token, digest))
}

/// Region is expressed in physical pixels of WGC ContentSize, NOT client logical
/// coordinates. Integration must calibrate client/nonclient offset and DPI before
/// arming input, and invalidate the sample rather than reuse it after a resize.
#[derive(Clone, Copy, Debug)]
pub struct Roi {
    pub x: u32,
    pub y: u32,
    pub cell_width: u32,
    pub cell_height: u32,
}

/// Maximum retained top-level occluder summaries. Counts/qualification still
/// include every intersecting candidate in the bounded native z-order walk.
pub const MAX_OCCLUDER_CANDIDATES: usize = 8;
/// Retained class-name capacity in UTF-16 units; never a window title.
pub const OCCLUDER_CLASS_UNITS: usize = 96;
#[cfg(any(windows, test))]
const MAX_DESKTOP_MONITORS: usize = 64;
#[cfg(any(windows, test))]
const MAX_COVERAGE_REMAINDERS: usize = 4096;

/// Half-open physical pixel rectangle. Negative desktop coordinates are valid.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct PixelRect {
    /// Inclusive left coordinate.
    pub left: i32,
    /// Inclusive top coordinate.
    pub top: i32,
    /// Exclusive right coordinate.
    pub right: i32,
    /// Exclusive bottom coordinate.
    pub bottom: i32,
}

impl PixelRect {
    #[cfg(any(windows, test))]
    fn area(self) -> Option<u64> {
        let width = u64::try_from(i64::from(self.right) - i64::from(self.left)).ok()?;
        let height = u64::try_from(i64::from(self.bottom) - i64::from(self.top)).ok()?;
        (width > 0 && height > 0).then_some(width.checked_mul(height)?)
    }

    #[cfg(any(windows, test))]
    fn intersection(self, other: Self) -> Option<Self> {
        let intersection = Self {
            left: self.left.max(other.left),
            top: self.top.max(other.top),
            right: self.right.min(other.right),
            bottom: self.bottom.min(other.bottom),
        };
        intersection.area().map(|_| intersection)
    }
}

/// Read-only placement advice preserving the current DWM-frame dimensions.
/// Applying `delta` to the public Slint physical outer position translates both
/// rectangles equally without assuming an invisible resize-border thickness.
#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct WindowWorkAreaAdjustment {
    /// Minimal physical translation needed for the workarea and optional margin.
    pub delta: [i32; 2],
    /// Own DWM extended-frame rectangle before any caller-applied translation.
    pub frame_before: PixelRect,
    /// Actual workarea of the nearest native monitor, not virtual desktop bounds.
    pub work_area: PixelRect,
    /// Native monitor rectangle used to validate the selected workarea.
    pub monitor_bounds: PixelRect,
}

/// Inspect only the caller process's HWND and suggest a physical translation
/// into its nearest monitor workarea. Keeps the frame size, leaves up to eight
/// pixels per side where space permits, and never moves/resizes/focuses a window.
/// Errors retain rectangle/dimension evidence when the selected workarea cannot
/// fit the frame. The caller must still verify strict coverage after moving.
pub fn window_work_area_adjustment(own_hwnd: usize) -> Result<WindowWorkAreaAdjustment, String> {
    #[cfg(windows)]
    {
        native::window_work_area_adjustment(own_hwnd)
    }
    #[cfg(not(windows))]
    {
        let _ = own_hwnd;
        Err("native window workarea placement requires Windows".into())
    }
}

#[cfg(any(windows, test))]
fn work_area_adjustment(
    frame: PixelRect,
    work_area: PixelRect,
    monitor_bounds: PixelRect,
) -> Result<WindowWorkAreaAdjustment, String> {
    let frame_width = i64::from(frame.right) - i64::from(frame.left);
    let frame_height = i64::from(frame.bottom) - i64::from(frame.top);
    let work_width = i64::from(work_area.right) - i64::from(work_area.left);
    let work_height = i64::from(work_area.bottom) - i64::from(work_area.top);
    let evidence = || {
        format!(
            "frame_before={frame:?}, frame_size=[{frame_width},{frame_height}], work_area={work_area:?}, work_size=[{work_width},{work_height}], monitor_bounds={monitor_bounds:?}"
        )
    };
    if frame.area().is_none()
        || work_area.area().is_none()
        || monitor_bounds.area().is_none()
        || monitor_bounds.intersection(work_area) != Some(work_area)
    {
        return Err(format!(
            "invalid native placement rectangles: {}",
            evidence()
        ));
    }
    if frame_width > work_width || frame_height > work_height {
        return Err(format!(
            "unchanged window frame cannot fit selected monitor workarea: {}",
            evidence()
        ));
    }
    let margin_x = 8_i64.min((work_width - frame_width) / 2);
    let margin_y = 8_i64.min((work_height - frame_height) / 2);
    let left = i64::from(frame.left).clamp(
        i64::from(work_area.left) + margin_x,
        i64::from(work_area.right) - frame_width - margin_x,
    );
    let top = i64::from(frame.top).clamp(
        i64::from(work_area.top) + margin_y,
        i64::from(work_area.bottom) - frame_height - margin_y,
    );
    let dx = i32::try_from(left - i64::from(frame.left)).map_err(|_| {
        format!(
            "horizontal placement delta exceeds native range: {}",
            evidence()
        )
    })?;
    let dy = i32::try_from(top - i64::from(frame.top)).map_err(|_| {
        format!(
            "vertical placement delta exceeds native range: {}",
            evidence()
        )
    })?;
    Ok(WindowWorkAreaAdjustment {
        delta: [dx, dy],
        frame_before: frame,
        work_area,
        monitor_bounds,
    })
}

/// Exact rectangle-union coverage of the own DWM frame and client. A virtual
/// desktop bounding rectangle is insufficient because monitor layouts have gaps.
/// Incomplete/invalid enumeration or an arithmetic/allocation limit rejects.
#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
pub struct DesktopCoverage {
    /// Successfully read monitor/workarea pairs, capped at 64.
    pub monitor_count: u32,
    /// Native enumeration reached its end without errors or the monitor cap.
    pub enumeration_complete: bool,
    /// All four bounded rectangle-subtraction calculations completed.
    pub calculation_complete: bool,
    /// Physical pixels in the own DWM extended-frame rectangle.
    pub frame_pixels: u64,
    /// Physical pixels in the own client rectangle.
    pub client_pixels: u64,
    /// DWM-frame pixels outside the union of native monitor rectangles.
    pub frame_uncovered_monitor_pixels: u64,
    /// Client pixels outside the union of native monitor rectangles.
    pub client_uncovered_monitor_pixels: u64,
    /// DWM-frame pixels outside the union of native workareas.
    pub frame_uncovered_work_area_pixels: u64,
    /// Client pixels outside the union of native workareas.
    pub client_uncovered_work_area_pixels: u64,
}

impl DesktopCoverage {
    /// Require complete geometry and the whole own frame/client within actual
    /// workareas. This does not prove physical scanout or continuous visibility.
    pub fn necessary_checks_pass(self) -> bool {
        self.enumeration_complete
            && self.calculation_complete
            && self.monitor_count > 0
            && self.frame_pixels > 0
            && self.client_pixels > 0
            && self.frame_uncovered_monitor_pixels == 0
            && self.client_uncovered_monitor_pixels == 0
            && self.frame_uncovered_work_area_pixels == 0
            && self.client_uncovered_work_area_pixels == 0
    }
}

/// Fixed-size native candidate metadata. The intersection is relative to the
/// own client origin; foreign window titles, pixels and command lines are absent.
#[derive(Clone, Copy, Debug)]
pub struct OccluderCandidate {
    /// Native process ID; zero means the read failed.
    pub process_id: u32,
    /// OS class-name prefix, containing exactly `class_name_units` UTF-16 units.
    pub class_name_utf16: [u16; OCCLUDER_CLASS_UNITS],
    /// Number of retained class units; never larger than the fixed capacity.
    pub class_name_units: u16,
    /// FNV-1a of the full successfully queried class name, not content/security.
    pub class_name_hash: u64,
    /// Whether the class name exceeds the retained prefix capacity.
    pub class_name_truncated: bool,
    /// PID and full class query both succeeded at this point-in-time snapshot.
    pub identity_read_complete: bool,
    /// Exact pre-existing pointer-policy match; no new exception is introduced.
    pub recognized_pointer: bool,
    /// Rectangle overlapping the own client, relative to its top-left.
    pub client_intersection: PixelRect,
}

impl OccluderCandidate {
    const EMPTY: Self = Self {
        process_id: 0,
        class_name_utf16: [0; OCCLUDER_CLASS_UNITS],
        class_name_units: 0,
        class_name_hash: 0,
        class_name_truncated: false,
        identity_read_complete: false,
        recognized_pointer: false,
        client_intersection: PixelRect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        },
    };

    /// Allocate a report only when serializing diagnostics, not on the capture
    /// callback. A cut surrogate is represented lossily and remains flagged.
    pub fn report(self) -> OccluderCandidateReport {
        let length = usize::from(self.class_name_units).min(OCCLUDER_CLASS_UNITS);
        OccluderCandidateReport {
            process_id: self.process_id,
            class_name: String::from_utf16_lossy(&self.class_name_utf16[..length]),
            class_name_hash: self.class_name_hash,
            class_name_truncated: self.class_name_truncated,
            identity_read_complete: self.identity_read_complete,
            recognized_pointer: self.recognized_pointer,
            client_intersection: self.client_intersection,
        }
    }
}

impl Default for OccluderCandidate {
    fn default() -> Self {
        Self::EMPTY
    }
}

/// Owned, serializable projection of the bounded native class/PID metadata.
#[derive(Clone, Debug, serde::Serialize)]
pub struct OccluderCandidateReport {
    /// Native PID only, with no executable path or command line.
    pub process_id: u32,
    /// OS class name, never a title; possibly truncated as explicitly reported.
    pub class_name: String,
    /// Identity checksum of the full queried OS class string.
    pub class_name_hash: u64,
    /// Retained class string does not contain the whole queried name.
    pub class_name_truncated: bool,
    /// PID/full class queries succeeded.
    pub identity_read_complete: bool,
    /// Matched the explicitly selected existing Sky pointer policy.
    pub recognized_pointer: bool,
    /// Own-client-relative intersection, without the foreign window extent.
    pub client_intersection: PixelRect,
}

#[cfg(any(windows, test))]
fn subtract_rectangle(rectangle: PixelRect, cover: PixelRect) -> ([PixelRect; 4], usize) {
    let mut pieces = [PixelRect::default(); 4];
    let Some(overlap) = rectangle.intersection(cover) else {
        pieces[0] = rectangle;
        return (pieces, 1);
    };
    let strips = [
        PixelRect {
            bottom: overlap.top,
            ..rectangle
        },
        PixelRect {
            top: overlap.bottom,
            ..rectangle
        },
        PixelRect {
            right: overlap.left,
            top: overlap.top,
            bottom: overlap.bottom,
            ..rectangle
        },
        PixelRect {
            left: overlap.right,
            top: overlap.top,
            bottom: overlap.bottom,
            ..rectangle
        },
    ];
    let mut count = 0;
    for strip in strips {
        if strip.area().is_some() {
            pieces[count] = strip;
            count += 1;
        }
    }
    (pieces, count)
}

#[cfg(any(windows, test))]
fn uncovered_pixels(rectangle: PixelRect, covers: &[PixelRect], limit: usize) -> Option<u64> {
    rectangle.area()?;
    if limit == 0 || covers.iter().any(|cover| cover.area().is_none()) {
        return None;
    }
    let mut remaining = Vec::new();
    remaining.try_reserve(1).ok()?;
    remaining.push(rectangle);
    for cover in covers {
        let mut next = Vec::new();
        for piece in remaining {
            let (strips, count) = subtract_rectangle(piece, *cover);
            if next.len().checked_add(count)? > limit {
                return None;
            }
            next.try_reserve(count).ok()?;
            next.extend_from_slice(&strips[..count]);
        }
        remaining = next;
        if remaining.is_empty() {
            break;
        }
    }
    remaining
        .into_iter()
        .try_fold(0_u64, |sum, rectangle| sum.checked_add(rectangle.area()?))
}

#[cfg(any(windows, test))]
fn measure_desktop_coverage(
    frame: PixelRect,
    client: PixelRect,
    monitors: &[PixelRect],
    workareas: &[PixelRect],
    enumeration_complete: bool,
) -> DesktopCoverage {
    let frame_pixels = frame.area().unwrap_or(0);
    let client_pixels = client.area().unwrap_or(0);
    let mut result = DesktopCoverage {
        monitor_count: monitors.len().min(MAX_DESKTOP_MONITORS) as u32,
        enumeration_complete,
        frame_pixels,
        client_pixels,
        // Unknown coverage is conservatively the whole nonempty own rectangle.
        frame_uncovered_monitor_pixels: frame_pixels,
        client_uncovered_monitor_pixels: client_pixels,
        frame_uncovered_work_area_pixels: frame_pixels,
        client_uncovered_work_area_pixels: client_pixels,
        ..Default::default()
    };
    if !enumeration_complete
        || monitors.is_empty()
        || monitors.len() > MAX_DESKTOP_MONITORS
        || monitors.len() != workareas.len()
        || frame_pixels == 0
        || client_pixels == 0
        || monitors.iter().zip(workareas).any(|(monitor, work)| {
            monitor.area().is_none()
                || work.area().is_none()
                || monitor.intersection(*work) != Some(*work)
        })
    {
        return result;
    }
    if let (Some(frame_monitor), Some(client_monitor), Some(frame_work), Some(client_work)) = (
        uncovered_pixels(frame, monitors, MAX_COVERAGE_REMAINDERS),
        uncovered_pixels(client, monitors, MAX_COVERAGE_REMAINDERS),
        uncovered_pixels(frame, workareas, MAX_COVERAGE_REMAINDERS),
        uncovered_pixels(client, workareas, MAX_COVERAGE_REMAINDERS),
    ) {
        result.calculation_complete = true;
        result.frame_uncovered_monitor_pixels = frame_monitor;
        result.client_uncovered_monitor_pixels = client_monitor;
        result.frame_uncovered_work_area_pixels = frame_work;
        result.client_uncovered_work_area_pixels = client_work;
    }
    result
}

/// Read-only native geometry and conservative top-level occlusion snapshot.
/// No titles, other-process pixels or application data are read. An explicitly
/// selected pointer policy queries its process image path only for identity.
#[derive(Clone, Copy, Debug)]
pub struct WindowInspection {
    /// Point-in-time native window and desktop eligibility.
    pub visibility: Visibility,
    /// Physical client dimensions, excluding native decorations.
    pub client_size: [u32; 2],
    /// Client top-left minus DWM extended-frame top-left, in physical pixels.
    pub client_origin_in_capture: [u32; 2],
    /// DWM extended-frame extent; compare against WGC Ready/ContentSize exactly.
    pub expected_capture_size: [u32; 2],
    /// Physical monitor/workarea union coverage, including desktop-layout gaps.
    pub desktop_coverage: DesktopCoverage,
    /// All visible, noniconic, noncloaked preceding windows intersecting the
    /// client rectangle, including explicitly recognized pointer adornments.
    pub raw_intersecting_windows: u32,
    /// Exact matches to an explicitly pinned Sky process and pointer signature.
    /// This exception is not proof of transparency or absence of all covering.
    pub recognized_cursor_overlays: u32,
    /// Intersecting application/unknown windows after the explicit pointer
    /// exception. Other transparent/layered windows conservatively reject.
    pub potential_occluders: u32,
    /// Whether the bounded native z-order walk reached this exact own HWND.
    pub occlusion_scan_complete: bool,
    /// At most eight metadata-only candidates; qualification counts all windows.
    pub occluder_candidates: [OccluderCandidate; MAX_OCCLUDER_CANDIDATES],
    /// Number of initialized entries in the fixed candidate array.
    pub occluder_candidate_count: u32,
    /// Candidates not retained because the summary array was full.
    pub occluder_candidates_overflow: u32,
}

impl WindowInspection {
    /// Necessary snapshot checks; integration must still monitor the entire
    /// sample for focus, resize, source, desktop and occlusion transitions.
    pub fn necessary_checks_pass(self) -> bool {
        self.visibility.necessary_checks_pass()
            && self.desktop_coverage.necessary_checks_pass()
            && self.occlusion_scan_complete
            && self.potential_occluders == 0
    }
}

/// Calibratable physical marker coordinates plus the geometry they came from.
#[derive(Clone, Copy, Debug)]
pub struct MarkerGeometry {
    /// ROI in physical WGC coordinates, including client/nonclient translation.
    pub roi: Roi,
    /// Exact native geometry at translation time.
    pub inspection: WindowInspection,
}

/// Inspect only geometry/eligibility; never focuses, moves or redraws a window.
pub fn inspect_window(own_hwnd: usize) -> Result<WindowInspection, String> {
    inspect_window_with_pointer_policy(own_hwnd, None)
}

/// Inspect with an explicit, narrowly pinned automation-pointer exception.
/// The default `inspect_window` remains strict. Recognized pointer adornments
/// are reported separately and never establish transparency/absolute visibility.
pub fn inspect_window_with_pointer_policy(
    own_hwnd: usize,
    policy: Option<&PointerOverlayPolicy>,
) -> Result<WindowInspection, String> {
    #[cfg(windows)]
    {
        native::inspect_window(own_hwnd, policy)
    }
    #[cfg(not(windows))]
    {
        let _ = (own_hwnd, policy);
        Err("native window inspection requires Windows".into())
    }
}

const SKY_IMAGE_SUFFIX: &str = r"\node_modules\@oai\sky\bin\windows\codex-computer-use.exe";

#[derive(Clone, PartialEq, Eq)]
struct PointerProcessIdentity {
    process_id: u32,
    creation_time_100ns: u64,
    canonical_image: String,
}

/// Explicit diagnostic exception for one already running Sky pointer process.
/// Private identity binds PID, process creation time and canonical image path;
/// a recycled PID, read failure or changed window signature cannot match.
/// The caller must report the exception and must not claim absolute no-covering.
#[derive(Clone)]
pub struct PointerOverlayPolicy {
    identity: PointerProcessIdentity,
}

/// Redacted policy evidence: the image path itself is never exported.
/// The FNV-1a path hash is an identity checksum, not a security signature.
#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct PointerOverlayReport {
    /// Explicitly authorized running process, not a guessed image name.
    pub process_id: u32,
    /// Native creation FILETIME, disambiguating recycled process IDs.
    pub creation_time_100ns: u64,
    /// Checksum of the complete normalized canonical path, not executable bytes.
    pub image_path_hash: u64,
    /// Exact required image path suffix, containing no user profile path.
    pub verified_image_suffix: &'static str,
}

impl PointerOverlayPolicy {
    /// Pin an explicitly selected running Sky process using read-only process
    /// metadata. Does not activate/control it or read any process memory/pixels.
    pub fn for_sky_process(process_id: u32) -> Result<Self, String> {
        #[cfg(windows)]
        {
            Self::from_identity(native::pointer_process_identity(process_id)?)
        }
        #[cfg(not(windows))]
        {
            let _ = process_id;
            Err("Sky pointer process identity requires Windows".into())
        }
    }

    #[cfg(any(windows, test))]
    fn from_identity(identity: PointerProcessIdentity) -> Result<Self, String> {
        if identity.process_id == 0
            || identity.creation_time_100ns == 0
            || !identity.canonical_image.ends_with(SKY_IMAGE_SUFFIX)
        {
            return Err(
                "pointer exception requires a running process with the exact Sky image suffix"
                    .into(),
            );
        }
        Ok(Self { identity })
    }

    /// Return only owned/scalar evidence and the public required suffix.
    pub fn report(&self) -> PointerOverlayReport {
        let mut hash = 0xcbf29ce484222325_u64;
        for byte in self.identity.canonical_image.bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        PointerOverlayReport {
            process_id: self.identity.process_id,
            creation_time_100ns: self.identity.creation_time_100ns,
            image_path_hash: hash,
            verified_image_suffix: SKY_IMAGE_SUFFIX,
        }
    }
}

#[cfg(any(windows, test))]
#[derive(Clone)]
struct PointerOverlayAttributes {
    process_id: u32,
    class_name: String,
    style: u32,
    extended_style: u32,
    alpha: u8,
    color_key: u32,
    layered_flags: u32,
}

#[cfg(any(windows, test))]
fn is_recognized_pointer_overlay(
    policy: Option<&PointerOverlayPolicy>,
    identity: Option<&PointerProcessIdentity>,
    attributes: Option<&PointerOverlayAttributes>,
) -> bool {
    let (Some(policy), Some(identity), Some(attributes)) = (policy, identity, attributes) else {
        return false;
    };
    policy.identity == *identity
        && attributes.process_id == identity.process_id
        && attributes.class_name == "CodexComputerUseCursorOverlay"
        && attributes.style == 0x9400_0000
        && attributes.extended_style == 0x0808_00a8
        && attributes.alpha == 255
        && attributes.color_key == 0
        && attributes.layered_flags == 3
}

fn aligned_pixel(value: f64) -> Result<u32, String> {
    if !value.is_finite()
        || value < 0.0
        || value > u32::MAX as f64
        || (value - value.round()).abs() > 0.001
    {
        return Err("marker geometry is not aligned to physical pixels".into());
    }
    Ok(value.round() as u32)
}

fn translated_roi(
    inspection: WindowInspection,
    logical_left: f64,
    logical_top: f64,
    logical_cell_width: f64,
    logical_cell_height: f64,
    scale: f64,
) -> Result<MarkerGeometry, String> {
    if !scale.is_finite() || scale <= 0.0 {
        return Err("invalid marker scale factor".into());
    }
    let client_x = aligned_pixel(logical_left * scale)?;
    let client_y = aligned_pixel(logical_top * scale)?;
    let roi = Roi {
        x: client_x
            .checked_add(inspection.client_origin_in_capture[0])
            .ok_or("marker x overflow")?,
        y: client_y
            .checked_add(inspection.client_origin_in_capture[1])
            .ok_or("marker y overflow")?,
        cell_width: aligned_pixel(logical_cell_width * scale)?,
        cell_height: aligned_pixel(logical_cell_height * scale)?,
    };
    let (width, height) = roi.dimensions()?;
    if client_x
        .checked_add(width)
        .is_none_or(|right| right > inspection.client_size[0])
        || client_y
            .checked_add(height)
            .is_none_or(|bottom| bottom > inspection.client_size[1])
        || roi
            .x
            .checked_add(width)
            .is_none_or(|right| right > inspection.expected_capture_size[0])
        || roi
            .y
            .checked_add(height)
            .is_none_or(|bottom| bottom > inspection.expected_capture_size[1])
    {
        return Err("marker falls outside native client/capture bounds".into());
    }
    Ok(MarkerGeometry { roi, inspection })
}

/// Translate the diagnostic Slint slot through ClientToScreen and DWM's extended
/// frame bounds. Fractional physical cell geometry is rejected rather than
/// guessed. A matching initial decoded WGC marker remains mandatory calibration.
pub fn client_marker_roi(
    own_hwnd: usize,
    logical_left: f64,
    logical_top: f64,
    logical_cell_width: f64,
    logical_cell_height: f64,
    scale_factor: f64,
) -> Result<MarkerGeometry, String> {
    translated_roi(
        inspect_window(own_hwnd)?,
        logical_left,
        logical_top,
        logical_cell_width,
        logical_cell_height,
        scale_factor,
    )
}

impl Roi {
    fn dimensions(self) -> Result<(u32, u32), String> {
        if !(2..=32).contains(&self.cell_width) || !(2..=32).contains(&self.cell_height) {
            return Err("marker cell dimensions must be physical 2..=32 pixels".into());
        }
        let width = self.cell_width * MARKER_COLUMNS;
        let height = self.cell_height * MARKER_ROWS;
        self.x.checked_add(width).ok_or("ROI x overflow")?;
        self.y.checked_add(height).ok_or("ROI y overflow")?;
        Ok((width, height))
    }
}

/// Point-in-time read-only checks only. Never treat these as proof that another
/// topmost/transparent window did not obscure the target during the sample.
#[derive(Clone, Copy, Debug, Default)]
pub struct Visibility {
    pub own_process: bool,
    pub foreground: bool,
    pub visible: bool,
    pub iconic: bool,
    pub cloaked: Option<u32>,
    pub input_desktop_accessible: bool,
}

impl Visibility {
    /// Necessary eligibility only; continuous focus and occlusion remain external.
    pub fn necessary_checks_pass(self) -> bool {
        self.own_process
            && self.foreground
            && self.visible
            && !self.iconic
            && self.cloaked == Some(0)
            && self.input_desktop_accessible
    }
}

/// Owned frame evidence. No borrowed D3D buffers or whole-window pixels escape.
#[derive(Clone, Debug)]
pub struct CapturedMarker {
    pub token: [u8; 16],
    pub nonce: u32,
    pub generation: u64,
    pub phase: u8,
    /// WGC SystemRelativeTime.Duration: QPC domain expressed in 100ns units.
    pub frame_time_100ns: i64,
    pub previous_observed_frame_time_100ns: Option<i64>,
    /// Last observed phase < ALL for this exact nonce/generation, if captured.
    pub last_nonfinal_time_100ns: Option<i64>,
    pub content_size: [u32; 2],
    /// FNV-1a of the ROI only; association is established by decoded token+CRC.
    pub roi_digest: u64,
    pub visibility_before: Visibility,
    pub visibility_after: Visibility,
    /// Nonzero means the consumer must not claim an unbroken observation stream.
    pub dropped_output_messages: u64,
}

/// Diagnostic status is bounded separately from successful marker observations.
#[derive(Clone, Debug)]
pub enum ObserverStatus {
    Ready {
        content_size: [u32; 2],
        qpc_frequency: i64,
    },
    RejectedFrame {
        frame_time_100ns: i64,
        reason: String,
    },
    Stopped,
    Failed(String),
}

#[derive(Default)]
struct Shared {
    stop: AtomicBool,
    running: AtomicBool,
    dropped: AtomicU64,
    failure: Mutex<Option<String>>,
}

/// Worker resources remain on one MTA thread. Drop stops, wakes and joins it;
/// never drop this from a capture callback (the callbacks only send wake signals).
/// DO_NOT_WAIT mapping keeps GPU-copy polling cancellable; native driver calls
/// can still stall independently of Rust and no hard shutdown deadline is claimed.
pub struct Observer {
    pub frames: Receiver<CapturedMarker>,
    pub status: Receiver<ObserverStatus>,
    shared: Arc<Shared>,
    wake: SyncSender<()>,
    worker: Option<JoinHandle<()>>,
}

impl Observer {
    /// Start observing ONLY a window belonging to this process. Asynchronous
    /// Ready means the pool is running, not that a calibration marker was seen:
    /// wait for a valid initial marker before asking Sky to supply actual input.
    pub fn start(own_hwnd: usize, roi: Roi) -> Result<Self, String> {
        roi.dimensions()?;
        if own_hwnd == 0 {
            return Err("null HWND".into());
        }
        #[cfg(not(windows))]
        {
            let _ = (own_hwnd, roi);
            Err("WGC requires Windows 10 1903+".into())
        }
        #[cfg(windows)]
        {
            let (frames, receiving) = mpsc::sync_channel(8);
            let (status, statuses) = mpsc::sync_channel(8);
            let (wake, waking) = mpsc::sync_channel(1);
            let shared = Arc::new(Shared::default());
            shared.running.store(true, Ordering::Release);
            let state = shared.clone();
            let notify = wake.clone();
            let worker = std::thread::Builder::new()
                .name("audiolib-wgc-observer".into())
                .spawn(move || {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        native::observe(own_hwnd, roi, &state, &frames, &status, notify, waking)
                    }));
                    let failure = match result {
                        Ok(Ok(())) => None,
                        Ok(Err(message)) => Some(message),
                        Err(_) => Some("WGC observer worker panicked".into()),
                    };
                    if let Some(message) = failure {
                        *state.failure.lock().unwrap_or_else(|e| e.into_inner()) =
                            Some(message.clone());
                        send_bounded(&status, ObserverStatus::Failed(message), &state);
                    } else {
                        send_bounded(&status, ObserverStatus::Stopped, &state);
                    }
                    state.running.store(false, Ordering::Release);
                })
                .map_err(|error| error.to_string())?;
            Ok(Self {
                frames: receiving,
                status: statuses,
                shared,
                wake,
                worker: Some(worker),
            })
        }
    }

    /// Poll terminal failure even if the bounded status queue was full.
    pub fn failure(&self) -> Option<String> {
        self.shared
            .failure
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn is_running(&self) -> bool {
        self.shared.running.load(Ordering::Acquire)
    }

    pub fn dropped_messages(&self) -> u64 {
        self.shared.dropped.load(Ordering::Acquire)
    }
}

impl Drop for Observer {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        let _ = self.wake.try_send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(windows)]
fn send_bounded<T>(sender: &SyncSender<T>, value: T, state: &Shared) {
    if sender.try_send(value).is_err() {
        state.dropped.fetch_add(1, Ordering::AcqRel);
    }
}

/// Compare directly with QPC sampled at the actual input handler. Do not combine
/// WGC absolute timestamps with Rust Instant's opaque origin. Negative deltas and
/// unavailable frequency are errors, never zero-latency successes.
pub fn input_to_frame_ms(
    input_qpc: i64,
    qpc_frequency: i64,
    frame_100ns: i64,
) -> Result<f64, String> {
    if qpc_frequency <= 0 || input_qpc < 0 || frame_100ns < 0 {
        return Err("invalid QPC timestamps/frequency".into());
    }
    let numerator =
        i128::from(frame_100ns) * i128::from(qpc_frequency) - i128::from(input_qpc) * 10_000_000;
    if numerator < 0 {
        return Err("frame predates the input event".into());
    }
    Ok(numerator as f64 / (qpc_frequency as f64 * 10_000.0))
}

#[cfg(windows)]
mod native {
    use super::*;
    use std::{
        ffi::c_void,
        time::{Duration, Instant},
    };
    use windows::{
        Foundation::TypedEventHandler,
        Graphics::{
            Capture::{
                Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureItem,
                GraphicsCaptureSession,
            },
            DirectX::{Direct3D11::IDirect3DDevice, DirectXPixelFormat},
        },
        Win32::{
            Foundation::{
                COLORREF, CloseHandle, FILETIME, HANDLE, HMODULE, HWND, LPARAM, POINT, RECT,
                STILL_ACTIVE,
            },
            Graphics::{
                Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP},
                Direct3D11::{
                    D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    D3D11_MAP_FLAG_DO_NOT_WAIT, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE,
                    D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
                    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Multithread,
                    ID3D11Texture2D,
                },
                Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute},
                Dxgi::{
                    Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC},
                    DXGI_ERROR_WAS_STILL_DRAWING, IDXGIDevice,
                },
                Gdi::{
                    ClientToScreen, EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR,
                    MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
                },
            },
            System::{
                Performance::{QueryPerformanceCounter, QueryPerformanceFrequency},
                ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX},
                StationsAndDesktops::{
                    CloseDesktop, DESKTOP_CONTROL_FLAGS, DESKTOP_READOBJECTS, OpenInputDesktop,
                },
                Threading::{
                    GetCurrentProcess, GetExitCodeProcess, GetProcessTimes, OpenProcess,
                    PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
                    QueryFullProcessImageNameW,
                },
                WinRT::{
                    Direct3D11::{
                        CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
                    },
                    Graphics::Capture::IGraphicsCaptureItemInterop,
                    RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize,
                },
            },
            UI::WindowsAndMessaging::{
                GW_HWNDNEXT, GWL_EXSTYLE, GWL_STYLE, GetClassNameW, GetClientRect,
                GetForegroundWindow, GetLayeredWindowAttributes, GetTopWindow, GetWindow,
                GetWindowLongPtrW, GetWindowRect, GetWindowThreadProcessId, IsIconic,
                IsWindowVisible, LAYERED_WINDOW_ATTRIBUTES_FLAGS,
            },
        },
        core::{BOOL, IInspectable, Interface, PWSTR, factory},
    };

    struct Apartment;

    pub(super) fn process_memory_usage() -> Result<ProcessMemoryUsage, String> {
        let mut counters = PROCESS_MEMORY_COUNTERS_EX {
            cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
            ..Default::default()
        };
        // SAFETY: GetCurrentProcess supplies only this process's pseudo-handle
        // (which must not be closed). The extended counters structure is repr(C)
        // with the PROCESS_MEMORY_COUNTERS prefix, live writable storage and the
        // full documented cb size. No foreign process or memory pages are read.
        unsafe {
            GetProcessMemoryInfo(
                GetCurrentProcess(),
                (&mut counters as *mut PROCESS_MEMORY_COUNTERS_EX).cast(),
                counters.cb,
            )
            .map_err(|error| error.to_string())?;
        }
        Ok(ProcessMemoryUsage {
            peak_working_set_bytes: counters.PeakWorkingSetSize as u64,
            current_working_set_bytes: counters.WorkingSetSize as u64,
            private_usage_bytes: counters.PrivateUsage as u64,
        })
    }

    fn pixel_rectangle(rectangle: RECT) -> PixelRect {
        PixelRect {
            left: rectangle.left,
            top: rectangle.top,
            right: rectangle.right,
            bottom: rectangle.bottom,
        }
    }

    pub(super) fn window_work_area_adjustment(
        value: usize,
    ) -> Result<WindowWorkAreaAdjustment, String> {
        let hwnd = HWND(value as *mut c_void);
        let mut process_id = 0;
        // SAFETY: only scalar PID/geometry queries on a supplied native HWND.
        // Reject foreign/stale HWNDs before reading placement. The monitor handle
        // is borrowed from Windows; no window/desktop state or input is changed.
        unsafe {
            if value == 0
                || GetWindowThreadProcessId(hwnd, Some(&mut process_id)) == 0
                || process_id != std::process::id()
            {
                return Err("refusing placement advice for a non-owned HWND".into());
            }
            let mut bounds = RECT::default();
            DwmGetWindowAttribute(
                hwnd,
                DWMWA_EXTENDED_FRAME_BOUNDS,
                (&mut bounds as *mut RECT).cast(),
                std::mem::size_of::<RECT>() as u32,
            )
            .map_err(|error| format!("read own DWM placement bounds: {error}"))?;
            let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            if monitor.0.is_null() {
                return Err(format!(
                    "nearest native monitor unavailable: frame_before={:?}",
                    pixel_rectangle(bounds)
                ));
            }
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if !GetMonitorInfoW(monitor, &mut info).as_bool() {
                return Err(format!(
                    "nearest monitor workarea unavailable: frame_before={:?}",
                    pixel_rectangle(bounds)
                ));
            }
            let mut checked_id = 0;
            if GetWindowThreadProcessId(hwnd, Some(&mut checked_id)) == 0
                || checked_id != process_id
            {
                return Err("own HWND identity changed during placement inspection".into());
            }
            work_area_adjustment(
                pixel_rectangle(bounds),
                pixel_rectangle(info.rcWork),
                pixel_rectangle(info.rcMonitor),
            )
        }
    }

    struct MonitorEnumeration {
        monitors: [PixelRect; MAX_DESKTOP_MONITORS],
        workareas: [PixelRect; MAX_DESKTOP_MONITORS],
        count: usize,
        valid: bool,
    }

    unsafe extern "system" fn monitor_geometry_callback(
        monitor: HMONITOR,
        _dc: HDC,
        _clip: *mut RECT,
        data: LPARAM,
    ) -> BOOL {
        let pointer = data.0 as *mut MonitorEnumeration;
        if pointer.is_null() {
            return BOOL(0);
        }
        // SAFETY: the only caller is synchronous EnumDisplayMonitors below. Its
        // LPARAM points to our live stack buffer for the call's full duration;
        // no callback/thread retains it, and each callback has exclusive access.
        let enumeration = unsafe { &mut *pointer };
        // Never unwind across the Windows ABI, including future callback edits.
        // No allocation, formatting, user callback or device-name query occurs.
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if enumeration.count >= MAX_DESKTOP_MONITORS {
                enumeration.valid = false;
                return false;
            }
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            // SAFETY: monitor comes from this enumeration; MONITORINFO is an
            // initialized writable local with the required cbSize.
            if !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
                enumeration.valid = false;
                return false;
            }
            let rectangle = pixel_rectangle(info.rcMonitor);
            let work = pixel_rectangle(info.rcWork);
            if rectangle.area().is_none()
                || work.area().is_none()
                || rectangle.intersection(work) != Some(work)
            {
                enumeration.valid = false;
                return false;
            }
            enumeration.monitors[enumeration.count] = rectangle;
            enumeration.workareas[enumeration.count] = work;
            enumeration.count += 1;
            true
        })) {
            Ok(keep_going) => BOOL(i32::from(keep_going)),
            Err(_) => {
                enumeration.valid = false;
                BOOL(0)
            }
        }
    }

    fn desktop_coverage(frame: PixelRect, client: PixelRect) -> DesktopCoverage {
        let mut enumeration = MonitorEnumeration {
            monitors: [PixelRect::default(); MAX_DESKTOP_MONITORS],
            workareas: [PixelRect::default(); MAX_DESKTOP_MONITORS],
            count: 0,
            valid: true,
        };
        // SAFETY: NULL HDC/clip asks only for native monitor geometry. The callback
        // executes synchronously, is panic-contained, and writes at most 64 owned
        // entries into the stack buffer, which outlives this entire native call.
        let completed = unsafe {
            EnumDisplayMonitors(
                None,
                None,
                Some(monitor_geometry_callback),
                LPARAM((&mut enumeration as *mut MonitorEnumeration) as isize),
            )
        }
        .as_bool()
            && enumeration.valid;
        measure_desktop_coverage(
            frame,
            client,
            &enumeration.monitors[..enumeration.count],
            &enumeration.workareas[..enumeration.count],
            completed,
        )
    }

    fn occluder_candidate(
        hwnd: HWND,
        recognized_pointer: bool,
        intersection: PixelRect,
    ) -> OccluderCandidate {
        let mut result = OccluderCandidate {
            recognized_pointer,
            client_intersection: intersection,
            ..OccluderCandidate::EMPTY
        };
        // SAFETY: query only PID and OS class metadata into initialized owned
        // buffers. GetClassNameW does not query title text or foreign contents.
        unsafe {
            let pid_ok = GetWindowThreadProcessId(hwnd, Some(&mut result.process_id)) != 0
                && result.process_id != 0;
            let mut class = [0_u16; 512];
            let length = GetClassNameW(hwnd, &mut class);
            if let Ok(length) = usize::try_from(length)
                && length > 0
                && length < class.len()
            {
                let retained = length.min(OCCLUDER_CLASS_UNITS);
                result.class_name_utf16[..retained].copy_from_slice(&class[..retained]);
                result.class_name_units = retained as u16;
                result.class_name_truncated = retained < length;
                let mut hash = 0xcbf29ce484222325_u64;
                for unit in &class[..length] {
                    for byte in unit.to_le_bytes() {
                        hash ^= u64::from(byte);
                        hash = hash.wrapping_mul(0x100000001b3);
                    }
                }
                result.class_name_hash = hash;
                result.identity_read_complete = pid_ok;
            }
        }
        result
    }

    struct QueryProcess(HANDLE);
    impl Drop for QueryProcess {
        fn drop(&mut self) {
            // SAFETY: OpenProcess returned this owned, noninheritable handle.
            // It is never shared with a callback or transferred to another API.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }

    pub(super) fn pointer_process_identity(
        process_id: u32,
    ) -> Result<PointerProcessIdentity, String> {
        use std::os::windows::ffi::OsStringExt;
        if process_id == 0 {
            return Err("pointer process ID must be nonzero".into());
        }
        // SAFETY: only limited read-only identity queries; no process memory,
        // input, state mutation, termination or foreign window pixels are read.
        unsafe {
            let process = QueryProcess(
                OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id)
                    .map_err(|_| "pointer process identity is inaccessible")?,
            );
            let mut creation = FILETIME::default();
            let mut exit = FILETIME::default();
            let mut kernel = FILETIME::default();
            let mut user = FILETIME::default();
            GetProcessTimes(process.0, &mut creation, &mut exit, &mut kernel, &mut user)
                .map_err(|_| "pointer process creation time is inaccessible")?;
            let mut image = [0_u16; 32768];
            let mut length = image.len() as u32;
            QueryFullProcessImageNameW(
                process.0,
                PROCESS_NAME_WIN32,
                PWSTR(image.as_mut_ptr()),
                &mut length,
            )
            .map_err(|_| "pointer process image identity is inaccessible")?;
            let length = usize::try_from(length).map_err(|_| "invalid process image length")?;
            let image = image.get(..length).ok_or("invalid process image length")?;
            let path = std::path::PathBuf::from(std::ffi::OsString::from_wide(image))
                .canonicalize()
                .map_err(|_| "pointer process image cannot be canonicalized")?;
            let canonical_image = path
                .to_str()
                .ok_or("pointer process image is not valid Unicode")?
                .replace('/', "\\")
                .to_lowercase();
            let mut code = 0;
            GetExitCodeProcess(process.0, &mut code)
                .map_err(|_| "pointer process state is inaccessible")?;
            if code != STILL_ACTIVE.0 as u32 {
                return Err("pointer process is no longer running".into());
            }
            Ok(PointerProcessIdentity {
                process_id,
                creation_time_100ns: (u64::from(creation.dwHighDateTime) << 32)
                    | u64::from(creation.dwLowDateTime),
                canonical_image,
            })
        }
    }

    fn pointer_overlay_attributes(hwnd: HWND) -> Option<PointerOverlayAttributes> {
        // SAFETY: these calls only query native scalar/class metadata into live
        // buffers. Failure is an unknown application occluder, never an exception.
        unsafe {
            let mut process_id = 0;
            if GetWindowThreadProcessId(hwnd, Some(&mut process_id)) == 0 || process_id == 0 {
                return None;
            }
            let mut class = [0_u16; 256];
            let length = GetClassNameW(hwnd, &mut class);
            if length <= 0 {
                return None;
            }
            let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
            let extended_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            let mut color = COLORREF::default();
            let mut alpha = 0;
            let mut flags = LAYERED_WINDOW_ATTRIBUTES_FLAGS::default();
            GetLayeredWindowAttributes(hwnd, Some(&mut color), Some(&mut alpha), Some(&mut flags))
                .ok()?;
            let mut checked_id = 0;
            if GetWindowThreadProcessId(hwnd, Some(&mut checked_id)) == 0
                || checked_id != process_id
            {
                return None;
            }
            Some(PointerOverlayAttributes {
                process_id,
                class_name: String::from_utf16(&class[..length as usize]).ok()?,
                style,
                extended_style,
                alpha,
                color_key: color.0,
                layered_flags: flags.0,
            })
        }
    }

    pub(super) fn qpc_now() -> Result<QpcStamp, String> {
        let mut ticks = 0;
        let mut frequency = 0;
        // SAFETY: both functions only read the native clock into live i64s;
        // they do not touch UI, COM apartments or application state.
        unsafe {
            QueryPerformanceCounter(&mut ticks).map_err(|error| error.to_string())?;
            QueryPerformanceFrequency(&mut frequency).map_err(|error| error.to_string())?;
        }
        if ticks < 0 || frequency <= 0 {
            return Err("invalid native QPC clock".into());
        }
        Ok(QpcStamp { ticks, frequency })
    }

    impl Apartment {
        fn new() -> Result<Self, String> {
            // SAFETY: this newly created worker has no previous COM apartment;
            // Apartment drops after every WinRT object on this same thread.
            unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.map_err(|e| e.to_string())?;
            Ok(Self)
        }
    }
    impl Drop for Apartment {
        fn drop(&mut self) {
            // SAFETY: balances the successful initialization on this worker.
            unsafe { RoUninitialize() };
        }
    }

    fn visibility(hwnd: HWND) -> Visibility {
        // SAFETY: calls only query a caller-supplied native handle. HWND validity
        // is rechecked through owning PID, and all output pointers are live.
        unsafe {
            let mut pid = 0;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            let mut cloaked = 0_u32;
            let cloak = DwmGetWindowAttribute(
                hwnd,
                DWMWA_CLOAKED,
                (&mut cloaked as *mut u32).cast(),
                std::mem::size_of::<u32>() as u32,
            )
            .ok()
            .map(|()| cloaked);
            let input_desktop_accessible =
                match OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_READOBJECTS) {
                    Ok(desktop) => {
                        let _ = CloseDesktop(desktop);
                        true
                    }
                    Err(_) => false,
                };
            Visibility {
                own_process: pid == std::process::id(),
                foreground: GetForegroundWindow() == hwnd,
                visible: IsWindowVisible(hwnd).as_bool(),
                iconic: IsIconic(hwnd).as_bool(),
                cloaked: cloak,
                input_desktop_accessible,
            }
        }
    }

    pub(super) fn inspect_window(
        value: usize,
        policy: Option<&PointerOverlayPolicy>,
    ) -> Result<WindowInspection, String> {
        let hwnd = HWND(value as *mut c_void);
        let state = visibility(hwnd);
        if value == 0 || !state.own_process {
            return Err("refusing geometry of a non-owned HWND".into());
        }
        let mut client = RECT::default();
        let mut origin = POINT::default();
        let mut bounds = RECT::default();
        // SAFETY: all native functions only query handles/geometry into live
        // initialized outputs. No foreign memory or native window state is changed.
        unsafe {
            GetClientRect(hwnd, &mut client).map_err(|error| error.to_string())?;
            if !ClientToScreen(hwnd, &mut origin).as_bool() {
                return Err("ClientToScreen failed".into());
            }
            DwmGetWindowAttribute(
                hwnd,
                DWMWA_EXTENDED_FRAME_BOUNDS,
                (&mut bounds as *mut RECT).cast(),
                std::mem::size_of::<RECT>() as u32,
            )
            .map_err(|error| error.to_string())?;
        }
        let positive = |value: i64| {
            u32::try_from(value)
                .ok()
                .filter(|value| *value > 0)
                .ok_or_else(|| "empty/invalid native bounds".to_string())
        };
        let client_size = [
            positive(i64::from(client.right) - i64::from(client.left))?,
            positive(i64::from(client.bottom) - i64::from(client.top))?,
        ];
        let expected_capture_size = [
            positive(i64::from(bounds.right) - i64::from(bounds.left))?,
            positive(i64::from(bounds.bottom) - i64::from(bounds.top))?,
        ];
        let offset = |value: i64| {
            u32::try_from(value).map_err(|_| "client origin lies outside capture frame".to_string())
        };
        let client_origin_in_capture = [
            offset(i64::from(origin.x) - i64::from(bounds.left))?,
            offset(i64::from(origin.y) - i64::from(bounds.top))?,
        ];
        let client_right = i64::from(origin.x) + i64::from(client_size[0]);
        let client_bottom = i64::from(origin.y) + i64::from(client_size[1]);
        let screen_client = PixelRect {
            left: origin.x,
            top: origin.y,
            right: i32::try_from(client_right).map_err(|_| "client right coordinate overflow")?,
            bottom: i32::try_from(client_bottom)
                .map_err(|_| "client bottom coordinate overflow")?,
        };
        let desktop_coverage = desktop_coverage(pixel_rectangle(bounds), screen_client);
        let mut potential_occluders = 0;
        let mut raw_intersecting_windows = 0;
        let mut recognized_cursor_overlays = 0;
        let mut occluder_candidates = [OccluderCandidate::EMPTY; MAX_OCCLUDER_CANDIDATES];
        let mut occluder_candidate_count = 0;
        let mut occluder_candidates_overflow = 0;
        // Read the selected process once per inspection, not once per overlay.
        // Revalidate creation+canonical path each time: PID reuse/read failures
        // cannot continue a previously recognized exception.
        let pointer_identity =
            policy.and_then(|policy| pointer_process_identity(policy.identity.process_id).ok());
        let mut complete = false;
        // SAFETY: traverse only native HWNDs returned by the OS, querying no
        // window text or contents. Cap traversal to prevent malformed/racing lists
        // from making the diagnostic unbounded. Unknown rectangles reject.
        unsafe {
            let mut current = GetTopWindow(None).map_err(|error| error.to_string())?;
            for _ in 0..4096 {
                if current == hwnd {
                    complete = true;
                    break;
                }
                if IsWindowVisible(current).as_bool() && !IsIconic(current).as_bool() {
                    let mut cloaked = 0_u32;
                    let cloak_known = DwmGetWindowAttribute(
                        current,
                        DWMWA_CLOAKED,
                        (&mut cloaked as *mut u32).cast(),
                        std::mem::size_of::<u32>() as u32,
                    )
                    .is_ok();
                    if !cloak_known || cloaked == 0 {
                        let mut rectangle = RECT::default();
                        GetWindowRect(current, &mut rectangle)
                            .map_err(|error| error.to_string())?;
                        if let Some(intersection) =
                            pixel_rectangle(rectangle).intersection(screen_client)
                        {
                            raw_intersecting_windows += 1;
                            let attributes =
                                policy.and_then(|_| pointer_overlay_attributes(current));
                            let recognized = is_recognized_pointer_overlay(
                                policy,
                                pointer_identity.as_ref(),
                                attributes.as_ref(),
                            );
                            if recognized {
                                recognized_cursor_overlays += 1;
                            } else {
                                potential_occluders += 1;
                            }
                            if occluder_candidate_count < MAX_OCCLUDER_CANDIDATES {
                                let relative = |coordinate: i32, origin: i32| {
                                    i32::try_from(i64::from(coordinate) - i64::from(origin))
                                        .map_err(|_| "occluder/client coordinate overflow")
                                };
                                let local = PixelRect {
                                    left: relative(intersection.left, origin.x)?,
                                    top: relative(intersection.top, origin.y)?,
                                    right: relative(intersection.right, origin.x)?,
                                    bottom: relative(intersection.bottom, origin.y)?,
                                };
                                occluder_candidates[occluder_candidate_count] =
                                    occluder_candidate(current, recognized, local);
                                occluder_candidate_count += 1;
                            } else {
                                occluder_candidates_overflow += 1;
                            }
                        }
                    }
                }
                match GetWindow(current, GW_HWNDNEXT) {
                    Ok(next) => current = next,
                    Err(_) => break,
                }
            }
        }
        Ok(WindowInspection {
            visibility: state,
            client_size,
            client_origin_in_capture,
            expected_capture_size,
            desktop_coverage,
            raw_intersecting_windows,
            recognized_cursor_overlays,
            potential_occluders,
            occlusion_scan_complete: complete,
            occluder_candidates,
            occluder_candidate_count: occluder_candidate_count as u32,
            occluder_candidates_overflow,
        })
    }

    struct Capture {
        pool: Direct3D11CaptureFramePool,
        session: GraphicsCaptureSession,
        item: GraphicsCaptureItem,
        arrived: Option<i64>,
        closed: Option<i64>,
    }
    impl Drop for Capture {
        fn drop(&mut self) {
            // Handlers capture only a wake sender/atomic, never this Capture,
            // its COM objects, mapped memory, Observer or any Slint object.
            if let Some(token) = self.arrived {
                let _ = self.pool.RemoveFrameArrived(token);
            }
            if let Some(token) = self.closed {
                let _ = self.item.RemoveClosed(token);
            }
            let _ = self.session.Close();
            let _ = self.pool.Close();
        }
    }

    struct Frame(Direct3D11CaptureFrame);
    impl Drop for Frame {
        fn drop(&mut self) {
            let _ = self.0.Close();
        }
    }

    struct Mapped<'a> {
        context: &'a ID3D11DeviceContext,
        texture: &'a ID3D11Texture2D,
    }
    impl Drop for Mapped<'_> {
        fn drop(&mut self) {
            // SAFETY: created only after successful Map of subresource zero;
            // texture/context and all reads outlive this guard on one worker.
            unsafe { self.context.Unmap(self.texture, 0) };
        }
    }

    fn create_device() -> Result<(ID3D11Device, ID3D11DeviceContext), String> {
        let mut last_error = String::new();
        for driver in [D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP] {
            let mut device = None;
            let mut context = None;
            // SAFETY: output Options are valid; no application rendering context
            // is touched. Both returned objects stay on this observer worker.
            let result = unsafe {
                D3D11CreateDevice(
                    None::<&windows::Win32::Graphics::Dxgi::IDXGIAdapter>,
                    driver,
                    HMODULE::default(),
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    None,
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    None,
                    Some(&mut context),
                )
            };
            match result {
                Ok(()) => {
                    return device
                        .zip(context)
                        .ok_or_else(|| "D3D11 returned no device/context".into());
                }
                Err(error) => last_error = error.to_string(),
            }
        }
        Err(format!("D3D11 capture device unavailable: {last_error}"))
    }

    fn staging(device: &ID3D11Device, roi: Roi) -> Result<ID3D11Texture2D, String> {
        let (width, height) = roi.dimensions()?;
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        let mut texture = None;
        // SAFETY: valid immutable descriptor and live output Option. The staging
        // texture is ROI-sized only; no complete-window CPU image is allocated.
        unsafe { device.CreateTexture2D(&desc, None, Some(&mut texture)) }
            .map_err(|e| e.to_string())?;
        texture.ok_or_else(|| "D3D11 returned no ROI staging texture".into())
    }

    fn read_marker(
        context: &ID3D11DeviceContext,
        target: &ID3D11Texture2D,
        frame: &Direct3D11CaptureFrame,
        roi: Roi,
        state: &Shared,
    ) -> Result<([u8; 16], u64), String> {
        let (width, height) = roi.dimensions()?;
        let surface = frame.Surface().map_err(|e| e.to_string())?;
        let access: IDirect3DDxgiInterfaceAccess = surface.cast().map_err(|e| e.to_string())?;
        // SAFETY: WGC owns a D3D11 texture surface; the frame remains checked out
        // until GPU copy and mapped ROI reads finish and the map guard is dropped.
        let source: ID3D11Texture2D =
            unsafe { access.GetInterface() }.map_err(|e| e.to_string())?;
        let mut source_desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: live texture and writable descriptor output on this worker.
        unsafe { source.GetDesc(&mut source_desc) };
        if source_desc.Format != DXGI_FORMAT_B8G8R8A8_UNORM
            || source_desc.SampleDesc.Count != 1
            || source_desc.Width < roi.x + width
            || source_desc.Height < roi.y + height
        {
            return Err("capture texture format/extent differs from checked ROI".into());
        }
        let region = D3D11_BOX {
            left: roi.x,
            top: roi.y,
            front: 0,
            right: roi.x + width,
            bottom: roi.y + height,
            back: 1,
        };
        // SAFETY: caller checked ContentSize bounds and format; source and target
        // use BGRA8 with one subresource. Context is touched on this worker only.
        unsafe {
            context.CopySubresourceRegion(target, 0, 0, 0, 0, &source, 0, Some(&region));
            context.Flush();
        }
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        let started = Instant::now();
        loop {
            if state.stop.load(Ordering::Acquire) {
                return Err("observer stopped".into());
            }
            // SAFETY: target is CPU-readable staging, output points to live
            // storage, and DO_NOT_WAIT lets stop/timeout interrupt GPU polling.
            match unsafe {
                context.Map(
                    target,
                    0,
                    D3D11_MAP_READ,
                    D3D11_MAP_FLAG_DO_NOT_WAIT.0 as u32,
                    Some(&mut mapped),
                )
            } {
                Ok(()) => break,
                Err(error)
                    if error.code() == DXGI_ERROR_WAS_STILL_DRAWING
                        && started.elapsed() < Duration::from_millis(500) =>
                {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(error) => return Err(format!("ROI Map failed/timed out: {error}")),
            }
        }
        let _map = Mapped {
            context,
            texture: target,
        };
        if mapped.pData.is_null() || mapped.RowPitch < width * 4 {
            return Err("invalid mapped ROI storage".into());
        }
        (mapped.RowPitch as usize)
            .checked_mul(height as usize)
            .ok_or("mapped ROI overflow")?;
        let row = |y: usize| {
            // SAFETY: a successful Map of the exact ROI-sized texture provides
            // checked RowPitch/height. Callers use y<height and read ONLY the
            // copied width*4 pixel bytes, never undefined row-pitch padding.
            // No pointer or borrowed pixels escape the live map guard.
            unsafe {
                std::slice::from_raw_parts(
                    mapped.pData.cast::<u8>().add(y * mapped.RowPitch as usize),
                    width as usize * 4,
                )
            }
        };
        decode_rows(roi, row)
    }

    pub(super) fn observe(
        hwnd_value: usize,
        roi: Roi,
        state: &Arc<Shared>,
        frames: &SyncSender<CapturedMarker>,
        statuses: &SyncSender<ObserverStatus>,
        wake: SyncSender<()>,
        waking: Receiver<()>,
    ) -> Result<(), String> {
        let _apartment = Apartment::new()?;
        let hwnd = HWND(hwnd_value as *mut c_void);
        if !visibility(hwnd).own_process {
            return Err("refusing HWND not owned by this process".into());
        }
        if !GraphicsCaptureSession::IsSupported().map_err(|e| e.to_string())? {
            return Err("Windows Graphics Capture is unsupported".into());
        }
        let factory: IGraphicsCaptureItemInterop =
            factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
                .map_err(|e| e.to_string())?;
        // SAFETY: checked live own-process HWND; Windows owns the native handle.
        // The observer owns only its capture item, never destroys the window.
        let item: GraphicsCaptureItem =
            unsafe { factory.CreateForWindow(hwnd) }.map_err(|e| e.to_string())?;
        let size = item.Size().map_err(|e| e.to_string())?;
        let (width, height) = roi.dimensions()?;
        if size.Width <= 0
            || size.Height <= 0
            || roi.x + width > size.Width as u32
            || roi.y + height > size.Height as u32
        {
            return Err("marker ROI lies outside capture ContentSize".into());
        }
        let (device, context) = create_device()?;
        let multithread: ID3D11Multithread = context.cast().map_err(|e| e.to_string())?;
        // SAFETY: this owned capture context is live; enable D3D serialization
        // before WGC can use its device internally on capture worker threads.
        let _previous = unsafe { multithread.SetMultithreadProtected(true) };
        let dxgi: IDXGIDevice = device.cast().map_err(|e| e.to_string())?;
        // SAFETY: dxgi is a live interface of this owned D3D device; cast creates
        // an independently refcounted WinRT wrapper with no borrowed pointers.
        let wrapped =
            unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }.map_err(|e| e.to_string())?;
        let wrapped: IDirect3DDevice = wrapped.cast().map_err(|e| e.to_string())?;
        let target = staging(&device, roi)?;
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &wrapped,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            2,
            size,
        )
        .map_err(|e| e.to_string())?;
        let session = match pool.CreateCaptureSession(&item) {
            Ok(session) => session,
            Err(error) => {
                let _ = pool.Close();
                return Err(error.to_string());
            }
        };
        let mut capture = Capture {
            pool,
            session,
            item,
            arrived: None,
            closed: None,
        };
        let notify = wake.clone();
        capture.arrived = Some(
            capture
                .pool
                .FrameArrived(
                    &TypedEventHandler::<Direct3D11CaptureFramePool, IInspectable>::new(
                        move |_, _| {
                            let _ = notify.try_send(());
                            Ok(())
                        },
                    ),
                )
                .map_err(|e| e.to_string())?,
        );
        let closed_state = state.clone();
        let closed_wake = wake;
        capture.closed = Some(
            capture
                .item
                .Closed(
                    &TypedEventHandler::<GraphicsCaptureItem, IInspectable>::new(move |_, _| {
                        closed_state.stop.store(true, Ordering::Release);
                        let _ = closed_wake.try_send(());
                        Ok(())
                    }),
                )
                .map_err(|e| e.to_string())?,
        );
        // Cursor capture is optional on older OS builds; never rely on disabling
        // the system capture border or request extra borderless permissions.
        let _ = capture.session.SetIsCursorCaptureEnabled(false);
        capture.session.StartCapture().map_err(|e| e.to_string())?;
        let mut frequency = 0;
        // SAFETY: live writable i64 output; this reads only the system counter.
        unsafe { QueryPerformanceFrequency(&mut frequency) }.map_err(|e| e.to_string())?;
        send_bounded(
            statuses,
            ObserverStatus::Ready {
                content_size: [size.Width as u32, size.Height as u32],
                qpc_frequency: frequency,
            },
            state,
        );
        let mut previous_frame = None;
        let mut tracked = None;
        let mut last_nonfinal = None;
        while !state.stop.load(Ordering::Acquire) {
            let _ = waking.recv_timeout(Duration::from_millis(20));
            if state.stop.load(Ordering::Acquire) {
                break;
            }
            // One wake can represent several frames. Never retain frame surfaces
            // after Close; callbacks carry no pixels and never block the UI.
            loop {
                if state.stop.load(Ordering::Acquire) {
                    break;
                }
                let frame = match capture.pool.TryGetNextFrame() {
                    Ok(frame) => Frame(frame),
                    // windows-core 0.62 maps the successful nullable WinRT return
                    // to Error::empty(), whose public HRESULT is S_OK (zero).
                    // This is NOT E_POINTER; genuine error HRESULTs propagate.
                    Err(error) if error.code() == windows::core::HRESULT(0) => {
                        break;
                    }
                    Err(error) => return Err(format!("WGC frame dequeue failed: {error}")),
                };
                let timestamp = frame
                    .0
                    .SystemRelativeTime()
                    .map_err(|e| e.to_string())?
                    .Duration;
                let content = frame.0.ContentSize().map_err(|e| e.to_string())?;
                if content != size {
                    return Err("capture resized; invalidate sample and recalibrate ROI".into());
                }
                if timestamp < 0 || previous_frame.is_some_and(|old| timestamp < old) {
                    return Err("nonmonotonic/invalid compositor timestamp".into());
                }
                let before = visibility(hwnd);
                let decoded = read_marker(&context, &target, &frame.0, roi, state).and_then(
                    |(token, digest)| decode_marker(token).map(|decoded| (token, digest, decoded)),
                );
                let after = visibility(hwnd);
                match decoded {
                    Ok((token, digest, (nonce, generation, phase))) => {
                        if tracked != Some((nonce, generation)) {
                            tracked = Some((nonce, generation));
                            last_nonfinal = None;
                        }
                        if phase < PHASE_ALL {
                            last_nonfinal = Some(timestamp);
                        }
                        send_bounded(
                            frames,
                            CapturedMarker {
                                token,
                                nonce,
                                generation,
                                phase,
                                frame_time_100ns: timestamp,
                                previous_observed_frame_time_100ns: previous_frame,
                                last_nonfinal_time_100ns: last_nonfinal,
                                content_size: [content.Width as u32, content.Height as u32],
                                roi_digest: digest,
                                visibility_before: before,
                                visibility_after: after,
                                dropped_output_messages: state.dropped.load(Ordering::Acquire),
                            },
                            state,
                        );
                    }
                    Err(reason) if !state.stop.load(Ordering::Acquire) => {
                        send_bounded(
                            statuses,
                            ObserverStatus::RejectedFrame {
                                frame_time_100ns: timestamp,
                                reason,
                            },
                            state,
                        );
                    }
                    Err(_) => {}
                }
                previous_frame = Some(timestamp);
            }
            // A static idle window legitimately produces no frames. The caller
            // owns armed-sample deadlines; idle never fabricates a success.
        }
        drop(capture);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn protocol_is_msb_first_crc_checked_and_retains_full_identity() {
        for phase in [PHASE_PENDING, PHASE_PAGE, PHASE_ALL] {
            let token = encode_marker(0x12345678, u64::MAX, phase).unwrap();
            assert_eq!(decode_marker(token).unwrap(), (0x12345678, u64::MAX, phase));
            let bits = marker_bits(&token);
            let mut reconstructed = [0_u8; 16];
            for (index, bit) in bits.into_iter().enumerate() {
                if bit {
                    reconstructed[index / 8] |= 0x80 >> (index % 8);
                }
            }
            assert_eq!(reconstructed, token);
            for index in 0..128 {
                let mut damaged = token;
                damaged[index / 8] ^= 0x80 >> (index % 8);
                assert!(decode_marker(damaged).is_err());
            }
        }
    }
    #[test]
    fn qpc_conversion_uses_frame_timestamp_not_callback_latency() {
        assert_eq!(
            input_to_frame_ms(10_000_000, 10_000_000, 10_125_000).unwrap(),
            12.5
        );
        assert!(input_to_frame_ms(10_000_000, 10_000_000, 9_999_999).is_err());
        assert_eq!(
            input_to_frame_ms(30_000_000, 3_000_000, 100_500_000).unwrap(),
            50.0
        );
        for (ticks, frequency, frame) in [(-1, 1, 0), (0, 0, 0), (0, -1, 0), (0, 1, -1)] {
            assert!(input_to_frame_ms(ticks, frequency, frame).is_err());
        }
    }

    #[test]
    fn pixel_capture_decoder_handles_bgra_rows_and_rejects_corrupt_or_ambiguous_cells() {
        let roi = Roi {
            x: 10,
            y: 20,
            cell_width: 3,
            cell_height: 4,
        };
        let (width, height) = roi.dimensions().unwrap();
        let token = encode_marker(0xfeed_1234, 0x1234_5678_9abc_def0, PHASE_ALL).unwrap();
        let mut rows = vec![vec![0x55; width as usize * 4 + 17]; height as usize];
        for (y, row) in rows.iter_mut().enumerate() {
            for x in 0..width as usize {
                let bit = (y / 4) * 64 + x / 3;
                let level = if token[bit / 8] & (0x80 >> (bit % 8)) != 0 {
                    255
                } else {
                    0
                };
                row[x * 4..x * 4 + 4].copy_from_slice(&[level, level, level, 255]);
            }
        }
        let (decoded, digest) = decode_rows(roi, |index| &rows[index]).unwrap();
        assert_eq!(
            decode_marker(decoded).unwrap(),
            (0xfeed_1234, 0x1234_5678_9abc_def0, PHASE_ALL)
        );
        for row in &mut rows {
            row[width as usize * 4..].fill(0xff);
        }
        assert_eq!(
            decode_rows(roi, |index| &rows[index]).unwrap(),
            (decoded, digest),
            "pitch padding must be excluded"
        );
        let x = 3 + 1;
        let y = 4 + 2;
        rows[y][x * 4..x * 4 + 3].fill(127);
        assert!(decode_rows(roi, |index| &rows[index]).is_err());
        rows[y][x * 4..x * 4 + 3].fill(0);
        let mut corrupt = token;
        corrupt[7] ^= 0x04;
        assert!(decode_marker(corrupt).is_err());
        assert!(decode_rows(roi, |_| &[0; 3]).is_err());
    }

    #[test]
    fn roi_rejects_degenerate_or_overflowing_physical_bounds() {
        for roi in [
            Roi {
                x: 0,
                y: 0,
                cell_width: 0,
                cell_height: 4,
            },
            Roi {
                x: 0,
                y: 0,
                cell_width: 2,
                cell_height: 1,
            },
            Roi {
                x: 0,
                y: 0,
                cell_width: 33,
                cell_height: 4,
            },
            Roi {
                x: u32::MAX,
                y: 0,
                cell_width: 2,
                cell_height: 4,
            },
            Roi {
                x: 0,
                y: u32::MAX,
                cell_width: 2,
                cell_height: 4,
            },
        ] {
            assert!(roi.dimensions().is_err());
        }
    }

    #[test]
    fn client_geometry_uses_measured_offsets_and_rejects_fractional_cells_or_occlusion() {
        let mut inspection = WindowInspection {
            visibility: Visibility {
                own_process: true,
                foreground: true,
                visible: true,
                iconic: false,
                cloaked: Some(0),
                input_desktop_accessible: true,
            },
            client_size: [1000, 800],
            client_origin_in_capture: [8, 31],
            expected_capture_size: [1016, 839],
            desktop_coverage: measure_desktop_coverage(
                rect(0, 0, 1016, 839),
                rect(8, 31, 1008, 831),
                &[rect(0, 0, 1016, 839)],
                &[rect(0, 0, 1016, 839)],
                true,
            ),
            raw_intersecting_windows: 0,
            recognized_cursor_overlays: 0,
            potential_occluders: 0,
            occlusion_scan_complete: true,
            occluder_candidates: [OccluderCandidate::default(); MAX_OCCLUDER_CANDIDATES],
            occluder_candidate_count: 0,
            occluder_candidates_overflow: 0,
        };
        assert!(inspection.necessary_checks_pass());
        let geometry = translated_roi(inspection, 100.0, 20.0, 3.0, 6.0, 2.0).unwrap();
        assert_eq!(
            [
                geometry.roi.x,
                geometry.roi.y,
                geometry.roi.cell_width,
                geometry.roi.cell_height
            ],
            [208, 71, 6, 12]
        );
        assert!(translated_roi(inspection, 100.0, 20.0, 3.0, 6.0, 1.25).is_err());
        assert!(translated_roi(inspection, 990.0, 20.0, 3.0, 6.0, 1.0).is_err());
        assert!(translated_roi(inspection, -1.0, 20.0, 3.0, 6.0, 1.0).is_err());
        inspection.potential_occluders = 1;
        assert!(!inspection.necessary_checks_pass());
        inspection.raw_intersecting_windows = 10;
        inspection.recognized_cursor_overlays = 9;
        inspection.occluder_candidates_overflow = 2;
        assert!(
            !inspection.necessary_checks_pass(),
            "recognized pointers/summary overflow cannot hide an unknown window"
        );
        inspection.potential_occluders = 0;
        inspection.occlusion_scan_complete = false;
        assert!(!inspection.necessary_checks_pass());
        inspection.occlusion_scan_complete = true;
        inspection.visibility.foreground = false;
        assert!(!inspection.necessary_checks_pass());
    }

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> PixelRect {
        PixelRect {
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn workarea_placement_translates_taskbar_overlap_and_preserves_frame_size() {
        let frame = rect(40, 40, 2924, 1939);
        let work = rect(0, 96, 3840, 2160);
        let adjustment = work_area_adjustment(frame, work, rect(0, 0, 3840, 2160)).unwrap();
        assert_eq!(adjustment.delta, [0, 64]);
        let translated = rect(
            frame.left + adjustment.delta[0],
            frame.top + adjustment.delta[1],
            frame.right + adjustment.delta[0],
            frame.bottom + adjustment.delta[1],
        );
        assert_eq!(translated.area(), frame.area());
        assert_eq!(work.intersection(translated), Some(translated));
        assert_eq!(translated.top, work.top + 8);
        assert_eq!(adjustment.frame_before, frame);
        let unchanged = work_area_adjustment(translated, work, adjustment.monitor_bounds).unwrap();
        assert_eq!(
            unchanged.delta,
            [0, 0],
            "one adjustment is stable; repeated movement is unnecessary"
        );
    }

    #[test]
    fn workarea_placement_uses_minimal_clamping_with_negative_coordinates() {
        let negative = work_area_adjustment(
            rect(-2000, -1100, -1000, -400),
            rect(-1920, -1040, 0, 0),
            rect(-1920, -1080, 0, 0),
        )
        .unwrap();
        assert_eq!(negative.delta, [88, 68]);
        let right_bottom = work_area_adjustment(
            rect(900, 900, 1300, 1200),
            rect(0, 0, 1000, 1000),
            rect(0, 0, 1000, 1000),
        )
        .unwrap();
        assert_eq!(right_bottom.delta, [-308, -208]);
        assert_eq!(
            work_area_adjustment(
                rect(100, 200, 500, 600),
                rect(0, 0, 1000, 1000),
                rect(0, 0, 1000, 1000),
            )
            .unwrap()
            .delta,
            [0, 0]
        );
    }

    #[test]
    fn workarea_placement_reduces_margin_without_resizing_and_reports_cannot_fit() {
        let exact = work_area_adjustment(
            rect(20, 20, 120, 120),
            rect(0, 0, 100, 100),
            rect(0, 0, 100, 100),
        )
        .unwrap();
        assert_eq!(exact.delta, [-20, -20]);
        let small_slack = work_area_adjustment(
            rect(0, 0, 100, 100),
            rect(0, 0, 103, 115),
            rect(0, 0, 103, 115),
        )
        .unwrap();
        assert_eq!(small_slack.delta, [1, 7]);
        for frame in [rect(0, 0, 101, 100), rect(0, 0, 100, 101)] {
            let error = work_area_adjustment(frame, rect(0, 0, 100, 100), rect(0, 0, 100, 100))
                .unwrap_err();
            assert!(error.contains("cannot fit"));
            assert!(
                error.contains("frame_before=")
                    && error.contains("work_area=")
                    && error.contains("monitor_bounds=")
            );
        }
        assert!(
            work_area_adjustment(
                rect(0, 0, 0, 100),
                rect(0, 0, 100, 100),
                rect(0, 0, 100, 100)
            )
            .is_err()
        );
        assert!(
            work_area_adjustment(
                rect(0, 0, 10, 10),
                rect(-1, 0, 100, 100),
                rect(0, 0, 100, 100)
            )
            .is_err()
        );
        assert!(
            work_area_adjustment(
                rect(i32::MIN, 0, i32::MIN + 10, 10),
                rect(i32::MAX - 10, 0, i32::MAX, 10),
                rect(i32::MAX - 10, 0, i32::MAX, 10),
            )
            .is_err(),
            "delta must be representable for the caller's native physical position"
        );
    }

    #[test]
    fn rectangle_union_matches_independent_pixel_oracle_with_negative_coordinates_and_overlap() {
        let target = rect(-3, -2, 4, 4);
        let mut seed = 0x42ab_ef01_u32;
        for _ in 0..512 {
            let mut covers = Vec::new();
            for _ in 0..4 {
                let mut next = || {
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    seed
                };
                let left = (next() % 10) as i32 - 5;
                let top = (next() % 9) as i32 - 4;
                let width = (next() % 5 + 1) as i32;
                let height = (next() % 5 + 1) as i32;
                covers.push(rect(left, top, left + width, top + height));
            }
            let mut expected = 0;
            for y in target.top..target.bottom {
                for x in target.left..target.right {
                    if !covers.iter().any(|cover| {
                        x >= cover.left && x < cover.right && y >= cover.top && y < cover.bottom
                    }) {
                        expected += 1;
                    }
                }
            }
            assert_eq!(uncovered_pixels(target, &covers, 128), Some(expected));
            covers.reverse();
            covers.push(covers[0]);
            assert_eq!(
                uncovered_pixels(target, &covers, 128),
                Some(expected),
                "union must not double-count overlapping monitors"
            );
        }
    }

    #[test]
    fn desktop_union_rejects_multimonitor_gaps_and_taskbar_workarea_strips() {
        let frame = rect(-120, 0, 120, 100);
        let client = rect(-110, 10, 110, 90);
        let with_gap = [rect(-120, 0, 0, 100), rect(20, 0, 120, 100)];
        let gap = measure_desktop_coverage(frame, client, &with_gap, &with_gap, true);
        assert!(gap.enumeration_complete && gap.calculation_complete);
        assert_eq!(gap.frame_uncovered_monitor_pixels, 2000);
        assert_eq!(gap.client_uncovered_monitor_pixels, 1600);
        assert!(
            !gap.necessary_checks_pass(),
            "the virtual desktop outer bounds contain an undisplayed gap"
        );
        let adjacent = [rect(-120, 0, 0, 100), rect(0, 0, 120, 100)];
        let full = measure_desktop_coverage(frame, client, &adjacent, &adjacent, true);
        assert!(full.necessary_checks_pass());
        let work = [rect(-120, 0, 0, 90), rect(0, 0, 120, 90)];
        let taskbar = measure_desktop_coverage(frame, client, &adjacent, &work, true);
        assert_eq!(taskbar.frame_uncovered_monitor_pixels, 0);
        assert_eq!(taskbar.frame_uncovered_work_area_pixels, 2400);
        assert_eq!(taskbar.client_uncovered_work_area_pixels, 0);
        assert!(!taskbar.necessary_checks_pass());
        let edge = measure_desktop_coverage(rect(-120, 0, 120, 90), client, &adjacent, &work, true);
        assert!(
            edge.necessary_checks_pass(),
            "exact right/bottom workarea edges are valid half-open bounds"
        );
    }

    #[test]
    fn coverage_rejects_incomplete_enumeration_degenerate_bounds_and_subtraction_limits() {
        let frame = rect(0, 0, 10, 10);
        let monitors = [frame];
        assert!(
            !measure_desktop_coverage(frame, frame, &monitors, &monitors, false)
                .necessary_checks_pass()
        );
        assert!(!measure_desktop_coverage(frame, frame, &[], &[], true).necessary_checks_pass());
        assert!(
            !measure_desktop_coverage(frame, frame, &monitors, &[rect(-1, 0, 10, 10)], true)
                .necessary_checks_pass()
        );
        assert!(
            !measure_desktop_coverage(rect(0, 0, 0, 10), frame, &monitors, &monitors, true)
                .necessary_checks_pass()
        );
        let excessive = [frame; MAX_DESKTOP_MONITORS + 1];
        assert!(
            !measure_desktop_coverage(frame, frame, &excessive, &excessive, true)
                .necessary_checks_pass()
        );
        assert_eq!(uncovered_pixels(frame, &[rect(4, 4, 6, 6)], 3), None);
        assert_eq!(uncovered_pixels(frame, &[rect(4, 4, 6, 6)], 4), Some(96));
        assert_eq!(uncovered_pixels(frame, &[], 0), None);
        assert_eq!(uncovered_pixels(frame, &[rect(1, 1, 1, 2)], 10), None);
        assert_eq!(
            rect(i32::MIN, i32::MIN, i32::MAX, i32::MAX).area(),
            Some(u64::from(u32::MAX).pow(2))
        );
    }

    #[test]
    fn bounded_occluder_report_retains_metadata_and_marks_a_truncated_class() {
        let mut candidate = OccluderCandidate {
            process_id: 123,
            recognized_pointer: false,
            client_intersection: rect(0, 10, 20, 30),
            ..Default::default()
        };
        candidate.class_name_utf16[0..5].copy_from_slice(&[35, 51, 50, 55, 54]);
        candidate.class_name_units = 5;
        candidate.class_name_truncated = true;
        candidate.class_name_hash = 99;
        candidate.identity_read_complete = true;
        let report = candidate.report();
        assert_eq!(report.class_name, "#3276");
        assert!(report.class_name_truncated);
        assert_eq!(report.client_intersection, rect(0, 10, 20, 30));
        assert_eq!(report.process_id, 123);
        assert!(!report.recognized_pointer);
    }

    fn pointer_fixture() -> (PointerProcessIdentity, PointerOverlayAttributes) {
        (
            PointerProcessIdentity {
                process_id: 42,
                creation_time_100ns: 132_456_789,
                canonical_image: format!(r"\\?\c:\owned\runtime{SKY_IMAGE_SUFFIX}"),
            },
            PointerOverlayAttributes {
                process_id: 42,
                class_name: "CodexComputerUseCursorOverlay".into(),
                style: 0x9400_0000,
                extended_style: 0x0808_00a8,
                alpha: 255,
                color_key: 0,
                layered_flags: 3,
            },
        )
    }

    #[test]
    fn pointer_exception_requires_explicit_policy_and_every_exact_window_attribute() {
        let (identity, attributes) = pointer_fixture();
        let policy = PointerOverlayPolicy::from_identity(identity.clone()).unwrap();
        assert!(is_recognized_pointer_overlay(
            Some(&policy),
            Some(&identity),
            Some(&attributes)
        ));
        assert!(!is_recognized_pointer_overlay(
            None,
            Some(&identity),
            Some(&attributes)
        ));
        assert!(!is_recognized_pointer_overlay(
            Some(&policy),
            None,
            Some(&attributes)
        ));
        assert!(!is_recognized_pointer_overlay(
            Some(&policy),
            Some(&identity),
            None
        ));
        for field in 0..7 {
            let mut changed = attributes.clone();
            match field {
                0 => changed.process_id += 1,
                1 => changed.class_name.push('X'),
                2 => changed.style ^= 1,
                3 => changed.extended_style ^= 1,
                4 => changed.alpha -= 1,
                5 => changed.color_key += 1,
                6 => changed.layered_flags ^= 1,
                _ => unreachable!(),
            }
            assert!(
                !is_recognized_pointer_overlay(Some(&policy), Some(&identity), Some(&changed)),
                "changed field {field}"
            );
        }
    }

    #[test]
    fn pointer_exception_rejects_recycled_pid_changed_image_and_failed_identity_queries() {
        let (identity, attributes) = pointer_fixture();
        let policy = PointerOverlayPolicy::from_identity(identity.clone()).unwrap();
        for field in 0..3 {
            let mut changed = identity.clone();
            match field {
                0 => changed.process_id += 1,
                1 => changed.creation_time_100ns += 1,
                2 => changed.canonical_image = format!(r"\\?\c:\different{SKY_IMAGE_SUFFIX}"),
                _ => unreachable!(),
            }
            assert!(!is_recognized_pointer_overlay(
                Some(&policy),
                Some(&changed),
                Some(&attributes)
            ));
        }
        let failed_query: Result<PointerProcessIdentity, String> = Err("access denied".into());
        assert!(!is_recognized_pointer_overlay(
            Some(&policy),
            failed_query.as_ref().ok(),
            Some(&attributes)
        ));
        let report = policy.report();
        assert_eq!(report.process_id, identity.process_id);
        assert_eq!(report.creation_time_100ns, identity.creation_time_100ns);
        assert_eq!(report.verified_image_suffix, SKY_IMAGE_SUFFIX);
        assert_ne!(report.image_path_hash, 0);
        assert!(!format!("{report:?}").contains("owned"));
    }

    #[test]
    fn pointer_policy_rejects_similar_image_names_or_unusable_process_identity() {
        let (identity, _) = pointer_fixture();
        for bad in [
            PointerProcessIdentity {
                process_id: 0,
                ..identity.clone()
            },
            PointerProcessIdentity {
                creation_time_100ns: 0,
                ..identity.clone()
            },
            PointerProcessIdentity {
                canonical_image: format!("{}.bak", identity.canonical_image),
                ..identity.clone()
            },
            PointerProcessIdentity {
                canonical_image: r"c:\other\codex-computer-use.exe".into(),
                ..identity.clone()
            },
        ] {
            assert!(PointerOverlayPolicy::from_identity(bad).is_err());
        }
    }
}
