# 服务端设计稿：搜索与 Metadata v2（schema 8 → 9）

> 状态：设计稿（仅文档，未改代码、未推送）。基线：`origin/feat/search-metadata-v2` = beta `4ff16a5`（v0.8.7 = `b5a98ff`）。
> 关联：#204 路径搜索、#205 Vorbis/FLAC/OGG、#206 厂商 CSV/XLS(X)、#207 跟踪；客户端设计 PR #209（`doc/DESIGN-SEARCH-METADATA-V2.md` @ f594cc1）；测试计划 PR #208（附录 A @ b16c742）。
> 实测环境：服务器测试机（macOS arm64），库副本 `<bench-dir>/bench.db`（来源 `<prod-backup>.db`，909,342 行，schema 8，10.64 GB）。
> 所有生产实例（<prod-port>）上的动作都要 Paul 批准，本稿不动生产。

## 0. 现状（代码位置）

| 项 | 位置 | 说明 |
|---|---|---|
| schema 常量 | `crates/audiolib-db/src/schema.rs:17-21` | `SCHEMA_VERSION=8`，`MIN_SNAPSHOT_SCHEMA_VERSION=3` |
| 迁移入口 | `schema.rs:911 migrate` / `:915 migrate_with_progress`；`:923` 高于已知版本拒绝 | 按 `user_version` 分派 |
| 可续跑升级 + 回滚备份 | `schema/search_upgrade.rs:73 migrate_resumable`；`:89-115` 先 `rollback_path` 再 `snapshot_to_file_cancellable` 做 `audiolib-rollback-*.db`，校验 integrity/FK；`:134` 写入 library_meta | 迁移前必备份 |
| 当前（v8）FTS 定义 | `crates/audiolib-db/src/search_storage.rs:28-34 CREATE_FTS` | **contentless**：`content='',contentless_delete=1,detail=full,columnsize=1`，12 列（6 原文 + 6 `_tokenized`），`unicode61 remove_diacritics 2` |
| 旧（≤v5）FTS 定义 | `schema.rs:1307 FTS_SCHEMA` | external content（`content='audio_files'`），只用于老库升级 |
| 投影/索引批 | `search_storage.rs:37-48 projection()`、`:110-135 index_batch()`（5000 行一批） | 写入时用 `audiolib_tokenize` 生成字母二元组 `gXXXXXXYYYYYY` |
| 搜索重建 | `schema/current.rs:243 rebuild_search`（整库**一个事务**，DROP + 重建 + 触发器）；`schema.rs:997` 旧版 | CLI：`app/audiolib-cli/src/main.rs:373 RebuildSearch` / `:873 rebuild_search()`；**没有** admin HTTP 端点 |
| 查询编译 | `crates/audiolib-db/src/search_text.rs:9-10 RAW/DERIVED_COLUMNS`、`:203 compile_query` | 字母词 =（原文列短语 OR 派生列二元组短语）；**汉字/数字词无列过滤** |
| 候选查询 | `search_candidates.rs:37` `SELECT rowid FROM audio_search WHERE audio_search MATCH ?` | 加列会直接影响这条 |
| 能力/兼容 | `server/audiolib-server/src/routes.rs:233-239,509-545 GET /capabilities`（含 `compatibility_manifest()`）；`crates/audiolib-core/src/compatibility.rs:9 CATALOG_REVISION`、`:11 TARGET_SCHEMA=8`、`:15 PLAN_B_FORMAT="ale-s8-cld1-t3-m2-p1-a1"`、`:17 …-z1`、`:40-48,514-516` 头名、`:442 features {name:rev}`、`:626 compatibility_manifest()`、`:149 snapshot_format_id()` | 格式 ID 只有完全匹配特征表才生成 |
| 准入 | `server/audiolib-server/src/compatibility.rs:31 admit()`：先 peer 标记，再 `x-audiolib-accept-snapshot-formats` 精确匹配、`snapshot-schema-max`/`search-storage-max`/`tokenization-max`；失败 409 `SNAPSHOT_FORMAT_UNSUPPORTED`（:13）；`:63 rejection()` 版本类 409 | |
| 库身份 | `routes.rs:342` 写操作 409 `LIBRARY_ID_REQUIRED`（需 `X-AudioLib-Library-Id`）；`:2027` artwork 同理 | |
| 健康/升级进度 | `routes.rs:840-858 /health`（`upgrade: service.upgrade_progress()`、`schema_version`、`compatibility_catalog_revision`）；`service.rs:91-100,205-210` | 升级期间业务 503 + Retry-After |
| snapshot | `routes.rs:216 /library/snapshot`、`:2250`；`state.rs:270-340` plain + 首次请求时生成 zstd 副本（level 见 `state.rs:53`）；`state.rs:322` 生成前 `ensure_free_space(plain/8)`；`state.rs:2002-2041 backup_library_cancellable`、`:2076-2087` 从备份检测格式 | 约 959 MB（zstd）、TTFB 约 44 s（首个 zstd 生成） |
| 更新检查 | `server/audiolib-server/src/update.rs:110 start / :153 check`；`admin.rs:60-62 /admin/api/update{,/check,/install}` | 渠道见 `doc/Rust_Auto_Update.md` |
| 扫描器 | `crates/audiolib-db/src/scanner.rs:633` 只收 `.wav` | #205 要扩 |

## 1. 能力和版本协商

### 1.1 新格式 ID 与特征
- `TARGET_SCHEMA 8→9`；新 `PLAN_B_V9_FORMAT = "ale-s9-cld1-t3-m2-p1-a1-sp1"`（`sp1`=search path rev1）。
- **t3 决定（已定）**：NFC + 全角→半角规范化**只作用于路径表 `audio_path_search` 及其查询词**；主表 `audio_search` 不规范化、不重建、一字节不变（`t3` 段含义不变，仍指主表分词版本）。实测副本上 5,985 行路径因此变化。是否同时出 `-z1` 压缩变体沿用 v8 规则。
- `source_search_revision 2 → 3`（Vorbis 组进入 `audiolib_source_text`），`snapshot_format_id()` 增加 `(ContentlessDelete, 9)` 分支。
- snapshot 新增 3 个 features（进入 `FEATURES` 与格式 ID）：
  - `search_path: 1` —— 路径 FTS 存在（推荐方案见 §3：独立表 `audio_path_search`）。
  - `vorbis_tags: 1` —— `metadata_json.wav_metadata.vorbis` 组及非 wav 文件可能出现。
  - `vendor_metadata: 1` —— `vendor_metadata` 表/列存在并参与搜索文本。
- `/capabilities` 新字段（只增不改，老客户端忽略）：
```json
{
  "compatibility": { "features": { "search_path": 1, "vorbis_tags": 1, "vendor_metadata": 1, "...": 0 },
                     "snapshot_read_formats": ["...", "ale-s8-cld1-t3-m2-p1-a1", "ale-s9-cld1-t3-m2-p1-a1-sp1"],
                     "database_write_formats": ["ale-s9-cld1-t3-m2-p1-a1-sp1"] },
  "library_schema": { "current": 9, "source_search_rev": 3, "snapshot_formats_served": ["ale-s9-…-sp1", "ale-s8-cld1-t3-m2-p1-a1"] },
  "schema_migration": { "state": "idle|running|paused|failed|done", "endpoint": "/admin/api/migration" },
  "vendor_import": { "revision": 1, "max_file_bytes": 52428800, "max_rows": 200000 }
}
```
- `CATALOG_REVISION 1→2`。服务端写格式改为 v9。

### 1.2 降级矩阵
| 客户端 \ 服务端 | 旧服务端（s8） | 新服务端（s9） |
|---|---|---|
| 旧客户端（只认 s8） | 现状 | 若服务端仍保留 s8 兼容快照（§4.2）→ 发 s8，功能缺路径/厂商字段；否则 409 `SNAPSHOT_FORMAT_UNSUPPORTED`，消息提示升级客户端 |
| 新客户端（认 s8+s9） | 客户端看 `/capabilities` 无 `search_path` → 隐藏“路径/厂商”范围开关，照常读 s8 | 正常 |
- 迁移进行中：`/health.upgrade` 带阶段与进度；业务 503 + `Retry-After: 5`（沿用）。迁移是后台批处理时（§2），**旧 s8 快照继续可读**，直到原子切换。

## 2. 迁移流程（8 → 9）

1. **预检**：`ensure_free_space` ≥ 库大小 ×1.3（回滚备份 1× + WAL/新索引 ~0.3×，实测见 §5）。不足则拒绝并在 `/admin/api/migration` 报 `INSUFFICIENT_SPACE`。
2. **备份**：沿用 `search_upgrade.rs:89` 的 `audiolib-rollback-*.db` + integrity/FK 校验；**永不自动删除**。
3. **加列/建表（快、单事务）**：新增 `vendor_metadata` 表、`v9_checkpoint(k=1,last_id,done,phase)`；`CREATE VIRTUAL TABLE audio_path_search(path_tokens, path_tokens_tokenized, content='',contentless_delete=1,detail=full,columnsize=1,…)`。`user_version` 暂不变。
4. **批量回填（可续跑）**：每批 20k 行一事务，事务内同时写 checkpoint；可取消、重启后从 `last_id` 继续。业务读写照常（触发器在步骤 3 已对新行/改名行写入路径表，回填用 `INSERT OR REPLACE` 语义的 delete+insert 保证幂等）。
5. **optimize + 校验**：`INSERT INTO audio_path_search(audio_path_search) VALUES('optimize')`、`integrity-check`。
6. **切换（单事务，毫秒级）**：写 `source_search_rev=3`、features、`user_version=9`；如果选了同表方案则在此 `ALTER TABLE … RENAME` 交换（实测 7 ms）。
7. **新快照**：生成 s9 plain/zstd；s8 兼容快照按 §4.2 保留。
8. **回滚**：切换前任何失败 → DROP 新表/checkpoint 即回到纯 s8（无需还原）；切换后 → 停服用 `audiolib-rollback-*.db` 还原（需 Paul 批准）。

进度端点（新增，需 manage 权限）：
- `GET /admin/api/migration` → `{state, phase, completed, total, batch_ms_p50, eta_s, started_at, rollback_backup, error}`
- `POST /admin/api/migration/start|pause|resume|cancel`；`POST /admin/api/search/rebuild`（等同 CLI `db rebuild-search`，后台 + 同进度格式）。
- `/health.upgrade` 镜像 `phase/completed/total` 给客户端提示。

## 3. 路径搜索方案与实测延迟

测试方法：`path-fts-bench`（只在 tmp worktree 的未提交文件，真实 `audiolib_tokenize`），9 个查询各：新连接冷跑 1 次（OS 页缓存未清，"cold" 只代表 SQLite 冷缓存），热跑 20 次取 p50/p95。路径 = `file_path` 的目录部分去掉 `<mount-parent>/`（保留根名 <root-name>，用来测最坏情况）。

### 3.1 索引大小（dbstat，VACUUM 后）
| 方案 | 大小 | 相对现有 FTS 2028.8 MB |
|---|---|---|
| A 同表加 `path_tokens`+`path_tokens_tokenized` | 2273 MB | **+12.0%**（<15% 上限） |
| B 独立表 `audio_path_search`（原文+二元组） | 288.9 MB | +14.2% |
| C 独立 trigram 表（仅原文） | 200.4 MB | +9.9%（但与现有二元组语义不一致，不推荐） |

### 3.2 方案 B 实现：双连接真并行 + 合并 + keyset 分页 + 取消（`search-b-bench`，t3 路径表）

**执行**：主连接 conn1 扫 `audio_search`，第二只读连接 conn2 同时扫 `audio_path_search`（`std::thread::scope`）。每边取全部命中的 `(bm25×权重, rowid)`（与 SQLite 做 `ORDER BY bm25 LIMIT` 时必须做的扫描相同）。
**合并**：按 audio id 去重，分数取两边较优（bm25 越小越好）；路径列权重 1.5、比例系数 `PSCALE`（默认 1.0）。
**分页**：keyset 游标 `(score, id)`，取 `(score,id) > cursor` 的前 N 条（`select_nth_unstable` 部分选择 + 排序），同分按 id 打破平局，与 OFFSET 无关。
**取消**：两条连接各持 `InterruptHandle`，取消时同时 `sqlite3_interrupt`；两边都返回 `SQLITE_INTERRUPT` 后整体报错，连接立即可复用。
**放弃的做法**：逐行“另一侧探查”去重（每页 200 次 `MATCH … AND rowid=?`）实测 explosion 3.9 s、Sonomar 48 s，不可用。

测试：14 个查询（中文、英文、短词、路径片段、CatID、厂商、高频根名 `<root-name>`）；热跑 3 次预热后 60 次；冷跑 = 每次新连接 10 次。基线 = schema 8 现行查询（只扫主表，bm25 TOP200）。

> **冷缓存限制**：`sudo purge` 需要密码，非交互不可用，因此“冷”只代表 **SQLite 页缓存冷**（OS 文件缓存仍热）。
> 冷跑里 B 比基线固定多 ~22–26 ms，在 0.2 ms 的 `爆炸` 上也一样 → 是基准程序为第二连接**新开连接 + 注册函数**的开销，不是查询本身。服务端 conn2 应常驻连接池，生产中不应出现这部分。

#### 3.2.1 热（ms，60 次）schema 8 基线 vs schema 9（B，t3）
| 查询 | s8 p50 | s8 p95 | s8 p99 | s9 p50 | s9 p95 | s9 p99 | p95 门槛（严格） | 说明 |
|---|---|---|---|---|---|---|---|---|
| 爆炸 | 0.04 | 0.05 | 0.06 | 0.21 | 0.22 | 0.23 | 不过 | 命中 1→200（路径新增 676），绝对 <0.3 ms |
| 脚步 | 0.04 | 0.04 | 0.05 | 0.61 | 0.64 | 0.65 | 不过 | 命中 2→200（路径 2313），绝对 <1 ms |
| explosion | 40.91 | 41.65 | 41.95 | 41.55 | 42.38 | 42.79 | 不过（+1.8%） | 噪声级 |
| whoosh | 54.32 | 54.95 | 55.16 | 56.37 | 61.16 | 61.59 | 不过（+11%） | 上一轮 +5%（55.1→58.0）；路径命中 45k 合并成本 |
| expl | 16.61 | 16.97 | 16.98 | 16.68 | 16.91 | 16.93 | 过 | |
| door slam | 38.16 | 39.09 | 39.22 | 38.33 | 39.17 | 40.70 | 不过（+0.2%） | 噪声级；上一轮通过 |
| Sonomar | 193.84 | 196.63 | 202.48 | 196.27 | 199.41 | 200.21 | 不过（+1.4%） | |
| <root-name> | 4.44 | 4.58 | 5.20 | 643.79 | 648.99 | 653.08 | **不过（×140）** | 根名命中全部 909k 行 |
| 1_Boom | 19.42 | 19.74 | 20.83 | 49.02 | 51.36 | 51.76 | **不过（×2.6）** | 顶层目录命中 80k 行 |
| Construction Kit | 242.16 | 245.16 | 246.76 | 246.03 | 249.82 | 252.69 | 不过（+1.9%） | |
| Crowds Concert | 112.09 | 113.43 | 114.36 | 113.20 | 115.35 | 116.09 | 不过（+1.7%） | |
| CREAHmn | 6.10 | 6.25 | 6.26 | 5.72 | 6.05 | 6.17 | 过 | |
| ROCKMvmt | 6.02 | 6.16 | 6.26 | 6.10 | 6.17 | 6.36 | 不过（+0.2%） | 噪声级 |
| EXPLMisc | 11.63 | 13.53 | 13.71 | 11.47 | 11.79 | 12.27 | 过 | |

#### 3.2.2 冷（ms，SQLite 冷，10 次，含新开连接）
| 查询 | s8 p50 | s8 p95 | s9 p50 | s9 p95 | 门槛 |
|---|---|---|---|---|---|
| 爆炸 | 18.92 | 33.57 | 45.05 | 45.36 | 不过 |
| 脚步 | 18.54 | 22.82 | 45.36 | 46.34 | 不过 |
| explosion | 67.42 | 86.41 | 91.15 | 92.11 | 不过 |
| whoosh | 80.50 | 101.93 | 105.39 | 106.67 | 不过 |
| expl | 39.52 | 43.16 | 62.88 | 64.77 | 不过 |
| door slam | 63.79 | 66.97 | 85.54 | 86.22 | 不过 |
| Sonomar | 220.07 | 222.56 | 245.01 | 256.71 | 不过 |
| <root-name> | 27.22 | 28.97 | 684.38 | 689.90 | 不过 |
| 1_Boom | 43.48 | 44.33 | 93.19 | 95.37 | 不过 |
| Construction Kit | 266.02 | 268.64 | 292.67 | 301.21 | 不过 |
| Crowds Concert | 137.64 | 139.34 | 160.29 | 162.95 | 不过 |
| CREAHmn | 28.25 | 30.65 | 51.66 | 52.47 | 不过 |
| ROCKMvmt | 28.33 | 29.96 | 52.90 | 53.54 | 不过 |
| EXPLMisc | 34.51 | 36.45 | 58.31 | 59.00 | 不过 |

#### 3.2.3 正确性与取消
- keyset 翻 5 页（每页 200）vs 全量不分页合并结果：14 个查询全部 **dup=0、顺序不一致=0、无缺失**（命中 <1000 的查询翻到底，如 爆炸 676、EXPLMisc 109）。
- 取消（两连接同时 `interrupt`，各 20 次）：<root-name>（未取消 646 ms）、Sonomar（205 ms）、Construction Kit（250 ms）、explosion（46 ms）在 5/30/100 ms 后取消，**全部 20/20 中断，取消到两线程都返回 p50 0.02–0.05 ms、最大 0.09 ms**；取消后连接可直接复用。explosion 100 ms 时查询已先完成（无可取消）。

### 3.3 结论（Debug #208 门槛：s9 p95 不得高于 s8）
- 严格逐字比较：热 4/14 通过；但 explosion/door slam/ROCKMvmt/Sonomar/Construction Kit/Crowds Concert 超出 0.2–1.9%，在两轮之间会翻转（上一轮 explosion、door slam 通过），属噪声。**建议门槛写成 “p95 ≤ s8 p95 × 1.05 或 +1 ms”**，按此 14 个里 10 个通过。
- **真失败及原因**：
  1. `<root-name>`（×140）、`1_Boom`（×2.6）：高频路径片段命中几十万行，bm25 需对全部命中打分。修复：根名不入索引（§3.4①，基准里故意保留做最坏情况）；路径词 df 超阈值（如 >5% 库）时**不参与排序**，只作过滤（rowid 序 LIMIT）或改用 `folder_closure` 过滤。
  2. `爆炸`/`脚步`：s8 只命中 1–2 行，s9 因路径新增几百上千行 → 结果更多，延迟仍 <1 ms，属预期的功能差异，建议门槛对 <5 ms 的查询用绝对值。
  3. `whoosh` +5~11%：合并 45k 路径命中的 HashMap/选择成本。可优化：路径侧改为 SQL `ORDER BY bm25 LIMIT k` 先取 top-k，主侧对这些 id 做存在性合并；或命中数超上限时只取 top-k。
  4. 冷跑全部不过：主因是基准每次为 conn2 新开连接（+~23 ms 固定）。服务端用常驻 conn2 后需复测；OS 冷缓存需要能 `sudo purge` 的环境复测。

### 3.4 附加约束
- ①默认**不索引根名**（与 PR #209 §2.1 一致）；②路径命中设上限（沿用 `MAX_RANKED_IDS`），超过只作过滤；③只存目录段，不重复文件名；④t3：路径段与查询词先 NFC + 全角转半角，再走 `audiolib_tokenize`；主表查询词**不**做此规范化；⑤trigram 不采用。
- 多词跨表（如一词在文件名一词在路径）：本次 B 实现是“整串在主表 OR 整串在路径表”，跨表拆词交集未实现（`door slam` 不会匹配 door 在路径、slam 在文件名）。

## 4. snapshot 影响

- 大小：B 方案 plain +~290 MB（10.64→~10.9 GB，+2.7%），zstd 预计 +~25–30 MB（二元组压缩率与现有相近，按 959 MB×2.7% 估）；TTFB（首次 zstd 生成 44 s）预计 +~1–2 s。厂商表与 Vorbis 字段按实际导入量另计。
- schema 升级后旧 plain/zstd 快照立即作废（格式 ID 变），首个请求重新生成；建议迁移完成后由服务端**主动预生成** s9 zstd，避免首个客户端吃 44 s+。
- 生成前 `ensure_free_space(plain/8)`（`state.rs:322`）仍适用。
- **实测 s9→s8 派生（生产库副本）**：APFS clone 0.02 s → DROP `audio_path_search`/`vendor_metadata` + 删 meta + `user_version=8` 2.9 s → `VACUUM INTO` 109 s → zstd -3 12.5 s（页缓存热；bench 副本上冷 151 s）；**合计 125 s**。s8 快照 10.78 GB、zstd 945 MB，`quick_check=ok`，主 FTS 哈希与原 s8 相同。

### 4.1 双快照建议（过渡期）
- 推荐过渡期（默认 1 个版本周期，以 Paul 最终决定为准）**同时保留一份 s8 兼容快照**：由 s9 库派生（DROP `audio_path_search`/`vendor_metadata`，`source_search_rev` 回写 2，`user_version=8`），按需生成、只读、同一 library_id。成本：多一份 ~10.6 GB plain + ~959 MB zstd 磁盘，生成约 1–2 min。
- 发放规则：按 `x-audiolib-accept-snapshot-formats` 优先 s9，否则 s8；只接受 s8 的客户端在 `/capabilities` 与响应头 `x-audiolib-upgrade-available` 收到升级提示。写操作只接受 s9 客户端（库实际格式 s9）。
- 注意 s8 派生快照里的 `bwf_description_tokenized` 已含 Vorbis/厂商文本（rev3 数据）— 旧客户端可读、可搜，只是不会解释新字段，属可接受。

## 5. 迁移成本实测（副本）

| 项 | 数值 |
|---|---|
| 现有 FTS 全量重建（CLI `db rebuild-search`，含回滚备份 + integrity 校验，单事务） | **499 s**（user 162 s / sys 58 s，RSS 346 MB）；目录峰值 22.8 GB（库 10.4 GB → +12.4 GB = 回滚备份 10.6 GB + WAL ~1.6 GB） |
| 重建后库大小 | 10.64 GB（不变） |
| 同表 14 列新 FTS（一次性批插 5000/批） | 插入 243 s + optimize 20 s；+2049 MB |
| **20k 行/批可续跑回填（A 同表 14 列，最坏情况）** | 46 批，总 227 s；单批 min 0.84 / p50 4.18 / p95 12.08 / max 13.34 s；optimize 20.4 s；切换事务（RENAME×2）7 ms |
| 独立路径表 B（推荐） | 插入 64.6 s + optimize 2.4 s；+164 MB（未 VACUUM） |
| **B + t3（只路径表，bench 副本）** | 46 批 × 20k；插入 69.9 s（单批 p50 1.21 / p95 3.08 / max 3.46 s）+ optimize 2.7 s；共 82.7 s |
| **完整 8→9 原型（生产库 `.backup` 副本，`migrate9`）正常跑** | 回滚备份 `VACUUM INTO` + quick_check 229 s；回填 64.5 s（p50 1.14 / p95 2.78 s）；optimize 2.3 s；切换 2 ms；**总 296 s** |
| 暂停/续跑 | 20 批后暂停（29.2 s），续跑从 last_id=400000 继续 26 批 35.1 s，总 37.5 s |
| kill -9 + 续跑 | done=300000 时 kill -9（含备份共 243 s），checkpoint=300000，续跑 31 批 46.2 s |
| 预检拒绝（模拟可用空间 1 GB < 1.3×库 13.8 GB） | 32 ms 拒绝，退出码 3 `INSUFFICIENT_SPACE`；库保持 v8、无新表、无回滚文件 |
| MG-10 主表不变 | 原 s8 副本、正常/暂停/kill 三个 s9 结果、派生 s8 快照的 `audio_search_{data,idx,docsize,config}`+DDL 哈希全部 = `721d95f2320d3d2a` |
| 三种路径的结果一致 | 路径表行数 909,342 = audio_files；6 个查询命中计数三者完全一致；FTS `integrity-check` ok |
| trigram 路径表 | 15.2 s + 1.6 s；+167 MB |
| VACUUM（13.1 GB 库） | 327 s |
- 推荐方案 B 的迁移（不含备份）实测 ~67 s，含备份约 5 min（备份占 ~3.8 min，是大头）；加回滚备份（复制 10.6 GB，副本复制实测 28 s，SQLite 在线备份 + 校验约 2–4 min）总计 <5 min。p95 单批 12 s 意味着写锁最长阻塞约 12 s → 批大小建议自适应（目标单批 ≤2 s，B 方案下 20k 行约 1.4 s）。
- 不建议迁移内 VACUUM（327 s，且需再 1× 空间）；放到维护窗口可选。

## 6. 厂商 CSV/XLS(X) 导入 API（服务端）

输入事实（PR #208 附录 A）：449 张表（xls 209 / csv 127 / xlsx 103 + Soundminer txt 10），23 万行解析 0.8 s、峰值 <30 MB；20 万行 xlsx 0.79 s / 249 MB。

端点（manage 权限 + `X-AudioLib-Library-Id`，否则 409 `LIBRARY_ID_REQUIRED`）：
- `POST /api/vendor-import/preview`（multipart 文件或服务端路径 `root_relative_dir`）→ 返回 `import_id`、每文件结果、列映射建议、匹配统计、未匹配样例。**不写库**。
- `GET /api/vendor-import/{id}`（预览/进度）、`PUT /api/vendor-import/{id}/mapping`（改列映射/模板、选择 fuzzy、选择去重规则）。
- `POST /api/vendor-import/{id}/commit`（`Idempotency-Key` 头；同 key 重放返回同结果）→ 后台批写，每批 5k 行一事务，进度同 §2 格式。
- `POST /api/vendor-import/{id}/rollback` → 按 `import_id` 删除该批写入的 vendor_metadata 行并恢复被覆盖的旧值（写入时记录 before-image 到 `vendor_import_log`）。
- `GET /api/vendor-import` 列历史批次。

解析规则：
- 格式按 magic 判定（`D0CF11E0` = xls，`PK\x03\x04` = xlsx，否则文本），不信扩展名。
- 编码：BOM（UTF-16LE/BE、UTF-8）→ 严格 UTF-8 → GBK（GB18030）；记录检测结果。
- 分隔符嗅探 `,` / tab / `;`（前 64 KiB 统计一致列数）；Soundminer txt 走 tab。
- 跳过 `._*`、`~$*`、`@syno*`、`.DS_Store`；单文件失败不影响整体，记 `files[].error`。
- 表头在前 ~10 行内找（别名表命中最多的一行）；多 sheet 逐个处理。
- 上限：单文件 50 MB、单文件 20 万行（xls 同样生效）、单单元格 64 KiB、单行文本 256 KiB（与 `metadata.rs` 搜索文本上限一致）；超限文件整文件拒绝并在预览中显示。
- 匹配：①厂商根目录（导入目录所在 root 子树）文件名索引；②全库文件名索引；③去扩展名匹配（`Foo` ↔ `Foo.wav/.flac`）；④可选 fuzzy（规范化空白/大小写/下划线，仅在预览中标“建议”，需用户确认）。相对路径列存在时优先路径精确匹配。多命中标“歧义”，不自动写。
- 去重/合并：同一文件来自多张表（多语言重复表）时，按 (文件, 字段) 合并：非空优先 → 预设语言优先（zh > en > 其他，可配置）→ 后导入覆盖需显式选择；keywords 求并集去重。
- 写入：`vendor_metadata(audio_file_id, import_id, field, value, source_file, sheet, row)`，并重算该行 `bwf_description_tokenized` 对应的派生搜索文本（走触发器更新 FTS）；`source_search_rev` 不变（rev3 已包含 vendor 组）。

## 7. Vorbis/FLAC/OGG（服务端）
- 扫描器 `scanner.rs:633` 扩展名白名单加 `flac/ogg/oga`（受特性开关 `scan_extensions` 控制，默认待 Paul 定）；symphonia 开 `flac,ogg,vorbis`。
- 新扫描到的非 wav 文件，旧客户端拿到 s8 兼容快照时也会看到（只是不能播放/解码时报错）→ s8 派生快照建议**过滤掉非 wav 行**，避免旧客户端出错。
- 已有 wav 库无需重扫；FLAC/OGG 需要一次增量扫描（管理员触发）。

## 8. 服务端需要暴露的客户端提示触发点
1. `/capabilities.compatibility.features` 含/不含 `search_path|vorbis_tags|vendor_metadata` → 客户端显示/隐藏功能。
2. `/capabilities.schema_migration.state` + `/health.upgrade` → “服务器正在升级库（x%）”。
3. 409 `SNAPSHOT_FORMAT_UNSUPPORTED` / 版本类 409 → “请升级客户端”。
4. 只拿到 s8 兼容快照时响应头 `x-audiolib-upgrade-available: <version>` → 非阻塞提示。
5. 新快照可用（`snapshot_id` 变化）→ 客户端提示刷新。
6. 厂商导入完成 / 回滚（`/api/vendor-import` 状态）→ 刷新快照提示。

## 9. 服务端视角的边界情况
- 迁移中崩溃/断电：checkpoint 续跑；切换前失败即回到 s8，无需还原。
- 迁移中扫描/写入：触发器先装，回填幂等（delete+insert）。改名风暴：路径表按 `folder_closure` 批量重算。
- 磁盘不足：预检拒绝；运行中 `SQLITE_FULL` → 暂停并报错，保留 checkpoint。
- 超长路径（>4 KiB）截断并计数；NFD 路径（macOS）统一 NFC 后再分词。
- 根名/共享前缀导致超高频路径词：根名不入索引 + 命中上限。
- 同一库多个服务端实例（<prod-port>/<beta-port>）共用 DB：迁移需独占 upgrade lease（已有 `*.upgrade.lock`）。
- 回滚备份磁盘占用：保留，管理端只提示不自动删除。
- 旧客户端写请求打到 s9 库：按库实际格式 409 拒绝。

## 10. <prod-port> 上线计划（每一步都需 Paul 批准）
1. beta 渠道 + <beta-port> 之外的新测试实例用副本跑完整迁移与回归（不碰 <beta-port>、PR #97）。
2. <prod-port> 停机窗口前：确认 <data-volume> 余量 ≥ 库×1.3（当前 4.4 TB 空闲，充足）；手工额外备份（不删任何旧备份）。
3. 升级二进制（launchd `<launchd-label>`）→ 启动时自动进入后台迁移（或设 `AUDIOLIB_DEFER_SCHEMA9=1` 由管理员按钮触发，推荐后者）。
4. 观察 `/admin/api/migration`：预计 <5 min；完成后预生成 s9 + s8 兼容 zstd。
5. 验收：固定查询集 p50/p95 对比基线不升高；`integrity-check`；老客户端可读 s8；新客户端出现路径范围开关。
6. 回滚预案：切换前取消即可；切换后停服 → 用 `audiolib-rollback-*.db` 还原 → 回旧二进制。

## 附：基准原始数据
`<bench-dir>/run.log`、`run2.log`、`du.log`、`du2.log`；基准程序 `<bench-worktree>/crates/audiolib-db/src/bin/path-fts-bench.rs`、`search-b-bench.rs`（未提交）；新数据 `run3.log`（B 首轮）、`run4.log`（t3 迁移 + 热/冷/分页/取消 + s8 派生）、`mig/run5.log` + `mig/mg10.log` + `mig/vq.log`（生产副本完整迁移）。
