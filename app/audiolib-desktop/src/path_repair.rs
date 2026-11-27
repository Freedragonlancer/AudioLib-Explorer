// app/audiolib-desktop/src/path_repair.rs
//! A paged, explicit selection over one worker-produced local path preview.
use super::*;
use audiolib_core::path_repair::{MatchKind, RepairPreview, RepairRequest};

const PAGE: usize = 12;
pub(super) struct Review {
    preview: Arc<RepairPreview>,
    selected: Vec<bool>,
    candidate: Vec<usize>,
    page: usize,
    counts: [usize; 5],
    selected_paths: HashSet<PathBuf>,
}
impl Review {
    pub(super) fn new(preview: RepairPreview) -> Self {
        let mut counts = [0; 5];
        let selected: Vec<_> = preview
            .rows
            .iter()
            .map(|r| {
                let group = match r.kind {
                    MatchKind::Relative | MatchKind::Filename => 0,
                    MatchKind::Ambiguous => 1,
                    MatchKind::Missing => 2,
                    MatchKind::Occupied => 3,
                    MatchKind::Inaccessible => 4,
                };
                counts[group] += 1;
                group == 0
            })
            .collect();
        let selected_paths = preview
            .rows
            .iter()
            .zip(&selected)
            .filter(|(_, yes)| **yes)
            .map(|(row, _)| row.candidates[0].path.clone())
            .collect();
        Self {
            candidate: vec![0; selected.len()],
            page: 0,
            counts,
            selected_paths,
            selected,
            preview: Arc::new(preview),
        }
    }
    fn choices(&self) -> Vec<(usize, usize)> {
        self.selected
            .iter()
            .enumerate()
            .filter(|(_, selected)| **selected)
            .map(|(i, _)| (i, self.candidate[i]))
            .collect()
    }
}
impl Controller {
    pub(super) fn bind_path_repair(self: &Arc<Self>, ui: &AppWindow) {
        let c = self.clone();
        ui.on_repair_start(move |source, whole| {
            if c.remote_read_only() || !c.available() || source.is_empty() {
                return;
            }
            let source = source.to_string();
            c.choose_file(false, None, move |target| {
                LibraryOperation::PreviewPaths(RepairRequest {
                    source,
                    target,
                    missing_only: !whole,
                })
            });
        });
        let c = self.clone();
        ui.on_repair_cancel(move || {
            c.repair_cancel.store(true, Ordering::Release);
        });
        let c = self.clone();
        ui.on_repair_dismiss(move || {
            if c.available()
                && let Some(ui) = c.ui.upgrade()
            {
                *c.path_review.lock().unwrap() = None;
                c.publish_path_review(&ui);
                ui.set_repair_whole(false);
            }
        });
        let c = self.clone();
        ui.on_repair_page(move |delta| {
            if !c.available() {
                return;
            }
            if let Some(review) = c.path_review.lock().unwrap().as_mut() {
                review.page = review
                    .page
                    .saturating_add_signed(delta as isize)
                    .min(review.preview.rows.len().saturating_sub(1) / PAGE);
            }
            if let Some(ui) = c.ui.upgrade() {
                c.publish_path_review(&ui);
            }
        });
        let c = self.clone();
        ui.on_repair_choice(move |index, delta| {
            if !c.available() {
                return;
            }
            {
                let mut state = c.path_review.lock().unwrap();
                let Some(review) = state.as_mut() else {
                    return;
                };
                let Ok(offset) = usize::try_from(index) else {
                    return;
                };
                let i = review.page * PAGE + offset;
                let Some(row) = review.preview.rows.get(i) else {
                    return;
                };
                if row.candidates.is_empty() {
                    return;
                }
                let next = review.candidate[i]
                    .saturating_add_signed(delta as isize)
                    .min(row.candidates.len() - 1);
                let was_selected = review.selected[i];
                let selected = if delta == 0 {
                    !was_selected
                } else {
                    was_selected
                };
                let path = &row.candidates[next].path;
                let current = &row.candidates[review.candidate[i]].path;
                if selected
                    && (!was_selected || current != path)
                    && review.selected_paths.contains(path)
                {
                    if let Some(ui) = c.ui.upgrade() {
                        ui.set_library_feedback(c.localized(
                            "另一条记录已选择此文件，请先取消原选择。",
                            "Another record has selected this file. Clear its selection first.",
                        ));
                    }
                } else {
                    if was_selected {
                        review.selected_paths.remove(current);
                    }
                    if selected {
                        review.selected_paths.insert(path.clone());
                    }
                    review.selected[i] = selected;
                    review.candidate[i] = next;
                }
            }
            if let Some(ui) = c.ui.upgrade() {
                c.publish_path_review(&ui);
            }
        });
        let c = self.clone();
        ui.on_repair_apply(move || {
            if c.remote_read_only() || !c.available() {
                return;
            }
            let operation = c
                .path_review
                .lock()
                .unwrap()
                .as_ref()
                .filter(|r| !r.selected_paths.is_empty())
                .map(|r| LibraryOperation::ApplyPaths(r.preview.clone(), r.choices()));
            if let Some(op) = operation {
                c.run_library_operation(op);
            }
        });
    }
    pub(super) fn publish_path_review(&self, ui: &AppWindow) {
        let state = self.path_review.lock().unwrap();
        ui.set_repair_reviewing(state.is_some());
        let Some(review) = state.as_ref() else {
            ui.set_repair_rows(ModelRc::default());
            ui.set_repair_summary("".into());
            return;
        };
        let cn = self.chinese.load(Ordering::Acquire);
        let rows = &review.preview.rows;
        let begin = review.page * PAGE;
        let end = (begin + PAGE).min(rows.len());
        let [unique, multiple, missing, occupied, denied] = review.counts;
        let selected = review.selected_paths.len();
        let mut summary = if cn {
            format!(
                "唯一匹配 {unique} · 多候选或待确认 {multiple} · 未找到 {missing} · 已占用 {occupied} · 无法访问 {denied} · 已选 {selected}"
            )
        } else {
            format!(
                "Unique {unique} · Review {multiple} · Missing {missing} · Occupied {occupied} · Inaccessible {denied} · Selected {selected}"
            )
        };
        let preview = &review.preview;
        summary = format!(
            "{} → {}\n{summary}",
            preview.request.source,
            preview.request.target.display()
        );
        if preview.issue_count > 0 {
            let label = if cn { "读取问题" } else { "Read issues" };
            summary.push_str(&format!(
                "\n{label} ({}): {}",
                preview.issue_count,
                preview.issues.join("\n")
            ));
        }
        ui.set_repair_summary(summary.into());
        ui.set_repair_range(
            format!(
                "{}–{} / {}",
                if rows.is_empty() { 0 } else { begin + 1 },
                end,
                rows.len()
            )
            .into(),
        );
        ui.set_repair_previous(begin > 0);
        ui.set_repair_next(end < rows.len());
        ui.set_repair_can_apply(selected > 0);
        let model = rows[begin..end]
            .iter()
            .enumerate()
            .map(|(offset, row)| {
                let i = begin + offset;
                let index = review.candidate[i];
                let reason = match (row.kind, cn) {
                    (MatchKind::Relative, true) => "相对路径、大小及音频属性一致",
                    (MatchKind::Relative, false) => "Relative path, size and audio properties",
                    (MatchKind::Filename, true) => "文件名、大小及音频属性一致",
                    (MatchKind::Filename, false) => "Filename, size and audio properties",
                    (MatchKind::Ambiguous, true) => "请确认候选项",
                    (MatchKind::Ambiguous, false) => "Review candidate",
                    (MatchKind::Missing, true) => "未找到",
                    (MatchKind::Missing, false) => "Missing",
                    (MatchKind::Occupied, true) => "目标已被占用",
                    (MatchKind::Occupied, false) => "Target already indexed",
                    (MatchKind::Inaccessible, true) => "无法读取",
                    (MatchKind::Inaccessible, false) => "Inaccessible",
                };
                crate::PathMatchRow {
                    original: row.original.clone().into(),
                    target: row
                        .candidates
                        .get(index)
                        .map(|c| c.path.display().to_string())
                        .unwrap_or_default()
                        .into(),
                    reason: if row.candidates.len() > 1 {
                        format!("{reason} ({}/{})", index + 1, row.candidates.len())
                    } else {
                        reason.into()
                    }
                    .into(),
                    chosen: review.selected[i],
                    available: !row.candidates.is_empty(),
                    previous: index > 0,
                    next: index + 1 < row.candidates.len(),
                }
            })
            .collect::<Vec<_>>();
        ui.set_repair_rows(ModelRc::new(VecModel::from(model)));
    }
}
