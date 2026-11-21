# audiolib-server

[中文说明 / Chinese](README.zh-CN.md)

A small HTTP server that exposes one AudioLib library (a SQLite database plus the audio
files under configured root directories) to remote AudioLib clients under `/api/v1`. It
serves library snapshots and incremental changes, audio streaming (range requests),
low-bandwidth previews, waveform peaks, metadata writes, remote scans and a LAN-only admin
page. It never modifies audio files.

- **API contract:** [doc/api/README.md](../doc/api/README.md) (currently in Chinese).
- **Deployment, upgrade, backup and rollback:** [deploy/README.md](deploy/README.md)
  (systemd, launchd, Docker examples included). Verified on arm64 (OrbStack) from commit d379894 with the stock Dockerfile: image builds (~511 MB), container starts and serves on port 8790.
- **License:** See LICENSE at repo root.

## Features

- A background GitHub Release check on every start, with a confirmed download and
  deployment in the LAN admin page; see [automatic updates](deploy/README.md#automatic-release-updates).
- Library snapshot (`/library/snapshot`, optional zstd) and revision-based change polling.
- `/audio/{id}/stream` with single `Range`, `ETag`, `If-Range`; downloads with
  `?download=1`.
- `/audio/{id}/preview` for low-bandwidth playback: in-process PCM decimation for WAV
  (`codec=pcm`) and an optional ffmpeg FLAC transcode (`codec=flac`).
- `/audio/{id}/waveform` with an on-disk LRU cache.
- `/albums/{id}/artwork` returns a shared album's embedded JPEG or PNG (optional
  `?side=`), bound to library identity and album namespace with the
  `album_entity_artwork` capability. The compatible `/audio/{id}/artwork` retains
  its per-audio meaning. Image bytes stay outside SQLite; a cache lives in
  `<library directory>/artwork`. Upgrade schema-8 clients and servers together and
  retain the rollback database. See [shared albums and storage](https://github.com/Freedragonlancer/AudioLib-Explorer/blob/main/doc/Rust_Shared_Albums.md).
- Metadata writes, play counts, remote scans (start/pause/resume/cancel), record and folder
  removal (`manage_mode` can restrict them).
- Token authentication with three modes (`off`, `external`, `all`), tokens generated on the
  admin page (stored hashed), optional native TLS listener.
- Embedded admin page (`/admin`, LAN only): public URL, auth mode, tokens, library roots.

## Download a prebuilt server

[GitHub Releases](https://github.com/Freedragonlancer/AudioLib-Explorer/releases) provide macOS arm64 and, from 0.8.4, Linux x64 GNU server archives with SHA256 files. Version 0.8.4 also includes Docker amd64/arm64 offline images. Verify the checksum before extracting a native package or loading an image. Native archives keep the executable at their root and public configuration examples under `server/audiolib-server/`; Linux packages use Debian 12 as their GNU libc build baseline. Keep existing libraries, roots, configuration and credentials when upgrading. See [Linux/Docker delivery](https://github.com/Freedragonlancer/AudioLib-Explorer/blob/main/doc/Rust_Linux_Docker_Release.md) and the [deployment guide](deploy/README.md). From 0.8.5 (first included version) a prebuilt Windows x64 server ZIP is provided (`AudioLibServer-<tag>-win-x64.zip`); earlier versions and source builds remain available. Standalone Linux arm64 servers build from source.

## Quick start: build from source

The next large release uses native server package layout 2: `audiolib-server` and
`audiolib-migrate` are siblings (`.exe` on Windows); Docker images retain both under
`/opt/audiolibserver`. The migration tool has JSON `status`, `start` and `rollback`
commands with an explicit `--db` path, rather than a `--version` command. Production
packages exclude test hooks. Their inventory and acceptance reports bind both hashes.

This layout requires a **manual server bootstrap** from pre-0.9 installations. The
frozen old updater genuinely rejects the new migration binary; package metadata states
`automatic_upgrade_compatible=false`, `manual_bootstrap_required=true`, layout revision 2
and minimum published updater version `0.9.0`. The candidate program version is frozen
at 0.9.0 but remains unpublished. Preserve the library, configuration and rollback backups, install
the complete native payload while the old server is gracefully stopped, and follow the
client/server migration runbook before explicitly migrating the library. Offline recovery
uses the packaged tool, for example `audiolib-migrate status --db /path/to/library.db`;
`start` or `rollback` targets that same explicitly selected library. Docker upgrades replace
the image and retain its data mounts. This adds no archive or release asset.

The repository root `rust-toolchain.toml` and `.cargo/config.toml` default to a **Windows**
target, so always pass an explicit `--target` for your platform. The server builds
`audiolib-audio` without its `playback` feature, so no audio device library is needed.

**macOS (Apple silicon)** — `server/rust-toolchain.toml` pins 1.97.1 for builds inside `server/`:

```sh
cargo build -p audiolib-server -p audiolib-db --bin audiolib-server --bin audiolib-migrate --release --locked --no-default-features --target aarch64-apple-darwin
# binaries: target/aarch64-apple-darwin/release/{audiolib-server,audiolib-migrate}
```

**Linux (Debian/Ubuntu)**:

```sh
sudo apt-get install -y build-essential pkg-config clang libclang-dev
export RUSTUP_TOOLCHAIN=1.97.1          # override the repository's Windows toolchain file
rustup toolchain install 1.97.1
cargo build -p audiolib-server -p audiolib-db --bin audiolib-server --bin audiolib-migrate --release --locked --no-default-features --target x86_64-unknown-linux-gnu
#   or --target aarch64-unknown-linux-gnu on ARM64
cargo test  -p audiolib-server --locked --target x86_64-unknown-linux-gnu   # optional
# binaries: target/<target>/release/{audiolib-server,audiolib-migrate}
```

Verified: the Linux build and the server test suite pass in an aarch64 container
(`rust:1.97.1-bookworm`). Verified on arm64 (OrbStack) from commit d379894 with the stock Dockerfile: image builds (~511 MB), container starts and serves on port 8790.

`GET /api/v1/health` reports the short git commit embedded at build time (`build.rs`); a
build from a source archive without `.git` reports `"commit": "unknown"`.

## Configure

Copy [`audiolib-server/config.example.toml`](audiolib-server/config.example.toml) outside the
repository and edit it. Every option is documented in that file. The library database must
already exist; the server never creates one. Relative paths resolve against the directory
of the config file.

Authentication follows `auth_mode` (API doc §2.2):

- `external` (default): LAN-direct requests need no token (an `Authorization` header is
  ignored). Requests that came through a reverse proxy (`X-Forwarded-For`, `Forwarded`,
  `X-Real-IP`, `X-Forwarded-Proto`/`-Host`) or from a non-private peer need
  `Authorization: Bearer <token>`.
- `off`: ordinary endpoints need no token. AI relay still requires a bearer token; see [Server settings](#server-settings).
- `all` (or the legacy `auth = true`): every endpoint except `/health` needs a token.

Accepted tokens are the environment token (the variable named by `token_env`, default
`AUDIOLIB_SERVER_TOKEN`, at least 32 random visible ASCII characters, never logged) and tokens
generated on the admin page. **Security:** for any deployment reachable from the internet use
a reverse proxy with TLS and tokens; do not expose the plain HTTP port directly, and do not use
`auth_mode = "off"`. In `external` mode LAN clients are trusted without a token.
Set `trust_lan_peers = false` and `auth_mode = "all"` behind NAT, container port
forwarding or a proxy that hides client addresses. This also disables the passwordless
admin page; configure that deployment through its TOML file. Container defaults use
this policy and publish the HTTP port on host loopback only. Library management defaults
to `manage_mode = "lan"`; enable `all` only for clients trusted to browse server disks,
add roots and scan them.
Before upgrading, replace environment tokens shorter than 32 characters with a random
secret of at least 32 visible ASCII characters. Generated `al_` tokens remain valid.

## Server settings

`config.toml` supplies the startup values. `admin-settings.json`, beside `library_db`, stores overrides from the LAN admin page and from `PATCH /api/v1/settings`. A missing field there means “use the TOML value”. Clearing a saved override returns to the TOML value. The server does not rewrite the TOML file. `trust_lan_peers` is read only at startup; the admin page cannot turn it back on. The same facts are in [Server deployment](../doc/Server_Deployment.md#server-settings). The Chinese page is [README.zh-CN.md](README.zh-CN.md#服务端设置).

### Settings that apply without a restart

| Key | Default | Required | Where it is set | Effect |
| --- | --- | --- | --- | --- |
| `legacy_snapshot_enabled` | `true` | No | TOML. The admin **数据库** tab and `PATCH /api/v1/settings` can override it | While enabled, a schema 9 library can also serve a derived read-only schema 8 snapshot containing WAV rows. Those older clients cannot write the schema 9 source. Turning it off leaves them on their last valid cache. Clients that accept schema 9 still receive the native snapshot. |
| `list_waveform_pause_stats_on_playback` | `false` | No | TOML. The admin **波形与资源** tab and `PATCH /api/v1/settings` can override it | While enabled, for 3 seconds after `GET /audio/{id}/stream` or `GET /audio/{id}/preview` starts, `POST /waveforms` does not read file attributes. `HEAD` does not open that window. |
| `pregenerate_list_waveforms` | off when omitted | No | TOML. The admin page and `PATCH /api/v1/settings` can override it | Background generation of list thumbnails. A saved `false` is an override; clearing the override returns to the TOML value. |
| `list_waveform_rate` | `10` | No | TOML. The admin **波形与资源** tab can override it | Maximum files per second that pre-generation may start, from 1 to 1000. On-demand thumbnail requests are not limited by this rate. |
| `lan_url` | detected from the host interfaces | No | `admin-settings.json` only. It is not a `config.toml` key | LAN base URL shown to clients. Clear it to detect the address again. |

Provider accounts, model ids and provider API keys are not server settings. The desktop client keeps those in its own data directory.

### AI request relay

Relay is off until `ai_upstreams` lists at least one endpoint. With an empty list, `POST /api/v1/ai/models` and `POST /api/v1/ai/generate` return 403 `AI_DISABLED`. The allowlist is TOML only. The admin page cannot edit it, and a change applies on the next process start. The server configuration has no model-id field; the desktop client sends the model with each request.

```toml
max_ai_requests = 4

[[ai_upstreams]]
protocol = "magpie"
base_url = "http://127.0.0.1:3425/v1"

[[ai_upstreams]]
protocol = "chat_completions"
base_url = "https://api.example.com/v1"
```

`protocol` is one of `chat_completions`, `responses`, `anthropic`, `gemini`, `jev`, `open_code_go`, `deepseek`, `magpie` or `open_ai_official`.

- At most 64 entries. Startup rejects a duplicate protocol and base URL after normalization, and rejects an unknown protocol or base URL.
- `max_ai_requests` is 1 through 64 and defaults to 4. Further in-flight calls receive 503 `BUSY`. Disconnecting the client does not free the slot until the blocking upstream call finishes.
- The protocol, host, port and base path must match one entry after normalization. Anything else returns 403 `AI_UPSTREAM_FORBIDDEN`. Redirects are not followed. The client cannot substitute another internal address.
- A valid AudioLib bearer token is required even when `auth_mode` is `off` and even for a trusted LAN peer. A missing token is 401. The provider API key travels only in that request body. It is not written to the TOML file, `admin-settings.json` or the log.
- The request body is limited to 64 KiB. This route allows 620 seconds. The provider deadline inside the request is 2 through 600 seconds. Relay does not wait for the library to open and does not read migration state.
- The published Docker config does not set `ai_upstreams`, so relay stays off until an administrator mounts a TOML file that lists endpoints.
- Acceptance: an authenticated `POST /api/v1/ai/models` for a listed protocol and base URL is admitted; a different base URL returns 403 `AI_UPSTREAM_FORBIDDEN`; a call without a token returns 401; an empty allowlist returns 403 `AI_DISABLED`. Calling a paid provider is not part of this check.

The request and error contract is [API §12](../doc/api/README.md#12-ai-请求转发).

### Limits that need a restart

The admin **波形与资源** tab shows `max_streams`, `max_previews`, `max_transcodes`, `max_waveforms`, `waveform_cache_mb` and `list_waveform_cache_mb` as read-only and marks them as requiring a restart. `max_ai_requests`, `list_waveform_workers` (default 3, never more than `max(1, max_waveforms - 1)`), `ffmpeg_path`, `bind`, `revision_poll_secs`, `snapshot_wait_secs`, `root_check_secs` and `root_offline_secs` are also changed in TOML and then by restarting the process. The admin page does not list every one of those keys.

## Run

```sh
audiolib-server --config /etc/audiolib/config.toml
# with an environment token:
AUDIOLIB_SERVER_TOKEN='<at least 32 random visible ASCII chars>' audiolib-server --config /etc/audiolib/config.toml
# or: AUDIOLIB_SERVER_CONFIG=/etc/audiolib/config.toml audiolib-server
```

Example locations: `/srv/audiolib/music` (audio), `/var/lib/audiolib` (library and data),
`/opt/audiolib` (binary), `/usr/local/var/audiolib` (macOS).

Operational behavior:

- Each listener permits at most 1024 connections, including TLS handshakes; excess
  connections are closed. Connections must send request headers within 10 s; non-streaming endpoints time out after
  30 s (408 `TIMEOUT`); on SIGTERM/Ctrl+C in-flight requests get at most 10 s.
- Startup is listen-first: the port is served immediately while the library opens on a
  background thread. Until then `GET /api/v1/health` reports `"status":"starting"` and
  authenticated endpoints answer 503 `UNAVAILABLE` (`Retry-After: 5`). If opening fails (for
  example a macOS removable-volume permission prompt was declined) `/health` reports
  `"status":"error"` and the open is retried with backoff (5 s doubling to 300 s) instead of
  exiting. Bind, configuration and token errors still exit immediately.
- Writable search upgrades now resume committed text and FTS5 batches after interruption.
  `/health` adds an optional `upgrade` phase with committed row counts. Normal shutdown
  drains snapshot work, releases owned caches and checks WAL truncation before persisting
  its revision baseline. See [upgrade recovery](../doc/Rust_Upgrade_Recovery.md) for disk
  accounting and the pending Mac release acceptance.
- ffmpeg is optional (only `preview?codec=flac`); without it that codec answers 503
  `TRANSCODER_UNAVAILABLE` and the server keeps running. The server does not bundle ffmpeg.
- Native TLS checks certificate and key contents every minute. Valid replacements apply
  to new handshakes without interrupting existing connections; invalid replacements keep
  the previous certificate and report a warning in TLS status and server logs.
- Failed bearer authentication is limited per socket-peer IP (60 failures per 60 s;
  subsequent attempts return 429 with `Retry-After`). Logs include the peer address,
  never the supplied token. A proxy shares one limit across its clients.
- Requests using token-free access must use an IP/LAN `Host` or the configured public/LAN
  URL hostname. A valid bearer token also permits other hostnames.

## First use (admin page)

The admin page has no password; it only answers **LAN-direct** requests (private peer
address, no forwarding headers, LAN `Host`) — see the guard in `src/admin.rs`.
`manage_mode` does not control the admin page; it only limits scans and removals through
the client API.

1. Start the server and open `http://192.0.2.10:8790/admin` from a computer on the same LAN.
2. **Tokens:** enter a name and click 生成令牌 (generate token). The token (`al_` + 64 hex
   characters) is shown **once**; only its SHA-256 hash is stored in `tokens.json` next to
   the library. Revoke it on the same page. Alternatively set the environment token
   (`AUDIOLIB_SERVER_TOKEN`, see Configure). Whether a token is required is decided by
   `auth_mode` (can also be changed on the page).
3. **Library roots:** browse the server's disks (read-only) and add a directory — a local
   disk or an already mounted network share (SMB, for example under `/Volumes/...` on macOS
   or `/mnt/...` on Linux; mount it with the OS first, the server does not mount shares).
   Changes apply without a restart and are stored in `admin-settings.json`.
4. **Scan:** start a scan of a root on the same page and watch its progress there
   (`GET /api/v1/scans/current` returns the same state).
5. Optional: set the public URL (reported as `public_url` by `/health`) and native TLS.

## Connecting the Windows client

In the client open 音效库管理 / Sound library management (Ctrl+D) → ＋ 添加服务器 / Add server:

- **Address:** enter the server's base URL, e.g. `http://192.0.2.10:8790` on the LAN or
  `https://audiolib.example.com` (a path prefix such as `https://audiolib.example.com/audiolib`
  is allowed). The client appends `/api/v1` itself; a pasted `/api/v1` is accepted and not
  doubled (`crates/audiolib-core/src/server_url.rs`).
- **Access token (访问令牌):** optional on the LAN with `auth_mode = "external"`; required
  over the internet or with `auth_mode = "all"`. Stored per profile with Windows DPAPI.
- Test the connection before saving (see the client UI for the exact button).
- **试听时转码 / Transcode auditions:** leave it off on the LAN (originals stream fine);
  turn it on for internet or slow links (uses `/audio/{id}/preview`; FLAC needs ffmpeg on
  the server, otherwise WAV PCM decimation only).

## Internet access

Do not expose port 8790 directly. Put a reverse proxy with HTTPS in front
(e.g. `https://audiolib.example.com` → `http://127.0.0.1:8790`), keep tokens required,
and **never publish `/admin`**: forward only `/api/v1/` or block `/admin` at the proxy
(`location /admin { return 403; }`). The proxy must send forwarding headers so requests are
treated as external. Full checklist and nginx/Caddy examples: API document §2.4 and
[deploy/README.md](deploy/README.md#reverse-proxy-checklist).

## Troubleshooting

- **First snapshot of a large library is slow:** preparing it can take 30–60 s for a
  million files; after `snapshot_wait_secs` (default 90) the server answers 503 `BUSY` with
  `Retry-After` and keeps preparing; clients retry.
- **409 `BUSY`** (e.g. removing a root during a scan) or **503 `BUSY`** (stream/preview slots
  full): wait and retry after `Retry-After`.
- **503 `UNAVAILABLE` right after start:** the library is still opening (`/health` says
  `starting`). A v3/v4 → v5 upgrade runs schema migration plus extracted-tag backfill on
  open (anonymized measurements from 2026-10-11: about 198 s for a large library copy
  and about 7 s for a smaller copy; record counts and database capacity are omitted).
  The full-text index is **not** rebuilt
  automatically — afterward run the CLI `AudioLibCli db rebuild-search` by hand (about
  513 s more on that large copy); reserve free disk about the size of the library for the
  rollback copy left during that step — see
  [deploy/README.md](deploy/README.md#search-index-backfill-and-fts-rebuild-db-rebuild-search).
- **Disk space:** snapshots go to `snapshot_dir` (default `<library_db directory>/snapshots`), waveforms to
  `waveform_cache_dir` (limited by `waveform_cache_mb`), backups and rollback copies next to
  the library.

## Upgrade, rollback and backup

See [deploy/README.md](deploy/README.md) (upgrade runbook, `redeploy-server.sh` with automatic
rollback, backup and health check).

## API overview

`/health`, `/library`, `/library/snapshot`, `/roots`, `POST /roots/{name}/check`,
`/audio/{id}`, `/audio/{id}/waveform`, `/audio/{id}/preview`, `/audio/{id}/stream`,
`/audio/{id}/artwork`,
metadata writes (`PATCH /audio/{id}/metadata`, `POST /audio/metadata/batch`,
deprecated no-op `POST /audio/{id}/plays`), remote scans (`POST /scans`, `GET /scans/current`,
`GET /scans/{id}`, `POST /scans/{id}/pause|resume|cancel`), history
(`GET /history/scans`; deprecated `GET /history/plays` is always empty, play history is client-local), removal (`POST /audio/remove`, `DELETE /folders/{id}`) and
`GET /capabilities`. Details, error codes and examples: [doc/api/README.md](../doc/api/README.md).

Library roots can be managed on the LAN-only admin page (`http://<LAN address>:8790/admin`):
a read-only server disk browser, add/remove roots (removal only makes the library forget
records; audio files are never touched; 409 `BUSY` during scans) and scan control. Changes
apply without a restart and are stored in `admin-settings.json` next to the library (its
`roots` list then replaces `[[roots]]` of `config.toml`). The `/admin/api/*` endpoints are
not part of the client API.

## Deployment

See [deploy/README.md](deploy/README.md). Examples: `deploy/systemd/`, `deploy/launchd/`,
`deploy/docker/`; the optional macOS-only launcher lives in [launcher/](launcher/README.md).

Docker notes:

- You must mount a volume that contains `/data/library.db`, or create the library with the client first; otherwise `/api/v1/health` keeps returning `status:"error"` (The library could not be opened; the server keeps retrying).
- The image's HEALTHCHECK requires HTTP success and `status:"ok"`; startup and failed library opens remain unhealthy.
- Only `/data` is a volume (library.db, snapshots, caches, tokens.json). `/config` is not a volume: the image ships `/config/config.toml`, and you may optionally bind-mount your own file read-only over it (`-v ./my-config.toml:/config/config.toml:ro`).
- The bundled Docker config uses `auth_mode = "all"`. With a plain `docker run` you must pass `-e AUDIOLIB_SERVER_TOKEN=...`; otherwise the server starts but every request answers 401. The compose file enforces it with `${AUDIOLIB_SERVER_TOKEN:?}`.

## License

See LICENSE at repo root.
