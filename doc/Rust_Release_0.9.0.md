# 0.9.0 发布准备与两端迁移方案

状态：**2026-11-04，隔离候选准备中，未发布**。`0.9.0` 是拟定 MINOR 版本；
Cargo 当前仍为 `0.8.7`，本文件不代表版本冻结、beta 提升、安装或生产迁移已经完成。
本轮授权为检查后台任务并准备下一大型版本；生产服务、工作库、源 WAV 和现有安装保持原状。
feat → beta、beta → main、标签、发布以及实际服务部署各须取得相应授权。

## 1. 候选范围与证据身份

拟纳入目录段路径搜索、迁移管理及客户端提示/刷新交互，同时保留 beta 已验收的 Default v2
和播放器反馈。厂商导入与 Vorbis 功能须分别验收，不能从 schema feature 标记推断功能已交付。
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
| 后续兼容性修正 | **pending**：schema 9 的真实快照读入、写格式协商、快照验证及相关集成检查由候选后续修正确定；完成后追加最终提交与结果 |

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

## 2. 存储目标与当前实现边界

程序版本、API、常规本地写目标和实际服务端库格式分别记录，不用一个 schema 常量替代所有含义：

| 对象 | 首个候选现状 | 发布前要求 |
| --- | --- | --- |
| 程序 / HTTP | Cargo `0.8.7`；API `/api/v1/` | 范围冻结后统一 Cargo/锁文件/说明至拟定 `0.9.0`；标签尚不创建 |
| 普通本地可写库 | `SCHEMA_VERSION=8`、`TARGET_SCHEMA=8`；本地升级 UI 当前构建路径派生索引，不能称为完整 schema 9 主库迁移 | 明确保留 s8 的本地策略与重开/降级保护；需要 s9 本地写入时另行实现和验收，不能只改常量 |
| 显式服务端迁移 | `audiolib-migrate` 或管理 API 将完整 s8 发布为 s9；服务端专用 `open_server_existing` 可重新打开 s9 可写库 | 验证切换、进程重启、快照、所有受支持写入和回滚 |
| s9 完整格式 | `ale-s9-cld1-t3-m2-p1-a1-sd1`；主 FTS5 tokenizer 为 t3，目录修订为 sd1 | 检查真实 schema/features/结构/身份；`source_search_rev=3` 与三项 feature 原子发布；保持原主 FTS5 不变 |
| 客户端 s9 快照 | 目录格式 ID 已被声明；实际完整读链及协商修正 **pending** | 下载前协商、解压后识别、完整性验证和只读打开均通过；声明支持不能替代实际读入 |
| `/capabilities` | 初始 `CATALOG_REVISION=1`，通用 manifest 仍声明 s8 写入；设计 VN-08/09 所列若干字段尚未全部输出 | 分角色/实际库报告能力，消除 s9 写入误拒与不实功能承诺；最终修正与验收 **pending** |
| s8 派生快照 | 双快照发放是 SC-01..09 的设计目标，当前候选尚无已验证实现 | 必须实现并验收，或明确批准不提供过渡快照并采用“所有客户端先升级”的方案；本轮不承诺旧客户端可读 s9 服务 |
| 厂商导入 / Vorbis | schema feature 标记不等于解析、扫描、导入和回滚业务实现；客户端具备部分 UI/HTTP 适配 | 服务端缺少厂商导入处理接口时隐藏未实现入口；字段映射、模板编辑、本地导入及 Vorbis 的未完成范围不得列作已交付 |

`-z1` 表示 Metadata 列存储格式，不是快照外层 zstd 传输。
不能因为发送 zstd，就宣称实现了 s9 `-z1` 格式。VN-08/09 的现状和目标区分见
[测试计划第 7 节](TEST-SEARCH-METADATA-V2.md#7-版本协商与提示)。

## 3. 两端兼容与迁移状态矩阵

表中“要求”仍须由最终候选验收；当前没有将 s9 内容伪装成 s8 的兼容通道。

| 客户端 / 服务状态 | 读取、搜索与缓存 | 写入与提示 |
| --- | --- | --- |
| 旧 0.8.7 → 原 s8 服务 | 沿用已有路径，保留旧快照 | 沿用已支持的 s8 写契约 |
| 新客户端 → 原 s8 服务 | 基础搜索及同步继续；缺能力时隐藏路径/厂商功能 | s8 写入仍须通过现有身份与权限检查 |
| 旧/新客户端 → 新二进制、库仍 s8 | 基础读取和旧快照保持可用；新功能只按实际能力开放 | 不因换二进制自动视作库已升级 |
| 回填中 → main / all / path | main 保持可读；all 回退 main 并带 unavailable；path 返回 503 `migration_in_progress`、Retry-After 30 | 回填期间普通支持写入可继续；客户端显示阶段及已提交计数 |
| switching | 读取继续；保留当前缓存，拒绝混合代际状态 | 写入 503 `migration_switching`、Retry-After 30；按原请求语义重试，避免重复提交 |
| 新客户端 → 完成 s9 | **pending**：真实 s9 全量同步、只读打开、路径查询、失效代际和重开验收 | **pending**：按实际 s9 写格式准入及 Metadata 写回，保留权限和 library_id 校验 |
| 旧 0.8.7 → 完成 s9 | 当前不承诺兼容快照；不支持的格式应在正文前拒绝并保留旧缓存 | 旧写请求必须被拒绝，不能因 HTTP 路由仍存在而获准写 s9 |
| 回滚后 → s8 | 重新确认格式、revision 和库身份，失效旧请求，重新同步 | 不复用旧 s9 写准入；不覆盖工作区、标注或偏好 |

离线、取消、校验失败及不支持的未来格式均须保留最后一份已验证缓存。
新快照只有完成身份、结构与完整性校验后才能发布给 UI；服务端模式不能迁移客户端本地工作库。

## 4. 分阶段执行方案

以下为部署前可审阅方案，当前不执行生产步骤；地址、token、根目录及配置只由私有环境提供。

1. **关闭源码缺口。** 完成兼容性修正及未实现入口隐藏；解决 VN-08/09 的能力现状差异。
   复核 client/server 合并输入，保留 beta 主题、工作区与数据保护行为；冻结候选身份。
   若双快照未实现，先确认所有需要连接的客户端均能使用真实 s9，并取得无旧端过渡支持的明确决定。
2. **在拥有的副本演练。** 从只读备份输入生成隔离库，登记 s8 格式、library_id、业务记录/标注、
   扫描历史和主 FTS5 基线；验证额外可用空间至少为库大小的 1.3 倍。
   使用隔离随机端口及合成/副本配置，不连接生产或现有 beta 服务。
   完成迁移、取消/恢复、进程退出恢复、重开、完整快照同步及回滚演练。
3. **先准备客户端。** 先交付能连接原 s8 且实测能读 s9 的客户端；保留服务配置、缓存、
   本地库路径和个人工作区。本地路径索引提示可以“稍后”，不能据此改写服务端库或宣称本地 schema 已升 9。
4. **部署新服务二进制但暂不迁移。** 授权后核对 launcher、配置、数据目录、提交与二进制 hash，
   保留原二进制和备份。当前 `AppState` 启动调用 `open_server_existing`，不会主动执行 s8→9；
   schema 8 仍走普通已支持的打开路径，较旧库原有迁移另有行为，不能据此保证所有启动均只读。
   `AUDIOLIB_DEFER_SCHEMA9=1` 仅被迁移器 `startup` 动作消费，不能代替真实启动和库版本检查。
   当前 HTTP 管理动作不接受 `startup`。未显式迁移前应核对实际仍为 s8，读取及原客户端仍可用。
5. **显式触发并监视。** 授权后，通过支持的管理接口或 `audiolib-migrate start --db <owned-copy>`
   开始；实际工作库路径只在批准的私有运行配置中提供。先确认唯一 owner、权限与库身份。
   监视 backup / path_build / optimize / switch，保持回滚备份；状态 paused/failed 时停止后续推广。
   不删除锁侧文件，不强制终止占用者，不在迁移中 VACUUM。
6. **完成后验收并同步。** 核对实际 `user_version=9`、sd1 格式、三项 feature 和修订，
   确认目录/runs 覆盖、主 FTS5、业务投影及未知 Metadata 保留。
   由新客户端完成真实快照下载、重开、路径查询、播放与受支持写入；核对记录数及库身份，
   保留配置根外不可播放的既有边界。旧客户端拒绝行为或已实现双快照分别单独验收。
7. **推广与交付。** beta/main 提升、标签、正式构建和发布须另获授权；各最终 payload
   从最终身份构建并验收，固定安装同步完成后才报告交付完成。

### 回退与恢复

- 切换前：pause/cancel 在持久批次边界生效；确认终态及 s8 可用，保留 checkpoint/备份供诊断或恢复。
- 切换后：当前目录迁移器支持验证备份后、持独占管理与扫描 lease 的逆向事务回滚，
  尽量保留备份后的普通注释及扫描写入；必须按最终 recipe 和实际新增功能再验证。
  不将历史设计中的“直接恢复整个备份”作为默认操作，否则可能覆盖新写入。
- 若新增 Vorbis、厂商字段或其他 s9 业务数据后不能无损退回 s8，停止自动回退，先评估和保全数据。
  不向旧二进制交付不支持的库；完整备份恢复另需授权及写入排空，并明确备份之后数据的处置。
- 所有回退保留源 WAV、原备份、Metadata、标注及未知扩展；不以重扫代替数据保全。

## 5. 最小验证范围与发布阻塞

集成候选后续修改由实际风险选择检查；已有全仓结果不能覆盖新合并行为。

| 阶段 | 所需证据 | 当前 |
| --- | --- | --- |
| 契约与文档 | schema/features/能力/Retry-After 一致；JSON 可解析、相对链接、`git diff --check` | 准备中；本轮统一 switching 为 30 |
| 兼容修正 | core/db/remote/server 相关严格 Clippy 与测试，覆盖直接消费者、s9 实际读/写/快照及拒绝路径 | pending，记录最终源码与特性 |
| 迁移安全 | MG 24 条及受修正影响的迁移/管理回归；跨进程控制、备份、取消、恢复、切换拒写 | 原 d29cd35 已通过；新增改动影响须复核 |
| 两端真实副本 | s8→s9、完整快照同步、保留旧缓存、重开及回滚；旧新客户端配对矩阵 | pending，不接生产 |
| 搜索 | main 与 s8 一致，路径 oracle/分页/过滤限制正确；变更影响时在同机同库副本复验冷/热查询闸门 | 历史冷测保留，集成有效性 pending |
| 桌面交互 | 升级稍后/取消/续跑、真实进度、点击刷新、主题/中文英文、关闭排空；未实现入口隐藏 | 原 debug 证据保留；新整合 pending |
| 实际包 | 本机平台 build/package/启动、许可证、隐私、身份与实际配置 | 全部新版 payload 未生成 |
| 最终安装 | 固定安装 package-owned 文件 SHA256、CLI 版本与隔离桌面启动 | 未执行 |

配置/Metadata/扫描协调或关闭行为受影响时，选择对应 `validate-review-fixes.ps1`、
`validate-desktop-close.ps1`；库管理/恢复行为受影响时选择 `validate-library-management.ps1`。
播放或 IPC 受影响时使用相应回归。真实平台/硬件未具备时记录 unverified，不靠重复执行替代。
ASIO 使用已验证 SDK 和实际发布配置；不串行再跑一套无关非 ASIO 全矩阵。

性能目标仍为 **P95 latency <500ms for 1M+ records**；s9/s8 相对闸门为原约定的
每查询五轮、每轮 p95 的中位数不超过 s8 ×1.2，两者应分别报告。

当前阻塞：兼容修正尚未验收、双快照未实现且无过渡策略定稿、真实两端副本验收未完成、
厂商/Vorbis 功能范围与入口待收口、最终版本与跨平台 payload 未冻结。
后台缓存维护分别报告外部未登记约 140.52 GiB / 80.97 GiB，回收 0，40 GiB 预算未达成；
不自动清理未登记历史缓存，不将该限制隐藏为清理成功。

## 6. 七包、固定安装与完成条件

范围和版本冻结后先按 [版本规范](Rust_Versioning.md) 及 [发布管线](Rust_Release_Pipeline.md)
预检，再以最终 main/tag 身份在匹配原生主机构建各 payload 一次、验收一次。
目前 Cargo 未改至 0.9.0，也未创建对应标签，不能运行正式交付后称其完成。

| 公开归档（`<tag>` 为最终批准标签） | 验收与更新边界 |
| --- | --- |
| `AudioLibExplorer-<tag>-win-x64-portable.zip` | 实际 ASIO 配置、portable 验收、固定安装 |
| `AudioLibExplorer-<tag>-macos-arm64.zip` | 原生结构/签名/启动退出；ad-hoc 不等于 Apple 公证，手动更新 |
| `AudioLibServer-<tag>-win-x64.zip` | 原生服务包及隔离 health；显式构建 acceptance 所需 benchmark helper |
| `AudioLibServer-<tag>-macos-arm64.tar.gz` | 原生服务启动、迁移/重开及包身份，不自动部署生产 |
| `AudioLibServer-<tag>-linux-x64.tar.gz` | 匹配环境的服务与文件系统验收；仿真须明确记录 |
| `AudioLibServer-<tag>-docker-linux-amd64.tar.gz` | 消费已验收 Linux 输入、离线镜像与运行检查 |
| `AudioLibServer-<tag>-docker-linux-arm64.tar.gz` | 同上；独立 Linux arm64 原生包是内部输入，不列公开资产 |

各归档附 `.sha256`，合计十四项资产。Docker 通过替换镜像升级，禁止在容器内自动换二进制；
Mac 客户端按既有手动流程更新。发布核验只核远端元数据/摘要，不能替代本机迁移和包验收。

Windows 必须按 [仓库固定安装流程](../AGENTS.md#mandatory-local-application-delivery-and-release-installation)
将已通过 portable 验收的完整包安装至仓库固定 `local_release/`：正常关闭并排空写入/扫描、
只复制 package-owned 文件、不清空目录，核对所有文件 SHA256、CLI 版本及一次显式隔离数据目录启动。
保留日常库、settings、日志、缓存和用户原环境；beta 独立安装不能替代固定安装。
若以后获准发布，再将固定安装与实际发布资产摘要闭环核验，分别报告远端发布和本地同步结果。
交付成功后执行一次有归属证据的缓存预算维护，保留 release 输出与诊断要求。
