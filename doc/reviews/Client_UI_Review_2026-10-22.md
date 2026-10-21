# 客户端代码 / UI / 交互 Review 报告

> 后续核对与修复记录见 [Client_UI_Fixes_2026-10-22.md](Client_UI_Fixes_2026-10-22.md)。下文保留原始审阅基线与发现，部分事项在最新 main 已有变化。

- 日期：2026-10-22
- 基线：分支 `codex/debug-snapshot-reports`，提交 `c5c6ca1`（工作区干净）
- 范围：`app/audiolib-desktop`（Rust 控制器 + Slint UI）为主，按需查看 `crates/audiolib-audio`、`crates/audiolib-ipc`、`crates/audiolib-core` 中被客户端直接依赖的部分。服务端 `server/audiolib-server` 未做专项审阅：下文涉及服务器模式的问题，根因和修复点都在客户端。
- 方法：通读 `app-window.slint`、`audition-panel.slint`、`theme.slint`、`main.rs`、`controller.rs`（非测试部分）、`startup.rs`、`dialogs.rs`、`diagnostics.rs`、`metadata_recovery.rs`、`window_repaint.rs`，以及 `remote_mode.rs`、`source_manager.rs` 的交互主路径；静态推理为主，未做实机点按。
- 验证：见文末“验证记录”。

## 总体评价

客户端整体工程质量很高：所有数据库、网络、解码工作都在 UI 线程之外执行；每类异步结果都带 generation，过期结果会被丢弃；Metadata 写入是有序 FIFO 加 barrier；关闭时会先排空写入和扫描；诊断报告有明确的隐私边界；测试覆盖面很广（UI、关闭、切换数据源、恢复等）。

主要问题不在并发正确性，而在 **“数据安全优先”的策略缺少用户出口**，以及若干 **操作后浏览上下文丢失、提示与实际不一致** 的交互问题。下表按严重度排序。

## 问题汇总

| 编号 | 严重度 | 类别 | 摘要 |
|---|---|---|---|
| H1 | 高 | 交互 / 数据安全 | 草稿写入持续失败时无法正常退出，也没有任何“放弃草稿”入口 |
| H2 | 高 | 交互 | 批量标注、移除、修复引用、导入标注后，目录、展开状态、页码、选中项和详情面板全部重置，并且会停止播放 |
| M1 | 中 | 交互 | 切换选中项会静默保存草稿；菜单里的“收藏”会顺带提交未保存的标签和备注 |
| M2 | 中 | 正确性 | 任意一条历史写入失败，会阻塞之后所有的库操作（备份、导出、校验、切换数据源） |
| M3 | 中 | 交互 / 文案 | 服务器离线提示写着“只读”，但标注仍可编辑，保存必然失败（进而触发 H1） |
| M4 | 中 | 交互 | 原生文件对话框没有 owner 窗口，可能被主窗口遮住；遮罩文案还写着“正在处理” |
| M5 | 中 | 交互 | 空结果页的“清除搜索和筛选”不重置“文件状态”，在缺失视图里点了仍是空 |
| M6 | 中 | 交互 / 数据安全 | 勾选集合跨搜索、跨页保留，“移除选择记录”会移除当前看不到的记录，确认框只显示数量 |
| M7 | 中 | 交互 | 播放引擎的错误每 65ms 覆盖一次状态栏，其它反馈信息看不到 |
| M8 | 中 | 交互 | 音频输出设置点“关闭”不会回滚未应用的修改，再次打开显示的不是实际生效的配置 |
| M9 | 中 | 性能 | 每次保存 Metadata 都会触发完整重搜，并作废目录 facet 缓存 |
| M10 | 中 | UI 约定 | 未实现的 “WebDAV 设置（功能未开发）” 入口仍然显示，相关 UI 属性是死代码，WebDAV 密码被载入一个不显示的 UI 属性 |
| M11 | 中 | 无障碍 | 目录树无法用键盘操作；多个模态框没有 Tab 焦点约束；顶部数据源标签只能用鼠标点击 |
| L1–L11 | 低 | 打磨 | 见“低优先级问题” |

---

## 高优先级

### H1 草稿写入持续失败时无法正常退出，且没有“放弃草稿”入口

**现象 / 触发场景**

1. 服务器模式下服务器离线（或网络断开），用户编辑了标签或备注（切换选中项时会自动提交，见 M1）。
2. 本地模式下，某条记录被 CLI 或另一流程移除后，GUI 仍持有该记录的草稿。
3. 在以上任一状态下关闭窗口或按 Ctrl+Q：
   - `request_close` 调用 `enqueue_transition_metadata` → `enqueue_all`，把所有草稿重新入队（[controller.rs:3928](../../app/audiolib-desktop/src/controller.rs#L3928)）；
   - `wait_transition_metadata` 在非“服务器库已更换”的情况下走 `wait_metadata(sequence, true)`，只要 `failures` 非空就返回错误（[metadata_recovery.rs:31](../../app/audiolib-desktop/src/metadata_recovery.rs#L31)、[controller.rs:3893](../../app/audiolib-desktop/src/controller.rs#L3893)）；
   - `finish_close` 收到错误后恢复窗口，“保留草稿”（[controller.rs:4047](../../app/audiolib-desktop/src/controller.rs#L4047)）。
4. 再次关闭会重复同样的流程（每次都要等网络超时）。整个 UI 中没有放弃草稿的入口（全仓库搜索 discard，只有服务器连接页的“放弃修改”），用户只能结束进程。

**影响**：正常退出被永久阻塞；用户只能强杀进程，反而丢失内存中的全部状态，而且日志里只看到 “close failed”。

**建议**

- 关闭失败时弹出明确的选择：「重试保存」/「把未保存标注导出到恢复文件后退出」/「放弃并退出」。`metadata_recovery.rs` 已有写恢复文件的成熟实现，可以推广到所有“持续失败”的草稿，而不仅限于服务器库更换的场景。
- 在详情面板为当前草稿提供「还原」按钮（参见 M1）。
- 失败信息需要指出是哪条记录、因为什么失败，而不是只显示一条原始错误字符串。

### H2 结构性操作后浏览上下文被整体重置，并且会停止播放

**现象**：`LibraryOperation::Batch / RemoveAudio / RemoveFolder / Relink / Import` 都属于 `changes_records()`：

- 执行前强制 `AudioCommand::Stop`（[controller.rs:1717](../../app/audiolib-desktop/src/controller.rs#L1717)）。只是批量打标签，也会把正在试听的声音停掉。
- 完成后调用 `reset_library_view(ui, false, false)`（[controller.rs:1899](../../app/audiolib-desktop/src/controller.rs#L1899)）。`retain_navigation=false` 分支会执行 `TreeState::default()`，并设置 `folder_id = None`、`offset = 0`（[controller.rs:2007](../../app/audiolib-desktop/src/controller.rs#L2007)），同时清空选中项、详情面板和勾选集合。

**用户视角**：在某个子目录的第 3 页勾选 20 条做批量标注，完成后视图跳回全库第 1 页，目录树全部折叠，详情面板清空，正在试听的声音也停了。“修复引用”是逐条进行的工作流（缺失视图 → 选中 → 选择新位置），每修一条都会丢一次目录上下文。

**建议**：本地、同一个库内的 Batch / Relink / Remove 不会使 ID 失效，可以复用 `retain_navigation = true` 的路径（保留目录、展开状态和页码，只清除派生行，然后按 ID 重新定位选中项）。Stop 只在被移除或被重新关联的记录正在播放时才需要执行。现有测试 `library_management_drains_drafts_before_batch_switch_and_close` 只断言了数据正确，可以补一个“批量标注后目录和选中项保留”的交互回归测试。

---

## 中优先级

### M1 草稿被隐式提交，没有还原入口

- `select_id_guarded` 在发起新的选中前先 `enqueue_current_metadata`，即**切换行就会保存当前草稿**（[controller.rs:3400](../../app/audiolib-desktop/src/controller.rs#L3400)，加载完成时还会再提交一次：[controller.rs:3538](../../app/audiolib-desktop/src/controller.rs#L3538)）。界面上却有“未保存”标签和显式的“保存 · Ctrl S”按钮，暗示不保存就不会写入。
- 编辑菜单 / 右键菜单里的“添加到收藏”作用于当前文件时，会把整个详情草稿（含未保存的标签和备注）一起保存（[controller.rs:4929](../../app/audiolib-desktop/src/controller.rs#L4929)）；而详情面板里的 ★ 按钮只把状态标成“未保存”（[app-window.slint:1604](../../app/audiolib-desktop/ui/app-window.slint#L1604)）。同一个动作有两种语义。

**建议**：二选一并在 UI 中表达清楚：要么明确采用“自动保存”（去掉“未保存/保存”的暗示，改为“已自动保存”状态，并提供撤销），要么切换前提示“保存 / 放弃 / 取消”。无论哪种都需要「还原」按钮。

### M2 一次写入失败会阻塞所有后续库操作

`MetadataState::failure()` 返回 `failures` 中任意一条（[controller.rs:494](../../app/audiolib-desktop/src/controller.rs#L494)），而 `run_library_operation` 对所有操作都调用 `wait_metadata(barrier, true)`（[controller.rs:1703](../../app/audiolib-desktop/src/controller.rs#L1703)）。因此只要有一条与当前操作无关的记录写失败（例如服务器离线时自动提交的草稿），备份、导出 CSV/JSONL、FTS5 校验、切换数据源都会以这条错误失败，用户很难把两者联系起来。

**建议**：只读类操作（Backup / Check / Export）不必要求写入全部成功；或者至少在报错中说明“记录 X 的标注保存失败，请先处理”，并提供跳转到该记录的入口。

### M3 服务器离线时的“只读”提示与实际行为不一致

离线提示为“服务器离线：继续使用本地缓存（只读）”（[remote_mode.rs:434](../../app/audiolib-desktop/src/remote_mode.rs#L434)），但 `refuse_remote_write` 明确不拦截标注编辑（[remote_mode.rs:643](../../app/audiolib-desktop/src/remote_mode.rs#L643)），`remote-writes-blocked` 也只在库身份更换时置位。用户按提示理解为只读，实际仍能编辑，保存失败后进入 H1 的状态。

**建议**：离线期间禁用标注编辑控件（复用 `remote-writes-blocked` 或增加 `remote-offline` 状态），或者实现离线写队列；文案与行为保持一致。

### M4 原生对话框没有 owner，遮罩文案误导

- `choose_folder` 调用 `IFileDialog::Show(NULL)`（[dialogs.rs:135](../../app/audiolib-desktop/src/dialogs.rs#L135)），`choose` 的 `OPENFILENAMEW.owner` 保持为零（[dialogs.rs:242](../../app/audiolib-desktop/src/dialogs.rs#L242)）。对话框不是主窗口的模态子窗口，点击主窗口后它可能被挡到后面。
- 选择备份、恢复、导出、导入或修复引用的文件时，主窗口遮罩显示“正在处理音效库操作… 请稍候”（[app-window.slint:2525](../../app/audiolib-desktop/ui/app-window.slint#L2525)），只有扫描文件夹选择用了“请在文件夹选择窗口中选择…”的文案。用户会以为程序卡住了。
- 对话框标题只有泛化的“保存文件 / 选择文件”，过滤器字符串只有英文（[dialogs.rs:218](../../app/audiolib-desktop/src/dialogs.rs#L218)）；备份也没有默认文件名。

**建议**：通过 `audiolib-windows` 取主窗口 HWND 传给对话框（线程模型不变，只设置 owner）；遮罩统一使用“请在弹出的文件窗口中完成选择”这类文案；按操作给出具体标题和默认文件名（如 `AudioLib-backup-2026-10-22.db`）。

### M5 “清除搜索和筛选”在缺失视图中无效

“音效库 → 查看缺失文件”会把 `visibility-index` 设为 1（[app-window.slint:1004](../../app/audiolib-desktop/ui/app-window.slint#L1004)）。没有缺失文件时出现空状态，按钮“清除搜索和筛选”（[app-window.slint:1573](../../app/audiolib-desktop/ui/app-window.slint#L1573)）调用的 `reset_filters` 不重置 `visibility_index`（[controller.rs:922](../../app/audiolib-desktop/src/controller.rs#L922)），点击后仍是空，提示语也只说“添加 WAV 文件夹”。

**建议**：`reset_filters` 一并把文件状态恢复为“存在的文件”（或在空状态里单独显示“当前仅显示缺失文件 · 显示全部”）；空状态文案区分“库为空”“筛选无结果”“没有缺失文件”三种情况。

### M6 隐藏勾选项可被批量移除

`multi_selection` 按 ID 跨搜索、跨页保留（搜索结果发布时只做标记：[controller.rs:2529](../../app/audiolib-desktop/src/controller.rs#L2529)）；普通单击也会把当前行写入勾选集合。之后换了搜索条件，`library_action(11)` 会对整个集合执行移除（[controller.rs:1437](../../app/audiolib-desktop/src/controller.rs#L1437)），确认文本只有 “Remove {} records…”（[controller.rs:180](../../app/audiolib-desktop/src/controller.rs#L180)），既不列名称，也不提示其中有当前不可见的记录。WAV 不会被删，但这些记录的标注会丢失。

**建议**：确认框列出前若干个文件名，并标注“其中 N 条不在当前列表中”；或者在搜索条件变化时清空勾选（或给出提示）；英文复数也需要处理（“1 records”）。

### M7 播放错误持续覆盖状态栏

状态 worker 每 65ms 运行一次，只要 `status.error` 存在就 `ui.set_status(error)`（[controller.rs:2921](../../app/audiolib-desktop/src/controller.rs#L2921)）。引擎中 Validation 错误、设备丢失等错误会一直保留到下一次成功的命令（例如 [engine.rs:1360](../../crates/audiolib-audio/src/engine.rs#L1360)）。在此期间“Metadata 已保存”“扫描完成”这类反馈刚显示就被覆盖。

**建议**：只在错误变化时发布（与 `last_audition` 的做法相同），或把播放错误放到试听面板自己的提示位，状态栏只显示一次。

### M8 音频输出设置“关闭”不回滚

对话框的“关闭”只把 `settings-visible` 设为 false（[app-window.slint:1845](../../app/audiolib-desktop/ui/app-window.slint#L1845)）；打开对话框时也只刷新设备列表（[app-window.slint:1190](../../app/audiolib-desktop/ui/app-window.slint#L1190)），不会把驱动、缓冲、声道对等字段重置为当前生效值。改了字段但不点“应用”就关闭，下次打开看到的是未生效的值，容易误以为已经生效。

**建议**：打开时从 `playback` 配置回填所有字段；或把按钮改为“取消”并在关闭时回滚。

### M9 每次保存都会重搜并作废 facet 缓存

Metadata worker 每次写入成功后调用 `request_search(&ui, true, false)`（[controller.rs:2452](../../app/audiolib-desktop/src/controller.rs#L2452)），而 `request_search` 会让 `facet_revision` 自增（[controller.rs:2934](../../app/audiolib-desktop/src/controller.rs#L2934)），`FolderFacets::matches` 因此必定失配，目录计数需要重算。在百万级库里，每切换一次收藏或保存一次标签，都会付出一次结果页查询加一次目录 facet 查询的成本，并伴随状态栏闪动“正在搜索… / 正在更新目录…”。

**建议**：标注写入只影响 tags/favorite/notes：先在本地按 ID 原地更新当前页的行（`publish_audio_rows` 已支持原地更新）；只有当排序依赖标签、或存在 favorites/tagged/tag/color 筛选时才重搜，并且不 bump facet revision（目录计数不受标注影响，除非存在标注类筛选）。具体收益需要用 `gui_search_benchmark` 实测。

### M10 未实现入口与死代码

- 连接页显示“WebDAV 设置（功能未开发）”和一个禁用的输入框（[app-window.slint:2204](../../app/audiolib-desktop/ui/app-window.slint#L2204)），与 AGENTS.md “hide unimplemented entry points”的约定冲突（[AGENTS.md:124](../../AGENTS.md#L124)）。实际上 WebDAV 回退路径已部分实现（可通过 settings.json 配置），“未开发”这个说法也不准确。
- `ds-webdav-user / ds-webdav-password / ds-webdav-base / ds-roots / data-source-root-edited` 在 Slint 中声明了，但没有任何控件使用（[app-window.slint:645](../../app/audiolib-desktop/ui/app-window.slint#L645)、[app-window.slint:682](../../app/audiolib-desktop/ui/app-window.slint#L682)），Rust 侧仍在设置这些属性，`load_form` 还会把解密后的 WebDAV 密码写入 UI 属性（[source_manager.rs:478](../../app/audiolib-desktop/src/source_manager.rs#L478)），关闭管理窗口时也只清空了 token。

**建议**：隐藏 WebDAV 行，或做成真正可用的配置；删除死属性，或至少不要把密码载入 UI。

### M11 键盘操作与无障碍

- 目录树行只有 `TouchArea`，没有 FocusScope 或 accessible-role，无法用键盘浏览、展开或选中（[app-window.slint:1293](../../app/audiolib-desktop/ui/app-window.slint#L1293)）。
- 音频输出设置、批量标注、关于、Spectral Peaks、Debug 等模态框没有 Tab 约束：`background-input` 对这些对话框的 Tab 直接 `reject`（[app-window.slint:1222](../../app/audiolib-desktop/ui/app-window.slint#L1222)），焦点会走到遮罩后面的控件上（按键会被吞掉，但焦点框会“消失”）；其中设置、批量、关于、Peaks 打开时也没有初始焦点（`focus-library-modal` 只处理 Debug、远程目录、确认框、扫描、库管理：[app-window.slint:972](../../app/audiolib-desktop/ui/app-window.slint#L972)）。
- 顶部数据源标签是可点击区域，但不可聚焦，也没有 accessible-role（[app-window.slint:1244](../../app/audiolib-desktop/ui/app-window.slint#L1244)）。
- 结果列表缺少 PageUp/PageDown/Home/End 和翻页快捷键。

**建议**：抽象一个通用的 `ModalFrame`（初始焦点、Tab 循环、Esc），各对话框复用；为目录树加入方向键导航（上/下选择，左/右折叠/展开）。

---

## 低优先级问题

- **L1 “全部声音”高亮错误**：选中目录的父级被折叠后，`selected_folder` 变成 -1（[controller.rs:3284](../../app/audiolib-desktop/src/controller.rs#L3284)），侧栏“全部声音”按 `selected-folder < 0` 亮起（[app-window.slint:1286](../../app/audiolib-desktop/ui/app-window.slint#L1286)），但搜索仍限定在该目录内。建议改用 `selected-folder-path != ""` 或 crumbs 判断。诊断快照中的 `folder_scoped` 也有同样的问题（[diagnostics.rs:246](../../app/audiolib-desktop/src/diagnostics.rs#L246)）。
- **L2 “我的收藏”语义不一致**：侧栏按钮在当前目录内切换（[app-window.slint:1287](../../app/audiolib-desktop/ui/app-window.slint#L1287)），视图菜单的“我的收藏”会清掉目录（[app-window.slint:1175](../../app/audiolib-desktop/ui/app-window.slint#L1175)）。
- **L3 全局设置放在单个服务器的页面上**：“版本检查间隔 / 有新版本时自动同步”作用于所有服务器，却放在某个服务器配置的连接页，修改后会把该配置标记为未保存（[app-window.slint:2188](../../app/audiolib-desktop/ui/app-window.slint#L2188)）。对当前激活的配置来说，这还会走一遍较重的 source change 流程（[source_manager.rs:863](../../app/audiolib-desktop/src/source_manager.rs#L863)）。建议移到“服务器设置”的公共区域。
- **L4 历史空状态文案**：“最近播放”为空时也提示“扫描文件夹后可在这里查看结果”（[app-window.slint:2104](../../app/audiolib-desktop/ui/app-window.slint#L2104)）。
- **L5 Pitch / Rate 只在回车时生效**：输入后点别处，值会显示但不生效；点击 Mono、Repeat 或重置时又会顺带应用这些未确认的文本（[audition-panel.slint:107](../../app/audiolib-desktop/ui/audition-panel.slint#L107)）。建议失焦时应用或回滚。
- **L6 “列 / Customize table columns” 名不副实**：菜单里只有重置列宽和排序（[app-window.slint:1702](../../app/audiolib-desktop/ui/app-window.slint#L1702)），不能隐藏列（包括 144px 的波形列；`list-waveforms-enabled` 在 Rust 中从未被设置）。
- **L7 计数口径不一致**：标题栏摘要扣除了服务器端已移除但被隐藏的记录（[controller.rs:2096](../../app/audiolib-desktop/src/controller.rs#L2096)），管理页 `library_health` 没有扣除（[controller.rs:2111](../../app/audiolib-desktop/src/controller.rs#L2111)）。
- **L8 国际化缺口**（AGENTS 要求中英文运行时切换）：
  - 时长筛选错误 “Duration must be a number in seconds”（[controller.rs:3082](../../app/audiolib-desktop/src/controller.rs#L3082)）；
  - 输出设置的全部校验提示和设备描述（[controller.rs:4526](../../app/audiolib-desktop/src/controller.rs#L4526)、[controller.rs:4559](../../app/audiolib-desktop/src/controller.rs#L4559)）、“No available devices / Unavailable”（[controller.rs:4472](../../app/audiolib-desktop/src/controller.rs#L4472)）；
  - 拖出取消（[controller.rs:5018](../../app/audiolib-desktop/src/controller.rs#L5018)）、新建库报错（[controller.rs:1819](../../app/audiolib-desktop/src/controller.rs#L1819)）；
  - 试听面板的 “Pitch / Rate / st / Mono” 和 accessible-label “Audition time”（[audition-panel.slint:106](../../app/audiolib-desktop/ui/audition-panel.slint#L106)、[audition-panel.slint:164](../../app/audiolib-desktop/ui/audition-panel.slint#L164)）；
  - 文件对话框的过滤器；启动失败弹窗。
- **L9 语言切换不刷新已生成文本**：`scan_summary`、`device_info`、`ds_report` 等由 Rust 拼接的文本在切换语言后保持旧语言，直到下一次刷新（[controller.rs:1233](../../app/audiolib-desktop/src/controller.rs#L1233)）。
- **L10 确认框**：固定高度 270px（[app-window.slint:2509](../../app/audiolib-desktop/ui/app-window.slint#L2509)），恢复或导入时会显示完整路径，较长的无空格路径可能溢出（需实机确认）。
- **L11 音量滑块与状态回写**：状态 worker 每 65ms 回写一次 `volume-db`（[controller.rs:2913](../../app/audiolib-desktop/src/controller.rs#L2913)），拖动时引擎状态如果滞后一拍，滑块可能回跳（未实机验证，建议拖动期间暂停回写）。

---

## 值得保留的设计（供后续改动参照）

- generation / cancel token 贯穿搜索、目录、选中、波形、历史和远程请求，过期发布一律丢弃。
- `publish_audio_rows` 在 ID 序列不变时原地更新，保留滚动位置和手势（[controller.rs:5357](../../app/audiolib-desktop/src/controller.rs#L5357)）。
- Metadata FIFO + barrier：选中、库操作和关闭都与写入顺序严格对齐；服务器库更换时会写入可恢复的本地恢复文件。
- 诊断报告只记录固定词汇和计数，不记录查询原文、路径或凭据，GitHub 链接有长度限制和来源白名单（[diagnostics.rs:367](../../app/audiolib-desktop/src/diagnostics.rs#L367)）。
- Slint 侧对菜单和模态框的互斥（`menu-ready` / `background-blocked`）做得很系统。

## 建议的处理顺序

1. H1 + M1 + M3：一起设计“草稿生命周期”（自动保存或显式保存、还原、离线策略、失败后的退出出口），这是对用户影响最大的一组问题。
2. H2：结构性操作后保留浏览上下文，并去掉不必要的 Stop。
3. M2、M6、M7、M8：改动小、收益直接。
4. M4、M5、M10、M11 和低优先级问题：作为 UI 打磨批次处理。
5. M9：用 `gui_search_benchmark` 先量化，再决定优化范围。

## 验证记录

- 本次 review 只阅读代码，没有修改任何源文件。
- `cargo clippy -p audiolib-desktop --all-targets --locked -- -D warnings`：通过（0 个 error / warning）。环境：`RUSTUP_TOOLCHAIN=1.97.1-x86_64-pc-windows-msvc`（仅当前 shell），`CARGO_TARGET_DIR` 指向本地临时目录，因为仓库 `target\` 位于网络共享上，写入时报 os error 5。
- 未运行功能测试、未实机点按 UI；标注了“需实机确认”的条目是静态推断。
