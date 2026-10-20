# Search upgrade recovery and WAL lifecycle

The `47ddd03` through `c0e8af9` builds remain excluded from the planned Release.
The reported Mac startup took 954 seconds (411 seconds tokenization, 542 seconds FTS5),
and retained an 18.9 GB WAL. These builds are not release candidates. The user has
explicitly deferred a rerun on that Mac; this repair is accepted through code review,
functional regressions and owned-copy checks. Windows measurements do not establish
that Mac's new startup time or invalidate its earlier report.

## Resumable writable-library startup

Existing writable Rust libraries now upgrade derived search state offline in committed
batches of 5,000 records. Local startup and server startup use this path. Fresh database
creation, legacy import, explicit rebuild maintenance and exclusively owned downloaded
snapshots retain their existing atomic/discard-on-cancellation contracts.

The upgrader acquires the library scan lease followed by the exclusive administration
and search-upgrade leases. It creates one retained `audiolib-rollback-*.db` backup beside the library, validates
that owned copy with SQLite quick-check and foreign-key checks, then records its backup
path, protocol, target revision, phase and watermarks in `audiolib_search_upgrade`.
The original Metadata, annotations, source paths, history and source WAVs are preserved.

| Phase | Durable work and recovery |
| --- | --- |
| `backing_up` | Copy and validate the original library before changing its derived state |
| `tokenizing` | Update derived text and the signed-ID watermark in the same transaction |
| `indexing` | Insert the next signed-ID batch into an unpublished FTS5 table and commit its independent watermark |
| `validating` | Verify FTS5 internal consistency and indexed record count; atomically rename the completed FTS and install authoritative triggers |
| `checkpoint` | Check SQLite's TRUNCATE result, clear the maintenance state and checkpoint the final metadata commit |

No complete FTS5 `rebuild` or final unbounded `optimize` follows the incremental insertion.
The nullable watermark supports `i64::MIN`, negative IDs, gaps and `i64::MAX`.
Cancellation rolls back only the active batch. Abrupt process exit retains committed
watermarks; the next writable open resumes them without making another rollback backup.
Unknown protocols or inconsistent target revisions fail closed.

During maintenance, a temporary feature revision rejects old binaries reopening the
database. Persistent write guards also reject older already-open connections after a
crash releases OS locks. The incomplete FTS has a separate name, so older readers cannot
query a partially populated canonical FTS table. Current writable-provider readers
participate in the shared search-upgrade lease and check the durable maintenance marker.
Other maintenance operations retain their existing reader-snapshot behavior.
Existing older application instances should be closed before upgrading. Raw SQL writers
must follow the administration lease protocol used by the application.

Upgrade remains an offline operation: server business endpoints return 503 until it
finishes. `/api/v1/health` includes optional `upgrade`, for example
`{"phase":"indexing","completed":5000,"total":900000}`. Counts describe committed rows,
not estimated overall completion. The local GUI displays those stages in English or
Chinese. Closing its startup window or stopping the server requests cancellation and
waits for the startup worker's safe exit.

## Checkpoints and snapshot ownership

Each committed upgrade batch runs `wal_checkpoint(TRUNCATE)` outside its transaction.
The implementation reads `busy`, log frames and checkpointed frames; a blocked
checkpoint is a retryable error and cannot be reported as successful reclamation.
Never delete SQLite WAL/SHM files to recover space. A busy old read transaction must
finish before the retry can truncate the WAL.

Graceful server shutdown stops admission of snapshot jobs, drains admitted jobs and
scans, releases cached snapshots, checkpoints under exclusive administration, and
persists the resulting revision baseline. SQLite connection ownership is released
normally; the former `mem::forget(service)` workaround is removed. SQLite may change
the observer's `data_version` when TRUNCATE resets a WAL, so this physical transition
does not increment the content revision. External commits before shutdown are counted;
changes made while stopped remain detectable on restart.
Snapshot copying checks cancellation between bounded SQLite backup steps; compression
checks between 1 MiB inputs. Unpublished staging files are removed when these jobs stop.

Responses keep ownership of their snapshot files until their last reference is dropped.
Every new snapshot directory carries a held `.owner.lock`. Stale cleanup removes only
marked directories whose owner lock can be acquired, leaving active downloads, other
instances and unmarked older directories untouched. A failed or timed-out shutdown
reports incomplete maintenance instead of claiming its files were reclaimed.

The live snapshot cache still holds its plain SQLite copy and optional compressed copy
while serving that revision. The rollback backup is deliberately retained for recovery.
Checkpointing neither shrinks the main database nor removes those valid files. Report
main DB, peak WAL, rollback backup and snapshot cache separately; do not promise a fixed
free-space multiplier or a total startup time from batch size alone.

## Reproducible measurement

`upgrade-benchmark --source <old.db> --live` pins the original read-only and copies it
into its own temporary directory. Only that copy runs the writable WAL upgrade,
including rollback backup, validation and checkpoints. The tool reports each stage,
confirms an empty WAL after successful open and validates the finished owned database.
It prints its temporary directory for an external disk/memory sampler and removes all
owned benchmark output at completion. Run the release binary and preserve the source
checksum. Without `--live`, it measures the existing atomic DELETE-journal snapshot path.

Compare the same actual source revision, hardware, filesystem, journal mode and cache
conditions. The earlier Windows 236.6-second measurement upgraded tokenization 2→3
in a warmed 19.49 GB owned DELETE-journal copy; the Mac report upgraded 1→3 in a live
WAL library growing from 10.3 to 19.5 GB on external SSD. They are not matched runs.

The search target remains **P95 latency <500ms for 1M+ records**. The previous 42 GUI
samples, seven samples at or above 500ms and unexecuted 224-sample matrix retain their
recorded limitations; this lifecycle fix does not establish search-performance acceptance.
