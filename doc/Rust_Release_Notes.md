# AudioLib Explorer v0.4.0

发布日期：2026-10-11

版本：程序 0.4.0 | API /api/v1/ | schema 5 | 快照兼容 3..=5

## 资料库 schema v5

- 提取标签（源 WAV Metadata 中的 keywords/category/subcategory 等）并入列表「标签」列，并参与「仅有标签」「标签包含」筛选与全文搜索；源标签与用户标签保持独立，不写入 user_tags，不修改源 WAV。
- 移除评分：UI、CLI、核心类型与服务器 API 均不再提供评分功能。数据库中的评分列及既有数据保留，不删除；v5 仅删除评分相关索引。旧客户端的评分写入请求在服务器端按兼容处理（200 空操作，评分排序与筛选被忽略）。
- 新增派生列 `audio_files.source_tags_text`（提取标签小写拼接）和覆盖部分索引 `idx_audio_visible_tagged_any`；筛选不再解析 metadata_json。
- 快照兼容范围 3..=5：客户端接受 v3/v4/v5 快照，原快照保持只读；需要修复稳定 ID、搜索派生文本或基础索引时，只在一致性备份所得 owned 副本升级并校验，失败保留原缓存。没有派生标签列的快照仍可回退到 JSON 路径。

## 升级与迁移（服务器运维须知）

- 用新版本打开 v3/v4 资料库会自动添加列和索引，但回填不是瞬时的。真实 90 万行资料库实测：标签回填约 3 分钟（198 秒），全文搜索（FTS）重建约 8.5 分钟（513 秒）。
- CLI 与服务器维护可运行 `AudioLibCli db rebuild-search`（先回填标签再重建 FTS，进度输出到 stderr，JSON 结果含各步耗时；`--if-needed` 在已是最新时跳过）。桌面启动准备工作在线程内完成必要的派生升级，窗口显示等待状态并持续响应，关闭会等待工作安全完成。
- FTS 重建经由维护路径，会在资料库目录留下与资料库同样大的 `audiolib-rollback-*.db` 回滚副本：请预留足够磁盘空间，验证结果后再手动删除。

## 其他改进

- 远程模式：快照下载遇到 409/503 BUSY 时按 Retry-After 重试，总等待窗口放宽到 240 秒（快照响应超时同为 240 秒），避免大型资料库首次生成快照时过早失败。
- 客户端修复保留 CLI 地址与库身份检查、按声音 ID 恢复选择、关闭保存失败后的读任务恢复，以及换源时的拖出、试听缓存、轮询代次保护。服务端播放历史由实时 API 提供。
- CLI：使用默认资料库，或空资料库搜索返回 0 条结果时，在 stderr 给出提示。
- 服务器 `/health` 新增 `api_version`、`schema_version` 与 `commit` 字段。
- 新增发布检查脚本 `tools/check-release.ps1`（标签、Cargo 版本、schema、API 与发布说明版本行），并接入发布 CI；版本规范见 [Rust_Versioning.md](Rust_Versioning.md)。

升级保留资料库、Metadata、标注与偏好设置，运行状态保存在 `%LOCALAPPDATA%/AudioLibExplorer/v1`。Windows x64 便携包包含 WASAPI 与 ASIO。

验证结果见 [Rust 验证报告](Rust_Validation.md)。
