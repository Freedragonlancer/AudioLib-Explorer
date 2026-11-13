# Beta P2 兼容修复（2026-11-13）

基于原 review 基线 beta `ab9c0d7` 修复 #292 引入的两项问题：试听标记在 UI 线程访问文件系统，以及远程试听等待首包时失焦仍可能晚到播放。补丁已同步到包含 #297 的 beta `3ad9674`，并重跑受影响组合的检查。

## 试听标记与库身份

UI 装饰行和发布试听标记只读取内存中的已解析身份。路径 canonicalization 留在后台 worker，缓存锁不覆盖文件系统访问；成功且仍有效的搜索在发布结果前准备所需身份。

本地缓存按打开的 provider 实例保存 Weak key，不保留数据库强引用；重新打开同一路径或只读别名会重新解析 canonical history ID。远程缓存使用服务器 library ID，继续与本地身份分开。标记仍按原 canonical history ID、音频 ID 和路径保存，因此 reader generation 刷新、本地/服务器切换和返回同一资料库会保留正确标记。过期 worker 的 source/library generation 和 provider 身份校验保留。

## 失焦取消与 Play 确认

检测到离开应用时，在已有选择 admission lock 内检查引擎状态和当前 generation 的待播放请求，再使用与手动 Stop 相同的取消步骤：使选择失效、清空待执行选择、设置远程取消标志、打断流读取并提交 Stop。

Play 提交后继续按既有约定消费 `latest_selection`，防止切库恢复重播已提交的试听。另行保留实际 Play 的确认接收器，覆盖引擎仍为 Ready 的入队窗口；只在收到终态结果后移除，新的行选择不能将已提交的 Play 误当作取消。提交时和状态轮询均非阻塞回收已完成确认，失焦检查先回收确认、再读取最新引擎状态；成功排队的 Stop 接管剩余请求。关闭选项、缺少失焦边沿、仅浏览且没有已提交 Play 的选择不触发新的取消。

## 验证范围

本轮只改变桌面控制层及其测试，未改变数据库格式、HTTP API、音频引擎、IPC 协议、配置存储或 Metadata 写入队列。

- Windows x64、Rust 1.97.1 MSVC、桌面 ASIO；通过 preparation 工具核验 Steinberg SDK 2.3.4。
- 格式检查、桌面全部 targets 严格 Clippy：通过。首轮发现的多余导入已修正，原失败日志保留。
- 身份缓存、失焦、历史隔离、Stop、源恢复与确认取消定向测试：**17 项通过，0 失败**。其中 2 项身份缓存测试按下述未变条件复用同轮结果；isolated child 由 parent 调用，不重复计数，也不把 ignored helper 计为通过。
- 新测试包含受控阻塞解析与并发缓存读取、冷缓存 UI 不解析路径、只读别名重新打开和源隔离，以及真实 loopback 首包等待后失焦与晚到响应。Ready 回归通过真实 Load/Play，在实际 Ready 状态的解码重新打开阶段设置门禁，再选择另一行并失焦，覆盖已提交 Play 跨选择 generation 的生命周期。
- 初轮测试对缓存注销和解码打开次数的 fixture 假设已修正：Stop 可以保留可复用缓存，解码门禁按真实 Ready 状态触发；无晚到 Load、文件或播放会话的断言保留。原失败记录保留，身份缓存 2 项在源码、测试与环境未变时复用本轮通过结果。
- 使用桌面/CLI ASIO 隔离开发构建执行 `validate-playback-ipc.ps1 -CleanCheckout`：**最终候选 20 项通过**，包括 3 次独立播放会话、暂停/seek 保持会话、客户端本地历史与源 WAV 哈希保护；仅使用 owned 合成静音 WAV 和临时设置/数据库。首次 Windows 音频客户端激活出现 `RPC_E_CHANGED_MODE`；同机已验收 1.0 release、同配置未修改 beta `3ad9674` 和源码/EXE 校验还原后的最终 P2 候选均通过隔离对照。原失败保留，原因未确认，未声称后端问题已修复，也未改设备配置。
- 原 review 在 `ab9c0d7` 的 core 设置兼容 2 项和 Windows 单元 27 项基线仍适用：本补丁未修改其源码、契约、特性或平台假设。AI 与 RIFF 模块继续复用 [1.0.0 发布记录](../Rust_Release_1.0.0.md) 的未变基线；不重复 workspace、百万记录或无关管理/Metadata 套件。

私有执行日志位于本轮 checkout 的 `.rust-validation/p2-fixes/`。成功验证后对本 worktree 的 owned target 执行 40 GiB Incremental 缓存维护；首轮发现新的编译器进程而受阻，保留剩余候选、不终止进程，确认占用结束后续跑通过。最终逻辑估算 **11.34 GiB／40 GiB**、预算达标，本轮合计移除约 **30.52 逻辑 GiB**，不是实际磁盘回收量。首轮和最终收据为 `cache-maintenance.json`、`cache-maintenance-resume.json`；release 输出、已编译依赖、正式证据和未登记历史缓存保留。

本次是开发修复，不部署服务、发布新版本或更新 `local_release/`。真实 NAS 与原生前台切换未作端到端验收；回归通过受控首包等待和同一个生产失焦 handler 验证取消顺序。未新增 ASIO 硬件、声学延迟或百万记录测量，性能目标仍为 **P95 latency <500ms for 1M+ records**。
