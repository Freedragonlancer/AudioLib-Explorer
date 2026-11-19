# AudioLib Explorer 1.0.8 发布记录

正式版于 2026-11-19 15:21:14 UTC（北京时间 2026-11-19 23:21:14）发布，七份归档和十四项资产已核验。 [正式发布](https://github.com/Freedragonlancer/AudioLib-Explorer/releases/tag/v1.0.8)

## 分支审计与发布身份

- [#364](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/364) 保持 Classic 与 Studio 的搜索框在关键词和 AI 模式下位置与宽度稳定，并对齐文件夹和资料库区段。
- [#362](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/362) 将资料库封面、名称和匹配数纵向排列，保留各字段开关及缺图时的名称回退。审核了与 #364 合并后的树及既有定向验证，再合入 beta。
- Search Metadata v2 的路径搜索、Vorbis 标签和厂商表格导入已由 [#228](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/228) 集成，并随 [v0.9.0](https://github.com/Freedragonlancer/AudioLib-Explorer/releases/tag/v0.9.0) 发布。现行本地路径索引沿用 schema 8，显式服务端迁移采用 schema 9。旧设计 [#209](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/209) 与 WIP [#218](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/218) 已关闭，对应三条远端分支已清理。
- [#363](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/363) 引入 Mac Beta 客户端预编译、Linux AMD64 持久缓存与 Docker 上传分段计时。共享卷的子阶段租约和 checkout 内工具链探测通过离线回归。

[#365](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/365) 冻结 v1.0.8 Beta `ba0aeb8c729f88668b7de0ab87cf7bee74c4207c`。经 [#366](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/366) 晋升的正式归档源码为 `54377f7d9a60b7d12e01e28bb01178d2c25977f9`，标签对象为 `afd489935bfade4d34e34081e38b0b6e330afb5f`。两者使用同一 Git tree `b4313ed21c09c93fe249c617715dff9b3af8f84a`。API `/api/v1/`、本地写 schema 8、服务端写 schema 9、快照 `3..=9` 及完整资料库格式沿用 1.0.7。

## Beta 验收

Windows 在实际发布 ASIO 配置下通过格式检查、严格桌面 Clippy、9 项侧栏和 24 项搜索交互测试。合成 WAV 封面与缺图名称回退在已验收 Beta 包的 Classic、Studio 原生界面中检查。客户端构建 1291.4 秒，Cargo 日志记录 10 条工作区编译、0 条第三方依赖编译。目录打包 25.3 秒，便携验收 20.5 秒，独立 Beta 安装 10.9 秒。

Mac Beta 使用通过原生包验收的服务端与迁移器，部署后二进制摘要匹配，健康响应为同一候选，配置根读取 1/1。构建、打包与验收分别为 74.4、14.0、10.2 秒。后续健康请求出现一次可复现的约 30 秒 HTTP 408，普通 HTTP 路由仍能回应。Beta 在只读诊断期间恢复为 HTTP 200，同一服务进程与资料库继续使用，具体根因没有确认。

恢复后 Windows 正式 Beta 冒烟 10/10，配置根内的音频范围请求 20/20。完整隔离同步 909,342 条、10,150.7 MiB，耗时 165.7 秒，整套冒烟 347.1 秒。已有 828,780 条记录位于配置根外，流读取覆盖沿用 1.0.7 的范围。

## Mac 预编译实测

| 客户端阶段 | 管线耗时 | Cargo 耗时 | Cargo 编译记录 |
| --- | ---: | ---: | ---: |
| 1.0.8 Beta 预编译 | 713.6 秒 | 710 秒 | 26，含 10 个工作区 crate |
| 1.0.8 正式标签构建 | 3.7 秒 | 0.70 秒 | 0 |
| 1.0.7 正式标签构建（历史对照） | 755.3 秒 | 751 秒 | 26，含 10 个工作区 crate |

Beta 与正式构建使用同一固定 checkout、Cargo target、Rust 1.97.1、SDK 27.0 和编译参数。Beta 预编译在 Windows Beta 构建期间开始并完成。两份未签名二进制的 SHA256 与 Beta 一致。正式构建生成自己的提交绑定收据，继续走原有 ad-hoc 签名、打包和原生验收，合并阶段实测 147.6 秒。正式构建阶段较 1.0.7 的历史记录少 751.6 秒，约 99.5%。这项差值用于比较正式发布路径。1.0.8 的大部分编译实际在 Beta 时完成，历史对照包含各次机器负载差异。

## 正式原生验收与缓存维护

| 归档阶段 | 构建 | 打包 | 原生验收 |
| --- | ---: | ---: | ---: |
| Windows x64 客户端 | 1.6 秒 | 26.7 秒 | 19.0 秒 |
| Windows x64 服务端 | 105.1 秒 | 15.0 秒 | 10.1 秒 |
| Mac arm64 服务端 | 88.8 秒 | 14.4 秒 | 10.9 秒 |
| Linux x64 服务端（仿真） | 454.2 秒 | 11.3 秒 | 9.1 秒 |
| Linux arm64 内部服务端 | 161.2 秒 | 7.9 秒 | 6.0 秒 |

Windows 正式客户端 Cargo 耗时 0.57 秒，零重编译。固定安装验证 908 个包文件、CLI 1.0.8 与隔离启动，耗时 75.3 秒，共复制 21 个变化文件。Windows 服务端的独立验收夹具构建耗时 63.0 秒。Mac 原生客户端签名与 10 项验收通过。

每个本机共享 target 在最后一份包通过后维护一次。Windows 保留 5,591,884,181 字节（5.21 GiB），Mac 保留 7,567,314,654 字节（7.05 GiB）。两端均符合 8 GiB 逻辑预算，移除均为 0 字节。

## Linux AMD64 持久缓存首次运行

AMD64 使用 ARM64 OrbStack 上的 Linux AMD64 仿真构建容器。新登记的 AMD64 专用 target 启动时有 0 个编译指纹。既有 Cargo registry 包含 627 个归档，锁定依赖的离线预检通过。构建日志有 255 条编译记录，其中 7 条属于工作区。因此本次已复用 registry，编译 target 属于首次填充。1.0.7 的 481.0 秒是旧构建方式下的历史读数。

Linux x64 与作为内部 Docker 输入的 Linux arm64 各通过 9 项原生包检查。AMD64 使用新入口的独占缓存租约与原有 Intermediate 清理，按 8 GiB 预算保留 797,617,795 字节（0.743 GiB），移除 0 字节。持久卷和固定容器 checkout 保留供后续发布检验跨版本复用。

## Docker 打包与同草稿上传

两份 Docker 镜像各消费本架构已通过的 Linux 原生验收 JSON。AMD64 打包 96.0 秒、8 项离线运行检查 11.0 秒，归档为 240,162,614 字节。ARM64 打包 68.1 秒、8 项离线运行检查 12.3 秒，归档为 220,925,100 字节。两组检查均通过。

每个上传者分别记录读取与 SHA256、归档与校验文件 POST、远端核验的墙钟耗时。读取与哈希包含适用的共享盘入盘复制和复验。POST 包含流式读文件、传输及 GitHub 响应。各段单位为秒。

| 架构 | 尝试 | 结果 | 读取与 SHA256 | 其中入盘 | 归档 POST | 校验文件 POST | 远端核验 |
| --- | ---: | --- | ---: | ---: | ---: | ---: | ---: |
| amd64 | 1 | 未通过 | 1.809 | 0.800 | 185.804 | 0.000 | 4.713 |
| amd64 | 2 | 通过 | 1.539 | 0.000 | 604.065 | 1.167 | 7.280 |
| arm64 | 1 | 未通过 | 1.656 | 0.716 | 438.455 | 0.000 | 4.379 |
| arm64 | 2 | 未通过 | 0.950 | 0.000 | 485.863 | 0.000 | 4.655 |
| arm64 | 3 | 通过 | 1.432 | 0.000 | 369.020 | 1.223 | 7.577 |

同一草稿 ID 为 451488530。两种架构上传进程的重叠窗口合计 687.1 秒。首次两对 Docker 归档的 POST 因连接中断失败。失败后回查同一草稿保留的五对资产，后续沿用已验收本地副本补传缺失资产。AMD64 第二次上传通过后，ARM64 第三次单独补传成功。累计 3 次尝试未通过。
标签冻结至正式发布 2066.9 秒，最后一对就绪至正式发布 1422.6 秒。GitHub 首项资产记录早于最后一对就绪 409.4 秒。

## 正式资产与固定安装

发布前核对同一草稿的完整集合。发布后复核正式标签对象、Latest 状态、十四项资产名称、大小和 SHA256，下载并比对七份校验文件。

| 归档 | SHA256 |
| --- | --- |
| `AudioLibExplorer-v1.0.8-macos-arm64.zip` | `e09c95452310b346aabac054541d80ed55208f85499aba21419bbb1f5688f257` |
| `AudioLibExplorer-v1.0.8-win-x64-portable.zip` | `6b570a0551dc0f451ea263330e674d242245a554f07dee4d288689030938ea24` |
| `AudioLibServer-v1.0.8-docker-linux-amd64.tar.gz` | `b67ce1535d279661135fa91370a8d26cbcaf4cf6cc69cd8cadd323c6dcef7e40` |
| `AudioLibServer-v1.0.8-docker-linux-arm64.tar.gz` | `6f22b7850f36d79b72de63fee0c50ca03b12788e58dcd410d7ed16c4fc06723d` |
| `AudioLibServer-v1.0.8-linux-x64.tar.gz` | `baaa7658be1755521be0a7b812e1aef1a9a769aa31285e76fb1a169e1fbb640f` |
| `AudioLibServer-v1.0.8-macos-arm64.tar.gz` | `7a05c55f6d3b8782477aac261f5c50ae001296f34bdcb04c1512549f10f76c09` |
| `AudioLibServer-v1.0.8-win-x64.zip` | `39161d84fb00633f0e9cc08f1e4614d809194e5f1f43bb3aa39cacaeebed94b4` |

Windows 正式归档与已验收的便携目录逐项核对。发布后固定 `local_release/` 再核验 908 项文件、CLI 1.0.8 和隔离启动，复制 0 项。
