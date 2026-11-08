# 0.9.4 发布准备

程序 0.9.4 | API /api/v1/ | 本地写 schema 8 | 服务端迁移 schema 9 | 快照兼容 3..=9

本次准备把 MP3、AIFF/BWF 音频源、RIFF 边界修复和自适应搜索任务标签整理为下一个 PATCH 候选。beta 功能证据可以按输入身份复用；最终 main/tag、正式包、平台验收和发布后安装同步仍待完成。远端 v0.9.4 标签与 GitHub Release 尚未创建。

## 候选与已发布身份

| 项目 | 身份与状态 |
| --- | --- |
| 版本基线 | 已发布 [v0.9.3](https://github.com/Freedragonlancer/AudioLib-Explorer/releases/tag/v0.9.3) |
| 0.9.3 原标签 | `5ca7e59c7fcadc22043f0a030c8d496854809b23`，不移动或覆盖 |
| 0.9.3 当前客户端 | 两个客户端曾按明确授权替换，来自 main `81ac352fdd73a009197a1442887dc65256aca302`；其余五份归档仍为原 tag 身份，见 [0.9.3 发布记录](Rust_Release_0.9.3.md) |
| 已集成 beta 功能基线 | `e8c0ba1f0566651da7bf647831811a933414a994`；[#255](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/255) 与 [#256](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/256) 已合并，`Linux (fmt)` 通过 |
| 准备版本 | workspace 及十个内部 package 统一为 `0.9.4`；另包含隐藏缓存所有权标记的脚本修正 |
| 候选只读预检 | `check-release -Tag v0.9.4 -BaseTag v0.9.3` 通过，`schemaChanged=false`；Windows 客户端管线 Plan 通过，`source_checks=not-requested`，没有执行正式构建或包验收 |
| 最终 main 提交 | pending，须通过 PR 完成 beta 提升 |
| 正式标签 | 计划 `v0.9.4`；最终 main 绑定、本地构建标签和远端推送分别登记，当前均 pending |
| 正式 payload 与 SHA256 | pending，不用 beta 或既有 0.9.3 包冒充新产物 |
| GitHub 发布与固定安装 | pending，正式 Release 尚未创建 |

当前已发布的 Classic 播放控件反馈属于 0.9.3 客户端替换，不再列为 0.9.4 新功能。版本比较以 `v0.9.3` 为契约基线，行为说明同时尊重两个实际已交付客户端的独立身份。

## 本版变更

- 默认音频源扩展名新增 `.mp3`、`.aif`、`.aiff`、`.bwf`，容器以实际字节识别，包含远程 opaque `.wav` 身份。已有 WAV、FLAC、Ogg/OGA 支持保留。
- RIFF 按父容器结束位置遍历子块，RF64/BW64 使用合法前置 ds64 的 64 位长度。容器外尾信息只告警，不污染 Metadata；容器内真截断、错误布局和不支持的长度规则仍明确拒绝。
- MP3 保留有界 ID3v2、COMM/TXXX、未知重复文本、ID3v1 fallback；封面仅在请求时提取，二进制不进入 Metadata JSON。帧计数、信息帧存在性、gapless trim 和缓存布局分别验证；坏可选计数不能截短实际音频。
- AIFF 保留 NAME、AUTH、ANNO、COMT 上下文及 ID3，支持 signed PCM 1–32 valid bits，以及 AIFC NONE/twos/sowt 和 big-endian fl32/fl64。其他 AIFC 压缩及 block-aligned SSND 仍不支持；`.aifc` 不是新增的默认扫描扩展名。
- 非 WAV 远程试听保持原始 `/stream`，不新增压缩预览路线。MP3 合成头仅存在于解码器内存中；源音频、HTTP 响应与完整下载缓存不被改写。AIFF seek、Waveform 和试听共享真实存储布局。
- 搜索标签先共享可用空间，最小 144 逻辑像素，再显示溢出导航和独立滚动条空间；关键词优先，目录和前缀按空间让位。
- 缓存维护工具修正隐藏所有权标记读取，补充真正 Hidden 属性的预览和清理回归；外部缓存仍须显式认领，release 输出和未知历史缓存继续保留。

## 兼容与升级边界

本地写目标仍为 schema 8，专用服务端迁移目标仍为 schema 9，客户端数字读取范围仍为 3..=9，HTTP 仍为 `/api/v1/`。按 [版本规则](Rust_Versioning.md#规则)，写目标未变化，因此 `0.9.4` 的 PATCH 选择符合仓库硬规则；这不表示所有完整数据库格式都与旧客户端兼容。

| 库中实际能力 | schema 8 精确格式 | schema 9 精确格式 |
| --- | --- | --- |
| 原有格式，尚未启用扩展源 | `ale-s8-cld1-t3-m2-p1-a1` | `ale-s9-cld1-t3-m2-p1-a1-sd1` |
| 必需 feature `audio_sources:1` | `ale-s8-cld1-t3-m2-p1-a1-af1` | `ale-s9-cld1-t3-m2-p1-a1-sd1-af1` |

AF1 在首次成功写入扩展源记录的同一事务中发布，不是启动时的全库迁移；原格式库和空库保持原 ID。删除测试记录不会自动移除已启用的能力要求。导入、迁移、回滚和派生状态重建均保留实际 af1 契约。

新客户端可校验 base/af1 两种格式，旧客户端不能因 schema 数字相同而打开 af1。既有 schema-9 WAV-only 兼容投影继续提供历史 WAV 表示，排除新增容器，包含伪装为 `.wav` 的非 WAV；它移除 af1 要求并保留原 schema-8 投影身份，不代表旧客户端获得新源能力。该投影不是任意 schema-8 af1 库的通用降级方式。

升级时先使用受支持的流程备份资料库，关闭共享本地库的旧版读写实例，再更新需要访问扩展源的客户端、CLI 和服务端，最后扫描新增格式。新检查不能追溯约束已经打开的旧 SQLite 实例。回退旧程序需要匹配的兼容备份或受支持的只读投影，不能手工删除 feature 标记。源文件和工作库运行数据不属于安装或发布资产。

## 可复用的验证及原始身份

以下记录来自本次 beta 功能开发，均非最终 0.9.4 正式 payload 的新验收。相关源码、features、工具链与环境保持一致时复用已有结果；版本号、标签或文档变化本身不要求重跑无关功能套件。

| 范围 | 原始身份和实际结果 |
| --- | --- |
| Windows 源码及应用检查 | 基于 `ff45063` 的 dirty 工作区，程序仍显示 0.9.3；发布所选 `asio` 客户端、无默认特性服务端。格式、strict Clippy、原有通过目标及最终受影响目标补跑共同组成通过证据，不能描述为单次全 workspace 从未失败 |
| 音频核心 | 最终音频目标 157 passed、0 failed、3 ignored；覆盖 RIFF/RF64/BW64、AIFF PCM/float、MP3 原生 Symphonia 逐样本参考、gapless、首尾及非零 anchor seek、缓存声明和坏信息帧 |
| 数据安全与直接消费者 | 受影响库管理、review fixes、ACL、播放 IPC、迁移、服务端格式与快照契约，以及旧 golden/统计 fixture 修正后的目标已验证；保留各自日志和输入，不把 0.9.3 历史全套验收当作新结果 |
| Windows 已有安装 | 同一 0.9.3 dirty 源码 payload 已构建、打包、portable 验收及固定安装；新版三种格式 GUI Metadata、搜索、播放、seek、自然结束通过，源 SHA256 保留。这不是 0.9.4 的构建或安装 |
| beta 合入审计 | 原工作区 51 tracked 与 16 untracked，共 67 项源码/fixture，逐项与 `e8c0ba1` Git 内容一致；两个 PR 均合入 beta。`Linux (fmt)` 只证明格式检查 |
| Mac 原生 beta 服务端 | 功能检查来自 `36b7ca8df30ebe42402afe044e17ef464d09c8c6`；至 `e8c0ba1` 只有四个客户端 UI/测试/文档文件变化，因此相关服务端结果复用。最终 e8 beta 原生构建、打包与服务验收另有独立收据，仍不是 0.9.4 最终 main/tag payload |
| 最终 beta 冒烟 | `e8c0ba1` 服务身份；9 checks、20/20 Range 通过。此前 909,345 条隔离完整同步在 revision 11 通过，最终冒烟使用 NoSync 复用该同步，不宣称重新全量下载 |
| 缓存工具候选回归 | Windows 与 macOS arm64 的正式 `test-build-artifacts.ps1` 合成回归均 exit 0；覆盖隐藏 owner 的 Plan/Run、边界保护和管线成功/失败交付。Mac 使用 PowerShell 7.6.6 的独占临时目录，不是对旧 beta 真实缓存重新执行维护 |

自生成端到端输入为 MP3、AIFF、BWF 各一个 30 秒、48 kHz mono 文件，AIFF 24 bit、BWF 16 bit、MP3 bit depth 为 0。首次扫描 added=3 / errors=0；Metadata、FTS、MIME、Range 和完整原始 SHA256 通过；写入三条标注后，重扫 unchanged=3，ID 和标注保留。GUI 三格式均完成 playing、seek 到 15 秒和自然 ended=30 秒，route 为 `stream`，无 error 或 underrun；AIFF 另验证暂停/继续。临时 root 和三条库记录已移除，最终总数恢复为 909,342，源 SHA256 保留，af1 要求持续存在。

原始私有证据位于 `.rust-validation/audio-source-formats/` 与 `.rust-validation/beta-audio-source-formats/`，包含各阶段日志、汇总和原生收据；不上传这些文件、地址、token、配置、截图或工作库作为 Release 资产。后续正式交付另记录最终源码和实际二进制身份。

## 保留的验收限制

- 原始问题样本未纳入本轮验收。没有重新验收报告中的 2,005 个真实 WAV 或其余异常样本，不能据合成回归宣称它们全部修复入库。音频核心三个 ignored 中，一个是主动导出跨实现 fixture 的辅助入口，另外两个依赖真实 BOOM/SMED 样本。
- beta 原有工作库 909,342 条中，当前已配置 root 覆盖 80,562 条，另 828,780 条历史保留记录在该配置开放范围之外。受保护 `/stream` 对这些路径返回 404 是该 beta 的既有配置边界，不是本次漏扫或音频丢失证据，也不是逐文件读权限审计或所有用户的产品限制。
- 数字解码、状态、Range/SHA256 与 underrun 证据不等于物理输出或主观音质验收。本轮没有新测真实 button-to-output 延迟、ASIO 硬件或其他驱动。
- 旧 beta 的标准缓存维护因隐藏所有权标记读取失败，未删除字节，标准预算检查未完成。独立只读逻辑估算为 debug 7.835 GiB、release 1.024 GiB，不能替代标准工具通过。0.9.4 工具修正的 Windows/Mac 合成回归通过，不代表旧 beta 真实维护已补跑，也不改写旧失败。
- 最终 0.9.4 的各平台编译、实际包、正式签名/启动、SHA256、线上身份与固定安装仍 pending。已有 beta 包和 Windows 0.9.3 本地包不能替代这些 gate。
- Linux x64 计划沿用 Apple Silicon 主机上的 OrbStack Linux amd64 仿真环境，必须按仿真条件记录；不描述为物理 x64 原生主机验收。

## 最小准备与正式 payload 计划

准备改动限制为 workspace/Cargo.lock 版本、当前发布说明与本准备记录、版本规范/索引/README 当前状态，以及上述缓存脚本修正及其回归。历史发布记录、标签、既有资产和用户资料库不改写。候选只读版本预检、Windows 管线预览和 Windows/Mac 缓存工具合成回归已通过；未把 Plan 的 `source_checks=not-requested` 改写为新功能测试通过。应用源码证据按上述身份复用，准备命令为：

```powershell
pwsh tools/check-release.ps1 -Tag v0.9.4 -BaseTag v0.9.3
pwsh tools/release-pipeline.ps1 -Tag v0.9.4
```

`check-release` 不加 `-CheckGitHubRelease`，因为 v0.9.4 还未发布；管线默认只是预览。准备 PR 和 beta gate 通过后，通过 PR 提升 main。最终干净 main 提交确定后，才可创建与其绑定的本地 `v0.9.4` 供本机构建校验；匹配主机使用该确切提交/本地标签运行实际原生计划，不先推送远端标签来触发准备。每份 payload 构建、打包及验收一次，后续同一身份续跑复用收据。

| payload | 实际主机和计划 | 最终状态 |
| --- | --- | --- |
| Windows x64 客户端 | `windows-client.json`，默认经校验的 ASIO SDK，portable 验收后更新原固定 `local_release/` 并核对身份/隔离启动 | pending |
| macOS arm64 客户端 | 已登录桌面的 Apple Silicon Mac，`macos-client.json`，合并结构与原生验收；ad-hoc 签名不冒充 Apple 公证 | pending |
| Windows x64 服务端 | `server.json`，无播放后端，显式 benchmark helper 仅供包 fixture | pending |
| macOS arm64 服务端 | 匹配主机 `server-unix.json`，server 与 migrator 同源，启动/health/格式契约 | pending |
| Linux x64 服务端 | Apple Silicon 上 OrbStack Linux amd64 仿真，使用 `server-unix.json` 构建及验收；按仿真环境记录 | pending |
| Docker linux/amd64 | 消费同一已验收 Linux x64 包，单平台构建并在上述仿真条件下验证离线镜像 | pending |
| Docker linux/arm64 | 消费匹配主机验收的内部 Linux arm64 包，单平台构建并验证离线镜像；不额外发布独立 arm64 原生包 | pending |

新增 `symphonia-bundle-mp3 0.6.1` 必须进入实际选中特性的许可证预检。库能力/迁移和原文件传输已改变，匹配主机的服务端/数据库回归及包验收不能用 Windows 或另一架构的结果冒充。除此之外不因为一个新 tag 重跑未变的性能、旧硬件或所有 ASIO 条件分支，也不新增百万条 benchmark 门槛。

正式包最终仍需白名单、隐私、许可证、版本/提交和启动验收；Windows 安装同步保留正常数据目录、库和偏好，仅复制 package-owned 文件。隔离发布 checkout 中的临时安装不能替代原固定 `local_release/`。成功交付后按 40 GiB debug 预算维护一次缓存，保留 release、SDK、数据及未验证外部缓存；维护阻塞与未满足预算如实登记。

## 待发布资产与后续步骤

远端 v0.9.4 标签与 Release 尚未创建。后续在最终身份和各平台实物验收完成后，推送最终标签，并通过 `tools/release-create.sh <tag>` 创建 `AudioLib Explorer v0.9.4` 正式 Release；不得覆盖 v0.9.3 标签或替换其资产。预计仍为七个归档和七个同名 `.sha256`，精确十四项：

```text
AudioLibExplorer-v0.9.4-win-x64-portable.zip
AudioLibExplorer-v0.9.4-macos-arm64.zip
AudioLibServer-v0.9.4-win-x64.zip
AudioLibServer-v0.9.4-macos-arm64.tar.gz
AudioLibServer-v0.9.4-linux-x64.tar.gz
AudioLibServer-v0.9.4-docker-linux-amd64.tar.gz
AudioLibServer-v0.9.4-docker-linux-arm64.tar.gz
```

七份 SHA256、发布时刻和线上资产身份在实际完成后填写，当前均 pending。发布后再核对 GitHub 标题/标签/非 prerelease/资产集合和 SHA256，并将相同正式 payload 与原固定安装同步；远端发布和本地安装分别报告。可选 GitHub hosted verifier 只检查公开元数据与摘要，不替代本机实际包验收。
