# AGENTS.md

This file provides guidance to Codex (Codex.ai/code) when working with code in this repository.

## Project Overview

**AudioLib Explorer** is a high-performance desktop audio library management tool for sound designers. It manages 1M+ WAV audio files with sub-500ms search latency using SQLite FTS5 full-text search.

**Current Phase**: Rust desktop implementation. Legacy Python source and its test scripts have been removed; previous committed versions preserve that history. The original database and local YAML configuration remain read-only migration inputs.

## Rust Workflow (Primary)

The current implementation is the Cargo workspace at the repository root. See `doc/Rust_Architecture.md` for its contracts and `doc/Rust_Validation.md` for actual verification results. Python architecture examples in retained design specifications are historical guidance; do not use SQLAlchemy ORM, tkinter, PyQt6, or Alembic for new Rust functionality.

- Rust 1.97.1 MSVC, Slint 1.17.1, rusqlite 0.40.2 with bundled SQLite FTS5.
- `crates/audiolib-core`: owned domain DTOs, DataProvider, validation and settings.
- `crates/audiolib-db`: versioned SQLite migration, search, folder hierarchy, Metadata editing and cancellable scanning.
- `crates/audiolib-audio`: streaming decode, resampling, CPAL playback and Waveform caches.
- `crates/audiolib-windows`: x64 ASIO driver identity and Windows path integration.
- `crates/audiolib-ipc`: bounded local named-pipe playback control; local-only, never a network listener.
- `crates/audiolib-remote`: blocking HTTP client for server mode (snapshot sync, WebDAV playback cache, DPAPI-protected secrets); see `doc/Rust_Remote_Mode.md`.
- `server/audiolib-server`: axum library server for server mode; its HTTP contract is `doc/api/README.md`.
- `app/audiolib-desktop` / `app/audiolib-cli`: GUI and CLI adapters.

Run `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings` and `cargo test --workspace --all-targets --locked`. For ASIO builds, dot-source `tools/prepare-asio.ps1`, then pass `--features audiolib-desktop/asio,audiolib-cli/asio`. The preparation script pins and verifies Steinberg SDK 2.3.4; never let the dependency build download an implicit SDK.

Release: `tools/build-release.ps1`; portable packaging: `tools/package-portable.ps1 -SkipBuild`. Packaging and `validate-portable.ps1` read the release EXEs from `$env:CARGO_TARGET_DIR` when it is set (else `<repository root>/target`); run the tools with `pwsh` (PowerShell 7) where possible, the privacy check no longer needs .NET Core `GetRelativePath`. Routine outputs replace `dist/latest/portable` and `dist/latest/AudioLibExplorer-latest-win-x64-portable.zip`. The fixed local installation is `<repository root>/local_release/`; update it using the mandatory workflow below. Keep runtime state in `%LOCALAPPDATA%/AudioLibExplorer/v1`, separate from binaries. Tests use temporary directories and must not modify the legacy database or WAV assets.

Release validation scripts: `tools/validate-runtime.ps1`, `tools/validate-playback-ipc.ps1` and `tools/validate-access-denial.ps1`. Real Windows ACL tests must remain within a verified owned temporary subtree and restore permissions in finally. The million-record performance runner is `cargo run --release --locked -p audiolib-db --bin search-benchmark -- --records 1000000 --queries 100`.

After packaging, run `tools/validate-portable.ps1` against `dist/latest/portable` for EXE identity, isolated first startup, CLI/UI and startup diagnostic checks.

After changes to scanning, configuration or Metadata, run `tools/validate-review-fixes.ps1` for release regressions. Scanning must acquire the per-database OS lock; leave its sidecar file in place after unlocking to avoid file-replacement races. Bind GUI drafts to the displayed file ID and an edit revision, keep saves ordered, and drain requested writes before closing.

Library management uses schema v5 and the owned contracts in `audiolib-core/src/library.rs`; server snapshots support schema 3 through 5. Older snapshots are validated and any required derived-state upgrades run only in an owned copy. Run `tools/validate-library-management.ps1` after backup/restore, relink/removal, import/export, scan history or multi-library changes. Restore and maintenance must validate an owned snapshot, create a rollback backup, drain GUI writes and scans, and invalidate old reader/UI generations. An unrelated SQLite file must never replace a library. All removal operations are database-only; never delete a source WAV. See `doc/Rust_Library_Management.md` for the delivered workflow.

Ordinary CLI commands must validate existing libraries and must not create an explicit or missing non-default library; use `db create` or `db import`. Maintenance/restore must exclude all cross-instance writes through the administration lease, including single-record Metadata and playback history, while scans still permit ordinary annotations. Restore adapts page size only in its owned staging copy. Resolve and validate the real WAV target before relinking. Protect shared preferences with the desktop instance lease before database initialization. Playback history distinguishes successful play sessions from load generations: Stop/Ended followed by Play starts a new session, while pause/resume, seek, transform changes and repeat retain the session.

All business data accesses go through DataProvider and return owned values. SQL values must be parameterized; sort fields must be allowlisted. GUI updates return to the Slint event loop. Audio callbacks must not perform application file I/O, database work, logging, allocations or blocking synchronization. Handle cancellation and stale requests explicitly. Preserve all existing library Metadata and user annotations when importing and rescanning.

Document Rust `unsafe` safety invariants and keep FFI narrow. Add a source path comment at the start of each Rust module and document public interfaces. Continue using the terminology in Technical_Architecture.md Appendix A and the performance target **P95 latency <500ms for 1M+ records**.

## Mandatory local development and release installation

`local_release/` is the persistent local installation used for manual acceptance and everyday use. Reuse this exact directory; do not create versioned or timestamped local installation directories. `dist/latest/` remains the packaging output, and `target/` remains the build output. Neither replaces the required local installation step.

For every completed change affecting the application, follow this sequence:

1. Run the required formatting, Clippy and workspace tests above, plus the applicable `tools/validate-*.ps1` regressions for the changed subsystem. Keep test databases in isolated temporary directories.
2. Build the latest source with `tools/build-release.ps1`, then package with `tools/package-portable.ps1 -SkipBuild`. Use the same ASIO selection for build and packaging; the default includes the verified ASIO SDK. Never install stale binaries from an earlier build.
3. Run `tools/validate-portable.ps1 -BinaryDirectory dist/latest/portable`. Only proceed after successful build, packaging and validation.
4. Gracefully close any application instance using `local_release/`, allowing requested Metadata writes and scans to drain. If files remain locked, report the blocker instead of forcing termination or claiming installation succeeded. Create `local_release/` if absent, then overwrite its application files with the validated portable payload: both EXEs, launcher, packaged documentation, resources, example configuration and licenses. Copy only package-owned files; do not delete or mirror-purge the installation directory or overwrite local runtime data. Verify resolved destinations stay within this repository's `local_release/` before any cleanup.
5. Verify SHA256 identity of both installed EXEs against the validated package, then run `tools/validate-portable.ps1 -BinaryDirectory local_release` (add `-CleanCheckout` where the legacy fixture is unavailable). This validation uses isolated temporary data, never the user's working database. Use `local_release/Start-AudioLibExplorer.cmd` for subsequent manual acceptance and normal launches.
6. Report the build/version or source commit, validation results and fixed installation path. A successful build alone does not complete an application development task; the local installation must also be updated and verified. Documentation-only changes do not require rebuilding or reinstalling unchanged binaries.

### Persistent local database and settings

- Keep the working database, `settings.json`, logs and caches on this machine at `%LOCALAPPDATA%/AudioLibExplorer/v1`; preserve any existing explicit `AUDIOLIB_DATA_DIR` and configured library paths. Do not relocate or recreate an existing library merely to install a build. Program files in `local_release/` and persistent data have separate lifecycles.
- Every local upgrade must retain all libraries, Metadata, annotations, scan history and preferences. Never copy an empty, fixture, CI or packaged database over the working library, and never delete SQLite sidecars as installation cleanup. Use the supported backup/restore workflow when data migration or recovery is needed.
- Restore the user's normal data-directory environment after isolated validation. Daily GUI/CLI use must continue to select the same local data directory and active library across builds.
- Keep `/local_release/` ignored by Git. Do not upload local databases, preferences, logs or caches as release assets; publish only the clean portable package from `dist/latest/`.

### Published release synchronization

After a requested GitHub Release has been successfully published, update and verify the same `local_release/` installation before declaring the release task complete. Install the published portable payload, or a local payload whose hashes match that release; verify the installed version and EXE hashes against the published artifact. If CI produced the release, do not assume an unrelated local build is identical. For a downloaded CI artifact, verify its published archive checksum and use its EXE hashes as the identity baseline; the existing portable validator also requires matching local release build outputs, so do not claim it passed without those prerequisites. Preserve the local data described above throughout the update. Report remote publication and local installation status separately if either step is blocked. This synchronization rule does not authorize publishing a release unless the user requested publication.

## Repository scope and configuration

The active stack is Rust, Slint and SQLite FTS5. Desktop text supports English and Chinese switching at runtime. Older references in design documents describe the prior Python application.

Rust preferences use `settings.json` in `%LOCALAPPDATA%/AudioLibExplorer/v1`; `AUDIOLIB_DATA_DIR` selects an isolated alternative. See `config/rust_settings.example.json`. Preserve the local `config/audiolib_config.yaml`, original `data/audiolib.db` and its SQLite sidecars for first-run read-only import. Preserve source WAV fixtures in `test_audiolib/P-51 Mustang` and the shared `resources/audio_terms.txt`, which is included by Rust at compile time.

Formal Rust unit/integration tests and supported `tools/validate-*.ps1` scripts are maintained source. `.rust-validation/` contains disposable generated output and is recreated by validation tools. Do not treat original libraries, source audio, the verified ASIO SDK or formal regression tooling as temporary artifacts. Do not add Python runtime dependencies or restore obsolete Python launch/test commands.

## Essential documentation

- `doc/Rust_Architecture.md`: current layers, contracts, migration and audio pipeline.
- `doc/Rust_Library_Management.md`: library operations, integrity and recovery.
- `doc/Rust_Spectral_Audition.md`: Waveform, Spectral Peaks and audition behavior.
- `doc/Rust_Validation.md` and `doc/Rust_Review_Fixes.md`: measured verification and reviewed corrections.
- `doc/INDEX.md`: current documentation navigation.
- `doc/Technical_Architecture.md` Appendix A: mandatory terminology. The rest retains historical design examples.
- `doc/AudioLib_Explorer_PRD.md`, `doc/Security_Guidelines.md` and `doc/Testing_Strategy.md`: retained requirements and design history; apply their requirements through the current Rust contracts and tools.

## Performance targets and terminology

| Metric | Target | Context |
|--------|--------|---------|
| Search latency (P95) | <500ms | 1M+ records |
| Search latency (P50) | <100ms | 1M+ records |
| Playback start | <50ms | Button click to actual audio output |
| UI response | <100ms | User interactions |
| Scan throughput | >100 files/sec | Metadata extraction on SSD |

Use **Waveform**, **Metadata**, **FTS5** and **DAW** consistently. State the primary metric as **P95 latency <500ms for 1M+ records**. Report actual benchmark conditions and hardware measurements separately from these targets; driver-reported latency is not button-to-output measurement.

Validate user inputs through Rust core contracts, bind SQL values, validate real file targets against configured scan roots, and cap search queries at 500 characters. Chinese indexing uses jieba-rs with the shared audio terminology dictionary. Preserve unknown extension Metadata and user annotations during migration and rescanning. Playlist/project-reference data is retained, while new editors, DAW parsers and plugins remain deferred; hide unimplemented entry points. Server mode exists through `audiolib-remote` and `server/audiolib-server`: v1 clients open a validated read-only snapshot, refuse library writes, never touch the local working library, and never log or export the token or WebDAV password.

## Git Workflow
The default branch is `main`. Matching `v<workspace version>` tags trigger `.github/workflows/release.yml` (Windows runner), which runs `tools/check-release.ps1`, `tools/ci-release.ps1` (including `tools/validate-portable.ps1 -CleanCheckout`) and publishes a GitHub Release. Pushes and pull requests run `.github/workflows/ci.yml`. Follow Published release synchronization above to update the fixed local installation after publication. Keep local libraries, source audio fixtures and build outputs outside Git. See README for release operations.
