# 服务端代码 Review 报告

> 补交原始审阅记录，以下结论与行号基于下述旧工作区。后续 main 已有 schema v6、格式兼容和客户端本地播放历史等变更；本文件不表示所有问题仍可在当前版本复现，修复前应重新核对当前实现。

- 日期：2026-10-22
- 基线：分支 `codex/debug-snapshot-reports` 的工作区，即提交 `c5c6ca1` 加上审阅时尚未提交的改动（`routes.rs`、`state.rs`、`error.rs` 等文件有修改，`snapshot_registry.rs` 是未跟踪的新文件）。行号以审阅时的工作区为准。
- 范围：`server/audiolib-server`。重点是鉴权与请求来源判定（`auth.rs`）、管理页（`admin.rs`、`admin/`）、路由与准入中间件（`routes.rs`）、音频流的路径约束（`state.rs`）、预览转码（`preview.rs`）、快照与波形整包（`state.rs`、`wfdb.rs`）、根目录管理与目录浏览（`manage.rs`、`browse.rs`）、TLS（`tls.rs`）、监听循环（`server.rs`），以及部署样例（`deploy/docker`、`deploy/systemd`、`config.example.toml`）。`scans.rs`、`roots.rs`、`list_waveform.rs`、`pcm.rs` 只看了接口与限额，没有逐行审阅。
- 方法：对照 `doc/api/README.md` 的契约阅读代码，静态推理为主，没有部署实例做攻击验证。
- 客户端审阅见 [Client_UI_Review_2026-10-22.md](Client_UI_Review_2026-10-22.md)。

## 总体评价

服务端的防御细节做得很扎实：

- 音频路径先 canonicalize，再校验必须位于某个根目录内；打开后还会比对 inode，防止检查与打开之间被替换。
- 浏览器发起的修改请求要求 Origin 同源，并且必须是 `application/json`。
- 库修改请求必须带 `X-AudioLib-Library-Id`。
- 生成的令牌只保存 SHA-256 哈希，用常量时间比较，以 0600 权限原子写入。
- 管理页有 `Host` 白名单、自定义头、CSP、`X-Frame-Options` 和 `no-store`。
- 各类阻塞操作都有名额上限和超时；采用先监听、后开库的启动方式；写快照前检查剩余空间。

主要风险集中在**信任模型**：“局域网直连就免令牌，并拥有全部管理权限”这一默认组合，加上 `/api/v1` 缺少管理页那样的 `Host` 校验，在默认配置下会被 DNS 重绑定利用。另一个较大的问题是**播放计数会推进全库 revision**，在没有增量同步的情况下，多客户端会被迫反复全量同步。

## 问题汇总

| 编号 | 严重度 | 类别 | 摘要 |
|---|---|---|---|
| S-H1 | 高 | 安全 | `/api/v1` 不校验 `Host`：默认配置下，局域网内任一用户访问恶意网页，就可能被 DNS 重绑定读取整个库，并执行管理操作 |
| S-H2 | 高 | 性能 / 交互 | 播放计数推进全库 revision，而客户端只有全量快照同步：任何一次试听都会让其他客户端重新下载整库快照 |
| S-M1 | 中 | 安全 | 令牌没有权限分级，默认 `manage_mode = all`：任何令牌持有者都能浏览服务器整个文件系统、把任意目录添加为根并扫描 |
| S-M2 | 中 | 安全 / 部署 | “局域网”只按对端 IP 判定：在 Docker Desktop/OrbStack、rootless 容器、同机反代未加转发头等场景下，所有请求都被当成局域网请求，无密码的管理页随之暴露；compose 实际映射到 0.0.0.0，与注释不符 |
| S-M3 | 中 | 运维 | 原生 TLS 不会重新加载续期后的证书，Let's Encrypt 续期后仍使用旧证书，直到重启或重新保存设置 |
| S-M4 | 中 | 安全 | 鉴权失败没有限速，也没有带来源的日志；环境变量令牌最短只需 16 个字符 |
| S-M5 | 中 | 可用性 | 波形整包只有一个名额，而且在整个响应体发送期间都占着；慢客户端可长期独占；每次请求都会重新执行 `VACUUM INTO` |
| S-L1–L6 | 低 | 打磨 | 见“低优先级问题” |

---

## 高优先级

### S-H1 `/api/v1` 缺少 Host 校验，默认配置可被 DNS 重绑定

**前提**（全部是默认值）：`bind = "0.0.0.0:8790"`、`auth_mode = external`、`manage_mode = all`（[config.example.toml:11](../../server/audiolib-server/config.example.toml#L11)、[auth.rs:616](../../server/audiolib-server/src/auth.rs#L616)）。

**原因**

- `external` 模式下，局域网直连请求完全不检查令牌（[auth.rs:867](../../server/audiolib-server/src/auth.rs#L867)）。
- 管理页专门用 `is_lan_host` 防 DNS 重绑定（[admin.rs:103](../../server/audiolib-server/src/admin.rs#L103)），但 `/api/v1` 的准入链（`require_bearer` → `guard_api_mutation`，[routes.rs:262](../../server/audiolib-server/src/routes.rs#L262)、[routes.rs:395](../../server/audiolib-server/src/routes.rs#L395)）没有任何 `Host` 校验。
- 同源检查 `same_request_origin` 比较的是 `Origin` 与 `Host`（[auth.rs:226](../../server/audiolib-server/src/auth.rs#L226)）。发生重绑定后，两者都是攻击者的域名，校验会通过。

**攻击路径**：局域网内的某个用户打开攻击者控制的网页 → 该网页的域名在短 TTL 后重新解析为 `192.168.x.y` → 页面脚本以同源身份访问 `http://evil.example:8790/api/v1/...`：

- 读取：`/library`（含 `library_id`）、`/library/snapshot`（整库，含服务器绝对路径和播放历史）、`/audio/{id}/stream`（音频文件）；
- 写入和管理：带上读到的 `X-AudioLib-Library-Id` 和 `Content-Type: application/json`，可以修改 Metadata、`POST /audio/remove` 移除记录、`DELETE /roots/{name}`、`POST /roots` 添加任意目录为根、`POST /scans` 扫描、`GET /fs` 浏览服务器目录（见 S-M1）。

攻击者需要猜中服务器的 IP 和端口；8790 是默认端口，局域网网段也可以枚举。公开的重绑定工具链（如 Singularity）已经把这一步自动化了。

**建议**

1. 对**未持有有效令牌**的请求（`off` 模式和 `external` 的 LAN 分支），要求 `Host` 满足 `is_lan_host`，或等于 `public_url` / `lan_url` 的主机名；不满足时返回 403，可复用 `ADMIN_LAN_ONLY` 或新增一个错误码。这个改动小，可以直接复用 `admin.rs` 里的函数。
2. 增加回归测试：LAN 对端 + 公网域名 `Host` + 无令牌 → 403；LAN 对端 + IP `Host` → 200。
3. 在 API 文档 §2.2 中写明这条规则。

### S-H2 播放计数推进全库 revision，引发其他客户端反复全量同步

- `POST /audio/{id}/plays` 走的是 `AppState::write` → `record_own_write`，每次都会让 revision +1（[state.rs:1204](../../server/audiolib-server/src/state.rs#L1204)、[state.rs:1133](../../server/audiolib-server/src/state.rs#L1133)）。
- 客户端只能通过 `GET /library/snapshot` 全量同步（增量接口 `/library/changes` 在文档 §6.5 中仍是草案）。客户端自己的写入可以按 revision 规则在本地打补丁，但**其他客户端的写入**在对方轮询时会表现为 `NewRevision`。
- 后果：
  - 多台客户端同时使用时，A 每试听一次，B 就会看到“服务器素材库有更新”。开启自动同步时，会下载整库快照（百万级库在客户端测试夹具中约 18 GB 解压后大小、约 1.5 GB zstd 传输量）。如果有人持续试听，服务器一直“不稳定”，自动同步会一直推迟，提示条也一直挂着。
  - 服务端的快照缓存按 `(library_id, revision)` 失效（[state.rs:1798](../../server/audiolib-server/src/state.rs#L1798)），每次播放后的下一次快照请求都要重新做一次整库 `backup_library`，在 NAS 上意味着大量 I/O。

**建议**：短期把播放计数改为不推进 revision（单独维护 `plays_revision`，或者使用 `write_if` 并约定播放历史不属于快照一致性的范围），客户端的“最近播放”改为直接读取 `/history/plays`；长期实现 `/library/changes` 增量同步。如果改动 revision 语义，需要同步更新文档 §4、§6.3 和客户端的 revision 规则。

---

## 中优先级

### S-M1 令牌不分权限：可浏览整个文件系统、任意添加根目录

- 客户端 API `GET /api/v1/fs` 列出服务器上任意绝对路径的子目录（支持 `hidden=1`），并统计 WAV 和其他文件的数量（[routes.rs:664](../../server/audiolib-server/src/routes.rs#L664)、[browse.rs:106](../../server/audiolib-server/src/browse.rs#L106)）；`POST /api/v1/roots` 可以把任何已存在且不与现有根重叠的目录加为根（[manage.rs:448](../../server/audiolib-server/src/manage.rs#L448)）。
- 这两个接口只受 `manage_mode` 限制，而默认值 `all` 表示“任何通过鉴权的请求都可以”。令牌本身没有“只读 / 试听 / 管理”之分。
- 结果是：发给朋友只用于试听的令牌，从外网就能枚举服务器的目录结构（例如 `/Users/<name>/...`），把 `~/Downloads` 之类的目录加为根并扫描，然后流式下载其中所有 WAV。

**建议**：令牌增加 scope（`read` / `write` / `manage`），管理页生成令牌时可以选择；或者至少把 `manage_mode` 的默认值改为 `lan`；`/fs` 可以限制在已挂载卷或配置的“可选根前缀”范围内浏览。

### S-M2 “局域网”判定只依赖对端 IP，在部分部署形态下会失效

- `classify` 看两件事：是否有转发头，以及对端 IP 是否属于私有网段（[auth.rs:213](../../server/audiolib-server/src/auth.rs#L213)）。在以下场景中，**所有**请求的对端都是私有地址，并且不带转发头：
  - Docker Desktop / OrbStack（macOS，README 中写的验证环境）、rootless Docker / Podman（slirp4netns）这类用户态端口转发，容器看到的来源是网关地址；
  - 同机或局域网内的反代没有配置 `X-Forwarded-For`（文档已经提醒，但这是一个极易出错的点）；
  - 路由器端口转发启用了 SNAT 或 hairpin NAT。
- 这时来自公网的请求会被当成局域网请求：`external` 模式下免令牌；**无密码的管理页**也可以访问，攻击者能把 `auth_mode` 改成 `off`、生成令牌、添加根目录。
- `docker-compose.yml` 的注释写着“Bind to 127.0.0.1”，但实际映射是 `"8790:8790"`，即所有网卡（[docker-compose.yml:17](../../server/deploy/docker/docker-compose.yml#L17)）。

**建议**：compose 默认改为 `"127.0.0.1:8790:8790"`；管理页增加可选的管理密码，或首次启动时在服务端日志中打印一次性配对码；增加 `trusted_peers` / `lan_cidrs` 配置，让部署者显式声明哪些网段算局域网；健康检查或管理页检测到“容器内 + 所有对端都是网关地址”时给出警告。

### S-M3 原生 TLS 不会自动加载续期后的证书

TLS 管理器只在设置发生变化时重新 bind 并加载证书（[tls.rs:64](../../server/audiolib-server/src/tls.rs#L64)），不监视证书文件。Let's Encrypt 每 60–90 天续期一次，续期后服务端继续使用内存中的旧证书，到期后客户端因证书校验失败无法连接，直到重启或在管理页重新保存 TLS 设置。

**建议**：定期（例如每 12 小时）检查证书文件的 mtime 或内容哈希，变化后热加载（`ResolvesServerCert` 可以做到不断开连接）；管理页显示证书到期时间。

### S-M4 鉴权失败没有限速和来源日志

`authorize` 失败时直接返回 401（[auth.rs:867](../../server/audiolib-server/src/auth.rs#L867)），既不按来源限速，也不记录 peer。生成的令牌有 256 位熵，不受影响；但环境变量令牌只要求不少于 16 个可见 ASCII 字符（[config.rs:404](../../server/audiolib-server/src/config.rs#L404)），用户自选的弱口令可以被在线暴力尝试，而且管理员无从察觉。

**建议**：按 peer 记录 401 并做简单的令牌桶限速（经反代时 peer 是反代地址，可以先只记日志）；文档建议环境令牌使用 `al_` 格式或至少 32 个随机字符。

### S-M5 波形整包：单一名额被慢客户端长期占用，且每次都重新生成

`GET /waveforms/bundle` 只有一个全局名额，而且名额要一直持有到响应体发送完毕（[routes.rs:1993](../../server/audiolib-server/src/routes.rs#L1993)），这条路由没有时间上限。一个慢速或恶意的已授权客户端可以长时间占住名额，其他客户端只能一直拿到 503 `BUSY`。每次请求都会重新执行 `VACUUM INTO`（约 300 MB），即使 revision 没有变化。

**建议**：按 `(library_id, revision)` 缓存生成好的整包文件，多个响应共享同一份；生成完成后就释放名额，只把文件引用交给响应流；对整包下载设置最低速率或最长时限。

---

## 低优先级问题

- **S-L1** `snapshot_dir` 默认是系统临时目录（[README](../../server/README.md) Troubleshooting 一节）。在 `/tmp` 为 tmpfs 的发行版（Fedora、Arch 等）上，数 GB 的快照会占用内存。建议默认放到库文件旁边，或在启动时检测 tmpfs 并给出警告。
- **S-L2** 500 错误的 `message` 可能包含服务端路径（文档 §2.1 已注明）。对外部令牌持有者来说，这会泄露目录结构；建议外部请求只返回概括性的错误信息，完整内容写入日志。
- **S-L3** 在 Windows 上，FLAC 预览把路径直接作为 ffmpeg 的 `-i <path>` 参数（[routes.rs:1601](../../server/audiolib-server/src/routes.rs#L1601)），没有加 `file:` 前缀，路径中的特殊前缀会被 ffmpeg 当作协议解析。路径已经 canonicalize 并位于根目录内，风险很低，但加上 `file:` 更稳妥。
- **S-L4** Docker `HEALTHCHECK` 只检查 HTTP 200，不看 `status` 字段（README 已注明）。可以改用 `curl ... | grep '"status":"ok"'`，或增加一个 `/health?strict=1`。
- **S-L5** 管理页和客户端 API 共用 4 个文件系统名额（[admin.rs:362](../../server/audiolib-server/src/admin.rs#L362)）。令牌持有者可以通过浏览无响应的共享把名额占满，使管理员自己的目录浏览也收到 503。可以给管理页单独保留一个名额。
- **S-L6** 监听循环不限制并发连接数，只有 10 秒的请求头超时（[server.rs:82](../../server/audiolib-server/src/server.rs#L82)）。在局域网内可以被大量空闲连接耗尽文件描述符；建议增加连接上限（例如 1024），超出后直接拒绝 accept。

---

## 值得保留的设计

- 音频流的路径约束：canonicalize → 根目录前缀匹配 → 打开后比对 inode/设备；根目录外、缺失和非常规文件统一返回 404，不泄露路径是否存在（[state.rs:1567](../../server/audiolib-server/src/state.rs#L1567)）。
- 修改请求的三重防护：同源 Origin、必须是 JSON Content-Type、必须带库身份头；管理页再加自定义头和 Host 白名单。
- 令牌处理：只保存哈希、常量时间比较、明文只显示一次、`last_used_at` 按分钟节流写盘、文件 0600 原子写入。
- 管理页 JS 不使用 `innerHTML`，CSP 为 `script-src 'self'`，文件名中的恶意内容无法执行。
- 资源边界：流、预览、转码、波形批量、文件系统调用都有独立名额；对卡住的系统调用有专门的“未完成打开”计数。
- ffmpeg 通过 `/dev/fd/0` 读取已校验的文件句柄（Unix），不会重新按路径打开；`kill_on_drop` 保证客户端断开后进程被回收。

## 建议的处理顺序

1. **S-H1**：改动小、风险高，建议立即修复并补测试。
2. **S-M2** 中的 compose 端口绑定，以及 **S-M1** 把 `manage_mode` 默认值改为 `lan`：只是配置默认值的调整。
3. **S-H2**：需要同时调整服务端 revision 语义和客户端同步规则，先出设计。
4. **S-M3**、**S-M4**、**S-M5**：运维与可用性改进。
5. 低优先级问题按需处理。

## 验证记录

- 本次只阅读代码，没有修改任何源文件，也没有部署实例做攻击验证；S-H1、S-M2 的攻击路径是根据代码推导出来的。
- `cargo clippy -p audiolib-server --all-targets --locked -- -D warnings`：通过（0 个 error / warning；Windows MSVC 工具链，`CARGO_TARGET_DIR` 指向本地临时目录）。服务端的主要目标平台是 macOS/Linux，Windows 上的编译只覆盖 `cfg(not(unix))` 分支。
