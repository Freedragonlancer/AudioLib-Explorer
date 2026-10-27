# Handoff: desktop server-mode album artwork

Date: 2026-10-27. Branch: `codex/handoff-remote-album-artwork`. Base: `beta` (`1b41121`).
Status: **WIP, does not compile.** Stopped for a Codex handoff before the desktop
remote helpers and validation.

## Goal

Windows/desktop client phase 1: when server mode is active and `/capabilities`
offers `album_artwork` at revision ≥ 1, the **selection** cover comes from
`GET /api/v1/audio/{id}/artwork`. Missing capability or HTTP 404 leaves a blank
cover and does not spam errors. Local artwork stays on the ID3 path.

## Locked API

- Capability name: `album_artwork` (revision ≥ 1 on `GET /api/v1/capabilities`).
- Phase 1 route only: `GET /api/v1/audio/{id}/artwork` with optional `?side=`.
- Client requests `side=512` for the selection cover.
- `404` and `405` are “no cover”, not failures. Do not open `/audio/{id}/stream`
  or `/preview`, and do not Range-download a file to read ID3.
- Cache key is remote: `library_id` + audio id + snapshot revision + response
  ETag. Not local mtime or WAV path.
- A marker catalog that names `album_artwork` must match this build’s feature
  revision (`1`). A marker catalog that omits the name still honors a top-level
  `album_artwork` object. No object (production servers still on 0.7.8) means
  blank, and no request.

Accepted capability shapes (so one bad field cannot reject all of `/capabilities`):

- `{"album_artwork":{"revision":1}}`
- `{"album_artwork":1}`
- missing, null, or a non-object/non-number → not offered

## Done

- `FEATURES` includes `("album_artwork", 1)` in `audiolib-core` so marker
  negotiation can enable it. The feature entry alone does not fetch covers.
- `Capabilities::album_artwork` + `album_artwork_usable()`.
- `RemoteClient::album_artwork` → `ArtworkReply` (`Data`, `NotModified`, `Missing`).
  Bearer auth, optional `If-None-Match`, 8 MiB body cap, `image/*` or
  `application/octet-stream` only. Unit tests are written in `client.rs` and
  `manage.rs` and have **not** been run.
- Desktop cache + `remote_selection_cover` in `artwork.rs`: decode with existing
  `decode_thumbnail`, store a PNG, remember 404 until the revision changes.
  Tests are written and have **not** been run.
- `Controller::queue_artwork` keeps the local branch (`probe_with_artwork`,
  empty album skip, path validation). Remote selections enqueue
  `ArtworkSource::Remote` instead of returning immediately. The artwork worker
  publishes `album-cover-ready` the same way as local. `publish_albums` is
  still remote-gated (sidebar covers untouched).

## TODO (compile break)

`controller.rs` calls two methods that were **not** added:

- `Controller::remote_artwork_library()` — used by `queue_artwork` on the UI
  thread. Must return `Some(library_id)` only when `remote_read_only`, cached
  `/capabilities` for the active server URL is `album_artwork_usable()`, and a
  snapshot `CacheState` exists. No network on this path.
- `Controller::remote_artwork_client()` — used by the artwork worker. Returns
  `(RemoteClient, cache_dir, library_id, revision)` from the same capability
  gate, `config.paths.cache`, and `secrets()`. Drop locks before HTTP.

Put both next to `remote_list_waveform_provider` in
`app/audiolib-desktop/src/remote_mode.rs`. The worker already ignores a reply
whose `library_id` does not match the queued job, and `enqueue_artwork`
cancels the previous in-flight GET.

Then:

```text
cargo fmt --all -- --check
cargo clippy -p audiolib-core -p audiolib-remote -p audiolib-desktop --all-targets --locked -- -D warnings
cargo test -p audiolib-remote --lib album_artwork --locked
cargo test -p audiolib-desktop --bin AudioLibExplorer artwork --locked
```

`audiolib-desktop` tests live in the `AudioLibExplorer` binary. Narrow the
filter if those names do not match. Do not treat a green build as local
installation; this change is client behavior, and installation was not started.

## Files touched

- `crates/audiolib-core/src/compatibility.rs`
- `crates/audiolib-remote/src/manage.rs`
- `crates/audiolib-remote/src/client.rs`
- `crates/audiolib-remote/src/lib.rs`
- `app/audiolib-desktop/src/artwork.rs`
- `app/audiolib-desktop/src/controller.rs`
- `doc/handoff/2026-10-27-remote-album-artwork-client.md` (this file)

Not modified: `publish_albums`, `probe_with_artwork`, `artwork::thumbnail_path`,
server routes, playback/preview.

## Out of scope

- Remote play/preview regression (Debug owns that). Do not mix it in.
- Sidebar album covers (phase 2). Leave `publish_albums` remote-gated.
- Issue/PR #97 and production server 8790. Do not touch them.
- No private IPs or domains in code or docs.
