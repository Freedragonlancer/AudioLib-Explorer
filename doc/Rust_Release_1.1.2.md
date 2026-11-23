# AudioLib Explorer 1.1.2 发布记录

GitHub 于 2026-11-23 04:49:09 UTC（北京时间 2026-11-23 12:49:09） 将 `v1.1.2` 发布为 Latest。七份归档与七份 SHA256 校验文件通过线上核对。 [正式发布](https://github.com/Freedragonlancer/AudioLib-Explorer/releases/tag/v1.1.2)

## Windows 切片拖出

服务器模式在选区确定后后台准备 WAV，首次拖出复用已经准备好的切片。按住鼠标时，等待手形会跟随鼠标移出波形区域，目标接受文件时显示系统复制标记。冷缓存需要先取得原素材，实际等待取决于文件大小和网络。API v1、本地写 schema 8 和显式服务端迁移 schema 9 沿用 1.1.1。 [修复 PR](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/391)

Windows x64 上的格式检查、严格 Clippy 和 40 项相关回归通过。Windows Shell 接收文件成功。REAPER 7.82 以标准文件拖放导入 2 秒、48 kHz 的测试 WAV，并按设置将其复制到工程媒体目录。工程副本与导出文件 SHA256 相同，峰值读取返回有效样本，磁盘生成了 `.reapeaks` 文件。开发验证使用回环服务器与本机投放。 [验证说明](Rust_Validation.md)

## Beta 与正式源码

Beta 提交为 `e4d93282c5f6842158fea9eb740d7b79a4c63077`。Windows Beta 与测试服务器之间的 10 项验收全部通过。隔离同步 909,342 条、10,150.7 MiB，耗时 136.0 秒。配置根内的 20 条音频抽样全部通过。配置根外有 828,780 条，与上一版基线一致。Beta 服务刷新后的首次根读取在 15 秒内未返回。核对部署二进制和健康版本后，原服务同一记录的只读重试在 0.29 秒内返回 HTTP 206 和 4,096 字节有效 WAV 数据。

通过晋升 PR 后，正式提交 `63d5bd3b2bbc355f2413e2b0b852ae6a98685609` 与候选提交的源码树同为 `ef028af002f88482b58de81780342faa7c5672b9`，注释标签对象为 `fe95f5fd83b5454faefd8e63a764868a6de1a5de`。正式发布使用标签对应的原生计划和实际产物。 [晋升 PR](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/392)

## 平台验收

| 平台 | 结果 |
| --- | --- |
| Windows x64 客户端 | ASIO 便携包验收通过。固定安装核对 908 项包文件，CLI 报告 1.1.2，隔离桌面启动通过。 |
| macOS arm64 客户端 | ZIP、签名与原生合成运行验收通过，使用 ad-hoc 签名。 |
| Windows x64、macOS arm64、Linux x64 和 Linux arm64 服务端 | 各架构的原生包均通过 9 项验收。Linux x64 在 ARM64 主机的 AMD64 仿真环境运行，Linux arm64 包用于构建 Docker 镜像。 |
| Docker linux/amd64 和 linux/arm64 | 两个离线镜像分别通过 8 项运行验收。 |

首次 macOS Beta 服务端解包时，发布 shell 的权限掩码收紧了文件权限。恢复标准掩码后，同一份归档通过权限和运行验收。构建与打包收据均复用。Docker 打包器的控制台说明和 JSON 收据分别读取，两个镜像均使用首次打包的归档完成离线验收。

四个原生计划的缓存维护合计移除约 1.88 GiB 编译文件（逻辑大小）。Windows、macOS 和 Linux AMD64 的 8 GiB 预算均达标。Linux ARM64 清理中间文件后保留约 36 MiB 最终产物。

## 正式资产与本地同步

GitHub 上的 14 项资产均为 `uploaded`，名称、字节数与 SHA256 和验收文件一致。重新下载的七份校验文件逐字节一致，正文中的归档名及哈希均与线上归档匹配。发布后比对 Windows ZIP 的 906 个条目，再次核验固定安装的文件身份、CLI 版本及隔离启动。

| 归档 | SHA256 |
| --- | --- |
| `AudioLibExplorer-v1.1.2-macos-arm64.zip` | `e6c96d3fc9099e204ac4d27d4e172448c1a4300735caf9ac7fce920e5d807e72` |
| `AudioLibExplorer-v1.1.2-win-x64-portable.zip` | `2849c12aa69a354a4781068af59c444bb711c26b14dbaece86571846be1a631d` |
| `AudioLibServer-v1.1.2-docker-linux-amd64.tar.gz` | `76d0e1e9a18ae9d7dfee4fe5248901c45eab612cc108712852ed7d9ee9199565` |
| `AudioLibServer-v1.1.2-docker-linux-arm64.tar.gz` | `fd2d1c58acbc58f39f6f4d27f2e97d5c042983533d173572f4c12f989b986ef3` |
| `AudioLibServer-v1.1.2-linux-x64.tar.gz` | `c25b6dc5cf78e64e43f3a0065f82b9d93528f98a609982a5c29722c8ff6033aa` |
| `AudioLibServer-v1.1.2-macos-arm64.tar.gz` | `b3d0bd4da2ff7c019caae29ed2a7a6d832093cb67bb2f7f6ee6cc230bc868e22` |
| `AudioLibServer-v1.1.2-win-x64.zip` | `6a27137bd889ed9c42e45dda38e0b842981e7768df223f75ae5a0301ddc00482` |
