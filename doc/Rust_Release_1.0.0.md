# 1.0.0 发布准备

状态：**未发布，beta 集成准备中**。程序版本 `1.0.0`；API `/api/v1/`；普通本地写 schema 8；专用服务端迁移 schema 9；客户端快照读取 `3..=9`。base / `audio_sources:1`（af1）完整格式沿用 [0.9.6](Rust_Release_0.9.6.md)，不引入资料库迁移。正式目标为 `v1.0.0`、标题 `AudioLib Explorer v1.0.0`，非 prerelease；当前尚未创建该正式发布身份。

## 来源与范围

本次整合 AI 分支 `90edcfac68bacd7033df14b1304d02076f15fb5b` 与 beta `0185f1f`。后者包含已发布 `v0.9.6` 的分页偏好、会话搜索历史，以及之后的发布工具修正和 [#281](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/281) RIFF 兼容修复。最终 beta 候选提交以合并 PR 为准，不能用这两个父提交之一冒充集成结果。

- AI 使用本地关键词、UCS 和素材配方，支持多协议供应商、提示词、模型列表与保存前测试、会话草稿、Windows DPAPI 密钥及请求尝试上限。最多展示五项经过当前实际查询核验的推荐；手动五任务和可选自动三任务均先校验容量。详见 [AI 智能搜索](Rust_AI_Search.md)。
- AI 结果绑定描述、供应商修订和资料库身份；目录过滤保留已完成推荐，过期生成仍拒绝。查询草稿与来源引用的审查修正、跨任务和历史恢复，以及侧栏、状态栏和紧凑播放器纳入本次集成。
- RIFF 仅在子块结构完整时兼容将八字节头计入父 size 的文件；真实截断、内部越界、错误布局及 RF64/BW64 检查保留。
- 发布工具支持固定安装目的地、版本清单及身份验证后的草稿续传。检查按影响范围选择，保留稳定模块原始通过证据；省略检查不计为通过。

其他 fix 分支审查未发现遗漏的应用修复：#281 已在 beta；[#280](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/280) 的稳定模块工作流补丁与 AI 祖先 `031d397` 的 patch-id 同为 `9faa269f64f1f5c08e3ed492336422874b855a45`，四个核心工作流文件 blob 相同；其余历史 fix/review 分支已合并或存在等价补丁，不重复合并。

## 已有证据及复用边界

以下为本机已有结果的只读核对，保留原始来源；不是合并后重新执行的结果。私有原始证据位于工作 checkout 的 `.rust-validation/`，不作为发布资产。

| 来源 | 已确认结果 | 对集成候选的适用边界 |
|---|---|---|
| AI `90edcfa`；`.rust-validation/ai-cr-push/final-validation.json` | 格式、AI 与桌面 ASIO strict Clippy；AI 15 项、桌面 18 项不同回归通过。原首轮 1 项失败和相关 3 项重跑通过均保留；四个最终源码文件哈希匹配原记录 | 保留原身份；合并所触及的搜索任务、分页、UI 与直接消费者需补集成检查 |
| AI `90edcfa`；`.rust-validation/ai-cr-push/final-delivery.json` | Windows ASIO release、许可证、打包、portable 通过；908 个固定安装文件核验、CLI 1.0.0 与隔离启动通过；原 CI run `43236216741` 成功 | 这是功能分支 payload，不能当作最终 beta 或 main/tag payload |
| `.rust-validation/ai-delivery/review-fixes.log`、`close.log`，及 AI 文档中各次实际交互记录 | 原子配置、实例锁、关闭排空及隔离交叉进程回归已有通过基线；后续源码检查保留相关未变路径的复用理由 | 实例锁、持久化、Metadata/扫描契约未变时可复用；若集成修正触及这些边界，重新选择相关数据安全回归 |
| `v0.9.6`，最终 main/tag `460629144d6a0df24af9c69303c9409c38efbde2` | 七份 payload、十四项线上资产和固定安装的历史验收记录 | 保留 [0.9.6 发布记录](Rust_Release_0.9.6.md)；版本号变化本身不重跑无关功能套件，旧包不能替代 v1.0 包 |
| 2026-11-12 AI checkout 的只读版本预检 | `pwsh tools/check-release.ps1 -Tag v1.0.0 -BaseTag v0.9.6` 通过，写目标与快照范围未变 | 合并及最终身份冻结后按实际 checkout 再核对版本，不创建标签 |

真实供应商账户、真实休眠 NAS、真实 ASIO 硬件和声学起音延迟没有新增验收。非 Windows 平台暂只支持会话密钥，不能宣称已实现跨平台持久密钥库。性能目标仍为 **P95 latency <500ms for 1M+ records**；本次 AI 网络耗时不算数据库查询延迟，未新增百万记录性能测量。

## 候选和最终交付的剩余工作

1. 完成合并冲突审查，针对搜索任务、分页、历史、AI 请求及推荐应用运行集成检查，检查实际三主题交互；受影响 RIFF 和直接消费者复用或补跑范围须保留来源。已选集成范围为格式、单次桌面 ASIO strict Clippy，以及分页、AI 缓存/范围、历史、恢复和页脚定向回归；AI 后端未变，保留原 15 项通过基线。证据目录为隔离工作树的 `.rust-validation/v1-preparation/`，当前不宣称这些集成检查已通过。Windows 使用与交付相同的 ASIO 配置，不追加无关全工作区套件。
2. 完成合并后候选的 Windows build/package/portable 和固定 `local_release/` 安装身份、CLI 与隔离启动。用原生脚本，保留用户库、偏好和源音频；原 AI 分支的成功不能替代集成包验收。
3. 冻结 beta 候选，按 [Beta testing flow](../AGENTS.md#beta-testing-flow) 完成 Mac beta-refresh 的 served commit / health / roots，以及 Windows beta-smoke 的快照、流读取和预期 EXE 身份。当前未宣称这些门禁通过；后续执行沿用对应部署授权和数据边界。
4. 门禁通过并进入正式发布任务后，经 PR 晋升 main，冻结最终 main/tag；以下每份 payload 从该最终身份构建、打包并验收一次。准备工作不创建 Release 或部署生产服务。

## 正式 payload 矩阵

以下各行的 v1.0 最终包均待交付；已有 Windows 功能分支候选和 0.9.6 历史证据按上表保留，不改写成新通过。实际主机、运行目录与最终提交须在执行时记录。

| payload | target / features | 原生主机与计划 | v1.0 剩余验收 |
|---|---|---|---|
| Windows x64 client | `x86_64-pc-windows-msvc`，ASIO SDK 2.3.4 | Windows；`windows-client.json` | 最终 build/package、portable、固定安装身份与隔离启动 |
| macOS arm64 client | `aarch64-apple-darwin`，ad-hoc | 已登录桌面的 Mac；`macos-client.json` | AI/direct-consumer 平台检查；`package-macos-client.ps1 -ValidateRuntime` 合并结构与原生启动/Quit 验收；ad-hoc 不等于 Apple 公证 |
| Windows x64 server | `x86_64-pc-windows-msvc`，无默认 feature | Windows；`server.json` | 最终包、独立构建的 `search-benchmark` fixture helper、包隐私/启动/health |
| macOS arm64 server | `aarch64-apple-darwin` | Mac；`server-unix.json` | 独立 run/target/output；最终包启动/health |
| Linux x64 server | `x86_64-unknown-linux-gnu` | 匹配 Linux 环境；`server-unix.json` | 独立 run/target/output；最终包启动/health；仿真执行须注明 |
| Docker linux/amd64 | 已验收 Linux x64 包 | 对应 Linux/Docker 环境 | 镜像、离线归档及自有 fixture 验收 |
| Docker linux/arm64 | 已验收 Linux arm64 内部包 | 对应 Linux arm64/Docker 环境 | arm64 独立原生计划后制作镜像；离线归档及自有 fixture 验收 |

精确发布集合为七个归档及各自 `.sha256`，共十四资产；内部 Linux arm64 原生包不单独发布。Mac 客户端的结构与原生验收由组合入口执行一次；Docker 可在其对应 Linux 包验收后开始，无需等待另一架构。共享 target 或 `dist/latest` 输出必须串行，Unix 两架构必须使用不同显式 RunDirectory。最终包的许可证、隐私、完整身份及本地启动验收不由 GitHub 的 format-only CI 替代。

执行使用 [Release pipeline](Rust_Release_Pipeline.md) 原生计划，先预览再 Run，失败仅续跑受影响阶段。全部 Rust 交付结束后集中维护一次已验证拥有的构建缓存。正式发布授权到位后，核验精确资产及 checksum，发布完成再同步固定安装与正式包身份；当前未执行正式 tag、七包交付、发布或生产部署。
