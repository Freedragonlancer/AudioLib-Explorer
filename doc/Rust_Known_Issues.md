# Rust 客户端待解决问题

## UI-001：失焦后重新聚焦，界面内容丢失

- 登记日期：2026-10-12。
- 状态：0.4.4 已修复重绘逻辑，复用 buffer 回归通过；完整前台失焦/聚焦验收待桌面解锁后补充。
- 用户报告：软件失去焦点后，再次聚焦时 UI 大面积消失或变黑，仅部分按钮和局部控件仍然可见。报告截图处于音效库管理的服务器设置界面，原因为待查，尚不能认定数据或窗口状态丢失。
- 复现线索：打开音效库管理 → 切到其他窗口 → 返回 AudioLib Explorer。出现频率、最小化/遮挡是否必要、缩放与显示器组合尚待确认。
- 修复：Slint 1.17.1 软件 renderer 可能按 buffer age 复用已被 Windows 清空的表面，而 Windows 不保证发送 Occluded 通知。Windows 原生 RedrawRequested 标记完整逻辑窗口，恢复事件请求重绘；保留原组件、草稿与选择，无定时重绘。
- 验证：确定性回归先验证 request_redraw 单独使用不能恢复清空的复用 buffer，再验证完整损伤恢复整帧、表单和选择保持、空闲不重绘。新 ASIO release 的自有窗口 DC 抓帧在 192 DPI、1094×630 client 下完成失焦/遮挡/最小化各三次操作，像素保持。
- 实机限制：当次 Windows 会话锁定，input desktop 不可访问；自有窗口表面检查不代表前台焦点和真实暴露转换已验收。工具明确记录 `foreground_exposure_verified=false`，未使用会触发重绘的 PrintWindow 来替代验收。解锁后运行 `tools/validate-window-repaint.ps1 -BinaryDirectory local_release` 补充实际前台转换。
- 验收：主窗口和音效库管理在反复失焦/聚焦、遮挡恢复、最小化/还原后都能完整显示；覆盖实际 DPI 缩放，且草稿和选择不丢失。

原始现象截图包含私人库信息，保留在本机忽略的审计目录，不随源码或发布包分发。
