# Supplemental favorite tests

This note records extra desktop tests for the row-favorite and current-folder favorite filter work in pull request #299. It does not change product code.

## Why these tests exist

The #299 tests already cover:

- the row star requesting a favorite toggle in both list layouts
- a favorite write that persists without saving tag or note drafts
- the folder filter keeping the open folder and returning only that folder's favorites

They did not cover:

- reopening the same temporary SQLite file in a new process after a favorite write with unsaved tag and note drafts
- keeping `favorites_only` on after `select_folder_id` and scoping search to that folder

Creating a new `Controller` in the same process is not a process restart. The child-process test is the restart evidence.

## How to run

Use the full Rust test path with `--exact`. A bare function name plus `--exact` can exit 0 while executing 0 tests. That result is a failure, not a pass.

In an early combined retry, six invocations used short names with `--exact`. Each command exited 0, executed 0 tests, and filtered out 870 tests. Those runs must not be treated as passes.

Re-run each test as:

```text
cargo test -p audiolib-desktop --locked <full-path> -- --exact --nocapture --test-threads=1
```

Full paths:

- `controller::favorite_list_tests::folder_favorite_filter_keeps_the_open_folder`
- `controller::favorite_list_tests::row_star_requests_the_toggled_favorite_in_both_themes`
- `controller::favorite_list_tests::favorite_toggle_persists_without_saving_tags_and_the_filter_is_folder_local`
- `controller::favorite_list_tests::favorite_survives_new_controller_on_same_sqlite_without_tag_or_note_drafts`
- `controller::favorite_list_tests::favorites_only_stays_on_after_select_folder_id_and_scopes_to_that_folder`
- `controller::favorite_list_tests::favorite_survives_child_process_reopen_of_same_sqlite`

Treat cargo exit 0 with 0 tests executed as failure.

## Results of the full-path re-run

Date: 2026-11-13 (Asia/Shanghai). rustc/cargo toolchain: 1.97.1, `x86_64-pc-windows-msvc`.

Each of the six tests actually ran and passed: `running 1 test`, `1 passed`, `0 failed`.

| Test | Result | Test body time |
| --- | --- | --- |
| `folder_favorite_filter_keeps_the_open_folder` | pass | 0.70s |
| `row_star_requests_the_toggled_favorite_in_both_themes` | pass | 1.74s |
| `favorite_toggle_persists_without_saving_tags_and_the_filter_is_folder_local` | pass | 2.37s |
| `favorite_survives_new_controller_on_same_sqlite_without_tag_or_note_drafts` | pass | 2.77s |
| `favorites_only_stays_on_after_select_folder_id_and_scopes_to_that_folder` | pass | 2.40s |
| `favorite_survives_child_process_reopen_of_same_sqlite` | pass | 2.58s |

The folder-change test asserts that `favorites_only` stays on and that provider search is scoped to the selected folder. It does not assert that the UI list is published when the controller is initialized without an event loop.

The in-process `Controller::new` test is not restart evidence. The child-process test opens the same temporary SQLite file in a spawned copy of the test binary.

The test file blob at the time of this note is `920e92630f67b4240671ac2e4718076dd89ea5a4` (`app/audiolib-desktop/src/favorite_list_tests.rs`, 284 insertions on top of the #299 file). This branch is based on #299 head `f369a16b681f75dd6ae85b0cca4b3716f14d621d`. It does not contain pull request #296 or a combined-merge commit. At the time of this note, `beta` was `08e397803fb9062ed760b7bb61c3fb1a9f53b4a9` and did not contain #299.

## Boundaries not covered by these tests

- Native GUI clicks were not executed. Do not treat these unit tests as UI acceptance.
- Reveal-in-Explorer behavior is out of scope for this branch.
- A later combined checkout was used only for local verification. Its merge commits are not part of this branch.
