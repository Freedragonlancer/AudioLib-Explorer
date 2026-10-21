# AudioLib Explorer 版本规范

本文对应隔离开发分支 `0.5.0-dev.1`，不表示已发布或修改了当前安装。程序、API、schema 和派生 feature 各有版本，兼容性由[共享契约与完整格式目录](api/Library_Migration_Compatibility.md)共同判断，不能互相推导。

| 版本 | 来源（唯一事实） | 当前 | 含义 |
|---|---|---|---|
| 程序版本 | 工作区 `Cargo.toml` 的 `[workspace.package] version`（MAJOR.MINOR.PATCH） | 0.5.0-dev.1（未发布） | 客户端、CLI、服务端共用，发布时一起升 |
| API 版本 | URL 路径前缀 `/api/v1/` | v1 | 客户端与服务端的 HTTP 接口契约；不兼容改动才换成 `/api/v2/` |
| schema 版本 | `crates/audiolib-db/src/schema.rs` 的整数常量 `SCHEMA_VERSION` | 6 | SQLite 资料库结构（`user_version`） |

schema 边界为 `MIN_SNAPSHOT_SCHEMA_VERSION..=SCHEMA_VERSION`（当前 `3..=6`），但这不是充分的兼容判据；必须同时核对 storage、tokenization、Metadata、退役 feature 及迁移协议。客户端先验证快照身份、结构和完整性；派生搜索状态已准备的快照直接只读打开，需要修复的旧状态仅在客户端拥有的 staging/copy 中升级，原始只读文件保持不变。收紧下限（提高 `MIN_SNAPSHOT_SCHEMA_VERSION`）属于不兼容变化，应同时评估是否需要升 MAJOR 或 API 版本。

## 规则

1. **schema 变了，程序版本至少升 MINOR**（`SCHEMA_VERSION` 与上一个发布标签相比不同）。
2. 标签必须是 `vX.Y.Z`，预发布为 `vX.Y.Z-rc.N`，且 `X.Y.Z` 必须与 `Cargo.toml` 一致。其他预发布后缀（beta 等）会被拒绝。
3. 程序版本不得低于基准标签的版本。
4. 服务端与远程客户端源码中出现的 `/api/vN` 必须只有一个版本。
5. `doc/Rust_Release_Notes.md` 必须有**一行**同时写明程序版本、API 版本、schema 版本和快照兼容范围，例如：

   `程序 0.4.0 | API /api/v1/ | schema 5 | 快照兼容 3..=5`

6. 发布产物附 SHA256：`package-portable.ps1` 生成 `<zip>.sha256`，GitHub Actions 发布步骤把它连同 ZIP 上传为 Release 资产，并写入发布说明正文。

规则 1 到 5 由 `tools/check-release.ps1` 检查，失败时列出全部错误并以非零退出。

## 发布流程

**v0.4.11 已发布，不得覆盖其标签或把新的开发包当作该 Release 的原产物。** 本隔离工作区使用 `0.5.0-dev.1` 验证 schema 6、共享迁移及客户端/服务端集成；该开发标识不用于现有正式 Release 标签流程。下一次正式发布前，须确定发布号，同步更新工作区及锁文件、经过测试的双方程序范围与发布说明版本行，再以新标签执行完整发布流程。基准应使用已发布的 v0.4.11；本轮尚未创建发布标签，也未部署 8790。未完成的搜索性能验收须继续在发布说明中列明。

1. 把 `Cargo.toml` 的 workspace version 改为目标版本（同时更新 `Cargo.lock`）。
2. 在 `doc/Rust_Release_Notes.md` 写发布说明，并加入上述版本行。
3. 本地预检（不创建标签，标签用参数传入）：
   `pwsh tools/check-release.ps1 -Tag v0.4.6 -BaseTag v0.4.5`
   省略 `-BaseTag` 时，默认取除自身外最新的 `vX.Y.Z` 标签。
4. 运行 `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --locked -- -D warnings`、`cargo test --workspace --all-targets --locked`；ASIO 配置按 AGENTS.md 的准备和验证流程执行。
5. 合并后在发布提交上打标签 `v0.4.6`（或 `v0.4.6-rc.1`）并推送。
6. GitHub Actions（`.github/workflows/release.yml`）分别构建 Windows 客户端/服务端和 Linux x64 服务端。Windows 运行 `check-release.ps1`、`ci-release.ps1`；Linux 运行服务端和数据库严格 Clippy、全部 targets 测试及原生 release 构建。独立 server 包由 `package-server.ps1` 生成，只含允许的公共文件。默认 Windows workspace 使用 ASIO 构建，server 打包及验证也传入 `-Asio`，以同一 feature 闭包生成依赖通知，并核验已准备的固定 SDK 许可证；无 ASIO 构建及 Unix 不传该选项。所有构建成功后，发布 job 核验归档校验和及报告的标签/提交身份，再创建 GitHub Release，上传客户端和服务端归档、SHA256 与报告。
7. 从已发布 Release 下载客户端包、SHA256 和 `Rust_CI_Release.json`，核验归档及报告的标签/提交，再按 AGENTS.md 更新固定 `local_release/` 并核对两个 EXE。CI 产物作为安装身份基准，不以不同本机构建的哈希替代。使用 `pwsh`（PowerShell 7.3 以上）执行 portable validator，传入 `-PublishedReleaseReport <report> -ExpectedTag <tag> -ExpectedCommit <full-commit> -CleanCheckout` 核对已发布报告中的 EXE 哈希，并运行隔离首次启动、CLI/UI 和诊断检查；省略报告时仍要求本机 release 输出一致。

常用模拟（`-Root` 可指向临时克隆，便于不改工作区验证）：

- 通过：Cargo 0.4.6 + 版本行 + `-Tag v0.4.6 -BaseTag v0.4.5`
- 失败：`-Tag v0.4.12`（与 Cargo 不一致）；schema 变了只升 PATCH；缺版本行。
