# 0.9.5 发布准备

程序 0.9.5 | API /api/v1/ | 本地写 schema 8 | 服务端迁移 schema 9 | 快照兼容 3..=9

2026-11-10 候选准备快照。本版以已发布 [v0.9.4](https://github.com/Freedragonlancer/AudioLib-Explorer/releases/tag/v0.9.4) 为基线，计划 PATCH 发布。当前记录的是准备范围及既有验证证据，尚未完成本轮 beta 提升、创建正式标签、构建最终 payload 或发布资产。本轮 Windows/Mac 所选许可证预检与原生计划预览已通过，初始私有证据保存于 `.rust-validation/release-095/`；这不是编译或运行验收。后续阶段须以真实收据更新本记录，不能把下列计划或历史通过结果写成新执行的验收。

## 范围与兼容性

- 统一平台菜单与全局设置、View / 视图下的主题及语言切换、侧栏入口和有界工具栏提示；保留既有偏好。
- 紧凑搜索任务标签采用关键词优先、目录按剩余宽度显示，完整条件仍可悬停查看。
- Waveform 按原素材采样帧框选、反向框选和端点调整；区间试听、Stop、结束后重播与循环共用实际源身份和播放顺序。
- 选区后台准备并拖出原素材 WAV 切片，保留原采样率、全源声道及对应精度。输出不应用试听变换，不覆盖已有编号文件；只保留目标接受的交付文件，清理只限验证过的自有临时文件。
- 远程显示、试听表示及导出原件通过版本凭据绑定；原件无法证明或已变化时拒绝使用旧表示，原件不可取得时不退回低质量预览。
- Waveform 操作光标、系统 DPI 栅格化和准备反馈；SoundTouch 原生对象配对构造/析构；省略 panic payload、配置与运行参数的同步 Rust panic 记录。
- 构建前按实际产品、目标和 feature 精确预检依赖许可证材料；保留打包与运行验收的独立检查。

源音频、资料库 Metadata 和用户标注保持原有生命周期。切片只重建必要音频结构，不复制描述、封面、cue、循环标记、BEXT、iXML 或未知厂商块，避免错误时间引用及附带原嵌入信息。详见 [选区与切片契约](Rust_Spectral_Audition.md#waveform-选区与-wav-切片)。

API、schema 写目标、快照读取范围与完整格式目录相对 0.9.4 不变。base 格式仍为 `ale-s8-cld1-t3-m2-p1-a1` / `ale-s9-cld1-t3-m2-p1-a1-sd1`；已支持的 af1 格式仍为 `ale-s8-cld1-t3-m2-p1-a1-af1` / `ale-s9-cld1-t3-m2-p1-a1-sd1-af1`，对应 `audio_sources:1`。旧客户端不能因为 schema 数字相同就访问 af1；0.9.4 的升级、备份与降级边界继续有效。

## 候选与发布身份

| 项目 | 当前身份或状态 |
| --- | --- |
| 已发布基线 | `v0.9.4`，main/tag `f6adb2bd777d4db78d43cbdcee626153592c1482`；既有标签和资产不变 |
| 已审计功能 beta | `a3c0a169625685dc309ed9d39e146272b1279efe`；[#261](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/261)、[#262](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/262)、[#263](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/263)、[#264](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/264)、[#265](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/265)、[#266](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/266) 已合入 |
| 其他分支整合 | pending；依据实际差异和工作树审计决定，旧分支或等价已合入提交不直接重放 |
| 版本准备 | 0.9.5；版本、锁文件、说明与 `check-release.ps1 -Tag v0.9.5 -BaseTag v0.9.4` 待统一确认 |
| beta refresh / Windows smoke | pending；须匹配冻结后的 beta 候选提交及实际客户端哈希 |
| 提升 main / 正式 tag | pending；通过适用 beta 门禁后以 PR 提升，最终 `main` 与 `v0.9.5` 身份尚未冻结 |
| 七份最终 payload | pending；只能来自最终干净 main/tag，旧 beta 二进制不能代替 |
| GitHub Release / 十四项资产 | pending；尚未创建或上传本版正式资产 |
| 固定 `local_release/` | pending；最终 portable 验收通过后安装并核对身份，发布后再核对其与线上 payload 一致 |

## 已核对的源码验证证据

以下均为复用的历史执行结果，原 base、局部输入清单和二进制身份保留在本机私有 `.rust-validation/`，不作为公开发布资产。2026-11-10 审计确认 `beta-landing-094/accepted-source.json` 与 `waveform-cursor-fine/final-source.json` 各 30/30 输入匹配上述 a3 候选；版本号或说明变化本身不要求重跑未变业务源码。

| 原执行证据 | 原源码与配置身份 | 结果及复用范围 |
| --- | --- | --- |
| `wave-selection-refinement/workspace-result.json`、`initial-check-inputs.json` | `4a1f01b3d8d7795de84d24f87585f2a535c428f0` 加当时记录的局部输入；Rust 1.97.1、Windows x64 MSVC、desktop/CLI ASIO | 全 workspace 严格 Clippy、2396 tests、0 failed；覆盖 SoundTouch 依赖接线后的业务实现。其后改变的 desktop UI 由后述专项覆盖，不声称此历史完整套件针对最终 a3 再执行 |
| `wave-selection-refinement/diagnostic-tests.log`、`diagnostic-clippy.log` | 同一基线加 panic 诊断输入 | 同步 panic 记录的隔离 abort 回归及严格 Clippy 通过；不记录 panic payload |
| `waveform-cursor-scale/checks.json`、`final-source.json` | 同一基线加光标及 UI 局部输入，desktop/all-targets/ASIO | 严格 Clippy、93 Waveform tests 通过；语义缩放 100/150/200/300%，视觉 100/150/200%，系统 DPI 栅格化 100/125/150/200/300% |
| `waveform-cursor-fine/checks.json`、`final-source.json` | 已交付候选 `0.9.4-0+local.c586d85b5cbe`，最终 30 项输入 | 后续仅两个选区 SVG 线宽与说明变化，保留上述源码检查，补充矢量尺寸、透明边界、热点及深浅背景观察；与 a3 内容一致 |
| `menu-native-followup/result.json` | `b506809e0124a9030f15185ad7d4ca8673c48967` 加当时菜单改动 | 所选菜单、焦点、主题、双语和客户端交互回归通过；Windows 菜单已有用户观察确认。保持原证据，后续未改变该菜单实现 |
| `menu-global-settings/review-fixes.log` | 当时已验收菜单/设置候选；隔离合成数据 | 相关配置继承、实例隔离、扫描锁与 Metadata 数据安全回归通过；不沿用被取代包作为本轮正式安装证明 |
| `wave-selection-refinement/playback-ipc.log`、`desktop-close.log` | 当时候选 `0.9.4-0+local.6219178f10c7` | 真实播放/IPC 与关闭排空通过；后续仅光标/UI 改动时复用未变相关行为，最终包启动验收仍待执行 |
| `wave-selection-refinement/result.json` | 同一任务的实际包和脚本输入 | SoundTouch native-only 包边界及缺失原生材料拒绝回归、portable 通过；不能代替 0.9.5 最终归档验收 |

许可证预检与发布计划的隔离合成回归已有 2026-11-09 结果，见 [管线工具验证](Rust_Release_Pipeline.md#发布缓存和工具验证)。只有其相关脚本或规则继续变化时才追加对应回归；不因新增 tag 重复完整应用测试。后续整合若改变上述源码或配置，重新选择失效部分的检查。

## 最终 payload 矩阵

每份正式归档另附 `.sha256`，共七归档、十四资产。Linux arm64 服务端仅为 Docker 内部输入，不作为第八个公开归档。下列运行目录为本轮计划位置，尚无最终构建、验收或发布收据；主机与路径在本机私有配置中核实，不写入服务地址或凭据。

| Payload / 目标 | Feature 与主机要求 | 计划运行目录或入口 | 当前状态与剩余门禁 |
| --- | --- | --- | --- |
| Windows x64 客户端 / `x86_64-pc-windows-msvc` | desktop/CLI ASIO，已验证 SDK 2.3.4；Windows | `.rust-validation/release-pipeline/v0.9.5/windows-client/`；`windows-client.json` | pending；license-preflight → build → package → portable → 固定安装 |
| macOS arm64 客户端 / `aarch64-apple-darwin` | 原生 CoreAudio，ad-hoc 签名；已登录桌面的 Mac | `.rust-validation/release-pipeline/v0.9.5/macos-client/`；`macos-client.json` | pending；精确 client 许可证、构建证明、合并打包/签名/原生运行验收及相关交互 |
| Windows x64 服务端 / `x86_64-pc-windows-msvc` | no-default-features，无播放；Windows | `.rust-validation/release-pipeline/v0.9.5/windows-server/`；`server.json` | pending；许可证、server/migrator、打包、显式 fixture helper、包启动验收 |
| macOS arm64 服务端 / `aarch64-apple-darwin` | no-default-features；原生 Mac | `.rust-validation/release-pipeline/v0.9.5/macos-server/`；`server-unix.json` | pending；最终双二进制、许可证、包结构/隐私/health |
| Linux x64 服务端 / `x86_64-unknown-linux-gnu` | no-default-features；匹配 Linux 环境，仿真需明示 | `.rust-validation/release-pipeline/v0.9.5/linux-x64-server/`；`server-unix.json` | pending；独立 target/output/run，许可证、包与实际启动验收 |
| Docker linux/amd64 | 已验收 Linux x64 公开包；Docker | 独立私有 Docker 运行目录；`package-server-docker.ps1` / `validate-server-docker.ps1` | pending；上游包身份、镜像隔离/health、离线归档及摘要 |
| Docker linux/arm64 | 已验收 Linux arm64 内部包；Docker | 独立私有 Docker 运行目录；同上正式入口 | pending；内部包身份、镜像隔离/health、离线归档及摘要 |
| 内部 Linux arm64 服务端 / `aarch64-unknown-linux-gnu` | no-default-features；匹配 Linux arm64 环境 | `.rust-validation/release-pipeline/v0.9.5/linux-arm64-server/`；`server-unix.json` | pending；与 x64 分离输出和运行目录，验收后仅供 arm64 Docker 使用 |

Windows 客户端发布资产名为 `AudioLibExplorer-v0.9.5-win-x64-portable.zip`；本机打包器继续生成 `AudioLibExplorer-latest-win-x64-portable.zip`。其余正式名称为 `AudioLibExplorer-v0.9.5-macos-arm64.zip`、`AudioLibServer-v0.9.5-win-x64.zip`、`AudioLibServer-v0.9.5-macos-arm64.tar.gz`、`AudioLibServer-v0.9.5-linux-x64.tar.gz`、`AudioLibServer-v0.9.5-docker-linux-amd64.tar.gz`、`AudioLibServer-v0.9.5-docker-linux-arm64.tar.gz`。

## 未完成门禁与覆盖限制

1. 确认本地未提交与未合入分支的最终整合范围，冻结候选；完成版本预检、diff 检查及因新整合而失效的源码检查。
2. 按 [Beta testing flow](../AGENTS.md#beta-testing-flow) 完成匹配候选的 Mac beta refresh 与 Windows smoke，按本版实际菜单和 Waveform 风险补充交互/视觉验收。失败保持失败，不能因历史测试通过而提升。
3. PR 提升 beta 至 main，冻结正式 main/tag，按上述原生计划逐份构建、打包和验收；共享 target/输出目录串行，独立主机可并行。
4. 以正式 `release-create.sh` 入口发布完整十四项资产，核对标签、标题、prerelease、资产集合、size 和 SHA256；更新同一 `local_release/` 并核对线上身份，保留正常数据与偏好。
5. 所有共享构建结束且交付成功后，集中执行一次拥有缓存的预算维护；失败保留诊断缓存，不重复清理或删除未验证的外部缓存。

已有 Windows 证据不能代替 macOS 原生运行或拖放。Explorer/Finder/真实 DAW 接收、释放鼠标后的文件可读性及长期引用尚未通过，本轮不得虚报。Linux 桌面原生拖出当前明确不支持。Mac 客户端继续为 ad-hoc 签名，未经 Apple 公证。

此前有单次隔离 WASAPI `RPC_E_CHANGED_MODE`，干净重启后两段选区均正常结束，尚未建立原因；原先默认参数间歇直接退出的问题后续未再复现，不能据 SoundTouch 析构修正宣称已找到该退出根因。相关历史失败与观察保留在私有证据中。

本轮尚无真实 ASIO 硬件、听感盲测或按钮到模拟输出的起音测量。P95 latency <500ms for 1M+ records 仍是项目目标，本版未改搜索性能路径，不据此重复百万记录基准或宣称新的性能实测。
