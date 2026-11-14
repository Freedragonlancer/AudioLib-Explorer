# Go DeepSeek 与六个推荐标签（2026-11-13）

基于 beta `08e3978`，修复 OpenCode Go 渠道 DeepSeek 请求的推理控制，并将当前开发版 AI 已核验推荐标签上限从 5 增至 6。已发布的 v1.0.0 产物仍为原来的 5 项，不改写历史验收记录。

## 请求与推荐行为

Go 的 Chat 兼容请求在去掉可选的 `opencode-go/` 前缀后，对 `deepseek-*` 模型发送 `reasoning_effort: "none"`，将短关键词请求的输出预算留给最终 JSON。DeepSeek 官方直连仍发送原生 `thinking: {"type":"disabled"}`；其他 Go 模型及普通 Chat 协议不附加此控制。端点、认证、稳定会话标识、原有超时和取消机制保持既有契约。

生成提示优先要求 6 个不同候选；原始候选上限仍为 12。桌面经过真实命中核验后最多保留 6 个，不足时显示实际数量。侧栏、顶部快捷标签和“查询全部”均支持第 6 项，任务批次使用同一 Rust 上限。单标签正文和独立“+”沿用当前目录及筛选条件；名额不足整批拒绝。可选的生成后自动搜索仍最多 3 项。

## 真实账户诊断

仅在内存中克隆已有 Go 配置，将本次诊断模型设为 `deepseek-v4.1-flash`；凭据只在当前用户进程内解保护后发往原配置的官方 HTTPS Go 服务。没有保存模型、地址、密钥、账户隐私设置或请求计数；配置文件前后 SHA256 相同。私有日志仅保留状态、耗时、数量和 token 统计，不保留密钥、请求正文或供应商原始回复。

- 最小修正请求：HTTP 200，约 5.25 秒，有效 JSON，推理字段内容长度为 0。
- 同一最小示例不加推理控制：HTTP 200，约 4.22 秒，有效 JSON，推理字段内容长度为 164。此对照没有重现超时，也不能用这两次耗时宣称加速或证明所有传输失败的根因。
- 最终生产 `provider::generate`、默认提示词、完整参考与 JSON 契约、`wind` 示例、原有 15 秒超时：**约 6.90 秒成功，9 个原始候选通过解析**，供应商报告输入 907、输出 405 tokens。候选数量不等于当前音库命中核验后的标签数量。使用任务内临时诊断程序，未增加自动调用真实账户的测试。

真实 Flash 请求通过；Pro 的前缀和请求控制由 loopback 回归覆盖，未作真实 Pro 调用。有关账户规则和直连协议分别见 [Go 官方说明](https://opencode.ai/docs/console/go/) 与 [DeepSeek thinking 文档](https://api-docs.deepseek.com/guides/thinking_mode/)。

## 开发验证与适用范围

Windows x64、Rust 1.97.1 MSVC。AI 后端默认特性与桌面 ASIO 分开执行，避免纯 AI 测试混入音频宿主链接特性。

- `cargo fmt --all -- --check`：通过。
- `cargo clippy -p audiolib-ai --all-targets --locked -- -D warnings`：通过。
- `cargo test -p audiolib-ai --lib --locked`：**31 项通过**。Chat 请求回归覆盖 Go 原始 Flash、带前缀 Pro、其他 Go 模型、直连 DeepSeek 和普通 Chat 同名模型，校验各自独立的推理参数与规范化路径、头部、解析。
- `cargo clippy -p audiolib-desktop --all-targets --locked --features asio -- -D warnings`：通过；准备脚本核验固定 Steinberg SDK 2.3.4。
- 桌面 ASIO 定向测试：**4 项通过**，覆盖 7 个均有实际命中的候选截断为 6、单词推荐优先、第 6 项搜索和“+”新任务、全部 6 项批量搜索、自动仍为 3 项、任务容量不足整批拒绝、无效筛选拒绝，以及目录过滤后的推荐复用。
- Slint 软件渲染和真实控件回调检查通过，确认第 6 个快捷标签及“+”动作可用；目视检查 Classic 顶栏及 Studio 侧栏截图，6 个标签无重叠。短窗口与缩放状态沿用同一渲染回归中的交互和几何断言。截图在 `screenshots/ai-keyword-tags-classic.png`、`screenshots/ai-sidebar-studio.png` 等本轮私有输出内。
- `git diff --check`：通过；文档链接与新旧版本表述已检查，2026-11-12 首次交付记录及其后的历史记录未改写。

本轮不改变 core 查询 DTO、数据库/扫描/Metadata、配置写入、音频后端、IPC 或远程资料库契约。复用 [同日 beta P2 验证](Beta_P2_Fixes_2026-11-13.md) 中未变的 core 设置、Windows、播放/IPC 基线，以及 [1.0.0 发布记录](../Rust_Release_1.0.0.md) 中未变的存储与音频基线；本轮只重新验证 AI 与桌面消费者，不扩展到 workspace、真实音库或百万记录性能验收。CLI/服务端不暴露 AI，保持既有验证范围。

执行证据在本轮 checkout 的 `.rust-validation/go-six/`，包括后端与桌面检查、渲染图和仅含安全统计的真实服务诊断摘要。

成功验证后，对本 worktree 默认 owned target 执行一次 40 GiB Incremental 缓存维护：通过，逻辑估算剩余 **21.39 GiB／40 GiB**、预算达标，无需删除。收据为 `cache-maintenance.json`；release 输出和未登记的历史缓存保留。本次是开发修复，不构建发布包、不部署 beta 服务，也不更新 `local_release/`。
