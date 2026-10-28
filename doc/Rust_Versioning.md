# AudioLib Explorer 版本规范

程序、API、schema 和派生 feature 各有版本，兼容性由[共享契约与完整格式目录](api/Library_Migration_Compatibility.md)共同判断，不能互相推导。下表描述当前 0.8.3 源码；历史开发版本和已发布版本的验证记录保留原来的版本标识。

| 版本 | 来源（唯一事实） | 当前 | 含义 |
|---|---|---|---|
| 程序版本 | 工作区 `Cargo.toml` 的 `[workspace.package] version`（MAJOR.MINOR.PATCH） | 0.8.3 | 客户端、CLI、服务端共用，发布时一起升 |
| API 版本 | URL 路径前缀 `/api/v1/` | v1 | 客户端与服务端的 HTTP 接口契约；不兼容改动才换成 `/api/v2/` |
| schema 版本 | `crates/audiolib-db/src/schema.rs` 的整数常量 `SCHEMA_VERSION` | 8 | SQLite 资料库结构（`user_version`） |

当前可写完整格式为 `ale-s8-cld1-t3-m2-p1-a1`，必需 feature 包括 `album_entities=1`，Metadata 保持原始 JSON TEXT；Metadata 压缩暂缓。实验格式 `ale-s8-cld1-t3-m2-p1-a1-z1` 的 `metadata_storage=1` 仅保留为严格只读兼容来源，不在可写目录或迁移目标中。快照外层 zstd 传输与 Metadata 列存储分别协商。

schema 边界为 `MIN_SNAPSHOT_SCHEMA_VERSION..=SCHEMA_VERSION`（当前 `3..=8`），但这不是充分的兼容判据；必须同时核对 storage、tokenization、Metadata、退役 feature 及迁移协议。客户端先验证快照身份、结构和完整性；当前 schema 8 的完整快照直接只读打开。schema 3–7 即使原搜索状态就绪，也仅在客户端拥有的 staging/copy 中升级并建立共享专辑投影；原始只读文件保持不变。收紧下限（提高 `MIN_SNAPSHOT_SCHEMA_VERSION`）属于不兼容变化，应同时评估是否需要升 MAJOR 或 API 版本。

## 规则

1. **schema 变了，程序版本至少升 MINOR**（`SCHEMA_VERSION` 与发布基线相比不同；历史 0.8.1 使用下述明确的基线例外）。
2. 标签必须是 `vX.Y.Z`，预发布为 `vX.Y.Z-rc.N` 或 `vX.Y.Z-beta.N`，且 `X.Y.Z` 必须与 `Cargo.toml` 一致。其他预发布后缀会被拒绝。
3. 程序版本不得低于基准标签的版本。
4. 服务端与远程客户端源码中出现的 `/api/vN` 必须只有一个版本。
5. `doc/Rust_Release_Notes.md` 必须有**一行**同时写明程序版本、API 版本、schema 版本和快照兼容范围，例如：

   `程序 0.4.0 | API /api/v1/ | schema 5 | 快照兼容 3..=5`

6. 发布产物附 SHA256：`package-portable.ps1` 生成 `<zip>.sha256`，本机发布步骤把它连同 ZIP 上传为 Release 资产，并写入发布说明正文。
   每个 Release 只发布两个包：Windows 客户端和 macOS arm64 服务端，各附一个 `.sha256`。只有 macOS 服务端支持自动更新；不发布 Windows 和 Linux 服务端包。

规则 1 到 5 由 `tools/check-release.ps1` 检查，失败时列出全部错误并以非零退出。

## 发布流程

不得覆盖既有标签或把新构建当作旧 Release 的原产物。当前准备 0.8.3 候选，以已发布的 `v0.8.2` 为发布基线；API、schema 和完整存储格式未变，因此本次使用 PATCH 版本。发布前同步工作区、锁文件、双方程序版本与发布说明，并完成以下本地及 beta 门禁。既有 `v0.8.2` 及更早标签和资产保持原发布身份。发布状态以本地验收、beta 门禁与 Mac 产物检查的实测结果为准。

历史基线说明：`v0.8.0` 的 Release 已撤回，但标签保留。0.8.1 当时按用户确认使用最后实际公开的 `v0.7.8` 为发布基线：程序 MINOR 从 7 升至 8，可写 schema 6 库升级为 8；撤回的 0.8.0 开发库（schema 7）也通过受保护迁移升级为 8。回退旧程序须配套恢复相应旧 schema 备份。这个例外不重写既有标签，也不改变后续版本的版本升级规则。

1. 把 `Cargo.toml` 的 workspace version 改为目标版本（同时更新 `Cargo.lock`）。
2. 在 `doc/Rust_Release_Notes.md` 写发布说明，并加入上述版本行。
3. 本地预检（不创建标签，标签用参数传入）：
   `pwsh tools/check-release.ps1 -Tag v0.8.3 -BaseTag v0.8.2`
   仅标签去除 beta/rc 后缀后为 `v0.8.1`且未给出 `-BaseTag` 时，脚本默认 `v0.7.8`；显式 `-BaseTag` 始终优先。其他版本仍默认取除自身外最新的 `vX.Y.Z` 标签。显式以撤回标签 `v0.8.0` 作基线仍会按 schema 7→8 检查并拒绝仅 PATCH 的比较；这不是移动或替换该标签。
4. 正式发布前在本地执行 `tools/check-local.ps1 -Asio`、受影响的专项回归及打包启动验收；日常开发按 [Testing Strategy](Testing_Strategy.md) 选择最小范围。缺少平台或硬件覆盖时明确记录，不能以云端构建代替功能验证。
5. 合并后在发布提交上打标签 `v0.8.3`（或对应的预发布标签）并推送。
6. 发布包只在对应本机平台构建并验证（不再由 CI 构建）：Windows 上构建 Windows x64 客户端包，Mac 上构建 macOS arm64 服务端包，各自生成对应的 `.sha256`。独立 server 包由 `package-server.ps1` 生成，只含允许的公共文件；Windows 的 ASIO 构建、打包与许可证检查使用同一 feature 选择，Unix 不传 `-Asio`。Clippy、功能测试及 UI/服务端启动/health 均在本地执行。正式包必须从最终 main/tag 提交构建；beta 的预编译服务器不能替代嵌入最终提交身份的正式产物。
7. 上传本机构建的 4 个资产（两个包及各自的 `.sha256`）：用 `tools/release-create.sh <tag> <资产…>` 创建 Release 并上传，或对已创建的 Release 用 `gh release upload <tag> <资产…>`。之后可选地在 GitHub Actions 中手动触发 `.github/workflows/release.yml`（仅 `workflow_dispatch`，输入 `tag`）做只读核验：确认 Release 恰有上述 4 个资产且 SHA256 正确。该 workflow 不构建、不创建/编辑 Release、不上传资产（`contents: read`）；推送标签不会触发它，因此打标签前无需先禁用 release.yml。本机构建包先执行 `pwsh tools/validate-portable.ps1 -BinaryDirectory dist/latest/portable`（旧 fixture 不可用时加 `-CleanCheckout`），再按 AGENTS.md 更新固定 `local_release/`，核对包内文件哈希及隔离启动。下载的已发布产物使用 `-PublishedReleaseReport <report> -ExpectedTag <tag> -ExpectedCommit <full-commit> -CleanCheckout` 模式；`ExpectedTag` 和 `ExpectedCommit` 不能脱离发布报告单独使用。发布后按 AGENTS.md 的 Published release synchronization 核对本地安装与正式资产身份。

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
