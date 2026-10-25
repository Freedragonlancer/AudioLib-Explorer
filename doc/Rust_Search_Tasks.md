# Search tasks and saved searches

The desktop supports up to six independent search tasks for the active library. Each task owns its query, directory navigation, filters and result-list view. Search-task favorites are personal client state; audio favorites keep their existing library Metadata behavior.

## Using tasks

The document-style task strip sits above the search field. Its active task has a connected content edge and emphasis line, making it distinct from toolbar buttons and the smaller panel selectors. The strip provides creation, selection, task favorite and close actions. Long task names elide, and the strip scrolls horizontally when the window cannot show every task.

New tasks start with an empty query, the whole current library and default filters. Closing a task leaves other tasks intact; closing the last task leaves one empty task available. Appearance settings select the maximum number of open tasks from one to six, with six as the default. Reaching the limit never replaces an existing task. Lowering the limit below the current task count requires closing tasks first.

All tasks use the current data source. Switching between libraries saves the old library's task group and restores the destination library's group. Connections to the same server library share that library's personal group, including LAN and external connection profiles. A different server library has a separate group.

## Independent and shared state

Each task retains its query text, complete search filters, selected directory and descendant policy, directory expansion, sort order, page, selected and checked records, selection anchor and view position. Raw filter drafts are retained too: changing tasks does not submit a partially typed tag or turn invalid duration text into a valid search. The current filter/details page and directory/favorites page are part of the task view.

Column widths, pane widths and visibility, language, theme, interface scale, audio output and playback remain application preferences. Sorting is task-local; the existing table preference supplies the initial sort for a new task.

The audio engine and audition session are shared. Changing or closing a task does not load, start, pause or stop audio. Restoring a task's selected row may restore its details, but it must not pass through an audio-loading selection path. Playback remains subject to the existing explicit transport and selection actions.

Metadata drafts and their ordered write queue are shared by file identity and edit revision. Leaving a task captures requested edits before changing the displayed record. Returning to the same audio in another task shows the current pending draft or confirmed Metadata, rather than a separate editable copy. Closing the application drains requested writes through the existing shutdown barriers.

## Favorites

Favorites has separate audio and search-task categories. Audio favorites continue to filter `AudioFile.favorite` and use the existing local or remote Metadata write path. A search-task favorite saves a named search definition: query, directory scope, descendant policy, complete filters and sorting. It does not freeze the resulting audio records.

Opening a saved search focuses an open task only when its validated search definition still matches the favorite. Otherwise it opens an independent task when capacity is available, preserving any temporary edits in the previous task. Results are queried again against the current library. Changing a task does not change its saved search; the explicit update action replaces the saved definition. Search favorites can be renamed or removed without changing audio Metadata.

Personal search favorites remain on this computer, independently of server write permission and connectivity. They do not travel in library exports, backups or server snapshots, and do not require an HTTP API or main-library schema change.

## Persistence and recovery

The client stores task order, active task, search definitions, navigation and view state in a versioned `workspace.json` under the configured user data root. The default root is `%LOCALAPPDATA%/AudioLibExplorer/v1`; `AUDIOLIB_DATA_DIR` selects an isolated alternative. Per-library scopes use the existing canonical local-library identity or the server library ID. Tokens, passwords, decoded audio, result rows, Slint models and Waveform images are not persisted in a task workspace.

All open tasks, including tasks without a favorite, are restored on the next launch. Only the active task queries at startup; other tasks query when activated. Restoration never resumes audio automatically. Persisted file and directory identities include their indexed paths and are resolved against the opened library before reuse. A missing saved directory is reported as an unavailable scope, rather than silently broadening the search to the whole library.

Workspace writes run off the Slint event loop and retain their original library target. Switching the current source cannot redirect a late save into another library's group. Unknown or damaged workspace formats are preserved and reported; an empty fallback must not overwrite them as an ordinary save.

## Search execution and invalidation

Tasks share the existing bounded result and directory-facet workers. Switching tasks cancels obsolete requests. Every asynchronous result, directory model, detail response and visible-row Waveform publication must match the current task, request and library/source generations before updating the UI.

An inactive task can retain its completed result-page cache in memory. Reuse requires the same provider identity and a matching `DataProvider::search_revision`; a missing or failed revision probe causes a fresh query. Provider replacement invalidates old caches even when revision numbers coincide. Metadata changes, scans, removals and refreshes invalidate dependent task views. Background tasks refresh lazily when selected, avoiding six simultaneous whole-library searches.

Query validation and FTS5 behavior remain the existing core contracts, including the 500-character query limit. The performance target remains **P95 latency <500ms for 1M+ records**. Task behavior tests and layout checks do not establish a new million-record benchmark or physical button-to-audio-output measurement.

## Validation

Regression coverage exercises independent query/filter/directory/list state, capacity and last-task behavior, late asynchronous publications, same-file Metadata drafts across tasks, manual favorite updates and restart restoration. Identity tests reject recycled folder/file IDs, and cache tests observe external Metadata changes. A failed workspace save blocks a library switch and remains failed even after another library's workspace saves successfully.

Local verification on Windows x64 with Rust 1.97.1 MSVC on 2026-10-25 passed formatting, strict Clippy for core/db/remote/desktop/CLI, 105 core tests, five workspace-storage tests, the remote offline-workspace test, 304 desktop tests across the controller and other desktop groups, and 19 CLI tests. After the final cache-revision guard, the 12 task/UI tests and three directory-search regressions passed again. The synthetic package-boundary regression passed with this document included in the portable allowlist. Ignored desktop entries include subprocess helpers exercised by their parent tests and explicitly interactive hardware/UI checks; they are not claimed as separately run acceptance.

Layout acceptance covers six long task titles, both desktop languages, the minimum 1080×700 logical window and supported interface scaling. Keyboard navigation must remain available without interpreting task changes as audio transport. Real-device audio and performance acceptance are recorded separately from synthetic UI checks.
