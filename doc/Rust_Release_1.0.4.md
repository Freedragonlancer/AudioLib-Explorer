# AudioLib Explorer 1.0.4 发布记录

已于 2026-11-18 15:03:21 UTC（北京时间 2026-11-18 23:03:21）正式发布 [v1.0.4](https://github.com/Freedragonlancer/AudioLib-Explorer/releases/tag/v1.0.4)。[#337](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/337) 将通过验收的 beta `1e031ca745f50540e8ed612721a3c85442d3b16a` 晋升至 main。最终源码为 `1a80e755aec264132b71b18c455790947b81f220`，tag object 为 `cf1ad4c3074262ee431de8921fec7f2431e47000`。最终 main 的源码树与验收通过的 beta 一致。相对 1.0.3 为 PATCH，API `/api/v1/`、本地写 schema 8、专用服务端写 schema 9、快照读取范围 `3..=9` 和完整资料库格式沿用 1.0.3。

## 功能范围

- [#333](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/333) 增加 OpenAI 官方 Responses 供应商，验证固定官方地址、独立凭据和既有转发协议兼容性。
- [#334](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/334) 在紧凑结果列表中保留文件名、时长和 Waveform，分别反馈检索结果与目录统计，按可见窗口加载列表预览。
- [#335](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/335) 更新 Classic 配色、焦点、任务书签和监听反馈，并压缩音高与速率控件。
- [#336](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/336) 更新程序及 11 个 workspace 锁文件条目，保留原外部依赖闭包。

## 源码与 beta 验收

Windows MSVC Rust 1.97.1、Slint 1.17.1、已核验 ASIO SDK 2.3.4。`41bc2c5` 的格式与桌面 all-targets strict Clippy 通过，17 项定向回归覆盖结果表格和异步 Waveform、目录与结果发布、外观设置、标签操作及 OpenAI 官方入口。三项 ignored 入口在原始日志单列。用 Git 对象比对确认 `6d068b5` 的 AI、远程客户端和转发服务端源码输入一致，复用 63 项 AI、Mac arm64 远程和服务端各 9 项及相应 strict Clippy 结果。数据库管理、音频后端、IPC、依赖许可范围与更新器的行为沿用之前已验收基线。

Mac beta 以 1.0.4 / `1e031ca` 启动，音频根目录读取 1/1。Windows 候选包通过便携验收，两个 EXE 与独立 beta 安装的哈希一致。冒烟 10 passed / 0 failed，完整同步 909,342 条、10,150.7 MiB，159.0 秒，配置根内随机流式读取 20/20 通过。配置根之外共有 828,780 条记录，覆盖范围与上轮一致。

## 正式包验收

| 公开交付物 | 核验结果 |
| --- | --- |
| Windows x64 客户端 / ASIO | 许可证、便携包边界、两个 EXE 身份、CLI 和隔离 GUI 启动通过 |
| Windows x64 服务端 | 无默认 feature。包边界、原生启动、迁移工具与合成 fixture 通过 |
| macOS arm64 客户端 | 原生结构、ad-hoc 签名和 GUI/CLI 独立运行验收通过 |
| macOS arm64 与 Linux x64 服务端 | 无默认 feature。对应原生包启动、合成 fixture 和迁移工具验收通过 |
| Docker linux/amd64、linux/arm64 | 各自消费通过验收的 Linux 原生包，两个离线镜像各 8 项运行验收通过 |

macOS 服务端验收在使用系统链接临时目录时失败，切换至本轮已验证的真实临时目录后通过，沿用原构建与归档。Linux 验收通过后的日志解析已从原成功输出提取正式收据。

Linux arm64 原生服务端为 Docker 内部输入。Linux amd64 包在 ARM64 OrbStack 环境仿真验收。Mac 应用使用 ad-hoc 签名。真实付费 OpenAI 推理与实际 ASIO 设备不在本轮覆盖范围。

## 线上核验与本地安装

GitHub 正式 Release 的 14 项线上资产、上传状态、字节数和 SHA256 均与已验收文件一致，已下载 7 份校验正文复核。归档 SHA256 如下。

| 归档 | SHA256 |
| --- | --- |
| `AudioLibExplorer-v1.0.4-macos-arm64.zip` | `8ecbb715f96316739cb51af0f1b13a81082a3e4bf0cd1362a234ae524a5a492a` |
| `AudioLibExplorer-v1.0.4-win-x64-portable.zip` | `d002e8c8c057ae05395d2e043ff6bf5e43a5d8a4a05d60bec81dac61c707c420` |
| `AudioLibServer-v1.0.4-docker-linux-amd64.tar.gz` | `4026425edd64878dedab8191009adee5e71d02f37c74c46ee0b5c9f12d613c48` |
| `AudioLibServer-v1.0.4-docker-linux-arm64.tar.gz` | `b2e773957f9ca6cfbac9c58f626890c3ff12a16cb7c8c8361d37457bb54ddf65` |
| `AudioLibServer-v1.0.4-linux-x64.tar.gz` | `6c3d8f4c10f12d5b65c2e5f701e1fe66a197fbe4cb35bc61b136dfc1bd7185a1` |
| `AudioLibServer-v1.0.4-macos-arm64.tar.gz` | `2662b70789745b7b5f3a541ceb70cebe91c6b7bf315929184412ddc878b793d8` |
| `AudioLibServer-v1.0.4-win-x64.zip` | `48ade08d68e84c243fd65367eb6607ebc1735e2507ace7f0829048bdd9c5b799` |

固定 `local_release/` 在发布前验收时复制 23 个变化文件。发布后以线上 ZIP 内两个 EXE 的 SHA256 为依据，再次验证 908 个包文件、CLI 1.0.4 和隔离桌面启动，copied=0。正常工作资料库和偏好保留。五个本轮已用且已验证归属的 Cargo target 完成 Intermediate 清理，按逻辑大小移除 38,319,613,285 字节，保留正式包、profile 根二进制和验证证据。
