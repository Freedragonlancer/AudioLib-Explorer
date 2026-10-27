# Album artwork

Artwork belongs to a shared Metadata library or fallback folder entity. Selecting
another WAV in that album does not change its cover or custom choice. See
[shared albums and Metadata storage](Rust_Shared_Albums.md) for the schema-8
ownership, namespace and migration contracts.

Encoded images and decoded pixels never enter SQLite, FTS5, JSONL/CSV exports or
server snapshots. Reading or managing a cover does not change source WAV files,
Metadata or annotations.

## Display and management

Select a sound, then open **Sound library management → Album artwork ▸**. In
server mode these controls appear under **Cache and export** for the connection.
Both modes offer:

- **Show covers**, persisted as `show_album_artwork` (old settings default on).
- **Choose / replace image**: PNG/JPEG up to 8 MiB, 4096 pixels per dimension and
  64 MiB decode budget. The client stores a thumbnail up to 512 pixels; moving the
  original image does not break the choice.
- **Remove cover**, hiding the album's custom and embedded cover.
- **Restore embedded**, returning the album to automatic shared extraction.
- **Reload cover**, revalidating the current shared cover.
- **Clear cover thumbnails**, clearing generated client images while retaining
  custom images, hidden markers and migration receipts.

Custom choices live under `<user data>/artwork-overrides/`. Native schema-8 keys
include the library namespace and album ID. LAN/external profiles of the same
server library share choices. For source schemas 3–7, locally projected album IDs
are never sent as native server IDs; stable Metadata/folder aliases retain choices
across revisions and the later native schema-8 upgrade. Choices belong to this
PC and are outside `.db` backup/restore and inter-client synchronization.

Old album-name and per-WAV choices migrate by copying into the shared identity.
Identical choices merge; conflicting images or image/hidden choices retain every
original and display a conflict notice so the user can choose. No arbitrary WAV
wins a conflict. Explicit restore-to-embedded stores a migration receipt, so
retained old files cannot resurrect a removed choice after restart.

## Shared extraction and caching

Default extraction checks up to 64 present members in ascending audio ID order,
using the first valid embedded picture. The candidates come from the whole album,
independent of the selected member, search query and sidebar page. A WAV supplies
pixels but never owns the cover. A new scan or explicit reload revalidates changed
source pictures. Manual images and hidden markers take precedence.

Generated thumbnails live under `<cache>/artwork/` with a 256 MiB budget. Keys
identify the shared album. Local validation includes member IDs, source paths,
sizes and recorded modification times; HTTP uses ETags. The sidebar remembers at
most 128 generated album paths and reads only cached pixels, without opening WAVs
or issuing per-album search/count queries.

A dedicated coalescing worker performs extraction and HTTP outside the Slint event
loop and audio callback. Same-album selections reuse in-flight work. Source,
epoch, cancellation and shutdown checks reject stale publication. Extraction and
HTTP run outside the cache mutex. Admitted manual edits retain their original
library/album destination across source changes; close drains those edits off the
event loop. Picker cancellation and rejected images resume automatic loading.

## Server compatibility

With `compatibility.features.album_entity_artwork >= 1`, a native schema-8
snapshot uses `GET /api/v1/albums/{id}/artwork?side=512`. Requests bind both
`X-AudioLib-Library-Id` and `X-AudioLib-Album-Namespace`; the client validates
`X-Library-Id`, `X-Album-Id` and `X-Album-Namespace` on 200 and 304 replies.
A mismatched library or namespace must be synchronized before showing the image.

For source schemas 3–7, the client validates and upgrades only an owned copy. It
uses the same sorted member set through the older `/audio/{id}/artwork` capability
instead of sending local projected album IDs. Missing capability, 404 or 405
produces an empty embedded display while custom images remain usable. Artwork
never downloads or seeks through a remote audio stream to find a picture.

Server extraction uses the already-open file handle that passed root/path checks.
Two separate artwork slots bound decoding without occupying playback slots.
A full queue returns `503 BUSY` with `Retry-After: 1`. See
[API §8.2](api/README.md).

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
