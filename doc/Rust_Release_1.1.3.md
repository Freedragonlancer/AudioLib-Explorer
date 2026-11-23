# AudioLib Explorer 1.1.3 发布记录

GitHub 于 2026-11-23 10:11:47 UTC（北京时间 2026-11-23 18:11:47）将 `v1.1.3` 发布为 Latest。七份归档和七份 SHA256 校验文件通过线上核验。 [正式发布](https://github.com/Freedragonlancer/AudioLib-Explorer/releases/tag/v1.1.3)

## 服务器模式下首次导出短选区

对符合条件的 PCM 或浮点 WAV，客户端在冷缓存下按范围读取文件头和选区帧。完整缓存、长选区和需要解码的格式继续使用原导出路径。空目录、未发布的临时文件和长度错误的缓存不会阻挡范围读取。状态栏在准备期间提示首次拖出可能需要几秒。v1.1.3 相对 v1.1.2 提升 PATCH，API v1、本地写 schema 8、服务端迁移 schema 9、快照读取 `3..=9` 及资料库完整格式沿用上一版。 [实现 PR](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/395)

受影响的桌面、音频和远程客户端沿用 #395 的格式检查、严格 Clippy 和定向回归。最后一次审查改动后的 21 项远端下载测试通过。PCM、浮点和多声道字节验证、ETag 与区间验证、中英文状态栏，以及取消过期下载的独立复测均有通过记录。限速回环测试的 2 秒切片从 7.790 秒降至 0.583 秒，计时截至 WAV 写入磁盘。 [验证说明](Rust_Validation.md)

## Beta 验收与正式源码

冻结的 Beta 提交为 `c089cac5704c1c58642aca31d173735c32f0e383`。Windows x64 ASIO 构建、目录打包、便携验收和 Beta 并排安装分别耗时 1,055.0、18.9、16.5 和 8.7 秒。远端冒烟 10/10 通过，耗时 323.7 秒。隔离同步 909,342 条、10,150.7 MiB，耗时 136.4 秒，配置根中的 20 条读取样本通过。配置根外 828,780 条记录与上一版一致。Mac Beta 服务端通过原生包验收，使用通过验收的服务端与迁移器刷新 Beta 用时 257.6 秒，配置根实读通过 1/1。 [准备 PR](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/396)

在冻结的 Windows Beta 实包上，以 7,680,044 字节的合成 PCM WAV 验证完整缓存及冷缓存。冷缓存通过两次 HTTP 206 取得 65,536 字节的头部和 382,024 字节的选区音频，落盘的 48 kHz 双声道切片为 1.9897 秒，PCM 与原件逐字节一致。首次验证遇到辅助中继使用 HTTP/1.0 导致的响应问题，改为 HTTP/1.1 并显式关闭连接后，用同一包通过。实包验证覆盖切片准备与写盘，真实公网到 DAW 接收的端到端耗时未测量。

正式提交 `b3b18a6fcb4a47496348e69a572515d9378a78ee` 和冻结的 Beta 源码树均为 `e090fa5891ea863810f1b5760bb9666013d09aad`。各平台原生计划绑定正式标签 `v1.1.3`，注释标签对象为 `68d84fb27aa38329928a14df720e4de507fa6858`。 [晋升 PR](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/397)

## 平台验收与构建缓存

| 平台 | 构建（秒） | 打包（秒） | 验收（秒） |
| --- | ---: | ---: | ---: |
| Windows x64 客户端 | 1.9 | 21.7 | 16.7 |
| Windows x64 服务端 | 91.5 | 11.9 | 8.3 |
| macOS arm64 客户端 | 3.4 | 132.1（含验收） | 同左 |
| macOS arm64 服务端 | 88.1 | 14.4 | 11.6 |
| Linux x64 服务端 | 184.5 | 14.3 | 15.5 |
| Linux arm64 服务端（Docker 输入） | 175.6 | 7.9 | 6.3 |

Windows 正式客户端 Cargo 零重编译，两个 EXE 与 Beta 的 SHA256 相同。macOS 客户端 Beta 在固定 checkout、target、Rust 1.97.1 和 macOS SDK 27.0 下预编译耗时 739.4 秒。正式构建耗时 3.4 秒，Cargo 用时 0.61 秒，零重编译，两个未签名二进制的 SHA256 均与 Beta 相同。两次构建阶段观测相差 736.0 秒。正式客户端照常执行 ad-hoc 签名、打包与全部 10 项原生运行验收。各架构原生服务端包分别通过 9 项验收。

Linux x64 使用 ARM64 OrbStack 上的 AMD64 仿真。独立持久 target 在构建前有 303 个 release 指纹及 627 份已缓存 crate。本轮 Cargo 记录 248 项 Fresh 和 7 项工作区编译。8 GiB 预算下，Windows、macOS 和 Linux AMD64 分别保留 7,779,231,626、8,482,356,173 和 1,521,452,865 逻辑字节，三者均达标。Linux ARM64 清理 726,100,034 逻辑字节，保留 37,844,097 逻辑字节。

| Docker 归档 | 体积（字节） | 打包（秒） | 离线验收（秒） |
| --- | ---: | ---: | ---: |
| linux/amd64 | 240,154,155 | 80.2 | 10.8（8/8） |
| linux/arm64 | 220,939,310 | 44.6 | 10.3（8/8） |

## Docker 上传

两种架构各自完成原生镜像验收后，在同一草稿 `455228357` 开始并行上传。ARM64 首轮归档 POST 断线，协调器等 AMD64 上传和核验结束，再回查原草稿。AMD64 文件对齐全，ARM64 缺少归档和校验文件，随后只补传 ARM64 文件对。

| 架构 | 尝试 | 结果 | 读取与 SHA256（秒） | 归档 POST（秒） | 校验文件 POST（秒） | 远端核验（秒） |
| --- | ---: | --- | ---: | ---: | ---: | ---: |
| ARM64 | 1 | 连接中断 | 0.916 | 97.653 | 0.000 | 4.333 |
| AMD64 | 1 | 通过 | 1.513 | 437.935 | 1.068 | 7.340 |
| ARM64 | 2 | 通过 | 1.430 | 373.688 | 1.042 | 7.160 |

据尝试目录创建和终态收据落盘时间估算，首轮重叠 52.1 秒。ARM64 断线后等在途 AMD64 结束约 398.8 秒，回查与启动补传约 14.6 秒。首轮开始至最终 Docker 收据落盘为 905.7 秒。

同时运行的 macOS 服务端归档首次 POST 在 30.750 秒后断线。Docker 文件对核验后，原生上传入口回查同一草稿并补齐缺项。Mac 服务端归档补传 POST 为 26.746 秒，校验文件 POST 为 1.163 秒，远端核验为 7.489 秒。

## 正式资产与固定安装

完善发布说明时，GitHub 草稿短暂返回临时标签名。在原草稿校正标签关联后，复核正式 Git 标签、提交与全部资产的 ID、字节数及 SHA256，再正式发布。发布后核对线上 14 项资产并下载七份校验文件逐字节比对。Windows 固定安装复核将线上 ZIP 对应的 906 个条目与已验收便携包比对，核验 `local_release/` 的 908 项包文件及两份 EXE。CLI 1.1.3 和隔离桌面启动通过，耗时 27.9 秒，复制 0 项。

| 归档 | SHA256 |
| --- | --- |
| `AudioLibExplorer-v1.1.3-macos-arm64.zip` | `743d673a8e4598bddb206b4fc59c10bc74702f6e14eed76e6ef71c9b03a2963f` |
| `AudioLibExplorer-v1.1.3-win-x64-portable.zip` | `cc007dc42d18d203366da62f9f9d9d8cea7285ee751af8a9b1dc7946002124bb` |
| `AudioLibServer-v1.1.3-docker-linux-amd64.tar.gz` | `89baba04cd57b1cdbbefeb1ab9559a917066a5e6430277edde22601859e66e3c` |
| `AudioLibServer-v1.1.3-docker-linux-arm64.tar.gz` | `26d347cab29f0e8be6c6821acbb0a475de0f8cb7701fd598bc5590f7d8bdcac7` |
| `AudioLibServer-v1.1.3-linux-x64.tar.gz` | `6ebb6ebad0eed5fb7d8ef473b353abb27cd88477ea49144ec13d04873235401a` |
| `AudioLibServer-v1.1.3-macos-arm64.tar.gz` | `6ffae8e84d3d26c38b60dac3bd333c124f56a3ee927b14b2a8076b9ce44f66a6` |
| `AudioLibServer-v1.1.3-win-x64.zip` | `28c6ce75e612fad10dffb0a058698c7aebb0fea8c949406a99e8fff09bc2a3f8` |
