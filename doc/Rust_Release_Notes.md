# AudioLib Explorer v0.4.8

Release 0.4.8; the client, CLI and server share the same program version. Patch release on top of the published v0.4.7.

Versions: program 0.4.8 | API /api/v1/ | schema 5 | snapshot compat 3..=5

## Changelog

### Server-mode startup and close

- Preparing an offline cache copy now bounds Busy/Locked SQLite backup steps to 5 seconds and reports an unknown backup step result as an error instead of retrying forever. The owned staging connection no longer waits inside SQLite for its own lock.
- Closing the window waits at most 15 seconds for cancelled remote cache work, then closes anyway. The active cache is never modified by the abandoned work; its staging copy is removed on a later start.
- Server-mode startup removes this library's cache staging files that are older than one hour (left behind by an interrupted preparation or download). Snapshots and fresh staging files are kept.
- An unusable server selection (no active profile, a missing profile, or an empty or invalid server URL) opens the local library for the session. Saving preferences keeps the original server selection on disk unless you explicitly choose a data source. Without an openable local library, server mode is kept so the profile can be fixed.
- Library management checks that the selected local library exists before switching to local mode and explains the problem instead of saving a switch that cannot open.

## Compatibility

API paths, the database schema and the snapshot compatibility range are unchanged, and the settings file format is unchanged. Upgrading keeps libraries, Metadata, annotations, scan history and preferences; runtime state stays in `%LOCALAPPDATA%/AudioLibExplorer/v1` or the existing explicit data directory. The Windows x64 portable package includes WASAPI and ASIO.

The Release provides the Windows x64 client portable package and Windows x64 and Linux x64 standalone server packages, each with a SHA-256 file and an allowed-field validation report.
