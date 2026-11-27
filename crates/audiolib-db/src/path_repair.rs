// crates/audiolib-db/src/path_repair.rs
//! Read-only discovery followed by one separately authorized, atomic path update.
use super::*;
use audiolib_core::path_repair::{Candidate, MatchKind, RepairPreview, RepairRequest, RepairRow};
use std::collections::{HashMap, HashSet};

fn invalid(text: &str) -> Error {
    Error::Validation(text.into())
}
fn checkpoint(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Acquire) {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

#[derive(PartialEq)]
struct Properties {
    bytes: u64,
    rate: u32,
    channels: u16,
    bits: u16,
    duration: f64,
}
impl Properties {
    fn matches(&self, other: &Self) -> bool {
        self.bytes == other.bytes
            && self.rate == other.rate
            && self.channels == other.channels
            && self.bits == other.bits
            && (self.duration - other.duration).abs() <= 0.000001
    }
}
struct Record {
    id: i64,
    path: String,
    filename: String,
    properties: Properties,
}
const RECORD: &str = "SELECT af.id, af.file_path, af.filename, COALESCE(af.file_size,0), COALESCE(af.sample_rate,0), COALESCE(af.channels,0), COALESCE(af.bit_depth,0), COALESCE(af.duration,0.0) FROM audio_files af";
fn record(row: &Row<'_>) -> rusqlite::Result<Record> {
    Ok(Record {
        id: row.get(0)?,
        path: row.get(1)?,
        filename: row.get(2)?,
        properties: Properties {
            bytes: unsigned(row, 3)?,
            rate: row.get(4)?,
            channels: row.get(5)?,
            bits: row.get(6)?,
            duration: row.get(7)?,
        },
    })
}
fn modified_ns(metadata: &fs::Metadata) -> Result<i64> {
    metadata
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|time| time.as_nanos().try_into().ok())
        .ok_or_else(|| invalid("Invalid file modification time"))
}
fn probe(path: &Path, root: &Path) -> Result<(Candidate, Properties)> {
    let real = audiolib_core::validate_audio_path(path, &[root.to_owned()])?;
    if real != path {
        return Err(invalid("Target path changed or traverses a link"));
    }
    let before = fs::metadata(&real)?;
    let audio = audiolib_audio::probe(&real).map_err(|e| Error::Audio(e.to_string()))?;
    let after = fs::metadata(&real)?;
    if before.len() != after.len() || before.modified()? != after.modified()? {
        return Err(invalid("Audio changed during matching"));
    }
    Ok((
        Candidate {
            path: real,
            size: after.len(),
            modified_ns: modified_ns(&after)?,
        },
        Properties {
            bytes: after.len(),
            rate: audio.sample_rate,
            channels: audio.channels,
            bits: audio.bit_depth,
            duration: audio.duration,
        },
    ))
}
fn old_missing(path: &str) -> Result<bool> {
    if !Path::new(path).is_absolute() || (cfg!(windows) != path_identity::windows_path(path)) {
        return Ok(true);
    }
    match fs::metadata(path) {
        Ok(m) => Ok(!m.is_file()),
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound
                    | std::io::ErrorKind::NotADirectory
                    | std::io::ErrorKind::NetworkUnreachable
                    | std::io::ErrorKind::HostUnreachable
            ) =>
        {
            Ok(true)
        }
        #[cfg(windows)]
        Err(e) if matches!(e.raw_os_error(), Some(15 | 21 | 53 | 67 | 1203 | 2250)) => Ok(true),
        Err(e) => Err(e.into()),
    }
}
fn linked(m: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        m.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        m.file_type().is_symlink()
    }
}
fn issue(preview: &mut RepairPreview, path: &Path, error: impl std::fmt::Display) {
    preview.issue_count += 1;
    if preview.issues.len() < 32 {
        preview.issues.push(format!("{}: {error}", path.display()));
    }
}
fn inventory(
    preview: &mut RepairPreview,
    cancel: &AtomicBool,
) -> Result<HashMap<String, Vec<PathBuf>>> {
    let mut names: HashMap<String, Vec<PathBuf>> = HashMap::new();
    let mut dirs = vec![preview.request.target.clone()];
    while let Some(dir) = dirs.pop() {
        checkpoint(cancel)?;
        match dir.canonicalize() {
            Ok(real) if real == dir && real.starts_with(&preview.request.target) => {}
            Ok(_) => {
                issue(preview, &dir, "Directory changed during matching");
                continue;
            }
            Err(e) => {
                issue(preview, &dir, e);
                continue;
            }
        }
        let entries = match fs::read_dir(&dir) {
            Ok(v) => v,
            Err(e) => {
                issue(preview, &dir, e);
                continue;
            }
        };
        for entry in entries {
            checkpoint(cancel)?;
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    issue(preview, &dir, e);
                    continue;
                }
            };
            let path = entry.path();
            let meta = match fs::symlink_metadata(&path) {
                Ok(m) => m,
                Err(e) => {
                    issue(preview, &path, e);
                    continue;
                }
            };
            if linked(&meta) {
                continue;
            }
            if meta.is_dir() {
                dirs.push(path);
            } else if meta.is_file() && audiolib_core::is_supported_audio_extension(&path) {
                names
                    .entry(entry.file_name().to_string_lossy().to_lowercase())
                    .or_default()
                    .push(path);
            }
        }
    }
    for paths in names.values_mut() {
        paths.sort_unstable();
    }
    Ok(names)
}
/// Use the spelling stored in SQLite even when the old volume is unavailable.
fn relative_target(source: &str, original: &str, root: &Path) -> Option<PathBuf> {
    let source = path_identity::comparison_key(source);
    let original = path_identity::comparison_key(original);
    let suffix = original
        .strip_prefix(source.trim_end_matches('/'))?
        .strip_prefix('/')?;
    let mut wanted = root.to_owned();
    for name in suffix.split('/') {
        if !matches!(
            Path::new(name).components().collect::<Vec<_>>().as_slice(),
            [std::path::Component::Normal(_)]
        ) {
            return None;
        }
        wanted.push(name);
    }
    Some(wanted)
}
struct DiskFile {
    candidate: Candidate,
    properties: Properties,
    owner: Option<i64>,
}

struct MatchGroup {
    candidates: Arc<[Candidate]>,
    inaccessible: bool,
    occupied: bool,
    uses: u8,
}

#[derive(Hash, PartialEq, Eq)]
struct GroupKey {
    name: String,
    bytes: u64,
    rate: u32,
    channels: u16,
    bits: u16,
    duration_bits: u64,
}

#[derive(Default)]
struct MatchIndex {
    files: HashMap<PathBuf, Option<DiskFile>>,
    groups: HashMap<GroupKey, MatchGroup>,
}

impl MatchIndex {
    fn file(
        &mut self,
        conn: &Connection,
        path: &Path,
        preview: &mut RepairPreview,
    ) -> Result<Option<&DiskFile>> {
        use std::collections::hash_map::Entry;
        let entry = match self.files.entry(path.to_owned()) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let file = match probe(path, &preview.request.target) {
                    Ok((candidate, properties)) => Some(DiskFile {
                        candidate,
                        properties,
                        owner: path_identity::audio_id(conn, &display_path(path), true)?,
                    }),
                    Err(e) => {
                        issue(preview, path, e);
                        None
                    }
                };
                entry.insert(file)
            }
        };
        Ok(entry.as_ref())
    }

    fn group(
        &mut self,
        conn: &Connection,
        name: &str,
        paths: &[PathBuf],
        props: &Properties,
        preview: &mut RepairPreview,
        cancel: &AtomicBool,
    ) -> Result<&mut MatchGroup> {
        let key = GroupKey {
            name: name.into(),
            bytes: props.bytes,
            rate: props.rate,
            channels: props.channels,
            bits: props.bits,
            duration_bits: props.duration.to_bits(),
        };
        if !self.groups.contains_key(&key) {
            let mut candidates = Vec::new();
            let mut inaccessible = false;
            let mut occupied = false;
            for path in paths {
                checkpoint(cancel)?;
                let Some(file) = self.file(conn, path, preview)? else {
                    inaccessible = true;
                    continue;
                };
                if props.matches(&file.properties) {
                    if file.owner.is_some() {
                        occupied = true;
                    } else {
                        candidates.push(file.candidate.clone());
                    }
                }
            }
            let group = MatchGroup {
                candidates: candidates.into(),
                inaccessible,
                occupied,
                uses: 0,
            };
            return Ok(self.groups.entry(key).or_insert(group));
        }
        Ok(self.groups.get_mut(&key).expect("existing candidate group"))
    }
}

impl SqliteProvider {
    pub(super) fn preview_replacements(
        &self,
        request: &RepairRequest,
        allowed: &[PathBuf],
        cancel: &AtomicBool,
    ) -> Result<RepairPreview> {
        self.require_writable()?;
        checkpoint(cancel)?;
        let target = audiolib_core::validate_scan_root(&request.target, allowed)?;
        let conn = self.reader()?;
        let folder = path_identity::folder_id(&conn, &request.source)?
            .ok_or_else(|| invalid("Select an indexed source directory"))?;
        let mut preview = RepairPreview {
            session: self.cursor_session.to_string(),
            library_namespace: albums::namespace(&conn)?,
            request: RepairRequest {
                target: target.clone(),
                ..request.clone()
            },
            rows: vec![],
            issue_count: 0,
            issues: vec![],
        };
        let names = inventory(&mut preview, cancel)?;
        let incomplete = preview.issue_count != 0;
        let mut matches = MatchIndex::default();
        let mut claims = HashMap::new();
        let sql = format!(
            "{RECORD} JOIN folder_closure fc ON fc.descendant_id=af.folder_id WHERE fc.ancestor_id=?1 ORDER BY af.id"
        );
        let mut stmt = conn.prepare(&sql).map_err(db_error)?;
        for audio in stmt.query_map([folder], record).map_err(db_error)? {
            checkpoint(cancel)?;
            let audio = audio.map_err(db_error)?;
            let mut row = RepairRow {
                id: audio.id,
                original: audio.path,
                kind: MatchKind::Missing,
                candidates: Arc::default(),
            };
            if request.missing_only {
                match old_missing(&row.original) {
                    Ok(false) => continue,
                    Ok(true) => {}
                    Err(error) => {
                        issue(&mut preview, Path::new(&row.original), error);
                        row.kind = MatchKind::Inaccessible;
                        preview.rows.push(row);
                        continue;
                    }
                }
            }
            let desired = relative_target(&request.source, &row.original, &target);
            let name = audio.filename.to_lowercase();
            let paths = names.get(&name).map(Vec::as_slice).unwrap_or_default();
            let exact = desired
                .as_ref()
                .and_then(|p| paths.binary_search(p).ok())
                .map(|i| &paths[i]);
            if let Some(exact) = exact
                && let Some(file) = matches.file(&conn, exact, &mut preview)?
                && audio.properties.matches(&file.properties)
            {
                if file.owner == Some(row.id) {
                    continue;
                }
                if file.owner.is_some() {
                    row.kind = MatchKind::Occupied;
                } else {
                    row.candidates = Arc::new([file.candidate.clone()]);
                    row.kind = MatchKind::Relative;
                    let count = claims.entry(exact.clone()).or_insert(0u8);
                    *count = count.saturating_add(1);
                }
            } else {
                let group =
                    matches.group(&conn, &name, paths, &audio.properties, &mut preview, cancel)?;
                row.candidates = group.candidates.clone();
                row.kind = match row.candidates.len() {
                    0 if group.occupied => MatchKind::Occupied,
                    0 if group.inaccessible || incomplete => MatchKind::Inaccessible,
                    0 => MatchKind::Missing,
                    1 if !group.inaccessible && !group.occupied && !incomplete => {
                        MatchKind::Filename
                    }
                    _ => MatchKind::Ambiguous,
                };
                // The same set may serve thousands of identical filenames and audio formats.
                // For conflicts it suffices to distinguish one claimant from two or more.
                if group.uses < 2 {
                    group.uses += 1;
                    for c in row.candidates.iter() {
                        let n = claims.entry(c.path.clone()).or_insert(0u8);
                        *n = n.saturating_add(1);
                    }
                }
            }
            preview.rows.push(row);
        }
        for row in &mut preview.rows {
            if row.candidates.iter().any(|c| claims[&c.path] > 1) {
                row.kind = MatchKind::Ambiguous;
            }
        }
        Ok(preview)
    }
    pub(super) fn commit_replacements(
        &self,
        preview: &RepairPreview,
        choices: &[(usize, usize)],
        allowed: &[PathBuf],
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(usize),
    ) -> Result<BackupReport> {
        checkpoint(cancel)?;
        self.require_writable()?;
        if preview.session != self.cursor_session.to_string() || choices.is_empty() {
            return Err(invalid("Invalid or expired path selection"));
        }
        let _scan = management::LibraryLease::acquire(&self.path)?;
        let _admin = management::AdministrationLease::exclusive(&self.path)?;
        self.require_writable()?;
        let root = audiolib_core::validate_scan_root(&preview.request.target, allowed)?;
        if root != preview.request.target {
            return Err(invalid("Target directory changed"));
        }
        let mut conn = self.writer_unlocked()?;
        if albums::namespace(&conn)? != preview.library_namespace {
            return Err(invalid("Library identity changed; preview again"));
        }
        let folder = path_identity::folder_id(&conn, &preview.request.source)?
            .ok_or_else(|| invalid("Source folder changed"))?;
        let backup = self
            .path
            .with_file_name("backups")
            .join(format!("before-path-repair-{}.db", uuid::Uuid::new_v4()));
        let report = self.backup_library_cancellable(&backup, cancel)?;
        let tx = conn.transaction().map_err(db_error)?;
        let mut ids = HashSet::new();
        let mut targets = HashSet::new();
        let mut changed_folders = HashSet::new();
        let sql = format!(
            "{RECORD} JOIN folder_closure fc ON fc.descendant_id=af.folder_id WHERE af.id=?1 AND fc.ancestor_id=?2"
        );
        for &(row_index, candidate_index) in choices {
            checkpoint(cancel)?;
            let row = preview
                .rows
                .get(row_index)
                .ok_or_else(|| invalid("Unknown preview row"))?;
            let candidate = row
                .candidates
                .get(candidate_index)
                .ok_or_else(|| invalid("Unknown preview candidate"))?;
            let audio = tx
                .query_row(&sql, params![row.id, folder], record)
                .map_err(db_error)?;
            let text = display_path(&candidate.path);
            if !ids.insert(row.id)
                || !targets.insert(&candidate.path)
                || audio.path != row.original
                || (preview.request.missing_only && !old_missing(&audio.path)?)
                || path_identity::audio_id(&tx, &text, true)?.is_some()
            {
                return Err(invalid(
                    "Record or target changed; preview the matches again",
                ));
            }
            let (now, properties) = probe(&candidate.path, &root)?;
            if !audio.properties.matches(&properties)
                || now.modified_ns != candidate.modified_ns
                || now.size != candidate.size
            {
                return Err(invalid("Candidate changed; preview the matches again"));
            }
            let old_folder: i64 = tx
                .query_row(
                    "SELECT folder_id FROM audio_files WHERE id=?1",
                    [row.id],
                    |r| r.get(0),
                )
                .map_err(db_error)?;
            let parent = candidate
                .path
                .parent()
                .ok_or_else(|| invalid("Candidate has no parent"))?;
            let new_folder = ensure_folder(&tx, parent, &root)?;
            let filename = candidate
                .path
                .file_name()
                .ok_or_else(|| invalid("Candidate has no filename"))?
                .to_string_lossy();
            tx.execute("UPDATE audio_files SET file_path=?2,path_key=?3,filename=?4,folder_id=?5,missing=0,source_mtime_ns=?6,modified_at=strftime('%Y-%m-%d %H:%M:%f','now') WHERE id=?1",
                params![row.id,text,path_key(&text),filename,new_folder,now.modified_ns]).map_err(db_error)?;
            tx.execute(
                "DELETE FROM waveform_cache WHERE audio_file_id=?1",
                [row.id],
            )
            .map_err(db_error)?;
            changed_folders.extend([old_folder, new_folder]);
            progress(ids.len());
        }
        dir_search::maintain(&tx, &[], &changed_folders.into_iter().collect::<Vec<_>>())?;
        refresh_folder_statistics(&tx)?;
        // A previous candidate may have moved while later files were probed on a slow disk.
        for &(row, choice) in choices {
            checkpoint(cancel)?;
            let candidate = &preview.rows[row].candidates[choice];
            let actual = candidate.path.canonicalize()?;
            let metadata = fs::metadata(&actual)?;
            if actual != candidate.path
                || metadata.len() != candidate.size
                || modified_ns(&metadata)? != candidate.modified_ns
            {
                return Err(invalid("Candidate changed before commit; preview again"));
            }
        }
        checkpoint(cancel)?;
        tx.commit().map_err(db_error)?;
        Ok(report)
    }
}
