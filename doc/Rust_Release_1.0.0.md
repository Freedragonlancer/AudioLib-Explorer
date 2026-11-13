# 1.0.0 发布记录

程序 1.0.0 | API /api/v1/ | 本地写 schema 8 | 专用服务端迁移 schema 9 | 快照兼容 3..=9

已于 2026-11-13 02:17:52 UTC（北京时间 10:17:52）正式发布 [v1.0.0](https://github.com/Freedragonlancer/AudioLib-Explorer/releases/tag/v1.0.0)，Release ID `443726042`；非 draft、非 prerelease，核验时为 latest。七份归档、十四项线上资产及发布后固定安装同步均已核验。

## 来源、范围与兼容性

正式源码与 `v1.0.0` 固定为 main `1fc6f825520bdf24d01e2889dbcc7c96831be04c`，经 [#291](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/291) 晋升；tag object 为 `e62761a1f9a30294184d061354d1216e216b8182`。最终 Git tree 与已验收 beta `b6d1b1c97c6a9ed9c261adbc77537f50edcfda13` 相同，应用实现基线为 `ecbe972b17774aeaabdba413ec60301d42c3e4af`。以 [v0.9.6](Rust_Release_0.9.6.md) 为发布基线，正式标题为 `AudioLib Explorer v1.0.0`、非 prerelease。

新增 AI 关键词／UCS／配方检索、供应商与预设设置、Jev 结构化候选及模式一致性；推荐按实际目录、范围和筛选核验，最多五项，支持独立 tag 新任务和可选自动三任务。播放准备、NAS 首响应、缓冲及关闭恢复反馈分别绑定请求身份；显式取消与真实错误保持区别。另保留 RIFF 八字节父 size 兼容及完整子块／截断校验。细节见 [AI 智能搜索](https://github.com/Freedragonlancer/AudioLib-Explorer/blob/v1.0.0/doc/Rust_AI_Search.md) 和 [状态生命周期审查](reviews/Status_Feedback_Review_2026-11-13.md)。

API、schema 写目标、快照范围及 base／`audio_sources:1`（af1）完整格式沿用 0.9.6；升为 1.0.0 不引入新的资料库迁移。旧客户端不能仅凭相同 schema 数字访问 af1。[#290](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/290) 的流程／工具改动于正式发布后合入，不在本次 tag 中；tag 冻结后经 [#292](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/292) 合入后续 beta `6b9bdf1` 的应用功能也不属于 1.0.0。本次文档 hotfix 从最终 main 派生，不引入这些后续代码。

## beta 门禁与源码检查

| 证据 | 实际身份和结果 |
| --- | --- |
| Mac beta refresh | beta `b6d1b1c`，served commit 相符、health `ok`；配置根读取 1 通过、0 跳过；未使用 `--with-db`，生产服务未动。准备 checkout 的 `.rust-validation/release-v100/mac/beta-gate.private.json` |
| Windows beta smoke | 同一 beta：10 检查通过、0 失败；full sync 909,342 条与服务端相符，20/20 随机 stream 通过。255 个根外候选跳过，不能推断全部历史条目可播放。证据 `.rust-validation/release-100/windows-beta-smoke.private.json`；原 collector 参数拼写修复仅重读成功原日志，未重跑门禁 |
| Windows 原源码基线复用 | `ecbe972`、Rust 1.97.1 MSVC／ASIO：audio、desktop、CLI strict Clippy 和 28 项不同定向回归通过；原首轮错误、共享 fixture 修复与受影响项重跑分别保留。共享 checkout 的 `.rust-validation/status-feedback-review/final-report.json`。原 Jev `8a1212c` 后端 31 项／桌面 AI 26 项基线及数据安全证据按未变边界复用，不改称本轮重跑 |
| 本轮原生 Mac 源码检查 | beta `b6d1b1c`、Rust 1.97.1、`aarch64-apple-darwin`、无 ASIO：格式及 AI／audio／desktop／CLI strict Clippy 通过；AI 后端 31、audio 确认 7、desktop AI 26、desktop status 8 项通过。desktop AI 的 26 个 ignored 和 status 的 3 个 ignored 均为辅助用例，不计通过。证据 `.rust-validation/release-v100/mac/source-checks.private.json` |

源码检查保留原始执行身份，未因 tag 重跑整个 workspace、百万记录或无关数据安全套件。Metadata FIFO／revision／drain、扫描／实例锁、原子偏好存储未改变；原 `ai-delivery/review-fixes.log` 基线继续适用，输出确认／设置关闭链及最新关闭状态另有定向回归。各正式产物仍从最终 main/tag 构建；管线未请求的源码检查如实保留 `source_checks=not-requested`，不写成通过。

## 七份最终 payload

七个公开归档均来自 `1fc6f825`，各带原始同名 `.sha256`，共十四资产。平台许可证、打包边界与实际产物验收均已通过；独立平台并行，共享 target／输出串行，每份实际包仅验收一次。

| Payload | 实际配置、验收与主要收据 |
| --- | --- |
| Windows x64 client | MSVC、desktop／CLI ASIO、SDK 2.3.4；正式 `windows-client` 管线 preflight／licenses／build／package／portable／install 全过。发布前原固定 `local_release/` 的 908 文件、CLI `AudioLibCli 1.0.0` 和隔离启动通过；`windows-client/install.stdout.log` 指向安装证据 `fd2114ecce794c0a9e800123def5c110` |
| macOS arm64 client | CoreAudio、无 ASIO；精确许可证、build proof、合并 package-accept 及 10 项实际原生启动／Quit 验收通过；ad-hoc，未公证。准备 checkout 的 `release-v100/mac/client-delivery.private.json`；实际 runtime 以独立 native acceptance 为准，原结构报告的 `not-run` 字段不覆写为新结果 |
| Windows x64 server | MSVC、no-default-features；server／migrator、打包、独立 release `search-benchmark` fixture 和合成库启动／health／迁移工具验收通过。共享 checkout 的 `release-100/windows-server-delivery.private.json` |
| macOS arm64 server | no-default-features；最终双二进制、许可证及 9 项包验收通过，含 migrator 8/9/8、格式／隐私及 health。自有测试进程 SIGTERM 使用有界终止 fallback。准备 checkout 的 `release-v100/mac/server-delivery.private.json` |
| Linux x64 server | `x86_64-unknown-linux-gnu`、no-default-features；ARM64 OrbStack 上 x86_64 仿真。254 packages／438 notices，build／package／9 项原生包接受检查通过。共享 checkout 的 `release-100/linux-delivery/amd64/native-delivery.private.json` |
| Docker linux/amd64 | 消费上述已验 Linux x64 字节；x86_64 仿真。公开上下文、镜像身份、classic docker-save 归档及 8 项 runtime 检查通过，含 auth／Range／持久化／health／正常 SIGTERM。`release-100/linux-delivery/docker-amd64/docker-delivery.private.json` |
| Docker linux/arm64 | 消费独立已验内部 Linux arm64 包；ARM64 Linux VM／OrbStack。正式镜像／离线归档及 8 项 runtime 检查通过。`release-100/linux-delivery/docker-arm64/docker-delivery.private.json` |

内部 `aarch64-unknown-linux-gnu` 服务端另通过 253 packages／436 notices、独立 build／package／9 项接受检查，SHA256 为 `06c1b7623d5caa1b0c615fdc24019f6cd3df4b8a2461bca5647fba2f1c285ee1`；仅供 arm64 Docker，不是第八份公开归档。Linux／Docker 四条正式接受链由 `release-100/linux-delivery/delivery-summary.private.json` 索引；`publication-staging.private.json` 将三个公开包及原 sidecars 绑定到暂存字节。ARM64 collector 的十六进制大小写比较修复未改原始字节，也未重跑成功阶段。

| 正式归档 | 已验收 SHA256 |
| --- | --- |
| `AudioLibExplorer-v1.0.0-win-x64-portable.zip` | `9bb84c198636c6c40b8d62bdb586921e202286f7311f1c6de1fc21fc82c08731` |
| `AudioLibExplorer-v1.0.0-macos-arm64.zip` | `891029e1acfdd88d75836cf51988046194a67ee8e314517bdefb50d7b7c5743b` |
| `AudioLibServer-v1.0.0-win-x64.zip` | `8c2f49ebfe12c9003a22227981a94ed95f0ff919538bffd90752428199545de3` |
| `AudioLibServer-v1.0.0-macos-arm64.tar.gz` | `249596070b5d612c10508a746085225b562e88205dbdb196431bc7b7b297864a` |
| `AudioLibServer-v1.0.0-linux-x64.tar.gz` | `dc8e08db6e88e589be43743186534ba93d4a7df75c13331a7bd4cc2ef18c5825` |
| `AudioLibServer-v1.0.0-docker-linux-amd64.tar.gz` | `7c37465435d02fc365099df8ce7ce7c1535d8e4546f31869c7e14f8592a7a8a5` |
| `AudioLibServer-v1.0.0-docker-linux-arm64.tar.gz` | `4a801be9431f2a43aff94f88d78cce27328da11d5dfd4e6d31848870a079e196` |

上表为最终实际归档摘要，已与线上 GitHub digest 逐项核对。公开资产仅含七归档及同名校验文件，私有 context／接受报告不上传。

## 发布核验与固定安装

正式发布只读核验通过：精确十四项线上资产的名称、uploaded 状态、大小和 GitHub SHA256 digest 均匹配已验收字节；实际下载七份 `.sha256`，raw bytes、正文文件名及摘要一致。发布说明与审定正文相同，正文 SHA256 为 `4a10b6c6f1e98a3aec997fbe2bfc560b6282fb73639606b9bb053d028c78cdef`；tag object `e62761a1f9a30294184d061354d1216e216b8182` 仍指向固定源码 `1fc6f825`。线上归档 digest 完整，无需再次下载大型归档。证据为 `.rust-validation/release-100/published-verification.private.json`。

发布后使用已证实与线上客户端归档一致的原 portable，同步同一 `local_release/`：908 个包文件及两 EXE 哈希均匹配，copied=0，CLI `AudioLibCli 1.0.0` 和一次隔离桌面启动通过，正常数据环境已恢复；未重复整套 portable。证据为 `.rust-validation/release-100/post-published-install.private.json`，原安装报告 `.rust-validation/local-install/3ed9d42c27ba4967a53d5b05ffe07385/report.json`。工作资料库、Metadata、标注、扫描历史、偏好及源音频未被 fixture 或空库替换。

发布后的文档收尾只同步事实，保持 `v1.0.0` tag、正式二进制源提交和已验收归档不变；不为文档重建、重打包、重装或再次清理缓存。后续 beta 的应用功能保持独立，只同步本次文档提交。

## 缓存范围与覆盖限制

全部本机 Rust 交付完成后各执行一次正式 Debug 维护。Windows 范围为原仓库 `target/`（project-target），逻辑估算约 **37.11 GiB／40 GiB**、removed=0；报告为 Windows checkout 的 `.rust-validation/release-100/windows-cache-maintenance.private.json`。Mac 仅维护已登记的 `target/aarch64-apple-darwin` 子目录，约 **18.819 GiB／40 GiB**、removed=0；报告 `.rust-validation/release-v100/mac/cache-maintenance.private.json`。两者预算达标且 release 输出保留。这些是逻辑文件字节估算，不是实际磁盘回收量。

Mac 父级 host profiles、beta target 及历史／Linux 外部缓存未取得相应所有权或真实 Cargo marker，保持未扫描、未认领、未删除；不声明这些范围满足预算。Linux 两个配置缓存的活动容器数均为 0，未进行额外清理。SDK、Cargo／Rustup、dist、安装、用户库、Metadata、标注、扫描历史、偏好及源音频保留；生产部署未执行。

Mac ad-hoc 不等于 Apple 公证；Linux x64／Docker amd64 仿真不代表物理 x64 硬件覆盖。真实供应商账户、Jev 语义准确率／阈值校准、真实休眠 NAS、真实 ASIO 设备、听感及按钮到实际输出的声学起音延迟未新增验收；非 Windows AI 密钥仍仅保留会话。Windows server 的合成启动不证明生产服务优雅排空。**P95 latency <500ms for 1M+ records** 仍为项目目标，本次没有新增百万记录性能或物理音频延迟测量。
