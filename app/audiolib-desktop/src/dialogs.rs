// app/audiolib-desktop/src/dialogs.rs
//! Native file selection runs on a worker; no subprocess or extra runtime is required.
use std::path::PathBuf;

/// Select one filesystem folder with the Windows common dialog on a worker thread.
/// Cancellation returns `None` so the caller can retain the existing typed path.
#[cfg(windows)]
pub fn choose_folder(chinese: bool) -> Result<Option<PathBuf>, String> {
    use std::{ffi::c_void, os::windows::ffi::OsStringExt, ptr};

    #[repr(C)]
    struct Guid {
        data1: u32,
        data2: u16,
        data3: u16,
        data4: [u8; 8],
    }
    #[repr(C)]
    struct UnknownVTable {
        unused: [usize; 2],
        release: unsafe extern "system" fn(*mut c_void) -> u32,
    }
    #[repr(C)]
    struct DialogVTable {
        unknown: UnknownVTable,
        show: unsafe extern "system" fn(*mut c_void, *mut c_void) -> i32,
        before_options: [usize; 5],
        set_options: unsafe extern "system" fn(*mut c_void, u32) -> i32,
        get_options: unsafe extern "system" fn(*mut c_void, *mut u32) -> i32,
        before_title: [usize; 6],
        set_title: unsafe extern "system" fn(*mut c_void, *const u16) -> i32,
        before_result: [usize; 2],
        get_result: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32,
    }
    #[repr(C)]
    struct ItemVTable {
        unknown: UnknownVTable,
        before_name: [usize; 2],
        get_display_name: unsafe extern "system" fn(*mut c_void, u32, *mut *mut u16) -> i32,
    }
    #[link(name = "ole32")]
    unsafe extern "system" {
        fn CoInitializeEx(reserved: *mut c_void, flags: u32) -> i32;
        fn CoUninitialize();
        fn CoCreateInstance(
            class: *const Guid,
            outer: *mut c_void,
            context: u32,
            interface: *const Guid,
            object: *mut *mut c_void,
        ) -> i32;
        fn CoTaskMemFree(memory: *mut c_void);
    }
    struct Apartment;
    impl Drop for Apartment {
        fn drop(&mut self) {
            // SAFETY: The guard exists only after successful COM initialization on
            // this worker, and all interface guards have already been released.
            unsafe { CoUninitialize() };
        }
    }
    struct Object(*mut c_void);
    impl Drop for Object {
        fn drop(&mut self) {
            // SAFETY: Each successful COM output owns one reference; the first
            // three vtable entries use IUnknown's SDK layout for both interfaces.
            unsafe {
                let table = &**(self.0 as *const *const UnknownVTable);
                (table.release)(self.0);
            }
        }
    }
    struct TaskString(*mut u16);
    impl Drop for TaskString {
        fn drop(&mut self) {
            // SAFETY: IShellItem::GetDisplayName allocates this output using COM's
            // task allocator; the guard owns the returned allocation exactly once.
            unsafe { CoTaskMemFree(self.0.cast()) };
        }
    }
    let check = |result: i32| {
        if result < 0 {
            Err(format!(
                "Windows folder dialog failed: {:#x}",
                result as u32
            ))
        } else {
            Ok(())
        }
    };
    let class = Guid {
        data1: 0xdc1c5a9c,
        data2: 0xe88a,
        data3: 0x4dde,
        data4: [0xa5, 0xa1, 0x60, 0xf8, 0x2a, 0x20, 0xae, 0xf7],
    };
    let interface = Guid {
        data1: 0x42f85136,
        data2: 0xdb7e,
        data3: 0x439c,
        data4: [0x85, 0xf1, 0xe4, 0x07, 0x5d, 0x13, 0x5f, 0xc8],
    };
    let title = if chinese {
        "选择音效文件夹"
    } else {
        "Choose a sound folder"
    }
    .encode_utf16()
    .chain(Some(0))
    .collect::<Vec<_>>();
    // SAFETY: This fresh worker initializes an STA; no interface crosses threads.
    // GUIDs and vtable prefixes exactly match IFileDialog/IShellItem in the Windows
    // SDK. All output buffers, strings and owned references live through each call.
    unsafe {
        check(CoInitializeEx(ptr::null_mut(), 2))?;
        let _apartment = Apartment;
        let mut raw_dialog = ptr::null_mut();
        check(CoCreateInstance(
            &class,
            ptr::null_mut(),
            1,
            &interface,
            &mut raw_dialog,
        ))?;
        if raw_dialog.is_null() {
            return Err("Windows folder dialog returned no interface".into());
        }
        let dialog = Object(raw_dialog);
        let table = &**(dialog.0 as *const *const DialogVTable);
        let mut options = 0;
        check((table.get_options)(dialog.0, &mut options))?;
        // FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST.
        check((table.set_options)(dialog.0, options | 0x20 | 0x40 | 0x800))?;
        check((table.set_title)(dialog.0, title.as_ptr()))?;
        let result = (table.show)(dialog.0, ptr::null_mut());
        if result as u32 == 0x8007_04c7 {
            return Ok(None);
        }
        check(result)?;
        let mut raw_item = ptr::null_mut();
        check((table.get_result)(dialog.0, &mut raw_item))?;
        if raw_item.is_null() {
            return Err("Windows folder dialog returned no folder".into());
        }
        let item = Object(raw_item);
        let item_table = &**(item.0 as *const *const ItemVTable);
        let mut raw_name = ptr::null_mut();
        // SIGDN_FILESYSPATH returns an allocated, null-terminated UTF-16 path.
        check((item_table.get_display_name)(
            item.0,
            0x8005_8000,
            &mut raw_name,
        ))?;
        if raw_name.is_null() {
            return Err("Windows folder dialog returned no filesystem path".into());
        }
        let name = TaskString(raw_name);
        let mut length = 0;
        while *name.0.add(length) != 0 {
            length += 1;
        }
        let path = std::ffi::OsString::from_wide(std::slice::from_raw_parts(name.0, length));
        Ok(Some(PathBuf::from(path)))
    }
}

/// Folder selection is available only on the Windows desktop target.
#[cfg(not(windows))]
pub fn choose_folder(_chinese: bool) -> Result<Option<PathBuf>, String> {
    Err("Native folder selection requires Windows".into())
}

#[derive(Clone, Copy)]
pub enum FileKind {
    Database,
    Wav,
    JsonLines,
    Csv,
    Annotations,
}

#[cfg(windows)]
pub fn choose(save: bool, kind: FileKind, chinese: bool) -> Result<Option<PathBuf>, String> {
    use std::{ffi::c_void, mem::size_of};
    #[repr(C)]
    struct OpenFileName {
        size: u32,
        owner: *mut c_void,
        instance: *mut c_void,
        filter: *const u16,
        custom_filter: *mut u16,
        custom_size: u32,
        filter_index: u32,
        file: *mut u16,
        file_size: u32,
        title_file: *mut u16,
        title_file_size: u32,
        initial_dir: *const u16,
        title: *const u16,
        flags: u32,
        file_offset: u16,
        extension_offset: u16,
        default_extension: *const u16,
        custom_data: isize,
        hook: *const c_void,
        template: *const u16,
        reserved: *mut c_void,
        reserved_word: u32,
        flags_ex: u32,
    }
    #[link(name = "comdlg32")]
    unsafe extern "system" {
        fn GetOpenFileNameW(value: *mut OpenFileName) -> i32;
        fn GetSaveFileNameW(value: *mut OpenFileName) -> i32;
        fn CommDlgExtendedError() -> u32;
    }
    let (filter, ext) = match kind {
        FileKind::Database => ("SQLite database (*.db)\0*.db\0All files\0*.*\0\0", "db"),
        FileKind::Wav => ("WAV / BWF audio\0*.wav\0\0", "wav"),
        FileKind::JsonLines => ("JSON Lines (*.jsonl)\0*.jsonl\0\0", "jsonl"),
        FileKind::Csv => ("CSV (*.csv)\0*.csv\0\0", "csv"),
        FileKind::Annotations => (
            "Library annotations\0*.jsonl;*.csv\0All files\0*.*\0\0",
            "jsonl",
        ),
    };
    let filter = filter.encode_utf16().collect::<Vec<_>>();
    let ext = ext.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let title = (if chinese {
        if save { "保存文件" } else { "选择文件" }
    } else if save {
        "Save file"
    } else {
        "Select file"
    })
    .encode_utf16()
    .chain(Some(0))
    .collect::<Vec<_>>();
    let mut file = vec![0u16; 32_768];
    // SAFETY: The Win32 POD is zero-initializable. All pointed buffers remain alive for
    // the synchronous call, writable file capacity equals file_size, and no hook exists.
    let mut value: OpenFileName = unsafe { std::mem::zeroed() };
    value.size = size_of::<OpenFileName>() as u32;
    value.filter = filter.as_ptr();
    value.filter_index = 1;
    value.file = file.as_mut_ptr();
    value.file_size = file.len() as u32;
    value.title = title.as_ptr();
    value.default_extension = ext.as_ptr();
    value.flags =
        0x0008_0000 | 0x0000_0008 | 0x0000_0800 | if save { 0x0000_0002 } else { 0x0000_1000 };
    // SAFETY: OPENFILENAMEW layout follows the Windows SDK, buffers and length fields
    // above are valid, and this worker owns the entire dialog lifetime.
    let accepted = unsafe {
        if save {
            GetSaveFileNameW(&mut value)
        } else {
            GetOpenFileNameW(&mut value)
        }
    };
    if accepted == 0 {
        // SAFETY: This thread made the immediately preceding common-dialog call.
        let error = unsafe { CommDlgExtendedError() };
        return if error == 0 {
            Ok(None)
        } else {
            Err(format!("Windows file dialog failed: {error:#x}"))
        };
    }
    let end = file
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(file.len());
    Ok(Some(PathBuf::from(String::from_utf16_lossy(&file[..end]))))
}

#[cfg(not(windows))]
pub fn choose(_save: bool, _kind: FileKind, _chinese: bool) -> Result<Option<PathBuf>, String> {
    Err("Native file selection requires Windows".into())
}
