# AudioLib Explorer v0.4.11

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
