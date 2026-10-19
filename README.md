# AudioLib Explorer

<img src="resources/icons/AudioLibExplorer.png" width="128" height="128" alt="AudioLib Explorer: audio library and Waveform icon">

[中文说明 / Chinese](README.zh-CN.md)

AudioLib Explorer is a fast, local-first sound-effects library manager for Windows, written in Rust with a [Slint](https://slint.dev) desktop UI. It indexes large WAV libraries into SQLite (FTS5), searches them instantly, previews sounds through WASAPI (optionally ASIO), and can also browse a remote library served by the bundled `audiolib-server`.

- **Program version:** 0.4.11 (workspace `Cargo.toml`)
- **HTTP API:** `/api/v1/` (client and server)
- **Database schema:** 5 (snapshot compatibility range 3..=5)
- **License:** [GPL-3.0-only](LICENSE)

## Features

- Incremental and full folder scans with professional WAV metadata extraction (BWF `bext`, iXML, INFO, ID3, XMP, Soundminer SMED read-only compatibility). Source WAV files are never modified.
- SQLite FTS5 search with Chinese segmentation (jieba-rs), tag/category filters and a documented target of P95 < 500 ms for 1M+ records.
- Playback with streaming decode, device-rate resampling, waveform and spectral views, independent pitch/rate audition (SoundTouch), WASAPI and optional ASIO output.
- Library management: add folders, repair moved or renamed files, batch annotations, backup/restore, integrity checks, JSONL/CSV exchange.
- Command-line tool `AudioLibCli` (JSON/table/CSV output, library management, remote playback control of a running desktop instance).
- Server mode: `server/audiolib-server` exposes one library over `/api/v1/` (snapshots, range streaming, previews, waveforms, token authentication, LAN-only admin page). The client opens a validated read-only snapshot; annotations and management writes use identity-bound HTTP requests. Upgrade client and server together for writes; older servers remain readable.
- Bilingual (Chinese/English) user interface.

## Build

Requirements: Windows with MSVC build tools and the pinned Rust toolchain (see `rust-toolchain.toml`). ASIO builds additionally need LLVM (libclang) and use the pinned, hash-verified Steinberg ASIO SDK (downloaded by `tools/prepare-asio.ps1`).

```powershell
cargo build --release -p audiolib-desktop -p audiolib-cli

# optional ASIO support
. ./tools/prepare-asio.ps1
cargo build --release -p audiolib-desktop -p audiolib-cli --features audiolib-desktop/asio,audiolib-cli/asio

# portable package (dist/latest/portable)
./tools/build-release.ps1
./tools/package-portable.ps1 -SkipBuild
```

The server builds on Windows, Linux and macOS (it does not need audio playback support). Because the repository's `.cargo/config.toml` defaults to a Windows target, pass an explicit target:

```sh
cargo build --release -p audiolib-server --locked --target "$(rustc -vV | sed -n 's/^host: //p')"
```

Development checks:

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
```

## Usage

### Desktop client and CLI

```powershell
cargo run -p audiolib-desktop
cargo run -p audiolib-cli -- scan "D:/Sounds" --full
cargo run -p audiolib-cli -- search "explosion" --format table
cargo run -p audiolib-cli -- stats
```

User settings, logs, caches and the library database live in `%LOCALAPPDATA%/AudioLibExplorer/v1`; set `AUDIOLIB_DATA_DIR` to use an isolated directory. In the Data source window, choose server mode and enter the server address (for example `https://audiolib.example.com:8443` or `http://server.example.lan:8790`) and an access token if the server requires one. See [Remote mode](doc/Rust_Remote_Mode.md) and [library management](doc/Rust_Library_Management.md).

### Server

```sh
cp server/audiolib-server/config.example.toml /etc/audiolib/config.toml   # edit roots and database path
audiolib-server --config /etc/audiolib/config.toml
```

**Deployment & usage guide:** [server/README.md](server/README.md) (build, first use, client connection, internet access) and [server/deploy/README.md](server/deploy/README.md) (systemd/launchd/Docker, upgrade, backup). See [server/README.md](server/README.md) for configuration, authentication and deployment (systemd, launchd, Docker examples) and [doc/api/README.md](doc/api/README.md) for the API contract. For internet access put the server behind a TLS reverse proxy and use tokens.

## Versioning

Three independent versions: the program version (workspace `Cargo.toml`, `MAJOR.MINOR.PATCH`, shared by client, CLI and server), the API version (URL prefix `/api/v1/`) and the database schema version (`SCHEMA_VERSION`, currently 5). A schema change requires at least a MINOR program bump. Release tags are `vX.Y.Z` or `vX.Y.Z-rc.N` and must match `Cargo.toml`; `tools/check-release.ps1` enforces the policy. Details: [doc/Rust_Versioning.md](doc/Rust_Versioning.md).

## Releases and CI

The current release is v0.4.11, a patch release on top of the published v0.4.10.

GitHub Actions run formatting, Clippy and tests on `main` pushes and pull requests (`.github/workflows/ci.yml`). Pushing a `v*` tag runs `.github/workflows/release.yml`, which verifies the version policy and builds the Windows x64 ASIO client, Windows x64 server and Linux x64 server. The release is published only after all build jobs pass; each archive has a SHA-256 file and a public validation report. Linux GNU server binaries are built on Ubuntu 22.04.

Portable payloads use `tools/portable-files.json` and fresh staging. Generated local reports, user screenshots and runtime state are excluded. Release builds remap compiler paths, and `tools/validate-package-privacy.ps1` checks the payload and public report schema before publication. These checks do not remove information from already published Git history or releases.

## Documentation

Most documents under `doc/` are currently written in Chinese (for example the [versioning policy](doc/Rust_Versioning.md), [architecture](doc/Rust_Architecture.md) and [validation notes](doc/Rust_Validation.md)); English versions will be added over time. The [documentation index](doc/INDEX.md) separates current guides from historical design notes. Repository conventions for contributors are in [AGENTS.md](AGENTS.md).

## License

AudioLib Explorer is free software licensed under the GNU General Public License, version 3 only. See [LICENSE](LICENSE). Third-party components and their licenses are listed in [THIRD_PARTY.md](THIRD_PARTY.md).
