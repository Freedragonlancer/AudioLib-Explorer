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
- 发布构建、打包及固定安装的最终结果在交付后补记。

## 覆盖限制

本机没有 Linux/macOS 执行环境或容器运行时，因此 Unix 专用分支及 Docker 部署运行未验证；不把静态配置检查当作平台运行结果。Windows 未配置真实 ffmpeg，file: 输入路径的实际 FLAC 转码未验证。此次不涉及真实音频设备后端或百万库搜索性能变更，未重复硬件/百万库验收。
