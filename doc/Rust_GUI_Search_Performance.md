# Large-library GUI search — 2026-10-10

The 2026-10-11 first-search follow-up uses fixed parallel page/facet workers, exclusive NO_MUTEX readers and a directory rollup that stays bounded with stale ANALYZE statistics. Fresh-process GUI publication and fresh-provider measurements for 1M rows with 1 or 20001 database folders are recorded in [Rust Validation](Rust_Validation.md) and the full first-search record. The earlier measurements below are retained with their original conditions.

The measurements and implementation below describe the 2026-10-10 change. The 2026-10-11 review adds bounded exact BM25 ranked-ID caching, Unicode tokenization and source/selection/Waveform corrections. Current measurements and their first-pass limits are in [Rust Validation](Rust_Validation.md) and the complete measurement record.

The primary target remains **P95 latency <500ms for 1M+ records**. This change addresses desktop search scheduling and directory presentation, including server mode's local read-only snapshot. It does not change BM25 ranking, search predicates, the server protocol or stored library data.

## Changes

- Each new search request, including invalid input, cancels the previous search and directory aggregation. SQLite checks a per-request flag every 2,000 VM instructions; cancelled readers remove their progress handler before returning to the pool. Both query APIs also check the flag before returning, so short queries below the progress-handler interval cannot report success after cancellation. The existing single search worker, debounce and generation checks remain bounded.
- One owned directory-facet snapshot is reused for pagination, sorting and folder scope changes with the same query and filters. Explicit searches, F5, Metadata writes, scan completion and library operations invalidate it before enqueueing work. A subsequent navigation cannot erase a pending invalidation. A provider-local revision token also invalidates facets after commits by another process or snapshot annotation updates. SQLite uses a dedicated read-only observer connection for `PRAGMA data_version` (tokens from pooled connections are not comparable); revisions are checked before and after facet reads, and unsupported/failed revision checks disable cache reuse.
- Expanding/collapsing directories no longer calls `stats()`, which scanned all audio records to sum duration and size. Library totals come from the existing background library-information refresh, including startup, F5, scan completion and library operations. Each information request has its own generation, so an older completion or error cannot overwrite newer totals.
- The UI captures directory data through `Arc` snapshots. Tree lookup construction and selected-row lookup run on the worker. An immutable Slint model creates `FolderRow` strings only for requested viewport rows, retaining the virtualized list and existing selection/path semantics.

## Reproduction and measurement boundaries

Measurements use Windows x64, AMD Ryzen 7 9800X3D (8 cores / 16 logical CPUs), Rust 1.97.1 and version 0.3.7 with the verified ASIO SDK. The database is the existing isolated 1,000,000-record synthetic benchmark, with 200-record pages, warm/uncontrolled OS caches, 64 MiB SQLite reader caches and a 1 GiB mmap limit. These are synthetic measurements; no production snapshot was used.

The GUI benchmark runs real controller workers and the Slint testing event loop. It measures query publication, cancellation followed by a replacement query, and 20 expansion/collapse publications with an independently constructed 20,001-directory presentation model. It also measures the old full-library statistics call that was removed from tree refresh. Testing-backend timing excludes physical display/pointer latency and is not a manual acceptance measurement. The directory model is synthetic; it does not claim that the database contains 20,001 populated directories.

```powershell
. ./tools/prepare-asio.ps1
$env:AUDIOLIB_GUI_BENCHMARK_DB = (Resolve-Path .rust-validation/performance/million.db).Path
cargo test --release --locked -p audiolib-desktop --features asio -- --exact controller::folder_search_tests::million_record_gui_navigation_benchmark --ignored --nocapture --test-threads=1
Remove-Item Env:AUDIOLIB_GUI_BENCHMARK_DB
cargo run --release --locked -p audiolib-db --bin search-benchmark -- --existing-db .rust-validation/performance/million.db --queries 100 --folder-facets
```

The GUI regression covers cross-folder navigation, zero-hit breadcrumbs, rapid query replacement, facet reuse, and Metadata invalidation immediately followed by navigation. Database regression interrupts an already acquired reader and verifies that its pooled connection can subsequently execute uncancelled work.

## Review regressions (2026-10-10)

The external-write GUI regression failed before the fix: after another provider cleared the last matching favorite, result rows were empty but the tree retained `sounds: 1` / `wind: 1` until timeout. The regression now requires both to become empty without F5. Additional checks cover invalid duration / overlength query cancelling pending work, stale statistics and errors, missing / changed cache revisions, and the observer seeing own writes, external writes, read-only snapshot changes, search-index maintenance and restore. All fixtures are isolated temporary databases.

Production acceptance remains pending: configure the server, then test search against its actual library. The measurements below predate these review corrections and are not production results.

## Measured results

Raw results retain all 20 directory/statistics samples and the provider benchmark summaries.

| Measurement | Result |
| --- | ---: |
| Removed full-library statistics query, 20 warm calls | 106.3–119.5 ms per tree refresh |
| New 20,001-directory expansion/collapse publication, 20 samples | 13.8–29.1 ms (median approximately 15.4 ms) |
| Replace an in-flight broad query with a no-hit keyword, one run | 15.748 ms |
| Broad `metal` query including directory publication, one run | 687.639 ms |

The statistics and directory measurements are different workloads: they show the cost removed and the new presentation cost, not a controlled before/after GUI speedup ratio. Broad first-query ranking and directory aggregation remain material costs; the complete `metal` interaction still exceeds 500 ms. Result-page latency is recorded separately in the JSON. No production snapshot was available for this run.

Formatting, default and ASIO Clippy/workspace tests, the explicit optimized GUI benchmark, and release runtime/review-fixes/library-management regressions passed. Packaging and the fixed local installation are verified separately with `validate-portable.ps1`; both installed EXEs must match the validated portable payload.
