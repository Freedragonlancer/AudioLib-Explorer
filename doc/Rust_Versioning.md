# AudioLib Explorer 版本规范

三套版本各管各的，互不推导。

| 版本 | 来源（唯一事实） | 当前 | 含义 |
|---|---|---|---|
| 程序版本 | 工作区 `Cargo.toml` 的 `[workspace.package] version`（MAJOR.MINOR.PATCH） | 0.4.1 | 客户端、CLI、服务端共用，发布时一起升 |
| API 版本 | URL 路径前缀 `/api/v1/` | v1 | 客户端与服务端的 HTTP 接口契约；不兼容改动才换成 `/api/v2/` |
| schema 版本 | `crates/audiolib-db/src/schema.rs` 的整数常量 `SCHEMA_VERSION` | 5 | SQLite 资料库结构（`user_version`） |

快照兼容范围：`MIN_SNAPSHOT_SCHEMA_VERSION..=SCHEMA_VERSION`（当前 `3..=5`）。客户端先验证快照身份、结构和完整性；派生搜索状态已准备的快照直接只读打开，需要修复的旧状态仅在客户端拥有的 staging/copy 中升级，原始只读文件保持不变。收紧下限（提高 `MIN_SNAPSHOT_SCHEMA_VERSION`）属于不兼容变化，应同时评估是否需要升 MAJOR 或 API 版本。

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

当前发布版本为 **0.4.1**（应用图标更新；API 与 schema 和 v0.4.0 一致）。

1. 把 `Cargo.toml` 的 workspace version 改为目标版本（同时更新 `Cargo.lock`）。
2. 在 `doc/Rust_Release_Notes.md` 写发布说明，并加入上述版本行。
3. 本地预检（不创建标签，标签用参数传入）：
   `pwsh tools/check-release.ps1 -Tag v0.4.1 -BaseTag v0.4.0`
   省略 `-BaseTag` 时，默认取除自身外最新的 `vX.Y.Z` 标签。
4. 运行 `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --locked -- -D warnings`、`cargo test --workspace --all-targets --locked`；ASIO 配置按 AGENTS.md 的准备和验证流程执行。
5. 合并后在发布提交上打标签 `v0.4.1`（或 `v0.4.1-rc.1`）并推送。
6. GitHub Actions（`.github/workflows/release.yml`）在 Windows runner 上：检出标签，拉取其他 `v*` 标签，运行 `check-release.ps1`，再执行 `ci-release.ps1`（校验、构建、打包），最后创建 GitHub Release 并上传 ZIP、SHA256 与报告。

常用模拟（`-Root` 可指向临时克隆，便于不改工作区验证）：

- 通过：Cargo 0.4.1 + 版本行 + `-Tag v0.4.1 -BaseTag v0.4.0`
- 失败：`-Tag v0.4.2`（与 Cargo 不一致）；schema 变了只升 PATCH；缺版本行。
