# AudioLib Explorer 1.1.1 发布记录

正式版于 2026-11-20 18:09:43 UTC（北京时间 2026-11-21 02:09:43）发布，GitHub 标记为 Latest，七份归档及十四项资产通过核验。 [正式发布](https://github.com/Freedragonlancer/AudioLib-Explorer/releases/tag/v1.1.1)

## 版本范围与 Beta

1.1.1 相对 1.1.0 提升 PATCH。Classic 和 Studio 的详情字段为滚动条预留空间，路径栏按窗口宽度折叠上级目录，切换文件夹时保留路径栏与仍可用的结果，直到新结果就绪。桌面和网页重新组织服务器管理页，支持调整旧客户端兼容快照及试听预缓冲期间的列表 Waveform 文件属性读取。API `/api/v1/`、本地写 schema 8、显式服务端迁移 schema 9、快照读取 `3..=9` 及完整资料库格式沿用 1.1.0。 [#382](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/382) · [#383](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/383) · [#384](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/384) · [#385](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/385)

冻结的 Beta 提交为 `db822f9c9404a1f294ab03b859419d4bc9667c92`，源码树为 `eac72d815837564a226fb78e1bf06bf61dea99e8`。双语 README 的 #386 未纳入本版，其截图仍显示部分服务器状态。 [准备 PR](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/387)

沿用 `15db5ee` 的 Classic 和 Studio 详情滚动条回归、`895d8b3` 的路径栏折叠与上级目录回归、`dead040` 的 ASIO 本地和只读快照导航回归，以及 #385 的桌面 ASIO、服务端和远程客户端严格 Clippy 与功能检查。服务端设置读写与重启恢复、旧快照协商、管理权限、浏览器扫描进度重试和过期响应保护沿用 #385 的检查记录。 [服务端验证](Rust_Validation.md)

Windows Beta 构建、目录打包、便携验收与独立安装分别耗时 1,104.4、20.4、17.6 和 9.6 秒。Cargo 编译 10 个工作区单元，第三方依赖全部命中。冻结的 Beta 便携包 EXE 使用三份合成 WAV，以软件渲染生成详情面板和四页签服务器管理截图。桌面锁定阻止了对该包的实体键鼠复测，交互覆盖沿用相关开发提交的实测和 ASIO 回归结果。

Windows 远端冒烟 10/10 通过，耗时 335.6 秒。隔离同步 909,342 条、10,150.7 MiB，耗时 137.3 秒，配置根内读取样本 20/20。配置根外 828,780 条，与 1.1.0 一致。

Mac Beta 服务端构建、打包与原生验收分别耗时 63.6、13.5 和 10.4 秒。使用已验收的服务端与迁移器刷新 Beta，耗时 249.1 秒，配置根探测 1/1 通过。对当前服务的只读请求确认五个管理页签、网页资产与源码一致，以及两项数据库设置均由配置提供。Mac 客户端在固定 checkout 和 target 上用 Rust 1.97.1、macOS SDK 27.0 预编译，耗时 734.3 秒，Cargo 记录 26 条编译，其中 10 条为工作区单元。

## 正式构建与缓存复用

正式提交 `8d2a74ef22a3437071780181f92dd9dcbe4f3553` 与 Beta 使用同一源码树，注释标签对象为 `26a33b1739d68ebe6d1fe9eefeb297bc284f51f0`。 [晋升 PR](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/388)

| 产物 | 构建秒 | 打包秒 | 验收秒 |
| --- | ---: | ---: | ---: |
| Windows x64 客户端 | 2.1 | 22.8 | 18.0 |
| Windows x64 服务端 | 99.6 | 13.3 | 8.6 |
| Mac arm64 客户端 | 3.6 | 127.7（含验收） | 同左 |
| Mac arm64 服务端 | 90.7 | 14.7 | 11.5 |
| Linux AMD64 服务端 | 189.0 | 16.6 | 10.8 |
| Linux ARM64 服务端（Docker 输入） | 177.3 | 7.7 | 8.6 |

Windows 正式客户端零重编译，两份 EXE 与 Beta 的 SHA256 一致。正式包的固定安装耗时 56.7 秒，核验 908 项包文件与隔离启动。Windows 服务端另用 56.1 秒构建验收夹具。8 GiB 缓存预算下保留 6,903,653,101 逻辑字节。

Mac 正式客户端 Cargo 耗时 0.64 秒，零重编译，两份未签名二进制与 Beta 的 SHA256 一致。构建阶段从 Beta 预编译的 734.3 秒降到 3.6 秒，减少 730.7 秒。正式 ad-hoc 签名、打包和 10 项原生运行检查照常执行。此次 Mac 客户端包验收覆盖隔离合成音频和原生退出，实际声学输出和真实服务器连接未在该包验收中执行。8 GiB 缓存预算下清理 815,428,835 逻辑字节，保留 8,088,523,886 逻辑字节。

Linux AMD64 在 ARM64 OrbStack 上仿真，复用专用 Cargo target。构建前有 279 个 release 指纹、936,370,010 逻辑字节和 627 个已下载的注册表包。Cargo 记录 248 项 Fresh，只编译 7 个工作区单元，构建阶段较 1.1.0 短 5.0 秒。8 GiB 预算下保留 1,231,603,651 逻辑字节。Linux ARM64 使用独立 target。两个架构的原生包各通过 9 项验收。

## Docker 归档和上传实测

两份离线镜像分别使用同架构已验收的原生包和收据。

| 架构 | 归档字节 | 打包秒 | 离线验收秒 | 离线验收 |
| --- | ---: | ---: | ---: | ---: |
| AMD64 | 240,156,579 | 79.7 | 10.9 | 8/8 |
| ARM64 | 220,915,682 | 45.0 | 10.2 | 8/8 |

五组原生包及校验文件已在草稿 `453007633` 核验。两路 Docker 上传使用同一草稿。首轮 AMD64 归档 POST 遇到连接中断。协调器等待 ARM64 的归档、校验文件和远端核验结束，随后回查同一草稿，确认 ARM64 已齐、AMD64 缺两项，切换为单路，仅补传 AMD64。

| 架构 | 尝试 | 结果 | 读取与 SHA256（秒） | 归档 POST（秒） | 校验文件 POST（秒） | 远端核验（秒） |
| --- | ---: | --- | ---: | ---: | ---: | ---: |
| ARM64 | 1 | 通过 | 1.418 | 461.924 | 1.113 | 7.970 |
| AMD64 | 1 | 连接中断 | 0.961 | 30.823 | 0.000 | 4.338 |
| AMD64 | 2 | 通过 | 1.490 | 395.179 | 1.163 | 7.493 |

按尝试目录创建和终态收据落盘时间估算，首轮两路重叠约 38.9 秒，AMD64 失败后 ARM64 继续约 398.6 秒。ARM64 完成至 AMD64 单路补传启动间隔 16.1 秒，包含两次回查。从首轮准备至 AMD64 补传收据落盘共 901.0 秒。POST 计时包含流式读取、传输和 GitHub 响应。

## 正式资产与固定安装

发布前核对同一草稿的完整资产集合。公开后复核指定注释标签、Latest 状态、十四项资产的名称、大小与 SHA256，并下载比对七份校验文件。

| 归档 | SHA256 |
| --- | --- |
| `AudioLibExplorer-v1.1.1-macos-arm64.zip` | `a12e94e2d9702a3d1c17fcd769cc7a70967fa8a4e3b543a02e8b49ecf0fa31f6` |
| `AudioLibExplorer-v1.1.1-win-x64-portable.zip` | `2258584f19d82c3cd40be39cf9ab6729601d674d925035d57ab63e3a559e092e` |
| `AudioLibServer-v1.1.1-docker-linux-amd64.tar.gz` | `1d1725634db9919b76d25709645064f10bd0934f9b20ee2362f66ce98e95d1e0` |
| `AudioLibServer-v1.1.1-docker-linux-arm64.tar.gz` | `8c65aa198e80ae0dd9e2378e0277fabbe81db986553ce15c3f14678c036ed60c` |
| `AudioLibServer-v1.1.1-linux-x64.tar.gz` | `6f3dfafcdcec67c3b5180fb1b1a7a758f09af4ac9feddd9d7d1bad9a8b0f0461` |
| `AudioLibServer-v1.1.1-macos-arm64.tar.gz` | `5390cd7e6a2d3ef0437816b69934c0782f3b744bc1528a92fd119c0fbe9f6236` |
| `AudioLibServer-v1.1.1-win-x64.zip` | `5c4d5882891efc66388f91c7663a629fc85254454fbf420a43f8a23c53c0c419` |

发布后将正式 Windows 归档的 906 个条目与已验收的便携目录逐一核对。固定 `local_release/` 核验 908 项包文件的 SHA256、CLI 1.1.1 和隔离桌面启动，耗时 27.6 秒，复制 0 项。
