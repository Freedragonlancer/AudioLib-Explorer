# 0.9.6 发布记录

程序 0.9.6 | API /api/v1/ | 本地写 schema 8 | 服务端迁移 schema 9 | 快照兼容 3..=9

已于 2026-11-11 07:40:41 UTC（北京时间 15:40:41）正式发布 [v0.9.6](https://github.com/Freedragonlancer/AudioLib-Explorer/releases/tag/v0.9.6)。正式二进制源码固定为 main/tag `460629144d6a0df24af9c69303c9409c38efbde2`。以已发布的 [v0.9.5](https://github.com/Freedragonlancer/AudioLib-Explorer/releases/tag/v0.9.5) 为基线使用 PATCH；历史标签、资产和验证身份保持原样。

## 范围与兼容性

- [#271](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/271)：可选分页，每页 1–1000 条、默认 200；原设置仍使用连续浏览的 200 条分段和最多 600 行窗口。分页与连续浏览切换保留搜索、筛选、排序、记录身份与 Metadata 草稿。
- 三个主题持久保存音效库侧栏显隐，保留隐藏前的宽度；修正恢复卡的鼠标点击热区。
- [#272](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/272)：每个任务独立保留最多 20 条会话内搜索历史，包含当前搜索。后退/前进恢复搜索条件与视图，新搜索替换前进分支；历史不跨重启保存。
- 字段范围移至音效库菜单，更新 SVG 导航按钮并仅保留一个原生清空 X。收藏历史修正避免未提交草稿污染先前成功搜索的侧栏状态。
- 发布助手新增显式 `--draft`；正常正式发布与预发布规则保持原样。

API、schema 写目标、快照读取范围与完整格式目录相对 0.9.5 不变。base 格式仍为 `ale-s8-cld1-t3-m2-p1-a1` / `ale-s9-cld1-t3-m2-p1-a1-sd1`；af1 格式仍为 `ale-s8-cld1-t3-m2-p1-a1-af1` / `ale-s9-cld1-t3-m2-p1-a1-sd1-af1`，对应 `audio_sources:1`。旧客户端不能仅凭 schema 数字相同访问 af1，已有升级、备份与降级边界继续有效。

## 源码与晋升身份

| 项目 | 身份或结果 |
| --- | --- |
| 已验收功能 beta | `63841572a471f73d4d006b503e48191d4f0b63a0` |
| 功能晋升 | [#273](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/273) → main `f5ece688ef26e2c6c5dcb1699b8edff014ff2de6`，Git tree 与已验收 beta 相同 |
| 版本准备 | [#274](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/274) → beta `e8a16aa1544567c0570fbdd0dfa767aab6c8d6c4`，仅升版和文档 |
| 最终 main 与正式 tag | [#275](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/275) → `460629144d6a0df24af9c69303c9409c38efbde2` / `v0.9.6` |
| 必需检查 | 两个升版 PR 与最终 main 的 `Linux (fmt)` 通过 |
| GitHub Release | [AudioLib Explorer v0.9.6](https://github.com/Freedragonlancer/AudioLib-Explorer/releases/tag/v0.9.6)，Release ID `441346705`；非 draft、非 prerelease，核验时为 latest |

## 复用源码证据与本轮执行

源码检查保留其原始源码、feature、工具链与环境身份，未因版本或标签重复完整 Cargo 套件。原 Windows 环境为 Rust 1.97.1 MSVC，desktop/CLI ASIO；相关实现与已验输入一致。正式包另从上述最终 main/tag 构建，beta 预编译包不作为正式 payload。

| 原证据 | 原执行身份与结果 | 本轮复用范围 |
| --- | --- | --- |
| `pr185-completion/integration-state.private.json` | tested `67a1a0f73eba8234abf28fd097105d82eaee616a` → beta `8756a69efd8b52c9826edd682080fc582b782b44`；原检查有 808 通过项及 1 个恢复卡失败；修复后相关 25 项 UI、10 项 DB/workspace 和严格 Clippy 等重跑通过 | 分页、配置/布局持久化、恢复卡指针行为；Windows Bash 与原生 Mac Bash 3.2 的发布 shell 证据 |
| `search-history-beta/review-and-validation.json` | reviewed `b62e4f65483946e5617d080a894832d20453a1e0`，与 beta `6384157` tree 相同；原定向检查 96 通过项及 1 个混用 mock/system clock 的测试失败；修正后收藏精确回归和导航 UI 回归通过 | 搜索任务历史、收藏侧栏状态、导航 UI；格式与 desktop ASIO 严格 Clippy 已通过 |
| `promotion-096/acceptance.private.json` | 上述功能 beta 两端门禁通过，main tree 一致 | Mac beta refresh、Windows beta smoke 和相关 Classic/Studio 指针及截图验收；不冒充正式包验收 |

原来两组混合结果没有改写为一次全绿套件。正式原生管线的 `source_checks=not-requested` 如实保留，上述复用证据单独记账。本轮新增运行版本/许可证预检、最终二进制构建、打包及其实际 payload 验收；只对相关安装路由做私有适配与静态检查，未改应用源码。

发布驱动的 Windows host/toolchain 预检与 Mac 校验摘要大小写比较曾分别阻止对应阶段；修正环境或报告比较后保留既有成功产物继续。Mac 官方大写校验正文保持原字节，未重复构建或包验收；安装报告读取修正如后文记录。这些是驱动/环境恢复，不描述为应用功能测试失败。

Mac beta refresh 的 served commit 为 `6384157`，health `ok`，从每个配置根读取 4 KiB、零跳过；没有替换 beta 数据库或部署生产服务。Windows beta smoke 为 10 项计分检查通过、20/20 stream，通过 full sync 的 909,342 条记录与服务端相同。抽样跳过 229 个根外历史候选：当前已验配置根有 80,562 条、在线且 missing=0；另 828,780 条历史记录仍在完整 library/snapshot 中。stream 验收覆盖配置根，不能推断根外记录可播放，也不是数据删除。

## 最终 payload 与验收

七个公开归档各附 `.sha256`，共十四项资产；内部 Linux arm64 原生服务端不作为第八个公开归档。各原生计划在构建前验证对应 target/features 的精确依赖许可证。独立平台并行，共享 Cargo target、打包输出串行；Linux 两架构使用独立 RunDirectory。

| Payload | 实际配置与环境 | 验收结果 |
| --- | --- | --- |
| Windows x64 客户端 | `x86_64-pc-windows-msvc`，desktop/CLI ASIO，已验 SDK 2.3.4；原生 Windows | 许可证预检、ASIO 构建、打包、一次 portable 运行验收与原目录固定安装通过；907 个包文件核对，CLI 0.9.6 和隔离启动通过 |
| macOS arm64 客户端 | `aarch64-apple-darwin`，no-default-features，CoreAudio；已登录桌面，ad-hoc | 精确客户端许可证、构建证明与一次合并打包/签名/原生运行验收、原生 Quit 通过；ad-hoc、未公证 |
| Windows x64 服务端 | `x86_64-pc-windows-msvc`，no-default-features；原生 Windows | license-preflight、server/migrator、打包、显式 release fixture helper、合成库启动/health 与迁移工具验收通过 |
| macOS arm64 服务端 | `aarch64-apple-darwin`，no-default-features；原生 Mac | 最终双二进制、许可证与 9 项包验收通过，含身份/隐私、旧更新器拒绝、migrator 8/9/8 与隔离 health/library；自有测试进程 SIGTERM 使用有界终止 fallback |
| Linux x64 服务端 | `x86_64-unknown-linux-gnu`，no-default-features；ARM64 OrbStack 上 x86_64 仿真 | 许可证、最终 server/migrator、归档结构/隐私与合成库原生包启动、health、迁移工具验收通过；x86_64 仿真 |
| Docker linux/amd64 | 消费已验收 Linux x64 包；ARM64 主机上 x86_64 仿真 | 从已验收 Linux x64 包构建，镜像/公开上下文/离线归档及容器 auth、stream、持久化、health、shutdown 通过；x86_64 仿真 |
| Docker linux/arm64 | 消费已验收内部 Linux arm64 包；ARM64 OrbStack Linux 环境 | 从已验收内部原生包构建，正式镜像/离线归档及实际容器运行验收通过 |
| 内部 Linux arm64 服务端 | `aarch64-unknown-linux-gnu`，no-default-features；ARM64 OrbStack Linux 环境 | 独立构建、打包、合成库启动/health 与迁移工具验收通过；仅供 arm64 Docker 输入 |

Windows 客户端本机包名仍为 `AudioLibExplorer-latest-win-x64-portable.zip`，公开资产名为下表版本名，二者必须为同一已验收字节。Mac 客户端用一次 `package-macos-client.ps1 -ValidateRuntime` 合并结构与原生运行验收；不追加未改变产物的独立完整验收。

| 正式归档 | SHA256 |
| --- | --- |
| `AudioLibExplorer-v0.9.6-win-x64-portable.zip` | `c393f5bba9438c2f980a9e56bc0d4490c46ab585ba20c2f22737b48d9bd5de07` |
| `AudioLibExplorer-v0.9.6-macos-arm64.zip` | `29f73332d0e10dd09bb6367bcdfd839c42073ff1ea2d9a0f18c3657f900d488b` |
| `AudioLibServer-v0.9.6-win-x64.zip` | `df3bd90eadf2a054708b423245e0247640efc8a6cf6fadea4ac37bafd3a65fd2` |
| `AudioLibServer-v0.9.6-macos-arm64.tar.gz` | `1039d6bcfb3fc08d59018c2b5232a695fe27add41c0e5b271232b7c080100b6b` |
| `AudioLibServer-v0.9.6-linux-x64.tar.gz` | `b0f3a58a8ebe6bd2671c4f6dfdeb6e2518f7b99d20f37d9e06c41a1f057a6c4a` |
| `AudioLibServer-v0.9.6-docker-linux-amd64.tar.gz` | `5add835dee8c6884988d08c8ee674f66e623dab2d7c91651a1193bf773d639c4` |
| `AudioLibServer-v0.9.6-docker-linux-arm64.tar.gz` | `a28ef0b134dba52f9727fd018e5236b8fc0ce63c3871d1ee16c0e9a9f27617f0` |

## 发布核验、固定安装与缓存

发布后只读核验通过：精确十四项线上资产的名称、大小写、状态、size 与 GitHub SHA256 digest 均匹配已验收字节；实际下载七份校验文件，raw bytes、正文文件名与摘要全部一致。十四项服务端 digest 均匹配，因此无需归档下载 fallback。Release 正文与已审定正文完全相同，正文 SHA256 为 `944e86da7c6bbb74f568d24141fbdd757402dc8bacc6ca1cc793f75ea3a6958d`。tag object `78fe6aed8969fd0847e3fe597bdbc716dd9fc401` → `4606291` 在核验前后不变。私有证据为 `release-096/post-published-release.private.json`。

发布前已将最终 Windows 客户端安装到原仓库固定 `local_release/`：verified=907、copied=18，CLI `AudioLibCli 0.9.6` 与一次隔离桌面启动通过。发布后再次核对同一固定安装的全部 907 个包文件及两 EXE，均与已验收 portable 和线上客户端归档匹配；copied=0，CLI `AudioLibCli 0.9.6` 与隔离启动再次通过，正常数据环境已恢复。归档摘要为上表 `c393f5bb…9bd5de07`，与线上 digest 相同，未重复整套 portable 验收。安装报告驱动的 `-Tail1` 拼写修正为 `-Tail 1` 后只重读实际成功日志，未重跑安装。私有证据为 `release-096/post-published-install.private.json`。

固定安装保留原仓库 `local_release/`。原工作区有先前未提交改动，源码与 manifest 不为安装临时修改；私有安装阶段仅把安装根路由到原目录、把版本依据绑定最终 clean checkout，原安装器的包白名单、路径约束、优雅关闭、文件哈希、CLI 版本与隔离启动逻辑保持。安装脚本和助手先与最终 Git 源码核对，改动只限四处路由语句；原生计划其余阶段不变，输入与输出身份由同一管线保留。

交付和共享构建结束后，Windows 与 Mac 分别只对本任务已核实登记的子 target 执行一次 Debug 40 GiB 预算维护。Windows 剩余逻辑估算 **2.485 GiB**，Mac 为 **4.561 GiB**，两端 removed=0 B、预算均满足；各自四个 release 二进制摘要仍与已验收包匹配。私有证据为 `release-096/windows-cache-maintenance.private.json` 和 `mac-cache-maintenance.private.json`。父级 host profiles、历史/旧系统盘及未登记 Linux 缓存保留、未扫描；Linux 缓存预算未核实。上述数值为逻辑文件字节估算，不是磁盘空闲空间回收实测。SDK、Cargo/Rustup、dist、安装、源音频和诊断证据保留。

正常数据目录、用户库、Metadata、标注、扫描历史及偏好保留；隔离启动使用自有临时 `AUDIOLIB_DATA_DIR` 并恢复环境。生产服务部署是独立动作，本任务未请求。发布后的文档收尾只同步事实，不移动 tag、不重建已验收字节，不改变二进制源码身份。

## 覆盖限制

Mac 客户端 ad-hoc 签名不是 Apple 公证。Linux x64 / Docker amd64 在 ARM64 主机以 x86_64 仿真执行，不能据此声称物理 x64 Linux 硬件覆盖。Windows server 包的合成启动/health 不证明真实服务的优雅排空。

本版未新增真实 ASIO 硬件、听感盲测或按钮到模拟输出的起音测量，也未重新进行 Explorer/Finder/真实 DAW 跨程序拖出验收。相关历史覆盖限制继续有效。**P95 latency <500ms for 1M+ records** 仍为项目目标，本版没有新的百万记录基准或物理音频延迟实测。
