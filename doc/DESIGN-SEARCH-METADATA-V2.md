# 搜索与 Metadata v2 设计（schema 8 → 9）

> 状态：设计稿，只含文档，不含业务代码。基于 `beta` 的 `4ff16a5`。
> 2026-11-03 更新（二）：与服务端设计稿逐条核对，13 处不一致已按服务端稿修正（厂商文本并入 source 列、迁移只建路径表、端点与 capabilities 形状、s8 派生步骤等）。
> 2026-11-03 更新：合入服务端基准结论（路径采用方案 B，909,342 行服务端库副本实测）和 PR #208 附录 A（`b16c742`）的厂商表格调研要求。
> 相关 issue：[#204](https://github.com/Freedragonlancer/AudioLib-Explorer/issues/204)、[#205](https://github.com/Freedragonlancer/AudioLib-Explorer/issues/205)、[#206](https://github.com/Freedragonlancer/AudioLib-Explorer/issues/206)、[#207](https://github.com/Freedragonlancer/AudioLib-Explorer/issues/207)。
> 测试计划：[PR #208](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/208)（`doc/TEST-SEARCH-METADATA-V2.md`，分支 `test/search-metadata-v2-plan`）。§11 逐条回答了它第 10 节「待设计确认的问题」。
> 标「待服务端确认」「待 Debug 补」「待 Paul 决定」的地方，在评审时补齐。

## 0. 现状（代码核对）

| 项 | 位置 | 现状 |
| --- | --- | --- |
| schema 版本 | `crates/audiolib-db/src/schema.rs:17` | `SCHEMA_VERSION = 8`；`MIN_SNAPSHOT_SCHEMA_VERSION = 3`（:21）；`SEARCH_TEXT_VERSION = 3`（:24） |
| 迁移入口 | `schema.rs:911` `migrate` / `:915` `migrate_with_progress` | 按 `user_version` 分派；高于 `SCHEMA_VERSION` 直接报不支持（:923） |
| 搜索重建 | `crates/audiolib-db/src/schema/current.rs:243` `rebuild_search`（`schema.rs:997` 是旧版） | DROP 触发器和 `audio_search` 后重建，**整库一个事务**；CLI `db rebuild-search`，目前没有 admin HTTP 端点 |
| 本地 FTS 表 | `schema.rs:1307` `FTS_SCHEMA` | external content（`content='audio_files'`），12 列：filename、user_tags、bwf_description、ixml_scene、ixml_take、user_notes 各有原文列和 `_tokenized` 列；`unicode61 remove_diacritics 2`；**没有路径列** |
| 修订号 | `schema.rs:391/401/402`、`:121` | `source_search_rev = 2`，`source_tags_rev = 2`，存于 `library_meta` |
| 内嵌文本拼接 | `crates/audiolib-core/src/metadata.rs:94-127` | `source_metadata_search_text` 汇总 riff_info/ixml/axml/xmp/id3/smed 及 `SOURCE_TAG_KEYS`，上限 256 KiB，写进 `bwf_description_tokenized` |
| snapshot FTS | `crates/audiolib-db/src/search_storage.rs:33` `CREATE_FTS` | contentless（`content=''`，`contentless_delete=1`），列同上；投影由 `projection()` 用 `audiolib_tokenize` / `audiolib_source_text` 生成 |
| 搜索 SQL | `crates/audiolib-db/src/lib.rs:2819` `search_sql`；`:2085`、`:2115` 用 `bm25(audio_search)` 排序，`LIMIT ? OFFSET ?` | 无列权重；中文走 jieba-rs + `resources/audio_terms.txt` |
| 查询编译 | `crates/audiolib-db/src/search_text.rs:203` `compile_query`（`:9-10` 原文/派生列） | 字母词 =（原文列短语 OR 派生列字母二元组短语）；**汉字/数字词无列过滤** |
| snapshot FTS 选项 | `search_storage.rs:28-34` | contentless：`content='',contentless_delete=1,detail=full,columnsize=1`；**contentless 表不支持 FTS `'rebuild'`** |
| 扫描器 | `crates/audiolib-db/src/scanner.rs:633` | 只收扩展名 `wav`，**不收 flac/ogg** |
| 解码 | 根 `Cargo.toml:22` | `symphonia =0.6.1`，features 仅 `wav,pcm,adpcm` |
| WAV ID3 | `crates/audiolib-audio/src/wav.rs`、`wav_id3.rs:52/75` | 只读 WAV 内 `id3 ` chunk |
| CSV | `app/audiolib-desktop/src/dialogs.rs:255` | 仅标注导入/导出（`*.jsonl;*.csv`），无厂商表导入 |
| 磁盘空间 | `crates/audiolib-db/src/space.rs:62` `ensure_free_space` | 余量 `max(64 MiB, 2%)`；未知空间放行 |
| 服务器模式 | `app/audiolib-desktop/src/remote_mode.rs:5`、`:108-170` | 只读打开校验过的 snapshot，不碰本地库 |
| capabilities | `doc/api/README.md:361`、§5.9（:747） | `GET /capabilities` 返回 `manage_mode`、`can_manage`、`compatibility.features.*` 等；库启动中也可用 |
| 兼容标记 | `crates/audiolib-core/src/compatibility.rs`、`doc/api/Compatibility_Markers.md` | schema、feature map、snapshot 格式 ID 三类标记；`crates/audiolib-remote/src/compatibility.rs:21` `peer` 判定 |
| 迁移执行器 | `doc/api/Library_Migration_Compatibility.md:189`、`doc/Rust_Upgrade_Recovery.md:12-36` | 原库备份 `audiolib-rollback-*.db`、owned candidate、checkpoint、发布与回滚，`required_free_bytes` |
| 升级期间 HTTP | `doc/api/README.md:454` | `/health` 可用；业务端点 `503 UNAVAILABLE` + `Retry-After: 5` |
| 自动更新 | `doc/Rust_Auto_Update.md:35` | `update-channel.txt`（stable/beta）+ `update-version.txt` |
| 性能目标 | `AGENTS.md` | P95 < 500ms、P50 < 100ms（1M+ 条）；GUI 30ms 防抖，新请求取消旧请求（`doc/Rust_GUI_Search_Performance.md:23/50`） |

服务端现网：v0.8.7，schema 8，约 909,342 条，snapshot 解压约 10.6GB。

## 1. 目标和非目标

目标：
1. 路径可搜：输入文件夹名或路径片段能命中（#204）。
2. 支持 FLAC/OGG(Vorbis/Opus) 文件和 Vorbis comment 标签（#205）。
3. 支持厂商 CSV/XLS/XLSX metadata 导入，可预览、按批回滚、幂等（#206）。
4. schema 8 → 9 可续跑、可回滚的迁移，以及客户端/服务端版本协商与提示（#207）。
5. 搜索延迟不变长（§10 硬要求）。

非目标：
- 不改写源音频文件（不回写标签，不改文件名）。
- 不做 MP3/AAC/M4A；不做 Opus 解码播放之外的转码。
- 不做在线厂商目录抓取；不做模糊/语义搜索。
- 不改 snapshot 协议以外的 HTTP 接口形状（只增字段）。
- 服务器模式客户端仍是只读，不在客户端写服务端库。

## 2. 路径可搜

### 2.1 索引什么
- 索引**相对路径的目录部分**：从所属扫描根（root）下一级开始到文件所在目录，例如 `Weapons/Guns/AK47`。文件名已在 `filename` 列，不重复。
- 根目录名本身默认**不**索引（多数是 `SFX`、`Library` 这类无区分度的词，且服务端根名可能带私有信息）。是否加一个开关见 §11。
- 来源：`folders` 表 + `folder_closure`，不读文件系统。

### 2.2 拆词
1. 分隔符：`/`、`\`、`_`、`-`、`.`、空格、`()[]{}`、`&`、`+`、`,` 全部替换为空格。
2. 驼峰和字母数字边界再拆一次：`GunShotAK47` → `Gun Shot AK 47`；同时保留原词 `GunShotAK47`，避免整词查询失效。
3. 拆分后复用现有 `audiolib_tokenize`（字母词生成二元组，中文 jieba-rs + `resources/audio_terms.txt`）：**路径列与文件名列用同一套分词**，片段/前缀查询语义一致，不另建 prefix 或 trigram 索引。
4. 每条记录路径文本上限 4 KiB；超出截断尾部目录并累计截断计数（迁移/扫描报告里可见）。

### 2.3 归一
- 统一 Unicode NFC（macOS 来源路径常为 NFD，`é` 与 `é` 必须同一个词）。需新增 `unicode-normalization` crate（纯 Rust，无系统依赖，许可 MIT/Apache-2.0）。
- 大小写：FTS5 `unicode61` 已大小写折叠；`remove_diacritics 2` 保持现状。
- 全角 ASCII（`ＡＢＣ`）转半角（NFKC 只用于这一步，不对中文整体做 NFKC）。
- 查询端 `compile_query`（`search_text.rs:203`）做同样的归一，保证两边一致。

### 2.4 文件夹改名
- 改名/移动在库里表现为 `folders` 变化 + 一批 `audio_files.folder_id` 或路径更新。
- 路径文本不新增 `audio_files` 列，直接写独立 FTS 表 `audio_path_search`（rowid = `audio_files.id`，§5.1）。触发器在 `folder_id`、`path_key` 变化时对该行 delete + insert。
- 目录名本身改名（`folders.name` 变）时，后台任务按 `folder_closure` 找后代文件，每批 5,000 行重写 `audio_path_search`，不在 UI 线程做。批内一个事务；中断后按 checkpoint 续跑。
- 改名期间旧路径仍可搜到，属可接受的短暂不一致。

### 2.5 「只搜文件名」
- 提供。搜索框旁加一个过滤项「范围：全部 / 仅文件名 / 仅路径」，默认「全部」。
- 「仅文件名」= 主表 `audio_search` 列过滤 `{filename filename_tokenized} : ...`；「仅路径」= 只查 `audio_path_search`；「全部」= 主表与路径表并行查询后合并（§10.4）。
- 状态写入 settings，服务器模式同样可用（列过滤在本地 snapshot 上执行）。

### 2.6 排序权重
路径不进主表，不参与主表 bm25 权重（§10.3）。路径表单独排序，合并时路径命中排在同分文件名命中之后、只做补充，不能把「文件夹名碰巧相同」的大量文件压到真正文件名命中之前。

## 3. Vorbis / FLAC / OGG 标签

### 3.1 现状与范围
- 扫描器只收 `wav`（`scanner.rs:633`）；symphonia 只开了 `wav,pcm,adpcm`（`Cargo.toml:22`）。所以本项包含：扫描收录、时长/采样率等基础信息、标签抽取、播放解码四件事。
- 支持：`.flac`（FLAC + Vorbis comment）、`.ogg`/`.oga`（Ogg Vorbis）。`.opus` 列为待 Paul 决定（symphonia 0.6 无 Opus 解码，需额外 crate）。
- 默认开启还是放进设置开关：见 §11。

### 3.2 字段映射（Vorbis comment 键不区分大小写）

| Vorbis 键 | 映射到 | 进 FTS |
| --- | --- | --- |
| `TITLE` | `fields.title` | 是（source 列） |
| `DESCRIPTION`、`COMMENT` | `fields.description` | 是 |
| `GENRE` | `fields.category` | 是 |
| `ARTIST`、`PERFORMER` | `fields.designer` | 是 |
| `ALBUM` | `fields.library`（厂商库名） | 是 |
| `KEYWORDS`、`TAGS` | `fields.keywords`（数组，按 `;`、`,` 拆） | 是 |
| `COPYRIGHT`、`LICENSE`、`ORGANIZATION` | `fields.publisher` 等 | 否（只显示） |
| `DATE`、`TRACKNUMBER` | 原样保存 | 否 |
| UCS 自定义键 `CATID`、`CATEGORY`、`SUBCATEGORY`、`FXNAME` | `fields.ucs_*` | 是 |
| 其它键 | `wav_metadata.vorbis.<key>` 原样保存 | 否 |

- 存储沿用 `metadata_json.wav_metadata` 结构，新增 group `vorbis`。`source_metadata_search_text`（`metadata.rs:94`）的 group 列表加 `"vorbis"`，这会让 `source_search_rev` 2 → 3（§5.6）。
- 同键多值保留为数组。

### 3.3 封面
- 读取 FLAC `PICTURE` 块和 Vorbis `METADATA_BLOCK_PICTURE`（base64），只接受 PNG/JPEG，大小上限沿用现有内嵌封面上限（WAV ID3 APIC 的处理，`doc/Artwork_Data_Safety_20261027.md`）。
- 不存图像本体进库，只记位置/哈希，与现有 `/audio/{id}/artwork` 同一条通路按需读取。

### 3.4 损坏、超长、编码不对
- 单条 comment 上限 64 KiB，整块上限 1 MiB，超出截断并记 `metadata_json.vorbis_diagnostics.truncated=true`。
- 规范要求 UTF-8；非法 UTF-8 用替换字符解码，记 `invalid_utf8=true`，**不猜 GBK**（Vorbis 不应出现，猜错代价大）。
- 块长度越界、CRC 错：放弃标签，仍按音频收录；解码也失败则按现有「不可读文件」处理，记扫描错误，不中断扫描。
- 搜索文本仍受 256 KiB 总上限（`metadata.rs:127` 附近）。

### 3.5 依赖评估

| 方案 | 说明 | 结论 |
| --- | --- | --- |
| symphonia 开 `flac`、`ogg`、`vorbis` feature | 已在用的 crate，同版本 `=0.6.1`；解码 + 元数据读取都有 | **推荐**，不新增 crate |
| `lofty` | 标签读写全，体积和依赖树较大，带写入能力 | 不用（我们不写标签） |
| `metaflac` / `lewton` | 单一功能 | 备选 |

标签解析建议自己写一个只读的小解析器（Vorbis comment 格式简单，约 200 行），便于实施上限和诊断；解码交给 symphonia。新增 feature 需跑许可证预检（`tools/` 打包流程）。

## 4. 厂商 CSV / XLS(X) 导入

### 4.1 列名映射
- 列名归一：去 BOM、去首尾空白、NFC、小写、去掉空格/下划线/连字符后匹配别名表。
- 内置别名表（可在预览里手动改映射，并保存为「模板」）：

| 目标字段 | 常见列名 |
| --- | --- |
| 文件名 `filename` | Filename, File Name, FileName, Name, 文件名 |
| 相对路径 `rel_path` | Path, File Path, Relative Path, Folder, 路径 |
| 描述 `description` | Description, Desc, BWF Description, Comment, 描述 |
| 关键词 `keywords` | Keywords, Tags, 关键词, 标签 |
| 分类 `category` | Category, Main Category, 分类 |
| 子分类 `subcategory` | SubCategory, Sub Category, 子分类 |
| 库名 `library` | Library, Collection, Album, Product |
| 厂商 `publisher` | Manufacturer, Publisher, Vendor, Brand |
| 设计师 `designer` | Designer, Creator, Artist, Recordist |
| 校验和 `checksum` | MD5, SHA1, SHA256, Checksum |
| UCS | CatID, Category, SubCategory, CategoryFull, FXName, CreatorID, SourceID, UserCategory, VendorCategory |

- 覆盖的模板：UCS 官方表、Soundminer 导出、BaseHead 导出、Pro Sound Effects / Boom Library / Sound Ideas 常见的产品清单（具体列名样本待 Debug 收集真实表格后补测试夹具）。
- 无法识别的列保存到 `vendor_metadata.extra_json`，不丢。

### 4.2 编码、分隔符、格式识别（依据 PR #208 附录 A，`b16c742`）
- 编码：有 BOM 按 BOM（UTF-8、UTF-16LE、UTF-16BE；样本里有 UTF-16LE 表）；无 BOM 先试严格 UTF-8，失败用 GBK（GB18030）兜底；预览显示检测结果并允许手选。依赖 `encoding_rs`（MIT/Apache）。
- 分隔符：嗅探逗号、Tab、分号（对前 64 KiB 统计引号外出现次数，选每行计数稳定且 ≥1 的）。CSV 解析用 `csv` crate。
- **按文件头识别格式，不看扩展名**：`.csv.xls` 可能是文本 CSV，`.xls` 可能是 AppleDouble 资源文件。魔数 `D0CF11E0` = xls，`PK\x03\x04` = xlsx，否则按文本；Soundminer txt 走 Tab。
- 跳过 `._*`（AppleDouble）、`~$*`（Office 锁文件）、`@syno*`（Synology 元数据目录）、`.DS_Store`。
- 单文件失败（损坏、加密、格式不认）只记录到 `files[].error`，不中断整个批次。
- XLSX/XLS 用 `calamine`：**每个 sheet 在前 10 行内找表头**；多 sheet 逐个处理（样本中 57 个表有多 sheet），不只读第一个。

### 4.3 匹配文件
匹配顺序（附录 A / 服务端稿 §6）：
1. 有相对路径列时，路径精确匹配优先（NFC + 大小写不敏感 + 分隔符统一为 `/`）。
2. 厂商根目录（导入目录所在 root 子树）文件名索引精确匹配。
3. 全库文件名索引精确匹配。
4. 去扩展名匹配（`Foo` ↔ `Foo.wav` / `Foo.flac`）。
5. 可选模糊匹配（规范化空白/大小写/下划线，默认关）：只在预览标「建议」，需用户确认。

- 未匹配的行在预览标「未匹配」（样本中约 7% 的表有未匹配行），不写库，可导出。
- 校验和匹配：库内无统一内容哈希，初版不启用（§11）。
- 多语言表按 (文件, 字段) 合并，各语言文本都进搜索文本。
- 合并规则**已定（Grok 暂定，Paul 可推翻）**：
  - 一行对多文件 → 标「歧义」，不自动写，在预览里由用户确认；
  - 一文件对多行 → 非空优先，keywords 求并集去重；
  - 详情显示语言中文优先，没有中文再用英文。

### 4.4 优先级
- 用户标注（user_tags、user_notes、收藏）永远不被覆盖：厂商数据写进独立表，不写用户列。
- 显示/搜索时同名字段的优先级：**用户标注 > 厂商导入 > 内嵌 metadata**。理由：厂商表通常比文件内嵌的更全、更新；用户手改的最权威。是否改成「内嵌优先」见 §11。
- 三者都进 FTS，优先级只影响详情页显示哪一个值，不影响能否搜到。

### 4.5 预览、批次、回滚、幂等
- 流程：`preview`（只读，不写库：每文件结果、列映射建议、匹配统计、未匹配样例）→ `mapping`（改列映射/模板、是否 fuzzy、去重规则）→ `commit`（`Idempotency-Key` 头，同 key 重放返回同一结果）→ 可 `rollback`。
- 每次导入一个 `import_id`（即批次 id，`batch_id`）。每条 `vendor_metadata` 记 `import_id`、`source_file`、`sheet`、`row`。
- 回滚 = 删除该 `import_id` 写入的行，并沿 `superseded_by` 链恢复被它覆盖的上一个值。
- 写入后重算该文件的 source 搜索文本（`bwf_description_tokenized`），由触发器更新 FTS；`source_search_rev` 不变（rev 3 已包含 vendor 组）。

### 4.6 大表性能与上限
- 上限（**本设计采用，服务端稿的 64 MiB / 50 万行不采用**）：单文件 50 MB 或 20 万行，单单元格 64 KiB，单行文本 256 KiB（与 `metadata.rs` 搜索文本上限一致）；超限文件整文件拒绝并在预览显示。`/capabilities.vendor_import` 的 `max_file_bytes` / `max_rows` 默认值与此一致（52428800 / 200000）；测试以接口返回值为准，做边界 ±1。
- 上限对 xls 同样生效：calamine 只有 xlsx 可用 `worksheet_cells_reader` 流式读，xls 整 sheet 入内存。
- 写入每 5,000 行一批一个事务，批间释放写锁并检查取消；进度格式同 §5.3。
- 实测（附录 A 样本）：449 个表（xls 209 / csv 127 / xlsx 103）+ 10 个 Soundminer txt，共 23 万行，全解析 0.8s，峰值内存 ≤30MB；合成 20 万行 xlsx 0.79s / 249MB。

### 4.7 服务器模式在哪导入
- 在**服务端**导入。客户端是只读 snapshot（`remote_mode.rs:5`），不能写。
- 端点（需 manage 权限和 `X-AudioLib-Library-Id`，缺库 id 返回 409 `LIBRARY_ID_REQUIRED`）：
  - `POST /api/vendor-import/preview`（multipart 文件或服务端 `root_relative_dir`）→ `import_id`
  - `GET /api/vendor-import/{id}`（预览/进度）
  - `PUT /api/vendor-import/{id}/mapping`
  - `POST /api/vendor-import/{id}/commit`（`Idempotency-Key`）
  - `POST /api/vendor-import/{id}/rollback`
  - `GET /api/vendor-import`（历史批次）
- 客户端只在 `can_manage` 为真时显示入口。导入完成或回滚后 `snapshot_id` 变化，客户端提示刷新（§7.6）。

### 4.8 公式与注入
- 读：XLSX 公式单元格只取缓存值，不计算；没有缓存值则当空。CSV 中以 `=`、`+`、`-`、`@`、`\t`、`\r` 开头的值按文本保存（不执行任何东西，我们本来也不执行）。
- 写（导出未匹配行、导出标注 CSV）：上述前缀的值前加 `'`，防止在 Excel 中打开时被当公式执行。现有标注 CSV 导出（`dialogs.rs:255` 相关路径）一并补上。
- 单元格上限 64 KiB，单行文本上限 256 KiB（§4.6）。

## 5. 数据库 schema 8 → 9

### 5.1 新增表和列
```sql
CREATE TABLE vendor_metadata(
  id INTEGER PRIMARY KEY,
  audio_file_id INTEGER NOT NULL REFERENCES audio_files(id) ON DELETE CASCADE,
  import_id INTEGER NOT NULL,            -- 即批次 id
  field TEXT NOT NULL, value TEXT NOT NULL,
  source_file TEXT, sheet TEXT, row INTEGER,
  superseded_by INTEGER, extra_json TEXT);
CREATE INDEX idx_vendor_metadata_file ON vendor_metadata(audio_file_id) WHERE superseded_by IS NULL;
CREATE INDEX idx_vendor_metadata_import ON vendor_metadata(import_id);
CREATE TABLE v9_checkpoint(k INTEGER PRIMARY KEY CHECK(k = 1), last_id INTEGER, done INTEGER, phase TEXT);
CREATE VIRTUAL TABLE audio_path_search USING fts5(
  path_tokens, path_tokens_tokenized,
  content='', contentless_delete=1, detail=full, columnsize=1,
  tokenize='unicode61 remove_diacritics 2');
```
- 批次元数据（文件 SHA-256、映射、行数、状态）的表形状以服务端实现为准。
- **主表列结构不变**（是否按新分词规则重建见 §5.2 选项）：厂商文本并入现有 source 文本 `bwf_description_tokenized`（与 Vorbis 同路），不加 `vendor_tokenized` 列，不换表。代价：厂商字段在 bm25 排序里**没有单独权重**（与内嵌描述同权）。
- 路径表两列：`path_tokens`（目录段原文，NFC）、`path_tokens_tokenized`（`audiolib_tokenize` 结果），rowid = `audio_files.id`。
- `v9_checkpoint.phase` 的取值与 `schema_migration.state` 枚举一致（`idle|running|paused|failed|done`），细分阶段在 `/admin/api/migration.phase`。

### 5.2 路径表回填：分批 + 续跑
- contentless 表**不支持 FTS `'rebuild'`**，只能 DROP 后重插，或建新表再 RENAME。因为主表不动，v9 不需要重建/换主表，只需建并回填路径表：
  1. 加表（单事务，快）：`vendor_metadata`、`v9_checkpoint`、`audio_path_search`，并先装路径触发器（新行/改名行直接写路径表）。`user_version` 暂不变。
  2. 回填：按 `id` 升序分批，事务内同时写 checkpoint；delete + insert 保证幂等；可取消，重启后从 `last_id` 继续。业务读写照常。
  3. `optimize` + integrity-check。
  4. 切换（单事务，毫秒级）：写 `source_search_rev=3`、features、`user_version=9`。
- **分词版本升到 t4 后的两个选项**（待 Paul 定，§11）：
  - (a) 归一只用于路径表：迁移约 70s；但主表里全角/NFD 的文件名等仍按旧规则分词，与查询端归一不一致。
  - (b) **推荐**：查询进主表前也归一，主表 `audio_search` 按新规则重建：分批可续跑（建新表分批写入再 RENAME，或 DROP 后重插），参考实测 2 万行/批 46 批约 227s + optimize 20s，主表约 250s，另加路径表约 70s（均不含备份），入库与查询行为一致。
  - 选 (b) 时迁移不再是「只建路径表 70s」；步骤 1–4 中主表重建与路径表回填共用同一 checkpoint 和自适应批大小。
- 批大小：**自适应，每批 ≤2s**（B 方案 2 万行约 1.4s）。2 万行固定批在同表最坏情况下 p95 12.1s，会长时间持写锁。
- 运行中 `SQLITE_FULL`：暂停并报错，保留 checkpoint。
- `VACUUM`（实测 327s）**不放进迁移**，放维护窗口可选。
- 多实例共用库：迁移持有 `*.upgrade.lock`（独占 upgrade lease），扫描在迁移完成前不启动。

### 5.3 备份、回滚、磁盘空间、进度
- 预检：空余空间 **≥ 库大小 × 1.3**（回滚备份 1× + WAL/新索引约 0.3×），不足拒绝，`/admin/api/migration` 报 `INSUFFICIENT_SPACE`。
- 备份：沿用 `schema/search_upgrade.rs:89` 的 `audiolib-rollback-*.db` + integrity/FK 校验，**永不自动删除**。
- 回滚：切换前任何失败 → DROP 新表和 checkpoint 即回到纯 s8，无需还原；切换后 → 停服 + 用 rollback 库还原 + 换回旧二进制（需 Paul 批准）。
- 进度端点（manage 权限）：
  - `GET /admin/api/migration` → `{state, phase, completed, total, batch_ms_p50, eta_s, rollback_backup, error}`
  - `POST /admin/api/migration/start|pause|resume|cancel`
  - `POST /admin/api/search/rebuild`（等同 CLI `db rebuild-search`，后台运行，同进度格式）
  - `/health.upgrade` 镜像 `phase/completed/total` 给客户端。
- 上线：设 `AUDIOLIB_DEFER_SCHEMA9=1` 推迟迁移，由管理员手动触发；迁移完成后服务端预生成 s9 zstd 快照，避免首个客户端等待生成（现约 44s）。

### 5.4 耗时（服务端库副本 909,342 行实测）

| 阶段 | 实测 |
| --- | --- |
| 现状整库重建（对照） | 499s，回滚备份峰值多占 12.4GB |
| 路径表插入 + optimize（方案 B） | 64.6s + 2.4s，约 70s |
| 回滚备份 + 校验 | 约 2–4 min |
| 选项 (a) 只建路径表 | 约 70s（不含备份） |
| 选项 (b) 主表按 t4 重建 + 路径表（推荐） | 主表约 250s（227s + optimize 20s）+ 路径表约 70s，合计约 320s（不含备份） |
| 合计（含备份） | (a) < 5 min；(b) 约 6–8 min |
| VACUUM（不进迁移） | 327s |

（46 批 227s、换表 7ms 来自同表分批实测，用作选项 (b) 主表重建的参考。）

测量工具：`cargo run --release --locked -p audiolib-db --bin upgrade-benchmark -- --source <snapshot.db>`（`doc/api/README.md:454`），在服务端副本上跑。

### 5.5 存量回填
| 数据 | 方式 |
| --- | --- |
| 路径 `audio_path_search` | 直接从库算（`folders` + `folder_closure`），不读文件 |
| 厂商文本 | 无存量，导入时写 |
| Vorbis 标签 | 存量库里没有 flac/ogg 记录，**需要重扫**；迁移不扫，迁移后提示 |
| WAV 内嵌字段 | 不变，不重扫 |

### 5.6 修订号与 snapshot 格式
- `TARGET_SCHEMA` 8 → 9（`compatibility.rs:11`）；`CATALOG_REVISION` 1 → 2（`compatibility.rs:9`）。
- `source_search_rev`：2 → 3（Vorbis/厂商组进入 `audiolib_source_text`）。
- `source_tags_rev`：保持 2，除非 `SOURCE_TAG_KEYS` 增加 UCS 键。
- `SEARCH_TEXT_VERSION`（tokenization）：3 → 4，格式 ID 中为 `t4`。理由：NFC/全半角归一改变了入库与查询两端的分词，旧客户端必须能明确识别不兼容（服务端已确认保持 t3 不安全）。
- snapshot 格式：`ale-s9-cld1-t4-m2-p1-a1-sp1`，`-z1` 压缩变体沿用 v8 规则；feature `search_path`、`vorbis_tags`、`vendor_metadata`。服务端写格式改为 s9。
- **s8 兼容快照**（过渡期）：由 s9 库派生——DROP `audio_path_search` 和 `vendor_metadata`、`source_search_rev` 回写 2、`user_version=8`、滤掉非 wav 行；按需生成、只读、同一 library_id。成本：多一份约 10.6GB plain + 约 959MB zstd，生成约 1–2 分钟。派生快照的 `bwf_description_tokenized` 已含 Vorbis/厂商文本，旧客户端可搜，只是不解释新字段。
  - s8 派生快照必须是 **t3 分词**（格式 `ale-s8-cld1-t3-m2-p1-a1`）：从 s9（t4）派生时需对主表按旧规则重建，或保留迁移前的旧主表供派生——**待服务端确认**。
  - 发放：按 `x-audiolib-accept-snapshot-formats` 优先发 s9，否则 s8；只拿 s8 的客户端收到响应头 `x-audiolib-upgrade-available`。
  - 写操作只接受 s9 客户端（按库实际格式，否则 409）。
  - 保留期：**建议一个版本周期**（待 Paul 定，§11；服务端稿建议 2 个发布周期）。
- v9 客户端可读 schema 3–9 的 snapshot；读 ≤8 时不提供路径搜索。

## 6. 版本协商与降级

| 情况 | 行为 |
| --- | --- |
| 旧客户端（只认 s8）连新服务器（s9） | 过渡期内服务端发 s8 兼容快照（§5.6），缺路径/厂商功能，响应头 `x-audiolib-upgrade-available` 触发非阻塞升级提示；无 s8 兼容快照时 409 `SNAPSHOT_FORMAT_UNSUPPORTED`，提示升级客户端 |
| 新客户端连旧服务器（s8） | 正常同步 s8；`/capabilities` 无 `search_path` 等 feature → 隐藏「仅路径」和厂商导入入口；不提示 |
| 服务器正在迁移 | 分批回填阶段 s8 快照**照常可读**；仅在切换瞬间业务端点 503 + `Retry-After: 5`；`/health.upgrade` 显示进度，客户端状态栏提示 |
| 本地库是旧 schema（≤8） | 打开时弹迁移确认框（§7.1）；拒绝 = 不打开该库，可切换到其它库 |
| 本地库 schema 比程序新（≥10） | 现有逻辑：报不支持（`schema.rs:923`），提示更新程序 |
| 本地库迁移中断 | 下次打开自动续跑，显示进度，不再次确认、不再次备份 |

`/capabilities` 新增字段（只增不改，老客户端忽略）：
```json
{
  "compatibility": {
    "features": { "search_path": 1, "vorbis_tags": 1, "vendor_metadata": 1 },
    "snapshot_read_formats": ["ale-s8-cld1-t3-m2-p1-a1", "ale-s9-cld1-t4-m2-p1-a1-sp1"],
    "database_write_formats": ["ale-s9-cld1-t4-m2-p1-a1-sp1"]
  },
  "library_schema": { "current": 9, "source_search_rev": 3, "snapshot_formats_served": ["ale-s9-cld1-t4-m2-p1-a1-sp1", "ale-s8-cld1-t3-m2-p1-a1"] },
  "schema_migration": { "state": "idle|running|paused|failed|done", "endpoint": "/admin/api/migration" },
  "vendor_import": { "revision": 1, "max_file_bytes": 52428800, "max_rows": 200000 }
}
```

## 7. 客户端提示文案草稿

### 7.1 本地库需要迁移
- 确认框标题：「音效库需要升级」
- 正文：「此音效库需升级到新格式（schema 9）才能使用路径搜索和厂商资料导入。升级前会自动备份到同目录。预计需要 {时长}，需要可用空间 {空间}。升级中可以取消，下次打开会从中断处继续。」
- 按钮：「立即升级」「暂不，打开其它音效库」
- 进度：「正在升级音效库：{阶段名} {已完成}/{总数}（{百分比}%）」，按钮「取消」
- 取消确认：「已完成的部分会保留，下次打开会继续。确定取消吗？」
- 续跑：「继续上次未完成的升级…」
- 空间不足：「磁盘空间不足：需要约 {需要}，当前可用 {可用}。请清理后重试。」
- 失败：「升级失败，音效库未改动。备份位于：{路径}。」按钮「复制诊断信息」

### 7.2 服务器版本或 schema 不匹配
- 「服务器版本较新（{服务器版本}），当前客户端无法读取它的音效库。请更新客户端。」按钮「检查更新」
- 「服务器版本较旧，部分新功能（路径搜索、厂商资料导入）不可用。」——只在用户点了这些功能时出现，不主动弹。

### 7.3 服务器正在迁移
- 状态栏：「服务器正在升级音效库（{百分比}%），暂时使用本地缓存。」

### 7.4 客户端有新版本
- 沿用 `Rust_Auto_Update.md` 现有提示，不新增文案。

### 7.5 不打扰用户的情况
- 新客户端连旧服务器、功能自动隐藏。
- 迁移续跑（只显示进度，不再确认）。
- 服务端迁移期间已有缓存可用时不弹框，只在状态栏提示。
- 厂商表中的未识别列、单条标签截断：只记到预览或日志。

### 7.6 刷新与升级提示触发点
- 收到响应头 `x-audiolib-upgrade-available`（只拿到 s8 兼容快照）：非阻塞提示「服务器已升级，更新客户端可使用路径搜索和厂商资料」，按钮「检查更新」。
- `snapshot_id` 变化，或厂商导入完成/回滚后：提示「服务器音效库已更新，点击刷新」。

## 8. 边缘情况清单（初稿，待 Debug 补）
1. 同一文件夹 NFC/NFD 两种写法并存（来自 macOS 拷贝）。
2. 路径极深（> 64 级）或单段名 > 255 字符。
3. 路径含 emoji、零宽字符、RTL 字符。
4. 文件夹改名与扫描同时发生。
5. 根目录本身改名或换盘符（relink）。
6. FLAC 带 ID3v2 头（非标准但常见）。
7. OGG 多逻辑流 / chained ogg。
8. Vorbis comment 声明长度远大于文件。
9. 厂商表同时有 `Filename` 和 `File Name` 两列。
10. 厂商表合并单元格、空行、表头不在第一行。
11. CSV 中引号不配对、行内换行。
12. 表里文件名不带扩展名而库里同主干有 `.wav` 和 `.flac` 两个文件。
13. 导入中途取消；导入中途关闭程序。
14. 回滚一个已被后续批次覆盖的批次。
15. 被导入的文件随后从库中移除再重扫回来（id 变了）。
16. 迁移中途断电；备份所在盘满。
17. snapshot 同步时服务端恰好迁移完成。
18. 查询只有路径分隔符或单字符。
19. 超长查询（已有 500 字符上限）。

## 9. 测试和验收
按 `AGENTS.md`「Development checks by change scope」：
- 本文档 PR：只做 `git diff --check`、链接检查。
- 代码子 PR：`cargo fmt --all -- --check` 一次；受影响包 `cargo clippy -p <pkg> --all-targets --locked -- -D warnings`；对应测试（如 `cargo test -p audiolib-db --test search_fragments --locked`）。
- schema/迁移/导入导出变更：数据库集成测试 + `tools/validate-library-management.ps1`。
- 扫描与 metadata 写入：`tools/validate-review-fixes.ps1`；扫描枚举变化：`tools/validate-access-denial.ps1`。
- 解码（FLAC/OGG）：解码器测试 + `tools/validate-runtime.ps1`（有真实夹具时）。
- 服务端：`tools/check-local.ps1 -Package audiolib-server -NoDefaultFeatures`。
- 性能：`search-benchmark --records 1000000 --queries 100` + §10.6 基准。
- 交付：`tools/validate-portable.ps1`；beta 流程 `beta-refresh.sh --with-db` + `beta-smoke.ps1`。

新增测试要点：路径拆词/NFC 单测；迁移分批中断续跑（每阶段注入失败）；回滚恢复；厂商导入幂等（同表导两次结果逐字节一致）；公式前缀导出转义；旧客户端读新 snapshot 得到明确错误。

## 10. 性能（硬要求）

**搜索延迟不能变长，可以多占 CPU。** 合并门槛：schema 9 的 p95 不能比 schema 8 慢。

### 10.1 路径搜索方案：采用 B（独立 FTS 表）

服务端在 909,342 行副本上实测（8790 实例的副本，不在生产库上跑）。**以下延迟数字均为热缓存 p50、B 为串行实现**；p95 和并行 B 尚未测，见 §10.6 合入闸门。

| | A：主表 `audio_search` 加 `path_tokens` 列 | B：独立 FTS 表 `audio_path_search`，主表不变 |
| --- | --- | --- |
| 不搜路径时的影响 | 高频片段拖慢：`hdd_A_Lib` 3.8 → 192ms；bm25 排序 205–828ms；普通查询慢 2–11% | 主表不变，无影响 |
| 中文 | 汉字无列过滤导致「爆炸」1 → 676 条 | 不影响主表结果 |
| 查询代价 | 一次 MATCH | 第二只读连接并行查询路径表，额外 2–6ms；并行后 ≈ 主查询 |
| 体积（以 FTS 2028.8MB 为基准） | +12.0% | +14.2%（289MB），≤15% |
| 结论 | 否决 | **推荐** |

热 p50（ms，现状 / A / B 串行）：

| 查询 | 现状 | A | B 串行 |
| --- | --- | --- | --- |
| explosion | 17.4 | 17.6 | 20.5 |
| whoosh | 20.3 | 21.8 | 26.1 |
| Sonomar | 90.5 | 101 | 95.1 |

- 前缀查询 `expl*` 6–7ms，不需要 prefix 索引。不用 trigram。
- 禁止 `LIKE '%...%'` 全表扫描。

### 10.2 字段合并
Vorbis 字段和厂商字段都并入主表 `bwf_description_tokenized`（source 文本），一次 MATCH，不 JOIN `vendor_metadata`；主表结构不变。路径只在 `audio_path_search`。

### 10.3 排序权重（bm25 列权重，初值，待基准调整）
主表列不变：filename, filename_tokenized, user_tags, user_tags_tokenized, bwf_description, bwf_description_tokenized, ixml_scene, ixml_scene_tokenized, ixml_take, ixml_take_tokenized, user_notes, user_notes_tokenized

`bm25(audio_search, 10, 10, 6, 6, 3, 3, 2, 2, 1, 1, 1, 1)`

- 厂商字段与内嵌描述同权（3），没有单独权重。
- 路径表不参与主表权重；路径命中按 1.5 权重折算后与主结果合并，只补充不压过文件名命中。
- 替换 `lib.rs:2085`、`:2115` 处的 `bm25(audio_search)`。

### 10.4 分页、防抖、取消
- 分页：保持现有 200 条一页（`Rust_GUI_Search_Performance.md:23`）。相关度排序继续 `ORDER BY score,rowid LIMIT ? OFFSET ?`；深翻页（OFFSET > 2,000）改为 keyset：`WHERE (score,rowid) > (?,?)`。
- 防抖保持 30ms；新请求取消旧请求，沿用每 2,000 VM 指令检查取消标志的进度回调（`Rust_GUI_Search_Performance.md:50`）。
- 「仅文件名/仅路径」切换视为新查询，同样取消旧查询。
- 路径命中上限沿用 `MAX_RANKED_IDS`，超出只作过滤、不参与排序。
- 多词查询：每个词取（主表 ∪ 路径表），词间求交。
- 方案 B「全部」范围：主表与路径表在两个只读连接上并行查询，结果按 rowid 去重、合并排序（文件名/主表命中优先）；keyset 翻页游标覆盖两表，保证不漏不重；取消时**两个连接都停**。

### 10.5 后台任务不卡搜索
导入、路径重算、FLAC/OGG 补抽取都在后台 worker：每批 ≤5,000 行、单批事务 < 200ms 目标，批间 `sleep(0)` 让出；WAL 模式下读不被写阻塞；搜索 worker 优先级不变。

### 10.6 基准
固定查询集（存 `crates/audiolib-db/benches/queries-v2.txt`，待建）：

| 类别 | 示例 |
| --- | --- |
| 中文 | 爆炸、脚步声 木地板、枪声 远处 |
| 英文 | explosion, footsteps wood, door slam |
| 短词 | ak, 门, ui |
| 路径片段 | Weapons Guns, Foley/Cloth, 环境 城市 |

每类各 25 条，热缓存和冷缓存（清 OS 文件缓存后）各跑。

已测（热 p50、B 串行）见 §10.1。

### 10.7 方案 B 合入闸门（服务端负责补测；Debug 列入 #208 P0）
1. 补测 **p95**：热缓存，以及清 OS 文件缓存后的冷缓存。
2. 按**并行实现**实测 B：p95 不高于现状（schema 8）。
3. 正确性：两表结果合并排序且去重；keyset 翻页不漏不重；取消时两个连接都停止。
4. FTS 体积增长 ≤15%（B 实测 +14.2%）。

数据在服务端库副本上实测（不在生产库上跑）。

## 11. 子 PR 拆分

| # | 内容 | 依赖 |
| --- | --- | --- |
| PR1 | 本设计文档 | — |
| PR2 | 归一与拆词工具函数（NFC、全角、驼峰拆分）+ 单测；基准查询集和 `search-benchmark` 扩展，先测 schema 8 基线 | PR1 |
| PR3 | schema 9 迁移框架：新表/列、分批续跑、备份/空间检查、路径表 audio_path_search 与分批回填；snapshot 格式和 feature 标记 | PR2 |
| PR4 | 搜索侧：bm25 列权重、「仅文件名/仅路径」、keyset 分页；跑 §10.6 门槛 | PR3 |
| PR5 | 服务端 capabilities 字段、迁移状态、旧客户端提示；客户端协商与 §7 文案 | PR3（可与 PR4 并行） |
| PR6 | FLAC/OGG 扫描 + Vorbis comment 解析 + 封面 + symphonia feature | PR3 |
| PR7 | 厂商导入：解析/编码/分隔符、匹配、预览、批次、回滚、导出转义（本地模式） | PR3 |
| PR8 | 服务端厂商导入接口 + 客户端上传入口 | PR5、PR7 |

### 待 Paul 决定
1. 路径是否采用方案 B（独立 FTS 表）——**推荐 B**，A 已被基准否决（§10.1）。
2. 根目录名是否加可选开关进索引——建议做开关，**默认关**（不进）。
3. s8 兼容快照保留期——**建议一个版本周期**（服务端稿建议 2 个发布周期）。
4. 迁移时主表是否按 t4 新分词规则重建（§5.2）：(a) 只归一路径表，约 70s，主表全角/NFD 不归一；(b) 主表也重建，主表约 250s（另加路径表约 70s），行为一致——**推荐 (b)**。
5. FLAC/OGG 默认收录，还是放设置开关。
6. 是否支持 `.opus`（需额外解码 crate）。
7. 厂商导入 vs 内嵌 metadata 的显示优先级（草案：用户 > 厂商 > 内嵌）。
8. 校验和匹配是否进初版。
9. 下表 Q1–Q6（PR #208 §10）的推荐答案是否采纳。


### 对照测试计划第 10 节的推荐答案（同时列入待 Paul 决定）

| # | 问题（PR #208 §10） | 推荐答案 | 理由 |
| --- | --- | --- | --- |
| Q1 | 路径 token 权重；是否加停用词 路径进独立表 `audio_path_search`，不参与主表权重（§10.3）。**加停用词**：静态表（`SFX`、`Sound Effects`、`Audio`、`WAV`、`Users`、`Library`、`Libraries`、`音效`、`素材` 等，存为资源文件，与 `audio_terms.txt` 并列）；根目录名不进索引（§2.1） | 通用段在 90 万条里几乎处处出现，bm25 区分度近 0 但会膨胀 posting list，拖慢查询、增大索引 |
| Q2 | 中文 bigram 还是词典分词 | **词典分词**：沿用 jieba-rs + `resources/audio_terms.txt`，与现有所有 `_tokenized` 列一致；不引入 bigram | 查询端已用 jieba（`search_text.rs:203`），两种分词混用会造成同词不同 token；bigram 索引体积明显更大，很难守住 15% 上限 |
| Q3 | Vorbis 白名单还是全量 | **白名单进索引，全量存 JSON**：§3.2 表中标「进 FTS」的键入索引；其它键原样存 `wav_metadata.vorbis`，只显示 | 防止 `ENCODER`、`REPLAYGAIN_*`、超长自定义键污染排序和膨胀索引；全量保存保证以后扩白名单只需重算、不需重扫 |
| Q4 | CSV 与用户标签冲突谁优先；CSV 存哪 | **用户标注优先**，永不覆盖用户列。CSV 存独立表 `vendor_metadata`（带 `import_id`，可回滚），当前有效值并入 source 搜索文本 `bwf_description_tokenized`，**搜索不 JOIN** | 写进 tags 会无法按批回滚；并入 source 列主表不动，代价是厂商字段无单独排序权重 |
| Q5 | 旧客户端遇 schema 9 snapshot：硬拒绝还是只读降级 | **硬拒绝并提示升级**（走现有 snapshot 格式 ID 判定，`Compatibility_Markers.md` §4），继续使用本机已有的旧 snapshot 离线浏览 | 已发布的旧客户端不认识新 FTS 列和 feature，无法安全读取；过渡期服务端另出 s8 兼容快照（多占约 10.6GB，建议保留一个版本周期），过渡期内旧客户端仍可同步 |
| Q6 | 索引增长上限 15% 是否可接受 | **接受**，作为合并门槛之一（与 p95 不变慢并列）。实测 A +12.0%、B +14.2%，均在内；不用 trigram | 目录名重复度高，拆词后新增 token 少；snapshot 现约 10.6GB，15% ≈ 1.6GB 是同步和磁盘可承受的上界 |

### 待服务端补
- §10.7 方案 B 闸门：热/冷 p95、并行 B 实测、合并/翻页/取消正确性。
- 自适应批大小（≤2s/批）实现后的迁移耗时复测。
- s8 兼容快照派生与发放实现。
- s8 派生快照的 t3 主表来源：派生时按旧规则重建，还是保留迁移前旧主表（§5.6）。

（厂商导入合并规则已定，见 §4.3，Grok 暂定、Paul 可推翻，不再列入待定。）

### 待 Debug 补
- #208 P0：方案 B 闸门测试（§10.7）。
- §8 边缘情况；厂商表真实样本夹具（附录 A 已提供调研数据）。
