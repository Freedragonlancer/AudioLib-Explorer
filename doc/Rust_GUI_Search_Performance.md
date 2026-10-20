# Large-library GUI search

## 2026-10-20 stage record — performance target not met

The primary target remains **P95 latency <500ms for 1M+ records**. At the user's request to close this development stage promptly, 42 GUI samples were collected: 14 real-library synthetic-Slint samples, 14 million-record synthetic-Slint samples and 14 real-library native-input samples. All 42 are valid; seven exceed the target. The original 224-sample formal matrix **has not started and is not complete**. This smaller stage must not be described as its replacement or as final SLA acceptance.

The source is `b49f7d278505669ac025b4b49b9eceec45873728`, version **0.4.11**, built with the verified ASIO SDK 2.3.4. The official measured GUI SHA256 is `588553215AF4C0A6133DEB0C456F0B66A756AD1DB68AC38495970161CC0A97EA`. The two completed synthetic-input populations did not meet the latency target:

| Population and input | Records / database folders | Collected / valid / invalid | Valid totals ≥500ms | Total P95 / maximum |
| --- | --- | ---: | ---: | ---: |
| Real library, synthetic Slint | 909,342 / 20,248 | 14 / 14 / 0 | 4 | 933.151 / 933.151 ms |
| Isolated synthetic database, synthetic Slint | 1,000,000 / separate synthetic hierarchy | 14 / 14 / 0 | 2 | 568.9038 / 568.9038 ms |
| Real library, native Sky input | 909,342 / 20,248 | 14 / 14 / 0 | 1 | 609.3829 / 609.3829 ms |

Each completed population has one run for each of the fixed 14 requests, rather than repeated observations of one query. Its stored summary uses sorted index `ceil((n-1)*p)`; at 14 samples its P95 is the maximum. These small populations do not establish a production latency tail. All four real-library synthetic slow totals—933.151, 511.5448, 511.0314 and 584.7215ms—both million-record slow totals, and the native `metal` total of 609.3829ms remain valid retained measurements. Slow samples are not reclassified as invalid or removed from the statistics.

For the million-record filtered filename-order case (`ambient`, 48kHz, duration 0–5s), the earlier valid 1,673.8516ms preflight sample and the current valid 125.9468ms sample are both retained. The real-library instance of that request still took **584.7215ms** in this stage. These are different populations/runs, not a controlled speedup ratio or evidence that every such request is now below 500ms.

### Current measurement boundary

The machine is Windows x64 with an AMD Ryzen 7 9800X3D (8 cores / 16 logical CPUs) and approximately 93.5GiB of system RAM. The build uses Rust 1.97.1 and Slint 1.17.1. These are measurements on this machine, not a resource requirement or evidence for macOS performance.

Each request starts a fresh process, uses a 200-record page across folders, and reports a first target-query ranking-cache miss. The real-library measurements use an explicitly selected read-only current snapshot through an isolated diagnostic profile; the million-record fixture is a separate database. Provider opening, startup empty browsing, library-information refresh, index preparation and initial WGC calibration finish before target input. Those reads may warm pages. **OS page caches are not cleared**; a fresh process does not establish physical cold-disk performance. The measured production path includes the current **30ms debounce**, worker queueing, model publication and rendering.

Synthetic Slint dispatch sends the complete text through the production LineEdit's KeyPressed/KeyReleased handling; it is not OS keyboard injection. Native Sky cross-checks use the native KeyboardInput Pressed text or Ime Commit timestamp before the production handler. Their results remain separate. The endpoint is the first observed qualifying, CRC-verified Windows Graphics Capture (WGC) compositor frame with the matching nonce/generation and phase ALL, after both the result page and complete directory-count model have been published. The current viewport is rendered through virtualized models; this does **not** mean all 20,248 real-library folders are painted simultaneously. It is not panel scanout or photon timing.

The correctness and frame qualifications are unchanged for this stage. Every accepted raw sample must match the post-timing oracle's ordered IDs, complete result count and all directory counts, with the read-only source and normal runtime data preserved. Foreground, visibility, monitor/workarea coverage, unknown/application occlusion, native geometry history, CRC and QPC checks remain strict. Outer-window or DPI changes invalidate a sample; client-only native layout transitions must restore the full armed geometry before the qualifying frame. Only the explicitly verified Sky pointer-decoration exception is permitted when selected. This protocol does not claim continuous absolute absence of every possible occluder. Invalid reports and all earlier failed probes remain preserved.

### Current implementation and verification status

- The native “Check current page” action keeps stable menu availability while the menu is ready. Its activation checks a separately stored has-page-rows boolean and does nothing for an empty page; normal result-model replacement no longer drives native-menu reconstruction. This preserves the action's selection behavior without claiming that every other menu state is static.
- Filename sorting can collect bounded exact MATCH membership and apply supported sample-rate/duration filters through verified existing covering indexes before counting. The complete intersection determines the count; SQLite preserves native NOCASE ordering and ascending ID ties in both directions, then hydrates only the selected page. Present, unscoped requests without additional filters require a same-snapshot proof that all rows are present. Missing/incompatible optional indexes, unsupported requests or exceeded budgets retain the complete existing path. Unscored IDs never enter the BM25 ranking cache.

Formatting, strict default/ASIO Clippy and **959 tests in each configuration** passed for this source. The official ASIO build completed in **262.641s**, and the release regressions passed. All three stage populations are complete, while their performance acceptance failed. The stage installation workflow retains those slow samples and records `PerformanceTargetMet=false`; it cannot turn them into a performance pass or complete the 224-sample matrix. Portable validation, full installed payload identity and normal runtime-data preservation have a separate installation report; installation is complete only when that report confirms those checks.

Private audit references are `.rust-validation/gui-e2e/coordination/stage-close-plan-v1.json`, `.rust-validation/gui-e2e/coordination/stage-report-storage-v1.json` and `.rust-validation/gui-e2e/round-v11-checks/`. The storage record identifies the `real-stage-slint-1`, `million-stage-slint-1` and `real-stage-native-1` batches. Raw private reports, database locations, screenshots and real audio filenames are not part of packaged documentation. See [Rust Validation](Rust_Validation.md) for the coordinated checks and final installation status.

## Historical record — 2026-10-10 and 2026-10-11

The remaining sections retain their original dates and measurement conditions; they do not override the current stage status above.

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
