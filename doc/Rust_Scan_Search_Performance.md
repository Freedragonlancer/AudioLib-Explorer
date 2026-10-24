# Scan and search performance — 2026-10-08

The application remains Rust + SQLite FTS5; no Python runtime, database-format migration or external service is introduced. The primary target remains **P95 latency <500ms for 1M+ records**.

## Implementation

- Scanning has one directory enumerator, a bounded Metadata worker pool, and one transaction writer. Automatic selection leaves two logical CPUs when possible and caps workers at eight. `scan --workers 1` selects serial probing; `--workers 2..16` selects explicit concurrency. Both queues hold at most twice the worker count; the existing batch-size and Metadata-byte limits still apply.
- Each worker owns its fingerprint reader. Incremental scans skip up-to-date Metadata; full scans avoid the unnecessary pre-probe fingerprint query. Workers never write SQLite. Cancellation wakes paused workers, disconnects blocked queues and discards uncommitted batches. The per-database OS scan lease remains held through worker shutdown and finalization.
- Batch writes reuse prepared SQL, read existing Metadata and fingerprints together, cache folder IDs within a batch, and consolidate eleven legacy header-column updates into one. User annotations and unknown extension Metadata are merged inside the writer transaction, preserving edits made while workers probe files.
- Ordinary ID3v2.2/2.3/2.4 tags seek past binary frames such as artwork instead of reading their payload. Text and comment frames still use the existing decoder. Complex tag flags and malformed layouts fall back to the bounded full decoder, retaining diagnostics. Playback continues to use the lightweight header path.
- Search sorts IDs and keys before loading wide Metadata records, with stable ID tie breaking and the original filters/visibility. Out-of-range pages retain the exact total without another page query. FTS directory statistics accumulate contiguous folder runs and then roll up per-folder counts, avoiding a temporary sorter with one row per match. Both stages share a SQLite read snapshot.
- No search result cache or extra persistent index was added. An experimentally measured covering index made directory statistics slower and was not included.

## Measurement scope

AMD Ryzen 7 9800X3D, 8 cores / 16 logical CPUs, Windows x64, optimized Rust 0.3.5 builds. Measurements use isolated databases and preserve source audio and the normal data directory. OS/file caches are warm or uncontrolled; these are not cold-disk guarantees.

The real-file scan used an anonymized commercial WAV sample on SMB; source paths, record counts and total capacity are omitted from the public report. The scanner seeks past PCM; many files contain approximately 4 MiB of SMED and 1.5 MiB of ID3 Metadata/artwork. Four/eight-worker trial results were similar, consistent with diminishing gains from concurrency on this storage and Metadata workload. Full extraction must retain all professional fields, not trade them for speed.

Final measured values and validation records are in the performance report. The search dataset has 1,000,000 synthetic records, ten query/filter cases, 200-record pages and 100 timed queries, using the existing benchmark's 64 MiB reader caches and 1 GiB mmap limit. The report distinguishes result-page latency from page-plus-directory latency; the GUI publishes pages before directory statistics finish. Synthetic records have less Metadata than many real production libraries.

| Measurement | Before | Final ASIO build |
| --- | ---: | ---: |
| Anonymized real-file full scan, including CLI startup | 40.679 s | 29.156 s |
| Anonymized real-file unchanged incremental scan | 1.249 s | 0.991 s |
| 2,000 tiny local synthetic WAVs, full scan | 0.430 s | 0.249 s |
| Synthetic local scan throughput | 4,650 files/s | 8,045 files/s |
| 1M search result-page P95 | 411.056 ms | 401.902 ms |
| 1M result page + directory counts P50 | 94.491 ms | 85.481 ms |
| 1M result page + directory counts P95 | 621.454 ms | 536.618 ms |

Real full-scan time decreased 28.3%; combined query/directory P95 decreased 13.7%. Result-page P95 meets the primary 500 ms target, but the combined page/directory workload still exceeds 500 ms. The result-page change alone is small and should not be treated as a large ranking-speed gain. Local tiny files measure the pipeline's overhead and do not predict throughput for large professional Metadata on SMB. Every real-file sample retained Description, Category, Subcategory, CatID, Library, Manufacturer and VendorCategory, all five embedded source groups and library-query recall; zero extraction errors/warnings and unchanged incremental records for every sample were verified.

## Reproduction

Use a new isolated `AUDIOLIB_DATA_DIR` and an explicitly created test database; never point benchmark commands at the working library.

```powershell
AudioLibCli.exe --database TEST_DATABASE db create TEST_DATABASE
AudioLibCli.exe --database TEST_DATABASE scan SOURCE_DIRECTORY --full --workers 1
AudioLibCli.exe --database TEST_DATABASE scan SOURCE_DIRECTORY --full --workers 8
AudioLibCli.exe --database TEST_DATABASE scan SOURCE_DIRECTORY
cargo run --release --locked -p audiolib-db --bin search-benchmark -- --records 1000000 --queries 100 --folder-facets --db NEW_BENCHMARK_DATABASE
cargo run --release --locked -p audiolib-db --bin search-benchmark -- --existing-db BENCHMARK_DATABASE --queries 100 --folder-facets
cargo run --release --locked -p audiolib-db --bin search-diagnose -- BENCHMARK_DATABASE
cargo run --release --locked -p audiolib-db --bin search-benchmark -- --existing-db BENCHMARK_DATABASE --queries 200 --folder-facets --tag-exclusion-cases
cargo run --release --locked -p audiolib-db --bin tag-exclude-diagnose -- BENCHMARK_DATABASE explosion door
```

Regression tests compare serial/4/16-worker extraction, unchanged/error counts, pause/resume/cancel, cross-process exclusion, Metadata preservation, every sort/visibility combination against direct SQL, and ID3 streamed/full decoding including malformed frames. A metered-reader test verifies that a 1 MB artwork frame needs fewer than 100 bytes of actual reads while retaining subsequent text. Required default/ASIO checks, release regressions and both package/installation validations must pass before delivery.
