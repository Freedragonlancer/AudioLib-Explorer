# Plan B implementation record

The current CR branch is `0.6.0-dev.1`, incorporating released 0.5.0 at `c46f583`.
Its review fixes and final revalidation are recorded in [the CR report](Search_Storage_B_CR.md).
The revised v6 goal, independent client history and final p1 format are specified in
[Schema_V6_Iteration.md](Schema_V6_Iteration.md); the evidence below is historical p0.
The detailed checkpoints below describe the preceding `0.5.0-dev.1` implementation;
their build/recipe identities and timings must not be attributed to the CR build.

User scope, 2026-10-21: implement Plan B, shared client/server migration tools,
compatibility checks, resumable execution and rollback; diagnose duration-filter
tail latency. Validate in isolation. Do not update the current installation or
working library. This explicit restriction supersedes installation steps during
this development phase.

## Workspace and baseline

- Branch: `codex/search-storage-b`
- Worktree: isolated `search-storage-b` checkout; the machine-local path is
  recorded in `.rust-validation/search-storage-b/*-run.json` (not packaged).
- Baseline commit: `0527ff13e81a4cc4218c3ce4a9f3493d3764f83b`
- Initial development identity: `0.5.0-dev.1`, shared by all workspace application crates.
  This is not a published version or a claim about future release numbering.
- Original checkout and `local_release` remain outside this implementation.
- Frozen experiment data:
  `%TEMP%\audiolib-search-b-20261021-5db579c5`
  (read baseline/candidate only; create separate owned copies for new tests).

## Completion evidence required

- [x] Pinned corrected SQLite, actual engine identity and statistics regressions.
- [x] Shared owned compatibility DTOs, tested version ranges, format validation,
      unknown-feature rejection and dedicated rebuild diagnosis.
- [x] Schema 6 contentless-delete storage; all Metadata, scanner, import, removal,
      maintenance and remote incremental writes use consistent indexing.
- [x] Shared inspect/plan/apply/resume/verify/rollback migration engine, source
      binding, checked space budget, administration/scan leases and backup.
- [x] Durable batches, signed-ID watermarks, cancellation and crash recovery at
      copy/index/validate/compact/publish/rollback boundaries.
- [x] Client/CLI/server integration; download negotiation and invalid-download
      cache preservation; source revision distinct from local format generation.
- [x] Supported historical schema/derived revision matrix, interrupted rev 3 upgrade,
      nonempty user data/reference preservation, corrupt/unknown inputs.
- [x] Duration-filter diagnosis, narrow fix and result/order/count regression.
- [x] Required formatting, Clippy, workspace tests and relevant library/Metadata
      regression scripts using isolated data.
- [x] Full-library migration and equivalence/performance measurements using the
      integrated implementation, plus 1M+ query conditions and tail follow-up.
- [x] Isolated build/package validation; no installation into the current
      `local_release` and no working-library migration.
- [x] Final documents reflect actual implementation and remaining limitations.

No item is complete merely because a previous prototype passed.

Checked implementation items are supported by the 1,135-test default suite and
isolated CLI regressions, the shared executor's actual subprocess-crash tests,
the real HTTP compatibility tests and the ASIO portable build/smoke evidence below.
Historical coverage includes captured v5 DDL, systematic additive v3/v4 fixtures,
old derived revisions and interrupted rev-3 protocol states; actual archived v3/v4
libraries are unavailable, so these checks do not claim archived-release coverage.
Process exits with dirty pages/hot journals are tested; physical power interruption
and every filesystem/hardware failure mode are not claimed. The final-recipe
full-library/performance rerun, rollback and document reconciliation are complete;
the final ASIO suite adds the two cache-crash tests for 1,137 passing tests.

## Pre-CR checkpoint (`0.5.0-dev.1`)

2026-10-21 implementation in this isolated worktree:

- Final ASIO release build and portable packaging pass. Portable validation uses
  fresh temporary data and passes EXE identity, empty first launch, synthetic WAV
  scan, CLI statistics, Chinese UI, invalid-preferences diagnostics and licenses.
  New migration/design/evidence documents are included in the package; machine-local
  paths in those documents are generalized. The public payload privacy check passes.
  Release CLI SHA256 is
  `251D5309DD0B44784E70FCE4A0D33B7B37EF0E00040F2B4419D8052C9B730B72`;
  desktop SHA256 is
  `77A48B4B6C947CB34CEE9047E3B799903C104737278C6E074F75F19ECB6BC3BA`.
  Evidence: `final-release-build.txt`, `final-build-summary.json`, `final-package.txt`
  and `final-portable.txt`. This is an isolated development payload, not deployment.
- Remote-cache process exits during indexing and before saving now have explicit
  restart tests. Previous cache/state bytes remain unchanged; retry succeeds from
  that intact source, preserving revision and recording one format generation.
  The abandoned job's files remain byte-identical and only the new job is cleaned.
  This verifies conservative retention, not automatic reclamation of abandoned jobs.
  The focused cache group passes **3 tests**, including the child entry point and
  existing WAL-copy case (`final-cache-crash-tests.txt`). Final ASIO workspace
  checks pass **67 targets, 1,137 tests, 0 failures, 75 ignored**, with formatting
  and all-target Clippy `-D warnings` also passing (`final-asio-summary.json`).
  The packaged release CLI passes library-management and review-fixes regressions
  in owned temporary data (`final-release-library-management.txt` and
  `final-release-review-fixes.txt`). The portable Chinese screenshot was inspected:
  text, controls, synthetic record and Waveform render without clipping/overlap.
  The final full-library migration, independent oracles and rollback all pass.
- Final recipe `30c99e49dbd9bd23f654b050a5c1bc069bc1df39157274114c9fe25e44b17226`
  migrates the 909,342-row source from **19,495,989,248 to 10,612,101,120 bytes**
  (**45.568% smaller**) in 984.76 seconds; independent verification takes 78.07
  seconds and rollback 156.49 seconds. All 58 complete query results/native BM25
  scores, 258 page/folder pairs and 1,198,133 vocabulary statistics agree.
  Only the explicitly retired DTO count is normalized for comparison. Rollback
  restores 752 play-history rows and a total play count of 759; all retained
  business data and `last_played_at` values agree, including 635 non-NULL timestamps.
  The frozen input hash remains unchanged. Final evidence and conditions are in
  [the integrated report](Search_Storage_B_Integrated_Validation.md#0-最终构建复验当前结论)
  and `final-full-validation-summary.json`.
- The final 1M synthetic workload uses the exact same 26 request objects as the
  earlier workload. Fresh-provider first-search P95 is **298.915 ms**; search plus
  folder facets P95 is **394.495 ms**. Both measured gates pass. OS caches were not
  cleared, provider opening is separate, and this does not establish every-query,
  arbitrary-hardware or physical-cold guarantees. Real duration-filter search plus
  facets is 94.5–97.6 ms; a first-pass Tags sample is still 504.6 ms combined.

## Implementation chronology (historical checkpoints)

The entries below preserve earlier findings and intermediate failures. Their
pending work and earlier measurements are superseded by the current checkpoint
above and the final integrated report; they are not additional open requirements.

- Added restart checkpoint validation before any copy/index writes. The exact
  table set, phase, counter types/ranges, strict completion flags, actual row
  counts, signed cursors and copied/indexed ID prefixes must agree. Indexing also
  requires every table's completed-copy marker. Corrupt state is retained for
  diagnosis/rollback instead of reset or silently accepted; final business/FTS
  validation is still required. A regression first reproduced acceptance of an
  incorrect counter, then passed **19 corruption variants** with unchanged
  candidate bytes, unchanged source business data and complete rollback.
  Focused migration suite: **28 passed, 0 failed**. Added child-process exits
  inside uncommitted copy/index transactions: dirty pages and a hot rollback
  journal are proven before exit; the public resume path recovers and restores
  every annotation/search result. The initial snapshot-upgrade suite passes
  **11 tests**. A separate new test passes for partial publication and partial
  rollback after one actual 4,096-page SQLite backup step; the opaque 5,000-page
  business payload and the full original digest round-trip. These are process
  crash tests with persisted uncommitted pages, not hardware power-loss tests.
  Evidence: `checkpoint-rejection-before.txt`, `checkpoint-migration-tests.txt`,
  `checkpoint-crash-tests.txt`, `checkpoint-publication-tests.txt` under
  `.rust-validation/search-storage-b`. Final default workspace/all-target suite
  passes **67 targets, 1,135 tests, 0 failures, 75 ignored**; the expanded snapshot
  suite now has **13 passing tests**. Formatting and workspace/all-target Clippy
  with `-D warnings` pass. The aggregate evidence is `checkpoint-final-summary.json`
  and `checkpoint-workspace-tests.txt` / `checkpoint-workspace-clippy.txt`.
  Compiled recipe identity is
  `30c99e49dbd9bd23f654b050a5c1bc069bc1df39157274114c9fe25e44b17226`;
  prior full-library results remain historical until rerun with the final build.
  The newly compiled debug CLI also passes isolated library-management (2 owned
  WAVs) and review-fixes (`-CleanCheckout`, 2,000 synthetic records) regressions;
  the real legacy fixture check is explicitly skipped. Evidence is
  `checkpoint-library-management.txt` and `checkpoint-review-fixes.txt`. CLI SHA256:
  `76DB6FF2AAD50F081EFBE315326C6AA62419A86D28DA156BB710F4357C5C5B79`.
  No current installation or working database was changed.

- Reconciled the reviewed `aecd59b` download/decompression patch into this isolated
  worktree (no cherry-pick or original-checkout writes). Network body bytes and
  decompressed database bytes have separate totals, UI rows and bounded updates.
  Existing catalog admission and actual snapshot-format validation remain in place.
  Added `CachePreparation::Migrating(SnapshotUpgradeProgress)` to preserve exact
  shared migration phases and committed audio-row counts instead of mapping them
  to generic building/saving messages. Cache-copy page counts stay distinct.
  Final default workspace/all-target run exits 0: **67 targets, 1,130 passed,
  0 failed, 75 ignored**. Formatting, diff whitespace checks and workspace/all-target
  Clippy with `-D warnings` pass. The expanded remote tests check that page counts,
  row counts and backup/validation/merge/publication/checkpoint stages survive
  end to end; duplicate reports of the same committed cursor are permitted, but
  counts must be monotone with the correct final total. All **193 remote tests**
  and the **5 progress-display tests** pass within the full suite (overlapping counts).
  The real Windows Slint render test also passes separately, producing **18 PNGs**;
  four representative Chinese/English transfer and migration screenshots were
  visually inspected with no clipped progress text or overlapping bars.
  Evidence: `progress-final-summary.json`, `progress-workspace-tests.txt`,
  `progress-workspace-clippy.txt`, `progress-display-tests.txt`,
  `progress-render-tests.txt` and `progress-render/` under `.rust-validation/search-storage-b`.
  This is debug/default-feature and real-render evidence, not isolated release/ASIO
  packaging or the finalized full-library rerun. No current installation was changed.

- Added dedicated read-only `migration::diagnose` and CLI `db migration diagnose`.
  Planning uses the same diagnosis: retained table B-trees, retained foreign keys,
  ID-watermark evidence, physical FTS shadow storage and docsize membership.
  Physical business corruption is distinct from retired-table corruption, broken
  references, unavailable extension functions and unknown future requirements.
  No diagnosis authorizes automatic reconstruction. Supported FTS damage is
  repaired only in the owned candidate; original damage is recoverable on rollback.
  Readable orphan play-history rows can be retired, and rollback restores their
  original references/counts. DB migration tests **29 passed**, CLI
  migration tests **4 passed**; these include six new fault/diagnosis cases.
- Required schema definitions now compare type, owning table and SQL tokens,
  preserving literal whitespace/case. Only eight catalog-owned indexes and the
  ID-watermark trigger can be replaced in a candidate. Missing/invalid sequence
  evidence or a reserved name belonging to another object blocks planning.
  Tests cover wrong columns, partial predicates, collation, string literals,
  trigger bodies, missing indexes, ambiguous ownership and damaged watermarks;
  business hashes and original rollback schema remain unchanged.
- These changes alter the compiled recipe identity. The earlier 909,342-row
  integrated migration is historical evidence, not a rerun of this executor.
  Final default workspace/all-target suite exits 0: **67 targets, 1,122 passed,
  0 failed, 75 ignored**. Formatting and workspace/all-target Clippy with
  `-D warnings` pass. A newly compiled debug CLI passes library-management
  **10 checks** (2 owned WAVs) and review-fixes **10 checks** (2,000 synthetic
  records, `-CleanCheckout`). The real legacy fixture check is explicitly skipped;
  no working library or current installation is opened or upgraded.
  Final evidence is `diagnosis-complete-summary.json`, the `diagnosis-complete-*`
  logs and `diagnosis-verified-clippy.txt` under `.rust-validation/search-storage-b`.
  Recipe identity is `a19beffbbc57728a62ef6f7dc5d601ceb501086d2259eb715251ddbcb9cdcca1`.
  Isolated release/ASIO packaging and remaining goal items stay open.
- Corrected the SQLite internal-name filter: SQL `LIKE 'sqlite_%'` also excluded
  legal extensions such as `sqliteX_private`. The inspector and schema blueprint
  now use the literal underscore in `GLOB 'sqlite_*'`. A focused end-to-end test
  passes for lower/mixed-case lookalike names, signed implicit rowids, BLOB/NULL
  values and AUTOINCREMENT high-water retention through migration and rollback.
- The initial broad diagnosis run found unnecessary replacement of an already
  equivalent ID trigger definition in the v3/v4 regression. Blueprint construction
  now retains equivalent original SQL and replaces only damaged owned objects;
  the unchanged six-case schema-v5 suite passes. An overlapping Windows test run
  then blocked a linker output; all runs ended before the successful serial retry.
  The final `diagnosis-complete-*` logs supersede these incomplete/broad runs.

- Live peer admission now uses the same mutual application/protocol and actual
  format checks for connection probes, revision polling and each Metadata or
  management write. No previous successful handshake is cached as permission
  after a server upgrade. Unsupported peers cannot appear up to date solely
  because library ID and revision remain equal. Each modern mutation also sends
  the bounded client catalog for server-side admission after authentication;
  malformed, duplicate, oversized or incompatible catalogs return
  `409 PEER_VERSION_UNSUPPORTED` before handlers. Headerless historical JSON
  callers retain existing authentication/identity behavior; schema-6 snapshots
  still require full format negotiation. Regression cases cover an intervening
  version/range/feature change, zero transmitted mutations, unchanged client
  cache bytes/state and unchanged server Metadata/revision/data_version.
- Prior live-admission checkpoint: full default workspace/all-target tests exit 0,
  **67 targets, 1,111 passed, 0 failed, 75 ignored**. Formatting, diff whitespace
  checks and workspace/all-target Clippy with `-D warnings` pass. The earlier
  live-admission broad run exposed old CLI/GUI mocks without a version handshake;
  those fixtures now explicitly answer supported legacy health/capability probes,
  and the final full run supersedes the failure log. Focused runs also pass:
  remote 190, GUI remote-mode 64 (36 helper ignores), CLI identity 5, server
  compatibility 2, plus the byte/state-preservation case. Counts overlap and must
  not be added to the workspace total. These are debug/default-feature results,
  not final ASIO, release packaging or a repeated finalized full-library migration.

- Pinned vendor engine 3.54.1. Actual source ID and contentless-delete aggregate
  statistics/native BM25 tests pass, including signed extreme IDs, repeated
  replacement, deletion, rollback and optimize.
- `audiolib-core::compatibility`: owned format/capability/evidence DTOs, mutual
  SemVer checks, exact snapshot format IDs, three dispositions with a separate
  blocked state. Five focused tests pass; HTTP integration is described below.
- `audiolib-db::migration::inspect`: read-only pinned observation, stronger source
  identity, unknown-feature handling and typed SHA256 digests of all ordinary
  business tables. Regenerated fields are explicitly excluded; unknown extension
  data participates. Implicit rowids and AUTOINCREMENT high-water values are included.
- Shared `migration::{plan,apply,resume,verify,rollback}` now has a first tested
  implementation: canonical source/schema/business binding, exact compiled recipe
  and tokenizer identity, Windows free-space observation, source guards, immutable
  backup checksum, committed copy/index cursors, owned legacy normalization,
  SQLite backup publication with a receipt, checkpoint recovery and rollback.
  Writable provider startup and owned snapshot upgrades now call this engine.
  `discover` uses the source marker or an owned sibling registry; explicit rollback
  holds automatic upgrades until a new plan/apply. Registry identity and reparse
  checks reject unrelated files without replacing them.
- Explicit user follow-up: the next schema upgrade retires `play_history` and
  `audio_files.play_count`, with complete original values kept in rollback backup.
  `last_played_at` is retained. Target format is now `ale-s6-cld1-t3-m2-p0`,
  including `play_history_retired=1`. The old full-library prototype did not include
  retirement and is not validation of this final format.
- Compiled schema operations cover field additions with defaults/NULL, derived
  field removal and explicit table/field retirement. Public apply rejects an
  edited plan/recipe. Dependency checks reject surviving views, triggers or FKs
  that refer to a removed object. More advanced type/rename/backfill adapters are
  still future registered steps, not claimed implemented capabilities.
- `search_storage`: common projection expressions, new contentless synchronization
  triggers, empty-schema blueprint rewritten by SQLite itself, signed-ID index
  batches and structural checks. Unknown business columns/constraints are retained;
  dependencies on removed columns stop planning. Source schema remains unchanged.
- Schema 6 is now the isolated writable target. Fresh creation builds it in one
  transaction; v1..v5 normalization remains separate. Raw Metadata, scans, batch
  annotations and schema-6 snapshot patches drive the common FTS triggers.
  Historical read-only hydration retains old counts; schema-6 DTOs return zero.
  Playback recording is a validated no-op and history is empty. No deleted-field
  access remains in the adapted application write paths.
- Import and restore own the whole staging/job subtree. Maintenance avoids
  VACUUM's implicit-rowid renumbering. Restore page-size adaptation verifies all
  business identities before publication and blocks if they change.
- CLI `db migration inspect/diagnose/plan/apply/resume/verify/rollback` is implemented.
  It requires an explicit existing `--database` and bypasses ordinary startup.
  Plan output is create-new, apply binds the source, Ctrl+C preserves committed
  progress, and resume/verify/rollback discover the active job. Local progress
  JSON is stderr; stdout contains the command result.
- A CLI end-to-end regression exposed stale schema-6 search markers after repair.
  Finalization now uses the shared target marker routine; candidate verification
  checks actual readiness. Copy batches regenerate source-tag keys before user
  triggers are installed, so repair does not fire extension UPDATE side effects.
- Duration-filter cause verified in `filter-diagnose`: the default relevance
  membership SQL chooses `idx_duration` and visits wide rows for `missing=0`.
  A proven `idx_audio_visible_filter` covers the same 424,888 IDs. Four alternating
  trials: planner 200.4–459.8 ms; covering 37.5–41.9 ms; ID SHA256 identical.
  This is the membership subquery, not end-to-end GUI latency.
- `ranked_search` now uses that proven covering index only for present files,
  no folder scope and only sample-rate/duration filters. Other predicates,
  missing or malformed optional indexes retain the original plan.
- Focused migration checks cover retirement and rollback of nonzero counts/history,
  unknown extension data/implicit IDs, field default/NULL copy, changed-source
  rejection, every outer phase, committed-publication recovery, guarded legacy
  writes, corrupt backup, later schema/annotation changes blocking rollback, and
  WAL checkpoint Busy recovery.
- Early implementation checkpoint (superseded by the final suite above): default-feature core library run **53 passed**,
  db library run **136 passed, 3 ignored** (including the **19** migration cases).
  CLI migration tests **3 passed**; six scanner/
  embedded-Metadata suites **19 passed**; restore/page-size suite **3 passed**.
  Core/db/CLI all-target default-feature Clippy with `-D warnings` passes.
  Workspace formatting and `git diff --check` pass.
  The ignored tests are the opt-in million-record benchmark and two explicit
  full-library oracles. Whole-workspace tests, supported regression scripts and
  isolated packaging are still outstanding; integrated evidence is below. Many older integration fixtures still build
  v5-shaped rows directly and need intentional historical/current separation;
  do not interpret the focused successes as a green complete suite.
- Initial provider/scanner runs found fixture connections missing the new scalar
  function registration. Adapted fixtures now call the same
  `register_library_functions` as owned tools, retaining their behavioral checks.
  One scanner expectation now asserts the explicitly retired playback count is 0.
- A regression exposed VACUUM renumbering implicit rowids in extension tables.
  The executor now copies into a fresh file and merges FTS segments, without
  VACUUM. The new final size must be measured; it is not the old prototype's
  VACUUM result. No claim of completely reclaimed free pages is made.
- HTTP negotiation is now implemented in this worktree. `/capabilities` carries
  the shared manifest, `/library` reports the observed completed format, and a
  schema-6 snapshot requires exact format support plus mutual application ranges.
  Rejection precedes ETag handling, backup and compression. Both ordinary and
  304 responses describe the prepared format. The remote client checks the
  handshake, bounded/unique headers and actual downloaded file before publishing.
  Compatibility 409s do not enter the BUSY retry loop.
- Cache metadata separates server `revision` from `format.source`,
  `format.local` and local conversion `generation`. A current legacy snapshot
  remains schema 3–5; stale search state converts only in an owned copy. Equal
  revisions alone no longer permit reuse when endpoint/source format differs or
  provenance is absent. Historic metadata stays usable offline; online sync
  establishes source provenance through one fresh download.
- Download/cache conversion now owns a whole temporary job subtree. Normal
  completion and cooperative cancellation clean backup/registry/guard files
  together, keeping the previously active cache intact. Abrupt-process-exit
  recovery/cleanup of these temporary jobs still needs coverage; no age-only
  recursive cleanup was added.
- Retired playback now has matching server and ordinary remote behavior:
  the compatibility endpoint validates/reads without changing revision; the
  normal remote provider/CLI no-op sends no play request, writes no cached
  record and emits no write event. Returned historical counters are ignored by
  the schema-6 Metadata patch adapter. Low-level HTTP compatibility methods
  remain available for endpoint tests.
- Latest full remote library run: **189 passed, 0 failed**. The real server/client
  HTTP test also passes, covering old-client refusal before snapshot preparation,
  actual schema-6 sync, unchanged server data_version/revision on playback,
  same-revision format mismatch forcing re-download, and incremental Metadata
  patching/search without a full re-download. Earlier 20 failures exposed old
  fixtures that mislabeled newly created v6 files as legacy or used obsolete
  test version strings. A schema-only historical v5 fixture now separates those
  cases; complete actual archived schema-3/4 variants are still required.
- The source-format metadata enlarged a desktop provider-swap enum. Its owned
  optional state is now boxed; callers retain the same ownership and swap order.
  Default-feature workspace all-target Clippy with `-D warnings` passes after
  the final fixture edits, as do formatting and `git diff --check`.
  The full workspace/all-target test run finished with **34 failing targets**.
  Many are stale fixtures using removed derived/play fields or pre-negotiation
  HTTP expectations; this is not a passing workspace test result. Subsequent
  targeted fixes and successes are listed below; remaining suites still need work.
- Integrated full-library CLI plan/apply/verify/rollback completed in the owned
  `audiolib-plan-b-integrated-54e8aa24e1d3480abde7d8881fbb14b1` directory.
  Source and input-copy SHA256 matched. 909,342 records migrated from
  **19,495,989,248 to 10,612,101,120 bytes (-45.568%)**. Plan 21.70 s,
  apply 1,001.12 s, independent verify 83.30 s; migration overlapped compilation.
  A hash-verified copy retains the migrated result. Rollback on the originally
  bound test job completed in 211.54 s and restored schema 5, **752 history rows,
  759 total play counts and all 635 non-NULL last_played_at values**.
- Full read-only oracles pass: **1,198,133 vocabulary rows**, all document lengths
  and aggregate statistics, **58 full query ID/native BM25 comparisons**, custom
  BM25 versus native with **0 ULP** difference, and **86 requests x 3 passes** of
  ordered DTOs/counts/facets. Only retired DTO play_count is normalized; timestamps
  and all other retained business fields are independently checked.
  See [integrated report](Search_Storage_B_Integrated_Validation.md); the older
  prototype report is not substituted for this actual p0 result.
- The first integrated timing still exposed a facet tail: target search P95
  481.9 / 375.3 / 239.7 ms across three passes, but the first duration-filter facet
  call took 1,102.4 ms. Warm BM25 reuse bypassed the numeric covering-index proof
  on the folder path. That path now uses the same checked covering index; all
  filters, cancellation and fallback budgets remain in place. After-fix timing
  is recorded separately, using the preserved migrated copy: all 258 before/after
  and source/target pairs remain equal. Target duration facets now take
  **50.3–86.8 ms**, search plus facets **104.3–167.9 ms**; target mixed-query search
  P95 is 374.7 / 349.7 / 336.2 ms. First-use Tags sorting still has a 1,639.2 ms
  outlier. Cache is uncontrolled; full performance acceptance is not complete.
- Required-index recovery defect fixed: owned pre-upgrade integrity checks no
  longer call missing performance indexes corrupt FTS; staging recreates the
  eight known required indexes before marking the target ready. Actual FTS
  structure/integrity and complete target validation remain mandatory. Existing
  missing-index read-only/writable tests now pass. The recipe hash changed after
  the full run; final release validation must re-run the finalized executor.
- Search fixtures now register shared SQLite functions and insert authoritative
  fields. Nullable legacy fixtures go through explicit owned import. Ranking
  tests assert play no-ops preserve the cache. Search benchmark generation also
  uses current columns/functions; the completed synthetic 1M result is recorded below.
- Latest selected results: DB lib **136 passed, 3 ignored**; search/facet suites
  pass across the targeted runs; visibility **5 passed**; CLI unit/migration
  **11 passed**. Server API **62**, root management **18**, scans **7**, management
  **7**, shutdown **6**, stats **2**, and real client/server negotiation **1** pass
  (103 tests across runs). Server startup progress now also passes (**1 test**),
  using historical v5 DDL and checking committed copy/index row counts. Copying
  logs have explicit phase/count/total fields. The upgrade-benchmark fixture suite
  (**4 tests**) and historical snapshot compatibility/derived-repair suite
  (**3 tests**) pass after using captured v5 external-content DDL. The latter's
  schema-3/4 labels test supported additive layouts, not recovered archived files;
  the earliest public repository commit already uses schema 5.
- Migration fixtures contain nonempty playlist items, project references and binary
  Waveform cache data. The latest focused run has **18 migration::tests passing**,
  including authorized retirement, full-digest rollback, generic added default/NULL
  columns and a committed index cursor with a stale outer Copying phase.
- The remaining historical/current fixtures are now separated. Captured v5 DDL
  supplies persisted derived fields and old counters; current raw fixture writes
  register the production SQLite functions. Legacy copies of scanner-produced
  rows copy only common business columns into the historical DDL, with an FK check.
  Tests retain source-byte, retained Metadata, stable-ID/reference and search oracles.
  Additive v3/v4 variants remain synthetic version-band evidence, not archived releases.
- Durable job state now advances to Indexing after the candidate commits its index
  phase. A crash between those commits resumes from the candidate watermark without
  re-copying rows. Progress relaying skips unrelated table events and generic
  no-count Indexing boundaries instead of reporting a false zero. Cancellation
  keeps the explicit owned job/source guard; callers resume/roll back or dispose
  of their complete temporary cache job. A published checkpoint-only target is
  readable; writable startup completes checkpoint recovery first.
- The 10,003-row progress fixture covers the entire signed ID range. Cooperative
  cancellation, resumed 5,003-row copy/index batches, abrupt child-process exit at
  copy/index/validation/checkpoint and unknown protocol rejection are covered.
  Old history is checked directly in the immutable backup, and target table/column
  absence is checked independently of the empty compatibility history API.
- Client tests now verify that playback sends no play request, leaves cached
  revision/Metadata unchanged, and allows profile edits/switches, close and snapshot
  publication. Empty remote history needs no HTTP request; scan history still reads
  the snapshot. Invalid-download fallback mocks use supported negotiation so the
  test actually reaches the invalid response, rather than failing earlier at handshake.
- A broad run exposed a scan-start error cleanup ordering race: settings exclusion
  could remain held during Waveform bookkeeping. It is now released when ScanService
  returns (the success callback already releases it earlier); the error still waits
  for final job cleanup. The final default-feature `--workspace --all-targets`
  test run exits **0: 1,109 passed, 0 failed, 75 ignored across 67 targets**.
  Ignored entries include opt-in full-library/1M cases and isolated GUI child helpers
  invoked by their parent tests; this is not blanket execution of every ignored test.
  Final workspace Clippy with `-D warnings`, formatting and diff whitespace checks pass.
  The earlier broad run's CLI no-op expectation and scan-start race are retained in
  its failure log and superseded by this complete successful run.
- Both supported regression scripts also pass against the isolated **debug** CLI:
  `validate-library-management.ps1` (10 workflow checks; 2 owned WAV records) and
  `validate-review-fixes.ps1 -CleanCheckout` (10 checks, including cross-process
  overlapping scans of 2,000 synthetic records). No real library or audio output
  was opened; source WAV bytes were preserved. CLI SHA256:
  `380107d5c6a8bff0755d0df2b1bb3e8f34224c704cdc651c45192925d81d39ef`.
  This is development smoke evidence, not release/ASIO/portable validation.
- Playback IPC and server-package validators have been adapted for retired history
  and schema 6/explicit format identity. Both modified PowerShell files parse;
  parsing alone is not execution of those release validators.
- The 1,000,000-row synthetic run completed (26 cases, 100 mixed queries, fragments,
  edge cases, folder facets, fresh providers), exit 0 in **276.55 s** including
  **260.21 s** generation. Main file **982,298,624 bytes**, no missing records.
  Shared-provider search-page P95 **164.4 ms**, search+facets P95 **258.2 ms**,
  with 77/100 ranking cache hits. Fresh-provider first-search P95 **335.4 ms**;
  first search+facets P95 **536.8 ms**, which FAILS that combined 500 ms gate.
  Opening is separate (P95 34.1 ms). OS cache is uncontrolled, synthetic Metadata
  and folder distribution are much simpler than the real 10.61 GB library.
  See the integrated report and `synthetic-million.json`; exit 0 is not a claim
  that every performance criterion passed.

Raw local evidence is in `.rust-validation/search-storage-b/`:
`duration-before.json`, `db-lib-tests.txt`, `migration-lib-tests.txt`,
`migration-historical-tests.txt`, `migration-clippy.txt`,
`current-provider-lib-tests.txt`, `migration-current-path-tests.txt`,
`current-core-db-tests.txt`,
`migration-cli-tests.txt`, `current-scanner-tests.txt`,
`current-restore-page-tests.txt`, `current-provider-clippy.txt`,
`compatibility-unit-tests.txt`, `compatibility-e2e-tests.txt`,
`cache-format-tests.txt`, `remote-retired-play-tests.txt`, `remote-full-tests.txt`,
`workspace-clippy.txt`, `integrated-cli-build.txt`, `integrated-run.json`,
`current-search-tests.txt`, `current-search-repair-tests.txt`, `current-facet-tests.txt`,
`current-visibility-tests.txt`, `current-server-tests.txt`, `current-server-repair-tests.txt`,
`current-server-final-tests.txt`, `current-cli-tests.txt`, `current-workspace-clippy.txt`,
`current-check-status.json`, `historical-fixture-tests.txt`,
`current-startup-final-tests.txt`, `migration-reference-tests.txt` and
`million-benchmark-build.txt`, `historical-tests-pass3.txt`,
`current-cli-identity-tests.txt`, `current-migration-recovery-tests.txt`,
`current-recovery-clippy.txt`, `workspace-tests-current.txt` and
`workspace-tests-recovery-final.txt`, `current-recovery-clippy-final.txt`,
`current-library-management-smoke.txt`, `current-review-fixes-smoke.txt` and
`scan-startup-recheck.txt` and `current-recovery-final-summary.json`,
`workspace-tests-live-admission-final.txt`, `live-admission-clippy-verified.txt`,
`live-admission-remote-final.txt`, `live-admission-cache-preservation.txt`,
`live-admission-desktop-final.txt`, `live-admission-cli-final.txt`,
`live-admission-server.txt` and `live-admission-final-summary.json`. Earlier failing logs remain as evidence; use the
latest named focused run when checking a repaired target.
The integrated run directory keeps plan/apply output, timing and job artifacts.
No name-based process cleanup is used; validation scripts are restricted to their own temporary files and child processes. Another chat's separately committed
download/decompression progress `aecd59b` has now been reconciled in this isolated
worktree; its original NAS checkout was not edited. Another chat reported main backport
`d479a2c` / merge `88d8bf3` (PR #16); those are integration information, not
commits imported or validated by this worktree.

## Acceptance boundaries and recovery decisions

The requested implementation and isolated validation are complete. This is a
development payload, not an installed or published release. A later release must
select its stable version/catalog and perform its own deployment acceptance.

- Archived v3/v4 libraries were unavailable. Captured v5 DDL, systematic additive
  v3/v4 layouts and old derived/recovery protocols provide the stated matrix;
  they do not establish coverage of unavailable archived artifacts.
- Crash tests include uncommitted copy/index writes with hot journals and partial
  SQLite backup publication/rollback. Corrupt checkpoint counters, cursor prefixes,
  phase/flags and source binding are checked before resume writes. These are
  subprocess terminations, not physical power-loss or all filesystem fault tests.
- Remote cache crash leftovers are deliberately retained. A retry starts from the
  unchanged previous cache and cleans only its own successful job. Automatic
  reclamation/adoption of abandoned jobs is not implemented. Headerless legacy
  API callers retain the documented compatibility exception.
- The final workload is measured on Windows x64 with uncontrolled OS cache and no
  simulated concurrent writers. The real library has 909,342 records; the 1M
  dataset is synthetic. Mac, physical-cold, audio-start and all GUI latency claims
  require their own evidence.
- User-reported server fix `e7b2483` was inspected: runtime parent `a312094`, tests
  `9ab035f`, and documentation `e7b2483`. This worktree independently implements and
  tests equivalent no-write/current-record and empty-history behavior, including
  revision/data_version preservation. No commit was cherry-picked and the other
  branch's reported test results are not this worktree's evidence. No new local
  history store is introduced.
- The current installation, working library and original checkout remain unchanged.
  Original backups retain the retired playback data. No deployment, installation
  or publication is implied by these results.

The new contentless triggers replace the old external-content triggers; they call
one shared projection definition and execute inside the business statement's
transaction. This avoids duplicating all twelve index fields across app adapters.
The design now documents this choice; retain rev 3 and all twelve logical FTS columns.
