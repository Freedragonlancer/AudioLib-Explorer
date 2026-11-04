# 0.9.0 发布准备与两端迁移方案

状态：**发布准备中。#204–206已有实现及相应源级/兼容证据，官方旧GUI声明范围完成；当前AudioFile COW工作集仍有Vorbis缓存命中回归失败，PG05严格1.1仍阻塞，新候选GUI/实际包/安装未完成，未发布。**
`0.9.0` 是拟定 MINOR 版本；Cargo 当前仍为 `0.8.7`，最终发行版本与跨平台 payload 尚未冻结。
生产服务、工作库和源 WAV 未变更；固定 `local_release/` 仍是下述上一轮已验收候选，须在本轮验证后更新。
feat → beta、beta → main、标签、发布以及实际服务部署各须取得相应授权。

## 1. 候选范围与证据身份

本次搜索范围扩充的核心范围包括路径可搜、Vorbis 标签提取、厂商 CSV/XLS/XLSX Metadata 导入并参与搜索。
这三项分别对应原始任务 #204、#205、#206；2026-11-04 用户要求再次核对并将不一致项列入 fix。
用户随后明确要求完成三项任务的全部开发。实现现已包含四种搜索范围与筛选/排序、真实 FLAC/Ogg 标签提取，
以及本地和服务端 CSV/XLS/XLSX 导入；本轮端到端、资源和交付验收仍进行中。
迁移管理、客户端提示/刷新、Default v2 和播放器反馈为配套范围。
完成全部核心功能并验收后才能冻结完整发布范围；schema feature 标记、客户端适配或原型不能代替交付。
原始设计与验收目标见 [服务端设计](Server_Search_Metadata_V2_Design.md)、
[测试计划](TEST-SEARCH-METADATA-V2.md) 和 [交接记录](HANDOFF-CODEX-20261103.md)。

| 输入 | 身份与实际状态 |
| --- | --- |
| 已发布基线 | `v0.8.7`，应用源码 `b5a98ff98fed501aafff0f6a4ef5015c8b4d435d`；历史七包及固定安装记录见 [0.8.7](Rust_Release_0.8.7.md) |
| Search Metadata v2 集成基线 | `feat/search-metadata-v2` 的 `f252620`；包含目录搜索、scope、升级提示及 EM/MG harness |
| 后台服务端任务 | 本地 `d29cd35f215d4aa78a583de09cf57467d31985c5`，第五轮完成；检查时远端 #225 仍为 `47f0205` draft，不能把本地完成视作 PR 已合入 |
| 后台客户端任务 | `feat/search-v2-client-ux2` 原 `1988eef` 合入 `f252620` 后完成，37 文件含合并输入，交付时保留未提交 merge；其最终 diff 已收入候选 `9a13978` |
| beta UI 输入 | `origin/beta` 的 `cfe0118`，含 `b5d7552` 的 Default v2/播放器反馈及 `131bbd6` 的 ignore 更新 |
| 首个合并候选 | `codex/release-090-prepare` 的 `02036a6`，整合上述服务端、客户端和 beta UI；这不是已验收或已发布版本 |
| 较早兼容修正 | `c2d2f8a`：完成 s9 快照读入、owned 校验、增量 Metadata、服务端 relink、本地格式标记和目录旧写入捕获；当时隐藏尚未实现的厂商导入入口 |
| 上一轮已验收/安装源码 | `bebf27211af741f1b3f11b7f2190b60e791fbe08`；当时尚无完整 Vorbis/厂商导入链路，下文历史结果只覆盖当时范围 |
| 本轮实现基线 | `4e77492aa78525fbed1f8c772013ba80a18447f6`，包含 #204–206 实现及新增回归；其后边界修正分别复验，下文记录组合证据，不宣称一次全仓全绿 |
| 迁移与旧 CLI 配对基线 | `d116dcd890fc03c7506c5b514d1338d296e18e68`；真实官方0.8.7 CLI 双快照、旧写拒绝、刷新和离线配对通过；不是最终包身份 |
| 最近完整 MG 身份 | `3562eead704357dcbe6e78bfabc9f083174990c4`，完整MG24/24、41.55秒通过；较早 `fd2e04e` 的40.28秒结果保留，不倒改身份 |
| 最近已提交性能实现 | `0b3f2f7b1de32cf00fe3c4ca46045169b778bbc1`，整页缓存原型；此前 `ffcfbce` 合并 scoped membership/减少重复 Metadata 投影；组合源检查与实际诊断见第5节 |
| 最新已验证改动 | `2c456979872e47cc15f6f52dbe832ad3ed0dd770`：池化升级锁句柄、读取快路径和重复排名维护削减；fmt、DB strict Clippy、35项定向单元、14个DB集成目标123项及2个HTTP目标7项通过；并发诊断仍未达标，不能称发行冻结 |
| 当前提交基线 | HEAD `3562eead704357dcbe6e78bfabc9f083174990c4`，包含固定SQL prepared statement复用；该身份已有原生MG、官方旧GUI和下述性能诊断，仍不是发行冻结 |
| 当前COW候选 | 基于3562eea：整个AudioFile共享不可变Data、写时复制；五包ASIO strict通过，扫描完成事务的缓存维护遗漏已修复，原Vorbis断言通过。后续44条测试目标调用1225通过/0失败/165忽略，与此前通过范围有重叠；性能及最终包仍待验 |
| 后续缓存键优化 | COW及扫描收尾已提交为`20c4024bd3cf31c708f31b9e10f21b5f43e667a8`；其后使用2KiB内联精确序列化查找键，仅在准入时取得所有权；DB strict与61项定向测试通过。容量、代次、revision证明与JSON身份不变，新性能验收待完成 |

### 1.1 三项核心功能核对与 fix 任务

下列复选框表示**完整发布验收及交付**，不把源码存在、单项通过或开发诊断当作全部完成。
原始任务继续使用已有 Issue；已有结果保留实际源码、特性、环境和二进制身份。

| 核心任务 | 已实现并有相应源级/链路证据 | 剩余发布验收 |
| --- | --- | --- |
| CORE-01 / [#204 路径可搜](https://github.com/Freedragonlancer/AudioLib-Explorer/issues/204) | 四种范围、严格目录多词/祖先目录、跨来源 AND、筛选/排序/完整分页和 facets；本地库及真实同步快照链路 | 当前性能 WIP、完整五轮 PG05、最终 GUI/实际包与匹配平台 |
| CORE-02 / [#205 Vorbis 标签提取](https://github.com/Freedragonlancer/AudioLib-Explorer/issues/205) | 真实 FLAC/Ogg/OGA 提取、原文与规范字段、FTS5、增量重提取保全、受支持解码；真实 HTTP 与离线链路已有通过证据 | 最终包中新格式播放/操作、原生 GUI 与平台验收；不以 WAV-only IPC 冒充新增 codec 实测 |
| CORE-03 / [#206 厂商表格导入](https://github.com/Freedragonlancer/AudioLib-Explorer/issues/206) | 正式解析/映射/模板、完整候选分页确认、持久来源/批次/before-image、提交/取消/恢复/回滚、权限、UI 与快照刷新；边界/RSS/退出恢复及真实 HTTP 证据 | PG05 真实导入并发性能仍阻塞；最终 GUI、实际包、平台和安装 |

具体功能边界：

- **#204：** `main` 是主字段（含 tags、notes、BWF/iXML 和新增来源），`filename` 严格限文件名及其分词列；
  `path` 每个词都须在目录/祖先目录命中，`all` 才允许每词跨主字段与目录组合。
  完整主字段层按 BM25/ID，混合/纯路径层按 ID；支持目录、专辑、收藏、Metadata 条件、允许的其他排序、
  精确/下界总数和不混代的续页游标。深层祖先、稀疏/NULL 成员与 facets 有参考集合回归。
- **#205：** 默认扫描 `wav/flac/ogg/oga`，真实格式 probe、FLAC/Vorbis 解码与 seek 已接通。
  Vorbis 原文保存在 `wav_metadata.vorbis`，重复值、中文、未知文本键保留；规范映射及搜索投影独立，
  损坏/截断/无效编码受预算和警告约束，图片/base64 不进 FTS5；提取版本触发增量重读并保留用户及厂商数据。
  Ogg Vorbis、Ogg FLAC 和链式 Ogg 标签取第一条逻辑流；Opus 仅标签 probe，不默认扫描 `.opus`，不承诺 Opus 播放。
- **#206：** CSV/XLS/XLSX 魔数解析、UTF-8 BOM/UTF-16/GB18030、分隔符/RFC4180、多 sheet/表头、
  列映射与模板、相对路径/vendor root/文件名/stem、可选 fuzzy 建议及歧义显式确认均已接通；全部候选可分页查看。
  单文件上限50MiB、20万行、单行文本256KiB；批次最多512文件、累计输入512MiB，另有实际 ZIP 展开安全预算。
  预览在独立 owned 存储中，不改业务库；提交幂等，取消/中断保留状态和来源；完整 before-image 校验、
  语言优先、当前内嵌内容与其他批次保全均有回归。显示按用户 > 厂商 > 内嵌，各来源仍能搜索；
  完成导入/回滚后修订变化并提示刷新快照。权限或服务器能力不支持时隐藏相应入口。验收范围为 CX-01..33。

- [ ] **CORE-01：** 收口最新性能 WIP，满足绝对延迟及 PG05，完成最终 GUI/包的范围、筛选、排序与分页验收。
- [ ] **CORE-02：** 用最终实际包验收格式扫描/提取/搜索/重扫/同步/离线及受支持播放，保持 VB-01..09 和 codec 边界。
- [ ] **CORE-03：** 用最终实际包验收导入/确认/状态/恢复/刷新/回滚和并发性能，保持 CX-01..33 及数据保全断言。

### 1.2 已登记的不一致修正

- [ ] **FIX-01 · P0：范围名称与实际集合一致。** `main` 保留原有 tags、notes、BWF/iXML 等主字段，
  UI 改称“主字段”；新增 `filename` 仅匹配文件名及其分词列。集合回归已通过，最终原生交互待验。
- [ ] **FIX-02 · P0：严格路径多词语义。** `path` 每个词都必须在目录/祖先目录命中；
  目录 `Door` + 文件 `slam.wav` 的 `door slam` 仅由 `all` 跨来源组合命中。参考集合/oracle 已通过，最终包待验。
- [ ] **FIX-03 · P0：路径与筛选/排序组合。** 已接通目录、专辑、收藏及 Metadata 条件，
  `all` 不再因条件退回主字段；参数化 SQL、facets、排序及游标使用一致集合。组合回归已通过，PG05 仍阻塞。
- [ ] **FIX-04 · P1：所有主题的迁移入口。** Studio、Default v2 已接入迁移面板，
  按权限/模式限制动作；源级状态/关闭排空回归已通过，最终包实际打开和操作仍待验。
- [ ] **FIX-05 · P0：排序口径。** 与服务端设计统一：完整主字段层按 bm25/ID；混合和纯路径层按 ID。
  PS-23 已同步，源测试及完整 oracle 保留 `main` 旧顺序并验证混合层参考序列；最终包和性能门槛另验。
- [ ] **FIX-06 · P0：两端迁移和管理边界。** 普通本地库保持 s8 写目标，支持本地目录索引准备、
  新格式扫描和厂商导入；UI 不称其为完整 s9 迁移。显式服务端迁移发布 s9，由专用 writer 写入；
  客户端只读 s8/s9 快照。restore/maintenance 仍受 s8 guard 保护，不能向普通 writer 交入 s9。
  本地新功能验收与服务端迁移分别记录；不承诺旧本地写程序对新增来源投影的保真。
- [ ] **FIX-07 · P0：能力与实际业务。** catalog 已升为2，按角色和实际源库格式返回迁移、
  写入、快照、四种范围及厂商导入能力/限额；真实 Vorbis/厂商生产投影已接通。
  VN-08/09、Metadata 增量与重扫已有源级/HTTP 证据；最终包/部署仍待验，不能只凭 feature 标记通过。
- [ ] **FIX-08 · P0：旧客户端过渡。** 已实现 s8/s9 双快照、精确协商和旧端 s9 写保护；
  派生 s8 只保留 WAV，保留已合入主表的厂商/Vorbis 搜索词；默认开启0.9过渡期兼容快照。
  SC 源级与官方0.8.7 CLI 配对已通过（`d116dcd`）；关闭兼容配置拒绝同步且保留旧缓存。官方旧GUI在 `3562eea` 的声明范围已完成；新候选GUI及最终包仍待验。
- [ ] **FIX-09 · P1：按库提示与去重。** 已改用稳定库身份隔离升级通知，
  库身份/切换/去重已有源级交互回归；最终实际 GUI 待验。

验收说明：本轮已有 MG/快照结果证明当前迁移实现；客户端 mock 仅证明请求/回复包装，
把 WAV 行改成 `.flac/.ogg` 扩展名的夹具不能证明真实标签解析，benchmark 私有厂商表不能证明正式导入。
CORE-02/03 完成前不将“完整搜索范围扩充”或“完整 Metadata v2”标为已验收。

已有证据保留原源码/feature/环境身份，不改写成在 `02036a6` 上新执行：

- 服务端第五轮：Windows MSVC `1.97.1`；fmt、workspace/all-targets strict Clippy、
  test-hooks strict Clippy 通过；workspace 93 targets，1969 passed / 0 failed / 198 ignored。
  DB/server `migration9` 专项共 10/10；完整 MG 24/24，43.54 秒；MG14 再跑 3/3。
  私有任务证据 `schema9/logs/round5-source-identity.json`、`round5-workspace.result.json`、
  `round5-mg-results.json` 及各阶段日志保留源码 SHA256、命令和时间。较早失败仍保留。
- 客户端最终未提交输入：fmt、workspace/all-targets strict Clippy 通过；workspace
  1985 passed / 0 failed / 198 ignored，桌面测试 488 passed / 154 ignored；debug 0.8.7
  离线启动及两项原生关闭通过。私有 `client-ux2` 日志、变更清单及截图保留。
  未做 release 打包、portable 或固定安装，且当时未包含 #225 的完整服务端实现。
- 冷测交接曾记录 20/20、s9/s8 比值 0.807–1.079；它是原基准实现和 macOS 副本的历史证据，
  合并后搜索相关输入是否相同仍须核对。不能当成当前集成候选或百万记录绝对延迟的测量。

以下是上一轮 `bebf272` 候选的历史证据，不能替代当前 #204–206 新增行为的验收：

- fmt、受影响五包的 ASIO 严格 Clippy 通过。五包检查在 fixture 失败前有 51 个通过 targets、
  499 passed / 34 ignored；`bebf272` 修正后 DB 剩余 5 targets / 34 passed，
  desktop/remote/server 的 32 targets / 1147 passed / 159 ignored。按有效身份组合为
  88 targets、1680 passed / 193 ignored，覆盖选定范围，不是一次全仓全绿。
  首次 workspace 的 2008 passed / 2 failed / 198 ignored 原记录保留。
- 原生临时 HTTP 服务完成 s8 同步 → 显式迁移 → s9 完整同步、路径查询和 Metadata 增量 →
  离线缓存重开 → 回滚后重新同步 s8；`migration_snapshot` 1/1 通过，保留库身份、旧缓存字节和未知 Metadata。
- test-hooks 严格 Clippy 及迁移专项 2 targets / 11 passed 通过。MG 首轮 20 项通过，4 项 HTTP 受私有监督器日期类型
  错误影响；只修正 runner 后 4/4 通过，24 项均有通过证据。首轮失败与监督退出记录保留，Rust 源码未改。
- 官方 `v0.8.7` CLI（发布归档及二进制摘要已核对）对同一隔离库先完成 s8 同步，服务端迁移9后
  拒绝同步及写入；缓存 DB、`cache.json`、active 标记字节保持不变，临时服务正常退出。
  私有报告 `mg/runs/old-client-1cbab243aae949d4b8488d77cc3b01d8/report.json` 保留实际配对身份。

以上是上一轮 `bebf272` 及其输入的历史验收；日志、命令、源码/二进制摘要保留在私有 `audiolib-release-090`。
本轮 #204–206、当前缓存及兼容演练的证据在私有 `audiolib-204-206`，不沿用旧包作为新功能通过证明。
Windows x64 ASIO 构建、打包及 portable 7 项通过；`validate-review-fixes.ps1 -CleanCheckout`
10 项、`validate-library-management.ps1` 10 项通过，均使用隔离合成数据，未打开真实工作库或音频设备。
固定安装核对 1252 个 package-owned 文件（更新13个）、CLI `AudioLibCli 0.8.7` 和隔离桌面启动通过；
portable 中文路径索引提示及安装后界面截图已检查。二进制源码身份仍为 `bebf272`，后续纯文档提交不改写该身份。

| 已验收并安装的 EXE | SHA256 |
| --- | --- |
| `AudioLibCli.exe` | `a468d19640b0b01fed103019d6aea3aa5a1641cd93d0e67507ef20cd60093b21` |
| `AudioLibExplorer.exe` | `e383a52466a0ac42a53ab4b9a0e98fc322112e8b94e7b22c85928e8051dda300` |

最初复审入口为 [Draft PR #226](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/226)，目标为
`feat/search-metadata-v2`，不执行 beta/main 提升。后台等待自动跟进已删除，原始 CLI 工作树保留。

## 2. 存储目标与当前实现边界

程序版本、API、常规本地写目标和实际服务端库格式分别记录，不用一个 schema 常量替代所有含义：

| 对象 | 当前源码现状 | 剩余边界 |
| --- | --- | --- |
| 程序 / HTTP | Cargo `0.8.7`；API `/api/v1/` | 范围冻结后统一 Cargo/锁文件/说明至拟定 `0.9.0`；标签尚不创建 |
| 普通本地可写库 | `SCHEMA_VERSION=8`、`TARGET_SCHEMA=8`；本地路径索引构建与重建保持真实 s8 格式，重开保护已覆盖 | 普通本地写入不支持 s9；不能将路径索引准备称为主库升级9 |
| 显式服务端迁移 | `audiolib-migrate` 或管理 API 将完整 s8 发布为 s9；专用 `open_server_existing` 可重新打开 s9；Metadata 和根 relink 已有回归 | restore/maintenance 保持 schema8 保护；实际生产部署及平台验收另行执行 |
| s9 完整格式 | `ale-s9-cld1-t3-m2-p1-a1-sd1`；主 FTS5 tokenizer 为 t3，目录修订为 sd1 | 检查真实 schema/features/结构/身份；`source_search_rev=3` 与三项 feature 原子发布；保持原主 FTS5 不变 |
| 客户端只读快照 | 支持完整格式校验的 schema3..9；s9 原样只读打开，owned staging 检查目录 FTS5、成员、runs 覆盖及无脏队列；真实 HTTP 同步和 Metadata 增量已通过 | 不把 s9 交给 schema8 修复器；未来/不完整格式拒绝并保留旧缓存 |
| `/capabilities` | catalog2、API v1，按实际源库和角色报告读写格式、迁移、四种 scope 和厂商导入限额；普通客户端本地写格式仍为8 | 真实服务角色/格式及旧 JSON 形状解析回归已通过；正式版本及原生部署另验 |
| s8 派生快照 | 已实现 s9/8 双缓存与精确协商；8派生只含 WAV、移除目录与厂商自有表，沿用已有主 FTS5 的 rev3 词和库身份；新端优先9 | router 配对、修订变化及关闭兼容选项回归通过；官方0.8.7 CLI 与 `d116dcd` 的真实兼容配对通过；官方旧GUI在3562eea的声明范围完成；最终新包待验 |
| 厂商导入 / Vorbis | 真实 CSV/XLS/XLSX、本地与服务端完整导入/回滚和 FLAC/Ogg/OGA 提取、重扫、搜索已接通；不支持的服务器隐藏入口 | 较早29项与后续受影响32项目标分别通过，真实 HTTP/离线链路和解析资源演练通过；PG05、原生交互与新包待验。Opus 仅标签，无原生解码 |
| AudioFile COW（当前WIP） | `AudioFileData`保留原字段、类型和serde属性；`AudioFile`以私有Arc持有整条记录，Clone共享、修改时分离，手工serde仍输出原JSON | **公开Rust源码接口变化**：构造使用`AudioFileData`后转`AudioFile`，移出字段先`into_data()`；JSON/API格式、数据库schema、`AudioMetadata`及`attributes: Value`不变。直接消费者和实际性能仍按新身份验收 |
| 服务端交付布局 | 新包包含 server 与不带 test-hooks 的离线 `audiolib-migrate`；仍为七归档十四资产 | layout_revision=2、automatic_upgrade_compatible=false、manual_bootstrap_required=true；minimum_updater_version=0.9.0。冻结0.7.8旧政策真实拒绝，新政策合成验证通过；首次手动引导，开发0.8.7候选不是已发布新更新器 |

目录索引自有 SQL 触发器捕获旧 schema8 写入产生的目录实体/层次及音频成员变化，不依赖新增
scalar 函数。脏队列使 readiness 失效，已有内存缓存也不能绕过；新写入者按目录增量消费，
索引、清队列和 generation 同事务提交。notes/tags 不标脏，统计变化只使 generation 更新。
NULL 归属行分开 runs，避免被路径范围误命中。缺捕获机制的旧 s8 索引须重新准备；完整的
历史 s9 无捕获对象来源仍可只读校验。对象安装/删除核对定义与归属，未知扩展保留。

`-z1` 表示 Metadata 列存储格式，不是快照外层 zstd 传输。
不能因为发送 zstd，就宣称实现了 s9 `-z1` 格式。VN-08/09 的现状和目标区分见
[测试计划第 7 节](TEST-SEARCH-METADATA-V2.md#7-版本协商与提示)。

## 3. 两端兼容与迁移状态矩阵

下表的源码与隔离 HTTP/官方旧 CLI 配对已有相应证据；正式 payload、实际 GUI、真实平台和部署仍分别验收。
旧端取得的是在自有副本中验证过的真实 s8 格式，按接受格式发放；源 s9 不改写。

| 客户端 / 服务状态 | 读取、搜索与缓存 | 写入与提示 |
| --- | --- | --- |
| 旧 0.8.7 → 原 s8 服务 | 沿用已有路径，保留旧快照 | 沿用已支持的 s8 写契约 |
| 新客户端 → 原 s8 服务 | 基础同步继续，主字段与仅文件名可用；缺服务能力时隐藏迁移/厂商入口，路径按实际索引 readiness 开放 | s8 写入仍须通过现有身份与权限检查 |
| 旧/新客户端 → 新二进制、库仍 s8 | 基础读取和旧快照保持可用；新功能只按实际能力开放 | 不因换二进制自动视作库已升级 |
| 回填中 → 客户端快照查询 | 主字段/文件名继续可查；无就绪目录索引时 All 回退主字段、Path 显示待准备。实际查询在客户端快照执行，不宣称已有在线 `/search` 路由 | 普通支持写入可继续；管理迁移状态可读，客户端显示阶段及已提交计数；预留路径查询拒绝契约为503/Retry-After 30 |
| switching | 读取继续；保留当前缓存，拒绝混合代际状态 | 写入 503 `migration_switching`、Retry-After 30；按原请求语义重试，避免重复提交 |
| 新客户端 → 完成 s9 | 精确接受9时总是取得9；完整校验后只读打开，真实全量同步、四种范围、标签/导入搜索、离线重开及回滚重同步有源测试证据 | 客户端快照仍只读；写回经服务端身份/角色/库格式准入；Metadata 与 owned cache 增量已测，导入/回滚后提示刷新 |
| 旧 0.8.7 → 完成 s9，兼容开启 | 只接受8时发放 WAV-only s8，保留已合入主表的厂商/Vorbis词；默认在0.9过渡期保留 | 旧8写契约不能因拿到派生8就写源9，服务端拒绝不匹配写入（409）；给出升级头与能力字段，不承诺旧 UI 呈现新增提示 |
| 旧 0.8.7 → 完成 s9，兼容关闭 | 明确拒绝同步，保留最后已验证 DB、cache 状态及 active 标记 | 旧端写入仍拒绝；关闭配置须由管理员在过渡结束后显式决定 |
| 回滚后 → s8 | 重新确认格式、revision 和库身份，失效旧请求，重新同步；这不是恢复旧数据备份 | 不复用旧 s9 写准入；新写入仍走实际8格式/权限保护，不覆盖工作区、标注或偏好 |

离线、取消、校验失败及不支持的未来格式均须保留最后一份已验证缓存。
新快照只有完成身份、结构与完整性校验后才能发布给 UI；服务端模式不能迁移客户端本地工作库。

## 4. 分阶段执行方案

以下为部署前可审阅方案，当前不执行生产步骤；地址、token、根目录及配置只由私有环境提供。

1. **冻结交付范围。** 完成 CORE-01..03、本轮修正及其验收，记录最终代码、配置、工具链和候选身份。
   按 catalog2、客户端读3–9/普通本地写8、显式服务端写9的边界准备候选，保留主题、工作区和数据保护。
   最终版本统一前不标正式0.9.0。双快照默认开启，不将旧端过渡能力静默删除。
2. **在拥有的副本演练。** 从只读备份输入生成隔离库，登记 s8 格式、library_id、业务记录/标注、
   扫描历史和主 FTS5 基线；验证额外可用空间至少为库大小的 1.3 倍。
   使用隔离随机端口及合成/副本配置，不连接生产或现有 beta 服务。
   完成迁移、取消/恢复、进程退出恢复、重开、完整快照同步及回滚演练。
3. **先准备客户端。** 先交付能连接原 s8 且实测能读 s9 的客户端；保留服务配置、缓存、
   本地库路径和个人工作区。本地路径索引提示可以“稍后”，不能据此改写服务端库或宣称本地 schema 已升 9。
4. **手动引导安装新服务，暂不迁移。** 旧更新器不能接受新增迁移工具的包布局，首次0.9必须手动安装；
   不用放宽冻结旧白名单绕过此限制。核对新布局声明、两个二进制 hash、launcher、配置、数据目录和提交，
   保留旧二进制与备份；此步仍须部署授权。当前 `AppState` 启动调用 `open_server_existing`，不会主动执行 s8→9；
   schema 8 仍走普通已支持的打开路径，较旧库原有迁移另有行为，不能据此保证所有启动均只读。
   `AUDIOLIB_DEFER_SCHEMA9=1` 仅被迁移器 `startup` 动作消费，不能代替真实启动和库版本检查。
   当前 HTTP 管理动作不接受 `startup`。未显式迁移前核对仍为8、健康检查及旧客户端可用。
   仍需旧端使用的窗口中，先不启动非 WAV 补扫；完成9及双快照验收后再启用新增来源。
5. **显式触发并监视。** 授权后，通过支持的管理接口或 `audiolib-migrate start --db <owned-copy>`
   开始；实际工作库路径只在批准的私有运行配置中提供。先确认唯一 owner、权限与库身份。
   监视 backup / path_build / optimize / switch，保持回滚备份；状态 paused/failed 时停止后续推广。
   不删除锁侧文件，不强制终止占用者，不在迁移中 VACUUM。
6. **完成后验收并同步。** 核对实际 `user_version=9`、sd1 格式、三项 feature 和修订，
   确认目录/runs 覆盖、主 FTS5、业务投影及未知 Metadata 保留。
   由新客户端完成真实快照下载、重开、路径查询、播放与受支持写入；核对记录数及库身份，
   保留配置根外不可播放的既有边界。用官方旧客户端另验 WAV-only兼容同步、旧写拒绝、修订后刷新；
   再演练关闭兼容选项的拒绝/保缓存，不把新客户端模拟旧头当作真实旧二进制验收。
7. **推广与交付。** beta/main 提升、标签、正式构建和发布须另获授权；各最终 payload
   从最终身份构建并验收，固定安装同步完成后才报告交付完成。

### 回退与恢复

- 切换前：pause/cancel 在持久批次边界生效；确认终态及 s8 可用，保留 checkpoint/备份供诊断或恢复。
- 切换后：当前目录迁移器支持验证备份后、持独占管理与扫描 lease 的逆向事务回滚，
  保留备份后的业务内容及普通注释，删除目录派生机制和9标记；当前实现不会把厂商正式数据或新增格式音频变成旧写者完全理解的数据。
  必须按最终 recipe、实际新增功能及原生包再验证，不能把 `user_version=8` 等同旧二进制写入保真。
  不将历史设计中的“直接恢复整个备份”作为默认操作，否则可能覆盖新写入。
- 若新增 Vorbis、厂商字段或其他 s9 业务数据后不能无损退回 s8，停止自动回退，先评估和保全数据。
  不向旧二进制交付不支持的库，也不靠删除新增业务数据获得兼容；完整备份恢复另需授权及写入排空，并明确备份之后数据的处置。
- 所有回退保留源 WAV、原备份、Metadata、标注及未知扩展；不以重扫代替数据保全。

## 5. 最小验证范围与发布阻塞

集成候选后续修改由实际风险选择检查；已有全仓结果不能覆盖新合并行为。

| 阶段 | 所需证据 | 当前 |
| --- | --- | --- |
| 契约与文档 | schema/features/能力/Retry-After 一致；链接及差异检查 | catalog2、switching30、实际格式和能力已统一；本次仅文档收口，不产生新的运行验收 |
| 受影响源码 | fmt、strict Clippy、相关测试及直接消费者 | COW五包ASIO strict通过；Core187及DB lib326通过/5忽略。原Vorbis失败保留且修正后完整通过；后续DB strict、server NoDefault strict通过，44条目标调用1225通过/0失败/165忽略，复用此前未受影响结果，不累加为唯一个案总数 |
| 迁移安全 | MG24、跨进程控制、备份、取消/恢复、切换拒写 | 最近完整执行为 `3562eea`，24/24、41.55秒通过；COW及后续扫描修正仍按影响复验。保留旧 BUSY/监督器/报告替换竞争及修正日志。演练不等于实际 VM 断电、配额/全部文件系统或最终 production migrator 包验收 |
| 两端真实 HTTP / 旧客户端 | 全量/增量、离线、回滚、实际官方旧二进制 | `2db7efd` 的5个同步目标16项、rooted7项通过；官方0.8.7 CLI与d116dcd配对通过。官方0.8.7原生GUI对3562eea完成O1–O12声明范围；独立旧写HTTP409、拒同步保缓存和冷离线重开有证据，均不是新候选正式包验收 |
| 搜索正确性 / 解析资源 | 完整 oracle/分页/深目录/NULL、表格RSS/边界、恢复 | `fd2e04e` 八范围完整 oracle/分页通过；18项资源边界、10k/200k CSV/XLSX、近50MiB有效 BIFF和真实退出恢复已有原身份证据。当前优化后最终完整 oracle/分页尚待复验，不把微基准代替集合正确性 |
| 百万记录性能 | 绝对 P95 <500ms；PG05五轮中位数倍率≤1.1 | **BLOCKED**。20c4024单轮64b81806诊断warm100+facets P95=216.5822ms、结果页P95=0.043ms；写入期查询P95为0.0093–0.0122ms，但倍率仍1.140–1.175。后续缓存键优化待复测，单轮不能代替五轮 |
| 桌面交互 | 范围/分页、导入、迁移、关闭排空及两个现代主题 | 已有源级交互/异步回归；原507项、后续定向与父/子进程结果各保留原身份。官方旧GUI声明范围已完成；新候选原生GUI、新格式播放和最终包仍待验 |
| 原生文件系统 | 根句柄、复制/替换、权限与限额 | Windows本地NTFS7项、自有SMB/UNC6项通过；另1项创建测试链接返回OS1，该链接替换场景未验证。不能用该平台结果代替 Mac/Linux 原生路径 |
| Windows实际包 | 客户端ASIO；server+migrate双二进制；许可/隐私/启动/数据安全 | 本轮尚未构建/打包。client479/server260许可闭包及合成材料/边界检查通过，含C#假二进制与53项Docker合成检查；无真实Docker daemon/image执行证明，旧 `bebf272` 包不替代本轮 |
| 最终安装 | 固定目录所有package-owned文件SHA256、CLI版本、隔离桌面启动 | 仍为上轮 `bebf272`、1252文件安装；实际新包通过后才更新主工作区固定 `local_release/`，候选工作树不创建另一个日常安装 |

较早的主性能诊断 run `b3df63be340b4718af8c0d66ef85733c`，源码 `3562eea`，
`diagnostic_only=true`。warm100+facets P95=213.9338ms、结果页P95=0.1073ms；严格PG05仍阻塞。
它使用原 `d116dcd` 五轮导入/回滚后的百万记录语料，经只读SQLite online backup复制到新owned目录；
原DB/WAL/报告保持不变。OS cache未purge，fresh process/provider不称真实冷缓存。

| PG05请求 | 空闲p95（ms） | 写入p95（ms） | 单轮倍率（上限1.1） |
| --- | --- | --- | --- |
| Main / relevance | 0.0407 | 0.0410 | 1.00737，在限内 |
| All / relevance | 0.0563 | 0.0498 | 0.88455，在限内 |
| Path / 收藏+48kHz / Filename排序 | 0.0539 | 0.0401 | 0.74397，在限内 |
| strict Filename / Filename排序 | 0.0318 | 0.0530 | **1.66667，未过** |
| 空中文 strict Filename / relevance | 0.0124 | 0.0109 | 0.87903，在限内 |

只达到4/5个单轮诊断边界，不能宣布正式PG05通过。phase `441df` 的分段诊断发现整条DTO深克隆开销：
Main的JSON约400字节且未变化，Clone仍由约20.6增至36.5微秒；Filename JSON约400增至3806字节、
包含约51行vendor数据时，Clone约20.7增至46.7微秒。这支持随后实现的**整个AudioFile COW**，
不把attributes-only原型视为已经解决瓶颈。临时phase计时模块已按原字节精确移除；
最新完成的 run `64b81806b51d4f3e9cd8de46fa1b3c3b` 使用已提交COW源码 `20c4024`：
warm P95=216.5822ms、结果页P95=0.043ms；五类写入P95依次0.0101、0.0121、0.0122、0.0102、0.0093ms，
倍率1.14773、1.17476、1.14019、1.15909、1.14815，严格门槛仍未通过。
其后2KiB内联查找键已接入并通过61项定向回归；它是候选实现，性能验收尚待执行。

原有失败和计时身份继续保留：`0b3f2f7` 的a58单轮warm227.8722ms/结果页0.1408ms，
五类倍率依次1.35794、1.77593、1.41844、1.49294、1.32733；`2c45697` 的f2eaba诊断亦未达1.1。
较快查询曾触达50000条采样保护上限，`41401b`中止夹具不复用、不算通过。
私有helper现两侧均在public search返回即停表，校验/记录在计时外；样本保护上限500000，
180秒截止、完整oracle、实际导入、取消/join排空、至少3批和每类30条真实重叠样本、正式五轮与1.1门槛均保留。
批次work interval包括取数、投影/预维护、提交和状态，不是精确SQL持锁时间；未对旧结果追溯重算。

较早结果继续保留原条件：`d116dcd` 百万记录100查询P95=112.537ms，扩展 warm P95=443.843ms/结果页329.371ms；
原五轮 PG05 失败。`fd2e04e` 的 run `07c5d07da8724b568898f13cb61fcc20` 完整 oracle通过，
warm729.5957ms/结果页521.2298ms及五轮 PG05均失败；`ffcfbce` 的 scoped SQL去重随后改善结果页。
`0b3f2f7` 之后曾因私有 helper 将 All 首屏下界201误当精确总数，遇到合法精确50000而断言退出；
原失败夹具/报告保留，改用可信完整 oracle并保证校验错误先取消/join后，才有上述 a58完成诊断。
这些运行的源码、物理FTS/历史状态及计时版本不同，不用前后耗时单独推断某缓存导致退化，也不相加为一次通过。

本轮涉及扫描协调、Metadata保全、导入/relink、关闭及格式播放。fresh accepted portable就绪后，
保留五个现有数据安全/真实二进制回归：`validate-review-fixes.ps1 -CleanCheckout`、
`validate-library-management.ps1`、`validate-access-denial.ps1 -CleanCheckout`、
`validate-desktop-close.ps1 -Cycles 3`、`validate-playback-ipc.ps1 -CleanCheckout`，均显式指向该 portable。
实际 GUI 单独协调窗口，避免与性能/服务器竞争；脚本的合成数据、NTFS ACL恢复、正常退出/排空边界保留。
真实 legacy fixture、新增格式播放和平台/硬件覆盖分别报告，不用旧 WAV-only 或模拟结果替代。
ASIO 使用已验证 SDK 和实际发布配置，不再追加无关的非 ASIO 全矩阵。

交叉复核另修正厂商任务状态：无完成回执且已开始回滚的任务不能再续提旧commit断点，
准入拒绝保留原rollback诊断、进度及来源；已有完成回执仍按原键返回不可变历史结果。
新增真实部分回滚失败和首批回滚失败两项回归，重复拒绝后仍须能完成回滚恢复。

并发修正的数据库包曾完整运行58目标、581通过/0失败/34忽略，保留实际测试EXE身份。
之后的扫描入口与Windows报告锁分别按受影响目标复验：真实FLAC/Ogg/OGA热查询重提取2项、
扫描边界/控制各4项、厂商32项，以及上述迁移25项均通过；不是一次最终源码的全仓全绿。
远程包级297项通过，另有1项更早定向观察，不能重复累加为298个唯一用例；服务端早前目标、
修复后原失败5目标25项及余下7目标42项分别保留身份。
`ffcfbce` 的57个DB顶层目标587通过/34忽略，server2目标7通过、fmt/DB strict通过。
`0b3f2f7` 对应源检查的DB lib曾309通过、1个取消钩子用例失败，修复后定向8个取消及5个controller通过；
其余56个DB目标294通过/29忽略，server2目标7通过，fmt/DB strict通过。
较早pool/快路径/排名去重检查的35项单元、123项DB集成及7项HTTP，合计165通过/0失败，
对应后来的 `2c45697` 实现；这些历史执行有重叠，不继续累计成workspace总数。
当前31文件COW输入在3562eea基线上：选定core/db/remote/desktop/cli五包ASIO strict已通过；
Core187通过（含6个新COW行为测试），DB lib326通过/5忽略（含真实provider/cache COW回归）。
选定测试已完成59个目标823通过；第60个vorbis_scan目标为1通过/1失败，累计观察824通过、1失败、34忽略后中止。
失败位于 `vorbis_scan.rs:250` 的阳性cache_hit断言；它尚不证明结果或Metadata陈旧，也不能改写成通过。
随后修正扫描完成事务：IMMEDIATE准入后，在同一实际最终图像中准备canonical页面，成功提交后经原有Dw/revision证明发布；
不重标旧DTO、不放宽原断言。`owned-file-validation-finalize-bf7bb9968ca04a65b54960d0816f1393` 完成复验：
取消/缓存/扫描/Vorbis/Waveform相关47通过；Desktop/remote实际ASIO的8条目标调用809通过/162忽略；
server NoDefault的25条目标调用369通过/3忽略；DB及server strict通过，源码前后身份一致。
合计44条目标调用1225通过/0失败/165忽略；过滤DB-lib重复使用同一binary，部分扫描目标与先前结果重叠，不能直接与823相加。
此前Core、CLI和未受扫描收尾影响的DB结果保留原始输入/特性/二进制身份，形成组合证据，非一次完整workspace重跑。
新候选原生GUI、实际payload与固定安装仍未交付。

最小证据定位：MG `final-3562eea-full24-42dd609abce047f8a2e5b4644cfe76d7/result.json`；
官方旧GUI `old-gui-e9a8c716ab574d98b2fee7a2d9b82bcd/native-gui-final-receipt.json`，绑定源码3562eea、
官方0.8.7 GUI SHA256=`859541a34d4eb05a04ddcda4f92d88596253235bc8ad1ed66a723999489faf81`。
旧GUI在隔离服务中s8初次449条→迁移后WAV-only派生8的409条，vendor搜索通过；
GUI禁止编辑/保存，不声称它发出禁止的PATCH；独立同旧契约HTTP写入409且服务器/缓存未改。
兼容关闭后同步明确失败且DB/cache字节保留；服务端正常停止后，官方旧GUI冷重开离线vendor搜索通过，全部正常退出0。
它完成O1–O12声明范围，不覆盖新候选GUI、正式server包或其他平台。
官方旧CLI证据仍为 `old-client-ed106a3ddcf6429691f204efbea81a5f/report.json`（candidate=d116dcd，
旧归档SHA256=`2ee3f3984e570ffadfe62ec052be2d0707e2845e0aa003434b2f337267fa6ba7`，
旧CLI SHA256=`f6fcd7d803311712d96ca704535dcafc5428d538fd770af1374749991fc7fb6f`）。
性能为上述b3df的 `search/pg05-probe-result.json` 与独立warm报告；phase441df仅用于归因。
COW检查保留 `owned-file-validation-*` 的实际输入/特性/EXE及停止位置，修正后重验使用新身份；
既有报告及计时修正收据不覆盖，不将变更的helper重绑定为旧编译。

性能目标仍为 **P95 latency <500ms for 1M+ records**；s9/s8 相对闸门为原约定的
每查询五轮、每轮 p95 的中位数不超过 s8 ×1.2；PG05另要求并发/空闲五轮p95中位数≤1.1。
绝对延迟、s9/s8及并发倍率分别报告，缺少真实冷缓存/平台条件时明确未验。

剩余发行条件：CORE-01..03 与关联 P0/P1 的最终实际验收、当前 WIP 源检查及完整 oracle/严格 PG05，
以及 GUI、原生平台、实际包/固定安装和授权后的部署尚未完成。方案已明确客户端先就绪 → 手动引导新服务、库保持8
→ 管理员显式迁移；双快照窗口结束及生产执行另需决议。最终版本和 payload 尚未冻结。
上一轮 Windows 开发候选本机交付已完成；本轮新增行为尚未本机交付，也不能当作最终 main/tag 的正式发行 payload。
上一轮候选缓存维护回收逻辑空间 88.44 GiB，保留 release 后剩余2.58 GiB，当时40 GiB 预算达成；本轮收尾再维护。
新登记的私有 MG 缓存5.73 GiB因缺真实 Cargo `CACHEDIR.TAG` 被清理工具拒绝，已保留，未伪造标记；
本次两处缓存合计约8.31 GiB。清理工具的原始成功/失败记录均保留。
原后台任务曾报告外部未登记缓存约 140.52 GiB / 80.97 GiB，回收 0，40 GiB 预算未达成；
不自动清理未登记历史缓存，不将该限制隐藏为清理成功。

## 6. 七包、固定安装与完成条件

范围和版本冻结后先按 [版本规范](Rust_Versioning.md) 及 [发布管线](Rust_Release_Pipeline.md)
预检，再以最终 main/tag 身份在匹配原生主机构建各 payload 一次、验收一次。
目前 Cargo 未改至 0.9.0，也未创建对应标签。若先交付 Windows 开发候选，须在最终冻结后
按实际 Cargo `0.8.7` 标识构建和验收；当前尚未交付，不代表正式0.9.0或七包发行完成。

| 公开归档（`<tag>` 为最终批准标签） | 验收与更新边界 |
| --- | --- |
| `AudioLibExplorer-<tag>-win-x64-portable.zip` | 实际 ASIO 配置、portable 验收、固定安装 |
| `AudioLibExplorer-<tag>-macos-arm64.zip` | `package-macos-client.ps1 -ValidateRuntime` 合并结构/签名/原生启动退出；ad-hoc不等于Apple公证，手动更新 |
| `AudioLibServer-<tag>-win-x64.zip` | server与离线migrate双身份、隔离health及8→9→8；首次手动引导；显式构建acceptance所需benchmark helper |
| `AudioLibServer-<tag>-macos-arm64.tar.gz` | 两个工具的原生启动、迁移/重开及包身份；首次手动引导，不自动部署生产 |
| `AudioLibServer-<tag>-linux-x64.tar.gz` | 两个production工具的匹配环境/文件系统与8→9→8验收；首次手动引导，仿真须明确记录 |
| `AudioLibServer-<tag>-docker-linux-amd64.tar.gz` | 消费已验收Linux双二进制输入；实际离线load/run、UID/只读/health/生产保护及两hash校验 |
| `AudioLibServer-<tag>-docker-linux-arm64.tar.gz` | 同上；独立 Linux arm64 原生包是内部输入，不列公开资产 |

各归档附 `.sha256`，合计十四项资产。Docker 通过替换镜像升级，禁止在容器内自动换二进制；
Mac 客户端按既有手动流程更新。发布核验只核远端元数据/摘要，不能替代本机迁移和包验收。

Windows 必须按 [仓库固定安装流程](../AGENTS.md#mandatory-local-application-delivery-and-release-installation)
将已通过 portable 验收的完整包通过**主工作区** `tools/install-local-release.ps1 -Source <候选portable>`
安装至该主工作区唯一固定 `local_release/`；候选工作树不另建日常安装。正常关闭并排空写入/扫描、
只复制 package-owned 文件、不清空目录，核对所有文件 SHA256、CLI 版本及一次显式隔离数据目录启动。
保留日常库、settings、日志、缓存和用户原环境；beta 独立安装不能替代固定安装。
若以后获准发布，再将固定安装与实际发布资产摘要闭环核验，分别报告远端发布和本地同步结果。
交付成功后执行一次有归属证据的缓存预算维护，保留 release 输出与诊断要求。
