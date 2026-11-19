# AudioLib Explorer 1.0.6 发布记录

正式版于 2026-11-19 02:19:43 UTC（北京时间 2026-11-19 10:19:43）发布。通过验收的 beta `b95c0f30a976bd64176ebd564897359b55bf1d72` 经晋升合并至 main，正式归档源码为 `f28e61a5de386ffe684c1b912f2f4621ed19c39d`，标签对象为 `fbd23e2eb8c9d01630516d29d8a31b83abd370a4`。API `/api/v1/`、本地写 schema 8、服务端写 schema 9、快照 `3..=9` 及完整资料库格式沿用 1.0.5。 [正式发布](https://github.com/Freedragonlancer/AudioLib-Explorer/releases/tag/v1.0.6) [晋升 PR](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/352)

## 变更和验收

- #349 将拖动中的 Waveform 提示约束在状态区域，同时显示选区时长和音频格式。沿用 `4297a42` 的 Windows 桌面严格 Clippy、19 项定向测试和实际界面截图。
- #350 新增青碧经典配色。沿用 `3b60258` 的核心及桌面 ASIO 严格 Clippy、7 项桌面与 1 项核心定向测试，覆盖中英文、100% 与 200% 外观、主题保存及 Waveform 取消选区。发布候选的应用、库和服务端源码与该提交一致。
- #348 稳定 Windows 路径映射参数等长时的排序。#351 在 Unix 服务端 release 构建中加入私有 `search-benchmark`，实际验收直接调用这个工具生成两个独立合成库。通过 PowerShell / JSON 语法、原生计划预览和合成构建及验证路由回归。

Windows Beta 目录打包 19.8 秒，便携验收 17.2 秒，隔离安装 8.3 秒。从其两个已校验 EXE 完成远端冒烟 10/10，涵盖完整同步 909,342 条、10,150.7 MiB（144.4 秒）以及配置根内流式读取 20/20。共有 828,780 条记录位于流式配置根之外，覆盖范围与 1.0.5 相同。

Mac Beta 服务端由已验收的本机候选刷新，健康信息为 1.0.6 / `b95c0f3`，配置根读取 1/1，部署的服务端和迁移器 SHA256 均与候选一致。构建 74.9 秒，打包 12.9 秒，原生验收 6.8 秒。

Windows 和 macOS 客户端通过实际正式包的隔离启动验收。Windows x64、Mac arm64、Linux x64 和内部 Linux arm64 服务端均通过原生包验收。两份 Docker 使用对应 Linux 原生验收收据，各通过 8 项离线运行检查。Linux x64 在 ARM64 OrbStack 上仿真运行。Mac 客户端采用 ad-hoc 签名。

## 构建缓存与发布耗时

Windows 与 macOS 继续使用固定 checkout 和已登记的 target，串行完成同一 target 上的客户端与服务端后按 8 GiB 预算统一检查。Windows 保留 4,717,840,755 字节（4.39 GiB），macOS 保留 6,677,802,250 字节（6.22 GiB）。两端均符合预算，本轮移除 0 字节。Linux 使用独立本机构建容器，最后在两个已验证的目标内共移除 1,480,861,033 字节的中间产物。

Windows Beta 客户端构建 1,060.5 秒，10 条编译记录均为工作区 crate。正式标签的同路径构建 1.4 秒，Cargo 0.47 秒，无重编译和指纹失效，两个 EXE 和已验收 Beta 的哈希一致。上轮正式 Windows 客户端构建为 1,097.6 秒、339 条编译记录。此次正式包另用 22.6 秒打包、17.7 秒通过便携验收。Windows 服务端独立构建验收夹具仍耗 56.2 秒，重复编译四个工作区 crate。

Unix 对比使用正式服务端「构建 + 原生验收」的合计耗时。工具的编译计入构建，新旧版本的缓存状态和并行负载可能不同。

| 平台 | 1.0.5 构建 + 验收 | 1.0.6 构建 + 验收 |
| --- | ---: | ---: |
| Mac arm64 | 160.4 秒 | 71.2 + 12.1 = 83.3 秒 |
| Linux arm64 | 284.3 秒 | 176.4 + 6.3 = 182.7 秒 |
| Linux x64（仿真） | 730.5 秒 | 489.9 + 9.6 = 499.5 秒 |

macOS 客户端构建 764.4 秒，合并打包与原生验收 87.3 秒。另从确切已验收的签名客户端核对了隔离中文青碧经典界面，截图背景、面板和按钮配色与主题值一致。

同一草稿完成 6 批上传。GitHub 首项资产进入草稿到最后一对文件就绪前相隔 893.7 秒。正式标签冻结到公开共 1560.7 秒，最后一对文件就绪到公开为 529.3 秒。Windows 一度未枚举到 Mac 原生文件系统刚写入的就绪标记，刷新 SMB 目录后以原标记和校验过的文件继续上传。

AMD64 Docker 单独上传批次用时 433.0 秒，包含本机校验、传输和 GitHub 响应。1.0.5 的标签冻结到公开为 1,603.2 秒，本轮为 1,560.7 秒。最后一对文件就绪后仍有进行中的上传批次。

## 线上核验和本地安装

线上 14 项资产的名称、大小及 SHA256 均与验收产物一致，下载核对了七份 SHA256 文件。

| 归档 | SHA256 |
| --- | --- |
| `AudioLibExplorer-v1.0.6-macos-arm64.zip` | `9f9480a300f134482a86a5380d0f7e458b65fdaf65e735a3eb2b99330114ffa3` |
| `AudioLibExplorer-v1.0.6-win-x64-portable.zip` | `3579cd53f47201d82ec024583a1acb862b30f030af8d575f7022bfe64d62abe8` |
| `AudioLibServer-v1.0.6-docker-linux-amd64.tar.gz` | `19418a63e0586b72ed6d94ce9d165b5276b23d75f9b265db451f705daa3b5cac` |
| `AudioLibServer-v1.0.6-docker-linux-arm64.tar.gz` | `750b4f8dd28a5f079f45bc98ffc3162d476b8e1a4891045c624c5d342239f9c3` |
| `AudioLibServer-v1.0.6-linux-x64.tar.gz` | `87ffb66ac07197528b564c3412859a87cb437d8413215058de2ed3c4e04ce435` |
| `AudioLibServer-v1.0.6-macos-arm64.tar.gz` | `fb831f51bf44a66dea367a7165ee19d8c64baed44e96d0abfa263df8caeb32b9` |
| `AudioLibServer-v1.0.6-win-x64.zip` | `e50ca65690c21fb9dc211f1ef7a06bb072ee2e5fbc6c1de6d8b38415fc8fac16` |

发布后将 Windows 归档内各文件与验收目录逐项核对，固定 `local_release/` 的 908 项包文件、CLI 1.0.6 和隔离桌面启动均通过。本次复核复制 0 个文件。
