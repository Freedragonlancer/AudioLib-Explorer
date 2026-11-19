# AudioLib Explorer 1.0.7 发布记录

正式版于 2026-11-19 07:33:05 UTC（北京时间 2026-11-19 15:33:05）发布。通过验收的 beta `4ed83cc7a5a715bce1b51c0176da09ef4bba8e3b` 经晋升合并至 main，正式归档源码为 `5a4604dd36317f77de240a3d6dda53950c1286b5`，标签对象为 `385e1c154062e905721c7dee9529790b67feb50f`。API `/api/v1/`、本地写 schema 8、服务端写 schema 9、快照 `3..=9` 及完整资料库格式沿用 1.0.6。 [正式发布](https://github.com/Freedragonlancer/AudioLib-Explorer/releases/tag/v1.0.7) [晋升 PR](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/359)

## 分支审计与版本范围

- #355 增加资料库封面、名称和计数显隐，放大仅封面视图并保留缺图时的库名，支持可选启动封面缓存。原有功能验证覆盖封面回退、配置回读、缓存和设置恢复。
- #356 改进 Classic 和 Studio 时间刻度、播放位置线和毫秒级素材时间提示，增加三声道以上素材与设备的独立 MULTI 输出。应用、库和服务端源码与已验收的 `6d54202` 一致。沿用 Windows 199 项 ASIO 音频、55 项合并后桌面、7 项远端路由和 1 项 IPC 用例，以及 Zen Go ASIO 96 kHz 设备上 2／6 声道的原始实机证据。
- #357 允许每个验收完成的 Docker 文件对独立写入同一草稿。相关脚本与 `2d62f33` 的离线模拟输入一致。
- 远端当前没有待合入 beta 的 PR。Search Metadata v2 的独立分支栈含格式、索引和接口变更，保留独立集成范围。近期本地全局设置、sidecar 与验证分支的功能已纳入早先 beta 合并。

## Beta 与正式验收

Windows Beta 包从实际候选构建，耗时 1,040.4 秒，10 条编译记录均为工作区 crate，第三方依赖命中缓存。目录打包 20.3 秒，便携验收 17.6 秒，独立安装 8.4 秒。用同一对 EXE 通过播放 IPC 20/20 和原生关闭 2/2。关闭使用隔离默认设置，预生成开启路径由 #355 定向测试覆盖。Windows 远端冒烟 10/10（320.6 秒），完整同步 909,342 条、10,150.7 MiB（136.9 秒），配置根内流读取 20/20。配置根外 828,780 条记录，覆盖范围与 1.0.6 一致。

Mac Beta 预构建服务端取得 1.0.7 / `4ed83cc` 健康响应和配置根读取 1/1，部署后的服务端和迁移器 SHA256 与候选一致。构建 71.7 秒，打包 13.3 秒。首次解包权限受宿主 umask 影响，核实原归档模式和哈希后按标准 umask 022 复验，原生验收 8.9 秒通过。针对 macOS 原生播放配置的 audiolib-audio 严格 Clippy 与 198 项单测通过，3 项忽略，独立开发缓存中间产物按零预算清理。

Windows 和 macOS 客户端通过各自正式包的隔离启动验收。Windows x64、Mac arm64、Linux x64 和内部 Linux arm64 服务端均通过原生包验收。两份 Docker 镜像使用对应的 Linux 原生验收收据，各通过 8 项离线运行验证。Linux x64 使用 ARM64 OrbStack 仿真环境。Mac 客户端采用 ad-hoc 签名。

Windows 正式客户端从与 Beta 相同源码树的最终标签运行构建，耗时 1.8 秒（Cargo 0.45 秒），零重编译和指纹失效。打包 22.7 秒、便携验收 17.8 秒、固定安装 70.0 秒。服务端构建 89.3 秒、打包 13.0 秒、隔离启动及迁移验收 8.1 秒，独立 Windows 验收夹具工具耗时 53.6 秒。

固定发布 target 在两个本机计划完成后各维护一次。8 GiB 预算下 Windows 保留 5,155,981,557 字节（4.80 GiB），macOS 保留 7,123,130,379 字节（6.63 GiB）。两端均符合预算，本轮移除 0 字节。

Mac arm64 服务端构建 72.5 秒、打包 18.6 秒、原生验收 10.1 秒。Mac arm64 客户端构建 755.3 秒，合并打包及原生验收 83.4 秒。

Linux x64 仿真构建 481.0 秒、打包 12.2 秒、原生验收 9.6 秒，Docker 打包 82.9 秒、离线验收 10.6 秒。Linux arm64 构建 164.6 秒、打包 6.2 秒、原生验收 5.4 秒，Docker 打包 53.7 秒、离线验收 12.2 秒。两个独立构建容器在各自交付后清理了约 1.48 GB 逻辑中间产物并停机。

Mac 隔离验收截图中的合成单声道样本可显示 Waveform 和 MONO 控件。封面渲染沿用 Windows 的定向验证，实体多声道设备测试来自 Windows Zen Go。

## 并行上传与线上复核

同一草稿核验了 7 个文件对。两种 Docker 的独立工作进程重叠 176 秒，计时包括本机校验、传输和 GitHub 响应。GitHub 首项资产记录比最后一对文件就绪早 953 秒，标签冻结到公开为 1274.5 秒，最后一对就绪到公开为 159 秒。发布前从同一草稿重新核对完整集合，发布后确认正式 Release 的资产集合、大小与 SHA256，另下载七份 SHA256 文件比对。

| 归档 | SHA256 |
| --- | --- |
| `AudioLibExplorer-v1.0.7-macos-arm64.zip` | `47134d7d10c0b355c9f6acc891b6ec38421f0b469b1465d7174d2ebc306873bf` |
| `AudioLibExplorer-v1.0.7-win-x64-portable.zip` | `ff108f9b7759b845bdf4c3ba3f60d185379e277f44670f3bc10c46396890b57f` |
| `AudioLibServer-v1.0.7-docker-linux-amd64.tar.gz` | `1db8a6fc5fa07e9789d5c7e93efc26cdd797fd84997fa1fe130d9e96f34cb332` |
| `AudioLibServer-v1.0.7-docker-linux-arm64.tar.gz` | `e11374d4003450fe4c3b0f4a7377aed00c1426dbdf3f927b3b7215444f4d3f4e` |
| `AudioLibServer-v1.0.7-linux-x64.tar.gz` | `6e9b3b2d38f894f11e3ecf10af797209b3b9ec55cb1ed998fa3ba60e0a91fa34` |
| `AudioLibServer-v1.0.7-macos-arm64.tar.gz` | `6bc9fd152cb87eca7a2786dc35e7f4f14ce88b6154c2c5e96827668d69643830` |
| `AudioLibServer-v1.0.7-win-x64.zip` | `84c2e65b97eea51534641e20625b957940cfa8f9539f1f5210d6354bbd8809de` |

发布后将 Windows 归档内各文件与验收目录逐项核对。固定 `local_release/` 的 908 项包文件、CLI 1.0.7 和隔离桌面启动均通过，本次复核复制 0 个文件。
