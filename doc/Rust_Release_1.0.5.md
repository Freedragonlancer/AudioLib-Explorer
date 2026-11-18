# AudioLib Explorer 1.0.5 发布记录

正式版于 2026-11-18 17:44:08 UTC（北京时间 2026-11-19 01:44:08）发布。验收后的 beta `fc6ef6bf01701c2cf95898d28f00abcd06c7476d` 通过 [#345](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/345) 晋升，正式归档的源码为 `f3fd17df2771e1a3aaa68b21b928ce17d4777f7e`，标签对象为 `5a30cd81e07a52349619d50bde900d7638590852`。API `/api/v1/`、本地写 schema 8、服务端写 schema 9、快照 `3..=9` 和完整资料库格式沿用 1.0.4。 [GitHub Release](https://github.com/Freedragonlancer/AudioLib-Explorer/releases/tag/v1.0.5)

## 改动和验收

- #340 允许长度短报 4 字节且 PCM 与所需填充恰好到达文件末尾的普通 RIFF WAV，#342 调整已播放颜色和 Waveform 选区提示。
- #341 支持在同一草稿分批上传已验收的归档与 SHA256 文件。Windows 和 Unix 服务端计划输出独立 JSON 验收收据。#343 升版并记录固定发布缓存流程。
- #344 修正旧扫描反例恰好使用新兼容条件的问题。修正前为 2 passed / 1 failed，修正后保留短报 2 字节、Metadata 越界和真实截断失败，并验证完整短报 4 字节样本按全时长扫描。Windows DB 格式与严格 Clippy 及 4 项定向测试通过。

Windows Rust 1.97.1 MSVC 的桌面 ASIO 严格 Clippy 和 19 项定向测试通过，1 项渲染光标预览默认忽略。沿用 `598c3dd` 在默认 feature 下的音频严格 Clippy 和 194 项单测，4 项默认忽略。涉及的 RIFF 解码路径独立于 ASIO。`c47bc69` 的模拟发布验证覆盖续传与失败恢复。Mac arm64 服务端使用无默认 feature 的严格 Clippy，Waveform API 定向测试通过。

Windows Beta 便携验收及 WASAPI 实机播放与 IPC 20 项通过。两个 EXE 保留原构建 `82acdd7` 的证据。到最终 beta 仅改变 DB 集成测试。Mac beta 健康检查报告 1.0.5 / `fc6ef6b`，配置根读取 1/1，部署的服务端及迁移器 SHA256 通过。Windows beta 冒烟 10 passed / 0 failed，完整同步 909,342 条、10,150.7 MiB、177.5 秒，流式读取 20/20。配置根覆盖与上轮一致，828,780 条记录位于可流式根之外。

Windows x64 客户端采用已核验的 ASIO SDK 2.3.4，便携验收通过。macOS arm64 客户端通过构建、ad-hoc 签名和原生启动验收。Windows x64、macOS arm64、Linux x64 与内部 Linux arm64 原生服务端包的运行验收均通过。Linux x64 在 ARM64 OrbStack 仿真环境执行。两份 Docker 均消费实际原生 JSON 收据，各完成 8 项离线镜像运行检查。

## 固定缓存与分批发布

Windows 和 macOS 使用各自固定的发布 checkout 与已登记 target。客户端、服务端依次使用共享 target，最后一个计划按 8 GiB 预算执行 Intermediate 维护。最终维护收据显示，Windows target 保留 4,282,891,532 字节（3.99 GiB），macOS target 保留 6,226,650,418 字节（5.80 GiB）。两端均符合 8 GiB 预算，本轮移除 0 字节。

Windows 正式客户端构建 1,097.6 秒，打包 22.5 秒，便携验收 17.9 秒。Windows 服务端构建 122.8 秒，打包 13.1 秒，验收 8.5 秒，另用 73.9 秒准备夹具。macOS 客户端构建 832.5 秒，打包及原生验收 87.2 秒。macOS 服务端构建 113.0 秒，打包 19.3 秒，验收 47.4 秒。Windows 正式客户端编译日志有 339 条编译记录。独立 PowerShell 进程复现了等长源码和 target 路径导致的 remap 参数换序，Cargo 可能将这组参数判为不同构建输入。尚未测得排序修正后的实际构建时间。

同一个草稿完成 6 批上传，合计七对已验收文件。GitHub 首项资产记录比最后一对文件就绪早 1213.1 秒。正式标签冻结到 GitHub 公开共 1603.2 秒，最后一对文件就绪到公开为 153.9 秒。这些区间包含本机准备、传输与 GitHub 响应。

## 线上和固定安装

线上 14 项资产的名称、大小及 SHA256 与本地验收产物一致。七份 SHA256 文件已下载复核。归档摘要如下。

| 归档 | SHA256 |
| --- | --- |
| `AudioLibExplorer-v1.0.5-macos-arm64.zip` | `2fbdc390aae0c2adc94ee7955346f92b90444094ad28b3336a9daeb0559db8d3` |
| `AudioLibExplorer-v1.0.5-win-x64-portable.zip` | `4558c2234658ea959965cb6588f4814e159fe234c0cd05c1769644c5a401b738` |
| `AudioLibServer-v1.0.5-docker-linux-amd64.tar.gz` | `4c4f1e069323a2089a89f12b0288b09ccdf0776a2e128035676d1fab5e694fe9` |
| `AudioLibServer-v1.0.5-docker-linux-arm64.tar.gz` | `07a0d3a7353ff4111b0991ae2ebf9c23860c836fb84e5571955e71273094af46` |
| `AudioLibServer-v1.0.5-linux-x64.tar.gz` | `3e1865ced219c2936e7964be557c807bcdedbdf2fac59cd3625e6f479ea9b8ce` |
| `AudioLibServer-v1.0.5-macos-arm64.tar.gz` | `1961dce6a0f462a5cd9cd4768038fa3efdbd784d2008de65ca09cdcb72d67d04` |
| `AudioLibServer-v1.0.5-win-x64.zip` | `60b776825e1d8e7863f50681bc1bc18df637788fc383c74af933302e3e23c186` |

发布后先将 Windows 正式归档内 906 项文件与便携目录逐项核对。随后对固定 `local_release/` 的 908 项包文件执行哈希复核（另含两份 EXE 校验文件），CLI 1.0.5 和隔离桌面启动通过，本次复制 0 个文件。
