# AudioLib Explorer

<img src="resources/icons/AudioLibExplorer.png" width="128" height="128" alt="AudioLib Explorer：音频素材库与 Waveform 图标">

[English](README.md)

AudioLib Explorer 是一款面向 Windows 的本地优先音效库管理软件，使用 Rust 编写，桌面界面基于 [Slint](https://slint.dev)。它把大型 WAV 音效库索引到 SQLite（FTS5）中，提供即时搜索，通过 WASAPI（可选 ASIO）试听，也可以浏览由自带的 `audiolib-server` 提供的远程音效库。

- **程序版本：** 0.4.8（工作区 `Cargo.toml`）
- **HTTP API：** `/api/v1/`（客户端与服务端）
- **数据库 schema：** 5（快照兼容范围 3..=5）
- **许可证：** [GPL-3.0-only](LICENSE)

## 功能

- 增量/完整文件夹扫描，提取专业 WAV Metadata（BWF `bext`、iXML、INFO、ID3、XMP，只读兼容 Soundminer SMED）。不会修改源 WAV 文件。
- 基于 SQLite FTS5 的搜索，支持中文分词（jieba-rs）、标签/分类筛选；文档化目标为 100 万以上记录 P95 < 500 ms。
- 流式解码播放、设备采样率重采样、波形与频谱视图、独立的音高/速率试听（SoundTouch），支持 WASAPI 与可选 ASIO 输出。
- 资料库管理：添加文件夹、修复被移动或改名的文件、批量标注、备份/恢复、完整性检查、JSONL/CSV 交换。
- 命令行工具 `AudioLibCli`（JSON/表格/CSV 输出、库管理、控制正在运行的桌面实例播放）。
- 服务器模式：`server/audiolib-server` 通过 `/api/v1/` 提供一个资料库（快照、Range 流式音频、预览、Waveform、令牌认证、仅局域网管理页）。客户端只读打开经过校验的快照；Metadata 和管理写入使用绑定库身份的 HTTP 请求。远程写入须同时升级客户端和服务端，旧服务端仍可读取。
- 中英文双语界面。

## 构建

要求：Windows、MSVC 构建工具以及固定版本的 Rust 工具链（见 `rust-toolchain.toml`）。ASIO 构建还需要 LLVM（libclang），并使用固定且校验哈希的 Steinberg ASIO SDK（由 `tools/prepare-asio.ps1` 下载）。

```powershell
cargo build --release -p audiolib-desktop -p audiolib-cli

# 可选：ASIO 支持
. ./tools/prepare-asio.ps1
cargo build --release -p audiolib-desktop -p audiolib-cli --features audiolib-desktop/asio,audiolib-cli/asio

# 便携包（dist/latest/portable）
./tools/build-release.ps1
./tools/package-portable.ps1 -SkipBuild
```

服务端可在 Windows、Linux、macOS 上构建（不需要音频播放支持）。由于仓库的 `.cargo/config.toml` 默认目标是 Windows，需显式指定目标：

```sh
cargo build --release -p audiolib-server --locked --target "$(rustc -vV | sed -n 's/^host: //p')"
```

开发检查：

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
```

## 使用

### 桌面客户端与 CLI

```powershell
cargo run -p audiolib-desktop
cargo run -p audiolib-cli -- scan "D:/Sounds" --full
cargo run -p audiolib-cli -- search "explosion" --format table
cargo run -p audiolib-cli -- stats
```

用户设置、日志、缓存和资料库位于 `%LOCALAPPDATA%/AudioLibExplorer/v1`；设置 `AUDIOLIB_DATA_DIR` 可使用隔离的数据目录。在“数据源”窗口选择服务器模式，填写服务器地址（例如 `https://audiolib.example.com:8443` 或 `http://server.example.lan:8790`），服务器要求时再填访问令牌。详见 [远程模式](doc/Rust_Remote_Mode.md) 与 [资料库管理](doc/Rust_Library_Management.md)。

### 服务端

```sh
cp server/audiolib-server/config.example.toml /etc/audiolib/config.toml   # 编辑根目录和数据库路径
audiolib-server --config /etc/audiolib/config.toml
```

**部署与使用指南：** [server/README.zh-CN.md](server/README.zh-CN.md)（构建、首次使用、客户端连接、外网访问）与 [server/deploy/README.md](server/deploy/README.md)（systemd/launchd/Docker、升级、备份）。配置、认证与部署（systemd、launchd、Docker 示例）见 [server/README.zh-CN.md](server/README.zh-CN.md)，API 契约见 [doc/api/README.md](doc/api/README.md)。面向公网时请放在带 TLS 的反向代理之后并启用令牌。

## 版本规范

三套互相独立的版本：程序版本（工作区 `Cargo.toml`，`MAJOR.MINOR.PATCH`，客户端、CLI、服务端共用）、API 版本（URL 前缀 `/api/v1/`）、数据库 schema 版本（`SCHEMA_VERSION`，当前为 5）。schema 变化时程序版本至少升 MINOR。发布标签为 `vX.Y.Z` 或 `vX.Y.Z-rc.N`，必须与 `Cargo.toml` 一致，由 `tools/check-release.ps1` 检查。详见 [doc/Rust_Versioning.md](doc/Rust_Versioning.md)。

## 发布与 CI

当前计划发布 v0.4.6。最近成功发布的版本仍为 v0.4.3；v0.4.4 和 v0.4.5 的发布验证未通过，已有标签保留。

GitHub Actions 在每次推送和 Pull Request 时运行格式、Clippy 与测试（`.github/workflows/ci.yml`）。推送 `v*` 标签会运行 `.github/workflows/release.yml`：校验版本规范、构建 Windows 便携包，并发布为带 SHA-256 文件的 GitHub Release。

## 文档

`doc/` 下的大部分文档目前为中文（如[版本规范](doc/Rust_Versioning.md)、[架构](doc/Rust_Architecture.md)、[验证记录](doc/Rust_Validation.md)），英文版将在后续补充。[文档索引](doc/INDEX.md)区分当前指南与历史设计文档。贡献者约定见 [AGENTS.md](AGENTS.md)。

## 许可证

AudioLib Explorer 是自由软件，采用 GNU 通用公共许可证第 3 版（仅 GPL-3.0）。见 [LICENSE](LICENSE)。第三方组件及其许可证见 [THIRD_PARTY.md](THIRD_PARTY.md)。
