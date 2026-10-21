# 键盘方向键试听验收记录（单击播放）

## 现象

勾选结果列表底部的“单击播放”后，鼠标单击声音会立即试听；用 `↑` / `↓` 切换选择时，
高亮和详情面板会跟着变，文件也被载入引擎（状态停在 `Ready`），但不产生声音。

## 原因

`app/audiolib-desktop/src/controller.rs` 中 `on_selection_step_requested` 固定以
`play = false` 调用 `select_row`；鼠标路径则传入
`audition-play-on-single-click && !missing && !修饰键`。两条入口对“选中是否试听”的
判定不一致。

## 修复

方向键移动后按与单击相同的规则决定是否试听，并补充两个边界：

- 步进被 clamp 回当前行（已在首行/末行）时直接返回，避免重新 `Load` 正在播放的声音；
- 缺失记录保持静默，与单击缺失行一致。

`Shift+↑` / `Shift+↓` 仍是扩选，不试听，与 `Shift+单击` 一致；未勾选“单击播放”时
键盘选择保持静默。

## 验证

| 项目 | 结果 |
| --- | --- |
| `cargo fmt --all -- --check` | 通过 |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 通过 |
| `cargo test --workspace --all-targets --locked` | 817 passed / 0 failed / 72 ignored |
| `tools/build-release.ps1`（含 ASIO） | 通过；固定 ASIO SDK 2.3.4 的 SHA-256 校验一致 |
| `tools/package-portable.ps1 -SkipBuild` | 通过；发布隐私边界检查通过 |
| `tools/validate-portable.ps1 -CleanCheckout` | 通过（7 项检查） |
| 新增 `keyboard_selection_auditions_in_single_click_mode` | 通过；回退修复后该用例失败（`state=Ready`，已载入第二条） |
| 0.5.0 主线复验 | `cargo fmt` 通过；`cargo test -p audiolib-desktop` 226 passed；同一端到端用例通过 |

构建环境：Windows x64 / MSVC、Slint 1.17.1、ASIO SDK 2.3.4（固定哈希）。上表中的
工作区测试与便携包验收在 0.4.11 源码上完成，随后把同一修复变基到 0.5.0 主线并重新
验证桌面 crate。
