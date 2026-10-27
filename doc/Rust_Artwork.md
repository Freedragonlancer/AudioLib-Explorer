# Album artwork

Artwork is optional presentation data. Encoded images and decoded pixels never
enter the library database, FTS5, JSONL/CSV exports or server snapshots. Reading
or managing a cover does not change source WAV files, Metadata or annotations.

## Display and management

Select a sound, then open **Sound library management → Album artwork ▸**. Expand
the artwork section to show its controls. In server
mode the controls are on **Cache and export** for the active connection.

- **Show covers** applies to both modes and persists in `settings.json` as
  `show_album_artwork`. Old settings default to enabled.
- **Choose / replace image** accepts PNG/JPEG up to 8 MiB, 4096 pixels per
  dimension and a 64 MiB decode budget. The client copies a thumbnail up to
  512 pixels; moving the original picture afterwards does not break the cover.
- **Remove cover** hides the current album's cover, including its embedded cover.
- **Restore embedded** removes the custom image and hidden marker, returning to
  extraction from the WAV or the server artwork endpoint.
- **Reload cover** revalidates the current cover. **Clear cover thumbnails**
  removes only generated client thumbnails; custom images and hidden markers stay.

Custom images and hidden markers live under `<user data>/artwork-overrides/`,
outside the disposable cache. They are scoped to the local database or remote
library identity and the declared album name. For a sound with no album they
apply only to that audio ID. LAN/external profiles of the same server library
share the custom cover. These choices belong to this PC and do not propagate to
other clients or form part of a `.db` backup/restore.

Generated thumbnails live under `<cache>/artwork/` with a 256 MiB pruning budget.
Remote keys include the library identity, revision and audio ID; ETags revalidate
the response. Selection, source changes, disabling covers, cache actions and
shutdown invalidate old work. A dedicated coalescing worker performs extraction
and HTTP requests; cover requests do not run on the Slint event loop or audio
callback. A search worker skips a thumbnail when the cover cache is busy.

Source validation, extraction and HTTP run outside the cache mutex. An admitted
manual edit retains its original library/album destination across source changes,
and closing waits for those edits off the event loop. Cancelling the image picker
or rejecting an invalid image resumes automatic loading. Input and disposable
cache errors are shown without becoming unsaved custom-cover edits.

The sidebar remembers at most 128 successfully displayed album representatives
in memory. Search refreshes validate those representatives against owned database
records and read generated thumbnails only; they never inspect source WAVs.
Custom images and hidden markers take precedence. Source or snapshot replacement
invalidates the representatives.

## Server compatibility

The client requests `GET /api/v1/audio/{id}/artwork?side=512` only when
`compatibility.features.album_artwork >= 1`. Missing capability, 404 or 405 gives
an empty embedded-cover display; custom client images still work. Artwork is
never extracted by downloading or seeking to the end of a remote audio stream.

Bound requests include `X-AudioLib-Library-Id` and validate the response's
`X-Library-Id`. A server-library replacement must not display the same numeric
audio ID from the new library against an old snapshot.

The existing schema 6 snapshot remains read-only at its original path. A selected
record derives its album from existing Metadata without upgrading the snapshot.
Remote album sidebar aggregation is enabled only for schema 7, whose indexed
album column avoids scanning every schema 6 row's JSON on each search. Embedded
remote covers and custom images do not require that sidebar.

Server extraction uses the same already-open file handle that passed root/path
validation. Two independent artwork slots bound decoding without occupying
playback slots. A full artwork queue returns `503 BUSY` with `Retry-After: 1`.
See [API §8.2](api/README.md).

## Acceptance boundaries

The authorized production deployment on 2026-10-27 updated the Mac server to the
then-current 0.8.0 development build. The existing launcher, its signature, launchd
configuration and file-access chain were retained. Schema 6 was upgraded in place
to schema 7 after validating an independent rollback backup and an upgrade copy.
The library identity and complete business-data digest remained unchanged.

Real Windows acceptance displayed an embedded cover from that server and played
its PCM preview through WASAPI, including pause, seek and Stop/Play. Older servers
without `album_artwork` still use the fallback described above. No GitHub release
was published by this deployment.

See [Validation](Rust_Validation.md) for verified coverage and remaining limits.
