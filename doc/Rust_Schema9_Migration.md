# Schema 9 服务端目录迁移

从完整 schema 8 库升级到 `ale-s9-cld1-t3-m2-p1-a1-sd1`。本阶段只交付
`search_path: 1`，不宣称已实现厂商 Metadata 或 Vorbis 功能。

```powershell
cargo build --locked -p audiolib-db --bin audiolib-migrate
audiolib-migrate status --db ./library.db
audiolib-migrate start --db ./library.db
audiolib-migrate pause --db ./library.db
audiolib-migrate resume --db ./library.db
audiolib-migrate cancel --db ./library.db
audiolib-migrate rollback --db ./library.db
```

`startup` 与 `start` 相同，但尊重 `AUDIOLIB_DEFER_SCHEMA9=1`。服务端以管理员
显式启动迁移；普通桌面数据库打开仍以 schema 8 为写入目标。迁移后的服务端可重新打开
schema 9。命令不会创建缺失的库，也不会接管无关 SQLite 文件。

命令最后一行与 `/admin/api/migration/status`、
`POST /admin/api/migration/{start,pause,resume,cancel,rollback}` 返回相同的
[JSON 报告](schemas/migration-report.schema.json)。HTTP start/resume 在后台执行，
客户端轮询 status；其余控制操作等待结果。管理接口沿用 LAN、`X-AudioLib-Admin: 1`、同源校验，额外检查
`manage_mode`。第二个执行器返回 `already_running`，CLI 退出码 2，HTTP 409。

`auth_mode=off` 仅关闭 token 要求，不关闭管理接口的网络来源校验。原生监听器把
回环和同机连接归为 External，以阻止同机反向代理转发的公网请求访问管理接口；即使
`manage_mode=all` 且携带正确管理头，也返回 403 `ADMIN_LAN_ONLY`。因此交接文档
所述本机临时服务可复现此拒绝，但不能用来证明管理操作成功。生产边界不因测试而豁免；
路由测试的无连接上下文是独立的 LAN fixture，不能替代原生 HTTP 验收。

读接口遵循 [HTTP 协议](schemas/migration-http.schema.json)：

- 状态非 idle/done 时发出 `x-audiolib-migration`；旧长名称只作客户端读取别名。
- 搜索 scope 为 `main|all|path`，默认 main；GET `/api/v1/search?q=...&scope=...`
  与既有 POST 搜索共用 DataProvider。
- 路径索引未发布时 path 返回 503 `migration_in_progress`、`Retry-After: 30`；
  all 返回主表结果并带 `x-audiolib-path-search: unavailable`。
- switching 时写接口返回 503 `migration_switching`、`Retry-After: 5`；读接口继续服务。

迁移预检只读权限和额外可用空间（至少库大小的 1.3 倍）。SQLite 在线备份写入同目录
唯一的 `audiolib-rollback-mig9-*.db`，验证完整性、外键及库身份后保留，绝不自动删除。
不执行 VACUUM。

每批默认处理 5,000 个目录；批次变慢后缩小下一批。目录行与
`v9_checkpoint` 同事务提交。仅 `audio_dir_search` 的本段目录名及其查询词进行
NFC、全角 ASCII/空格转换；根目录不进入词项。`audio_search`、原始路径、Metadata
及注释不被迁移改写。回填期间捕获目录增删改；切换事务重放目录变更并生成
`audio_dir_runs`，原子发布 `search_path_rev=1`、`source_search_rev=3`、格式 ID 和
`user_version=9`。故障恢复不会提前发布这些标记。

迁移使用持久的 `<db>.migration9.lock` OS 锁，文件写入 owner PID，进程退出自动释放。
不能复用已有 `.upgrade.lock`：普通搜索读者已经用它协调旧式离线重建。锁文件不删除，
避免锁 inode 被替换的竞态。`v9_control` 接收跨进程 pause/cancel，执行器在批次边界检查。
控制请求遇到准备阶段或忙锁可重试。SQLITE_FULL 保留断点并报告 paused/disk_full。
持久报告绑定库的 album namespace；异常退出后无 owner 的运行状态按 paused 读取。

取消删除未发布的路径派生对象，保留备份及 schema 8 业务数据。回滚须验证保留备份，
持有扫描和独占管理 lease，然后事务撤销路径对象及格式标记。本阶段只修改派生对象，
因此采用逆向事务回滚，保留备份之后的注释和扫描数据，避免覆盖新写入。

故障注入只在 `--features test-hooks` 构建中存在：
`AUDIOLIB_MIGRATION_FAULT`、`FAKE_FREE_BYTES`、`PAUSE_AFTER_BATCHES`、`BATCH_ROWS`、
`SLOW_BATCH`、`HOLD_SWITCH_MS`（后五项均带 `AUDIOLIB_MIGRATION_` 前缀），以及
`--sql-trace`。默认构建不解释这些变量或选项。测试的 abort 以无析构进程退出模拟崩溃，
不生成平台 crash dump。

## 验证范围与既有 harness 差异

目录契约回归位于 `crates/audiolib-db/tests/migration9.rs`；权限、状态头、scope 回退及
切换拒写回归位于 `server/audiolib-server/tests/migration9.rs`。

PR #211 的原始断言和 schema 文件保持不变，结果须逐项保留：

- MG01/MG10/MG17 读取逐文件 `audio_path_search`，与 t3 的目录表不一致。
- MG05 要求 `locked`，但 driver 及共享报告/HTTP schema 规定第二次 start 为
  `already_running`；迁移 OS 锁不能通过删除文件释放。
- MG11 的 50 行批次和 MG13 的默认第 2 批，在原 fixture 的 15 个目录上不可达。
  新增目录回归使用足量目录验证慢批缩减，故障回归以一目录一批验证第 2 批的
  disk_full 暂停与恢复，不把这些补充结果替代成原 MG 通过。
- MG18 的 Vorbis/厂商功能要求仍见于 `TEST-SEARCH-METADATA-V2.md` 的 MG-18/VN-08。
  JSON schema 没有强制这两个功能，而本阶段只发布目录搜索；这是完整 v2 的验收缺口，
  不能仅称断言过时，更不能通过虚报功能解决。
- HTTP 驱动缺少 `X-AudioLib-Admin: 1`，部分业务 URI 是可配置占位值，注释写入
  固定使用 POST 而实际 Metadata 接口使用 PATCH。还要求每条 HTTP 用例独占全新
  schema 8 库，因此单次运行全部用例会受前序迁移状态影响；须另做逐例隔离复跑。

验收必须区分契约差异、环境问题及实现错误，不能增加旧路径表或解除权限检查来满足
断言。Windows bench 基线问题应独立记录，不修改 `search-b-bench.rs` 来绕过验收。
大库耗时、跨平台文件系统及真实设备性能需要各自实测；
合成 fixture 的结果不代表 **P95 latency <500ms for 1M+ records** 的性能验收。
