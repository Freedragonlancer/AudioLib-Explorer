# AudioLib Explorer v0.4.10

Release 0.4.10; the client, CLI and server share the same program version. Patch release on top of the published v0.4.9.

Versions: program 0.4.10 | API /api/v1/ | schema 5 | snapshot compat 3..=5

## Changelog

### Customizable audio table columns and sorting

- Drag a header divider in the sound list to resize a column; double-click a divider to restore that column, or use the column menu to restore the default layout. The table scrolls horizontally when the columns are wider than the window.
- Click the file name, duration, format (sample rate), channels or tags header to sort ascending or descending; an arrow shows the active order. Sorting applies to all results and returns to the first page.
- Tags sort by user tags first and fall back to source tags when the user tags are blank. User and source tags share the same Unicode case-insensitive sort key, and annotations containing only Unicode whitespace (such as full-width spaces) sort by their source tags.
- Column widths and sort order are saved in the local preferences and kept across restarts and library switches. Existing library data is not modified.
- An empty search result keeps the full-width horizontal scroll, so the right-hand headers and dividers stay reachable, and headers and cells stay aligned when results return.

### Smoother preview audio across seek

- Seeking during preview cross-fades the old and new audio over 32 output frames per channel instead of cutting abruptly. Volume, channel routing, source positions and playback sessions stay independent.
- When the old audio runs out during a fade, a short cached PCM tail finishes it. The tail is cleared when the source is replaced or playback goes silent, and only real source frames count toward the playback position.
- Stopping drains queued fades before pausing the output; an active seek keeps the device stream running.

### Quiet seek on WASAPI

- Preview audio is quiesced before the seek reads the file, and PCM still retained by the output device is drained, so a WASAPI seek no longer replays a short burst of stale audio from the previous position.

## Compatibility

API paths, the database schema and the snapshot compatibility range are unchanged. The settings file gains an optional `audio_table` section for column widths and sort order; existing settings files load with the default layout. Upgrading keeps libraries, Metadata, annotations, scan history and preferences; runtime state stays in `%LOCALAPPDATA%/AudioLibExplorer/v1` or the existing explicit data directory. The Windows x64 portable package includes WASAPI and ASIO.

The Release provides the Windows x64 client portable package and Windows x64 and Linux x64 standalone server packages, each with a SHA-256 file and an allowed-field validation report.
