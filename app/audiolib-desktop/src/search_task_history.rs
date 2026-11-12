// app/audiolib-desktop/src/search_task_history.rs
//! Bounded session-local navigation through one task's executed search conditions.
use super::{SearchTask, SearchTaskDefinition, SearchTaskView, same_search_definition};
use std::time::{Duration, Instant};

const MAX_HISTORY_ENTRIES: usize = 20;
const LIVE_INPUT_PAUSE: Duration = Duration::from_secs(1);

/// An owned condition and view snapshot, without result rows or audio state.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct SearchHistoryEntry {
    pub(super) definition: SearchTaskDefinition,
    pub(super) view: SearchTaskView,
}

impl From<&SearchTask> for SearchHistoryEntry {
    fn from(task: &SearchTask) -> Self {
        // Editor drafts may already belong to the next unsubmitted request.
        // History replays the successful definition, while ordinary task saves
        // retain their independent raw drafts.
        let mut view = task.view.clone();
        if view
            .raw_inputs
            .get("favorites_only")
            .is_some_and(|value| (value == "true") != task.definition.filters.favorites_only)
        {
            // A cold task can retain the sidebar for an unsubmitted favorites
            // draft. Its history must show the executable condition instead.
            view.sidebar_page = i32::from(task.definition.filters.favorites_only);
            view.favorites_category = 0;
        }
        view.raw_inputs.clear();
        Self {
            definition: task.definition.clone(),
            view,
        }
    }
}

/// Twenty conditions including the current one, independently owned by a task.
pub(super) struct SearchHistory {
    entries: Vec<SearchHistoryEntry>,
    current: usize,
    typing: Option<Instant>,
}

impl SearchHistory {
    /// Seed navigation with the task's current executable conditions.
    pub(super) fn new(task: &SearchTask) -> Self {
        Self {
            entries: vec![task.into()],
            current: 0,
            typing: None,
        }
    }

    /// Record an admitted search. Live typing updates its current tail entry;
    /// explicit actions close that typing group and preserve the prior condition.
    /// Returns whether the current executable conditions changed.
    pub(super) fn record(&mut self, task: &SearchTask, coalesce: bool) -> bool {
        self.record_at(task, coalesce, Instant::now())
    }

    fn record_at(&mut self, task: &SearchTask, coalesce: bool, now: Instant) -> bool {
        if same_search_definition(&self.entries[self.current].definition, &task.definition) {
            self.entries[self.current] = task.into();
            if !coalesce {
                self.typing = None;
            } else if self.typing.is_some() {
                self.typing = Some(now);
            }
            return false;
        }
        self.entries.truncate(self.current + 1);
        if coalesce
            && self
                .typing
                .is_some_and(|last| now.saturating_duration_since(last) < LIVE_INPUT_PAUSE)
        {
            self.entries[self.current] = task.into();
        } else {
            self.entries.push(task.into());
            if self.entries.len() > MAX_HISTORY_ENTRIES {
                self.entries.remove(0);
            }
            self.current = self.entries.len() - 1;
        }
        self.typing = coalesce.then_some(now);
        true
    }

    /// Update only the current condition's view. Captures of another condition
    /// must not create entries or destroy the forward branch.
    pub(super) fn capture(&mut self, task: &SearchTask) {
        if same_search_definition(&self.entries[self.current].definition, &task.definition) {
            self.entries[self.current].view = SearchHistoryEntry::from(task).view;
        }
    }

    /// Keep the current entry bound to identities validated during restoration,
    /// without turning a snapshot's reassigned IDs into a new search.
    pub(super) fn reconcile_current(&mut self, previous: &SearchTaskDefinition, task: &SearchTask) {
        if same_search_definition(&self.entries[self.current].definition, previous) {
            self.entries[self.current] = task.into();
        }
    }

    /// Return the last successfully admitted condition at the current position.
    pub(super) fn current(&self) -> Option<&SearchHistoryEntry> {
        self.entries.get(self.current)
    }

    /// End a live input group when leaving a task or beginning navigation.
    pub(super) fn finish_typing(&mut self) {
        self.typing = None;
    }

    /// Move by the requested signed number of entries without adding a search.
    pub(super) fn step(&mut self, delta: i32) -> Option<SearchHistoryEntry> {
        self.typing = None;
        let destination = self.current.checked_add_signed(delta as isize)?;
        if destination == self.current || destination >= self.entries.len() {
            return None;
        }
        self.current = destination;
        Some(self.entries[self.current].clone())
    }

    /// Whether an older condition is available for this task.
    pub(super) fn can_back(&self) -> bool {
        self.current > 0
    }

    /// Whether a newer condition is available for this task.
    pub(super) fn can_forward(&self) -> bool {
        self.current + 1 < self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use audiolib_core::{SearchScope, SortOrder, search_workspace::SearchFolderRef};

    fn task(query: &str) -> SearchTask {
        SearchTask {
            definition: SearchTaskDefinition {
                query: query.into(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn retains_twenty_entries_including_current_and_discards_the_oldest() {
        let mut history = SearchHistory::new(&task("initial"));
        for index in 1..=25 {
            assert!(history.record(&task(&format!("search {index}")), false));
        }
        assert_eq!(history.entries.len(), 20);
        assert_eq!(history.entries[0].definition.query, "search 6");
        assert!(!history.can_forward());
        for index in (6..25).rev() {
            assert!(history.can_back());
            assert_eq!(
                history.step(-1).unwrap().definition.query,
                format!("search {index}")
            );
        }
        assert!(!history.can_back());
        assert!(history.step(-1).is_none());
    }

    #[test]
    fn new_search_after_back_truncates_forward_and_tasks_remain_independent() {
        let mut first = SearchHistory::new(&task("initial"));
        let second = SearchHistory::new(&task("other task"));
        first.record(&task("wind"), false);
        first.record(&task("boom"), false);
        assert_eq!(first.step(-1).unwrap().definition.query, "wind");
        assert!(first.can_forward());
        first.record(&task("rain"), false);
        assert!(!first.can_forward());
        assert_eq!(first.step(-1).unwrap().definition.query, "wind");
        assert_eq!(first.step(1).unwrap().definition.query, "rain");
        assert_eq!(second.entries.len(), 1);
        assert!(!second.can_back());
        assert!(!second.can_forward());
    }

    #[test]
    fn typing_coalesces_until_explicit_search_or_history_navigation() {
        let mut history = SearchHistory::new(&task("initial"));
        history.record(&task("bo"), true);
        history.record(&task("boo"), true);
        history.record(&task("boom"), true);
        assert_eq!(history.entries.len(), 2);
        assert!(!history.record(&task("boom"), false));
        history.record(&task("wind"), true);
        assert_eq!(history.entries.len(), 3);
        assert_eq!(history.step(-1).unwrap().definition.query, "boom");
        history.record(&task("rain"), true);
        assert_eq!(history.entries.len(), 3);
        assert_eq!(history.step(-1).unwrap().definition.query, "boom");
        assert_eq!(history.step(-1).unwrap().definition.query, "initial");
    }

    #[test]
    fn leaving_a_task_finishes_typing_without_changing_the_current_entry() {
        let mut history = SearchHistory::new(&task("initial"));
        history.record(&task("boom"), true);
        history.finish_typing();
        assert_eq!(history.current().unwrap().definition.query, "boom");
        history.record(&task("wind"), true);
        assert_eq!(history.entries.len(), 3);
        assert_eq!(history.step(-1).unwrap().definition.query, "boom");
    }

    #[test]
    fn a_pause_between_live_searches_starts_a_separate_entry_without_enter() {
        let mut history = SearchHistory::new(&task("initial"));
        let now = Instant::now();
        history.record_at(&task("bo"), true, now);
        history.record_at(&task("boom"), true, now + Duration::from_millis(50));
        history.record_at(
            &task("wind"),
            true,
            now + LIVE_INPUT_PAUSE + Duration::from_millis(50),
        );
        assert_eq!(history.entries.len(), 3);
        assert_eq!(history.step(-1).unwrap().definition.query, "boom");
    }

    #[test]
    fn captures_views_without_creating_conditions_or_losing_forward_entries() {
        let mut initial = task("initial");
        let mut history = SearchHistory::new(&initial);
        initial.view.offset = 400;
        initial.view.list_top_row = 12;
        initial.view.list_row_offset = 0.5;
        initial
            .view
            .raw_inputs
            .insert("duration_min".into(), "unfinished".into());
        history.capture(&initial);
        history.record(&task("boom"), false);
        let restored = history.step(-1).unwrap();
        let mut expected_view = initial.view.clone();
        expected_view.raw_inputs.clear();
        assert_eq!(restored.view, expected_view);
        assert_eq!(initial.view.raw_inputs["duration_min"], "unfinished");
        initial.view.offset = 200;
        history.capture(&initial);
        history.capture(&task("unrelated draft"));
        assert!(history.can_forward());
        assert_eq!(history.entries.len(), 2);
        history.step(1).unwrap();
        assert_eq!(history.step(-1).unwrap().view.offset, 200);
    }

    #[test]
    fn snapshots_retain_scope_folder_filters_sort_and_owned_view() {
        let mut original = task("boom");
        original.definition.scope = SearchScope::Path;
        original.definition.folder = Some(SearchFolderRef {
            id: 4,
            path: "C:/sounds".into(),
        });
        original.definition.include_subfolders = false;
        original.definition.filters.favorites_only = true;
        original.definition.filters.include_tags = vec!["impact".into()];
        original.definition.filters.exclude_tags = vec!["quiet".into()];
        original.definition.order_by = SortOrder::Duration;
        original.definition.descending = true;
        original.view.offset = 200;
        let mut history = SearchHistory::new(&original);
        history.record(&task("wind"), false);
        assert_eq!(
            history.step(-1).unwrap(),
            SearchHistoryEntry::from(&original)
        );

        let mut changed = original.clone();
        changed.definition.scope = SearchScope::Filename;
        assert!(history.record(&changed, false));
        assert_eq!(history.entries.len(), 2);
        assert_eq!(
            history.step(-1).unwrap().definition.scope,
            SearchScope::Path
        );
    }

    #[test]
    fn resolved_identities_replace_current_without_losing_the_forward_branch() {
        let mut original = task("wind");
        original.definition.filters.album_id = Some(1);
        let mut history = SearchHistory::new(&original);
        history.record(&task("boom"), false);
        history.step(-1).unwrap();
        let mut resolved = original.clone();
        resolved.definition.filters.album_id = Some(8);
        history.reconcile_current(&original.definition, &resolved);
        assert!(!history.record(&resolved, false));
        assert!(history.can_forward());
        // A failed, unrecorded search cannot replace the successful entry.
        history.reconcile_current(&task("failed").definition, &task("failed"));
        assert_eq!(history.current().unwrap().definition, resolved.definition);
    }
}
