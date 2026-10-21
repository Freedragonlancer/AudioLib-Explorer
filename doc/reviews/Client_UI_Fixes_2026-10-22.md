# 客户端 UI Review 修复核对

日期：2026-10-22。原报告：[Client_UI_Review_2026-10-22.md](Client_UI_Review_2026-10-22.md)。

核对基线为远端 `main` 的 `5130ea3`（0.6.0），不是原报告的 `c5c6ca1`。使用独立工作树，未包含原工作区已有的同步、数据库、服务端和发布流程未提交改动。

| 编号 | 核对结果与处理 |
| --- | --- |
| H1 | 确认。关闭保存失败后保留窗口，提供“保存恢复文件并退出”；排空已请求写入，不再重试失败项，先持久化恢复文件再退出。非法草稿、已删除记录也可导出，写恢复文件失败则继续保留草稿。 |
| H2 | 确认。同库批量标注、导入、修复与移除保留有效目录、展开、页码、勾选及详情；删除的对象才清除。仅影响正在加载的记录时停止播放，已删除的加载 generation 不可再次从 UI 播放。文件夹删除范围完整遍历，读取失败则在删除前返回。 |
| M1 | 确认。明确提示切换音效自动保存、Ctrl S 立即保存，提供当前草稿还原。菜单收藏与详情星标统一修改草稿。远程还原使用服务器最后确认值及草稿 baseline，避免尚未同步的旧快照覆盖新值。 |
| M2 | 确认。备份、导出、校验仍排空 FIFO，但历史写入失败不会阻断这些只读操作；报告明确说明未保存内容未被包含，错误携带记录 ID。会替换库身份的操作继续保护失败草稿。 |
| M3 | 确认。已知离线、认证失败或服务器启动期间禁用标注编辑和新保存；已有草稿可还原或恢复导出。库身份更换使用独立门禁。 |
| M4 | 确认。UI 线程取得 HWND，原生文件/文件夹选择 worker 使用主窗口 owner；遮罩区分选文件与处理中，提供中英文操作标题、过滤器和默认文件名。 |
| M5 | 确认。清除筛选同时恢复存在文件视图；空结果区分缺失文件、筛选无结果和空库。 |
| M6 | 确认。移除确认展示前五项名称/隐藏 ID，以及当前列表之外的勾选数量；修正单复数。源 WAV 保留。 |
| M7 | 确认。播放错误变化时才发布到状态栏，持续错误不会每 65ms 覆盖后来的保存/扫描反馈。 |
| M8 | 确认。每次打开输出设置从已确认 PlaybackConfig 回填驱动、策略、缓冲和声道对，再刷新设备。 |
| M9 | 确认。无全文查询、无标注筛选且排序不依赖 Tags/Modified 时，保存后原地更新行；全文检索、标注筛选、Tags/Modified 排序或尚未完成的结果页仍刷新。未假定 notes/tags 与全文查询无关。 |
| M10 | 确认。移除未实现 WebDAV 占位入口和无控件使用的 UI 属性/回调；不再把 WebDAV 密码解密装入 UI。保留已有 settings.json 配置及播放回退能力。 |
| M11 | 确认。目录树支持方向键、展开和选择；数据源按钮可聚焦；设置、批量、关于、Peaks、Debug 具有初始焦点、Tab 边界和 Esc；列表增加 Home/End、PageUp/PageDown 与 Ctrl+PageUp/PageDown。 |
| L1 | 确认。全部声音高亮及诊断目录作用域使用稳定路径，折叠祖先不再误判。 |
| L2 | 确认。侧栏和菜单收藏采用一致的全库范围。 |
| L3 | 确认。同步间隔/自动同步移至管理窗口公共区域，独立保存、独立草稿与关闭出口，跨服务器选择不丢全局草稿，不重开 provider。 |
| L4 | 最新 main 已按历史类型区分空状态；核实后保留。 |
| L5 | 确认。Pitch/Rate 失焦时应用；非法值回填当前生效设置。 |
| L6 | 最新 main 已实现 Waveform 列。增加会话内显示开关，菜单明确表达列宽、Waveform 与排序，避免暗示已有全部列隐藏功能。 |
| L7 | 确认。摘要与管理页健康统计采用相同的已隐藏远程移除记录扣减口径。 |
| L8 | 确认。补充时长、输出校验/设备描述、拖出取消、新建库、试听、文件选择和启动失败上下文的中英文文案；原始系统/服务器诊断详情保留来源文本。 |
| L9 | 确认。扫描展示保留 owned 状态，切换语言重绘扫描、设备和结构化连接报告，不发起额外网络测试，也不丢连接草稿。 |
| L10 | 风险成立。确认内容改为可滚动区域，长路径不会强迫固定高度弹框溢出。 |
| L11 | 原报告为未实测推断。试听音量控件获焦期间暂停引擎状态反写，覆盖拖动与键盘调整；失焦后恢复。 |

## 验证范围

测试和程序验收仅使用 owned 临时目录，不修改工作库、旧库或原始 WAV。环境为 Windows x64、Rust 1.97.1 MSVC；Cargo 输出放在本机 SSD，未使用网络共享上的 target。

- `cargo fmt --all -- --check`、`cargo clippy -p audiolib-desktop --all-targets --locked -- -D warnings`：通过。
- 桌面全套回归本轮先得到 268 通过、3 失败、91 ignored；修复后，3 个失败项与全部 5 个新 UI 回归复测通过。沿用未受最终 UI 焦点/菜单测试修正影响的通过结果，累计 271 个非 ignored 桌面测试通过；ignored 包括由父测试调用的隔离 helper 以及显式硬件/性能验收入口。
- `cargo test -p audiolib-audio -p audiolib-ipc --all-targets --locked`：音频 82 通过、2 ignored；IPC 1 通过。
- `tools/build-release.ps1`：0.6.0 默认 ASIO 构建通过，SDK 2.3.4 校验通过；`tools/package-portable.ps1 -SkipBuild` 与 `tools/validate-portable.ps1 -BinaryDirectory dist/latest/portable -CleanCheckout` 通过，含包边界/隐私、EXE 身份、首次启动、CLI、中文 UI、启动失败诊断和许可证。
- `validate-review-fixes.ps1 -CleanCheckout`、`validate-library-management.ps1`、`validate-desktop-close.ps1`：通过，后者包含各三轮 WM_CLOSE/SC_CLOSE、重复关闭请求、正常退出和库重开。
- 输出设置、批量标注、服务器公共设置各生成中英文原生软件渲染截图并目视检查，共 6 张；中文主界面也已检查。
- `validate-playback-ipc.ps1 -CleanCheckout`：通过。校正了最新 main 中仍把 CLI 客户端本地播放历史误认作退役主库历史的旧断言，保留并加强 Pause/Resume/Seek 不重复、Stop/Ended 后 Play 各新增会话、源 ID/路径/库身份、独立本地历史、主库不重建退役表及源 WAV 哈希不变的覆盖。实测 WASAPI 96 kHz / 960 帧，静音 synthetic fixture，105 秒 WAV 的工作集增量约 1.1 MiB；未测声学延迟。
- 固定 `local_release/` 安装已更新：1230 个包拥有文件逐项 SHA256 相符（包括两个 EXE），安装后的 `AudioLibCli 0.6.0` 与独立 `AUDIOLIB_DATA_DIR` 下中文桌面启动/正常退出通过。仅覆盖包文件，未删除或复制工作库、设置、SQLite sidecar；恢复正常环境。后续日常启动使用 `local_release/Start-AudioLibExplorer.cmd`。

此构建的 Desktop SHA256 为 `50c5c2966d4b11d989956e2e27af18b415a0867e581a43dd4f5f6081165ccffd`，CLI 为 `d0ead969606e27ae063889489e80abaf94158984d02f31196da24a23247a7d6c`。验证脚本和本文的后续修订未改变应用源码，沿用同一已验证便携包与安装结果。

Metadata 优化的直接证据为真实保存后结果仍正确且 search/facet generation 不增加。现有 `gui_search_benchmark` 打开只读库，仅测搜索输入到 WGC 最终帧，不能用于宣称 Metadata 保存前后耗时收益。此批不宣称百万记录实测达标；项目目标仍是 **P95 latency <500ms for 1M+ records**。

已用 owned 1000-WAV 库尝试一次 `validate-gui-search-performance.ps1 -Repeats 1`。WGC 校准报告 `foreground=false`、`input_desktop=false`，未取得合格的最终帧；保留无效样本并正常关闭该测试窗口，未在条件不变时重试，不报告搜索耗时或 P95。普通 UI 软件渲染和控制器查询/缓存回归已通过。

原生对话框 owner 已编译，真实 Windows 遮挡关系、ASIO 硬件延迟及屏幕阅读器体验不以测试后端结果代替实机测量。Waveform 显示开关本次只作用于当前会话。
