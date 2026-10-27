# 0.8.0

程序 0.8.0 | API /api/v1/ | schema 7 | 快照兼容 3..=7

新增：按音效库/专辑归类（文件内嵌 Library）、侧栏音效库列表与封面缩略图、`albums` / `search --album` 命令。

# AudioLib Explorer release notes

## 0.6.5: update check and portable package fixes

Release identity: program `0.6.5` | API `/api/v1/` | schema 6 | snapshot format
`ale-s6-cld1-t3-m2-p1`. No database migration; schema and snapshot formats are unchanged.

Versions: program 0.6.5 | API /api/v1/ | schema 6 | snapshot compat 3..=6

Upgrade notes:

- 0.6.5 is a patch release compatible with 0.6.x peers (`>=0.6.0, <0.7.0`); schema and
  API are unchanged, so client and server can be upgraded independently.
- The 0.6.4 Windows portable ZIP used backslash entry names and could not be installed by
  the auto-updater; 0.6.5 packages install normally from 0.6.3 and 0.6.4.

Changes:

- Updater: an empty release list is retried and then reported as an error instead of
  "up to date"; a newer release skipped for a missing or invalid package is reported
  explicitly; successful checks are logged (#62).
- Packaging: the Windows portable ZIP is written entry by entry with `/` separators, and
  validation rejects backslash, directory, `..` and duplicate entries (#63).

## 0.6.4: preview volume dB label live refresh fix

Release identity: program `0.6.4` | API `/api/v1/` | schema 6 | snapshot format
`ale-s6-cld1-t3-m2-p1`. No database migration; schema and snapshot formats are unchanged.

Versions: program 0.6.4 | API /api/v1/ | schema 6 | snapshot compat 3..=6

Upgrade notes:

- 0.6.4 is a patch release compatible with 0.6.x peers (`>=0.6.0, <0.7.0`); schema and
  API are unchanged, so client and server can be upgraded independently.

Changes:

- Desktop: the preview volume bar's dB readout now refreshes live while dragging
  (and shows `-inf dB` at 0), with a regression test (#58).

## 0.6.3: startup auto-update, WASAPI idle release, beta release flow

Release identity: program `0.6.3` | API `/api/v1/` | schema 6 | snapshot format
`ale-s6-cld1-t3-m2-p1`. No database migration; schema and snapshot formats are unchanged.

Versions: program 0.6.3 | API /api/v1/ | schema 6 | snapshot compat 3..=6

Upgrade notes:

- 0.6.3 is compatible with 0.6.x peers (`>=0.6.0, <0.7.0`); client and server can be
  upgraded independently.

Changes:

- Desktop and server: check GitHub Releases at startup and apply updates with
  verified downloads, staged rollout and startup recovery (#53).
- Audio: the WASAPI output stream is released when preview is idle, so other
  applications and DAWs can use the device in exclusive or shared mode (#54).
- Release flow: beta tags and pre-releases, side-by-side beta install, and a scripted
  beta smoke test (`tools/beta-smoke.ps1`) (#51, #52).
- Desktop: the preview volume dB label refreshes live while dragging (#50).
- Server: waveform bundle supports `If-None-Match` (304); unauthenticated requests
  still get 401 (#49).

## 0.6.2: snapshot download and low-disk fixes, clearer client errors

Release identity: program `0.6.2` | API `/api/v1/` | schema 6 | snapshot format
`ale-s6-cld1-t3-m2-p1`. No database migration; schema and snapshot formats are unchanged.

Versions: program 0.6.2 | API /api/v1/ | schema 6 | snapshot compat 3..=6

Upgrade notes:

- 0.6.2 is compatible with 0.6.0 and 0.6.1 peers (`>=0.6.0, <0.7.0`); client and
  server can be upgraded independently.
- v0.6.1 shipped only the macOS server package; the Windows client package resumes
  with 0.6.2.

Changes:

- Server: fixed an occasional 500 on the first waveform bundle download (#43).
- Server: when disk space is low, idle snapshots of old revisions are evicted before
  the server answers 507 (#45).
- Server: `/library` now reports only the database file name, not its path (#46).
- Client: tolerates a missing `LibraryInfo.database`; clearer messages for
  `HOST_FORBIDDEN`, `ADMIN_LAN_ONLY` and 429 rate limiting (showing `Retry-After`);
  the `.local` host-name hint is shown only for 401 responses that did not come from
  the AudioLib server (#44).
- Desktop: the list-waveform worker releases its database connection when idle,
  fixing a Windows test flake (#47).

## 0.6.1: server hardening, waveform bundle cache and patch compatibility

Release identity: program `0.6.1` | API `/api/v1/` | schema 6 | snapshot format
`ale-s6-cld1-t3-m2-p1`. No database migration; schema and snapshot formats are unchanged.

Upgrade notes:

- **One full snapshot download after upgrading from 0.6.0.** Snapshot ETags changed
  from weak `W/"<library_id>-r<revision>"` to strong `"snapshot-<UUID>"`, so the
  first sync re-downloads the library once; later syncs are incremental as before.
- **Clients and server must both be 0.6.1.** The version handshake is mutual and
  0.6.0 only accepts an exactly matching 0.6.0 peer, so 0.6.0 and 0.6.1 cannot connect
  in either direction. From 0.6.1 on, patch releases of the 0.6 line accept each other
  (`>=0.6.0, <0.7.0`), so later 0.6.x upgrades need not be simultaneous.
- **`manage_mode` now defaults to `lan`** (was `all`). Without an explicit setting,
  scans and deletions are only allowed from the LAN, even with a token.
  Set `manage_mode = "all"` to keep the previous behaviour.

Changes:

- Server: new **403 `HOST_FORBIDDEN`** for tokenless LAN requests whose Host is not
  an IP, a LAN host name or a configured address (DNS-rebinding protection);
  tokenless LAN access by IP or LAN host name is still allowed.
- Server: new **429 `AUTH_RATE_LIMITED`** after 60 failed authentications from one
  IP within 60 seconds. 500/507 error messages no longer include server paths.
  Each listener accepts at most 1024 connections. Tokens must be at least 32 characters.
- Waveform bundle downloads are cached.
- Snapshot downloads abort a read that stalls for 90 seconds and resume with
  `Range`; there is no limit on total transfer time.
- Client UI review fixes; editing context is preserved.

## 0.6.0 local iteration: mirror cache management

Based on main `e90596d`; local delivery does not publish a tag or deploy a server.

- Server Settings → Cache and export adds **Open mirror folder** for the selected
  server's existing, identity-matched snapshot. It remains available during sync.
  Saved LAN/external profiles in the same group can share a known matching library.
- Startup housekeeping reclaims known stale top-level temporary files across
  cached libraries. Generated old mirrors are pruned after a successful swap.
- Cleanup uses nonblocking OS leases and preserves the opened/current mirror,
  resumable downloads, protected recovery jobs, manual backups and unknown files.
  Replaced-library cleanup no longer recursively removes arbitrary contents.

Exact local validation, package and installation identities are recorded in
[Rust validation](Rust_Validation.md).

## Unreleased: resumable remote database sync

This iteration is based on `origin/main` at `d64c0a7` (workspace version 0.6.0),
including its Waveform management, Metadata publication ordering and keyboard
audition fixes. It is a local development build; it does not replace a published
tag or deploy the production server. Older candidate sections below are history.

- Snapshot downloads retain compressed checkpoints and resume after interrupted
  connections or client restarts while continuing to decompress during transfer.
- Both sync pages expose Pause, Continue Sync and Restart Download. A deliberate
  pause persists through startup, polling and write reconciliation.
- A 30-second network stall triggers bounded retries. Exhaustion retains the
  checkpoint; authentication, corrupt data and disk errors stop automatic retries.
- Matching client/server builds use fixed snapshot IDs, strong ETags and byte
  ranges. Expired objects or a server restart safely require a fresh snapshot.
- CLI `remote sync` reuses checkpoints; `--restart` discards the current endpoint's
  transfer. Snapshot formats remain subject to the shared compatibility catalog and protected migration; schema 6 support is retained.

See [remote mode](Rust_Remote_Mode.md) for protocol, disk-space and recovery details.

## 0.6.0: Plan B storage and shared migration

Release identity: program `0.6.0` | API `/api/v1/` | schema 6 |
snapshot schema boundary 3..=6, subject to the complete format/capability catalog.
This is the main baseline for the resumable-sync iteration above. Measurements
and candidate status below describe the earlier Plan B validation; current local
delivery evidence is recorded in [Rust validation](Rust_Validation.md).

- Plan B removes six persisted derived-token columns while retaining tokenization
  revision 3, all twelve logical FTS5 columns, full positions and document lengths.
  Schema 6 uses contentless-delete storage and the pinned SQLite statistics fixes.
- Client, CLI and server share compatibility diagnosis and the owned
  inspect/plan/apply/resume/verify/rollback executor. Version differences alone do
  not authorize rebuilding a library. Unknown formats block unsafe use.
- Migration preserves stable IDs, Metadata, annotations, references, scan history
  and unknown supported extensions. The explicitly authorized retirement removes
  `play_history`, `audio_files.play_count` and `audio_files.last_played_at`.
  Original playback data remains recoverable in the complete pre-upgrade backup.
  The p1 format declares play_history_retired=2; earlier unpublished p0 is read-only
  compatible and migratable.
- Playback events live only in client-local history.db, default 5000 per library.
  Local legacy history imports after backup and before retirement; server/snapshot
  history is not imported. Cache replacement leaves this independent store intact.
- AudioFile JSON/CSV no longer contain play_count; attributes no longer append
  last_played_at. New clients ignore these extra fields in old server responses.
  Existing server playback routes keep validation and no-write behavior. Deploy
  clients before servers: old binaries require the removed JSON counter field.
- Download/decompression counters and migration phases/counts reach the UI.
  Invalid checkpoints stop resumption; process-crash recovery is tested for copy,
  indexing, publication and rollback. Cache retries preserve the previous snapshot
  and leave abandoned jobs intact for owner review.
- The CR branch incorporates released 0.5.0, retains its Waveform cache management
  and same-revision replies, and explicitly tests both 0.4.11 and 0.5.0 legacy peers.
  It rejects stale/unbound snapshots, checks UPDATE OF retirement dependencies,
  binds recovery to actual tokenizer sources and admits settings mutations through
  the same peer-catalog check. See [the CR record](api/Search_Storage_B_CR.md).
- Full-copy validation reduced the 909,342-record library by 45.572%, with complete
  rollback and search-result checks. First-use sorting still has a reproducible
  latency regression despite passing measured P95 targets; resolve its release
  acceptance before deploying this draft candidate.

See the [migration contract](api/Library_Migration_Compatibility.md),
[v6 iteration and acceptance](api/Schema_V6_Iteration.md) and
[CR validation record](api/Search_Storage_B_CR.md) for exact evidence and
remaining limits. Database timings do not substitute for the separate GUI or Mac
acceptance work recorded below.

## Previous unreleased baseline: upgrade recovery and integrated client/server

This candidate includes `origin/main` at `71bb1fd`, client integration at `c50bd27`
and server integration at `3eef19c`, followed by the upgrade and shutdown repairs.
The previously rejected `47ddd03` through `c0e8af9` builds must not be published.
That earlier baseline reported 0.4.11 during preparation; publication requires a new
shared program version and matching tag. Do not replace the published v0.4.11 tag.

- Existing writable libraries upgrade search text and FTS5 in committed batches,
  resume after interruption and retain one validated rollback backup. Local startup
  displays bilingual progress; server health reports the current phase and committed
  row count. Business endpoints remain unavailable until the offline upgrade finishes.
- Upgrade batches and graceful server shutdown check WAL truncation. Shutdown stops
  list Waveform workers, drains scans and snapshot jobs, then releases owned cached snapshots.
  Active downloads retain their files until their last reference is released.
- The latest client and server list Waveform changes are included, along with the
  current toolbar/focus fixes. ARM64 BM25 comparisons retain exact result-order checks
  while accepting the documented two-ULP numerical tolerance.
- Libraries, Metadata, annotations, scan history and preferences are preserved.
  API `/api/v1/`, schema 5 and snapshot compatibility 3..=5 remain unchanged.

Validation and build identity are recorded in [Rust validation](Rust_Validation.md).
The original Mac's performance rerun is deferred at the user's request. Search latency
has not met the complete **P95 latency <500ms for 1M+ records** acceptance criteria;
the recorded slow samples and pending full GUI matrix remain open. See
[known issues](Rust_Known_Issues.md) and [upgrade recovery](Rust_Upgrade_Recovery.md).
This preparation does not publish a GitHub Release or deploy the service on port 8790.

## Published v0.4.11

Release 0.4.11; the client, CLI and server share the same program version. Patch release on top of the published v0.4.10.

Versions: program 0.4.11 | API /api/v1/ | schema 5 | snapshot compat 3..=5

## Changelog

### Click-free seek, pause and stop

- The fade at seek, pause, stop and source-load boundaries is now an 8 ms raised-cosine envelope whose length is computed from the output device's sample rate (at least 32 frames), so both ends of the fade have zero slope.
- The previous fixed 32-frame linear ramp lasted only 0.67 ms at 48 kHz and 0.33 ms at 96 kHz; its corners produced an audible click on every seek, especially at 96 kHz.
- The audio callback still only reads atomics and preallocated state. Volume, channel routing, source positions and playback sessions are unchanged.

### Column widths stay after dragging a divider

- Dragging a column divider shortly after clicking it no longer snaps the column back to its default width on release, and the default width is no longer saved in that case.
- Only a genuine double-click, where neither click drags the divider, restores the column's default width.

### Seek loopback diagnostic

- The `seek-loopback` diagnostic example gained `--signal sine`, `--tone-hz`, `--source-rate` and `--source-channels` options, so it can exercise the resampling and multichannel paths, and it reports envelope corners (second difference) next to the test tone's own slope and curvature.

## Known issues

- The first Play after the output device is opened still uses a 5 ms linear volume ramp.

## Compatibility

API paths, the database schema and the snapshot compatibility range are unchanged, and the settings file format is unchanged. Upgrading keeps libraries, Metadata, annotations, scan history and preferences; runtime state stays in `%LOCALAPPDATA%/AudioLibExplorer/v1` or the existing explicit data directory. The Windows x64 portable package includes WASAPI and ASIO.

The Release provides the Windows x64 client portable package and Windows x64 and Linux x64 standalone server packages, each with a SHA-256 file and an allowed-field validation report.
