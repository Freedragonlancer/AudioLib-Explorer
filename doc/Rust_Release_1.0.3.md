# 1.0.3 发布记录

程序 1.0.3 | API /api/v1/ | 本地写 schema 8 | 服务端迁移 schema 9 | 快照兼容 3..=9

[v1.0.3](https://github.com/Freedragonlancer/AudioLib-Explorer/releases/tag/v1.0.3) 已正式发布，以 `v1.0.2` 为基线。七份正式归档、十四项线上资产、发布后固定安装与本轮构建缓存维护均已核验通过。

- 发布时间：`2026-11-17 17:49:48`（UTC）；`2026-11-18 01:49:48`（UTC+8）。
- 最终 main 提交：`130a64429aaa826fef2c8acc0c6168eb95bcad90`。
- 标签：`v1.0.3`；annotated tag 对象：`c8079fd74733a18346ae6998b67d618c3aea9894`。
- 初始审查 beta：`bd1225f44e1bfb476f81930d86e61a22981a663b`；修复后源码检查候选：`b83d8349d248c29791b2956d78772c2c95d6b386`。
- 已验收 beta：`d2e423d2153fdc605b5e5c7708d7444c60d8d2c9`。候选至 beta／最终 main 的应用与构建输入相同；最终产物均从上述 main/tag checkout 执行正式构建、打包并验收。

## 范围与兼容

纳入 #321–#325 的系统字体、文字颜色、推荐主题与命名预设，[#326](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/326)–[#327](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/327) 的 AI 设置／Skill 编辑与本地查看文件，以及 [#328](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/328) 的全局设置滚动和关闭图标修正。[#329](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/329) 汇总发布准备、保存失败修复与相关测试修正；beta 全部验收通过后，由 [#330](https://github.com/Freedragonlancer/AudioLib-Explorer/pull/330) 提升至 main。

外观支持系统字体、文字颜色和命名预设，加入黑曜、深海蓝、暖砂琥珀等推荐主题与 OLED 纯黑表面，保留原始默认及工作室。旧 Forest 设置转换为 Obsidian，保留其他偏好。设置窗口固定标题、分类导航和底部操作，内容独立滚动；“应用”保存当前音频／同步草稿后保持打开，关闭继续确认未应用草稿。

新增 `ai/customization.json` 独立原子保存 Skill 文档与搜索行为。供应商、Skill 和搜索行为分开管理，支持三份 Markdown 文档编辑、启用、预览、文件夹导入导出及本地查看；保留已有外部修改。只有已保存且启用的方案参与生成，草稿不发送，文件夹文档不作为脚本执行。既有供应商、加密密钥与兼容提示词保留。旧转发服务器仍按原提示词长度校验，超过 2000 字的新 Skill 需要同步升级服务器；API v1 与 64 KiB 转发请求体上限保留。

资料库格式和完整格式校验沿用 1.0.2，本版不新增资料库迁移。数据库、扫描、Metadata、音频引擎、IPC、更新器及查询性能路径未改；相关通过基线按源码、feature、依赖与平台条件复用，详见 [Rust 验证报告](Rust_Validation.md)。

## 审查与源码验证

独立 review 发现并修复 P2：Skill 文档实际落盘失败后，保存搜索行为或供应商可清空共用错误，导致关闭时遗失失败编辑。新增 customization 操作按 Skill ID、搜索行为、兼容选择和导入分别保留失败；错误在释放保存屏障前记录，只有对应重试成功或明确放弃才解除相应保护。原供应商／兼容提示词错误独立保留。真实 Controller 回归在自有临时目录制造原子保存失败，检查同文件／跨文件无关保存、另一 Skill、无效导入、对应重试与放弃，以及草稿、磁盘内容和关闭屏障。

按实际影响选择 Rust 1.97.1 检查，未追加无关 workspace 或百万条性能套件。下表统计不同父测试，ignored 子进程辅助入口不计为通过，也不跨平台累加总数。

| 平台与配置 | 实际源码检查及复用范围 |
| --- | --- |
| Windows x64／MSVC，已核验 ASIO SDK 2.3.4 | 格式与受影响目标 strict Clippy 通过；39 个不同 AI 控制器父测试、1 项外观交互测试有通过收据，39 个辅助子入口为 ignored。组合保留原通过与修正后通过身份。 |
| macOS arm64／原生客户端配置 | 受影响 core／AI／desktop／remote／server strict Clippy 与格式检查通过。候选 AI 库 59、desktop 去重 47、remote relay 9、server relay 9，共 124 个不同父测试通过，39 个辅助入口 ignored。desktop 的 AI 与 Skill 过滤结果有 4 项重叠，已去重。 |
| macOS arm64／较早同输入基线 | 初始审查身份的 core config 44、appearance 2，共 46 项通过；对应输入未变，单列复用，不算入上述候选 124 项。 |
| Linux arm64／server no-default-features | 配置 44、外观 2、AI customization/storage/relay 17、remote relay 9、server relay 9，共 81 passed／0 ignored；受影响 core／AI／remote／server strict Clippy 通过。 |
| Linux x64／ARM64 OrbStack 仿真 | 同一受影响依赖闭包 strict Clippy 通过；功能测试为 not-requested，按平台无关输入复用 arm64 与 Windows 证据，不声称 x64 执行 81 项。最终 x64 包及 Docker amd64 仍分别完成实际运行验收。 |

保留原失败及修正证据：Windows 首轮 AI 为 38 passed／1 failed／39 ignored，失败来自零命中剪枝测试早于目录模型完整发布取值；四处等待补充既有完整发布条件，数量、顺序和超时断言保持原样，相关修正用例与静态检查通过。更早测试访问私有字段的编译失败改为使用公开 ProfileStore 后通过。Mac 原外观测试因整页跳到底部而找不到中间控件；测试改用有界真实滚轮寻找控件，保留点击、主题、重叠、对比度和预设可达性断言。误选 remote 测试过滤器导致零匹配的失败也保留，改正过滤器后 9 项通过。

中英文 100%／200%、收起／展开共 8 组软件渲染外观情境通过。本机字体枚举与系统 locale 只读探测通过。另有独立实验在 1080×700、200% 软件渲染窗口中发送实际水平滚轮事件，分别揭示右侧 Amber 卡片与高级预设控件，完成主题点击、预设名称输入和保存动作；两端截图的裁切对应水平滚动位置，控件能够到达。该实验单列，不加入正式源码父测试计数，也不等同于实际显示器或真实音频硬件验收。

## Beta 验收

对冻结 beta `d2e423d2153fdc605b5e5c7708d7444c60d8d2c9` 使用正式刷新流程，复用现有 beta 配置和资料库，未请求 `--with-db`。健康检查返回版本 1.0.3 与该提交；配置根目录探测 1/1 通过。客户端候选的构建、打包与 beta 安装 EXE 哈希分别核对一致，便携验收及真实 Windows `WM_CLOSE`／`SC_CLOSE` 三轮关闭检查通过。

官方客户端 beta smoke 为 **10 passed／0 failed**：完整同步 **909,342 条记录**，快照 **10,150.7 MiB**，同步 **144.9 秒**；随机根内流式请求 **20/20** 通过。

资料库保留了既有 Metadata，其中 **828,780 条记录不在当前配置的流式播放根目录内**，脚本据此报告警告；这些记录按接口契约不能通过 `/stream` 播放。随机取样时跳过 **154 条根外记录**后选取 20 条根内记录。此结果证明配置根内样本流与完整快照同步通过，不表示全部库记录的源文件当前均可播放，也没有为消除警告删除记录。

## 最终产物与安装

七份正式归档均由最终 main/tag 构建、打包并完成对应本地包验收，依赖许可证预检选择实际 target 与 feature。Windows 客户端完成便携验收；Windows／macOS／Linux 服务端完成各自包启动验收；Mac 客户端使用 `package-macos-client.ps1 -ValidateRuntime` 一次完成结构与原生运行验收；Docker 两种架构分别完成离线镜像运行验收。Linux arm64 原生服务端包是 Docker arm64 的内部输入，不作为额外公开归档。

公开资产合同为七份归档及各自原始 `.sha256`，共 **14 项、597,612,727 字节**。下表为已验收字节身份；实际线上资产的精确名称、上传状态、大小、全部提供的 SHA256、标签对象及源码提交、发布说明与 latest 身份均核验通过；七份下载校验文件的原始字节与正文均一致，未因已有可信归档摘要而重复下载大归档。

| 归档 | 字节数 | SHA256 |
| --- | ---: | --- |
| `AudioLibExplorer-v1.0.3-win-x64-portable.zip` | 47,814,988 | `15e9d48b014f3b0b8099e3a9e38e48d415a3cfac3ec599e7161cc629ee489d33` |
| `AudioLibExplorer-v1.0.3-macos-arm64.zip` | 37,557,053 | `1244fb762cfec398eb2d3454a6194129b75dae5ef6a179d5b6bcb5063638979f` |
| `AudioLibServer-v1.0.3-macos-arm64.tar.gz` | 15,845,352 | `09c56e8bff9501e3ca1ae741721acf3906aa3b5d1cdb7cf7dfb956e0bd0ea12a` |
| `AudioLibServer-v1.0.3-linux-x64.tar.gz` | 17,175,700 | `a37cc893d5d2a2e5a260d43dfd15f68201be4ba2c6ac44109925fdcdac987d9e` |
| `AudioLibServer-v1.0.3-docker-linux-amd64.tar.gz` | 240,163,882 | `9c5327458fd3c11bcd44e71332018711f280c51d9fc9ce767e92bf6a3e9982da` |
| `AudioLibServer-v1.0.3-docker-linux-arm64.tar.gz` | 220,899,085 | `e96814c8ffcc523f6fa88d8ac922c3562aba22775813aeaf048ab178edaee300` |
| `AudioLibServer-v1.0.3-win-x64.zip` | 18,155,909 | `6a2adae8f1307aa0f8d5a2f50f9718545abab2805bbf9c72e0bff98c98af3c1f` |

发布前 Windows 固定 `local_release/` 安装完成：908 个包属文件验证，复制 19 个变化文件，CLI 版本 1.0.3 与隔离桌面启动通过；工作库、配置与运行数据保留原位置。发布后同一固定安装与线上已发布字节的同步核对：908 个包属文件、两个 EXE 与已发布 ZIP 内 EXE 身份一致，CLI 1.0.3 及隔离启动通过，copied=0；完整 portable 验收未重复执行，正常工作数据保留。

## 清理与验收边界

所有相关 Rust／打包消费者结束后，对本轮已验证归属的 target 各执行一次 Intermediate／零中间缓存预算维护，保留最终 profile-root 二进制、发布包、源码、SDK、Cargo／Rustup 及工作数据。以下为逻辑字节估算，不作为整机物理空间或全盘预算声明。

| 范围 | 逻辑移除字节 | 保留字节 | 结果 |
| --- | ---: | ---: | --- |
| 本轮 macOS target | 58,838,794,876 | 240,032,111 | 通过；四个最终二进制哈希不变。 |
| 本轮 Linux arm64 与 amd64 target 合计 | 12,414,356,421 | 678,603,122 | 两个 target 分别通过；最终 server／migrator 与已验收包一致。 |
| 本轮 Windows target | 33,200,047,919 | 198,453,328 | 单次 Intermediate/0 通过；四个最终 EXE 与已验收包哈希一致。 |

macOS 使用 ad-hoc 签名，未做 Apple 公证。Linux x64 与 Docker amd64 在 ARM64 OrbStack 上仿真；arm64 在 ARM64 Linux VM 运行。本轮未执行真实付费供应商 API、实际 ASIO 硬件、声学起音延迟或新的百万条性能基准。AI 网络行为使用确定性夹具；ASIO 发布配置的编译与打包不等同于驱动硬件验收。性能目标仍为 **P95 latency <500ms for 1M+ records**。

本文与后续文档提交保留上述标签、最终源码和已验收产物的字节身份；本次文档闭环不触发重建、重新安装或再次缓存扫描。
