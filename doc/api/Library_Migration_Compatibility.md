# 客户端、服务端与数据库迁移兼容契约

状态：**2026-10-21 下一版契约，隔离实现与验证完成，尚未发布**。与 [搜索存储 B](Search_Storage_B.md) 一并验证；实现与验收边界以 [Implementation_B_Status.md](Implementation_B_Status.md) 为准。原工作目录的当前应用、安装和工作库不变；隔离分支使用 `0.5.0-dev.1` 开发标识及独立依赖。

## 1. 三种判定与版本范围

升级规划器返回以下三种处置。它们描述数据如何处理，不只是比较两个程序版本字符串：

| 判定 | 含义 | 自动行为 |
|---|---|---|
| `compatible` | 在声明的互操作范围内，当前格式的读取、所需写入和搜索语义均受支持 | 直接使用；不为版本号不同而改库 |
| `migratable` | 格式不同，但存在已测试、保留业务数据或执行明确授权退役的迁移路径 | 在满足备份、空间、锁和恢复条件后，由内置工具自动迁移 |
| `rebuild_required` | 专门检查确认当前数据不能通过已支持路径形成可信目标库 | 不自动丢弃/重扫；列出证据、可恢复数据、所需源数据及修复方案 |

**方案 B 属于 `migratable` 候选。** 自动迁移可包含从已有 Metadata 全量重建 FTS、重排业务表或压缩文件。只要无需重新采集源 WAV、可完整保留业务数据并有可恢复执行器，就不归为“整个音效库必须全量重建”。

`rebuild_required` 不由某个程序版本段、主版本跳跃或单独的 schema 数字推出。专门检查至少包括：库身份/业务结构、业务数据可读性、必要 Metadata 是否足以重建派生状态、未知必需 feature、稳定 ID/引用是否可保留、迁移图是否完整、旧迁移状态和备份是否可恢复。

未知未来格式返回“升级工具版本/取得对应迁移器后重试”，不能仅因当前工具不认识便判定数据必须销毁重建。磁盘不足、锁被占用、权限不足、网络故障是 `blocked` 执行状态，不是第四种数据迁移类别，也不能被误报为必须重建。

## 2. 多维版本描述

同一次发布的桌面客户端、CLI 和服务端继续采用同一个 workspace SemVer；release manifest 同时记录 commit 和迁移目录摘要。已部署两端允许暂时处于不同程序版本，只要双方声明的互操作范围相交。

每个构建内置只读 compatibility manifest：

```text
application_version / build_commit
compatibility_catalog_revision
peer_application_ranges       # SemVer 区间，可多段；由经过测试的发布目录生成
direct_use_application_ranges # 测试过无需迁移的发布组合；必须再核对实际格式
auto_migrate_application_ranges # 测试过可自动迁移的来源发布组合；必须存在迁移图路径
api_protocols                 # 例如 api/v1；不能用来推断数据库格式
snapshot_read_formats         # schema + 所有必需 feature 的范围/集合
database_write_formats        # 可以安全写入的格式，通常比可读范围窄
migration_source_formats      # 有受支持迁移路径的源格式
migration_target_format
migration_protocols
sqlite_version / source_id / required_fixes
tokenizer_and_dictionary_identity
```

程序版本区间用于快速判断部署组合是否经过支持；最终能否打开/写入仍由实际数据库格式和能力检查决定。不能写成“所有 0.4.x 数据库都相同”，也不能仅用 `schema <= max` 接受未知 feature。

区间采用闭开边界 `[min_inclusive, max_exclusive)`，可声明多段并排除已知有缺陷的构建；预发布版本默认不进入正式兼容范围。同一个构建可能读取旧库、生成新库，因此来源程序版本仅是规划提示，不能替代库内格式描述。`direct_use` 和 `auto_migrate` 两类范围分别表示已验证的零迁移路径与有迁移路径；**没有“按版本号强制全量重建”的范围**。

判定顺序固定为：验证库身份 → 检查实际格式与进行中的迁移 → 检查双方程序/协议支持范围 → 判断直接支持所需操作 → 搜索有损失保护的迁移路径 → 对无法规划的情况运行专门诊断。读兼容不自动授予写兼容；客户端和服务端必须分别检查对方范围，不能只做单边版本比较。

对于未知未来格式、未知迁移协议等尚不能完成诊断的状态，报告 `disposition: null`、`execution_state: blocked` 和 `TOOL_UPGRADE_REQUIRED` 等原因；不是第四种已确定的数据类别，也不能填写没有证据的 `rebuild_required`。已确定可迁移但磁盘不足时则保留 `disposition: migratable` 并返回对应 blocker。

本次规划的格式矩阵：

| 来源实际状态 | 新版读取/使用 | 迁移政策 |
|---|---|---|
| schema 5、rev 3、external-content、完整派生状态 | 支持旧格式只读搜索，不因程序 patch 版本不同而强制迁移 | 若需本地写入或主动采用 B，规划 v5 → v6 |
| schema 6、storage 1、rev 3、Metadata 提取 rev 2 | 满足 SQLite 修复要求的新构建可直接读写 | 无迁移 |
| schema 3–5，旧 tokenization/派生状态落后 | 不能假装搜索语义已就绪 | 在 owned 副本完成已有受支持升级步骤，再进入 B；只公开最终完成态 |
| schema 1–2 / 旧 Python 库 | 不在普通原地打开/自动写入范围内 | 使用既有只读导入路径创建 owned Rust 库，再评估 B；先验证支持的源身份 |
| rev 3 升级未完成，含临时 tokenization=4 | 不作为完成的 rev 4 打开 | 识别旧协议并 resume/rollback；不可直接覆盖旧迁移清单 |
| 高于当前能力或未知必需 feature | 拒绝写入/替换缓存 | 获取受支持工具；不得盲目降级或直接判定必须重扫 |

确切的客户端/服务端 SemVer 区间必须在下一次实际发布号确定、兼容矩阵通过后填入构建目录。本设计不修改当前 `0.4.11`，也不为尚未测试的版本编造可用区间。

## 3. 两端握手和格式同步

隔离实现中 `/health.compatibility_catalog_revision` 报告兼容目录 revision；
`/capabilities.compatibility` 公布共享目录中的程序范围、协议、快照读格式、写格式和迁移协议；
`/library.snapshot_format` 报告已观察到的完整实际格式 `{id, format}`。
请求/响应头与拒绝代码见 [搜索存储 B §6](Search_Storage_B.md#6-http-快照兼容契约隔离实现尚未发布)。

客户端连接顺序：

1. 比对程序互操作区间及 API 协议，再读取实际库格式。
2. 运行同一兼容规划器，结果必须包含 `disposition`、原因、计划步骤和当前允许操作。
3. `compatible` 时直接用缓存/下载；`migratable` 时选择正确执行端；`rebuild_required` 或执行受阻时保留旧缓存与草稿并显示可操作原因。
4. 下载前声明支持范围，服务端在正文发送前拒绝不兼容快照；下载后核对实际文件身份、revision、schema/features。具体 headers/error 见方案 B。

兼容判定不能只发生在下载时。隔离实现的连接测试、后台 revision 轮询和每次 Metadata /
管理写入预检，均复用同一套程序双向范围、协议和实际库格式检查；不缓存“曾经兼容”的
结论。轮询在比较库 ID/revision 前完成检查，所以服务端即使未改变业务 revision，也不会
因升级到不支持的版本/格式而继续显示“已同步”。检查失败保留本地快照；不触发自动迁移
工作库、提交修改或重试写请求。连接测试可保留已成功的在线、库身份和根目录诊断证据，
但整体结果必须失败，音频抽样步骤跳过。

新版客户端的 Metadata 与管理写请求携带 `X-AudioLib-Client-Compatibility`。服务端在
鉴权之后、业务处理之前重新校验当前双方范围与 API 协议，覆盖预检和实际写请求之间的
升级窗口；不兼容、无效 JSON、重复或超过 4096 字节的声明返回
`409 PEER_VERSION_UNSUPPORTED`，不改变业务数据、revision 或 `data_version`。
此声明不代替 `X-AudioLib-Library-Id` 身份绑定，也不授予客户端修改服务端库格式的能力。
为保留既有 JSON API，未携带该头的历史客户端仍使用原鉴权/身份/参数校验路径；服务端
无法从缺失的声明推断其程序版本。快照下载仍单独要求 schema 6 的完整格式协商。

服务端权威库需要升级时，由服务端内置迁移器执行。客户端不能因连接到新服务端而修改服务端库，也不能把自己的较新库反向覆盖服务端。服务端完成升级后生成新格式快照。

客户端本地库由客户端内置迁移器处理。远程旧快照可以在 owned 缓存副本中转换用于读取；其 source revision 与本地 format generation 分开保存，不伪造服务端 revision。格式转换后的缓存，只有经验证的相应增量适配器才能应用远程 Metadata 响应；否则完整同步。

缓存 `cache.json` 的 `revision` 始终来自服务端。`format.source` 保存下载字节观察到的
`LibraryFormat`，`format.local` 保存当前本地格式，`format.generation` 是该次下载的本地
转换次数（未转换为 0）。重新下载重新建立来源；只转换缓存时不改变服务端 revision。
旧缓存元数据缺少来源证明时仍可离线读取，下次在线同步重新下载一次，不根据本地升级后的
格式倒推出源格式。相同 revision 只有在服务端地址和已知源格式也一致时才复用。

完整 schema 3–5 快照可直接使用；派生状态落后的快照才在独立 owned job 目录转换。
远程缓存转换可以从保留的原缓存/服务端重新开始，成功或协作取消时清理整个临时 job，
避免在已改名缓存旁留下指向旧路径的备份、锁和迁移清单。进程异常退出的遗留 job 保留，
不自动采用或递归删除；重试从未改动的原缓存创建新任务。进程退出回归已覆盖索引中和
保存前两个阶段，验证原缓存及状态文件字节不变、重试成功、遗留任务不被改动。
遗留目录的自动回收不在当前实现内，需由所有者核对任务与缓存后处理。
权威库与显式 CLI 迁移继续保留持久化 job、
原始备份及断点，不采用这种缓存清理政策。

不兼容客户端在服务端升级后收到明确错误并保留已有缓存；不会下载整个 schema 6 文件后才发现打不开。支持同一格式的不同 patch 版本可以继续互操作，不强制同步升级所有机器。

## 4. 双方内置迁移工具

迁移核心放在共享 Rust 数据层模块，通过 DataProvider/管理接口返回 owned DTO。桌面、CLI 和服务端复用同一个规划器、步骤实现、校验器和恢复执行器；不分别维护三套迁移 SQL，也不依赖用户安装 Python 或 sqlite3.exe。

拟提供的逻辑接口：

| 操作 | 行为 |
|---|---|
| `inspect` | 只读识别库、实际格式、feature、旧迁移和必要完整性信号 |
| `diagnose` | 只读遍历已识别且受支持的来源，区分业务损坏、派生损坏、引用/身份问题与检查未完成 |
| `plan` | 返回三类处置、步骤、迁移器身份、备份/空间预算和是否需维护窗口 |
| `apply` | 验证计划绑定的来源未变化，取得锁、备份，执行并持久化进度 |
| `resume` | 校验源摘要、格式、词典、工具/协议后从已提交阶段恢复 |
| `verify` | 检查全部业务数据、引用、索引结构及语义回归 |
| `rollback` | 在独占维护条件下恢复已验证备份并失效旧 generations |

隔离开发分支已接入 `db migration inspect/diagnose/plan/apply/resume/verify/rollback`，
CLI 在加载默认设置或打开 DataProvider 前分派这些命令。所有迁移命令必须显式指定
`--database`，文件不存在时不会创建库；应用读写启动入口使用同一个执行器。
当前版本未安装这些改动。HTTP 能力协商、升级进度、受支持历史布局及全库集成验证
已在隔离分支通过；历史布局包含捕获的 v5 DDL 与系统构造的 v3/v4 加法兼容布局，
不声称覆盖未取得的实际归档 v3/v4 数据库。详见实现记录和最终全库报告。

```powershell
AudioLibCli --database .\owned-library.db db migration inspect --deep --writable
AudioLibCli --database .\owned-library.db db migration diagnose
AudioLibCli --database .\owned-library.db db migration plan --output .\migration-plan.json
AudioLibCli --database .\owned-library.db db migration apply --plan .\migration-plan.json
AudioLibCli --database .\owned-library.db db migration resume
AudioLibCli --database .\owned-library.db db migration verify
AudioLibCli --database .\owned-library.db db migration rollback
```

`inspect`/`diagnose`/`plan` 只读访问来源；计划文件必须是新文件，不覆盖已有文件。`apply` 要求计划中
的规范化来源路径与 `--database` 一致，并复核来源数据、结构和编译步骤身份。后三个命令
定位该库最近一次登记的持久化任务；进度 JSON 写 stderr，stdout 保留单个命令结果。
Ctrl+C 请求协作取消，已提交批次保留。任务目录和原始备份不自动清理；继续处理应使用
原工具和步骤版本。显式回滚后登记 `RolledBack`，普通写入启动不会立即再次自动升级，
需重新 `plan/apply` 才解除这一状态。

复制批次和索引批次分别记录已提交行数。候选库先提交复制完成状态，再将持久化任务推进到
`Indexing`；若在二者之间退出，下次启动根据候选库状态恢复，不能重复复制。恢复界面只显示
真实的音频行水位，不能把不含行计数的阶段通知当成“已完成 0 行”。
恢复复制或索引前必须核对完整的断点条目集合、阶段、非负行数、严格的 0/1 完成标记，
以及候选库实际行数和游标。带 rowid 的表必须对应来源中截至游标的完整 ID 前缀，
索引断点必须对应 `audio_search_docsize` 的已索引 ID 前缀；`WITHOUT ROWID` 表不允许
携带 rowid 游标。索引阶段要求全部业务表复制完成。发现不一致时停止，不自动重置游标、
补写数据或发布；保留任务及原始备份供诊断和回滚。该检查不代替发布前的完整业务摘要、
结构及 FTS 校验。SQLite 先恢复未提交事务，再核对这些断点。
发布前来源保持旧业务内容并被迁移保护锁阻止写入；发布后仅剩 `Checkpoint` 时，完整目标库
可以只读打开，写入启动仍先完成恢复。显式 owned 快照升级取消后也会保留保护标记和任务，
调用方须恢复、回滚，或按缓存政策丢弃整个自有任务目录，不能把中断文件直接作为完成的缓存。

迁移报告至少包含：

```text
disposition, execution_state, blockers[], source_format, target_format,
source_library_id, source_revision, source_digest,
tool_version, catalog_revision, migration_protocol, plan_id,
steps[], committed_progress, backup_path, owned_staging_path,
required_free_bytes, verification_results, recovery_action
```

日志/HTTP 状态不泄露源 Metadata、密钥或不必要的本机路径；本机管理报告可提供备份和隔离目录定位。

自动迁移前提是：明确拥有目标、本机权限、支持的来源、可用回滚备份、足够空间、排空写入、独占 administration 和 scan 协议、没有未知 feature、完整性检查通过。自动表示无需用户手工运行 SQL；不表示可以忽略长任务进度、取消或磁盘预算。

服务端迁移期间 `/health` 持续可访问并报告阶段与已提交进度，需要库的端点返回既有 `503 UNAVAILABLE`/`Retry-After`。服务端重启识别迁移清单后恢复；客户端启动行为一致。缓存下载、源库迁移和格式转换不能共用一个模糊的进度值。

隔离客户端已整合 `aecd59b` 的传输进度接口：`Downloading` 按压缩 HTTP body 字节和
`Content-Length` 计数，`Decompressing` 按写出的数据库字节和 `X-Uncompressed-Length`
计数。流式解压与下载会交错，界面同时保留两条计数；未知总量只让对应进度条保持不定。
下载达到 100% 不表示解压、校验或迁移完成。普通未压缩响应只显示下载。

`CachePreparation::Copying` 仍表示复制缓存的 SQLite 页数；新增
`CachePreparation::Migrating(SnapshotUpgradeProgress)` 原样传递共享执行器的备份、复制行数、
索引行数、校验、整理、发布和 checkpoint 阶段。界面复用本地迁移的阶段文字及已保存行数，
不会把页数和行数合并，也不会把某一批次百分比当作整体升级百分比。计数更新通常按 200 ms
合并，但阶段改变、已提交计数完成或恢复水位改变须立即显示；下载/解压信息在进入校验和
缓存准备时清除，旧 generation 的事件仍不能改变当前界面。

## 5. 重建专门检查与数据保护

规划器发现无法自动迁移时生成结构化证据，不执行破坏性“修复”：

- 派生索引坏、业务 Metadata 完整：安排可恢复的派生重建，优先仍归 `migratable`。
- 当前工具缺少已知格式转换路径：提示需要哪个迁移器/中间版本，保留数据；不默认要求重扫。
- 关键业务数据缺失、不可恢复的库损坏、ID/引用无法可靠保留：检查备份、导出可读数据和源素材可用性后，才给出 `rebuild_required` 方案。
- 任何全库重建都必须另有授权，优先恢复备份；尽可能保留用户标注与稳定关联。永远不删除源 WAV。

不得凭文件名、文件大小或程序 SemVer 主版本变化判断必须全量重建。也不得把“索引重建耗时长”误归为无法自动迁移。

隔离实现的 `migration::diagnose` 返回 `LibraryDiagnosis`，在固定只读事务中先识别格式，
再强制遍历普通表的主 B-tree（`NOT INDEXED`）、检查保留的外键引用、稳定 ID 水位，
以及 FTS shadow 表页可读性和 docsize 成员关系。`plan` 调用同一个诊断器；只有可修复的
派生异常可以继续规划，其余发现阻止执行。不会先创建迁移任务再发现来源无法安全处理。

| 证据代码 | 当前处置 |
|---|---|
| `DERIVED_DATA_INVALID` | 原始 Metadata 可读；在拥有的候选库重建 FTS，保留原始备份 |
| `BUSINESS_ROWS_UNREADABLE` | 已识别的保留业务表实际返回 SQLite 损坏代码；`rebuild_required + blocked`，先进入备份恢复/人工抢救评估 |
| `RETIRED_DATA_UNREADABLE` | 仅已声明退役表不可读；阻止迁移，等待专用保全适配器，不据此要求整个库重建 |
| `BUSINESS_REFERENCES_INVALID` | 保留的外键引用断裂；阻止迁移并修复引用，不自动删除孤儿数据 |
| `UNSAFE_SCHEMA_REPAIR` | 水位证据缺失/异常或保留名称被其他对象占用；阻止自动修复 |
| `INSPECTION_UNAVAILABLE` | I/O、权限、未注册扩展函数或其他原因使检查无法完成；不是业务损坏证据 |

未知未来 schema/必需 feature 在遍历业务表前即返回 `TOOL_UPGRADE_REQUIRED`，即使该文件
另有损坏也不由旧工具猜测。无法打开路径的错误直接返回调用方，取消仍为 `Cancelled`。
报告始终为 `automatic_rebuild_allowed: false`。上述 `rebuild_required` 只表示已支持的
自动路线不能生成可信目标，不证明数据永久无法抢救，也不授予重扫或丢弃标注的权限。

当前诊断不是对全部 FTS 内部 posting 语义的穷尽证明；候选库仍须完成既有完整性、FTS 和
业务投影校验。不能把单独 `quick_check` 失败直接当成业务丢失：SQLite 的完整性检查还涉及
索引，而且不包含外键检查；见 [SQLite integrity_check](https://www.sqlite.org/pragma.html#pragma_integrity_check)。

目标结构检查比较对象类型、所属表和 SQL token；字符串字面量的空格/大小写保持精确，
不再通过删除所有空白/统一小写来判断等价。只在候选库重建编译目录内的 8 个必需索引及
ID 水位触发器；未知扩展对象和可选加速索引保留。ID sequence 表必须具有已知约束、唯一
singleton 行及不低于现存 ID 的有效水位；不得用当前最大 ID 猜出已经删除记录的历史水位。

## 6. 必测矩阵与本轮边界

生产交付前至少覆盖：

- 同格式不同 patch 版本直接使用、允许范围内不同两端版本握手。
- 老库逐级/组合迁移，schema 3、4、5 及旧派生修订。
- 已完成/未完成 rev 3 的 tokenization=4 判别。
- 未知未来 schema/feature 拒绝，旧客户端下载前拒绝。
- 服务端迁移后快照、客户端缓存转换、Metadata 增量应用及退回完整同步。
- 各提交/整理/发布边界的取消、进程突然退出、重启恢复、回滚及空间不足；实际断电另需验证。
- 业务记录与历史、标注、未知 Metadata、身份及引用保持；写入与索引同事务。

前期已用内部实验执行器完成方案 B 全库副本的体积、数据和搜索比较，实测见
[全库验证报告](Search_Storage_B_Validation.md)。用户随后授权在隔离分支开发下一版；
共享自动迁移、恢复、回滚和 CLI 已完成实现及隔离回归，范围与限制见
[实现记录](Implementation_B_Status.md)。最终步骤版本已完成含播放结构退役的
[全库迁移、搜索一致性及完整回滚复验](Search_Storage_B_Integrated_Validation.md#0-最终构建复验当前结论)，
并通过两端协商、进度、支持的历史布局、进程崩溃恢复及百万条查询样本验证。
这些结果不代表已升级当前安装，也不覆盖实际断电或不可得历史归档。

## 7. 通用结构演进与播放统计退役

2026-10-21 用户补充：服务端 `fix/no-play-tracking` / `e7b2483` 已通过另一侧 CR，
停止写入播放次数及历史，旧接口响应形状维持，`/history/plays` 返回空数组。
已比对该提交及其运行时修复父提交；隔离分支独立实现等价行为，未 cherry-pick 或据此部署服务端。
停止记录是行为变化，不能据此推断旧库已无历史值或结构已经改变。

隔离分支已实现对应空操作：服务端保留身份/id/存在性校验及响应 DTO，
播放不写库、不增加 revision；普通客户端播放不再调用 `/plays`，也不发出缓存写入事件，
历史直接返回空数组。低层兼容 HTTP 客户端仍保留该方法用于旧接口验证。

迁移目录必须描述通用的、编译进两端同一 Rust 模块的步骤，不接受客户端传来的任意 DDL：

| 结构操作 | 步骤必须声明的契约 | 必须验证的结果 |
|---|---|---|
| 添加字段/表 | 类型、NULL/default、旧记录回填表达式、业务或派生属性 | 旧字段全量不变；新字段默认值与回填正确 |
| 删除字段/表 | 完整对象清单、依赖、废弃原因、旧值处置及兼容读接口 | 仅声明的对象消失；其他列、稳定 ID、引用和未知扩展不变 |
| 重命名/转换类型/拆合表 | 明确来源到目标映射、转换失败策略、引用重映射 | 无静默截断、NULL/类型损失或孤儿引用 |
| 重建索引/约束/触发器 | 目标 SQL、重建顺序、语义与事务边界 | 搜索结果、外键和原子写入约束成立 |

每一步有稳定 step ID、来源/目标格式和实现摘要。计划与持久化 job 绑定步骤目录摘要；
新二进制只能在明确支持原步骤身份时继续旧 job，不能把同一 step ID 悄悄换成不同转换。
通用复制执行器按表/列映射生成 owned staging，SQLite 重写并验证 DDL，恢复水位与该映射绑定。
新增业务字段允许有明确默认值；未知扩展列/表默认保留。依赖被删字段的自定义索引、视图或
触发器无法受控转换时必须阻止执行，不能默默丢弃。

校验分为“原业务投影必须保留”“显式转换/退役的数据”“新派生值”三部分。
不能简单排除所有目标中不存在的列来让 hash 通过。删除业务字段时必须逐项声明旧值去向：
保留可查询归档或显式授权的退役政策，并保留完整升级前备份与回滚能力。自动迁移的默认
路线仍要求业务无损；仅一个通用 `DROP COLUMN` 能力不代表获准删除任何历史数据。

**播放统计的最新授权边界（覆盖此前保留计划）：** 用户已明确要求下一次结构升级删除
`play_history` 表和 `audio_files.play_count` 列。隔离实现把它们注册为显式退役步骤；
升级前备份完整保留旧表、旧值，失败回滚恢复它们。其余业务内容不因退役而排除校验，
包括实际列名 `last_played_at`；该列不在本次删除清单中。退役不扫描或删除 WAV。
当前工作库不执行这些步骤。

可读的旧 `play_history` 若只含指向已不存在音频的引用，仍可随已授权表退役；原始记录和
无效引用一并保留在完整备份中，回滚还原原状。保留表的断裂引用仍阻止迁移。旧播放表
本身不可读时不会假装已完成完整保全，诊断须阻止自动执行。

目标 schema 6 增加 feature `play_history_retired=1`；完整格式标识调整为
`ale-s6-cld1-t3-m2-p0`。检查同时确认指定表/列确实不存在，不能只相信 feature 标记。
之前全库研究样本保留播放数据，仍是历史实验结果，不等于已验证带退役步骤的最终格式。
最终共享迁移器的 909,342 行隔离验证已包含退役及回滚，详见
[集成验证报告](Search_Storage_B_Integrated_Validation.md#0-最终构建复验当前结论)。报告记录最终工具与步骤身份；
目标中表/列不存在，回滚恢复 752 条播放记录及合计 759 次计数，保留的最后播放时间逐项一致。
尚未迁移的旧格式可返回原有非零 `play_count`，旧 API 示例中的 `0` 不是其固定值；
结构升级后的兼容 DTO 保留字段并返回 `0`、历史返回 `[]`，不得再读已删除的列或表。
客户端/服务端适配与兼容测试完成前不能发布该目标格式。

结构演进回归至少包含：有旧值的播放统计与历史、添加 nullable/default 字段、生成列、
删除派生列、业务字段退役有无归档、未知扩展保留、依赖阻断、引用约束、分批中断恢复、
发布后新写入导致回滚被阻止，以及客户端/服务端不同但受支持版本的读写矩阵。
