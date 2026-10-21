# 搜索存储方案 B：设计、升级与快照契约

状态：**下一版隔离实现与验证完成，尚未发布或部署**。2026-10-21 经用户授权编写并进行全库副本实验；当前安装和工作库不变。隔离分支已有 schema 6、HTTP 协商和共享迁移器实现；这些契约不代表当前部署服务端的能力。

客户端/服务端版本同步、兼容范围、三种升级判定与双方内置迁移工具的完整契约见 [Library_Migration_Compatibility.md](Library_Migration_Compatibility.md)。本文 B 是其中一种可自动迁移的存储变化；全量重建派生 FTS 不等于丢弃业务库后重扫 WAV。

前期 909,342 条全库实验见 [Search_Storage_B_Validation.md](Search_Storage_B_Validation.md)：19.50 GB → 10.50 GB，数据和测试查询一致；当时仍有筛选长尾，属于历史原型结果。

最终共享迁移器的 [全库实测](Search_Storage_B_Integrated_Validation.md#0-最终构建复验当前结论) 已覆盖播放结构退役和恢复：19.496 GB → 10.612 GB，58 类完整查询及 258 对页面/目录结果一致，完整回滚恢复旧播放数据。时长筛选修复后搜索加目录统计为 94.5–97.6 ms；百万条合成库新查询实例的搜索 P95 为 298.9 ms。两份报告属于不同阶段，最终实现未使用 VACUUM；测试条件与限制以最终报告为准。

## 1. 目标与范围

保留现有搜索语义，把六列膨胀的派生分词文本从业务表中移除，只在写入 FTS5 时生成。保留 SQLite 单文件业务库及快照，不引入独立搜索服务。

- 原始 Metadata、用户标注、未知扩展字段、ID、路径、目录关系、扫描历史、播放列表和项目引用全部保留；按用户后续授权退役 `play_history` 和 `play_count`，原值完整保存在回滚备份，`last_played_at` 继续保留；源 WAV 不读写。
- 保留 FTS5 的 12 个逻辑检索列、列顺序、词频、位置、文档长度及现有 BM25 规则。
- 本次采用 **tokenization rev 3**。紧凑字对 rev 4 是独立候选优化，不与本次存储验证混用，也不把两项空间收益相加。
- 目标仍为 **P95 latency <500ms for 1M+ records**。真实样本不足百万条时，不据此宣称通过百万级目标。
- 全库验证报告记录实际数据量、机器、SQLite 版本、缓存条件、正确性覆盖和未验证项。容量估算不能代替生成文件后的测量。

## 2. 数据结构与写入

目标 schema 6 从 `audio_files` 移除：

```text
filename_tokenized, user_tags_tokenized, user_notes_tokenized,
bwf_description_tokenized, ixml_scene_tokenized, ixml_take_tokenized
```

这些名称仍是 FTS5 的逻辑列，不再是业务表里的持久化输入文本：

```sql
CREATE VIRTUAL TABLE audio_search USING fts5(
  filename, filename_tokenized, user_tags, user_tags_tokenized,
  bwf_description, bwf_description_tokenized, ixml_scene, ixml_scene_tokenized,
  ixml_take, ixml_take_tokenized, user_notes, user_notes_tokenized,
  content='', contentless_delete=1, detail=full, columnsize=1,
  tokenize='unicode61 remove_diacritics 2'
);
```

`bwf_description_tokenized` 的历史名称不表示输入只有 BWF description：其输入继续来自 core 的 `source_metadata_search_text`，包括现有规则允许的公开源 Metadata。不得缩减这部分搜索范围。

所有应用写入由 DataProvider 内统一的索引写入函数执行：

1. 在同一 SQLite 事务中写业务记录、生成完整检索投影并写 FTS5；提交时一起推进 revision。
2. 新增使用显式稳定 rowid；修改搜索字段时向 FTS 提供全部 12 列新值；删除先确定已有 ID，再同时删除业务记录及索引。
3. 收藏、颜色等不影响搜索文本的更新不重建 FTS；播放记录接口为空操作，不写库。
4. 扫描、重扫、Metadata 单条/批量编辑、导入、移除、快照增量应用必须共用该写入规则。不能遗留仍更新 `*_tokenized` 的 SQL。
5. 移除旧 external-content 的插入/更新/删除触发器；contentless-delete 不支持原有 FTS 特殊 `'delete'` 命令。不得混用两种删除协议。
6. 取消、错误和崩溃不能提交只有业务记录或只有索引的一半状态。跨实例 administration/scan lease、GUI 写入顺序及 generation 规则继续适用。

业务读取仍从 `audio_files` 返回 owned DTO；FTS 仅提供 ID、匹配及评分。当前页 hydration、准确总数、目录统计、筛选和稳定排序保留现有实现。不得只验证前 200 个候选便把其计数冒充完整结果。

`highlight`/`snippet` 不能依赖 contentless 表恢复输入文本；未来展示高亮需另行设计。索引维护从业务 Metadata 通过同一个投影函数重建，不能直接调用 contentless 表不支持的 FTS `'rebuild'`。

## 3. SQLite 与语义前置条件

方案 B 的读写程序必须链接包含 SQLite 官方修复
[39ca0ac365 / b0d5cbd](https://github.com/sqlite/sqlite/commit/b0d5cbd)
的版本：修复 contentless-delete 删除后的 `xRowCount` / `xColumnTotalSize` 失准。项目当前 bundled SQLite 3.53.2 缺少该修复；2026-10-21 隔离验证基线选用 **SQLite 3.54.1**。

- 当前正式 Cargo.lock 不因研究而修改。将来交付时验证最终 EXE 实际链接版本、source ID、编译选项及修复回归；系统安装了新版 sqlite3.exe 不满足此条件。
- 客户端快照增量应用也是写入者，必须满足同一最低引擎要求。
- 不修改 FTS shadow tables 来补偿引擎统计错误。
- 原始列继续索引；`detail=none`、`detail=column`、`columnsize=0` 不在本方案中。
- 字对相邻性、词边界 guard、Unicode folding、组合字符、点/下划线、单字母、数字及中文词典规则保持 rev 3。
- 自定义 BM25 必须和同库原生 BM25 对照，包括更新、删除、回滚和 segment merge 后的统计，不只比较初次建库。

官方语义依据：[FTS5](https://www.sqlite.org/fts5.html)。

## 4. 版本标记

目标完成态：

| 项目 | 值 | 含义 |
|---|---|---|
| `PRAGMA user_version` | `6` | 六个业务派生列不再存在，旧写入路径不兼容 |
| `audiolib_schema_features.tokenization` | `3` | 检索编码不变 |
| `audiolib_schema_features.search_storage` | `1` | 本文定义的 contentless-delete 格式 |
| `audiolib_schema_features.play_history_retired` | `1` | 按用户最新授权删除 `play_history` 与 `audio_files.play_count`；原值保存在回滚备份 |
| `library_meta.source_search_rev` | `2` | 源 Metadata 提取规则不变 |
| `library_meta.source_tags_rev` | `2` | 标签筛选投影不变 |

`search_indexes`、稳定 ID 等已有 feature 按实际迁移保留。遇到未知、更高的必需修订必须拒绝，不能猜测其兼容性。

迁移进行态不能占用完成态的 tokenization 编号。**现有代码把 tokenization=4 用于未完成的 rev 3 升级**；新程序遇到该组合须检查旧升级表、target revision 和备份，交由旧恢复流程完成或恢复备份。不得把它视为已完成的紧凑编码 rev 4，也不得直接作为 B 的迁移输入。

schema 6 的检查必须同时验证 feature、FTS 建表选项、列顺序及业务列结构；仅检查 `user_version` 不足。研究样本使用 schema 6 标记，保证未适配旧程序拒绝打开，不伪装为可生产使用的 schema 5。早期样本尚未包含播放退役，下一版目标格式以 `ale-s6-cld1-t3-m2-p0` 为准；除这两个明确退役对象外继续保留所有业务数据，包括 `last_played_at` 和未知扩展。

## 5. owned 副本迁移与恢复

生产迁移协议版本为 `search_storage_upgrade_protocol=1`，由新实现提供；本轮实验不宣称已经交付该恢复协议。

1. 验证来源为受支持 AudioLib 库且旧升级已完成；取得 administration lease、scan 排他权并排空写入。
2. 用 SQLite backup API 固定一致读快照，创建回滚备份和带 owner 标记的构建目录。不能直接复制正在写入的 `.db` 而漏掉 WAL。
3. 校验磁盘预算：旧库、备份、候选库、整理临时空间和 WAL 分别计入；空间不足在替换前失败。
4. 按已审核步骤复制保留的业务字段/表，显式执行已授权退役；对象在行复制后安装，避免重放用户触发器。从 Metadata 按稳定 ID 顺序生成索引。每批的内容和进度在同一事务提交。
5. 迁移清单包含源 library ID/revision、源摘要、目标 schema/feature、协议版本、tokenizer/词典摘要、步骤实现摘要、已提交 ID、行数、阶段和构建工具身份。恢复前逐项匹配；任一不符则阻止执行并保留全部产物用于诊断。
6. 阶段为 `copying → indexing → validating → compacting → ready → published`。进度是已提交批次；整理阶段没有可信百分比时只报阶段。
7. 验证保留的业务数据、退役边界、FTS 完整性、匹配/排序/计数后，在 owned 候选上整理 FTS 段，关闭连接并确认文件落盘。新实现不执行可能重排未知扩展表隐式 rowid 的 `VACUUM`；业务表由新文件复制完成整理，实际体积须重新测量。整理后重新验证，不能发布半成品。
8. 保持排他权，通过 SQLite backup 事务原子写入目标；候选含发布 receipt，以恢复“发布已提交而清单未落盘”的窗口。checkpoint Busy 保留恢复状态。失效全部旧 reader/UI generation，重新打开验证；失败用受支持回滚路径恢复。
9. 保留回滚备份直至验收。不能把 schema 6 回写给旧程序，也不能用空库覆盖回滚备份。

发布前源业务数据保持完整；源库会有持久化维护标记和写入防护。停机/取消后防护继续生效，须 resume 或 rollback，不能把 OS 锁已释放误认为迁移已完成。备份、主库和 SQLite sidecar 均按既有维护协议处理，不能手工删除 sidecar 作为清理。

contentless-delete 的旧 posting 随 segment merge 回收，应验证连续更新/删除下的索引增长，并设计有预算的后台合并。数据库文件实际收缩依赖重建/整理，checkpoint 不等于收缩。[VACUUM 官方说明](https://www.sqlite.org/lang_vacuum.html)

## 6. HTTP 快照兼容契约（隔离实现，尚未发布）

沿用 `/api/v1`；业务 DTO 的 Metadata 字段不因存储调整变化。

`GET /library` 增加当前可发布快照的格式描述，例如：

```json
{
  "schema_version": 6,
  "snapshot_format": {
    "id": "ale-s6-cld1-t3-m2-p0",
    "format": {
      "schema_version": 6,
      "storage": "contentless_delete",
      "features": {
        "foreign_key_indexes": 1,
        "play_history_retired": 1,
        "search_indexes": 1,
        "search_storage": 1,
        "stable_audio_ids": 1,
        "tokenization": 3
      },
      "source_search_revision": 2,
      "source_tags_revision": 2
    }
  }
}
```

此示例只展示新增/相关字段，不省略或改名现有 `library_id`、`revision` 等实际字段。`GET /health.schema_version` 仍表示构建能力，不能替代 `/library` 的实际快照格式；新增 `compatibility_catalog_revision`。`GET /capabilities.compatibility` 返回共享 `CompatibilityManifest`，不能把尚未完成升级的库说成 schema 6 ready。

schema 6 的快照下载必须带客户端支持声明：

```http
X-AudioLib-Snapshot-Schema-Max: 6
X-AudioLib-Search-Storage-Max: 1
X-AudioLib-Tokenization-Max: 3
X-AudioLib-Accept-Snapshot-Formats: ale-s5-ext-t3-m2, ale-s6-cld1-t3-m2-p0
```

同时必须发送 `X-AudioLib-Client-Compatibility`，值为客户端内置的完整
`CompatibilityManifest` 紧凑 JSON，包含自身 `application_version`、`catalog_revision`、
双向检查所需版本范围、`api_protocols`、`snapshot_read_formats` 等真实构建能力。
该头不是用户可填写的任意能力授权。各兼容头限单值、最多 4096 字节；拒绝重复、非法文本
及不一致的修订号。精确字段见 `audiolib-core::compatibility::CompatibilityManifest`。

- `Accept-Snapshot-Formats` 是明确的格式 ID 集合，每个 ID 在双方内置兼容目录中映射到完整的 schema、必需 feature、FTS 结构和运行时前置条件。服务端只能发送请求明确包含且自身目录认识的格式，未知 ID 不作通配符。上述 `Max` 头仅用于快速拒绝，不能把小于上限的任意组合都视为兼容；支持区间有空洞也不扩大其含义。客户端必须再次检查下载文件，不能只信任响应格式 ID。
- 新客户端先读取 `/health`、`/capabilities`，核对双方程序版本/API 范围，再读取 `/library` 并检查实际格式，兼容后才下载。没有目录的旧端只接受明确支持的 `0.4.11`、`0.5.0`；其他版本必须按目录判断。
- 已支持旧端同样必须提供快照的库身份与 revision。身份须与 `/library` 一致，revision 不得低于先前观察值；缺失头或较旧的 single-flight 快照会被拒绝，活动缓存保持不变。不能把 `/library` 的版本号套在未绑定的旧快照上。
- schema 6 服务端对缺少格式声明或不兼容的请求返回 **409 `SNAPSHOT_FORMAT_UNSUPPORTED`**，正文给出 required format；客户端目录缺失、非法或双方版本范围不符返回 **409 `PEER_VERSION_UNSUPPORTED`**。在 ETag 判断、快照复制、压缩及发送正文之前拒绝；这两种错误不按 `BUSY` 重试。旧 schema 3–5 的请求行为保持现状。
- 新客户端可继续读取已支持的旧快照；不因为服务端版本号较新便推断格式兼容。
- 快照响应（含 304）除既有身份/revision/schema/ETag 外，携带
  `X-AudioLib-Snapshot-Format`（精确 ID）、`X-AudioLib-Library-Format`（完整 `LibraryFormat` JSON）、
  `X-AudioLib-Server-Compatibility`（完整服务端目录 JSON），以及
  `X-Search-Storage-Revision`、`X-Tokenization-Revision`、
  `X-Source-Search-Revision`、`X-Source-Tags-Revision`。客户端在建立下载文件之前检查头，
  下载完成后再检查实际 schema/features/FTS、库身份和 revision；只有与 `/library`
  声明一致的完整格式才可发布缓存。探测后格式变化须重新同步。
- `.part` 下载、校验失败或版本不兼容时保留已有有效缓存与草稿，不替换、不就地迁移工作库。
- 服务端生成并校验一次完整 schema 6 快照，客户端无需重复做全库重建。
- Metadata 响应的 revision-before 规则不变，但 schema 6 本地缓存应用必须走本文同事务索引函数，不能沿用写六个派生列的旧 SQL；不能安全增量应用时完整同步。
- 发布前必须完成双方实现及兼容测试。本设计文档本身不授权部署、发布或向外部群组发送消息。

## 7. 隔离全库验证

前期研究阶段只写临时目录和设计/验证文档。用户后续已授权在隔离开发分支实现下一版，
该分支可修改源码和依赖；当前安装、工作库及研究用冻结基线保持不动。
进度见 [实现记录](Implementation_B_Status.md)。

必须记录：

1. 来源快照 ID/revision、字节数、摘要及采集时间；使用 backup API 生成独立基线。
2. 原版本基线、同 SQLite 引擎的 external-content 对照及 B 候选，区分引擎变化和存储变化的影响。
3. 全部原业务表逐行摘要/计数，包括原始 JSON、标注、历史与身份序列；完整原摘要用于备份/回滚验证。目标校验仅排除六个派生列及明确授权的播放表/计数字段，不能排除其他未知差异。
4. 对全库重新计算 rev 3 投影，与已有投影比较，出现差异停止宣称“仅存储变化”，单独报告来源和影响。
5. SQLite integrity/foreign-key 检查、FTS 完整性、全库文档长度及词项统计；代表查询的完整匹配 ID 集合与原生 BM25，DataProvider 的排序、分页、总数及目录统计。
6. 查询集覆盖常见词、高命中短字对、稀有词、无命中、中英文、Unicode、数字、标点组合、筛选与排序；不只使用能命中首屏的小样本。
7. 使用实际 DataProvider 测量首轮/新进程与热查询，分别记录结果读取和目录统计。新进程不等于 OS 冷缓存；没有安全清空 OS 缓存时必须明确说明。
8. 更新、删除、回滚、重开、合并及重新构建后的正确性；原生和自定义 BM25 对照。
9. 整理前后主文件、WAL/sidecar、索引及业务表体积、构建/整理耗时和峰值空间。不得把紧凑编码的估算再次叠加。

结果写入同目录验证报告。实验适配代码、参数、来源版本和原始指标保留于隔离目录以便复核；对外文档不包含素材路径、原始 Metadata 或凭证。

## 8. 交付判定

前期结果已用于进入隔离分支的下一版实现。只有业务数据保持、查询行为保持、增删改正确、
依赖修复验证通过、性能达到约定且完整迁移/恢复流程通过后，才能申请切换工作库或发布；
当前授权仍仅限隔离验证。

本轮测量不能替代生产迁移故障注入、GUI 验收、服务端兼容或现有发布安装流程。若性能不达标或发现语义差异，保留候选和证据，继续定位；不安装实验产物。

## 9. 当前写入与整理实现

schema 6 的 FTS 插入、删除、更新触发器调用共享投影，直接从原始列和 Metadata 计算
全部十二个逻辑索引字段；Metadata、扫描及缓存增量更新只写业务列。触发器与业务 SQL
在同一事务内完成。owned 工具的裸 SQLite 连接须调用共享
`register_library_functions` 注册相同函数；该函数不赋予写入权限，也不代替管理锁。

候选复制阶段重新计算派生 `source_tags_text`，然后再安装扩展触发器，避免重建搜索时
触发用户自定义 UPDATE 副作用。新库创建、owned 快照升级及结构迁移复用目标格式标记。

当前候选整理使用新文件复制和 FTS optimize，不调用 VACUUM，避免隐式 rowid 被重新编号。
普通 Optimize 同样保留隐式 rowid，不承诺收缩主文件。恢复时必须调整 page size 的情况，
仅在 owned staging 中执行，并检查业务投影（含隐式 rowid）前后一致；不一致就拒绝发布。
最终库体积须实测，不能直接沿用前期 VACUUM 原型的体积数字。
