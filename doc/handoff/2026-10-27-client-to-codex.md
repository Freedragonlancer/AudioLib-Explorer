# AudioLib Explorer — 客户端 → Codex 交接（2026-10-27）

## 背景
Paul：Grok Bot 额度紧张，开发组收尾后交 Codex 继续。本文件是 **Windows 客户端** 侧交接。

## 仓库 / 基线
- GitHub: Freedragonlancer/AudioLib-Explorer
- 开发合入目标：**beta**（不直接推 main）
- beta tip（交接时）：约 `1b411215`（含本地 album artwork / schema v7 / #133）
- main / 正式 Release：**v0.7.8**（tag 仍在；**v0.8.0 Release 已撤**，tag 可能仍留）
- 本地 Win 根：`X:\AI_Build\AudioLib_Explorer_oss`（machine Dragonpark-Home）
- local_release 留有 0.8.0 包供复现：zip sha256 `1D34DAD22777F3A5A29AD6AC32C86054C37E46330039EBB41D4693BC027635E8`

## 紧急：0.8 远程播放回归（Debug 主修，勿与封面混 PR）
- 现象：0.8 客户端连仍为 0.7.8/schema6 的 8790 时，服务器模式预览/播放异常。
- 服务端对照：stream/preview 0.7.8→0.8 **未改**；像客户端回归。
- 优先假设：schema6 快照被 0.8 误走 `upgrade_owned_snapshot`/拷贝只读临时库；其次 `playable_path`→`RemoteClient::audio_source`→`StreamSource`。
- #97 / 8790 生产升级：**不动**（除非 Paul 点头）。

## 进行中：服务器模式封面（一期）
已定稿（Paul 点头）：
- Capability 名锁死：**`album_artwork`**（rev≥1）
- API：`GET /api/v1/audio/{id}/artwork`（可选 `?side=`）；404=无图；像素不进 snapshot
- 客户端：有 capability 才请求；无标/404 → 空白降级；**本地** `queue_artwork`/`publish_albums`/`probe_with_artwork` 路径不动
- 侧栏相册封面 = **二期**
- 禁止：为封面 Range/seek 远程 stream 尾部抽 ID3

客户端实现分支（CloudAgent）：`codex/handoff-remote-album-artwork`（或同名 PR→beta）；agent https://cursor.com/agents/bc-70a2c289-5c89-559d-b585-c116a6b80746
服务端同步实现 `album_artwork`（另一 CloudAgent）。

## Codex 建议下一步
1. 拉 `origin/beta` + 检查是否已有 `codex/handoff-remote-album-artwork` / 对应 PR；未完成则接上客户端 remote `queue_artwork` HTTP 路径。
2. 与服务端 PR 联调 capability 名必须同为 `album_artwork`。
3. 播放回归另开 PR，交给/对齐 Debug 假设，勿塞进封面 PR。
4. 验证：`cargo fmt`、`clippy -D warnings`、相关 test；Win 构建注意 `RUSTUP_TOOLCHAIN=1.97.1-x86_64-pc-windows-msvc`。

## 不做
- 不升 8790、不动 #97、不直接推 main、不重开 v0.8.0 Release（除非 Paul 下令）。