# Search Metadata v2 边界用例清单与回归测试计划

- 基线：v0.8.7 / commit b5a98ff / schema 8；feat 分支 `feat/search-metadata-v2`（自 beta 4ff16a5）
- 关联 issue：#204 路径搜索、#205 Vorbis/FLAC/OGG、#206 厂商 CSV/XLS 导入、#207 总跟踪
- 优先级：P0 = 阻塞合入 feat 分支；P1 = 阻塞合入 beta；P2 = 可后续跟进
- 验证方式：U = 单元测试，I = 集成测试（临时 DB/样本库），C = CLI/脚本，G = 手工 GUI

## 0. 现状代码路径（用例锚点）

| 位置 | 说明 |
|---|---|
| `crates/audiolib-db/src/schema.rs:17` | `SCHEMA_VERSION = 8`，`MIN_SNAPSHOT_SCHEMA_VERSION = 3`（:21） |
| `schema.rs:~1307-1311`、`lib.rs:~2837-2840` | 基础搜索 = 单个 FTS `MATCH`，字段为文件名、用户标签、BWF description、iXML scene/take、notes 及分词变体；路径不入索引，文件夹只做过滤 |
| `schema.rs:1075`、`audiolib-core/src/metadata.rs:94-127` | UCS CatID/Category/SubCategory/VendorCategory 拼接进 description token |
| `wav.rs:218-238` | ID3（WAV 内 text、COMM、TXXX）提取 |
| `remote_mode.rs:114/156` | 服务器模式打开只读快照，执行同一套 SQL |
| `lib.rs:151` | 查询被中断时返回 `OperationInterrupted`，取消逻辑依赖它 |

规模基线：约 90.9 万文件，DB 约 10.6 GB，快照压缩后 959 MB。

---

## 1. P0 性能门禁（Performance Gate）

**规则：只要有一项超出门禁，就不能合入 `feat/search-metadata-v2`。** 允许多用 CPU，但延迟不能变长。基准数据由 server bot 用 8790 生产库的**副本**提供（不碰生产服务）。

| ID | 场景 | 预期 / 门禁 | 验证 | 优先级 |
|---|---|---|---|---|
| PG-01 | 固定查询集：中文、英文、1–2 字符短词、路径片段、UCS CatID、多词组合，每类 ≥10 条 | schema 9 的 p50/p95 ≤ schema 8；冷缓存（重启进程并清 OS 缓存）和热缓存各测一次 | C：bench 脚本，同机同库副本，每条跑 ≥30 次 | P0 |
| PG-02 | 新增路径片段查询 | p95 ≤ schema 8 文件名查询的 p95 | C | P0 |
| PG-03 | 路径搜索的 SQL 计划 | `EXPLAIN QUERY PLAN` 不能出现针对 path 的 `LIKE '%..%'` 全表扫描，必须走 FTS | U：对生成的 SQL 断言执行计划 | P0 |
| PG-04 | Vorbis/CSV 字段的接入方式 | 合并进现有 FTS 表/列，搜索时不能多出 JOIN（和 schema 8 的执行计划对比） | U：执行计划断言 | P0 |
| PG-05 | 后台导入 CSV 或重新提取 Vorbis 时并发搜索 | 搜索 p95 不超过空闲时 p95 的 1.1 倍；写事务分批提交，读不被阻塞（WAL） | I：边导入边跑 bench | P0 |
| PG-06 | 快速连续输入（防抖） | 防抖窗口内只发一次查询；新查询发出时取消上一次（走 `OperationInterrupted`，lib.rs:151），不出现旧结果覆盖新结果 | U + G | P0 |
| PG-07 | 索引体积增长 | 记录 DB 和 FTS 的大小（前后对比）；目标增长 ≤15%，超出需设计确认 | C | P0（必须记录） |
| PG-08 | 快照大小与首字节时间 | 记录压缩快照大小和服务器首字节时间；首字节时间不能变长 | C：curl 计时 | P0（必须记录） |
| PG-09 | 服务器模式下同样的查询集 | 远程 p95 ≤ schema 8 远程 p95 | C | P0 |

---

## 2. 路径搜索（#204）

| ID | 场景 / 输入 | 预期 | 验证 | 优先级 |
|---|---|---|---|---|
| PS-01 | 路径长度 >260、带 `\\?\` 前缀 | 能索引，也能搜到；前缀不产生 token；搜索结果和打开文件都正常 | U：分词；I | P0 |
| PS-02 | UNC 路径 `\\nas\share\SFX\...` 和映射盘 `Z:\` | 服务器名、共享名按段切分；同一文件不因 UNC 和盘符两种写法被重复索引 | U + I | P1 |
| PS-03 | 中文、日文、emoji 路径段（如 `音效库/爆炸`、`効果音`、`🔥fire`） | 中文能按词或二元组搜到；emoji 不导致崩溃或乱码 token | U | P0 |
| PS-04 | 分隔符混用 `a\b/c`、连续分隔符、末尾分隔符 | 统一规范化，切出的段一致，不产生空 token | U | P0 |
| PS-05 | 大小写（`Foley` 和 `foley`） | 不区分大小写，和文件名搜索的折叠规则一致 | U | P1 |
| PS-06 | 文件或文件夹重命名、移动后重新扫描 | 旧路径 token 删除，新路径能搜到；FTS 中没有残留行 | I | P0 |
| PS-07 | 文件夹过滤加路径关键词，且关键词就是过滤目录名 | 结果是两个条件的交集，不重复；过滤语义和 schema 8 相同 | I | P0 |
| PS-08 | 排序：查询词同时命中文件名和路径 | 文件名/描述命中排在只命中路径的前面（路径列权重更低）；整个目录都命中时，前 50 条结果不能全被路径命中占满 | I：在已知样本上断言排名 | P0 |
| PS-09 | 盘符、通用段（`X:`、`Users`、`SFX`） | 盘符不入索引；通用段是否作为停用词，见第 11 节待确认问题 | U | P2 |
| PS-10 | 路径中有 FTS 语法字符（`"`、`*`、`-`、`(`、`:`、`NEAR`） | 输入被转义，不报 FTS 语法错误 | U | P0 |

## 3. Vorbis / FLAC / OGG 标签（#205）

| ID | 场景 / 输入 | 预期 | 验证 | 优先级 |
|---|---|---|---|---|
| VB-01 | FLAC metadata block 损坏或被截断 | 跳过该文件的标签提取并记录警告，不 panic，扫描继续 | U：fuzz 样本 | P0 |
| VB-02 | 超大 comment block（如 16 MB） | 有大小上限，超出部分截断；内存占用有界 | U | P0 |
| VB-03 | 同一个 key 出现多次（如多个 ARTIST） | 所有值都保留并能搜到 | U | P1 |
| VB-04 | 值不是 UTF-8 | 有损解码，不丢整条记录、不 panic | U | P0 |
| VB-05 | 值为空、只有 `KEY=`、没有 `=` | 忽略，不产生空 token | U | P1 |
| VB-06 | 带嵌入图片（METADATA_BLOCK_PICTURE / FLAC PICTURE） | 不进 FTS，不把 base64 当文本 | U | P0 |
| VB-07 | Opus（OpusTags）、Ogg Vorbis、OGG 封装的 FLAC、链式 OGG | 都能提取；链式 OGG 至少取第一段 | U + I | P1 |
| VB-08 | 未知 key、key 大小写混用 | key 统一大小写；未知 key 按白名单策略处理（见待确认问题） | U | P1 |
| VB-09 | 已入库的 FLAC/OGG 文件需要重新提取 | 有触发机制；重提取在后台运行，不阻塞搜索（见 PG-05） | I | P0 |

## 4. 现有元数据回归

| ID | 场景 / 输入 | 预期 | 验证 | 优先级 |
|---|---|---|---|---|
| EM-01 | BWF bext 损坏、长度字段越界 | 和 schema 8 行为一致，不 panic | U | P0 |
| EM-02 | iXML 格式错误或编码错误 | 跳过 iXML，其他 chunk 照常提取 | U | P0 |
| EM-03 | ID3 COMM/TXXX 损坏（wav.rs:218-238） | 不 panic | U | P0 |
| EM-04 | GBK、Shift-JIS、Latin-1 编码的 INFO/bext 文本 | 解码结果和 schema 8 一致（不能新增乱码） | U：golden 样本 | P1 |
| EM-05 | 超大 chunk（单个 chunk ≥100 MB 的声明长度） | 有上限，不 OOM | U | P0 |
| EM-06 | 基线命中数（r824 快照）：`zildjian`=323、`electrovoice`=228、`sortilege`=2776、`personage`=5943 | schema 9 的命中数 ≥ 基线，且基线命中的结果全部还在（新增结果只能来自路径或 Vorbis 字段，并逐条列出） | C：命中数脚本 + ID 集合对比 | P0 |
| EM-07 | UCS CatID/Category 拼接（schema.rs:1075，metadata.rs:94-127） | token 不变，按 CatID 搜索的命中数一致 | U + C | P0 |
| EM-08 | 用户标签、notes 搜索 | 和 schema 8 一致 | I | P0 |

## 5. 厂商 CSV/XLS 导入（#206）

| ID | 场景 / 输入 | 预期 | 验证 | 优先级 |
|---|---|---|---|---|
| CX-01 | 空文件 | 提示“无数据”，不写库 | U | P0 |
| CX-02 | 只有表头 | 同 CX-01 | U | P1 |
| CX-03 | 缺少文件名/路径列，或列被改名 | 进入列映射界面；缺少匹配键时禁止导入 | U + G | P0 |
| CX-04 | UTF-8 BOM、UTF-16 LE/BE、GBK | 自动识别编码或可手动选择；预览不乱码 | U | P0 |
| CX-05 | 分隔符为 `,`、`;`、`\t`、`\|` | 自动探测，也可覆盖 | U | P1 |
| CX-06 | 引号内换行、转义引号 | 按 RFC 4180 解析 | U | P0 |
| CX-07 | 以 `=`、`+`、`-`、`@` 开头的单元格（公式） | XLS 读缓存值，不执行公式；导出时防公式注入 | U | P1 |
| CX-08 | 合并单元格 | 按左上角的值处理，或在预览中给出警告 | U | P2 |
| CX-09 | 多个 sheet | 让用户选择 sheet，默认第一个 | G | P1 |
| CX-10 | 10 万行以上（以及 100 万行压力测试） | 流式解析；分批事务；有进度条；不阻塞搜索（PG-05） | I + C | P0 |
| CX-11 | 一行匹配到多个文件（同名文件在不同目录） | 标记为歧义，默认不写入，在预览中列出 | I | P0 |
| CX-12 | 行匹配不到任何文件 | 计入未匹配并可导出报告 | I | P1 |
| CX-13 | 按文件名匹配和按相对路径匹配；大小写和分隔符差异 | 匹配规则与 PS-04、PS-05 一致 | U | P0 |
| CX-14 | 导入值和用户标签冲突 | 按设计的优先级处理（默认不覆盖用户标签）；在预览中显示 | I + G | P0 |
| CX-15 | 同一个文件重复导入 | 幂等，不重复追加 token | I | P0 |
| CX-16 | 回滚一次导入 | 精确恢复导入前的状态（包括 FTS），不影响其他批次 | I | P0 |
| CX-17 | 导入中途取消或进程被杀 | 已提交的批次可以回滚或续导；不留下半批数据 | I | P0 |
| CX-18 | 服务器模式（只读快照） | 导入入口禁用或给出提示 | G | P1 |

## 6. 迁移 schema 8→9

| ID | 场景 | 预期 | 验证 | 优先级 |
|---|---|---|---|---|
| MG-01 | 迁移中途断电或被 kill | 重启后从断点继续，最终结果和一次跑完一致 | I：在多个批次点注入 kill | P0 |
| MG-02 | 磁盘空间不足（备份阶段或重建阶段） | 迁移前预检所需空间；失败时保持 schema 8 可用 | I | P0 |
| MG-03 | 自动备份失败 | 中止迁移，不修改原库 | I | P0 |
| MG-04 | DB 文件只读 | 不尝试迁移，以只读方式打开或给出明确提示 | I | P0 |
| MG-05 | DB 被另一个进程锁住 | 等待后提示，不损坏数据 | I | P0 |
| MG-06 | 10.6 GB 库的迁移耗时 | 记录总耗时和每批耗时；有进度显示，可取消 | C | P0（必须记录） |
| MG-07 | 回滚到备份 | 回滚后 schema 8 客户端能正常打开，搜索结果和基线一致 | I | P0 |
| MG-08 | 迁移跑两次 | 第二次是空操作 | U | P0 |
| MG-09 | 降级：旧客户端打开 schema 9 库 | 旧客户端拒绝打开或以只读模式打开，并给出提示，不能写坏库 | I | P0 |
| MG-10 | FTS 重建后执行 integrity-check | FTS 和主表的行数一致 | U | P0 |

## 7. 版本协商与提示

| ID | 场景 | 预期 | 验证 | 优先级 |
|---|---|---|---|---|
| VN-01 | 旧客户端连新服务器（schema 9 快照） | 能降级使用基础搜索，或明确提示升级；不崩溃 | I | P0 |
| VN-02 | 新客户端连旧服务器（schema 8） | 隐藏路径搜索和 Vorbis 相关功能，基础搜索正常 | I | P0 |
| VN-03 | 快照 schema 低于 `MIN_SNAPSHOT_SCHEMA_VERSION`，或高于客户端支持的版本 | 给出清晰的错误信息 | U | P0 |
| VN-04 | 服务器 capabilities 字段缺失或未知 | 视为旧服务器 | U | P1 |
| VN-05 | “本地库需要迁移”的提示 | 文案准确；可以“稍后”关闭；同一版本只提示一次 | G | P1 |
| VN-06 | 服务器版本或 schema 与客户端不匹配的提示 | 每个会话只提示一次，不重复弹窗 | G | P1 |
| VN-07 | 客户端有更新可用；离线时检查更新 | 离线时静默失败，不阻塞启动 | G + I | P1 |

## 8. 本地模式与服务器模式一致性

| ID | 场景 | 预期 | 验证 | 优先级 |
|---|---|---|---|---|
| PA-01 | 同一个库，本地模式和服务器模式（remote_mode.rs:114/156）跑 PG-01 的查询集 | 结果 ID 集合与排序一致 | C | P0 |
| PA-02 | 文件夹过滤加路径搜索 | 两种模式一致 | I | P0 |
| PA-03 | 服务器端路径和客户端看到的路径前缀不同 | 远程路径映射后，路径 token 仍然一致 | I | P1 |

## 9. 合入 beta 前的回归冒烟清单

1. PG-01 到 PG-09 门禁全部通过，数据附在 PR 中。
2. EM-06 的 4 个基线命中数和 ID 集合对比通过。
3. 在 10.6 GB 库副本上完成一次 8→9 迁移 + MG-01 kill/续跑 + MG-07 回滚。
4. 本地/服务器一致性 PA-01 通过。
5. VN-01、VN-02 新旧版本交叉测试通过。
6. 导入一份 10 万行样本 CSV，然后回滚（CX-10、CX-16）。
7. `cargo test --workspace` 全绿；在 GUI 中手动搜索中文、路径片段，并验证防抖与取消。

## 10. 待设计确认的问题

1. 路径 token 的权重比例，以及是否把通用段（`SFX`、`Users`）当停用词？
2. 中文分词用 bigram 还是词典分词？这会影响索引体积和 PG-07 的阈值。
3. Vorbis 用白名单（TITLE/DESCRIPTION/COMMENT/GENRE/…）还是全部 key 入索引？
4. CSV 导入和用户标签冲突时，默认优先级是什么？导入的数据是写进 tags 还是独立字段（不能多出 JOIN）？
5. 旧客户端遇到 schema 9 快照时，应该硬拒绝还是只读降级？
6. PG-07 索引增长 15% 的上限是否可以接受？

## 附录 A. 真实厂商元数据表格普查与解析基准（2026-11-03）

只读采集，没有修改素材库。素材根目录是 `\\Z4S-PAEH\hdd\A_Lib`，服务器镜像库 `library-r824` 中共 909,342 条 `audio_files`，有 38 个一级厂商目录。文件本身受授权限制、体积也大，所以没有入库，这里只记录特征。

### A.1 普查结果

- 按名称匹配（`*.csv/*.tsv/*.xls/*.xlsx`，或文件名含 metadata/soundminer）共找到 610 个文件，合计 182 MB。其中表格类 449 个：`.xls` 209、`.csv` 127、`.xlsx` 103、`.txt`（Soundminer 导出）10。另外还有 PDF 125、PNG/JPG 30、`.numbers` 1，以及 Synology 的 `@synoeastream/@synoresource` 旁路文件。
- 文本编码（137 个 csv/txt）：UTF-8 117、ASCII 7、UTF-8 BOM 3、**UTF-16LE BOM 10**（9 个 Soundminer `.txt`，加上 1 个扩展名为 `.csv` 的 Tab 分隔文件）。**没有发现 GBK 或 cp1252**，也没有解码错误。
- 分隔符：`,` 122、`\t` 11、`;` 4。
- 工作簿：多 sheet 的有 57 个（最多 12 个 sheet）；有 15 个文件的行宽不一致（ragged）。
- 解析失败 5 个：2 个 Sonniss `*.csv.xls` 实际是纯文本 CSV；2 个 SoundMorph `*.xls` 实际是 AppleDouble 资源叉（魔数 `00 05 16 07`，内容 "Mac OS X"）；1 个 `~$*.xlsx` 是 Office 锁文件（0 KB）。
- 常见表头：Filename 366、Keywords 256、Description 255、Category 221、Library 211、Manufacturer 199、Source 196、BWDescription 173、SubCategory 162，也有 CatID、FXName、RecID、TrackTitle 等。很多文件第 1 行是标题或厂商名，真正的表头在第 2～3 行。

### A.2 代表性样本

| # | 路径（相对 A_Lib） | 特征 |
|---|---|---|
| S1 | `1_Boom/Crowds Concert Audiences/00_Crowds_Concert_Audiences_Metadata.xls` | BIFF xls，3.4 MB，4692 行，Soundminer 列齐全，是全量中 RSS 最高的样本 |
| S2 | `1_Pro Sound Effect/The Odyssey Collection/Designed/The-Odyssey-Collection-Designed-Metadata.xlsx` | 行数最多（5159），第 1 行是标题，表头不在首行 |
| S3 | `1_Boom/Cannons/Construction Kit/00_Cannons_CK_Metadata.xlsx` | 12 个 sheet，ragged，解析最慢之一 |
| S4 | `3_VO Lib/人声_YDH配音源文件/《女神竞技场》配音格式需求_4.2.xlsx` | 体积最大（12 MB，主要是嵌入图片），但只有 905 行，8 个 sheet，中文，没有 Filename 列，只能按不带扩展名的 stem 匹配 |
| S5 | `1_Sound Ideas/Premier/Premiere_Edition_6_The_2448/.../PE6 METADATA.txt` | Soundminer 导出，UTF-16LE BOM + Tab，扩展名是 `.txt` |
| S6 | 同目录 `PE Vol. 6 Catalog.xls` | Filename 列不带扩展名；一份清单覆盖多张盘（`1of2`/`2of2` 目录） |
| S7 | `1_SoundBits/Whooshes Impacts 2/Whooshes And Impacts 2.csv` | 扩展名是 `.csv`，实际是 UTF-16LE + Tab；只有 RecID，没有 Filename 列 |
| S8 | `2_Cartoon Lib/Vintage_Anime_SFX/Vintage Anime SFX/VASE.csv` | `;` 分隔，表头首列为空 |
| S9 | `4_Vehicle/Car Collection/Volvo_XC90/_Documents/Metadata Translations/Volvo XC 90 Sound File List - 中国語 (Japanese).csv` | UTF-8 CJK，同一库有 6 种语言的翻译表（会重复导入） |
| S10 | `4_Others/Sonniss.com 2016 FX/Audio.Bundle.Part2/The Complete Sonniss Catalog - 2016.csv.xls`；`1_SoundMorph/Mechanism/_SoundMorph_Mechanism_Soundminer.xls`；`1_Boom/Aquatic Predators/~$00_...xlsx` | 伪装 xls（实为 CSV）、AppleDouble、Office 锁文件 |
| S11 | `1_The Recordist/TR_Guns/TR_Guns_Rifle/The_AK_HD_Pro_Pt1/.../The_Recordist_The_AK_HD_Pro_List_XLS.xls` | 没有 Filename 列，只有 Description（`GUN: Assault Rifle: ... - DPA4062`），而实际文件名是去掉标点的近似值 |
| S12 | `1_Pro Sound Effect/PSE_Gen Collection/Doors/Doors-Metadata.xlsx` | Filename 后缀 `_PSE_DRS_096` 和磁盘上的 `_PSE_GEN3_jif2O` 不一致（重新打包时改了名） |

### A.3 解析基准

环境：Dragonpark-Home，Rust release 版，依赖 `csv 1.4` + `encoding_rs 0.8.44` + `calamine 0.38`。先把文件复制到本地 SSD 再解析，这样不计 SMB I/O。实现是一次性读入，并把全部单元格物化为 `Vec<Vec<String>>`。parse 取 5 次的中位数；峰值 RSS 取进程 PeakWorkingSet 的最大值，包含约 5.4 MB 的进程基线。

| 样本 | 大小 | 行（含表头） | parse ms | 进程 wall ms | 峰值 RSS MB | 行/秒 | 行命中率 | 文件覆盖率 |
|---|---|---|---|---|---|---|---|---|
| S1 Crowds .xls | 3478 KB | 4692 | 21.2 | 29.9 | 29.7 | 221k | 100% | 100% |
| S2 Odyssey .xlsx | 310 KB | 5159 | 11.4 | 18.9 | 10.3 | 454k | 99.9% | 95.6% |
| S3 Cannons CK .xlsx | 1773 KB | 3370 | 29.9 | 37.8 | 9.6 | 113k | 27.1% | 99.8% |
| S4 女神竞技场 .xlsx | 12114 KB | 905 | 3.2 | 10.1 | 5.9 | 287k | 59.2%（仅 stem） | 0%（精确） |
| S5 PE6 .txt UTF-16 | 1008 KB | 1100 | 2.1 | 9.4 | 9.2 | 535k | 31.8% | 100% |
| S6 PE6 Catalog .xls | 282 KB | 1100 | 1.9 | 8.8 | 7.0 | 589k | 31.8%（仅 stem） | 0%（精确） |
| S7 Whooshes .csv UTF-16 | 81 KB | 267 | 0.3 | 7.1 | 5.4 | 810k | 0% | 0% |
| S8 VASE.csv `;` | 61 KB | 289 | 0.3 | 7.1 | 5.5 | 928k | 100% | 100% |
| S9 Volvo JP .csv | 657 KB | 856 | 1.4 | 8.3 | 6.5 | 594k | 100% | 92.6% |
| S10 Sonniss/AppleDouble | 109/4 KB | 0 | 0.1 | 6.7 | 5.4 | – | 解析失败 | – |
| S11 Recordist AK | 60 KB | 227 | 0.4 | 7.1 | 5.6 | 632k | 0% | 0% |
| S12 PSE Doors | 16 KB | 126 | 0.7 | 7.5 | 5.8 | 182k | 0% | 0% |
| 合成 200k 行 CSV（10 列） | 39 MB | 200001 | 117.6 | 171.8 | 161.6 | 1.70M | – | – |
| 合成 200k 行 XLSX（10 列） | 7.6 MB | 200001 | 788.5 | 845.4 | 248.9 | 254k | – | – |

全量 449 个文件：共 230,345 行，parse 累计 797 ms，进程 wall 累计 4.2 s，峰值 RSS 最大 29.7 MB、中位数 5.8 MB。遍历厂商目录（SMB）收集文件名，单个目录最慢 1.16 s。匹配规则：任意单元格的 basename（不区分大小写）等于磁盘上的文件名记为精确命中，等于 stem 记为 stem 命中；匹配范围是表格所在的厂商目录，会跳过 Info/Data/Docs/Catalog/Metadata/Translations 这类子目录。在 440 个有音频的目录中，精确文件覆盖率 ≥90% 的有 362 个，为 0 的有 33 个；只能按 stem 命中的文件有 21 个。

### A.4 对设计（#209）的影响

1. **编码**：没有遇到 GBK 或 cp1252，但**必须支持 UTF-16LE BOM**（Soundminer 导出，且可能用 `.csv` 扩展名）。检测顺序建议为 BOM → UTF-8 校验 → GBK 兜底。**分隔符必须嗅探**（`,` `;` `\t`），不能由扩展名决定。
2. **按内容识别格式**：`.xls` 可能是文本 CSV 或 AppleDouble 资源叉。需要按魔数分派（OLE `D0CF11E0` / ZIP `PK` / 文本）；应跳过 `._*`、`~$*`、`@syno*`，并且容错不中断。
3. **表头定位**：大量文件第 1 行是标题。需要在前 N（≈10）行里扫描含 `Filename`/`File Name`/`FileName` 的行作为表头，并逐 sheet 处理（多 sheet 占 13%）。
4. **匹配键**：
   - 精确 basename（不区分大小写）在约 82% 的目录可行。
   - 必须加一级 **stem（去扩展名）匹配**（Sound Ideas Catalog、VO 表格）。
   - 一份表会覆盖多个分卷或 Part 目录（Recordist、Sound Ideas、Boom 多 sheet），所以**匹配范围应是整个厂商根目录或整库的 basename 索引**，不能只看同级目录：行命中率低，但文件覆盖率接近 100%。
   - 约 7% 的表格没有 Filename 列（只有 Description 或 RecID），或者文件名在重新打包后被改过（PSE），这些无法可靠匹配。建议在导入预览里标为"未匹配"，不做模糊匹配，或者把模糊匹配作为可选项。
   - 多语言翻译表会对同一文件重复导入，需要定义合并或去重规则（以 path + 列名为键，或按语言区分）。
5. **内存**：真实表格最大只有 5159 行，峰值 RSS 不超过 30 MB，不构成风险。合成的 200k 行 XLSX 全量物化后达到 249 MB、0.79 s，CSV 为 162 MB；这些数字按 `Vec<Vec<String>>` 物化计，流式写库会更低，但流式方案未实测。calamine 对 xlsx 必须整 sheet 读入 Range，所以建议给导入设置行数和文件大小上限（例如 xlsx 50 MB），或者分 sheet 处理并分批写入。体积大不等于行数多：12 MB 的 xlsx 主要是图片。

基准工具在 `C:\Users\Dragonpark\smv2-bench`，不在仓库内：`rbench/` 是 Rust 程序，`results.jsonl` 是全量结果，`samples_bench.json` 是样本结果。
