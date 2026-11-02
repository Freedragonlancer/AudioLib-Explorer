# 0.8.7 发布准备跟踪

记录日期：2026-11-02。本文记录范围整理与后续门禁；“准备完成”不表示候选或正式产物已验收。

## 身份与范围

| 项目 | 本轮基准 |
|---|---|
| 已发布基线 | [v0.8.6](https://github.com/Freedragonlancer/AudioLib-Explorer/releases/tag/v0.8.6)，提交 `9b883f9c6c35886682ec3e1df1dbd40720c25339` |
| 整理时 beta | `20455819ea15b642a33f779f6d83abde60a13d36` |
| 目标程序版本 | 0.8.7，PATCH；最终候选提交待确定 |
| API / schema | `/api/v1/` / 8；快照兼容 `3..=8` |
| 完整格式 | `ale-s8-cld1-t3-m2-p1-a1`，无新增数据库迁移 |
| 正式资产 | 七个归档及各自 SHA256，共十四项，沿用现有命名契约 |

本轮已合入 beta 的主要范围：

- 恢复原 Default 外观，注册可选 Studio 工作台；主题切换保留共享播放、Waveform 与浏览状态。
- 修正 Studio 输入、静音确认、视图恢复与过期回调的竞态，保留原生键盘浏览行为。
- 修复 Windows Common Controls 加载依赖。
- 缩小播放重绘区域，缓存 Studio 表面位图，保留真实窗口露出时的完整重绘；恢复细播放头并完善播放/暂停反馈。
- 本地 beta 使用开发身份并隔离自动更新；发布管线支持续跑、产物身份校验、固定安装和预算缓存维护。

这些内容来自已合并的 [#186](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/186)、
[#188](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/188)、
[#189](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/189)、
[#190](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/190)、
[#191](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/191)、
[#192](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/192)、
[#193](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/193) 和
[#187](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/187)。行为说明见[主题契约](Rust_Themes.md)与[发布说明](Rust_Release_Notes.md)。

## 暂不纳入的工作

- 原工作区尚未提交的“最多 30 个搜索任务 / 收藏夹”功能保持独立，当前按独立工作保留；如后续纳入，另行补齐其源码验证和交付，当前准备分支不包含这些改动。
- [#185](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/185) 与已合并 Studio 外观部分重叠，但并非全部被取代：其独有的分页、结果数量、侧栏显示偏好和 draft 上传 helper 延后单独评估，不直接合入冲突分支。
- [#135](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/135) / [#137](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/137) 的封面工作已由合并的 [#138](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/138) 接续；[#97](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/97) 的 marker 方向已有后续实现。这三个过时 PR 已关闭，保留分支与提交；[标记设计](api/Compatibility_Markers.md)已注明历史状态，未合入旧草稿代码。

## 已有证据与未解决门禁

以下为原 PR 的历史记录，未在本次准备中重新执行；只有相关源码、feature、工具链及环境仍相同时才可复用。

| 原始身份 | 已记录结果 | 当前限制 |
|---|---|---|
| #193，集成提交 `db5b31e` | 17 项最终定向回归、格式与 desktop strict Clippy；默认 ASIO 构建、portable、播放/IPC、固定与 beta 安装通过 | 早于 #187，不能用其二进制充当最终 0.8.7 payload |
| #187，提交 `6506013` | Windows desktop 469 passed / 4 failed；Windows crate 27 passed；格式、相关 Clippy、loader 测试通过 | 四项结果滚动测试仍失败；旧 beta `e3a3bde` 同样失败并不使它们成为通过 |
| #187，Windows 非 ASIO 测量 | 局部播放重绘、最小化恢复与缩放有记录 | 未执行 ASIO 打包及固定安装；锁屏使完整原生窗口露出验收中断，焦点/遮挡尚未完成 |
| v0.8.6，`9b883f9c6c35` | 正式发布及十四资产记录 | 仅证明旧版，不能替代本轮候选或最终身份 |

必须先处理四项滚动失败：`result_paging_tests::appending_rows_keeps_the_visible_content_and_other_pane_scrolls_in_place`
及 #187 报告中的三项 `mode_switch_scroll_tests`。确认具体失败、修复或提供可复核的环境归因后，重跑失败及受影响检查。
在可交互、未锁定的 Windows 桌面完成两主题的原生焦点、遮挡、恢复与重绘验证；此前锁屏记录不算通过。

## 按实际影响执行的后续计划

1. 同步 workspace 与十个本地包的锁文件版本、README、版本规则及本轮发布说明；保留旧标签和历史证据身份。
2. 运行版本预检及文档差异检查；本次准备只执行这些轻量检查和管线计划预览；没有构建、打包或更新应用安装。

   ```powershell
   pwsh tools/check-release.ps1 -Tag v0.8.7 -BaseTag v0.8.6
   git diff --check
   ```

3. 对最终候选按 [Testing Strategy](Testing_Strategy.md) 选择源码检查。覆盖 core 主题配置、desktop 的主题/输入/滚动/Waveform/重绘/更新策略、Windows 加载与露出、update 开发身份；公共接口检查相应直接消费者。Windows 客户端使用一次默认 ASIO 配置，先按[仓库规则](../AGENTS.md)准备并核验 SDK；不串行追加整套非 ASIO 矩阵。
4. 复用仍有效的配置、关闭排空和数据保护证据；配置持久化或 Metadata/扫描行为若被后续修复影响，补相应回归及 [review validator](../tools/validate-review-fixes.ps1)，关闭行为变化再选 close validator。播放与 IPC 变化使用 [playback validator](../tools/validate-playback-ipc.ps1)。不因文档提及资料库便自动扩展库管理全套。
5. 版本本身不触发全 workspace 或百万记录 benchmark。重绘风险使用 [window validator](../tools/validate-window-repaint.ps1) 的 `-BinaryDirectory` 参数及相关播放重绘测试；性能结论保留实际平台、分辨率、构建配置和采样条件。
6. release pipeline、beta 安装或包规则若无可复用同源证据，分别执行已有 synthetic 回归；不修改规则时不重复包边界测试。
7. 准备 beta 候选时使用独立 beta 数据目录和开发安装身份；由 Mac beta refresh 与 Windows [beta smoke](../tools/beta-smoke.ps1) 对同一预期提交验收。服务地址和凭据仅从本机环境提供。必要门禁失败继续阻止提升。
8. beta 通过后才按 [版本流程](Rust_Versioning.md) 提升 main。最终 main/tag 身份确定后，各原生平台构建、打包与验收各一次；Windows client 优先使用 [release pipeline](Rust_Release_Pipeline.md)，不要重复调用其内部阶段。
9. Windows 实际 payload 通过一次 portable 验收后更新固定 `local_release/`，核对全部 package-owned SHA256、CLI 版本及一次隔离启动；保留 `%LOCALAPPDATA%/AudioLibExplorer/v1` 的库、Metadata、偏好与缓存。beta 安装不能替代固定安装。
10. Mac 客户端与各服务端/Docker 包按本机计划及实际变更选择检查，记录未覆盖平台；最终正式包须具有同一发布提交身份。成功交付后执行一次 40 GiB debug 缓存预算维护，报告阻塞或未达预算。

## 当前状态

- 已完成：beta/已发布基线核对、范围划分、原始证据与缺口清单，0.8.7 版本/锁文件/发布说明同步、过时 PR 关闭及历史设计状态校正。
- 本次实测通过：`check-release.ps1 -Tag v0.8.7 -BaseTag v0.8.6`、`git diff --check`；Windows client/server、Unix server、Mac client 四种原生 release plan 的 `Plan` 路由预览通过（`source_checks=not-requested`）。计划预览不运行构建、源码测试或平台验收。
- 待完成：最终候选身份、必要源码与失败回归、未锁屏重绘、beta 两端门禁、最终多平台 payload 与固定安装。
- 本次未创建正式标签、未发布 Release、未部署 beta 服务端；未把旧构建或历史测试描述为本轮新验收。

门禁结果在实际执行后追加其提交、feature、平台与私有报告路径；不得将本文件的计划表当成通过记录。
