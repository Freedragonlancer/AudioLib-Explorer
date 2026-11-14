// crates/audiolib-windows/src/lib.rs
//! Windows driver discovery and native diagnostics; no drivers are installed here.
use audiolib_core::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

mod documents;
mod drag;
pub use documents::documents_directory;
pub use drag::{DragOutcome, drag_files, primary_button_down};

/// Main-thread interception of native macOS Quit through the desktop close drain.
#[cfg(target_os = "macos")]
pub mod macos_lifecycle;

/// Explicit diagnostic capture of this process's own GUI completion marker.
/// Normal application use does not start capture; codec helpers are cross-platform.
pub mod gui_capture;

/// Opt-in own-window geometry history on the HWND's existing Windows UI thread.
#[cfg(windows)]
pub mod gui_geometry;

/// Native exposure record for the desktop window's partial software repaint.
#[cfg(windows)]
pub mod paint_exposure;

/// Display a fatal startup diagnostic for a desktop application without a console.
/// Headless callers should write their diagnostics directly and skip this dialog.
pub fn show_error_dialog(message: &str) {
    #[cfg(windows)]
    {
        #[link(name = "user32")]
        unsafe extern "system" {
            fn MessageBoxW(
                window: *mut std::ffi::c_void,
                text: *const u16,
                caption: *const u16,
                flags: u32,
            ) -> i32;
        }
        let text = message
            .replace('\0', " ")
            .encode_utf16()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let caption = "AudioLib Explorer\0".encode_utf16().collect::<Vec<_>>();
        // SAFETY: Both UTF-16 buffers are NUL-terminated and remain alive during the
        // synchronous call. A null owner is valid for a fatal startup diagnostic.
        unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), caption.as_ptr(), 0x10) };
    }
    #[cfg(target_os = "macos")]
    {
        // The system helper owns its UI thread. Diagnostic text stays in argv,
        // never AppleScript source or shell commands, and status reaps the child.
        const SCRIPT: &str = r#"on run argv
    display alert (item 1 of argv) message (item 2 of argv) as critical buttons {"OK"} default button "OK"
end run"#;
        let text = message.replace('\0', " ");
        match std::process::Command::new("/usr/bin/osascript")
            .arg("-e")
            .arg(SCRIPT)
            .args(["AudioLib Explorer", &text])
            .status()
        {
            Ok(status) if status.success() => {}
            Ok(status) => {
                eprintln!("AudioLib Explorer: {message}\nmacOS error dialog failed ({status})");
            }
            Err(error) => {
                eprintln!("AudioLib Explorer: {message}\nmacOS error dialog failed: {error}");
            }
        }
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    eprintln!("AudioLib Explorer: {message}");
}

/// Put the calling thread into background processing mode (lowest CPU scheduling plus
/// very low I/O and memory priority) for the rest of its life. Intended for optional
/// background analysis such as list Waveforms, so it never competes with audition
/// decoding or the audio device. A no-op elsewhere or if Windows refuses.
pub fn enter_background_thread_mode() {
    #[cfg(windows)]
    {
        const THREAD_MODE_BACKGROUND_BEGIN: i32 = 0x0001_0000;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetCurrentThread() -> *mut std::ffi::c_void;
            fn SetThreadPriority(thread: *mut std::ffi::c_void, priority: i32) -> i32;
        }
        // SAFETY: GetCurrentThread returns a pseudo handle for the calling thread that
        // needs no closing; SetThreadPriority only changes this thread's own scheduling
        // and accepts THREAD_MODE_BACKGROUND_BEGIN for the current thread. Failure is
        // harmless and ignored.
        unsafe {
            SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN);
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AsioDriver {
    pub name: String,
    pub aliases: Vec<String>,
    pub clsid: String,
    pub dll_path: Option<PathBuf>,
    pub available: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AsioBufferRange {
    pub min: u32,
    pub max: u32,
    pub preferred: u32,
    pub granularity: i32,
}
impl AsioBufferRange {
    /// SDK granularity -1 means powers of two, 0 means continuous, >0 means steps from min.
    pub fn supports(&self, frames: u32) -> bool {
        frames >= self.min
            && frames <= self.max
            && match self.granularity {
                -1 => frames.is_power_of_two(),
                0 => true,
                step if step > 0 => (frames - self.min).is_multiple_of(step as u32),
                _ => false,
            }
    }
}

#[cfg(all(windows, feature = "asio"))]
unsafe extern "C" {
    fn audiolib_asio_buffer_range(
        min: *mut i32,
        max: *mut i32,
        preferred: *mut i32,
        granularity: *mut i32,
    ) -> i32;
    fn audiolib_asio_channel_name(channel: i32, name: *mut std::ffi::c_char) -> i32;
    fn audiolib_asio_control_panel() -> i32;
}

/// Query an already initialized CPAL ASIO driver from its serialized control thread.
/// The caller must retain the CPAL device/stream; this helper never loads a second driver.
/// # Safety
/// Hold an initialized ASIO driver through a live CPAL iterator/stream and serialize
/// driver lifecycle/control operations against all other calls into this global SDK.
pub unsafe fn loaded_asio_buffer_range() -> Result<AsioBufferRange> {
    #[cfg(all(windows, feature = "asio"))]
    {
        let (mut min, mut max, mut preferred, mut granularity) = (0, 0, 0, 0);
        // SAFETY: Four initialized i32 slots match Windows SDK long; they remain valid for
        // the synchronous call. The caller guarantees a retained driver and serialization.
        let error = unsafe {
            audiolib_asio_buffer_range(&mut min, &mut max, &mut preferred, &mut granularity)
        };
        if error != 0 || min <= 0 || max < min || preferred <= 0 {
            return Err(audiolib_core::Error::Audio(format!(
                "ASIO buffer query failed ({error})"
            )));
        }
        Ok(AsioBufferRange {
            min: min as u32,
            max: max as u32,
            preferred: preferred as u32,
            granularity,
        })
    }
    #[cfg(not(all(windows, feature = "asio")))]
    {
        Err(audiolib_core::Error::Audio(
            "ASIO is not enabled in this build".into(),
        ))
    }
}
/// Get a loaded driver's output channel labels, without creating any audio buffers.
/// # Safety
/// Retain the initialized driver and serialize SDK lifecycle/control calls. `channels`
/// must be the output count reported by that same driver.
pub unsafe fn loaded_asio_channel_names(channels: u16) -> Result<Vec<String>> {
    #[cfg(all(windows, feature = "asio"))]
    {
        (0..channels)
            .map(|channel| {
                let mut name = [0_i8; 32];
                // SAFETY: The initialized 32-char buffer is writable for the synchronous
                // shim call. The caller retains the driver and serializes SDK access.
                let error =
                    unsafe { audiolib_asio_channel_name(i32::from(channel), name.as_mut_ptr()) };
                if error != 0 {
                    return Err(audiolib_core::Error::Audio(format!(
                        "ASIO channel query failed ({error})"
                    )));
                }
                // SAFETY: The shim always sets byte 31 to NUL on success, so CStr cannot
                // read beyond this initialized 32-byte array; it outlives the conversion.
                Ok(unsafe { std::ffi::CStr::from_ptr(name.as_ptr()) }
                    .to_string_lossy()
                    .into_owned())
            })
            .collect()
    }
    #[cfg(not(all(windows, feature = "asio")))]
    {
        let _ = channels;
        Err(audiolib_core::Error::Audio(
            "ASIO is not enabled in this build".into(),
        ))
    }
}
/// Ask the currently loaded ASIO driver to open its own control panel.
/// # Safety
/// Retain the initialized driver and serialize SDK lifecycle/control operations; never
/// call from a real-time callback. The driver owns its window and any reset notification.
pub unsafe fn loaded_asio_control_panel() -> Result<()> {
    #[cfg(all(windows, feature = "asio"))]
    {
        // SAFETY: There are no Rust pointers in this synchronous FFI call. The caller
        // guarantees a retained global driver and serialized control-thread execution.
        let error = unsafe { audiolib_asio_control_panel() };
        if error != 0 {
            return Err(audiolib_core::Error::Audio(format!(
                "ASIO control panel failed ({error})"
            )));
        }
        Ok(())
    }
    #[cfg(not(all(windows, feature = "asio")))]
    {
        Err(audiolib_core::Error::Audio(
            "ASIO is not enabled in this build".into(),
        ))
    }
}

/// Enumerate x64 ASIO registrations, deduplicating aliases by COM CLSID.
pub fn asio_drivers() -> Result<Vec<AsioDriver>> {
    #[cfg(windows)]
    {
        use std::collections::BTreeMap;
        use winreg::{
            RegKey,
            enums::{HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY},
        };
        let machine = RegKey::predef(HKEY_LOCAL_MACHINE);
        let root =
            match machine.open_subkey_with_flags("SOFTWARE\\ASIO", KEY_READ | KEY_WOW64_64KEY) {
                Ok(key) => key,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
                Err(error) => return Err(error.into()),
            };
        let mut drivers = BTreeMap::<String, AsioDriver>::new();
        for name in root.enum_keys().filter_map(std::result::Result::ok) {
            let key = root.open_subkey_with_flags(&name, KEY_READ | KEY_WOW64_64KEY)?;
            let clsid: String = match key.get_value("CLSID") {
                Ok(value) => value,
                Err(_) => continue,
            };
            let identity = clsid.to_ascii_uppercase();
            if let Some(driver) = drivers.get_mut(&identity) {
                driver.aliases.push(name);
                continue;
            }
            let server = format!("SOFTWARE\\Classes\\CLSID\\{}\\InprocServer32", clsid);
            let dll_path = machine
                .open_subkey_with_flags(server, KEY_READ | KEY_WOW64_64KEY)
                .ok()
                .and_then(|key| key.get_value::<String, _>("").ok())
                .map(PathBuf::from);
            let available = dll_path.as_ref().is_some_and(|path| path.is_file());
            drivers.insert(
                identity,
                AsioDriver {
                    name,
                    aliases: Vec::new(),
                    clsid,
                    dll_path,
                    available,
                },
            );
        }
        Ok(drivers.into_values().collect())
    }
    #[cfg(not(windows))]
    {
        Ok(Vec::new())
    }
}

/// Encrypt a small secret for the current Windows user with DPAPI.
/// Other platforms return the bytes unchanged; callers must still restrict file access.
pub fn protect_user_secret(plain: &[u8]) -> Result<Vec<u8>> {
    #[cfg(windows)]
    {
        dpapi::transform(plain, true)
    }
    #[cfg(not(windows))]
    {
        Ok(plain.to_vec())
    }
}

/// Decrypt bytes produced by [`protect_user_secret`] for the same Windows user.
pub fn unprotect_user_secret(protected: &[u8]) -> Result<Vec<u8>> {
    #[cfg(windows)]
    {
        dpapi::transform(protected, false)
    }
    #[cfg(not(windows))]
    {
        Ok(protected.to_vec())
    }
}

/// True when [`protect_user_secret`] really encrypts (Windows DPAPI).
pub const fn user_secret_protection_available() -> bool {
    cfg!(windows)
}

#[cfg(windows)]
mod dpapi {
    use audiolib_core::{Error, Result};
    use std::ffi::c_void;

    #[repr(C)]
    struct DataBlob {
        size: u32,
        data: *mut u8,
    }
    #[link(name = "crypt32")]
    unsafe extern "system" {
        fn CryptProtectData(
            input: *const DataBlob,
            description: *const u16,
            entropy: *const DataBlob,
            reserved: *mut c_void,
            prompt: *mut c_void,
            flags: u32,
            output: *mut DataBlob,
        ) -> i32;
        fn CryptUnprotectData(
            input: *const DataBlob,
            description: *mut *mut u16,
            entropy: *const DataBlob,
            reserved: *mut c_void,
            prompt: *mut c_void,
            flags: u32,
            output: *mut DataBlob,
        ) -> i32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LocalFree(memory: *mut c_void) -> *mut c_void;
    }

    /// Run one DPAPI call. CRYPTPROTECT_UI_FORBIDDEN keeps it non-interactive.
    pub(super) fn transform(input: &[u8], protect: bool) -> Result<Vec<u8>> {
        let size = u32::try_from(input.len())
            .map_err(|_| Error::Validation("Secret is too large".into()))?;
        let blob = DataBlob {
            size,
            data: input.as_ptr().cast_mut(),
        };
        let mut output = DataBlob {
            size: 0,
            data: std::ptr::null_mut(),
        };
        // SAFETY: DataBlob matches the Windows SDK DATA_BLOB layout. The input buffer
        // outlives the synchronous call and DPAPI never writes through pDataIn. Null
        // description/entropy/prompt pointers are documented as optional. On success the
        // output allocation is owned by us and released exactly once with LocalFree.
        unsafe {
            let ok = if protect {
                CryptProtectData(
                    &blob,
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    0x1,
                    &mut output,
                )
            } else {
                CryptUnprotectData(
                    &blob,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    0x1,
                    &mut output,
                )
            };
            if ok == 0 {
                return Err(Error::Io(std::io::Error::last_os_error()));
            }
            let result = if output.data.is_null() {
                Vec::new()
            } else {
                std::slice::from_raw_parts(output.data, output.size as usize).to_vec()
            };
            if !output.data.is_null() {
                LocalFree(output.data.cast());
            }
            Ok(result)
        }
    }
}

/// Absolute audio file Explorer or Finder should select.
///
/// The process working directory and the executable directory are never
/// substitutes. A relative, empty, or non-local path is rejected so Explorer
/// cannot fall back to the application folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevealLocation {
    /// Normalized absolute path of the audio file.
    pub file: String,
    /// Absolute folder that contains [`Self::file`].
    pub folder: String,
}

/// Why a stored path must not be handed to Explorer or Finder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RevealPathError {
    /// Blank text. Explorer would otherwise open the process directory.
    Empty,
    /// Relative, or a server path that is not local on this computer.
    /// Resolving it would use the process directory.
    NotAbsolute,
    /// Absolute, but it has no parent folder and filename to select.
    NoParent,
}

/// Path syntax of the computer that would open Explorer or Finder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HostPaths {
    Windows,
    Posix,
}

/// Classify `path` for the computer this process is running on.
pub fn reveal_location(path: &str) -> std::result::Result<RevealLocation, RevealPathError> {
    reveal_location_for(path, host_paths())
}

fn reveal_location_for(
    path: &str,
    host: HostPaths,
) -> std::result::Result<RevealLocation, RevealPathError> {
    let absolute = normalize_local_path(path, host)?;
    let (folder, _name) = split_parent(&absolute).ok_or(RevealPathError::NoParent)?;
    Ok(RevealLocation {
        file: absolute,
        folder,
    })
}

/// Absolute local path, or why Explorer must not receive it.
/// `host` selects Windows versus POSIX rules so both can be tested on one machine.
fn normalize_local_path(
    path: &str,
    host: HostPaths,
) -> std::result::Result<String, RevealPathError> {
    let path = path.trim();
    if path.is_empty() {
        return Err(RevealPathError::Empty);
    }
    if let Some(windows) = windows_absolute(path) {
        return Ok(trim_trailing_separator(&windows));
    }
    if let Some(posix) = posix_absolute(path) {
        // A server snapshot path such as `/Volumes/...` is not a Windows file.
        // Treating it as relative would open the application directory.
        if host == HostPaths::Windows {
            return Err(RevealPathError::NotAbsolute);
        }
        return Ok(trim_trailing_separator(&posix));
    }
    Err(RevealPathError::NotAbsolute)
}

fn host_paths() -> HostPaths {
    if cfg!(windows) {
        HostPaths::Windows
    } else {
        HostPaths::Posix
    }
}

fn trim_trailing_separator(path: &str) -> String {
    if path.len() <= 1 || path.ends_with(':') {
        return path.to_owned();
    }
    // Keep `C:\` and `/`. Drop a trailing separator from every other path.
    let root = (path.len() == 3 && path.as_bytes().get(1) == Some(&b':')) || path == "/";
    if root {
        return path.to_owned();
    }
    path.trim_end_matches(['\\', '/']).to_owned()
}

/// Normalize a Windows drive or UNC path to backslashes, or return `None`.
fn windows_absolute(value: &str) -> Option<String> {
    let slash = value.replace('/', "\\");
    let native = if let Some(rest) = slash.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = slash.strip_prefix(r"\\?\") {
        rest.to_owned()
    } else {
        slash
    };
    let bytes = native.as_bytes();
    let drive =
        bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\';
    let unc = native.starts_with(r"\\") && native[2..].contains('\\');
    (drive || unc).then_some(native)
}

fn posix_absolute(value: &str) -> Option<String> {
    value.starts_with('/').then(|| value.to_owned())
}

fn split_parent(path: &str) -> Option<(String, String)> {
    if path.starts_with('/') && !path.starts_with("//") {
        split_posix_parent(path)
    } else {
        split_windows_parent(path)
    }
}

fn split_windows_parent(path: &str) -> Option<(String, String)> {
    let slash = path.rfind('\\')?;
    let name = &path[slash + 1..];
    if name.is_empty() {
        return None;
    }
    let mut folder = path[..slash].to_owned();
    if folder.len() == 2 && folder.as_bytes()[1] == b':' {
        folder.push('\\');
    }
    // `\\host` has no share, so it is not a folder that can contain a file.
    if folder.starts_with(r"\\") && !folder[2..].contains('\\') {
        return None;
    }
    if folder.is_empty() {
        return None;
    }
    Some((folder, name.to_owned()))
}

fn split_posix_parent(path: &str) -> Option<(String, String)> {
    let slash = path.rfind('/')?;
    let name = &path[slash + 1..];
    if name.is_empty() {
        return None;
    }
    let folder = if slash == 0 {
        "/".to_owned()
    } else {
        path[..slash].to_owned()
    };
    Some((folder, name.to_owned()))
}

fn reveal_error(error: RevealPathError) -> audiolib_core::Error {
    audiolib_core::Error::Validation(match error {
        RevealPathError::Empty => "Cannot reveal an empty path. Explorer was not opened.".into(),
        RevealPathError::NotAbsolute => {
            "The audio path is not a full local path, so Explorer was not opened.".into()
        }
        RevealPathError::NoParent => "Cannot reveal a path without a parent folder.".into(),
    })
}

/// Select an existing audio file in Explorer or Finder.
///
/// Relative paths are rejected before any filesystem or shell call, so they
/// cannot resolve against the application directory.
pub fn reveal_path(path: &std::path::Path) -> Result<()> {
    let location = reveal_location(&path.to_string_lossy()).map_err(reveal_error)?;
    let file = std::path::PathBuf::from(&location.file);
    if !file.is_file() {
        return Err(audiolib_core::Error::NotFound(location.file));
    }
    open_reveal(&location)
}

/// Open an existing local folder. A relative path is not launched.
pub fn reveal_folder(path: &std::path::Path) -> Result<()> {
    let absolute =
        normalize_local_path(&path.to_string_lossy(), host_paths()).map_err(reveal_error)?;
    let folder = std::path::PathBuf::from(&absolute);
    if !folder.is_dir() {
        return Err(audiolib_core::Error::NotFound(absolute));
    }
    open_folder(&folder)
}

fn open_reveal(location: &RevealLocation) -> Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::System::Com::{
            COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize,
        };
        use windows::Win32::UI::Shell::{
            ILFindLastID, SHOpenFolderAndSelectItems, SHParseDisplayName,
        };

        // The shell API takes a parent PIDL and the selected child's relative PIDL.
        // `explorer.exe /select,<path>` parses spaces, commas and slashes itself and
        // opens the process directory when that parse fails.
        let folder_name: Vec<u16> = std::ffi::OsStr::new(&location.folder)
            .encode_wide()
            .chain(Some(0))
            .collect();
        let item_name: Vec<u16> = std::ffi::OsStr::new(&location.file)
            .encode_wide()
            .chain(Some(0))
            .collect();
        // SAFETY: This worker thread owns the COM apartment until every shell PIDL
        // has been released. Both UTF-16 buffers are NUL-terminated and live for
        // the synchronous shell calls below.
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED)
                .ok()
                .map_err(|error| std::io::Error::other(error.to_string()))?;
            let mut folder = std::ptr::null_mut();
            let mut item = std::ptr::null_mut();
            let result = (|| {
                SHParseDisplayName(
                    windows::core::PCWSTR(folder_name.as_ptr()),
                    None,
                    &mut folder,
                    0,
                    None,
                )?;
                SHParseDisplayName(
                    windows::core::PCWSTR(item_name.as_ptr()),
                    None,
                    &mut item,
                    0,
                    None,
                )?;
                let child = ILFindLastID(item);
                SHOpenFolderAndSelectItems(folder, Some(&[child.cast_const()]), 0)
            })();
            CoTaskMemFree(Some(item.cast()));
            CoTaskMemFree(Some(folder.cast()));
            CoUninitialize();
            result.map_err(|error| std::io::Error::other(error.to_string()))?;
        }
    }
    #[cfg(target_os = "macos")]
    {
        let file = std::path::PathBuf::from(&location.file);
        let status = std::process::Command::new("/usr/bin/open")
            .arg("-R")
            .arg(file.canonicalize().unwrap_or(file))
            .status()?;
        if !status.success() {
            return Err(std::io::Error::other(format!("Finder reveal failed ({status})")).into());
        }
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = location;
    }
    Ok(())
}

fn open_folder(folder: &std::path::Path) -> Result<()> {
    #[cfg(windows)]
    {
        std::process::Command::new("explorer.exe")
            .arg(folder)
            .spawn()?;
    }
    #[cfg(target_os = "macos")]
    {
        let status = std::process::Command::new("/usr/bin/open")
            .arg(
                folder
                    .canonicalize()
                    .unwrap_or_else(|_| folder.to_path_buf()),
            )
            .status()?;
        if !status.success() {
            return Err(
                std::io::Error::other(format!("Finder folder open failed ({status})")).into(),
            );
        }
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = folder;
    }
    Ok(())
}

/// Whether this process owns the foreground window. `None` means the owner is
/// temporarily unknown or this platform has no supported activation check.
pub fn foreground_application_is_current() -> Option<bool> {
    #[cfg(windows)]
    {
        use windows::Win32::UI::WindowsAndMessaging::{
            GetForegroundWindow, GetWindowThreadProcessId,
        };
        // SAFETY: Both calls only inspect the current foreground HWND. The PID
        // pointer is valid for the duration of GetWindowThreadProcessId.
        unsafe {
            let window = GetForegroundWindow();
            if window.0.is_null() {
                return None;
            }
            let mut process_id = 0;
            if GetWindowThreadProcessId(window, Some(&mut process_id)) == 0 || process_id == 0 {
                return None;
            }
            Some(process_id == std::process::id())
        }
    }
    #[cfg(target_os = "macos")]
    {
        // NSRunningApplication is thread safe and reports app-level activation,
        // so this app's own sheets and dialogs remain foreground activity.
        Some(objc2_app_kit::NSRunningApplication::currentApplication().isActive())
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use audiolib_core::DataProvider;

    fn process_directory() -> String {
        std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn spaced_windows_path_selects_the_audio_file_not_the_process_directory() {
        let location =
            reveal_location_for(r"D:\Sound Library\door hit.wav", HostPaths::Windows).unwrap();
        assert_eq!(location.file, r"D:\Sound Library\door hit.wav");
        assert_eq!(location.folder, r"D:\Sound Library");
        assert_ne!(location.folder, process_directory());
        assert!(!location.file.contains("/select"));

        let slashes =
            reveal_location_for("D:/Sound Library/door hit.wav", HostPaths::Windows).unwrap();
        assert_eq!(slashes, location);

        let verbatim =
            reveal_location_for(r"\\?\D:\Sound Library\door hit.wav", HostPaths::Windows).unwrap();
        assert_eq!(verbatim, location);
    }

    #[test]
    fn unc_file_selects_its_share_folder() {
        let location =
            reveal_location_for(r"\\nas\share\SFX\door.wav", HostPaths::Windows).unwrap();
        assert_eq!(location.file, r"\\nas\share\SFX\door.wav");
        assert_eq!(location.folder, r"\\nas\share\SFX");
        assert_ne!(location.folder, process_directory());
    }

    #[test]
    fn relative_and_empty_paths_are_not_resolved_against_the_process_directory() {
        for path in [
            "",
            "   ",
            ".",
            "..",
            "door.wav",
            r".\door.wav",
            r"sounds\door.wav",
            "sounds/door.wav",
            "C:door.wav",
        ] {
            let error = reveal_location_for(path, HostPaths::Windows).unwrap_err();
            assert_ne!(
                error,
                RevealPathError::NoParent,
                "{path} must not become the process directory"
            );
            assert!(reveal_path(std::path::Path::new(path)).is_err());
        }
    }

    #[test]
    fn server_snapshot_path_is_not_a_windows_application_folder() {
        let server = "/Volumes/SFX/door hit.wav";
        assert_eq!(
            reveal_location_for(server, HostPaths::Windows).unwrap_err(),
            RevealPathError::NotAbsolute
        );
        let local = reveal_location_for(server, HostPaths::Posix).unwrap();
        assert_eq!(local.file, server);
        assert_eq!(local.folder, "/Volumes/SFX");
        assert_ne!(local.folder, process_directory());
    }

    #[test]
    fn existing_absolute_file_is_revealed_from_its_own_folder() {
        let root = std::env::temp_dir().join(format!(
            "audiolib-reveal-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("library")).unwrap();
        let file = root.join("library").join("door hit.wav");
        std::fs::write(&file, b"wav").unwrap();
        let text = file.to_string_lossy().into_owned();
        let location = reveal_location(&text).unwrap();
        assert_eq!(location.file, text);
        assert!(location.folder.ends_with("library") || location.folder.ends_with("library/"));
        assert_ne!(location.folder, process_directory());
        assert!(reveal_path(&file).is_ok());
        assert!(reveal_folder(file.parent().unwrap()).is_ok());
        assert!(reveal_path(std::path::Path::new("door hit.wav")).is_err());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn scanned_library_file_reveals_its_folder_not_the_process_directory() {
        let root = std::env::temp_dir().join(format!(
            "audiolib-reveal-library-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let library = root.join("library");
        let folder = library.join("doors");
        std::fs::create_dir_all(&folder).unwrap();
        let source = folder.join("door hit.wav");
        let mut wav = Vec::new();
        wav.extend(b"RIFF");
        wav.extend(38u32.to_le_bytes());
        wav.extend(b"WAVEfmt ");
        wav.extend(16u32.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(48_000u32.to_le_bytes());
        wav.extend(96_000u32.to_le_bytes());
        wav.extend(2u16.to_le_bytes());
        wav.extend(16u16.to_le_bytes());
        wav.extend(b"data");
        wav.extend(2u32.to_le_bytes());
        wav.extend(0i16.to_le_bytes());
        std::fs::write(&source, wav).unwrap();
        let database = root.join("library.db");
        let provider = std::sync::Arc::new(audiolib_db::SqliteProvider::open(&database).unwrap());
        audiolib_db::ScanService::new(provider)
            .scan(
                &library,
                &audiolib_db::ScanOptions::default(),
                &audiolib_db::ScanCancellation::default(),
                |_| {},
            )
            .unwrap();
        let provider = audiolib_db::SqliteProvider::open(&database).unwrap();
        let stored = provider
            .search(&audiolib_core::SearchRequest::default())
            .unwrap()
            .results
            .into_iter()
            .next()
            .unwrap();
        let location = reveal_location(&stored.file_path).unwrap();
        let expected = source.canonicalize().unwrap();
        assert_eq!(
            std::path::PathBuf::from(&location.file)
                .canonicalize()
                .unwrap(),
            expected
        );
        assert_eq!(
            std::path::PathBuf::from(&location.folder)
                .canonicalize()
                .unwrap(),
            expected.parent().unwrap()
        );
        assert_ne!(location.folder, process_directory());
        assert!(reveal_location(stored.filename.as_str()).is_err());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn background_mode_applies_only_to_its_own_thread() {
        std::thread::spawn(enter_background_thread_mode)
            .join()
            .unwrap();
    }
    #[test]
    fn user_secret_round_trips_and_is_not_plaintext_on_windows() {
        let secret = b"token-\xe4\xb8\xad-123";
        let protected = protect_user_secret(secret).unwrap();
        if user_secret_protection_available() {
            assert_ne!(protected.as_slice(), secret.as_slice());
        }
        assert_eq!(unprotect_user_secret(&protected).unwrap(), secret);
        assert!(unprotect_user_secret(b"not a dpapi blob").is_err() || !cfg!(windows));
    }

    #[test]
    fn sdk_buffer_granularity() {
        let powers = AsioBufferRange {
            min: 32,
            max: 1024,
            preferred: 256,
            granularity: -1,
        };
        assert!(powers.supports(64));
        assert!(!powers.supports(48));
        let stepped = AsioBufferRange {
            min: 48,
            max: 1024,
            preferred: 240,
            granularity: 48,
        };
        assert!(stepped.supports(240));
        assert!(!stepped.supports(256));
        let continuous = AsioBufferRange {
            granularity: 0,
            ..powers
        };
        assert!(continuous.supports(257));
        assert!(!continuous.supports(2048));
    }
}
