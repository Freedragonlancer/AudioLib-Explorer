# AudioLib Explorer v0.4.1

发布日期：2026-10-12

版本：程序 0.4.1 | API /api/v1/ | schema 5 | 快照兼容 3..=5

## 应用图标与高 DPI 资源

- 新增现代扁平风格的应用图标：叠放的音频素材文件夹与 Waveform，深灰和青绿色配色，透明外边缘。
- Windows EXE 嵌入 16–256 px 的 16 个图标尺寸，覆盖常见 100%–400% 缩放；窗口图标一并接入，保留 PerMonitorV2 DPI 设置。
- 提供 macOS `.icns`、标准 `.iconset` 及最大 1024×1024 的 Retina 图层，并配置 cargo-bundle 图标元数据。此版本发布 Windows x64 便携包；macOS 桌面构建、签名及实机显示尚未验证。
- 中英文 README 展示新图标；附原始图稿及可重复生成各尺寸的 PowerShell 工具。日常运行及 Cargo 构建不需要图像服务或 Python。

API、数据库 schema 和快照兼容范围与 v0.4.0 一致。升级保留资料库、Metadata、标注、扫描历史与偏好设置，运行状态继续保存在 `%LOCALAPPDATA%/AudioLibExplorer/v1` 或原有显式配置路径。

Windows x64 便携包包含 WASAPI 与 ASIO。校验结果随 Release 的 `Rust_CI_Release.json` 提供；完整的 macOS/Windows 图标接入说明见仓库 `doc/Rust_App_Icon.md`。
