// server/audiolib-server/src/vendor_import/server_path.rs
//! Read-only, bounded table discovery inside an explicitly configured server root.
use super::{MAX_FILES, upload_error};
use crate::{ApiError, AppState};
use audiolib_core::{
    DataProvider,
    vendor_import::{VendorImportOptions, VendorImportPreview},
};
use serde::Deserialize;
use std::{
    collections::BTreeSet,
    fs,
    path::{Component, Path, PathBuf},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ServerPathPreview {
    root: String,
    root_relative_dir: String,
    #[serde(default)]
    options: VendorImportOptions,
}

fn relative(raw: &str) -> Result<PathBuf, ApiError> {
    if raw.contains('\0') || raw.contains(':') || raw.starts_with(['/', '\\']) {
        return Err(upload_error(
            "root_relative_dir must remain inside the named root",
        ));
    }
    let normalized = raw.replace('\\', "/");
    let path = Path::new(&normalized);
    if path
        .components()
        .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        return Err(upload_error(
            "root_relative_dir must not contain parent or absolute components",
        ));
    }
    Ok(path.to_owned())
}

fn discover(root: &Path, directory: &Path) -> Result<Vec<PathBuf>, ApiError> {
    let mut pending = vec![directory.to_owned()];
    let mut visited = BTreeSet::new();
    let mut files = Vec::new();
    let mut examined = 0_usize;
    while let Some(directory) = pending.pop() {
        let directory = audiolib_core::validate_scan_root(&directory, &[root.to_owned()])?;
        if !visited.insert(directory.clone()) {
            continue;
        }
        for entry in fs::read_dir(directory).map_err(audiolib_core::Error::from)? {
            examined += 1;
            if examined > 100_000 {
                return Err(upload_error(
                    "Server table discovery exceeds 100,000 directory entries; select a smaller directory",
                ));
            }
            let entry = entry.map_err(audiolib_core::Error::from)?;
            let kind = entry.file_type().map_err(audiolib_core::Error::from)?;
            // Do not traverse links from a selected table directory. The configured
            // root itself was resolved and validated before enumeration.
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                pending.push(entry.path());
                continue;
            }
            if !kind.is_file() {
                continue;
            }
            let path = entry.path();
            if !path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|extension| {
                    ["csv", "tsv", "txt", "xls", "xlsx"]
                        .iter()
                        .any(|known| extension.eq_ignore_ascii_case(known))
                })
            {
                continue;
            }
            let canonical = path.canonicalize().map_err(audiolib_core::Error::from)?;
            if !canonical.starts_with(root) {
                return Err(upload_error("Vendor table escaped its configured root"));
            }
            files.push(canonical);
            if files.len() > MAX_FILES {
                return Err(upload_error(
                    "One preview accepts at most 512 tables; select a smaller directory",
                ));
            }
        }
    }
    files.sort();
    if files.is_empty() {
        return Err(upload_error(
            "No supported vendor tables in the selected directory",
        ));
    }
    Ok(files)
}

pub(super) async fn preview(
    state: AppState,
    mut input: ServerPathPreview,
) -> Result<VendorImportPreview, ApiError> {
    let relative = relative(&input.root_relative_dir)?;
    let monitor = state.root_monitor();
    let configured = monitor
        .roots()
        .iter()
        .find(|root| root.name == input.root)
        .ok_or_else(|| upload_error("Unknown configured vendor root"))?
        .server_path
        .clone();
    tokio::task::spawn_blocking(move || {
        let root = configured
            .canonicalize()
            .map_err(audiolib_core::Error::from)?;
        let directory =
            audiolib_core::validate_scan_root(&root.join(relative), std::slice::from_ref(&root))?;
        let files = discover(&root, &directory)?;
        if input.options.vendor_root.is_none() {
            input.options.vendor_root = Some(audiolib_db::display_path(&root));
        }
        state
            .provider()
            .vendor_import_preview_files(&files, &input.options)
            .map_err(ApiError::from)
    })
    .await
    .map_err(|error| ApiError::internal(format!("Vendor table discovery failed: {error}")))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_paths_reject_cross_platform_escape_before_filesystem_access() {
        for path in [
            "../private",
            "nested/../../private",
            "/private",
            "\\private",
            "C:\\private",
            "C:private",
            "a\0b",
        ] {
            assert!(relative(path).is_err(), "{path:?}");
        }
        assert_eq!(
            relative("tables\\sub").unwrap(),
            PathBuf::from("tables/sub")
        );
        assert!(relative("").is_ok());
    }

    #[test]
    fn table_discovery_is_bounded_and_reads_only_selected_regular_tables() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let selected = root.join("tables");
        fs::create_dir_all(selected.join("nested")).unwrap();
        fs::write(
            selected.join("first.csv.xls"),
            "filename,description\na.wav,wind\n",
        )
        .unwrap();
        fs::write(selected.join("nested/second.XLSX"), "fixture").unwrap();
        fs::write(selected.join("source.wav"), "untouched").unwrap();
        assert_eq!(discover(&root, &selected).unwrap().len(), 2);
        for index in 0..MAX_FILES {
            fs::write(selected.join(format!("extra-{index}.csv")), "x").unwrap();
        }
        assert!(discover(&root, &selected).is_err());
        assert_eq!(
            fs::read_to_string(selected.join("source.wav")).unwrap(),
            "untouched"
        );
    }
}
