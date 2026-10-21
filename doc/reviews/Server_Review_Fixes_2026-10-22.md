# Server Review 修复记录（2026-10-22）

修复基线为远端 `main` 的 `4f98838`，原审查见 [Server Review](Server_Review_2026-10-22.md)。原工作区已有未提交的客户端/快照改动，全部保留；本次在独立工作树中修改服务器及相关测试、部署文档。

## 问题处理

| 编号 | 结果 |
| --- | --- |
| S-H1 | 免令牌 API 校验单一合法 Host，只允许 IP/LAN 名称或已配置 public_url/lan_url 的主机名；有效 bearer 可访问其他主机名。重复、缺失和畸形 Host 有真实连接回归。 |
| S-H2 | 最新 main 已将服务端播放历史停用：`POST /audio/{id}/plays` 为兼容只读路由，不写库、不推进 revision；客户端历史在本地。本次验证现有回归，没有重新改写语义。 |
| S-M1 | 默认 manage_mode 从 all 收紧为 lan；外部 token 默认不能浏览服务器目录、添加根或扫描。显式 all 设置保留，表示授权完整管理；本次没有引入 token scope。 |
| S-M2 | 增加 trust_lan_peers=false，把隐藏来源的私网网关也按外部请求处理并关闭无密码 admin。容器配置启用此策略并保持 auth_mode=all，compose 仅绑定 127.0.0.1。其他 NAT/反代部署需显式采用同样配置，通过 TOML 管理。 |
| S-M3 | 每分钟读取证书/私钥内容指纹，验证后热换 rustls resolver；半更新或读取失败保留旧证书，记录告警并继续检查。真实 TLS 回归覆盖新握手及已建立会话。 |
| S-M4 | 按真实 socket IP 限制每分钟 60 次失败，返回 429 AUTH_RATE_LIMITED、Retry-After:60；来源表最多 1024 项，日志不记录 token。自定义公网 Host 的 bearer 验证也受限。环境 token 至少 32 字符，旧短 token 升级前需替换。 |
| S-M5 | 以库身份/revision 与波形 SQLite data_version/total_changes 缓存不可变整包；生成名额在准备完成后释放，最多 8 个下载，每个发送最长 15 分钟。旧响应持有文件引用；owner 锁、新建宽限和关闭检查保护活跃文件及关闭流程。 |
| S-L1 | 默认 snapshot_dir 改为库文件旁 snapshots/；启动清理仅删除带 owner 标记、锁已释放且至少 30 秒的残留。 |
| S-L2 | 500/507 响应使用概括信息，原始路径和内部诊断只写服务端日志。 |
| S-L3 | Windows ffmpeg 输入加 file: 前缀；Unix 继续读取已验证文件描述符。 |
| S-L4 | 容器 HEALTHCHECK 同时检查 HTTP 成功和 JSON status=ok。 |
| S-L5 | 管理页单独保留 1 个文件系统名额，客户端 API 使用原有 4 个名额，互不抢占。 |
| S-L6 | 每个 HTTP/TLS 监听器最多 1024 个连接，未完成握手也计入；超额关闭。回归覆盖 HTTP 空闲连接及挂起 TLS 握手。 |

## 验证

- Windows x64，Rust 1.97.1 MSVC；构建/测试输出放在本机 NTFS，未修改工作库和 WAV。
- 格式与 `git diff --check` 通过；服务器与直接消费者 audiolib-remote 的 all-targets 严格 Clippy 通过。
- 服务器 273 项通过，2 项原有大型验收/基准测试保持 ignored。以包级测试启动，旧测试 Host 和默认管理权限假设修正后只重跑受影响目标，复用其他通过结果。
- audiolib-remote all-targets：225 项通过，2 项需要显式提供可丢弃服务的既有 live 测试保持 ignored。
- Docker HEALTHCHECK 的 shell 语法及 ok/starting/error/空响应状态匹配检查通过；没有启动容器。
- 集成随后进入 main 的 `93fba88`（仅文档、工作流和验证脚本）后，修复提交为 `b165173f1a53751b4f11bc460cd7e2a938e46aca`。对比集成前后的 server/crates/app、Cargo 与工具链输入没有变化，复用上述 Rust 测试结果。
- `tools/build-release.ps1` 使用验证过的 Steinberg ASIO SDK 2.3.4 成功：首次完整构建 5 分 57 秒；集成后为更新服务器内嵌提交号进行增量构建，58.71 秒。
- 便携包打包、发布边界/隐私检查和 `validate-portable.ps1 -CleanCheckout` 通过，涵盖隔离首启、合成 WAV 扫描、CLI、中文 UI 及启动失败诊断。集成后重新打包更新文档，两 EXE 哈希与已通过冒烟的包完全一致，因此复用完整便携冒烟；另通过新增的 `test-package-boundary.ps1`。
- 服务器包重新打包及 `validate-server-package.ps1 -Tag v0.6.0 -Target x86_64-pc-windows-msvc -Asio` 通过；校验归档边界/校验和、EXE 版本/哈希，以及隔离合成库 `/health` 的 API v1、schema 6 和提交 `b165173`。Windows 的该脚本只终止自己创建的测试进程，优雅关闭由 Rust 回归覆盖。

## 最终安装身份

最终包的 EXE SHA256（版本 0.6.0，源码提交 `b165173`）：

| 文件 | SHA256 |
| --- | --- |
| AudioLibExplorer.exe | `969D03FF7EB73CEB4ABA7EB2D0AAC12118BA226E68F474094D2D9D83C3C22D60` |
| AudioLibCli.exe | `D5084F38062A0827D5A15C2F8459A959CB1938B67045C4188A74A95EC40CCA47` |
| server/audiolib-server.exe | `02A6F64D5F7A5ED6A99A12F30A54B1E1B02827F218998366B1AB7D32C6057768` |

固定 `local_release/` 已同步并逐文件核对便携包的 1230 个文件；安装后 CLI 版本 0.6.0、使用显式隔离设置及自有临时库的桌面启动通过。服务器装在同一固定安装内的 `local_release/server/`，420 个文件哈希全部与最终服务器包相同，安装后 `--version` 为 0.6.0。两客户端 EXE 和服务器 EXE 哈希均符合上表。

安装只复制包拥有的文件，没有镜像清理或覆盖库、SQLite sidecar、用户设置、日志与缓存；隔离验证后恢复正常数据目录环境。原分支 `codex/debug-snapshot-reports` 与其既有未提交改动保留。本次只生成本地包，没有创建发布标签或 GitHub Release。

## 覆盖限制

本机没有 Linux/macOS 执行环境或容器运行时，因此 Unix 专用分支及 Docker 部署运行未验证；不把静态配置检查当作平台运行结果。Windows 未配置真实 ffmpeg，file: 输入路径的实际 FLAC 转码未验证。此次不涉及真实音频设备后端或百万库搜索性能变更，未重复硬件/百万库验收。
