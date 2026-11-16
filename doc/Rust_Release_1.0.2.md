# 1.0.2 发布记录

程序 1.0.2 | API /api/v1/ | 本地写 schema 8 | 服务端迁移 schema 9 | 快照兼容 3..=9

[v1.0.2](https://github.com/Freedragonlancer/AudioLib-Explorer/releases/tag/v1.0.2) 于 2026-11-16 02:47:34 UTC（北京时间 2026-11-16 10:47:34）正式发布。七份正式产物均来自最终 main/tag `702366c391d714ea197787e87cd7e25eaf7808b3`，tag object 为 `d72502fc4d5cf763892183cee7e3291c92089876`。十四项实际线上资产的精确名称、大小、上传状态与 SHA256，以及七份下载校验文件的原始字节和正文核验通过。本文的后续文档提交不改变这些二进制身份。

## 范围、审查与兼容

以已发布 `v1.0.1` 为基线的 PATCH。API、普通本地写 schema 8、专用服务端写 schema 9、快照 `3..=9` 与 base/af1 完整格式沿用 1.0.1，不新增资料库迁移。

初始审查候选 beta `944bdf3` 包含 [#314](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/314) 的资料库、播放和远程安全修正，以及 [#315](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/315) 的主题、缩放和键盘处理。独立审查发现 IPC Load 当前选中素材会取消在途 Waveform 却不补发；[#316](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/316) 保留过期任务取消，并按新的播放代际补发渲染，增加真实解码与 Slint 排队发布回归。[#318](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/318) 仅修正便携包中的文档链接，最后由 [#317](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/317) 从 beta 晋升 main。

用户另报官方 OpenAI 地址下的通用配置错误。`https://api.openai.com/v1` 本身有效；已复现密钥末尾剪贴板换行会使各入口在发送前失败，现清理首尾空白、继续拒绝内部控制字符，并返回不含配置值的字段错误。端点／协议改变后的凭据隔离保持不变。官方 Chat 请求使用 [OpenAI API 的 `max_completion_tokens`](https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create)，其他供应商保留原参数。没有持有该次失败的实际草稿或密钥，不能断言这就是用户当时的唯一原因；本轮未执行真实账户调用。

其余范围包括大音频回调分块、Seek／EOF／预缓冲边界、HTTP 请求的资料库身份、令牌跨实例锁与重载、厂商导入全程管理租约、Unicode 空白标签判断、URL 路径前缀大小写、三个主题的布局／缩放／输入，以及被占用中断后的受管缓存清理恢复。Metadata、标注和源音频保留。

## 源码验证与复用

Windows Rust 1.97.1 MSVC、客户端使用校验过的 Steinberg ASIO SDK 2.3.4；macOS 为原生 arm64；Linux server 使用 no-default-features。以下是所选命令的组合证据，忽略入口、嵌套子进程与重跑不重复计为新通过，不代表执行了一次全工作区套件。

| 本轮选择范围 | passed | ignored | 证据边界 |
| --- | ---: | ---: | --- |
| Windows 新增 IPC／AI／直接消费者 | 114 | 39 | `87a994e`，最终格式、desktop/AI ASIO 与 remote/server no-default strict Clippy 通过 |
| Mac 新增 AI／桌面 AI／IPC／server relay | 105 | 39 | `87a994e`，55 + 37 + 4 + 9；直接消费者 strict Clippy 通过 |
| Mac 原生安全／导入／键盘与布局 | 66 | 1 | `944bdf3`；相关输入未被后续 IPC／AI 补修改变，复用原身份；与上一行合计 171 / 40 |
| Linux arm64 所选安全／导入／请求身份及新增消费者 | 199 | 1 | 保留各项原源码身份；当前候选 strict Clippy 通过 |
| Linux x64 所选安全／导入／请求身份 | 126 | 1 | ARM64 主机上的 x64 仿真；新增消费者按相同运行输入复用已有证据，并补当前 strict Clippy |

原 Windows 开发基线见 [Rust 验证报告](Rust_Validation.md) 的 2026-11-16 两节：core/db/audio/remote/server/desktop 组合为 897 passed／38 ignored，UI 组合为 192 passed／2 ignored。原失败和修复后的通过记录分别保留；这些原基线不计入上表的新通过数。当前补修未改变库管理、扫描排他、Metadata 写入顺序、租约和关闭契约，因此保留相应 library-management／review-fixes、源 WAV 保留及更新器的原通过身份。

Mac 首次 libclang 环境缺失及一个零命中测试过滤器均曾失败，分别在进程内修正构建环境与过滤器后通过，没有改全局设置、放宽断言或把原失败改成成功。Windows/Mac/Linux 的最终 main/tag 应用输入与已验证候选相符；源码检查复用不替代七份最终产物从最终身份构建和验收。

## beta 与性能

正式 beta refresh 的 health 为 ok、程序 1.0.2、提交 `0eecf42`；配置根探测 1/1 通过。Windows 候选的 20 项 playback/IPC、portable 验收、beta 安装 EXE 身份与 10 项完整 smoke 通过。完整同步 909,342 条，授权范围内 20/20 流样本通过。

其中 80,562 条在当前配置根内，828,780 条既有记录在根外，missing=0 且保留不变。此警告已核对，不能据此声称全库均在配置根内或所有记录均可流读取。文档链接修正后的晋升 beta `8a2d129` 与已验收 runtime `0eecf42` 的运行输入一致，未为文档变更重跑整套 beta。

百万条合成资料库、100 次查询在 Apple M4／OrbStack ARM64 Linux VM、容器限 2 CPU 下，缓存感知 P50 2.480 ms、P95 102.350 ms；首轮 P95 377.903 ms。tagged-only 首轮 51.886 ms、重复 P95 51.643 ms。未清空 OS 缓存，不能称为冷盘测试，也不能代替 Windows 或真实工作库测量。性能目标仍为 **P95 latency <500ms for 1M+ records**。

## 最终产物、安装与维护

Windows 客户端／服务端、macOS 客户端／服务端、Linux x64 服务端及两个 Docker 镜像均已从最终 main/tag 构建、打包并通过对应实际包与启动验收。Docker 使用已验收的 Linux 原生产物；独立 Linux arm64 服务端归档仅作为 Docker 输入，不公开分发。Mac 客户端的结构与原生启动验收在同一次 package-accept 中完成。

Windows 发布前已将经 portable 验收的 payload 安装至同一 `local_release/`，核对所有包内文件、两个 EXE、CLI 1.0.2 与隔离启动。发布后同一固定 `local_release/` 已同步核验：908 个包文件与两个 EXE 哈希、CLI 1.0.2 及隔离启动通过，copied=0，正常工作数据保留。 日常数据仍在原数据目录，资料库、Metadata、标注、扫描历史和设置没有被安装夹具替换；本次没有部署生产服务。

所有相关构建结束后，各自已核验归属的本轮 target 统一执行一次 `Intermediate -MaxGiB 0`，最终 EXE／库／PDB、SDK、正式归档、安装和工作数据保留：

| 本轮已验证范围 | 删除逻辑字节 | 保留逻辑字节 |
| --- | ---: | ---: |
| Windows target | 31,305,126,892 | 572,640,972 |
| Mac target | 51,895,617,205 | 314,416,063 |
| Linux 两架构 target 合计 | 7,784,482,598 | 404,214,087 |

以上是逻辑空间估计，不等于物理空闲增量；未扩大到其他项目、历史 target、主机缓存或未知归属目录。后续文档收尾不重建、不重新清理，不移动标签。

Mac ad-hoc 签名不是 Apple 公证；Linux x64／Docker amd64 在 ARM64 OrbStack 上仿真，arm64 在 ARM64 Linux VM 运行。隔离 Slint／原生启动结果不等于物理键盘、不同系统 DPI、真实 ASIO 设备或声学起音延迟验收；这些与真实供应商账户均未新增验收。

## 七份正式归档的 SHA256

| 归档 | SHA256 |
| --- | --- |
| `AudioLibExplorer-v1.0.2-win-x64-portable.zip` | `03b28b106d3caebeb548b66dd21a6468084ab569a76305f76e9f89230bae03ba` |
| `AudioLibExplorer-v1.0.2-macos-arm64.zip` | `b1619b614182c345c4be6f57b9532813d16e5fd57e6898ad6efbe59288555f4a` |
| `AudioLibServer-v1.0.2-macos-arm64.tar.gz` | `38f84e838fe64da50137fac0074b66c176182a602d04fe498ac0a1d6ae79b11f` |
| `AudioLibServer-v1.0.2-linux-x64.tar.gz` | `7ec058c0381c64de352f629fb27a5162325a3d30a95d3bed38d2a3097d2cd958` |
| `AudioLibServer-v1.0.2-docker-linux-amd64.tar.gz` | `324710f8ff0fc74f94dd559f782c7350d7fb73a1a609043aa133e1a38a618621` |
| `AudioLibServer-v1.0.2-docker-linux-arm64.tar.gz` | `a229b931046026d3441c1674eeab5c0a01f581f78569c799e30b8fa96fbb4a8e` |
| `AudioLibServer-v1.0.2-win-x64.zip` | `afea7425583e5a9b990204587cfaf235bdc62c3e93ba4a5bc5ae807199a37625` |

每份归档同时发布同名 `.sha256`，共十四项资产。线上核验使用实际 Release 元数据和下载的校验文件，不把预发布本地合同验证当作已发布证明；原始收据和私有索引留在本地，不随发布资产上传。
