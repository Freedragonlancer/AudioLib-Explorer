# 0.9.3 发布准备

程序 0.9.3 | API /api/v1/ | 本地写 schema 8 | 服务端迁移 schema 9 | 快照兼容 3..=9

## 范围与当前状态

本次以已发布的 `v0.9.2` 为基线，准备 PATCH 版本 `v0.9.3`。应用改动包括 [#239](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/239) 的 Waveform 已播放区域接缝与 Play 按钮状态反馈修复，并借鉴旧 [#185](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/185) 的空页处理，修复服务器移除遮罩使整页结果不可见后连续浏览停止的问题。当前实现保留有界双向窗口、任务恢复和导航失败后的读取能力；不整合该旧 PR 的视觉重做、分页偏好、草稿工具等新功能。其他未合入功能需独立评估，不计入本次范围。

API、本地与服务端写目标、快照读取范围及完整格式均不变；不引入数据库迁移。

用户已授权完整 Release 流程，包括通过验收后正式发布。本轮桌面源码检查与交叉 CR 已通过；beta 服务端刷新、Windows smoke 及 Mac 客户端候选原生验收尚待针对冻结候选执行；提升 `main`、最终提交及标签身份尚未冻结；七份正式 payload 尚待从最终 main/tag 提交构建、打包和验收。本记录不表示尚未执行的验收已经通过。

`v0.9.2` 已于北京时间 2026-11-07 01:28 发布。准备审计时，GitHub Release 实际只有 Windows 客户端、Mac 服务端及各自 SHA256，共四项资产；历史产物保持原身份。本次按[版本规范](Rust_Versioning.md)准备完整七归档、十四资产，不能把既有包改名作为 0.9.3 产物。

## 已有源码验证证据

复用 #239 原始源码检查和视觉证据，保留其原始身份，不描述为本轮重新运行。私有证据为 `.rust-validation/playback-diagnosis/report.json` 及同目录日志；不上传此目录作为发布资产。

| 身份 | 原始记录 |
| --- | --- |
| 修改前 beta 基线 | `306e07de2bff31033a8f52bf4dd7e9bd9c2ab301` |
| 被验证功能提交 | `ced087b9fda57d93afd0a8410662b1f67c84fdea` |
| #239 合入 beta | `abbc6024c648fa6b8cacac22acba1c37ef636f1f` |
| 合入后的源码树 | `6d04ee9e2653a536daa6cd7979b5cf26341b63c9`，原记录确认与功能提交相同 |
| 工具链与 features | Rust `1.97.1-x86_64-pc-windows-msvc`，`asio` |
| 原始二进制版本 | `0.9.2 local GUI fixes on beta base 306e07d`，不是正式 v0.9.2 或 v0.9.3 发布包 |
| AudioLibExplorer.exe SHA256 | `d7618b84a4d256f2eb8bd3b53b709c287e49a2ed8de661cb9c21bfe4da80ea88` |
| AudioLibCli.exe SHA256 | `1895cfaf841d5df11e4fab025672f01eaaabc1f9208f536204d80a331cf60594` |

原记录包括格式检查、桌面 strict Clippy、2 个 Waveform 回归及 4800 次像素比较；隔离视觉矩阵覆盖 100% / 125% / 150% / 175% / 200% DPI，共 24160 帧，另有 12800 个大坐标浮点案例。Play 视觉覆盖 disabled / ready / playing / paused。发布构建、portable 验收与固定安装在该原始身份下已通过；它们证明当时的候选，不替代最终 0.9.3 交付验收。

源码检查仅在相关应用/测试输入、features、工具链和环境未变时复用；合并准备文档或仅更新版本不自动触发无关测试。相关输入变化后只重跑受影响检查。上述证据未包含新的声学输出延迟测量、真实 ASIO 硬件验收或 Mac 原生画面观察。原记录的缓存维护为 incomplete，40 GiB 预算未满足，不能记为已完成。

从 #185 定向移植的连续加载修复属于本轮新增行为，不能用 #239 的旧记录代替验证。选择受影响的桌面 strict Clippy 和针对性测试，检查服务器移除遮罩下整页不可见、正常可见页、SQL offset / opaque continuation、前后导航和相关状态恢复；新增 `search_task_scope_paging_tests::hidden_server_pages_do_not_stop_continuous_browsing` 使用已有真实 controller、SQLite 搜索和独占临时 fixture，不访问工作库或调用远端服务。实现和实际验证结果后续单独记录。

此次修复只调整查询结果的显示和继续加载，不改变服务器删除业务、存储、库管理协议或移除遮罩的持久化行为，因此不因该修复扩跑 `validate-library-management.ps1`。若实现范围改变并触及上述行为，应重新选择相应数据安全回归。

## 本轮桌面源码验证

以下为本轮实际执行结果，工具链为 Rust `1.97.1-x86_64-pc-windows-msvc`，发布所选 `asio` feature。证据保存在 `.rust-validation/release-093/source-checks/final-cache-ready/`，其 `identity.json` 记录被检查文件、工具链和配置；不上传私有日志或测试库。

| 检查 | 结果 |
| --- | --- |
| `cargo fmt --all -- --check` | 通过 |
| `cargo test -p audiolib-desktop --features asio --locked search_tasks::tests` | 58 个父入口/直接测试通过；54 个隔离 child 入口由相应父测试调用 |
| `cargo test -p audiolib-desktop --features asio --locked result_window::tests` | 7 个通过 |
| `cargo test -p audiolib-desktop --features asio --locked result_page_navigation::tests` | 8 个通过 |
| `cargo clippy -p audiolib-desktop --all-targets --features asio --locked -- -D warnings` | 通过 |

回归覆盖整页隐藏、跨多个原始查询跨度的双向加载、Path cursor 与不同 limit、窗口裁剪、冷热任务恢复、空 Previous、取消后的继续读取，以及资料库 revision 变化后的完整重建。单次查询最多 200 行，可见窗口最多 600 行；窗口位置和 revision 仅进入内存缓存，持久化工作区格式未改变。小范围交叉 CR 的阻断项均在合并前闭合。

测试使用自有临时库与合成记录，未访问工作库或源 WAV。数据库、扫描、配置与 Metadata 写入实现未改变，因此没有扩跑无关库管理、跨进程写入或音频硬件矩阵。#239 的原始视觉证据按上一节保留身份复用；最终各平台实际包仍需单独验收。

## beta 提升门槛

1. 将版本准备 PR 合入 `beta`，冻结其完整提交身份；检查实际功能输入与上述证据的对应关系。所有必要检查通过后才允许提升。
2. 在 Mac 上按 [AGENTS.md](../AGENTS.md) 的 Beta testing flow 运行当前 `tools/beta-refresh.sh`，绑定实际 `origin/beta` 提交，检查 `/health`、提交身份及每个配置根的读取。保留独立 beta 配置和库，不复制生产库。历史私有 wrapper 固定旧版本，不能直接作为本轮验收命令。
3. Windows 使用独立 beta 安装和对应 EXE SHA256 运行当前 smoke：

   ```powershell
   pwsh tools/beta-smoke.ps1 -BaseUrl $env:AUDIOLIB_BETA_URL -ExpectedCommit <beta完整提交> -MinEntries <实际预期记录数> -ReleaseDir beta_release -ExpectedExeSha256 <本轮已验收EXE的SHA256>
   ```

4. 本版 Waveform / Play 风险使用上述证据；连续加载修复需完成本轮针对性回归及必要的交互观察。输入变化或出现具体新风险时补充相应验证，不重复无关历史清单。真实失败阻止提升。
5. 通过后从 `beta` 开 PR 合入 `main`，等待 required `Linux (fmt)` 并冻结最终 main 身份，再按已授权的发布流程准备正式标签。发布前不得把尚未执行的 gate 写为通过。

## 完整发布矩阵

每个归档附同名 `.sha256`，最终精确为十四项资产。报告、截图、配置、日志、fixture 和数据库均不上传。

| 产物 | 正式归档 | 构建/验收环境 |
| --- | --- | --- |
| Windows 客户端 | `AudioLibExplorer-v0.9.3-win-x64-portable.zip` | Windows x64，默认 ASIO |
| Mac 客户端 | `AudioLibExplorer-v0.9.3-macos-arm64.zip` | arm64 Mac，已登录桌面 |
| Windows 服务端 | `AudioLibServer-v0.9.3-win-x64.zip` | Windows x64，无播放 features |
| Mac 服务端 | `AudioLibServer-v0.9.3-macos-arm64.tar.gz` | arm64 Mac |
| Linux 服务端 | `AudioLibServer-v0.9.3-linux-x64.tar.gz` | 匹配架构 GNU/Linux |
| Docker amd64 | `AudioLibServer-v0.9.3-docker-linux-amd64.tar.gz` | 已验收 Linux x64 包作为输入 |
| Docker arm64 | `AudioLibServer-v0.9.3-docker-linux-arm64.tar.gz` | 已验收 Linux arm64 包作为输入 |

Linux arm64 原生归档仅为 Docker 内部输入，不发布独立资产。AMD64 如在 ARM64 主机上仿真运行，报告必须标明；不能记为物理 x64 硬件覆盖。Mac 客户端签名和原生启动以本轮报告为准；ad-hoc 签名不等于 Apple 公证。

## 后续执行入口

版本契约预检不创建标签，准备期间可执行：

```powershell
pwsh tools/check-release.ps1 -Tag v0.9.3 -BaseTag v0.9.2
pwsh tools/release-pipeline.ps1 -Tag v0.9.3
```

管线默认仅预览；现有正式计划的 `require_tag=true`，`-Action Run` 需要真实标签指向当前干净 checkout。先完成 beta 门槛和合并，再冻结正式标签；不要为运行计划提前创建发布标签。各平台使用相同最终 main/tag 身份，每份 payload 构建、打包和验收一次，相关 source checks 按实际风险通过私有 `-ChecksFile` 选择。未请求的 source checks 保持 `not-requested`。

| 平台 | 正式入口与补充要求 |
| --- | --- |
| Windows 客户端 | `pwsh tools/release-pipeline.ps1 -Action Run -Tag v0.9.3 -CleanBuildArtifacts`；默认计划包含 ASIO 构建、打包、portable 验收及固定 `local_release/` 安装 |
| Windows 服务端 | `pwsh tools/release-pipeline.ps1 -Action Run -Tag v0.9.3 -PlanFile tools/release-plans/server.json`；计划显式构建验收所需 benchmark fixture helper |
| Mac 客户端 | 在已登录桌面的 arm64 Mac 执行 `pwsh tools/release-pipeline.ps1 -Action Run -Tag v0.9.3 -PlanFile tools/release-plans/macos-client.json`；结构与原生运行验收合并执行 |
| Mac/Linux 服务端 | 对应主机运行 `pwsh tools/release-pipeline.ps1 -Action Run -Tag v0.9.3 -PlanFile tools/release-plans/server-unix.json -VariablesFile <私有平台变量JSON>`；Mac 变量为 `{"Target":"aarch64-apple-darwin","Suffix":"macos-arm64"}`，Linux x64 使用计划默认值，内部 arm64 为 `{"Target":"aarch64-unknown-linux-gnu","Suffix":"linux-arm64"}` |
| Docker | 用 `tools/package-server-docker.ps1` 消费已验收的对应 Linux archive/report/validation，再用 `tools/validate-server-docker.ps1` 在自有合成 fixture 和临时容器中验收；分别选择 `linux/amd64` / `linux/arm64`，参数见[Linux 与 Docker 交付](Rust_Linux_Docker_Release.md)及脚本 param |

Windows 客户端本地产物默认文件名仍为 `dist/latest/AudioLibExplorer-latest-win-x64-portable.zip`；正式资产组装时按矩阵命名并生成匹配的 SHA256 文件，不能修改已验收 payload 内容。续跑使用相同 `Run`，状态及失败尾部使用 `-Action Status` / `-Action Logs`，不重复重建未失效阶段。

最终交付后按 [AGENTS.md](../AGENTS.md) 验证固定安装的包文件 SHA256、CLI 版本及一次隔离启动，保留正常资料库和数据目录。实际执行过 Rust 构建/测试的成功交付还需完成受管缓存预算维护；不同平台拥有的缓存分别按已验证注册处理。

全部必要验收通过、十四项资产齐备后，使用 `tools/release-create.sh v0.9.3 --notes-file <说明文件> <十四项资产>` 正式发布；发布后核验标题、标签、非 prerelease 标志、精确资产集合与 SHA256，并将固定 `local_release/` 与正式 Windows 资产身份同步。本记录尚未执行或宣称这些发布步骤完成。
