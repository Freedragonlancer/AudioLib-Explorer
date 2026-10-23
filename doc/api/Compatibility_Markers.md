# 基于标记的同步兼容判定（设计草案）

状态：**设计草案，未实现**（PR #97）。目标：客户端与服务端同步兼容**不再依赖应用版本号**，改由三类标记判定：`protocol`、库格式 ID（schema/format）、`features`。应用版本只用于显示、更新检查和可选的已知坏版本黑名单。现行契约见 [Library_Migration_Compatibility.md](Library_Migration_Compatibility.md)。

## 1. 现状（代码对照）

| 位置 | 现行字段/逻辑 | 问题 |
|---|---|---|
| `crates/audiolib-core/src/compatibility.rs` `CompatibilityManifest` | `application_version`、`catalog_revision`、`peer_application_ranges`、`legacy_peer_application_ranges`、`direct_use_/auto_migrate_application_ranges`、`api_protocols`（`"api/v1"`）、`snapshot_read_formats`、`database_write_formats`、`migration_*`、`minimum_sqlite_version` | 版本范围是硬门槛 |
| `peer_versions_compatible()` | 双向检查对方版本落在 `peer_application_ranges`，且 `catalog_revision == CATALOG_REVISION`，且 `api_protocols` 有交集 | 任一侧发新 minor（如 0.8.0）即拒绝，即使协议和格式完全一致 |
| `SnapshotFormat::is_readable_by` / `LibraryFormat::snapshot_format_id()` | 格式 ID 由实际库推导（`ale-s6-cld1-t3-m2-p1` 等）并须在 `snapshot_read_formats` 中 | 已是正确的格式判定，保留 |
| `crates/audiolib-remote/src/compatibility.rs` | `peer()`、`library()`、`live_library()`（探测/轮询/每次写入前复查）、`response_format()` 校验快照头 | `peer()` 依赖版本 |
| `server/audiolib-server/src/compatibility.rs` | `admit()`（快照）、`admit_peer()`、`admit_mutation()`、`admit_library_mutation()` | `admit_peer()` 依赖版本，返回 `409 PEER_VERSION_UNSUPPORTED` |
| `server/audiolib-server/src/manage.rs` `CapabilitiesResponse` | `compatibility` + `list_waveforms`、`list_waveform_jobs` 等可选能力 | 可选能力已按“缺失即关闭该功能”处理，保留并统一为 features |

## 2. 三类标记

| 标记 | 类型 | 含义 | 何时变更 |
|---|---|---|---|
| `protocol` | 整数区间 `{min, max}`（当前 `1..=1`，对应 `api/v1`） | API 线协议 | **仅**在不兼容 API 变更时 +1（删除/改名字段或端点、改变语义、改变必需请求头）。新增可选字段/端点不升 |
| 库格式 ID | 字符串（现有 `ale-s{schema}-...`） | 快照/库的持久化格式 | schema（`user_version`）、FTS 存储、必需 feature 行、tokenization/source revision 任一改变 → 新格式 ID。读方只看“能否读该 ID” |
| `features` | `{name: revision}` | 可选能力 | 新增可选功能时添加；语义不兼容地改变某能力时提高其 revision。缺失只关闭该功能，**永不**拒绝整个同步 |
| `application_version` | semver 字符串 | 显示、更新检查 | 不参与兼容判定；仅可命中显式 `blocked_versions` |

## 3. 承载位置

| 标记 | 服务端 → 客户端 | 客户端 → 服务端 |
|---|---|---|
| protocol / features / 可读写格式 | `GET /api/v1/capabilities` 的 `compatibility` 对象；快照响应头 `x-audiolib-server-compatibility`（同一 JSON） | 每次修改请求及快照请求带 `x-audiolib-client-compatibility`（同一 JSON，≤ `MAX_COMPATIBILITY_HEADER_BYTES` 4096） |
| 快照格式 | 快照响应头 `x-audiolib-snapshot-format`（ID）、`x-audiolib-library-format`（`LibraryFormat` JSON）、`x-schema-version`、`x-search-storage-revision`、`x-tokenization-revision`、`x-source-search-revision`、`x-source-tags-revision` | `x-audiolib-accept-snapshot-formats`（逗号分隔 ID）、`x-audiolib-snapshot-schema-max` 等现有上限头 |
| 应用版本 | `/health.version`、`compatibility.application_version` | `compatibility.application_version` |

### 3.1 `compatibility` JSON（新增字段，旧字段在过渡期保留）

```json
{
  "application_version": "0.7.2",
  "protocol": { "min": 1, "max": 1 },
  "snapshot_read_formats": ["ale-s3-ext-t3-m2", "ale-s4-ext-t3-m2", "ale-s5-ext-t3-m2",
                            "ale-s6-cld1-t3-m2-p0", "ale-s6-cld1-t3-m2-p1"],
  "database_write_formats": ["ale-s6-cld1-t3-m2-p1"],
  "features": { "waveform_bundle_strong_etag": 1, "list_waveforms": 1, "list_waveform_jobs": 1 },
  "blocked_versions": [],
  "marker_revision": 1,

  "catalog_revision": 1,
  "api_protocols": ["api/v1"],
  "peer_application_ranges": [">=0.6.0, <0.7.0", ">=0.7.0, <0.8.0"],
  "legacy_peer_application_ranges": ["=0.4.11", "=0.5.0", "=0.5.1"]
}
```

- `marker_revision` 存在即表示对端采用本设计（“标记对端”）；缺失即“版本对端”（0.7.0/0.7.1 及更早）。
- 未知字段一律忽略（serde 默认），以便将来追加 features 不需升 protocol。
- `api_protocols: ["api/v1"]` 与 `protocol` 互相映射：`"api/vN"` ⇔ `N`。

## 4. 判定算法

```text
fn protocol_ok(local, peer):
    if peer.marker_revision is some:
        return max(local.protocol.min, peer.protocol.min) <= min(local.protocol.max, peer.protocol.max)
    // 版本对端：用 api_protocols 映射出协议号
    return peer.api_protocols ∩ local.api_protocols ≠ ∅

fn peer_ok(local, peer_version, peer_catalog):
    if peer_version in local.blocked_versions: return Err(BlockedVersion)
    match peer_catalog:
        None    => return legacy_ok(local, peer_version)        // 0.4.11/0.5.x 无目录旧端，保持现状
        Some(p) => if p.application_version != peer_version: return Err(Inconsistent)
                   if !protocol_ok(local, p): return Err(ProtocolMismatch)
                   return Ok

// 1) 探测 / 轮询（客户端，live_library）
probe():
    caps = GET /api/v1/capabilities  (404 => 无目录旧端)
    peer_ok(local, health.version, caps.compatibility)?
    format_ok(observed_library)?                      // 现有 library()：ID == 推导值 且 ∈ local.snapshot_read_formats
    enabled_features = { f | f ∈ caps.features 且本地支持其 revision }  // 其余功能仅隐藏/降级

// 2) 快照下载
client: 发送 x-audiolib-client-compatibility + x-audiolib-accept-snapshot-formats = local.snapshot_read_formats
server admit(): peer_ok(server, client) && format.id ∈ accepted && format.is_readable_by(client)
                否则 409 SNAPSHOT_FORMAT_UNSUPPORTED / PROTOCOL_UNSUPPORTED
client response_format(): 头部互相一致 且 id ∈ local.snapshot_read_formats；
                          下载后 downloaded() 复核文件实际格式 == 头部，否则保留旧缓存

// 3) 修改请求
client: 每次带 x-audiolib-client-compatibility，事先 live_library() 复查（不缓存准入）
server admit_library_mutation(): peer_ok && 当前库 format.is_readable_by(client)
server admit_mutation()（设置/缓存任务）: 有头则 peer_ok，无头沿用现行旧调用方豁免
依赖某 feature 的请求：服务端未声明该 feature → 客户端不发；服务端收到不支持的 → 404/400，仅该功能失败
```

`direct_use_/auto_migrate_application_ranges` 改由格式判定（`assess_library` 已基于 `LibraryFormat`），版本范围不再参与。

## 5. 变更规则（PR 检查清单）

1. **升 protocol**：删除/重命名端点或必需字段、改变已有字段语义或单位、使旧客户端的合法请求被拒或被误解。同时更新 `protocol.max`；若仍支持旧协议则保留 `min`。
2. **新格式 ID**：`TARGET_SCHEMA`、`LibraryFormat::target()` 中任一 feature/revision、`SearchStorage`、`TOKENIZATION_REVISION`、source revision 变化；或表/列/索引结构变化（即使 `user_version` 未变）。旧 ID 只要仍可读就保留在 `snapshot_read_formats`。
3. **加 feature**：新增可选端点/字段/行为（如强 ETag waveform bundle）。不升 protocol。若功能变为必需，则属于 protocol 变更。
4. **只改应用版本**：不改任何兼容标记；发版不应引起兼容失败。
5. **CATALOG_REVISION**：退役。它把“目录结构版本”变成了硬等值门槛，与 protocol 重叠。过渡期仍输出 `catalog_revision: 1` 供 0.7.0/0.7.1 校验（它们要求等值），标记对端不再检查；目录结构变化由 `marker_revision` + serde 忽略未知字段承担。

## 6. 从 0.7.0 / 0.7.1 过渡

- 0.7.0/0.7.1 已发布，按版本范围判定（`>=0.7.0, <0.8.0` 及 0.6.x）且要求 `catalog_revision == 1`。它们无法被改变。
- 新代码（0.7.2+）输出标记字段，**同时**继续输出旧字段：`catalog_revision: 1`、`api_protocols: ["api/v1"]`，以及覆盖所有声明同一 protocol 的已知版本的 `peer_application_ranges`。因此 0.7.2 与 0.7.0/0.7.1 互认（旧端看到 0.7.2 在 `<0.8.0` 内）。
- 新代码接受“版本对端”：其 `api_protocols` 映射出的协议号与本端相交，且所声明快照格式可读，即放行（不再检查对方版本是否在本端范围内）。
- 限制：若未来某一侧升至 0.8.0，旧 0.7.0/0.7.1 端仍会拒绝它。所以**两端都需先升级一次到 0.7.2（或更高的标记版本）**，此后版本号可自由前进。0.7.2 的 `peer_application_ranges` 为兼容旧端可写宽（如 `>=0.6.0`），因为标记端自己不再读取它。
- 无目录旧端（0.4.11/0.5.x）维持 `legacy_peer_application_ranges` 现状。

## 7. 防护测试

- **格式指纹 fixture**：`crates/audiolib-core/tests/fixtures/format_fingerprint.json` 记录 `{format_id: PLAN_B_FORMAT, schema: TARGET_SCHEMA, target: LibraryFormat::target(), ddl_sha256: <新建空库 sqlite_master 中 type/name/sql 排序后的哈希>}`。测试：新建空库算指纹；若 `ddl_sha256` 或 `target()` 变了而 `PLAN_B_FORMAT` 未变 → 失败，提示“改 schema 必须给新格式 ID 并加入 snapshot_read_formats”。
- **协议指纹**：对 `CapabilitiesResponse`、快照头名、修改请求必需头做快照测试；有删除/改名而 `protocol.max` 未变 → 失败。
- **版本无关性**：用同一目录、仅改 `application_version`（如 `0.7.2`↔`9.0.0`）互验，必须通过。
- **旧目录 fixture**：保存真实 0.7.0、0.7.1 输出的 `compatibility` JSON，新代码须接受；并断言新输出能通过 0.7.1 的 `peer_versions_compatible`（把旧函数复制到测试中）。

## 8. 测试矩阵（客户端填写）

| # | 客户端 | 服务端 | 期望 | 探测 | 快照 | 修改 | 备注 |
|---|---|---|---|---|---|---|---|
| 1 | 0.7.2 | 0.7.2 | 通过 | | | | |
| 2 | 0.7.2 | 0.7.1 / 0.7.0 | 通过 | | | | |
| 3 | 0.7.1 / 0.7.0 | 0.7.2 | 通过 | | | | |
| 4 | 0.7.2 | 0.6.x | 通过（同协议+可读格式） | | | | |
| 5 | 0.7.2 | 标记端，版本 0.8.0，同协议同格式 | 通过 | | | | |
| 6 | 0.7.1 | 0.8.0 | 拒绝（旧端限制，提示升级） | | | | |
| 7 | 标记端 | 协议区间不相交 | 拒绝 `PROTOCOL_UNSUPPORTED` | | | | |
| 8 | 标记端 | 未知格式 ID | 快照拒绝，保留旧缓存 | | | | |
| 9 | 标记端 | 缺 `waveform_bundle_strong_etag` | 同步通过，仅该功能降级 | | | | |
| 10 | 标记端 | 版本在 `blocked_versions` | 拒绝，明确提示 | | | | |
| 11 | 0.7.2 | 0.5.1（无目录） | 维持现状 | | | | |
| 12 | 快照头与文件实际格式不一致 | — | 拒绝，保留旧缓存 | | | | |

## 9. 待确认问题

1. `skipped_newer` 目前是 `audiolib-update` 的更新检查结果字段，不是同步能力；是否确实要作为 feature？
2. `protocol` 是否直接用整数替代 `"api/v1"` 字符串，还是长期并存？
3. `blocked_versions` 由谁维护：内置于二进制，还是随更新检查清单下发？
4. 0.7.2 的旧兼容字段 `peer_application_ranges` 写宽到什么程度（`>=0.6.0` 无上限？）。
5. features 是否也需要客户端声明（服务端据此调整响应），还是只由服务端声明？
