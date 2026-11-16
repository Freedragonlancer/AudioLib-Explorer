# 1.0.2 发布记录

程序 1.0.2 | API /api/v1/ | 本地写 schema 8 | 服务端迁移 schema 9 | 快照兼容 3..=9

本记录在 2026-11-16 开始发布准备，以正式版 `v1.0.1` 为基线。当前尚未发布，最终 main/tag、七份正式归档、十四项线上资产与固定安装状态须由完成后的实际收据补齐。API、资料库写目标、快照读取范围及完整格式保持 1.0.1 的兼容边界，不新增资料库迁移。

## 范围与审查

审查候选 beta 为 `944bdf3e7e8f27e3631c6e1fd8e314e099ea49cb`，包含 #314 的资料库、播放和远程安全修正，以及 #315 的主题、缩放和键盘处理。独立审查覆盖 db/core/server、audio/remote/IPC 和 UI。发现 IPC Load 当前选中素材时会取消 Waveform 而不补发渲染；发布准备增加按新播放代际补发的修复和确定性真实渲染回归，继续拒绝旧素材的排队发布。

用户另报官方 OpenAI 地址下的配置或输入错误。`https://api.openai.com/v1` 本身有效；代码确认粘贴密钥的末尾换行会在各入口发送请求前被拒绝，现清理首尾空白、继续拒绝内部控制字符。增加不含配置值的字段错误，并保留端点／协议改变后的凭据隔离。官方 Chat 请求使用 [OpenAI API 的 `max_completion_tokens`](https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create)，其他供应商保持原参数。不持有该次失败的实际草稿或密钥，不能据此断言用户当时唯一原因；未执行真实付费 API 请求。

## 验证选择与复用

原始 Windows 开发基线见 [Rust 验证报告](Rust_Validation.md) 的 2026-11-16 两节：core/db/audio/remote/server/desktop 相关组合为 897 passed、38 ignored；UI 组合为 192 passed、2 ignored。保留原失败、修复后定向通过及其源码、feature、工具链身份，忽略项不计为通过；不把这些不同组合简单合并为一次新套件。

本次新增 IPC／AI 修复重新选择相关 package 的严格 Clippy、AI lib、桌面 IPC／AI 交互及直接消费者；改变后的播放路径使用当前 EXE 执行 playback-ipc。未变化的库管理、扫描排他、Metadata 写入和更新器沿用原通过基线，因为本次补修未改变相应存储、写入顺序、租约和关闭契约。库管理／review-fixes 的原合成验收与源 WAV 保留证据仍见原记录。

Mac 和 Linux 按受影响的 Unix 锁、Token、导入管理租约与请求身份执行定向检查。每份正式 payload 仍须从最终 main/tag 构建并在对应平台验收一次；beta 二进制不充当正式产物。

## 交付边界

正式资产合同为七份归档及七份 `.sha256`：Windows x64 客户端和服务端、macOS arm64 客户端和服务端、Linux x64 服务端，以及 Docker linux/amd64 和 linux/arm64。Linux arm64 原生服务端只作为 Docker 输入。Windows 固定安装为仓库原有 `local_release/`，与工作数据目录独立。

Mac ad-hoc 签名不是 Apple 公证；Linux x64／Docker amd64 在 ARM64 OrbStack 上仿真运行，arm64 在 ARM64 Linux VM 中运行。原生 GUI、真实 ASIO 设备、声学起音或真实供应商账户的覆盖按完成收据分别记录。性能目标为 **P95 latency <500ms for 1M+ records**，实际测量条件和数值与目标分开报告。
