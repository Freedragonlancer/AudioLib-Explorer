# 1.0.1 发布准备

程序 1.0.1 | API /api/v1/ | 本地写 schema 8 | 服务端迁移 schema 9 | 快照兼容 3..=9

2026-11-14：按用户所称“v1.01”准备规范版本 `v1.0.1`，以已发布的 `v1.0.0` 为基线。整合与选定源码验证由 [#308](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/308) 追踪，当前停留在 beta 发布候选准备阶段。未提升 main、创建标签、公开发布或生成最终身份的正式 payload。下文“复用”保留原执行身份，不代表本轮重新运行。

## 整合来源与范围

整合起点为 beta `08a867f8e59ff87f5dd8ab0a8737739eb11a8f2d`。当前本地功能分支已包含该 beta，并保留 AI 提交 `9ccd51d5f954e1721ff60936a03d3326a32c9ecc`、CLI fixture 修正 `c665ed1`、缓存兼容修正 `d179ff7`；本地未提交的构建与验证流程改动保全为 `a7b796857a62c33bd08686af1581efaa7e45ef10`。这些是来源身份，最终候选提交仍待本轮修正与检查完成后冻结。

| 来源 | 本轮范围 |
| --- | --- |
| `9ccd51d` | Magpie 本机／显式私网网关；AI 请求可经音库服务器转发；未保存供应商草稿、180 秒默认与 600 秒上限、移除本地请求配额；服务器凭据与 API 地址绑定并隔离部分保存后的错配 |
| [#296](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/296)，`fde217a` | Reveal 使用实际音频的绝对路径和 Windows shell 选择接口，拒绝空、相对及不适用的远程路径 |
| [#299](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/299)，`f369a16` | 行收藏立即持久化，保留未提交标签／备注；收藏筛选保留当前目录 |
| [#302](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/302)，`673f925` | #299 的重启与目录筛选补充测试，按依赖顺序整合；不引入本地 accept 包装提交 |
| [#301](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/301)，`cc6dd55` | Windows 更新 helper 早期失败恢复原服务、成功阶段清理与持锁保护；测试源功能禁止用于 release |
| [#298](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/298)，`d3e7e81` | 中英文主页截图及说明 |
| `a7b7968` 与后续本轮工具修正 | 本机固定磁盘 Cargo target、编译临时目录恢复、验证脚本路由和共享／独占租约；成功后清理 owned 编译中间产物并保留正式输出 |

相对 `v1.0.0`，beta 已有但尚未正式发布的功能也计入本候选：#292 的试听标记与失焦停止、#300 的身份缓存与晚到播放取消修正、#297 的 GBK／Shift-JIS WAV INFO／bext 解码，以及 #303–#306 的 Go DeepSeek、主声音／分层 6／8 标签、推荐顶栏自适应、当前模型与失败提示。#290 的日常检查与快速本地交付流程同样属于发布后的新工具范围。

GBK 原分支 `73e0dfe` 已由 #297 squash 应用，main 发布记录亦已有补丁等价提交，不重复整合。旧 Search Metadata v2 功能／设计分支、历史本地功能保全快照及 `accept/*` 联合验收分支保留；它们的旧提交关系不代表需要再次合并或恢复旧实现。

## 兼容性与数据边界

资料库完整格式、普通 schema-8 写者、专用 schema-9 服务端写者及快照 `3..=9` 契约沿用 1.0.0，包含既有 `audio_sources:1`／af1 边界；本候选不新增资料库迁移。AI 为 `/api/v1/ai/models`、`/api/v1/ai/generate` 增加兼容端点，旧服务器不支持时明确报错，不自动改为客户端直连。

AI 转发默认禁用，服务端必须显式允许协议与 Base URL，始终验证音库服务器令牌；供应商密钥仅随单次请求传递，不在服务端持久化。客户端令牌绑定实际 API 地址，部分偏好保存失败后不能转发至旧地址。原始音频、Metadata 与用户标注保持现有保护边界。协议与操作说明见 [AI 智能搜索](Rust_AI_Search.md)、[远程模式](Rust_Remote_Mode.md) 和 [HTTP 契约](api/README.md#12-ai-请求转发)。

## 源码验证与复用身份

本轮使用 Rust 1.97.1 MSVC、Windows x64 和已核验 ASIO SDK 2.3.4，重新执行下列受影响检查。测试源码身份为 `e9250e6`，包含更新锁修正和恢复测试的事件循环同步修正；其后文档收尾不改变应用或测试源码。没有把 GitHub 的格式检查当作功能验收。

| 本轮新执行 | 结果与范围 |
| --- | --- |
| 格式与 strict Clippy | 最终 `cargo fmt --all -- --check`、desktop ASIO、update／windows ASIO／server no-default-features 的 all-targets strict Clippy 通过 |
| 桌面收藏与 Metadata 恢复 | 收藏 6 passed；恢复 8 passed／8 ignored，ignored 为父测试调用的隔离子进程入口，不额外计数。覆盖即时持久化、重启、当前目录、失败回滚、草稿保护与关闭恢复；最终恢复日志位于 `.rust-validation/local-checks/e26665d7ac8e43558db3e1a90e24e492/` |
| 更新器与 Windows 接口 | update 40 passed／2 ignored（父测试调用的私有 helper 入口），使用 debug `test-update-source`；Windows 33 passed，包含 Shell 路径和扫描文件定位回归。新测试验证清理前打开的锁句柄和后续 updater 保持同一锁身份 |
| WAV 编码与直接消费者 | 补齐已在 beta 的 #297 功能证据：audio ASIO／db strict Clippy、两项 GBK／Shift-JIS 解码回归通过；`smv2_em` 为 9 passed／5 ignored，后者需要旧快照或专用迁移 fixture，不计为通过。仅有原 PR 的格式检查不足以替代这些回归，因此没有将其作为稳定基线跳过 |
| 服务端直接消费者 | Windows no-default-features `--test updates` 3 passed；不代替最终服务端包或其他原生平台运行验收 |
| 跨进程数据安全 | 当前源码的 ASIO debug CLI 构建通过；`validate-review-fixes.ps1 -CleanCheckout` 十项通过，重叠扫描 fixture 为 2,000 条。真实旧库只读拒绝／哈希检查按此模式排除，未计为通过；未打开真实音频输出 |
| 工具与发布预检 | PowerShell 语法、`git diff --check`，build-environment／validator-routing／development-pipeline／build-artifacts／release-pipeline All／package-boundary／server-package 七组合成回归通过；版本检查、WindowsClient／Windows Server 许可证预检和两份原生 Plan 预览通过。预览的 source checks 为 `not-requested`，不代替本表实际检查 |

原失败保留在 `.rust-validation/release-101/`：首次 Metadata 筛选名错误匹配零项，检查脚本按约定报错；改正筛选后为 7 passed／1 failed／8 ignored，暴露测试只等待 worker、未等待 Slint 回滚发布的问题。补上事件循环等待，保留并加强收藏、备注和 dirty 状态断言后，重新运行格式、桌面 strict Clippy 和整个恢复模块通过；生产 Controller 未作该项修改。缓存重复 Intermediate 维护也先复现失败，再通过修正后的完整合成回归。不把失败或零匹配记录覆盖成通过。

| 已有证据 | 原始身份与可复用范围 |
| --- | --- |
| AI／Magpie／转发整合 | `9ccd51d`；私有 `.rust-validation/cr-push/final-review-push.json` SHA256 `ee43d615d078180f609440e4db9533c9abc888124c149f7be9c437cafd9c11f8`。Windows ASIO 与 Mac arm64 的 AI、remote、desktop／CLI、server 定向检查及真实 HTTP 转发，具体次数与修复重跑见 [AI 验证记录](Rust_AI_Search.md#代码与验证)；仅在相关源码、feature、依赖和环境未变时复用 |
| AI 转发原验证 | `.rust-validation/ai-relay/final-validation.json` SHA256 `58dfda5c4504b268c1f80622bfaa87ba67cf124d0c7ec3d491c5081bf1928332`，原工作区基于 `6e4ea1d` 并记录逐文件哈希；Windows AI 初次 50 passed／1 failed，修正后六项 relay 重跑通过；另有 remote 9、desktop 控制器 35／UI 3，Mac server relay／migration／auth 33。保留组合证据与原失败，后续凭据修正使用前行 CR 证据，不以旧 receipt 覆盖新代码 |
| 最新 beta 上的 AI 草稿 | `.rust-validation/ai-unsaved-preset/final-validation.json` SHA256 `f44445f2d289f7dc9ae3604511199e0fd0da0a6a8f2ba1c3514acf5419879dc1`；记录 beta `08a867f`、逐文件哈希、44 项后端／33 项控制器／5 项 UI，以及双语实际渲染 |
| Magpie 私网与实际调用 | `.rust-validation/magpie-lan/final-validation.json` SHA256 `539a0134cf843c8f909655a60f59a312cc9666760e1e967cf041ffc17b2c3070`，以及 `magpie-live/final-validation.json` SHA256 `422630c0a88588a89608e9da73f08afbf2e80d8dfeb2643d3ef10ef2b3f59863`；仅保留历史模型发现、生成与出处验证观测。其 config/provider/storage 哈希与当前源码不同，不作为当前代码通过证明，不将单次网络耗时当作资料库性能 |
| 已在 beta 的播放修正 | `b1466e8`／`b4dc2e0`，见 [Beta P2 记录](reviews/Beta_P2_Fixes_2026-11-13.md)：17 项定向回归、最终隔离 playback IPC 20 项通过；原失败与平台限制保留 |
| 未变存储与音频模块 | [1.0.0 发布记录](Rust_Release_1.0.0.md) 的实际 `1fc6f825` payload 与已选源码基线，仅用于未改变的数据库／扫描／音频引擎／IPC 行为。GBK 解码、收藏写入、更新器及本轮受影响调用链分别选择相关检查，不以“稳定”排除它们 |

合并修改了 desktop 共享控制层与 UI、Windows 接口及 update／server feature 闭包，相关静态检查和交互检查需使用整合后的源码。先前 `magpie-delivery` 的 1.0.0 本地试用包不是本候选的最终产物；本轮不得把它改称 1.0.1 或正式 main/tag 构建。

本轮已逐项核对：AI／remote／core／db／audio／IPC Git 树与 `9ccd51d` 相同；Cargo.lock 除 11 个内部版本及 Windows 的数据库测试依赖外没有外部依赖变化。AI 转发收据的六个 AI 模块和所列服务端 Rust 文件哈希匹配，凭据修复由较新的 CR 记录覆盖。旧服务端 AI／鉴权／迁移子系统测试可复用；旧包级构建和 Clippy 不代表本轮 update 依赖闭包通过。

## 编译缓存维护

本轮成功后调用正式清理器，对已验证归属的本机 C 盘 target 执行 `Intermediate -MaxGiB 0`。清理器检测到另一项目的活动 Cargo 测试并返回 `blocked`；未停止进程、未删除编译产物。逻辑空间尚未扫描，回收量为 0，不能声称满足零预算清理。私有结果为 `.rust-validation/release-101/cache-cleanup.json`。原仓库旧 target 与其他主机缓存未额外扫描；SDK、最终输出、安装、工作资料库与源音频均不属本次清理范围。

## 正式产物计划

以下七份公开归档及各自同名 `.sha256` 共十四项资产均待最终 main/tag 身份冻结后构建、打包与验收；目前未生成。独立 Linux arm64 原生包仅供 Docker 输入，不增加公开资产数。

| 公开归档 | 实际构建／验收配置 | 状态 |
| --- | --- | --- |
| `AudioLibExplorer-v1.0.1-win-x64-portable.zip` | Windows MSVC x64，desktop／CLI ASIO，核验 SDK 2.3.4，portable 与固定安装 | 待执行 |
| `AudioLibExplorer-v1.0.1-macos-arm64.zip` | 原生 macOS arm64，CoreAudio，原生启动／Quit；签名状态按实际报告 | 待执行 |
| `AudioLibServer-v1.0.1-win-x64.zip` | Windows x64，no-default-features，server／migrator 与 owned fixture | 待执行 |
| `AudioLibServer-v1.0.1-macos-arm64.tar.gz` | 原生 macOS arm64，no-default-features，双二进制及 health／迁移验收 | 待执行 |
| `AudioLibServer-v1.0.1-linux-x64.tar.gz` | Linux x64，no-default-features；若使用仿真须明确记录 | 待执行 |
| `AudioLibServer-v1.0.1-docker-linux-amd64.tar.gz` | 使用已验收 Linux x64 包，owned 容器与持久化／退出验收 | 待执行 |
| `AudioLibServer-v1.0.1-docker-linux-arm64.tar.gz` | 使用独立已验收 Linux arm64 输入包，同上 | 待执行 |

发布准备不等于公开发布或生产部署授权。后续正式发布须先完成 beta refresh／client smoke 及与当前变更有关的 UI 验收，再经 PR 提升 main；最终产物必须来自冻结的 main/tag，使用原生 release plans 并保留实际收据。Windows 交付完成时更新同一 `local_release/` 并核对包文件身份、CLI 版本和隔离启动，保留原工作数据。本轮准备尚未更新该安装。

真实 ASIO 硬件、声学起音、真实休眠 NAS、原生跨程序 Reveal／收藏点击及最终多平台产物尚无本轮验收结论；此前供应商成功调用不能覆盖所有模型与账户。非 Windows AI 密钥仍仅保留会话；Mac ad-hoc 签名不等于 Apple 公证。未请求新的百万记录性能测量，项目目标仍为 **P95 latency <500ms for 1M+ records**。
