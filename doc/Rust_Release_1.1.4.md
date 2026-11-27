# AudioLib Explorer 1.1.4 发布记录

GitHub 于 2026-11-27 16:53:45 UTC（北京时间 2026-11-28 00:53:45）将 `v1.1.4` 发布为 Latest。七份归档和七份 SHA256 校验文件通过线上核验。 [正式发布](https://github.com/Freedragonlancer/AudioLib-Explorer/releases/tag/v1.1.4)

## 本地数据库位置与音频路径

本地设置支持导出可独立打开的 SQLite 数据库，新建库时可分别指定音频目录和数据库保存位置。迁移数据库会先验证副本，再切换登记，保留原库文件、播放历史、搜索工作区和厂商导入暂存。缺失音频可按相对路径、文件名和音频属性查找候选，预览确认后先备份再批量修复，保留素材 ID、Metadata 和用户标注。v1.1.4 相对 v1.1.3 提升 PATCH，API v1、本地写 schema 8、服务端迁移 schema 9 及快照读取 `3..=9` 沿用上一版。 [功能 PR](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/400)

发布前修复了新建库失败后无法在原路径重试的问题。SQLite 在同级自有临时目录内初始化，持有目标租约并检查目标路径后再发布完整数据库。初始化时取消或目标管理锁不可用，恢复后均能在相同路径重试并打开通过完整性检查的数据库。Windows MSVC 格式和严格数据库 Clippy 通过，库管理集成 29 项、导入目标租约 11 项通过。冻结的 macOS arm64 Beta 通过 30 项库管理集成测试，覆盖两条同路径重试及符号链接越界检查。 [修复 PR](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/402)

## Beta 验收与正式源码

冻结的 Beta 提交为 `7a8e9148d23266b1c162720a9003a7be5e4a4be9`。Windows x64 ASIO 构建、目录打包、便携验收和 Beta 并排安装分别耗时 1,173.9、21.5、18.3 和 10.2 秒。实际发行 CLI 的库管理验收和跨进程回归分别通过 10/10。Mac Beta 以通过原生验收的服务端与迁移器刷新，耗时 253.1 秒，配置根实读通过 1/1。Windows 远端冒烟 10/10 通过，耗时 375.1 秒，配置根内实读 20/20。隔离同步 909,342 条、10,150.7 MiB，耗时 152.1 秒。配置根外 828,780 条记录与上一版一致。 [准备 PR](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/401)

冻结的 Windows Beta 实包在隔离资料库中拒绝了已存在的数据库目标，并保留原文件。移开测试占位文件后，原表单在同一路径建立 schema 8 数据库并索引一条音频。原库标注与两份合成 WAV 保留，窗口正常关闭。实包还观察了导出保存对话框、数据库迁移入口及默认仅匹配缺失文件的页面。受原生文件选择器自动化限制，完整导出、迁移和路径修复操作沿用对应源码的定向回归。 [验证说明](Rust_Validation.md)

正式提交 `1e8cb7d4d2bd38ce6234938d1dc1c7fd51095167` 与冻结的 Beta 共享源码树 `78a5984f7696396f3aecb8a07d23fe98b585be19`。各平台原生计划绑定正式标签 `v1.1.4`，注释标签对象为 `b22031b7a7adca2a03fa5430b1154ce632c54ff7`。 [晋升 PR](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/403)

## 平台验收与构建缓存

| 平台 | 构建（秒） | 打包（秒） | 验收（秒） |
| --- | ---: | ---: | ---: |
| Windows x64 客户端 | 1.9 | 22.7 | 17.7 |
| Windows x64 服务端 | 93.0 | 13.1 | 8.6 |
| macOS arm64 客户端 | 3.5 | 135.1（含验收） | 同左 |
| macOS arm64 服务端 | 90.9 | 14.6 | 11.4 |
| Linux x64 服务端 | 193.7 | 12.9 | 16.4 |
| Linux arm64 服务端（Docker 输入） | 181.1 | 8.6 | 6.6 |

Windows 正式客户端 Cargo 零重编译，两个 EXE 与 Beta 的 SHA256 相同。macOS 客户端 Beta 在固定 checkout、target、Rust 1.97.1 和 macOS SDK 27.0 下预编译耗时 753.6 秒。正式构建耗时 3.5 秒，Cargo 用时 0.65 秒，零重编译，两个未签名二进制的 SHA256 均与 Beta 相同。两次构建阶段观测相差 750.1 秒。正式客户端照常执行 ad-hoc 签名、打包与 10 项原生运行验收。各架构原生服务端包分别通过 9 项验收。

Linux x64 使用 ARM64 OrbStack 上的 AMD64 仿真。持久 target 构建前有 315 个 release 指纹及 627 份已缓存 crate。本轮 Cargo 记录 248 项 Fresh 和 7 项工作区编译。维护后，Windows、macOS 和 Linux AMD64 分别保留 8,224,127,548、3,121,942,894 和 1,667,032,478 逻辑字节，均达到 8 GiB 预算。macOS 清理约 7.78 GB 逻辑字节，其中包含约 5.90 GB 的整个 `aarch64-apple-darwin/release/deps` 层。次轮构建需要补回该层所需依赖。Linux ARM64 清理 726,693,443 逻辑字节，保留 37,844,577 逻辑字节。

| Docker 归档 | 体积（字节） | 打包（秒） | 离线验收（秒） |
| --- | ---: | ---: | ---: |
| linux/amd64 | 240,179,333 | 85.7 | 10.5（8/8） |
| linux/arm64 | 220,919,960 | 47.7 | 10.0（8/8） |

## Docker 上传

两种架构各自通过镜像验收后，在同一草稿 `460839459` 开始并行上传。ARM64 首轮归档 POST 断线，协调器等 AMD64 上传和核验结束，再回查原草稿。AMD64 文件对齐全，ARM64 缺少归档和校验文件，随后只补传 ARM64 文件对。

| 架构 | 尝试 | 结果 | 读取与 SHA256（秒） | 归档 POST（秒） | 校验文件 POST（秒） | 远端核验（秒） |
| --- | ---: | --- | ---: | ---: | ---: | ---: |
| ARM64 | 1 | 连接中断 | 0.980 | 89.828 | 0.000 | 4.410 |
| AMD64 | 1 | 通过 | 1.622 | 422.744 | 1.153 | 7.593 |
| ARM64 | 2 | 通过 | 1.531 | 364.315 | 35.457 | 7.662 |

据尝试目录创建和终态收据落盘时间估算，首轮重叠 47.0 秒。ARM64 断线后等待 AMD64 约 390.1 秒，两者均结束后回查与启动补传约 18.6 秒。首轮开始至最终 Docker 收据落盘为 920.2 秒。

同时运行的 macOS 服务端归档首次 POST 在 31.445 秒后断线。Docker 文件对核验后，原生上传入口回查同一草稿并补齐缺项。Mac 服务端归档补传 POST 为 26.124 秒，校验文件 POST 为 1.037 秒，远端核验为 7.490 秒。

## 正式资产与固定安装

发布后核对 GitHub 上 14 项资产的名称、状态、大小及 SHA256，并下载七份校验文件逐字节比对。Windows 固定安装复核将线上 ZIP 对应的 906 个条目与已验收便携包比对，核验 `local_release/` 的 908 项包文件及两份 EXE。CLI 1.1.4 和隔离桌面启动通过，耗时 39.7 秒，复制 0 项。

| 归档 | SHA256 |
| --- | --- |
| `AudioLibExplorer-v1.1.4-macos-arm64.zip` | `8b69daccf380c4eb830ad3c6784d1be23a93e29cce4b8422651906cb2b4e31a3` |
| `AudioLibExplorer-v1.1.4-win-x64-portable.zip` | `e920ffa016455aa176732a2284502b243c91e5094a1117062fc5e2aea923561a` |
| `AudioLibServer-v1.1.4-docker-linux-amd64.tar.gz` | `ad2326f61742f419d4226eb7d45f80d6cfeef4a383cee30c13721cb4e6b6f8e6` |
| `AudioLibServer-v1.1.4-docker-linux-arm64.tar.gz` | `20cdb2c4d1c1cbf3d4e8c8044077e6181633dbb5a366fe8247280c3ffdf9545f` |
| `AudioLibServer-v1.1.4-linux-x64.tar.gz` | `0a15f11a830045d749713e27bc09bfd538685fe3358a72b65f6131c36d1e9493` |
| `AudioLibServer-v1.1.4-macos-arm64.tar.gz` | `4cfe086f779f507bb53238be4b178c55f0de454b8b34a38d11176bf348614a02` |
| `AudioLibServer-v1.1.4-win-x64.zip` | `3bdce7c49325376c20c14a1f765e113bb003ed3cb71f17b6518809a148139f45` |
