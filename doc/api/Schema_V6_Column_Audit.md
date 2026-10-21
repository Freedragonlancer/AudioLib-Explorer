# v6 列审计：保留项、历史字段与后续候选

状态：2026-10-21，方案 §5A 的逐列源码审计；供本轮 PR 引用。**本审计不授权删除任何额外字段。** 本轮只删除已经确认的 `play_count`、`last_played_at`、`play_history`，以及方案 B 明确重建的六个派生文本列；其他用户数据继续保留。

审计对象是当前 [AUDIO_COLUMNS / FOLDER_COLUMNS](../../crates/audiolib-db/src/schema.rs#L953)：46 个音频列、11 个目录列，共 57 项。两个表在基础 schema 中定义的 `id` 不在这两个数组内；它们仍是业务引用和稳定身份，不能当作废弃字段。`AUDIO_COLUMNS` 是兼容旧库的加列清单，不等于 p1 的最终物理列清单：其中六个 `*_tokenized` 音频列只供旧格式归一化使用，[新库模板](../../crates/audiolib-db/src/schema/current.rs)明确跳过，p1 只保留 FTS5 中的同名逻辑索引字段。

检索覆盖 `crates/`、`server/`、`app/` 的生产 `src`，逐项检查 SQL 和 DTO 的实际调用方。例如：

```powershell
rg -n 'user_rating|ixml_|peak_amplitude|rms_level|spectral_centroid|zero_crossing_rate|seen_scan_id' crates server app -g '*.rs' -g '!**/tests/**' -g '!**/*test*.rs' -g '!**/fixtures/**' -g '!**/schema/**' -g '!**/schema.rs' -g '!**/bin/**'
rg -n 'last_scanned|total_file_count|total_duration|total_size|updated_at|created_at' crates server app -g '*.rs' -g '!**/tests/**' -g '!**/*test*.rs' -g '!**/fixtures/**' -g '!**/schema/**' -g '!**/schema.rs' -g '!**/bin/**'
```

同文件内的 `#[cfg(test)]` 测试、基准/诊断程序、历史 fixture、单纯建表/加列语句和通用全量迁移复制，人工排除在“当前业务读写”证据之外。未找到引用表示在本次生产源码范围内未找到，不能证明外部用户扩展或旧库中没有数据。本轮未读取或抽样用户工作库，未以行值分布或磁盘占用作删除依据。

“通用透传”指 SQL 映射到 `AudioFile.attributes`、JSON/导出和选中素材的通用 Metadata JSON 查看器。它说明旧值仍可访问，**不代表有专用 UI 控件、筛选、排序、计算或持续维护该字段的生产流程**。以下逐项区分这两类用途。

## 结论与待确认项

| 类别 | 字段 | 源码结论 | 建议 |
| --- | --- | --- | --- |
| 未找到当前专用业务读或写 | `audio_files.user_rating` | 无 DTO 字段、评分编辑/排序/筛选或扫描写入；旧评分索引已移除，旧 CSV `rating` 仅容忍并忽略。 | 本轮保留旧值；**后续可提请删除**，由用户确认并制定旧值处置。 |
| 未找到当前业务读或写 | `folders.last_scanned` | 仅剩兼容列定义；扫描记录使用 `scan_sessions`，未找到该目录列的更新或消费。 | 本轮保留；**后续可提请删除**，不能把旧库值直接丢弃。 |
| 只有旧值的通用透传，未找到当前计算/写入 | `peak_amplitude`、`rms_level`、`spectral_centroid`、`zero_crossing_rate` | `audio_from_row` 读入 attributes；扫描和 WAV Metadata 提取不计算这些列。Waveform/频谱渲染有独立实时数据与缓存路径。 | 本轮保留；可单独讨论退役或迁入扩展 Metadata，须先约定归档和兼容，不能宣称“完全无人读取”。 |
| 当前有写、未找到该物理列的业务读取 | `folders.file_count`、`folders.total_duration`、`folders.total_size`、`folders.created_at`、`folders.updated_at` | 扫描、重定位或目录统计会维护；目录 DTO 使用的是 `total_file_count`，库总时长/大小从音频行统计。 | 本轮保留；可另议冗余统计清理，但须联动统计更新、旧契约及性能验证。 |
| 必须保留 | `seen_scan_id` | 参与扫描结束的缺失标记判断，也供服务端扫描后 Waveform 预生成挑选本轮记录。 | 保留。TEMP `scan_seen` 对未变化文件的优化没有替代这个列的全部职责。 |
| 已有真实写入；部分为通用透传 | `ixml_*` | WAV 的 iXML 解析和扫描 SQL 正在写入。scene/take 直接参与 FTS5；其余值经 Metadata 透传，原始 iXML 文本另可进入 Metadata 搜索。 | 全部保留；没有专用 UI 不构成本轮删列理由。 |

本表不是下一批删列清单。除已授权的播放对象与派生列之外，迁移 recipe 未增加退役项。

供 PR 中“最后一处引用”定位：`user_rating` 的实际列名仅保留在[兼容 DDL](../../crates/audiolib-db/src/schema.rs#L982)，旧 CSV `rating` 的[忽略分支](../../crates/audiolib-db/src/management.rs#L1444)不是评分读写；`folders.last_scanned` 仅剩[兼容列声明](../../crates/audiolib-db/src/schema.rs#L1012)及基础 DDL。四个分析指标的最后实际消费者是 [audio_from_row 的 attributes 映射](../../crates/audiolib-db/src/lib.rs#L2500)，不是专用分析/UI 功能；其余只写目录字段的当前维护入口是[目录汇总][rollup]及[目录创建][folder-build]。

## audio_files：46 个兼容列

| 列 | 来源 / 当前写入 | 实际读取与用途 | 处理建议 |
| --- | --- | --- | --- |
| `file_path` | [扫描 SQL][scan]从真实文件路径写入；[重定位][relink]更新。 | [DTO 映射][dto]、播放文件解析、[列表/详情][ui]、路径查找与导出。 | 保留，素材定位的业务字段。 |
| `filename` | [扫描 SQL][scan]从文件名写入。 | [FTS 投影][fts]、名称排序、[列表/详情][ui]及导出。 | 保留。 |
| `file_size` | [扫描 SQL][scan]写文件系统长度。 | [增量指纹][fingerprint]、库大小/目录统计、DTO/API/导出；不是仅为通用 JSON 而保留。 | 保留。 |
| `sample_rate` | [WAV fmt 解析][wav] / 解码探测，经[扫描 SQL][scan]写入。 | 采样率筛选/排序、[列表格式][ui]、播放/波形请求参数及 DTO。 | 保留。 |
| `bit_depth` | [WAV fmt 解析][wav]，经[扫描 SQL][scan]写入。 | 位深筛选、[列表格式][ui]及 DTO/导出。 | 保留。 |
| `channels` | [WAV fmt 解析][wav]，经[扫描 SQL][scan]写入。 | 通道筛选、[列表/详情][ui]及 DTO/导出。 | 保留。 |
| `duration` | WAV/解码帧数与采样率得到秒数，经[扫描 SQL][scan]写入。 | 时长筛选/排序、[目录汇总][rollup]、库统计、列表及播放定位。 | 保留。 |
| `codec` | [扫描 SQL][scan]写探测到的 `AudioMetadata.encoding`。 | [DTO 映射][dto]转为 `attributes.encoding`，详情 JSON/API/导出可见；无独立 codec 筛选控件。 | 保留，仍有真实来源和输出。 |
| `bwf_description` | [BEXT 解析][wav]，经[扫描 SQL][scan]写入。 | [FTS 投影][fts]，以及[详情描述][ui]的 BWF 回退；也与公开源 Metadata 合并成搜索文本。 | 保留。 |
| `bwf_originator` | [BEXT 解析][wav] → [扫描头字段 SQL][scan]。 | [DTO attributes 映射][dto] / 通用 Metadata 查看与导出；未找到专用编辑/筛选。 | 保留，活跃的源 Metadata 写入。 |
| `bwf_originator_reference` | [BEXT 解析][wav] → [扫描头字段 SQL][scan]。 | [DTO attributes 映射][dto]；通用透传，无专用 UI 消费。 | 保留。 |
| `bwf_origination_date` | [BEXT 解析][wav] → [扫描头字段 SQL][scan]。 | [DTO attributes 映射][dto]；通用透传，不能等同于库 `created_at`。 | 保留。 |
| `bwf_origination_time` | [BEXT 解析][wav] → [扫描头字段 SQL][scan]。 | [DTO attributes 映射][dto]；通用透传。 | 保留。 |
| `bwf_time_reference` | [BEXT 解析][wav] → [扫描头字段 SQL][scan]。 | [DTO attributes 映射][dto]的整数源时间参考；未找到专用播放定位消费。 | 保留。 |
| `bwf_version` | [BEXT 解析][wav] → [扫描头字段 SQL][scan]。 | [DTO attributes 映射][dto]；通用透传。 | 保留。 |
| `bwf_umid` | [BEXT 解析][wav]读取 UMID 字节并编码 → [扫描 SQL][scan]。 | [DTO attributes 映射][dto]；通用透传。 | 保留。 |
| `ixml_scene` | [iXML 解析][ixml]直接 BWFXML 子元素 → [扫描 SQL][scan]。 | [FTS 投影][fts]的 scene 列及派生分词列；同时进 attributes。 | 保留，真实搜索输入。 |
| `ixml_take` | [iXML 解析][ixml] → [扫描 SQL][scan]。 | [FTS 投影][fts]的 take 列及派生分词列；同时进 attributes。 | 保留。 |
| `ixml_tape` | [iXML 解析][ixml] → [扫描头字段 SQL][scan]。 | [DTO attributes 映射][dto]，没有单独 tape 列筛选；`metadata_json` 的原始公开 iXML 文本另由[Metadata 搜索投影][source-text]消费。 | 保留，不把泛型 JSON 搜索误称为该物理列的专用索引。 |
| `ixml_circled` | [iXML 解析][ixml]转布尔 → [扫描头字段 SQL][scan]。 | [DTO attributes 映射][dto]；未找到专用 UI/业务分支读取这个物理列。 | 保留，仍有真实写入。 |
| `ixml_wild_track` | [iXML 解析][ixml]转布尔 → [扫描头字段 SQL][scan]。 | [DTO attributes 映射][dto]；通用透传。 | 保留。 |
| `ixml_no_good` | [iXML 解析][ixml]转布尔 → [扫描头字段 SQL][scan]。 | [DTO attributes 映射][dto]；没有据此隐藏/禁止播放的产品规则。 | 保留。 |
| `peak_amplitude` | 未找到当前扫描/分析器对该列的专用计算或写入；可有旧库/导入值。 | [DTO attributes 映射][dto]，通用查看/导出；未找到峰值筛选或 Waveform 算法从此列读取。 | 本轮保留；后续候选，须确认旧值处置。 |
| `rms_level` | 未找到当前对该列的专用计算或写入。 | [DTO attributes 映射][dto]；与 Waveform 缓存中的 `rms_values` 不是同一对象。 | 本轮保留；后续候选。 |
| `spectral_centroid` | 未找到当前对该列的专用计算或写入。 | [DTO attributes 映射][dto]；未找到频谱 UI 读取这个库列。 | 本轮保留；后续候选。 |
| `zero_crossing_rate` | 未找到当前对该列的专用计算或写入。 | [DTO attributes 映射][dto]，仅通用透传。 | 本轮保留；后续候选。 |
| `user_tags` | [Metadata 编辑/批量/导入][metadata-write]；扫描保留用户值。 | [FTS 投影][fts]、标签筛选、[列表标签及编辑器][ui]、API/导出。 | 保留，用户标注。 |
| `source_tags_text` | 扫描从 `metadata_json` 的公开标签提取并折叠大小写；[修复/归一化][source-tags]重算。 | 标签包含/仅有标签筛选和相关 partial indexes；[查询条件][filters]直接使用该列。 | 保留，当前实际使用的派生加速列。 |
| `user_rating` | 未找到当前评分写入；旧源导入/通用复制可保留值，不能算现版评分功能。 | 未找到当前 DTO、UI、筛选、排序或计算读取；[旧 CSV rating][metadata-write]仅容忍并忽略。 | 本轮保留；可提请独立退役，须用户确认。 |
| `user_color` | [Metadata 编辑/批量/导入][metadata-write]。 | 颜色筛选、DTO `color`、[列表标记颜色][ui]。 | 保留，用户标注。 |
| `user_notes` | [Metadata 编辑/批量/导入][metadata-write]。 | [FTS 投影][fts]、详情编辑器、DTO/API/导出。 | 保留，用户标注。 |
| `created_at` | [扫描新增 SQL][scan]写库记录创建时间；旧导入值保留。 | [DTO attributes 映射][dto] / 通用 Metadata JSON；未找到独立创建日期排序/筛选。 | 保留，有持续写入及输出。 |
| `modified_at` | 扫描更新、[Metadata 编辑][metadata-write]和[重定位][relink]更新。 | [修改时间排序][filters]、DTO attributes；[服务端扫描后预生成][seen-cache]亦消费。 | 保留。 |
| `filename_tokenized` | 仅旧格式的[归一化分词][old-tokenize]写音频列。p1 音频表无此列。 | 仅旧 external-content FTS/迁移消费；p1 同名 FTS 逻辑列来自[即时投影][fts]。 | 已授权删除物理派生列；保留逻辑索引语义。 |
| `user_tags_tokenized` | 仅旧格式的[归一化分词][old-tokenize]及旧快照 Metadata 补丁维护。 | 旧 FTS/迁移；p1 由[即时投影][fts]替代。 | 同上，已授权删除。 |
| `folder_id` | [扫描][scan]与[重定位][relink]绑定目录。 | 目录作用域、目录查询统计、外键关系及 DTO。 | 保留。 |
| `user_notes_tokenized` | 仅旧格式的[归一化分词][old-tokenize]及旧快照补丁维护。 | 旧 FTS/迁移；p1 由[即时投影][fts]替代。 | 已授权删除物理派生列。 |
| `bwf_description_tokenized` | 仅旧格式的[归一化分词][old-tokenize]，合并公开 Metadata 搜索文本。 | 旧 FTS/迁移；p1 在[投影][fts]中计算。 | 已授权删除物理派生列。 |
| `ixml_scene_tokenized` | 仅旧格式的[归一化分词][old-tokenize]。 | 旧 FTS/迁移；p1 在[投影][fts]中计算。 | 已授权删除物理派生列。 |
| `ixml_take_tokenized` | 仅旧格式的[归一化分词][old-tokenize]。 | 旧 FTS/迁移；p1 在[投影][fts]中计算。 | 已授权删除物理派生列。 |
| `favorite` | [Metadata 编辑/批量/导入][metadata-write]。 | 收藏筛选、[列表星标/详情编辑器][ui]、DTO/API/导出。 | 保留，用户标注。 |
| `source_mtime_ns` | [扫描][scan]写源文件修改时间；[重定位][relink]重新取真实目标指纹。 | [增量扫描指纹][fingerprint]判断是否重探测及失效旧 Waveform 缓存。 | 保留。 |
| `metadata_json` | [WAV 嵌入 Metadata][wav]经[扫描合并][scan]写入，保留未知扩展和用户附加信息。 | [公开 Metadata 搜索][source-text]、源标签、DTO、通用 JSON 详情与导出；不能用已知列替代其未知扩展。 | 保留，权威 Metadata。 |
| `missing` | [扫描缺失/重现][scan-finish]及[重定位][relink]更新。 | 可见性、搜索/统计、列表状态、Waveform/服务端候选等。 | 保留。 |
| `seen_scan_id` | [扫描新增/更新][scan]写当前 session ID；未变化文件使用 TEMP `scan_seen` 补充。 | [扫描结束][scan-finish]防止把本轮已见文件标缺失；[服务端预生成][seen-cache]挑选本轮新增/变化记录。 | 必须保留，关系到扫描正确性。 |
| `path_key` | [扫描][scan]、[重定位][relink]写规范化路径键。 | [指纹查找][fingerprint]、增量扫描、重现、路径匹配、导入标注匹配及目录范围操作。 | 保留。 |

## folders：11 个兼容列

| 列 | 来源 / 当前写入 | 实际读取与用途 | 处理建议 |
| --- | --- | --- | --- |
| `folder_path` | [ensure_folder / 目录重挂接][folder-build]创建；[目录重定位][relink]更新。 | 目录定位、作用域、重定位验证、[目录 DTO][folder-read]；服务端根目录计数先匹配目录路径。 | 保留。 |
| `folder_name` | [目录创建][folder-build]从真实目录名取值；[重定位][relink]更新根名。 | [目录 DTO/排序][folder-read]与目录树显示。 | 保留。 |
| `parent_id` | [目录创建/重挂接][folder-build]维护。 | 子目录读取、祖先/后代递归、目录删除/重定位和层级重建。 | 保留。 |
| `depth` | [目录创建/重挂接][folder-build]维护。 | 子目录深度计算、层级重挂接与 closure 重建；不是仅 DDL 或测试字段。 | 保留，结构维护使用。 |
| `file_count` | [目录汇总][rollup]写“直属且存在”的音频数量。 | 未找到当前生产逻辑读取该物理列；`Folder.file_count` 实际映射的是 `total_file_count`，不能混淆。 | 本轮保留；可另议冗余直属计数。 |
| `total_file_count` | [目录汇总][rollup]写所有后代的存在音频数。 | [目录 DTO][folder-read]映射为 `Folder.file_count`，用于树节点、统计和[服务端根目录计数][root-count]。 | 保留。 |
| `total_duration` | [目录汇总][rollup]持续写后代时长之和。 | 未找到当前对 `folders.total_duration` 的业务读取；库统计从 `audio_files.duration` 汇总，同名 DTO 不是读取该目录列的证据。 | 本轮保留；另议统计冗余。 |
| `total_size` | [目录汇总][rollup]持续写后代字节数之和。 | 未找到当前对这个目录列的业务读取；库大小由音频行汇总。 | 本轮保留；另议统计冗余。 |
| `created_at` | [目录创建][folder-build]写创建时间，导入保留旧值。 | 未找到当前目录 DTO/UI/筛选/排序读取；与音频同名列的 JSON 透传无关。 | 本轮保留；仅写字段候选需另行确认。 |
| `updated_at` | [目录汇总][rollup]和[目录重定位][relink]写。 | 未找到当前生产业务读取该目录列。 | 本轮保留；仅写字段候选需另行确认。 |
| `last_scanned` | 除兼容 DDL、原值迁移外，未找到当前生产写入。 | 未找到当前生产读取；实际扫描历史来自独立 `scan_sessions` API。 | 本轮保留；可提请独立退役。 |

## 审核后的约束

`user_rating`、四个历史分析指标以及目录的未消费统计/时间列，均没有加入本次退役 recipe。下一次若确认删除，仍须逐项给出旧值去向、依赖检查、备份/回滚和兼容格式变化；不能以“没有专用界面”替代数据保留决策。

本轮已授权的最终主库格式是 p1：不含播放次数、最后播放时间或播放历史表；独立的客户端本地历史不属于本次两个主库列数组。旧 p0 读兼容和 p0→p1 迁移另见[数据库迁移契约](Library_Migration_Compatibility.md)。本审计未新增任何 Rust 行为，也不重新运行已有二进制验证。

[scan]: ../../crates/audiolib-db/src/lib.rs#L1164
[wav]: ../../crates/audiolib-audio/src/wav.rs#L129
[ixml]: ../../crates/audiolib-audio/src/wav_metadata.rs#L261
[dto]: ../../crates/audiolib-db/src/lib.rs#L2470
[ui]: ../../app/audiolib-desktop/src/controller.rs#L5599
[fts]: ../../crates/audiolib-db/src/search_storage.rs#L38
[source-text]: ../../crates/audiolib-core/src/metadata.rs#L80
[source-tags]: ../../crates/audiolib-db/src/schema.rs#L406
[metadata-write]: ../../crates/audiolib-db/src/management.rs#L367
[filters]: ../../crates/audiolib-db/src/lib.rs#L2358
[fingerprint]: ../../crates/audiolib-db/src/scanner.rs#L429
[scan-finish]: ../../crates/audiolib-db/src/lib.rs#L1330
[relink]: ../../crates/audiolib-db/src/management.rs#L896
[folder-build]: ../../crates/audiolib-db/src/lib.rs#L2546
[folder-read]: ../../crates/audiolib-db/src/lib.rs#L2098
[rollup]: ../../crates/audiolib-db/src/lib.rs#L2641
[seen-cache]: ../../server/audiolib-server/src/list_waveform.rs#L1758
[root-count]: ../../server/audiolib-server/src/state.rs#L1996
[old-tokenize]: ../../crates/audiolib-db/src/schema/search_upgrade.rs#L244
