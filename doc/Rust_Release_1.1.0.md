# AudioLib Explorer 1.1.0 发布记录

正式版于 2026-11-20 12:25:23 UTC（北京时间 20:25:23）发布，GitHub 标记为 Latest，七份归档与十四项资产通过核验。 [正式发布](https://github.com/Freedragonlancer/AudioLib-Explorer/releases/tag/v1.1.0)

## 版本范围与 Beta

1.1.0 相对 1.0.9 提升 MINOR。搜索任务可在文件夹和资料库索引之间切换，远端 Waveform 首次载入可逐步显示并在完整峰值就绪后缩放。试听区可拖至默认高度的 2.5 倍，双击音高或速度滑杆可单独复位。API `/api/v1/`、本地写 schema 8、显式服务端迁移 schema 9、快照读取 `3..=9` 及完整资料库格式沿用 1.0.9。 [#374](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/374) · [#375](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/375) · [#376](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/376)

冻结的 Beta 提交为 `76a8f75d9009e6b688c9726a11b96243716f5885`，源码树为 `e6cfb345ae43f9aba671533afa51c11978cc1ab2`。#374 的 6 项桌面回归、#375 的搜索索引和恢复回归、#376 的桌面严格 Clippy、Waveform 与试听区高度回归均沿用各自原始提交和覆盖范围。#377 的 Docker 协调器模拟覆盖并发、断线恢复、远端身份冲突和派发前检查。 [准备 PR](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/378) · [#377](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/377)

Windows Beta 构建、目录打包、便携验收和独立安装分别耗时 1,095.2、23.2、18.7 和 10.1 秒。Cargo 编译 10 个工作区单元，第三方依赖全部命中。实际 Beta 包在 Classic 下用三份合成 WAV 验证了文件夹和资料库索引双向替换、任务范围显示和命中结果。Waveform 从 1× 放大到 2.8×，拖高试听区后保留缩放。音高和速度滑杆在松开时分别应用，双击独立复位，应用正常退出。

Windows 远端冒烟 10/10 通过，耗时 344.1 秒。隔离同步 909,342 条、10,150.7 MiB，耗时 145.3 秒，配置根内读取样本 20/20。配置根外 828,780 条记录与 1.0.9 一致。

Mac Beta 服务端构建、打包和原生验收分别耗时 62.0、13.3 和 10.3 秒。使用通过验收的服务端与迁移器刷新 Beta，耗时 179.8 秒。随后客户端在固定 checkout、target 和工具链上预编译，耗时 685.6 秒，Cargo 记录 24 条编译，其中 10 条为工作区单元。

## 正式构建与缓存复用

正式提交 `790e8dffe943c46eeddcb74539299b34d7c0280e` 与 Beta 使用同一源码树，注释标签对象为 `894ce1ada9bada634a15ae673a384a2574565842`。 [晋升 PR](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/379)

Windows 正式客户端构建、打包、便携验收和固定安装分别耗时 1.9、26.2、18.9 和 57.8 秒。Cargo 零重编译，两份 EXE 与 Beta 的 SHA256 一致。固定安装核验 908 项包文件。服务端构建 101.9 秒、打包 14.7 秒、验收夹具构建 57.2 秒、包验收 9.7 秒。8 GiB 预算下保留 6,464,898,810 逻辑字节。

Mac 正式客户端构建 3.4 秒，其中 Cargo 为 0.69 秒，零重编译，两份未签名二进制与 Beta 的 SHA256 一致。原有 ad-hoc 签名、打包与 10 项原生验收合计 121.6 秒。作为同类阶段的历史参考，引入 Beta 预编译前的 1.0.7 正式客户端构建为 755.3 秒。Mac 正式服务端构建、打包和原生验收为 97.5、14.9 和 10.7 秒。8 GiB 预算下保留 8,457,164,180 逻辑字节。 [1.0.7 发布记录](Rust_Release_1.0.7.md)

Linux AMD64 在 ARM64 OrbStack 中仿真，使用上版已登记的专用 target。构建前有 267 个 release 指纹，Cargo 记录 248 项 Fresh，只编译 7 个工作区单元。原生构建 194.0 秒，打包 17.7 秒，9 项原生验收 10.5 秒。8 GiB 预算下保留 1,086,705,755 逻辑字节。1.0.8 首轮使用空 target 时构建为 454.2 秒。 [1.0.8 发布记录](Rust_Release_1.0.8.md)

Linux ARM64 使用独立 target，内部原生包构建、打包和 9 项验收为 182.6、8.6 和 6.6 秒。两份 Docker 镜像分别消费对应架构已验收的原生包及收据。

## Docker 归档和上传实测

| 架构 | 归档字节 | 打包秒 | 离线验收秒 | 离线验收 |
| --- | ---: | ---: | ---: | --- |
| AMD64 | 240,150,912 | 77.3 | 10.9 | 8/8 |
| ARM64 | 220,886,681 | 47.4 | 9.7 | 8/8 |

两路 Docker 上传均锚定草稿 `452644476`。首轮两架构的归档 POST 均遇到连接中断。协调器等待两路退出，回查同一草稿确认缺项，然后自动切换为单路补传。五组非 Docker 文件对已在同一草稿核验。

每个上传者记录读取与 SHA256、归档与校验文件 POST、远端核验的墙钟耗时。读取与 SHA256 包含工作进程内的本地读取及校验。POST 包含流式读取、传输及 GitHub 响应。

| 架构 | 尝试 | 结果 | 读取与 SHA256（秒） | 归档 POST（秒） | 校验文件 POST（秒） | 远端核验（秒） |
| --- | ---: | --- | ---: | ---: | ---: | ---: |
| ARM64 | 1 | 连接中断 | 0.927 | 139.568 | 0.000 | 4.366 |
| AMD64 | 1 | 连接中断 | 0.979 | 85.553 | 0.000 | 4.416 |
| ARM64 | 2 | 通过 | 1.406 | 370.130 | 1.208 | 12.045 |
| AMD64 | 2 | 通过 | 1.506 | 401.953 | 1.129 | 7.375 |

以尝试目录创建到输出收据落盘估算，首轮两路重叠约 86.7 秒。最后一个首轮收据至首个单路尝试间隔 11.9 秒，包含回查。ARM64 单路完成后间隔 6.1 秒启动 AMD64，两个文件对均在原草稿通过。首轮准备至最终 Docker 收据落盘为 974.5 秒。

## 正式资产与固定安装

发布前核对同一草稿的完整集合。发布后复核注释标签、Latest 状态、十四项资产名称、大小与 SHA256，并下载比对七份校验文件。

| 归档 | SHA256 |
| --- | --- |
| `AudioLibExplorer-v1.1.0-macos-arm64.zip` | `916ccd1d2541d9c1f817dfa9e04401de9bbb127f66c88f5b209e7f2b122a4279` |
| `AudioLibExplorer-v1.1.0-win-x64-portable.zip` | `891fd8deb3710b7c5d4386170e142adf4235d57ddf707959dfb93186c0bb6f93` |
| `AudioLibServer-v1.1.0-docker-linux-amd64.tar.gz` | `0c202c824c15abf7f9be3553c5eb5024b6efdbab9bf4ae0fdff954c97e68bf80` |
| `AudioLibServer-v1.1.0-docker-linux-arm64.tar.gz` | `d4a399249bb5a71bf7626593beedfbe15281bf6fa236e463ffbb0ba103c6f7ee` |
| `AudioLibServer-v1.1.0-linux-x64.tar.gz` | `b703d098023372a71907a613df05ebad7c863940e7d79148d5c3ede44c8268f9` |
| `AudioLibServer-v1.1.0-macos-arm64.tar.gz` | `dca5dc7ba66ca5d82f9589fbea98dea92afd6de1e6b68ac71e4497080e463538` |
| `AudioLibServer-v1.1.0-win-x64.zip` | `a23de978843b8749d50067bfde1144345e91a3771394399e91fa7b139d24136c` |

发布后将 Windows 归档的 906 个条目与已验收的便携目录逐一核对。固定 `local_release/` 再次通过 908 项 SHA256 核验、CLI 1.1.0 和隔离桌面启动，耗时 27.7 秒，复制 0 项。
