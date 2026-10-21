# GUI 数据库下载 / 解压进度：重做交接

> 历史交接记录：下文描述当时作废开发单的需求和边界。后续进度分离、快照续传和恢复已进入 main（如 `df4ee20`）；本文不作为当前待办清单。当前契约见 [Rust Remote Mode](Rust_Remote_Mode.md)。

本开发单已由用户废弃，交由 grok 在最新开发基线上重新实现。本文只交接需求、定位和已发现的坑，不要求复用旧分支或照搬旧实现。

## 1. 要修复的行为

用户示例：数据库原始大小 **19,495,989,248 字节 = 18592.8 MiB**，zstd 压缩传输约 **1.46 GB**。旧 GUI 却显示“下载 xxxx / 18592.8 MiB”，把解压后的数据库总量当成下载总量。

目标：音效库管理的**连接页、缓存/导出页**都分别显示下载和解压的文字、进度条。下载总量取实际压缩响应体长度，18592.8 MiB 属于解压总量。示例压缩大小不能硬编码，统一按实际字节换算 MiB（除以 1,048,576）。

## 2. 根因与计数位置

旧 `RemoteClient::download_snapshot` 的回调只有一组计数；循环读取的是 zstd 解码后的数据，GUI 却统一标成下载。

| 计数 | 应累计的位置 | 总量来源 |
| --- | --- | --- |
| 下载 `received` | HTTP 响应体 reader，位于 zstd decoder **下层** | `Content-Length`，压缩响应体字节 |
| 解压 `written` | decoder 输出成功写入暂存数据库之后 | `X-Uncompressed-Length`，原始数据库字节 |

必须保留流式下载和解压。两个过程会重叠，不能假定下载完成后才开始解压，也不需要新增整份压缩文件落盘步骤。预分配可能调用 `set_len`，因此不能用文件长度充当已解压量。

## 3. 最小实现要点

1. 传输回调提供独立事件，例如：

   ```rust
   enum SnapshotProgress {
       Downloading { received: u64, total: Option<u64> },
       Decompressing { written: u64, total: Option<u64> },
   }
   ```

2. 在 `sync_library` 的进度层转发两类事件。`SnapshotMeta.bytes`、缓存 `snapshot_bytes` 仍表示数据库原始字节数，不能改成网络传输量。
3. GUI 保存两组最新计数，下载到 100% 后继续显示尚未完成的解压进度。已有 reporter 内完成即可，不必照搬旧实现的独立辅助模块。
4. 两个总量各自可未知；未知时显示已处理字节和不定进度条，不能拿另一组总量补齐。普通未压缩响应只显示下载。
5. 开始新任务、重试、切换 provider、结束、取消、失败及关闭恢复路径都要清理旧的解压显示。保留已有 generation 检查，并在 Slint 事件循环更新界面。
6. 保留现有取消、长度校验、暂存文件清理和成功后发布缓存的顺序；失败不得替换活动缓存。没有数据库 schema 或服务端协议变更需求。

## 4. 已发现的三个坑

- **共享节流窗口**：下载与解压事件逐块交替；必须先更新两组内部计数，再按同一个约 200 ms 窗口刷新 GUI。不能因事件种类交替就每块投递一次。
- **重要变化立即显示**：第一次收到已知下载总量、第一次出现解压、首次达到完成量，应绕过节流。否则服务器发完响应头后暂停发送正文时，界面会一直停在“服务器准备中”。
- **越界只触发一次完成刷新**：错误响应头可能声明过小的解压总量；从 `< total` 跨到 `>= total` 时立即刷新一次，后续超过总量的块仍须节流。不能对每个 `written >= total` 的块都绕过节流。长度不匹配仍交给原有错误处理。进度条比例限制在 `[0, 1]`。

补充：每次 decoder 读取结束后都读取网络累计量，**包括 EOF 那次**，避免漏掉最后消费的 zstd 帧字节。Slint 新增嵌套 `VerticalLayout` 要限制纵向拉伸，避免两行进度之间出现大空白。

## 5. 文件定位

| 文件 | 关注点 |
| --- | --- |
| [client.rs](../crates/audiolib-remote/src/client.rs) | `SnapshotProgress`、`RemoteClient::download_snapshot`、`Counted` reader、EOF 回调 |
| [sync.rs](../crates/audiolib-remote/src/sync.rs) | `SyncProgress`、`sync_library` 的转发及缓存原始大小语义 |
| [lib.rs](../crates/audiolib-remote/src/lib.rs) | 若沿用新事件类型，补齐公开导出 |
| [remote_mode.rs](../app/audiolib-desktop/src/remote_mode.rs) | 同步 worker 回调、事件合并、GUI 投递、开始与结束清理 |
| [remote_sync_progress.rs](../app/audiolib-desktop/src/remote_sync_progress.rs) | 旧实现参考：双计数、显示格式、节流、头部和越界回归；接手时可按最新结构重写 |
| [app-window.slint](../app/audiolib-desktop/ui/app-window.slint) | 连接页及缓存/导出页；旧属性名为 `ds-decompression-info` / `ds-decompression-progress` |
| [controller.rs](../app/audiolib-desktop/src/controller.rs) | 缓存 provider 打开、关闭失败恢复时的状态清理 |
| [scale-probe.rs](../crates/audiolib-remote/examples/scale-probe.rs) | 补齐进度 enum 匹配；下载和解压应归入同一传输计时段，避免逐块追加计时记录 |

旧实现还改过 [test_server.rs](../crates/audiolib-remote/src/test_server.rs) 的未知 Content-Length fixture，以及 [startup.rs](../app/audiolib-desktop/src/startup.rs) 的截图 fixture。截图矩阵无需照搬扩展。

## 6. 与其他开发单集成时的注意点

- 搜索升级相关任务会扩展 `CachePreparation`（例如 `Tokenizing`），并可能改变 `StartupProgress::label()` 的返回类型。保留最新基线已有的准备/升级阶段，只拆分传输计数，避免把旧版本的 match 整块覆盖回来。
- 旧开发分支与当时 main 的 API 不一致，曾分别适配 `Tokenizing` 和 `String` / `&str`；这只是旧基线差异，重做时以当前代码为准。
- 本次需求不涉及搜索存储 B、数据库体积优化、列表 Waveform、迁移、版本发布或 CI 工作流。

旧提交仅供定位：

- 开发集成版：`aecd59b0f376d992940bea4d592407eec1ebd122`
- 当时 main 的最小适配：`d479a2c3d02a5e286e5d09dc939d597854d38aab`
- 曾通过 [PR #16](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/16) 合并：`88d8bf39a086d2040927f48bb841e144cfa1099f`

这些记录不代表当前冲突已解决。重做前检查目标分支是否仍含旧修改，避免重复增加 enum、属性或进度行；根据最新代码局部实现，不整分支合并。

## 7. 小范围验收

- 用小体积、高压缩率的 fixture 确认下载累计量等于压缩体长度、解压累计量等于原始数据长度，无需创建 19 GB 测试库。
- 检查下载完成但解压未完成时，两行仍正确；总量缺失时独立显示，普通响应没有解压行。
- 验证“首次总量立即显示”和“连续越界仍受节流”两个边界；复用已有取消保护活动缓存的用例。
- 编译涉及的进度 enum 调用点，快速查看两个面板排版即可。用户已明确要求跳过不必要的重复测试，不扩展百万条基准、迁移/备份恢复回归、ASIO 全套测试或多轮截图矩阵。

本交接为文档，不执行新的代码修改、测试、构建、安装、回滚或推送。
