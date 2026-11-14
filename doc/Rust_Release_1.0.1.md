# 1.0.1 发布记录

程序 1.0.1 | API /api/v1/ | 本地写 schema 8 | 服务端迁移 schema 9 | 快照兼容 3..=9

已于 **2026-11-14 11:22:02 UTC（北京时间 2026-11-14 19:22:02 +08:00）** 正式发布 [v1.0.1](https://github.com/Freedragonlancer/AudioLib-Explorer/releases/tag/v1.0.1)，Release ID `445230503`，为稳定版 Latest。七份正式归档来自最终 main／tag commit `21a0e1c2fc5de4a23a10f2139459885373f97ada`；tag object 为 `bf4bcf7d5986c47f3afbb9a0c719c370abccf8c5`，解引用到同一 commit。实际十四项线上资产、摘要和下载校验正文，以及发布后固定安装同步均通过；各平台正式 bytes 的包边界和原生／容器启动验收保留其完成身份。

本版以已发布的 `v1.0.0` 为基线，包含 Magpie／AI 服务端转发、凭据绑定、主声音／分层推荐、收藏持久化、Windows 定位与更新器修正、GBK／Shift-JIS WAV Metadata 和发布工具改进。原始源码检查、失败重跑与复用身份分别保留；文档收尾不移动 tag、不重建／重打包或替换已接受产物。API、schema 和完整格式沿用 1.0.0。

## 整合来源与范围

整合起点为 beta `08a867f8e59ff87f5dd8ab0a8737739eb11a8f2d`；[#308](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/308) 汇入 AI 提交 `9ccd51d5f954e1721ff60936a03d3326a32c9ecc`、CLI fixture 修正 `c665ed1`、缓存兼容修正 `d179ff7` 及保全本地工具改动的 `a7b796857a62c33bd08686af1581efaa7e45ef10`。实际 beta 运行提交为 `a6e7edc`。随后 [#310](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/310) 修正公开示例与隔离验证（beta `0b889a0`），[#311](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/311) 协调 main 的历史文档（beta `ab7da13`），最后由 [#309](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/309) 提升至最终 main `21a0e1c`。

| 来源 | 本轮范围 |
| --- | --- |
| `9ccd51d` | Magpie 本机／显式私网网关；AI 请求可经音库服务器转发；未保存供应商草稿、180 秒默认与 600 秒上限、移除本地请求配额；服务器凭据与 API 地址绑定并隔离部分保存后的错配 |
| [#296](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/296)，`fde217a` | Reveal 使用实际音频的绝对路径和 Windows shell 选择接口，拒绝空、相对及不适用的远程路径 |
| [#299](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/299)，`f369a16` | 行收藏立即持久化，保留未提交标签／备注；收藏筛选保留当前目录 |
| [#302](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/302)，`673f925` | #299 的重启与目录筛选补充测试，按依赖顺序整合；不引入本地 accept 包装提交 |
| [#301](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/301)，`cc6dd55` | Windows 更新 helper 早期失败恢复原服务、成功阶段清理与持锁保护；测试源功能禁止用于 release |
| [#298](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/298)，`d3e7e81` | 中英文主页截图及说明 |
| `a7b7968` 与后续本轮工具修正 | 本机固定磁盘 Cargo target、编译临时目录恢复、验证脚本路由和共享／独占租约；成功后清理 owned 编译中间产物并保留正式输出 |

相对 `v1.0.0`，本版还包含此前 beta 已整合的功能：#292 的试听标记与失焦停止、#300 的身份缓存与晚到播放取消修正、#297 的 GBK／Shift-JIS WAV INFO／bext 解码，以及 #303–#306 的 Go DeepSeek、主声音／分层 6／8 标签、推荐顶栏自适应、当前模型与失败提示。#290 的日常检查与快速本地交付流程同样属于发布后的新工具范围。

GBK 原分支 `73e0dfe` 已由 #297 squash 应用，main 发布记录亦已有补丁等价提交，不重复整合。旧 Search Metadata v2 功能／设计分支、历史本地功能保全快照及 `accept/*` 联合验收分支保留；它们的旧提交关系不代表需要再次合并或恢复旧实现。

## 兼容性与数据边界

资料库完整格式、普通 schema-8 写者、专用 schema-9 服务端写者及快照 `3..=9` 契约沿用 1.0.0，包含既有 `audio_sources:1`／af1 边界；本版不新增资料库迁移。AI 为 `/api/v1/ai/models`、`/api/v1/ai/generate` 增加兼容端点，旧服务器不支持时明确报错，不自动改为客户端直连。

AI 转发默认禁用，服务端必须显式允许协议与 Base URL，始终验证音库服务器令牌；供应商密钥仅随单次请求传递，不在服务端持久化。客户端令牌绑定实际 API 地址，部分偏好保存失败后不能转发至旧地址。原始音频、Metadata 与用户标注保持现有保护边界。协议与操作说明见 [AI 智能搜索](Rust_AI_Search.md)、[远程模式](Rust_Remote_Mode.md) 和 [HTTP 契约](api/README.md#12-ai-请求转发)。

## 源码验证与复用身份

Windows 源码检查使用 Rust 1.97.1 MSVC、Windows x64 和已核验 ASIO SDK 2.3.4。最初测试源码身份为 `e9250e6`，已整合为实际 beta `a6e7edc`；后续文档与 Windows 验证器修正不改变应用 Rust、依赖和运行构建输入，按相关输入等价复用至 `21a0e1c`。没有把 GitHub 格式检查当作功能验收，也不把这些结果描述为正式 main 单次全工作区测试。

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

合并影响 desktop 共享控制层与 UI、Windows 接口及 update／server feature 闭包，相应静态和交互检查使用实际整合源码，并保留上述执行身份。先前 `magpie-delivery` 的 1.0.0 本地试用包没有作为本版正式产物；七份正式归档均从最终 `21a0e1c` 构建，未对旧试用包重标版本。

本轮已逐项核对：AI／remote／core／db／audio／IPC Git 树与 `9ccd51d` 相同；Cargo.lock 除 11 个内部版本及 Windows 的数据库测试依赖外没有外部依赖变化。AI 转发收据的六个 AI 模块和所列服务端 Rust 文件哈希匹配，凭据修复由较新的 CR 记录覆盖。旧服务端 AI／鉴权／迁移子系统测试可复用；旧包级构建和 Clippy 不代表本轮 update 依赖闭包通过。

## Mac 与 Linux 的受影响源码检查

| 平台与实际执行身份 | 新执行结果 | 条件与限制 |
| --- | --- | --- |
| 原生 Mac arm64／`a6e7edc` | updater 37、平台路径 25、编码 2、Metadata 9；受影响 update／audio／db／server headless all-targets strict Clippy 及平台适配 Clippy 通过 | Metadata 5 ignored。路径回归实际对自有临时文件调用 Finder reveal／open，原生返回成功；未目检窗口选中效果，不作完整跨程序视觉通过声明 |
| Linux x64／`0b889a0`，按应用输入等价覆盖 `21a0e1c` | updater 37、编码 2、Metadata 9、server 12；update／server／audio／db headless strict Clippy 通过 | Metadata 5 ignored；ARM64 OrbStack 上执行 x86_64 仿真，不代表物理 x64 硬件 |
| Linux arm64／`0b889a0`，按应用输入等价覆盖 `21a0e1c` | 更新锁定向 4＋1＝5、编码 2、Metadata 9、server 12；相同受影响包 strict Clippy 通过 | Metadata 5 ignored；原生 ARM64 自动更新产品选择不受支持。原 broad updater 为 28 passed／9 failed，错误为 `Automatic updates are unavailable for this platform`；原失败保留，不能写成 ARM64 updater 全套通过。Docker 禁用二进制自更新 |

这些平台结果不跨平台累加成“不同用例总数”；ignored fixture、父测试调用的 helper 与同一用例重跑不重复计数。原始索引为 `.rust-validation/release-101/beta-gate/mac-source-checks.private.json` 和 `linux-delivery/{amd64,arm64}/pretag-source-proof.private.json`，包括实际 commit、功能配置、原失败与最终 receipts。

## Beta 环境验收与覆盖边界

Mac 正式 beta refresh 已完成优雅停止、候选安装、health／版本 1.0.1／schema 9、实际运行提交 `a6e7edc` 和配置根读取检查，保留原回滚备份。Windows 侧完整冒烟 **10／10**：客户端强制同步的 **909,342** 条与服务端一致，**20／20** 个授权音频流、ETag 304 和两个 EXE 哈希检查通过。后续 #310／#311 的文档／验证流程修正复用这一实际二进制门禁身份，没有为文档再部署服务或重新验收相同运行输入。

当前配置只有一个根：范围内 **80,562**、范围外 **828,780**，合计 909,342；前后计数不变，**missing=0**。选样时 210 个根外候选被跳过，作为覆盖限定说明，不计为流读取成功。结果不证明所有库记录均在当前配置根内或都已逐条读取；本轮没有为了验收扩大根、relink 或扫描工作库。完整门禁和前后范围核对索引为 `beta-gate/acceptance-summary.private.json`。生产实例未作为本轮部署目标。

## 真实交付失败与工具恢复

- 首次 Windows 打包隐私检查拒绝公开 AI 示例中的具体私网地址；修正为 `<magpie-host>` 后，使用已通过构建的相同 EXE 正式重新打包。随后 portable 首次运行成功，但永久 `.audiolib-update.lock` 留在原包，使安装前源包隐私复查失败。第二次是交付边界失败，不是应用编译失败。#310 改为先检查原包、逐文件核对隔离副本、从自有副本运行并复查原包不变；真实 portable 和 side-by-side beta 安装重试通过。永久锁与严格 allowlist 未被削弱，未删除活跃安装中的锁。
- Mac Metadata 初次误用 no-default-features，遗漏 scanner/audio 的编译失败保留；选择实际 scanner 功能后为 9 passed／5 ignored。原生收集器 60 秒期限超时也保留；后续以 600 秒只读收集期限读取原成功结果，保留已通过 gate 和二进制身份，没有重复部署、重启或重做同一运行 gate。
- Docker 自有 fixture 的私有 rustc harness 初次缺少正确绑定、host proc-macro 搜索路径和 ThinLTO／bitcode 设置；修正后绑定当前 headless Cargo compiler-artifact 的 core／db，生成并只读核对自有 backup，符合 database／WAV／manifest 三文件契约。原失败及通过 attempts 保留；没有修改产品源码或正式 release plan。

- 首次 publication resume 因原生子进程 PATH 缺少 `pwsh` 失败；仅补齐私有凭据桥接的 PATH，继续调用冻结源码内正式 helper。沿用同一草稿及已上传、已验收的十四个资产，没有重复上传、替换资产或重建产物。
- 发布后收集器最初以大小写敏感方式比较 Linux 十六进制摘要而误报失败。仅修正收集器的十六进制大小写归一，保留并复核原成功 metadata、原始下载校验文件与实际摘要；文件名仍精确匹配，没有重新下载或重做原生验收。最终 UTC 字段为 11:22:02，按 UTC+8 的北京时间为 19:22:02。
上述新阶段与已有恢复测试／缓存合成回归失败分别保留原记录；不能把后续收集成功、修正重跑或 scope 选择回写为此前从未失败。

## 正式产物与 accepted SHA256

以下七份归档及各自同名 `.sha256` 共十四项资产均已构建、打包并对实际最终 bytes 完成对应原生／容器验收，源码均为 `21a0e1c`。独立 Linux arm64 原生包是已验收 Docker 输入，不单独发布。公开上传、Latest／非 prerelease 及远端摘要结论以本记录首段的实际完成状态为准，不把草稿或本地 intended metadata 当作线上通过。

| 归档 | 字节数 | 实际验收配置 |
| --- | ---: | --- |
| `AudioLibExplorer-v1.0.1-win-x64-portable.zip` | 46782782 | Windows MSVC，desktop／CLI ASIO SDK 2.3.4；完整隔离 portable 和固定安装身份／启动 |
| `AudioLibExplorer-v1.0.1-macos-arm64.zip` | 36760948 | 原生 Mac arm64／CoreAudio，组合打包验收 10 项与 native Quit；ad-hoc，未公证 |
| `AudioLibServer-v1.0.1-win-x64.zip` | 18141028 | Windows x64 headless，server／migrator，owned fixture 与启动验收 |
| `AudioLibServer-v1.0.1-macos-arm64.tar.gz` | 15838945 | 原生 Mac arm64 headless，双二进制及 9 项原生包／启动验收；自有进程 SIGTERM 及有界退出 fallback |
| `AudioLibServer-v1.0.1-linux-x64.tar.gz` | 17163746 | Linux x64 headless 原生包；在 ARM64 主机进行 x86_64 仿真验收 |
| `AudioLibServer-v1.0.1-docker-linux-amd64.tar.gz` | 240105341 | 仅消费已验收 Linux x64 包；owned 非 root／只读、卷、流、SIGTERM／restart 检查；x64 仿真 |
| `AudioLibServer-v1.0.1-docker-linux-arm64.tar.gz` | 220883211 | 仅消费已验收内部 Linux arm64 包；相同容器边界与运行验收，ARM64 Linux VM |

```text
7e7df1fa2933cb89f1efe681148862de866b74fd881de2a5c28e5aea475f90ea  AudioLibExplorer-v1.0.1-win-x64-portable.zip
e08ffe0d690dc521153b8a55ed2e519f560c762ff674d9e976114257abd68838  AudioLibExplorer-v1.0.1-macos-arm64.zip
850694b89d007c6df98f82d26fcc8c677c3a8e5ed8c8308f776f3ee659052781  AudioLibServer-v1.0.1-macos-arm64.tar.gz
28131095654e55be309daf538b34c9095df7e8814884cca104ff315429a971ce  AudioLibServer-v1.0.1-linux-x64.tar.gz
c14812b66176cfb13c3440f9dc1c0356c9a6fa9d28f0c23c463b06431560b39b  AudioLibServer-v1.0.1-docker-linux-amd64.tar.gz
6de478017893aa97d09d86368a44e12d4f12c92616fb54af8db38aa284a5b8e1  AudioLibServer-v1.0.1-docker-linux-arm64.tar.gz
6552fc646d32ce989a39528536c1e5189db67e5f2ea342a11d1b6399cb5b487d  AudioLibServer-v1.0.1-win-x64.zip
```

Windows 版本化公开 ZIP 是 accepted latest 归档的相同字节副本，另有匹配公共 basename 的校验文件；原 pipeline latest 归档和 sidecar 保留，未因改名破坏续跑身份。Mac `package-macos-client.ps1 -ValidateRuntime` 已组合结构／签名／运行验收，没有再执行重复 standalone validator。各平台的 source checks 在原执行记录中；正式打包管线未请求的 source stage 仍为 `not-requested`，不改写为新执行 passed。已验收产物索引分别为 `.rust-validation/release-101/windows-final-delivery.private.json`、`mac-final-delivery.private.json`、`linux-delivery/delivery-summary.private.json`。

## 线上身份与固定安装同步

**线上核验通过。** 实际 Release 非 draft、非 prerelease，Latest=true；精确十四项资产的名称大小写、uploaded 状态、大小与 API SHA256 均匹配已验收公共副本。七份下载 sidecar 的原始字节、精确 archive basename 和按大小写归一后的十六进制摘要均一致，说明正文与已审阅 notes 相同；远端 annotated tag `bf4bcf7` 及其 `21a0e1c` 目标复核通过。服务端 archive digests 完整，无需重复下载大型归档。证据为 `.rust-validation/release-101/published-verification.private.json`，不是 intended metadata 或草稿状态证明。

**发布后固定安装同步通过。** 同一个 `local_release/` 全部 **908** 个包拥有文件核验，**copied=0**；两 EXE 哈希同时匹配原 accepted portable 和已验证的线上客户端归档。CLI 返回 `AudioLibCli 1.0.1`，一次显式隔离数据目录启动通过，正常数据环境已恢复。原正式 portable suite 保留，不重复执行；发布后只复核安装文件身份、版本与隔离启动。工作库、Metadata、标注、扫描历史、偏好和源音频保留。证据为 `.rust-validation/release-101/post-published-install.private.json`。

## 编译缓存维护

准备阶段曾因其他项目活动 Cargo 而 blocked，原报告保留为 `.rust-validation/release-101/cache-cleanup.preparation-blocked.json`，未被覆盖成 passed。本轮相关构建、打包和运行验收结束后，各已验证 owned target 仅执行一次正式 `Intermediate -MaxGiB 0` 维护，保留 profile 根最终输出及 markers。结果为逻辑文件字节估算，不是物理磁盘回收量：

| 已验证范围 | 移除逻辑 GiB | 保留逻辑 GiB | 状态 |
| --- | ---: | ---: | --- |
| 本机 Windows 实际使用的 owned target | 40.885 | 1.328 | passed |
| 本轮明确登记的 Mac target | 9.046 | 0.191 | passed |
| 本轮 x64 Linux 容器默认 owned target | 3.512 | 0.207 | passed |
| 本轮 arm64 Linux 容器默认 owned target | 3.385 | 0.193 | passed |

合计移除约 **56.828 GiB**；剩余为保留的 profile 根正式输出／markers，`budget_met` 未用于承诺整个 target 归零。历史、父级、兄弟及其他主机缓存未扫描、未认领；不声明整台机器满足预算。SDK、Cargo／Rustup home、正式工具、dist、安装、原始库和音频保留。证据为 `windows-cache-maintenance.private.json`、`mac-cache-maintenance.private.json` 及 Linux delivery summary；后续纯文档收尾不再次清理。

## 兼容性与实测限制

Mac ad-hoc 不等于 Apple 公证。Linux x64／Docker amd64 是 ARM64 主机上的 x86_64 仿真，ARM64 为 Linux VM，不冒充物理 x64 或所有发行版覆盖。原生 Linux arm64 自动更新产品不受支持，Docker 禁止容器内二进制自更新。真实 Windows 服务生产优雅排空、跨程序视觉选中、真实 ASIO 硬件、声学起音、真实休眠 NAS、所有供应商账户／模型和 Jev 语义阈值校准未新增完整验收；真实旧库专项按缺 fixture 范围保留未测。非 Windows AI 密钥仍仅保留会话。

本版未新增百万记录性能或物理音频延迟测量；**P95 latency <500ms for 1M+ records** 仍为项目目标，不能把单次网络、驱动报告或静音 fixture 结果当作声学测量。文档收尾只同步事实，保持 `v1.0.1` 标签、正式源码 `21a0e1c` 与 accepted 字节不变，不为记录历史而重建、重打包、重装或再扫描缓存。
