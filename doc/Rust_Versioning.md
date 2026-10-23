# AudioLib Explorer 版本规范

本文对应合入 0.5.0 主线后的隔离 CR 分支 `0.6.0-dev.1`，不表示已发布或修改了当前安装。程序、API、schema 和派生 feature 各有版本，兼容性由[共享契约与完整格式目录](api/Library_Migration_Compatibility.md)共同判断，不能互相推导。此前 `0.5.0-dev.1` 验证保留为历史证据。

| 版本 | 来源（唯一事实） | 当前 | 含义 |
|---|---|---|---|
| 程序版本 | 工作区 `Cargo.toml` 的 `[workspace.package] version`（MAJOR.MINOR.PATCH） | 0.6.0-dev.1（未发布） | 客户端、CLI、服务端共用，发布时一起升 |
| API 版本 | URL 路径前缀 `/api/v1/` | v1 | 客户端与服务端的 HTTP 接口契约；不兼容改动才换成 `/api/v2/` |
| schema 版本 | `crates/audiolib-db/src/schema.rs` 的整数常量 `SCHEMA_VERSION` | 6 | SQLite 资料库结构（`user_version`） |

schema 边界为 `MIN_SNAPSHOT_SCHEMA_VERSION..=SCHEMA_VERSION`（当前 `3..=6`），但这不是充分的兼容判据；必须同时核对 storage、tokenization、Metadata、退役 feature 及迁移协议。客户端先验证快照身份、结构和完整性；派生搜索状态已准备的快照直接只读打开，需要修复的旧状态仅在客户端拥有的 staging/copy 中升级，原始只读文件保持不变。收紧下限（提高 `MIN_SNAPSHOT_SCHEMA_VERSION`）属于不兼容变化，应同时评估是否需要升 MAJOR 或 API 版本。

## 规则

1. **schema 变了，程序版本至少升 MINOR**（`SCHEMA_VERSION` 与上一个发布标签相比不同）。
2. 标签必须是 `vX.Y.Z`，预发布为 `vX.Y.Z-rc.N` 或 `vX.Y.Z-beta.N`，且 `X.Y.Z` 必须与 `Cargo.toml` 一致。其他预发布后缀会被拒绝。
3. 程序版本不得低于基准标签的版本。
4. 服务端与远程客户端源码中出现的 `/api/vN` 必须只有一个版本。
5. `doc/Rust_Release_Notes.md` 必须有**一行**同时写明程序版本、API 版本、schema 版本和快照兼容范围，例如：

   `程序 0.4.0 | API /api/v1/ | schema 5 | 快照兼容 3..=5`

6. 发布产物附 SHA256：`package-portable.ps1` 生成 `<zip>.sha256`，GitHub Actions 发布步骤把它连同 ZIP 上传为 Release 资产，并写入发布说明正文。
   每个 Release 只发布两个包：Windows 客户端和 macOS arm64 服务端，各附一个 `.sha256`。只有 macOS 服务端支持自动更新；不发布 Windows 和 Linux 服务端包。

规则 1 到 5 由 `tools/check-release.ps1` 检查，失败时列出全部错误并以非零退出。

## 发布流程

**v0.4.11 和 v0.5.0 已发布，不得覆盖其标签或把新的开发包当作这些 Release 的原产物。** 本隔离工作区使用 `0.6.0-dev.1` 继续验证 schema 6、共享迁移及客户端/服务端集成；该开发标识不用于现有正式 Release 标签流程。下一次正式发布前，须确定发布号，同步更新工作区及锁文件、经过测试的双方程序范围与发布说明版本行，再以新标签执行完整发布流程。本轮只整理 PR，不创建发布标签或部署服务端。

1. 把 `Cargo.toml` 的 workspace version 改为目标版本（同时更新 `Cargo.lock`）。
2. 在 `doc/Rust_Release_Notes.md` 写发布说明，并加入上述版本行。
3. 本地预检（不创建标签，标签用参数传入）：
   `pwsh tools/check-release.ps1 -Tag v0.4.6 -BaseTag v0.4.5`
   省略 `-BaseTag` 时，默认取除自身外最新的 `vX.Y.Z` 标签。
4. 正式发布前在本地执行 `tools/check-local.ps1 -Asio`、受影响的专项回归及打包启动验收；日常开发按 [Testing Strategy](Testing_Strategy.md) 选择最小范围。缺少平台或硬件覆盖时明确记录，不能以云端构建代替功能验证。
5. 合并后在发布提交上打标签 `v0.4.6`（或 `v0.4.6-rc.1`）并推送。
6. 发布包在对应本机平台构建、验证，使用 `gh release upload <tag> <files>` 上传已创建的 Release。独立 server 包由 `package-server.ps1` 生成，只含允许的公共文件；Windows 的 ASIO 构建、打包与许可证检查使用同一 feature 选择，Unix 不传 `-Asio`。发布流程：在本机分别构建并验证 Windows x64 客户端包与 macOS arm64 服务端包（各带 `.sha256`），用 `tools/release-create.sh <tag> <资产…>` 创建 Release（或对已创建的 Release 用 `gh release upload`），然后可选地手动触发 `.github/workflows/release.yml`（仅 `workflow_dispatch`，输入 `tag`）做只读核验：确认 Release 恰有上述 4 个资产且 SHA256 正确。该 workflow 不构建、不创建/编辑 Release、不上传资产（`contents: read`）；推送标签不会触发它，也不会覆盖本地上传的资产，因此打标签前无需先禁用 release.yml。Clippy、功能测试及 UI/服务端启动/health 均在本地执行。
7. 从已发布 Release 下载客户端包、SHA256 和 `Rust_CI_Release.json`，核验归档及报告的标签/提交，再按 AGENTS.md 更新固定 `local_release/` 并核对两个 EXE。CI 产物作为安装身份基准，不以不同本机构建的哈希替代。使用 `pwsh`（PowerShell 7.3 以上）执行 portable validator，传入 `-PublishedReleaseReport <report> -ExpectedTag <tag> -ExpectedCommit <full-commit> -CleanCheckout` 核对已发布报告中的 EXE 哈希，并运行隔离首次启动、CLI/UI 和诊断检查；省略报告时仍要求本机 release 输出一致。

常用模拟（`-Root` 可指向临时克隆，便于不改工作区验证）：

- 通过：Cargo 0.4.6 + 版本行 + `-Tag v0.4.6 -BaseTag v0.4.5`
- 失败：`-Tag v0.4.12`（与 Cargo 不一致）；schema 变了只升 PATCH；缺版本行。

## Release 命名规则

- 正式版：标签 `vX.Y.Z`，Release 标题 `AudioLib Explorer vX.Y.Z`，不是 prerelease。
- Beta：标签 `vX.Y.Z-beta.N`（rc 同理 `-rc.N`），标题 `AudioLib Explorer vX.Y.Z-beta.N`，必须标为 prerelease（`--latest=false`）。
- 标题、标签与 `Cargo.toml` 工作区版本（忽略 `-beta.N` / `-rc.N` 后缀）必须一致。标题由标签自动生成：`tools/release-create.sh <tag> [--notes-file f] [资产…]` 传 `--title "AudioLib Explorer <tag>"`，不要手写标题。
- 发布后用 `pwsh tools/check-release.ps1 -Tag <tag> -CheckGitHubRelease` 核对（或 `-ReleaseTitle <title> -Prerelease <bool>` 离线检查）；标题不符或 prerelease 标记与标签后缀不符均为硬错误。

## Beta 流程

1. 功能 PR 合入 `beta` 分支；`main` 只接受经过 beta 测试的 `beta`（或 hotfix 分支）合并。
2. 在 `beta` 的待测提交上打 `vX.Y.Z-beta.N`（`X.Y.Z` 与 `Cargo.toml` 一致），发布为 GitHub pre-release（`tools/release-create.sh <tag>`，等价于 `gh release create <tag> --title "AudioLib Explorer <tag>" --prerelease --latest=false`；完全由标签推导：带后缀的标签（`-beta.N` / `-rc.N`）自动加 `--prerelease --latest=false`，纯 `vX.Y.Z` 为正式版）。
3. 本机用 `tools/install-beta.ps1` 安装到独立目录（默认仓库下 `beta_release/`，可用 `-Destination` 指定），并通过 `AudioLibExplorer-beta.cmd` 启动；该脚本把 `AUDIOLIB_DATA_DIR` 设为 `%LOCALAPPDATA%/AudioLibExplorer/beta`，与正式版 `v1` 数据完全隔离。
4. 每个 beta 待测提交按 [AGENTS.md](../AGENTS.md) 的 Beta testing flow 依次执行：Mac 上 `tools/beta-refresh.sh` 刷新 beta 服务端（含 commit 与库根读取自检），Windows 上 `tools/beta-smoke.ps1` 冒烟，再过 GUI 手动清单；任何一步失败都不得提升。
5. beta 验收通过后，从 `beta` 开 PR 合入 `main`，再按上文流程打正式标签。
