# 1.0.0 发布准备

状态：**未发布，发布准备阶段**。截至 2026-11-13，应用候选为 beta `ecbe972b17774aeaabdba413ec60301d42c3e4af`，以正式 [v0.9.6](Rust_Release_0.9.6.md) 为发布基线。本轮只核对已完成应用修复、原始证据与发布步骤，不重新构建、打包、安装或部署。用户原先明确排除的 14 项流程／文档改动保持独立归属，不纳入本轮候选；核对期间，另一工作流将该组内容整理为本地提交 `8a78801`，剩余 3 项未提交内容保留，本轮未重置或合并它们。

程序 `1.0.0`；API `/api/v1/`；普通本地写 schema 8；专用服务端迁移 schema 9；快照读取 `3..=9`。base / `audio_sources:1`（af1）完整格式不变。正式目标为 `v1.0.0`，标题 `AudioLib Explorer v1.0.0`，非 prerelease；当前没有完成最终 main/tag 的七份正式 payload 交付。

## 已纳入的修复

| 来源 | 本次正式候选包含的行为 |
|---|---|
| [#285](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/285)，首次集成 `a01a19b` | AI 关键词／UCS／配方检索、多协议供应商和提示词、Windows DPAPI、推荐核验、五项手动／三项可选自动任务；保持资料库／修订边界。沿用 0.9.6 分页与会话历史，以及 #281 RIFF 完整子块兼容和已合并发布工具修正 |
| `cda17c6`、`87d005c`、`d0840ed` | AI 设置与预设集中入口、合法已保存配置下侧栏默认收起；tag 正文搜索与右缘“+”独立新任务；Classic / Studio 缩放与交互反馈；取消旧播放请求不再报告失败 |
| [#287](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/287)，`a10b288` | Jev 加入关键词／UCS／配方结构化召回、独立候选评分、自检及请求体预算；保留原失败和相关修复后的定向重跑 |
| [#288](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/288)，`0019cbd` | 配方候选和最终查询共用真实模式及 12 条输出上限，避免请求展示不会执行的分层词；自检保持直接模式 |
| 当前候选 `ecbe972` | GUI 等待实际音频确认，准备、NAS 首响应与缓冲各自拥有状态；旧素材／来源／关闭后的消息失效。保护 Metadata 关闭恢复提示；内部 revision 失效仅重新请求原作用域，普通取消与真实错误保持原义 |

历史 fix/review 分支没有额外遗漏的应用补丁：#281 已在 beta；#280 与 AI 祖先 `031d397` 的 patch-id 同为 `9faa269f64f1f5c08e3ed492336422874b855a45`，四个核心工作流文件 blob 相同；其余已合并或等价，不重复合并。当前功能边界见 [AI 智能搜索](https://github.com/Freedragonlancer/AudioLib-Explorer/blob/ecbe972b17774aeaabdba413ec60301d42c3e4af/doc/Rust_AI_Search.md) 和 [状态生命周期审查](reviews/Status_Feedback_Review_2026-11-13.md)。

## 原始验证证据及本轮复用

下表是本轮只读核对的**既有结果**，不是本轮重新执行。私有收据保留在原工作 checkout 的 `.rust-validation/`，不加入发布资产；不同阶段的测试有重叠，不相加成新的总数。

| 原始源码与收据 | 已完成结果 | 当前适用范围 |
|---|---|---|
| `ecbe972`；`status-feedback-review/final-report.json` | MSVC 1.97.1 / ASIO 下 audio、desktop、CLI 的格式与 strict Clippy；28 项定向回归通过。CI [run 43336068023](https://github.com/Freedragonlancer/AudioLib-Explorer/actions/runs/43336068023) 的 `Linux (fmt)` 通过 | 最新音频确认、状态、选区／输出、共享 fixture、搜索及关闭恢复路径。保留修复前记录和受影响项重跑，未宣称整套 workspace 通过 |
| 同一 `ecbe972` 原报告 | release build、便携包及 portable 7 检查通过；静音 fixture 的播放 IPC 20 检查、原生关闭 6 场景通过；固定 `local_release/` 与隔离 `beta_release/` 各 908 个包文件核验，固定安装隔离启动通过 | 原任务已完成的 Windows 候选交付；不是最终 main/tag payload，也不证明远程 beta 服务已部署或 smoke 通过 |
| 同一 `ecbe972` 原报告及 `cache.log` | 缓存维护已通过：逻辑删除约 4.94 GiB，剩余约 35.84 GiB，40 GiB 预算达标 | 这是原任务结果。本轮纯准备不再扫描缓存；较早 `a01a19b` 的占用阻塞仅保留为历史 |
| 检查源码 `8a1212cf1fe0c6800bd0787973ea145ee166f926`，集成 `0019cbd9c5821f343310935ef1a6087d36d404ff`；`jev-cr-0019cbd/final-delivery.json` | Jev 后端 31 项、桌面 AI 26 项回归及对应 strict Clippy 通过；原 Windows 包／安装身份单独保留 | `0019cbd..ecbe972` 的 `crates/audiolib-ai`、`crates/audiolib-core`、`ai_search.rs`、`ai_search_tests.rs` 和 `ai-search-panel.slint` 均无差异，因此复用该范围；替代更早仅 15 项的后端基线 |
| `d0840ed`；上述状态审查引用的 `tag-button-polish-playback-status/` | Classic / Studio、100%／125%／200% 下 tag “+”实际渲染、独立点击及远程显式取消分类已有验收 | `ecbe972` 没有改变其按钮资源、命中区域和远程取消实现；不再使用“全部 Slint 与首次 AI 分支相同”作为当前依据 |
| `.rust-validation/ai-delivery/review-fixes.log` 及既有配置／关闭证据 | 跨进程配置、数据保留和写入排空原有通过基线；本轮另核对实际输出确认、失败草稿、重复 Apply、关闭等待设置等 4 项回归通过 | 相对 `0019cbd`，Metadata FIFO／revision／drain、扫描／实例锁及原子偏好存储未变；Configure 只改为等待实际结果，随后仍走同一 `preferences.save`。新的关闭 6 场景补充 UI 保护与恢复，不替代 Metadata 数据安全基线 |

`8ad0292..ecbe972` 的 Cargo.toml、Cargo.lock、schema、compatibility 与原生发布计划无差异。数据库格式、扫描协调和 Metadata 写入顺序不因本轮文档准备变化；无需为刷新证据日期重跑稳定模块、百万记录或完整数据安全套件。代码或相应依赖／feature／环境变化时，按实际影响重新选择检查。

### 首次集成历史

AI `90edcfac68bacd7033df14b1304d02076f15fb5b` 与 beta `0185f1f39a13305ce940e827ae38ec2ca557e3d7` 经源分支提交 `f1e37a430c4cef1ffdc413adc693a5f4d465a8be` 和 #285 纳入 `a01a19bcdbabf53de68284ef39114a7dff53c4cc`。原 `.rust-validation/v1-preparation/` 保存格式、strict Clippy、21 项桌面／11 项 RIFF／3 项扫描边界测试，以及该 `a01a19b` 的 Windows 打包、portable 和 908 文件固定安装结果。原 AI 交付在 `.rust-validation/ai-cr-push/`。这些身份保持原样，不能写成最新 `ecbe972` 的重跑或最终正式包验收。

## 剩余门禁与顺序

1. **冻结 beta 候选并完成两端验收**：先确定真正参加晋升的 beta commit。获准进行 beta 环境验收后，按 [Beta testing flow](../AGENTS.md#beta-testing-flow) 完成 Mac `beta-refresh.sh` 的 served commit／health／根目录读取，以及 Windows `beta-smoke.ps1` 的快照、流读取和预期 EXE 身份。当前二者均未标为通过，固定 beta 安装不替代远程 smoke。候选相关 UI／平台风险按有效证据选择，不追加历史手工清单。
2. **PR 晋升 main，再冻结 tag**：两端门禁与受影响检查通过后，经 beta→main PR；核对最终 main 身份、Cargo 1.0.0 和 `v1.0.0`。本轮准备不创建晋升 PR 或 tag；后续 tag 不能移动来迁就旧二进制。
3. **从最终 main/tag 交付七份 payload**：使用下表原生计划，先预览，再在后续交付任务 Run；许可证预检、实际包和原生启动验收完整保留。复用仍有效的源码检查，最终 payload 必须来自最终提交，不能将 beta 候选包改名冒充正式包。
4. **只在明确发布授权后发布**：核对七归档及各自 `.sha256`，精确十四资产、发布说明、标题和非 prerelease 身份。使用正式 `release-create.sh`；已有草稿先检查再安全续传，显式发布前再次核验。不在准备阶段创建或发布 Release，不部署生产。
5. **发布后同步固定安装**：以正式下载包或已证实哈希匹配的本地 payload 更新同一 `local_release/`，核对包内文件、CLI 版本和一次隔离启动，保留资料库、偏好及源音频；已验收的同一 payload 不重复整套 portable。

本轮文档合入后，若 beta 只增加这些文档，应用检查继续保留 `ecbe972` 的原始身份；两端验收仍须记录届时实际 beta HEAD，核对其 served commit，不能把旧服务提交当作新候选。

## 七份正式 payload

各项最终 main/tag 验收目前均为**未请求／未执行**（`not-requested`），不是通过。七归档各带同名 `.sha256`，共十四资产；Linux arm64 原生包仅为 Docker 内部输入，不单独发布。

| 正式归档 | target / features | 原生主机与计划／剩余验收 |
|---|---|---|
| `AudioLibExplorer-v1.0.0-win-x64-portable.zip` | `x86_64-pc-windows-msvc`，ASIO SDK 2.3.4 | Windows；`windows-client.json`，build/package/portable/固定安装 |
| `AudioLibExplorer-v1.0.0-macos-arm64.zip` | `aarch64-apple-darwin`，ad-hoc | 已登录桌面的 Mac；`macos-client.json`，受影响 AI/消费者检查及 `-ValidateRuntime`；ad-hoc 不是 Apple 公证 |
| `AudioLibServer-v1.0.0-win-x64.zip` | `x86_64-pc-windows-msvc`，无默认 feature | Windows；`server.json`，独立 fixture helper、包隐私/启动/health |
| `AudioLibServer-v1.0.0-macos-arm64.tar.gz` | `aarch64-apple-darwin` | Mac；`server-unix.json`，实际 target/suffix、包启动/health |
| `AudioLibServer-v1.0.0-linux-x64.tar.gz` | `x86_64-unknown-linux-gnu` | 匹配 Linux 环境；`server-unix.json`，包启动/health，仿真须注明 |
| `AudioLibServer-v1.0.0-docker-linux-amd64.tar.gz` | 已验收 Linux x64 包 | 对应 Linux/Docker 环境，镜像、离线归档和自有 fixture 验收 |
| `AudioLibServer-v1.0.0-docker-linux-arm64.tar.gz` | 已验收内部 Linux arm64 包 | 独立 Linux arm64 run/target/output，随后镜像及 fixture 验收 |

独立主机／输出可并行；共享 Cargo target 或 `dist/latest` 的任务串行，Unix 两架构使用不同显式 RunDirectory。Mac 客户端组合入口一次完成结构和原生验收；Docker 在对应 Linux 包通过后即可开始。完整门禁及恢复方式见 [Release pipeline](Rust_Release_Pipeline.md)。

## 本轮准备检查

本轮版本预检已通过：`v1.0.0`、程序 1.0.0、API v1、本地写 8／服务端写 9、快照 3..=9，相对 v0.9.6 无 schema 变化。四份原生计划预览已通过，源码检查均标为 `source_checks=not-requested`；精确七归档／十四资产集合已确认。`Plan` 不要求创建 tag，不执行构建或原生验收；本轮仅复用应用结果，不重跑 Cargo、安装或缓存维护。

```powershell
pwsh -NoProfile -File tools/check-release.ps1 -Tag v1.0.0 -BaseTag v0.9.6
pwsh -NoProfile -File tools/verify-release-assets.ps1 -Tag v1.0.0 -ListExpectedAssets
pwsh -NoProfile -File tools/release-pipeline.ps1 -Action Plan -Tag v1.0.0 -PlanFile tools/release-plans/windows-client.json
pwsh -NoProfile -File tools/release-pipeline.ps1 -Action Plan -Tag v1.0.0 -PlanFile tools/release-plans/server.json
pwsh -NoProfile -File tools/release-pipeline.ps1 -Action Plan -Tag v1.0.0 -PlanFile tools/release-plans/macos-client.json
pwsh -NoProfile -File tools/release-pipeline.ps1 -Action Plan -Tag v1.0.0 -PlanFile tools/release-plans/server-unix.json
git diff --check
```

上例 Unix 默认 Linux x64；后续 Mac server／Linux arm64 使用声明 Target、Suffix 的私有变量文件和各自运行目录，不能把默认预览当作其他 target 的验收。真实付费供应商、Jev 语义准确率／阈值校准、真实休眠 NAS、真实 ASIO 硬件和声学起音延迟仍保留原限制；非 Windows 密钥暂只支持会话。性能目标仍为 **P95 latency <500ms for 1M+ records**，AI 网络耗时另计，本轮没有新增性能测量。
