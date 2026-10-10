# 架构

本文规定实现机制。可见行为以 [product.md](product.md) 为准。模块路径如下，分成已经接入和尚未接入。

## 模块路径

### 已经接入

| 路径 | 现状 |
| --- | --- |
| `crates/core` | 包名 `lanwork-core`。模型、服务、存储放在这个 crate。不依赖 Slint，也不依赖 Win32 窗口 API。已接入 `search`：`classify_prefix`（`search/prefix.rs`，搜索条整段输入的前缀分类）和匹配引擎（应用、待办、便签共用）。已接入 `parse_todo_due_prefix`（`crates/core/src/capture.rs`）：待办收集剩余文本的日期前缀解析，今天的日期由调用方传入。存储模块 `storage`（`lanwork_core::storage`）已接入，见「数据」。便签服务 `notes`（`lanwork_core::notes`）已接入，见「数据」。待办服务 `todos`（`crates/core/src/todos`，`lanwork_core::todos`）已接入：清单与条目、收件箱、周期生成、软删除与恢复、当前标记、处理模式、跨清单移动，以及 `movedAt`、`currentSince` 的加载修复。薄命令是 `TodoCommands`。永久删除只删除回收站里的条目。删除前把 id 写入 `todo-purge-pending.json`，清单写入成功后发出 `TodoNotice::Purged`。进入回收站满 30 天（30×86400000 毫秒）时，启动加载和 `purge_expired` 清除。原清单已删除时恢复到系统收件箱，没有收件箱则创建一次；这个收件箱就是默认清单。每月重复的日是 31、目标月没有 31 日时落到该月最后一天。锚点日写在周期的 `monthDay`。旧记录没有该字段时，用当前到期日的日子。下一次仍按这个日子，所以 1 月 31 日完成后是 2 月的最后一天，再完成回到 3 月 31 日。29 日或 30 日在目标月不存在时仍不猜测。在重复截止日当天完成不再生成下一次。收纳服务 `shelves`（`crates/core/src/shelves`，`lanwork_core::shelves`）已接入：分组、路径引用、去重、待办关联和存在性检查。薄命令是 `ShelfCommands`。已有分组但未指定目标、再次关联另一条待办、关联到尚不存在的待办 id，仍见「收纳」。GitHub 服务 `github`（`lanwork_core::github`）已接入，见「GitHub 服务」。薄命令是 `GithubCommands`。追踪仓库的添加与移除已实现，见「GitHub 服务」。筛选的并集已写入产品规格；快照没有作者、审查和 CI，服务仍不计算这些筛选。应用索引 `apps`（`lanwork_core::apps`）已接入，见「搜索」。配置 `config`（`lanwork_core::config`）已接入，见「数据」。外壳里不调用 Win32 的决定在 `shell`（`lanwork_core::shell`），见「程序外壳」。文件索引 `files`（`lanwork_core::files`）已接入，见「搜索」。查询调度在 `lanwork_core::dispatch`，见「搜索」。备份与导入 `backup`（`lanwork_core::backup`）已接入，见「数据」。 |
| `crates/app` | 包名 `lanwork`，产物 `lanwork.exe`。依赖 `lanwork-core` 和 Slint。程序外壳在这里：单实例、托盘、全局热键、主题、开机启动，以及启动时创建并保持隐藏的搜索条和面板宿主。搜索条已有界面并调用查询调度，见「程序外壳」的「搜索条」。面板的可见内容还没有。 |
| `spikes/hello` | 技术验证目录里的示例程序。不在 `lanwork` 的依赖里，不进发布包。运行命令写在 `spikes/README.md`。 |
| `spikes/fileidx` | Everything SDK 与 Windows Search 文件名查询的技术验证程序。不在 `lanwork` 的依赖里，不进发布包。记录在 `docs/measurements/fileidx.md`。 |
| `spikes/render` | 渲染器与背景的测量程序，包名 `lanwork-render-spike`。不在 `lanwork` 的依赖里，不进发布包。四种渲染路径分开编译。运行命令写在 `spikes/README.md`。产品渲染器见「运行时」。 |
| `spikes/hotcorner` | 热角技术验证。方案 A 是独立线程上的 `WH_MOUSE_LL`，方案 B 是 50ms 或 100ms 的 `GetCursorPos`。不依赖 Slint，不进 `lanwork` 的依赖。运行命令写在 `spikes/README.md`。产品用 50ms 轮询，见「运行时」。记录见 `docs/measurements/2026-10-08-hotcorner.md`。 |
| `spikes/toast` | 通知技术验证的最小程序。只验证注册、显示、点击参数和模拟面板定位，不进发布包，也不被 `lanwork` 依赖。运行命令写在 `spikes/README.md`。验证记录没有全部通过之前，待办界面不调用它。 |
| `spikes/dnd` | 外部拖放验证程序，包名 `dnd`。不在 `lanwork` 的依赖里，不进发布包。运行命令写在 `spikes/README.md`。 |
| `docs/measurements/2026-10-08-hotcorner.md` | 热角 spike 的实机记录。不是「性能测量」协议的验收。 |
| `tools/fixture` | 测量夹具 `lanwork-fixture`。在显式给出的目录里生成「性能测量」的固定数据。不读 `LANWORK_DATA_DIR`，也不写入正式数据目录。 |
| `tools/sample` | 测量采样 `lanwork-sample`。按进程采样 CSV，并汇总延迟原始时间戳。运行命令和交换格式写在 `tools/README.md`。 |
| `docs/measurements/TEMPLATE.md` | 技术验证和性能测量的记录模板。还没有符合「性能测量」协议的实测记录。`docs/measurements/fileidx.md` 只说明 fileidx 那次场景。 |
| `docs/measurements/2026-10-08-empty-window.md` | 空 Slint 窗口采样。脚本通过：300 行，首尾 298.989 秒。偏离协议，不算「已达到」。 |
| `third_party/` | 第三方许可说明。已放入 Unicode 18.0.0 Unihan 读音摘录和 Unicode License v3，见 `third_party/unihan/`。已放入 Everything SDK 与 SDK3 的 x64 DLL 及许可，见 `third_party/everything/`。 |

### 尚未接入

| 内容 | 将落在 |
| --- | --- |
| 面板和其他界面 | `crates/app`。程序外壳和搜索条已接入。查询调度在 `lanwork_core::dispatch`，搜索条已调用，面板还没有。快速收集的预览和提交还没有。待办薄命令 `TodoCommands`、便签薄命令 `NoteCommands`、收纳薄命令 `ShelfCommands` 和 GitHub 薄命令 `GithubCommands` 已在 `crates/core`。热角的 50ms 轮询还没有接上 |
| 其余技术验证的最小程序 | `spikes/<名称>`。`spikes/hello`、`spikes/fileidx`、`spikes/render`、`spikes/hotcorner`、`spikes/toast` 与 `spikes/dnd` 已经在 |
| 渲染器 | 默认已写入「运行时」，外壳按此选择。技术验证表里的这一项仍未通过 |

## 技术验证

下面各项先在独立的最小程序里验证，结果记进 [roadmap.md](roadmap.md)。某项没有通过记录之前，不写依赖它的界面代码。某项失败时，先改本文的方案，必要时再改产品规格，不在功能代码里绕过。

每份记录写明日期、commit、Slint 精确版本、渲染器、机器、Windows build、GPU 与驱动、构建配置，每个通过条件分别标「通过」「失败」或「未测」。只有全部条件都通过才算该项通过。技术验证里的测量只说明该场景，不算「性能测量」一节的验收。

| 项 | 通过标准 | 依赖它的界面 |
| --- | --- | --- |
| 中文输入法 | 微软拼音在 Slint 文本框里能组合、上屏，候选窗贴在光标处；组合期间 Enter 不触发提交 | 全部 |
| 背景 | 选定渲染器下，透明窗口加 DWM Acrylic 正确显示；透明失败时能检测到并退回纯色 | 搜索条、面板 |
| 渲染器 | FemtoVG、Skia、软件渲染各做一个空面板和一个 200 行列表，按「性能测量」记录 Private Bytes、工作集和中文字形效果，选定一个写进本文 | 全部 |
| 外部拖放 | 从资源管理器拖入多个文件能拿到路径；从窗口拖出到资源管理器时，只出现复制和创建快捷方式两种效果。先测固定版本 Slint 自带的拖放，不满足再测原生 OLE | 收纳 |
| Everything | 通过随包的 SDK DLL，Everything 1.4 和 1.5 都能返回结果，并能区分「未运行」和「未就绪」 | 搜索 |
| Windows Search | Everything 不可用时，Rust 进程只按文件名查询 Windows Search 索引，正文命中不返回；记录结果 P95 和查询带来的 Private Bytes 增量 | 搜索 |
| 通知 | 到期通知能显示。安装版点击后打开面板并定位该条。便携版只显示提醒，点击不定位。安装版和便携版分开记录 | 待办 |
| 热角 | 收起状态下光标在屏幕上移动 5 分钟、静止 5 分钟，记录 Lanwork 的 CPU 占用和每秒唤醒次数（口径见「性能测量」） | 面板 |

## 运行时

- 只支持 Windows 11 x64。
- 界面用 Slint。渲染器默认 FemtoVG。初始化失败时自动退回软件渲染。外壳先 `BackendSelector::renderer_name("femtovg")`。这次 `select` 失败再用 `"software"`。不启用 Skia，因此窗口适配器的退路里也没有 Skia。`select` 成功之后，若 FemtoVG 在创建窗口适配器时失败（例如探测不到 OpenGL 2），Slint 1.18.1 的 winit 后端会再试已编译的渲染器，其中包含软件渲染。窗口已经显示之后，这一版不能再从 FemtoVG 换到软件渲染。窗口外观用 Desktop Window Manager：无边框、圆角、暗色、Acrylic（`DWMWA_SYSTEMBACKDROP_TYPE` 的 transient backdrop）。透明不可用时用纯色背景。不做 CSS `backdrop-filter`，也不提供按百分比调节的玻璃透明度。技术验证的渲染器一项仍未通过，这里的默认不是那一项的通过记录。
- Slint crate 固定为 1.18.1。升级单独提交，并重跑依赖该版本行为的技术验证。
- `DWMWA_SYSTEMBACKDROP_TYPE` 从 Windows 11 Build 22621 起可用。运行时检查 build 和调用返回值，不可用时不设置该属性，按纯色处理。产品支持的最低 build 尚未确定。
- 进程内没有 Vue、Tauri、WebView2。
- Slint 按 Royalty-free 许可证使用。归属声明用 Release 下载页上的 Slint 徽标，界面里不放 `AboutSlint`。许可证要求徽标在公开网页上，因此仓库公开之前不对外分发二进制。
- 界面调用薄命令。命令做校验，业务规则在服务中。需要保存的数据先原子写入，成功后再向本进程已打开的窗口发变更消息。非法输入返回明确错误。单进程、单写者。
- 中文输入法能在搜索条、面板搜索框、待办标题和便签正文中上屏。做不到这一点时，界面方案不成立，不能改用「只支持英文」通过验收。
- 搜索条和面板在启动时创建并保持隐藏，以满足热召回目标；隐藏中不做整页重绘。快速收集是搜索条的一种输入状态，不另建窗口。便签悬浮窗、番茄钟在使用时创建，关闭时释放文本、图标和绘图资源。到期提醒的调度保留在主进程里。
- Slint 1.18.1 编进本仓库的 winit 后端没有实现 `start_drag`（`i-slint-backend-winit` 1.18.1 的 `WinitWindowAdapter` 只实现了 `start_window_move`；`WindowAdapterInternal::start_drag` 的默认实现返回 false）。因此 `DragArea` 的拖动留在窗口内。`spikes/dnd` 用 `dispatch_event` 观察到：同一窗口里文件路径以复制放下，目标要求移动时放下被拒绝。winit 0.30.13 创建窗口时调用 `OleInitialize` 并 `RegisterDragDrop` 注册自己的 `FileDropHandler`；探针在我们注册之前得到 already-registered。资源管理器方向的手测见 [roadmap.md](roadmap.md)。`--slint` 拖放没有路径，外部拖放按「收纳」的原生 OLE 实现。
- 托盘用 Slint 1.18.1 的 `SystemTrayIcon`。它有菜单，图标是一张图，逾期数字画在这张图上。因此没有改用 Win32 `Shell_NotifyIcon`。
- 热角检测用 50ms 轮询读取光标位置，不用低级鼠标钩子。
- `spikes/hotcorner` 的实测记在 `docs/measurements/2026-10-08-hotcorner.md`。

Slint 负责版式。待办规则、便签保存、收纳、GitHub 刷新、搜索索引不写进界面回调里。

## 程序外壳

模块分成两处。`lanwork_core::config` 和 `lanwork_core::shell` 不依赖 Slint，也不调用 Win32。互斥量、热键、注册表和窗口在 `crates/app`。非 Windows 进程打印「Lanwork 只支持 Windows 11 x64」并退出，退出码 1。

启动顺序：先取得单实例，再解析数据目录并打开存储，然后完成存储启动、加载配置、按本地日期调用 `BackupCommands::backup_on_launch`、选择渲染器、注册热键、按配置写开机启动项，最后创建隐藏的搜索条、面板宿主和托盘。存储启动时外壳先注册 `BackupCommands` 的导入恢复钩子，再注册待办的加载和修复。存储启动之后打开 `NoteCommands`、`AppIndex::open_from_process`（随后用 `load_user_catalog` 读 `user-apps.json` 交给 `set_user_catalog`）和 `FileCommands`，组成 `Dispatch` 并 `build`。这几步任一失败只写日志，进程继续，搜索条不安装，按热键不出现。不打开收纳。不建 GitHub 刷新定时器。`githubRefreshIntervalMs` 原样保存，包括 0，外壳不解释它。

- **单实例。** 命名互斥量 `Local\Lanwork.SingleInstance.Mutex`，加上信号量 `Local\Lanwork.SingleInstance.Activate`（初值 0）。第一个进程拥有互斥量并等待信号量。后来的进程发现互斥量已存在，就对信号量加一，然后退出，退出码 0。信号量的计数会留到第一个进程来取，所以通知发生在等待之前也不会丢。第一个进程收到通知后不打开窗口。产品规格没有写这时要显示什么。
- **托盘。** 菜单顺序是「打开面板」「设置」「新建便签」「刷新 GitHub」「退出」。提示文字只有「Lanwork」。图标是 32×32 的图，底色不是产品规格。逾期数量来自 `TodoCommands::overdue_count`，今天的日期用 `GetLocalTime`。数量为 0 时不画数字。数字画不下时裁掉超出图标的笔画，不改成「99+」。待办变更消息到达后在界面线程重画。读数量或日期失败时不画数字，并写日志。左键和右键都由 Slint 打开同一份菜单，没有另加左键动作。
- **打开面板、设置、新建便签。** 菜单项会触发对应命令。不创建便签，也不显示搜索条、面板或设置页。这些窗口分别属于后续界面。
- **刷新 GitHub。** 在界面线程之外调用 `GithubCommands::load` 和 `refresh_all`。时间是 `SystemTime` 的 UTC 毫秒。设置取当前配置里的长期未更新天数、来源同步、关闭来源时自动完成待办和刷新间隔。失败写入日志，不包含 `gh` 的标准输出、标准错误或 token。已经有一次刷新在进行时，再次点击直接返回，不排队。没有另外的提示窗口。
- **退出。** 当前没有便签编辑器，`quit_without_note_editors` 返回结束进程。先隐藏托盘，再退出事件循环，然后卸下热键并释放互斥量。`QuitDecision::Stay` 留给以后的便签界面：那种情况下不结束进程，也不丢弃正文。现在没有这条返回值，也不做重试或放弃的对话框。
- **热键。** `RegisterHotKey` 带 `MOD_NOREPEAT`，注册在单独线程的一个不显示的顶层窗口上。这个窗口不带 `WS_VISIBLE`。不用 `HWND_MESSAGE`，否则收不到 `WM_SETTINGCHANGE`。搜索条 id 是 1，面板 id 是 2。字符串格式是实现选择：修饰键 `Ctrl`、`Alt`、`Shift`、`Win`，加一个键。键是 `A`–`Z`、`0`–`9` 或 `F1`–`F24`。比较和保存前收成 `Ctrl+Alt+Shift+Win+键`。其他键名是「热键无效」。没有修饰键的单个键当前可以保存。产品规格没有写键名表。搜索条热键为空是「搜索条热键不能为空」。两个热键相同是「搜索条热键与面板热键相同」，不写盘。运行中修改热键只走先注册、成功后写入配置、再卸掉不再使用的旧热键。注册失败时磁盘和当前注册都保持旧热键。本进程内两条热键互换，或把一条的组合改给另一条时，先卸掉本进程里与新组合冲突的注册，再注册；失败则全部恢复。内容没变时不调用系统。写入配置时先更新内存，再发布变更。第一次启动就占用时，不改 `config.json`，写日志，进程继续，热键保持未注册。搜索条热键显示或收起搜索条，见下面的「搜索条」。面板热键按下后不显示面板。
- **配置。** 见「数据」。不认识的 `schemaVersion` 让进程以退出码 1 结束，不改文件，没有对话框。
- **主题。** 明和暗直接写调色板。跟随系统时读 `HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize` 的 `AppsUseLightTheme`：0 是暗，其他值是明。读不到就不改调色板，也不猜测。`WM_SETTINGCHANGE` 且 `lParam` 为 `ImmersiveColorSet`（忽略 ASCII 大小写）时再读一次。调色板用 `ColorScheme.dark` 和 `ColorScheme.light` 这两个变体名。Slint 1.18.1 里这两个变体的注释文字和名字相反，以变体名为准。
- **开机启动。** `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 的值名 `Lanwork`，类型 `REG_SZ`，内容是带引号的 exe 路径。配置为真且与现有值不同就写；为假且值存在就删。路径为空、不是 Unicode 或含 `"` 时不写。写入或删除失败只记日志，不把配置里的开关改回去。读不到现有值时也不改注册表。
- **隐藏窗口。** 搜索条和面板宿主在 `Component::new` 时创建，不调用 `show`。Slint 1.18.1 在这时创建 winit 适配器并登记为未激活窗口，事件循环开始时创建不可见的操作系统窗口，不会把未显示的窗口设为可见。面板宿主里没有输入框，也没有亚克力。事件循环用 `run_event_loop_until_quit`：搜索条收起后所有窗口都隐藏，事件循环不能因此结束；托盘「退出」才结束。

外壳在 `boot` 前注册 `BackupCommands::register_boot_hooks`。有 `import.pending` 时先恢复，再加载待办和配置。`import_recovered` 为真时记日志「导入未完成」，并留在 `import_was_recovered`。界面尚未显示这句提示。读不到本地日期或自动备份失败时记日志，进程继续。没有恢复钩子时 `boot` 仍失败，外壳以退出码 1 结束。

### 搜索条

可见行为见产品规格「搜索条」「搜索」。代码在 `crates/app/src/searchbar.rs`（窗口与调度）、`crates/app/src/bar_win.rs`（Win32）和 `crates/app/ui/searchbar.slint`（版式）。不调用 Win32 的决定在 `lanwork_core::shell` 的 `bar`：背景选择、位置、修饰键与动作、Esc、收起后保留的文本、选择移动和结果区那一行文字。

技术验证的「背景」和「渲染器」两项仍没有通过记录。这一版搜索条的界面代码依赖这两项，写在通过之前；两项的记录不因此改变。

- **窗口。** `no-frame`，逻辑宽 680px，高度是输入框 56px 加结果区。结果行 48px，换组时多留 10px 并画一条分隔线，列表上下各留 6px。结果区最多约 9 行高，并且不超过工作区底边之上 24px；更多的行在 `Flickable` 里滚动。版式数值是实现选择，产品规格没有写。字体 `Microsoft YaHei UI`。高度变化由 winit 改窗口大小，左上角不动，所以上边缘留在 25% 处。
- **位置。** 每次显示前用 `GetCursorPos`、`MonitorFromPoint`、`GetMonitorInfoW` 的 `rcWork` 和 `GetDpiForMonitor(MDT_EFFECTIVE_DPI)` 算出物理坐标：水平居中，上边缘在工作区高度 25% 处，再 `set_position`。显示后用 `GetWindowRect` 再对一次，不一致时 `SetWindowPos`（不改大小、不激活）。
- **背景。** 窗口第一次存在（`winit_window()` 完成）时、每次显示前、主题变化时重新判断。Windows build 低于 22621 或读不到、`EnableTransparency` 为 0 或读不到（值不存在按开启）、`GetSystemPowerStatus` 报节电或读不到时用纯色。否则 `DwmExtendFrameIntoClientArea(-1)`，再设 `DWMWA_SYSTEMBACKDROP_TYPE = DWMSBT_TRANSIENTWINDOW`；任一调用失败也用纯色。纯色时仍扩展边框，并把背景类型设回 `DWMSBT_NONE`，客户区由 Slint 画满 `#F3F3F3`（浅）或 `#202020`（暗）。圆角是 `DWMWCP_ROUND`，暗色用 `DWMWA_USE_IMMERSIVE_DARK_MODE`。透明渲染本身失败（例如客户区画成黑色）没有在运行时检测，只看 DWM 调用的返回值。
- **任务栏。** 用 `unstable-winit-030` 的 `set_skip_taskbar(true)`，由 winit 在样式更新时保留。不手写 `WS_EX_TOOLWINDOW`，winit 每次改窗口状态都会重写扩展样式。
- **显示与收起。** 热键在搜索条可见时收起，否则显示：定位、设背景、`show`、`SetForegroundWindow`、焦点给输入框。winit 的 `Focused(false)` 到达时收起。收起时除未提交的收集文本外清空输入并向调度提交空串，然后把延迟记录写出。
- **输入。** 输入框是 `TextInput`，不是 `LineEdit`，因为要读 `preedit-text`。`key-pressed` 里预编辑非空时一律交还输入法。Esc、上、下、Enter 由搜索条处理，其他键交给 `TextInput`。`edited` 把已提交的文本交给 `Dispatch::submit(Surface::SearchBar, …)`，结果在界面线程上立即画出。收集前缀时调度返回收集状态，搜索条不显示结果；收集预览和提交属于快速收集，还没有。
- **后台。** 一个线程取当前可见行的图标；文件阶段还在等待时，等 60ms（新输入重新计时）再 `poll`，结果和图标交回界面线程。界面只接受 `accepts` 为真的序号。同一段输入的后续结果保留选中的那一条，输入变了回到第一条。上下键不回绕。每行有一个 `TouchArea`：指针在行上移动（`PointerEventKind.move`）时选中该行，列表在静止的指针下滚动不改选择；单击先选中该行，再按不带修饰键的 Enter 处理，组合输入期间同样无效。滚轮交给 `Flickable`。
- **动作。** 由 `row_action` 决定，没有动作时按键无效。Shell 调用在单独线程上，成功后收起，失败时在结果区下方显示错误并写日志，不收起；那时已经因失焦收起的只写日志。打开一项（不含打开所在目录）后 `record_open`。待办和便签结果先 `record_open` 再收起，面板定位和便签悬浮的调用点留空，等面板和便签悬浮窗。
- **延迟记录。** 环境变量 `LANWORK_LATENCY_OUT` 非空时，热召回和结果延迟的原始记录按 `tools/README.md` 的 JSONL 追加到该文件。界面不暴露这个变量。热召回起点是平台线程收到 `WM_HOTKEY` 时的 `SystemClock`，终点是显示后第一次 `AfterRendering`。结果记录在同一序号的视图画出后用 `mark_rendered` 填终点：本地结果在第一次画出时，文件来源在文件阶段结束的视图画出时。进程启动后第一次热召回和第一个查询序号标为预热。已经填了终点的记录不再因后来的输入标为被取代。没有按「性能测量」采样时，这些记录不表示目标已经达到。

## 通知

到期通知使用操作系统自带的 WinRT `ToastNotificationManager`，由 `windows` crate 调用。不使用 Windows App SDK 的 `AppNotificationManager`：后者要求随包或在机器上部署 Windows App Runtime，解包后的程序还要先调用引导程序。这份运行时不进入本项目。`ToastNotificationManager` 在 Windows 11 上由系统提供。微软目前把该 API 标为维护状态，并推荐 `AppNotificationManager`；若下面的验证失败，先改本节，再决定是否更换 API。

最小程序在 `spikes/toast`。正式的提醒调度、安静时段和待办页定位仍不在这里实现。

注册只写当前用户（HKCU），不写 HKLM，也不要求管理员权限。进程调用 `SetCurrentProcessExplicitAppUserModelID` 设置 AppUserModelID。通知 XML 的 `launch` 属性是待办 id。点击后，`INotificationActivationCallback::Activate` 的参数带回这个 id。仅拿到该参数还不算定位；定位是打开面板、切到待办并选中该条。验证程序用一个模拟列表面板演示这件事。注册缺失时不崩溃，改为只显示托盘逾期徽标。托盘徽标不能代替通知通过。

安装版同时做三件事：

- 在当前用户的开始菜单程序目录放置快捷方式。属性 `System.AppUserModel.ID` 为 AUMID，`System.AppUserModel.ToastActivatorCLSID` 为激活器 CLSID。
- 在 `HKCU\Software\Classes\AppUserModelId\<AUMID>` 写入 `DisplayName` 和 `CustomActivator`（同一个 CLSID）。
- 在 `HKCU\Software\Classes\CLSID\<CLSID>\LocalServer32` 写入本程序路径。进程运行时另外 `CoRegisterClassObject`，让点击落到已经打开的进程；进程已退出时由 `LocalServer32` 再启动。

类工厂和 `INotificationActivationCallback` 放在 STA，并且不能实现 `IAgileObject`，也不能用自由线程封送。通知平台在另一个进程里 `CoCreateInstance`。自由线程封送的数据包是进程内指针，跨进程解封送失败后，SCM 会再按 `LocalServer32` 启动带 `-Embedding` 的进程，`Activate` 仍然到不了回调。`CoRegisterClassObject` 使用 `REGCLS_MULTIPLEUSE | REGCLS_SUSPENDED`。窗口已经创建、线程准备进入消息循环之后调用 `CoResumeClassObjects`。`Activate` 由这个消息循环派发。回调里要 `ShowWindow` 并 `SetForegroundWindow`，面板上显示已定位的待办 id。

控制台子系统进程和 COM 拉起的本地服务器，启动信息里经常带 `SW_HIDE`。第一次 `ShowWindow` 会改用这个值。面板需要再调用一次 `ShowWindow(SW_SHOWNORMAL)`，并用 `SetWindowPos(SWP_SHOWWINDOW)` 确认 `IsWindowVisible`。读快捷方式属性之前要先 `CoInitializeEx`。

便携版只写 `HKCU\Software\Classes\AppUserModelId\<AUMID>` 的 `DisplayName`，不创建快捷方式，不写 `CustomActivator`，不写 CLSID。便携版通知只显示提醒，点击不定位。2026-10-09 的记录里，便携版通知能显示；运行中点击没有定位，日志没有 `COM Activate`；退出后点击没有拉起进程。

卸载要删掉上述快捷方式和 HKCU 键，由安装程序实现。验证程序自己的注销只清理它写过的 spike 标识，不清理将来产品用的标识。

## 数据

默认目录：`%USERPROFILE%\Documents\Lanwork`。该目录与 `%USERPROFILE%\Documents\MayDolist` 互不读取。存储层拒绝把数据目录或写入路径解析到 MayDolist 及其子目录，也不创建那个目录。

数据目录按下面的顺序解析，命中即停止：

1. 环境变量 `LANWORK_DATA_DIR`（去掉首尾空白后非空）。相对路径按当前工作目录补成绝对路径。
2. 引导文件 `%LOCALAPPDATA%\Lanwork\bootstrap.json` 的 `dataDir`。引导文件在数据目录之外，迁移之后仍能找到。
3. 默认 `%USERPROFILE%\Documents\Lanwork`。

引导文件是 JSON，字段为 `schemaVersion`（当前为 1）和 `dataDir`（绝对路径）。缺少 `schemaVersion` 时按 1 读取。无法解析、`dataDir` 为空，或 `schemaVersion` 高于当前版本时，解析报错，不改回默认目录。设置里的迁移在校验新目录之后调用存储层写入该文件。写入之前若进程中断，下次启动仍用旧位置。缓存目录固定为 `%LOCALAPPDATA%\Lanwork\cache`，不随数据目录改变。

模块在 `crates/core` 的 `storage`。不依赖 Slint，也不调用 Win32 窗口 API。变更消息在本进程内发布；投递到已打开窗口由后续的应用外壳订阅，不在这一层调用窗口 API。

```text
Lanwork/
├── config.json
├── user-apps.json           便携应用、别名和隐藏名单
├── backups/
├── logs/app.log
├── notes/<id>.json          一篇便签一个文件
├── todos/<id>.json          一个清单一个文件，含其中的条目
├── shelves/<id>.json        一个收纳分组一个文件，含其中的引用
└── github/
    ├── watchlist.json
    └── cache/<repo>.json
```

应用索引和图标缓存放在 `%LOCALAPPDATA%\Lanwork\cache`，可删除后重建，不进导出包。

便携应用、别名和隐藏名单在数据目录的 `user-apps.json`。字段见「搜索」。该文件进入导出包，也进入备份。`%LOCALAPPDATA%\Lanwork\cache\apps.json` 可以删除后重建，不能代替 `user-apps.json`。

- 文件名是 id。id 是单个路径分量，不允许分隔符、Windows 保留设备名和文件名非法字符。`github/cache/<repo>.json` 的 `<repo>` 也是单个分量。`owner/repo` 的编码由 GitHub 服务决定：每个 `/` 换成 `%2F`，见「GitHub 服务」。
- 写入：进程内一把互斥锁。同目录写 `<name>.tmp`，调用 `FlushFileBuffers` 后，目标已存在时用 `ReplaceFileW`（`REPLACEFILE_WRITE_THROUGH`），否则用 `MoveFileExW`（`MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH`）。替换失败则删除临时文件、保留原文件，并返回带路径的错误。不自动重试。数据目录位于 OneDrive 同步的「文档」下时，同步进程占用按同一规则报错。
- 单个 JSON 无法解析时，把该文件改名为 `<id>.json.corrupt-<UTC 毫秒>-<序号>` 并记日志，其他文件仍可读。日志不含文件内容。不认识的 `schemaVersion` 不隔离，由调用方拒绝。
- 每个 JSON 对象带数字字段 `schemaVersion`。当前版本是 1。新字段可选并有默认值；缺少 `schemaVersion` 的旧文件按 1 读取。示例模型是 `ExampleDocument`，不是待办或便签模型。
- 写盘成功后发布 `EntityChanged { kind, id, revision }`。`revision` 只在便签保存时携带新的修订号，其他实体为 None。跨文件批次在提交成功后按写入顺序发布；写盘失败或批次未提交不发布。订阅回调在写锁释放之后执行。
- 日志在 `logs/app.log`。单文件超过 1 MiB 时轮转，保留 `app.log`、`app.log.1`、`app.log.2`。不写文件内容，因此不写便签正文。落盘前去掉常见 GitHub token 形态，以及名称含 `TOKEN`、`SECRET`、`PASSWORD`、`CREDENTIAL`、或以 `_KEY` 结尾且值长度至少 8 的环境变量的值。错误里仍会写出文件路径。GitHub 服务通过这里记 `gh version <版本>`、`gh not installed` 或 `gh unavailable`。版本号只保留有限的版本字符，无法识别时记 `unknown`。不写环境变量，也不写 `gh` 的标准输出和标准错误。
- `import.pending` 的内容是备份绝对路径的 UTF-8 文本，不是 JSON。它不进入导出包。
- 收件箱是 `kind=inbox` 的待办清单，首次需要时创建一次。
- 待办可带到期日、提醒、周期、GitHub 来源，以及至多一个当前标记。每月重复的锚点日是周期上的 `monthDay`（1 至 31）。缺少该字段的旧记录按当前到期日的日子计算。已经写下的 `monthDay` 不因本次到期日落到月末而改掉。
- 收纳服务在 `lanwork_core::shelves`。分组文件是 `shelves/<id>.json`，字段为名称、排序、可选的关联待办 id，以及引用列表。每条引用保存绝对路径、显示名、是否文件夹和加入时间，不保存文件内容和图标。路径规范化和存在性检查见「收纳」。
- 便签带递增的 `revision`。保存时带上加载时的 revision，与磁盘不一致则返回冲突错误，调用方保留正文。冲突之后的可见行为以产品规格为准。
- 便签服务在 `lanwork_core::notes`。文件字段还有 `title`、`body`、`tags`、`pinned`、`createdAt`、`updatedAt`、`deletedAt`。时间是 Unix 纪元起的 UTC 毫秒。`deletedAt` 缺省表示未软删除，文件仍留在 `notes/`。标签去掉首尾空白后按原文去重，筛选是去重后的整段相等。列表中置顶在前，其后按 `updatedAt` 从新到旧，再按 id。标题没有非空白字符时显示「无标题」，原文仍写入 `title`。删除写入 `deletedAt`，文件留在 `notes/`。永久删除只删除已经软删除的文件；删文件成功后、发布变更前从内存去掉。进入回收站满 30 天（30×86400000 毫秒）的便签，`deleted` 不再返回，`restore` 不恢复、也不写盘。打开服务时清除一次；外壳也可以调用 `NoteCommands::purge_expired`。服务不建定时器，调用间隔未在规格里写明。某一篇删不掉时记日志并跳过，打开服务仍成功，其余便签照常可用。服务不做覆盖。冲突后的版本选择、关闭和退出时的未保存正文由界面处理。
- GitHub 通过本机 `gh` 读取。token 不进入配置、日志或导出包。
- 导出包包含配置、待办、便签、收纳分组、GitHub watchlist，以及 `user-apps.json`（便携应用、别名和隐藏名单），GitHub 缓存可选。导出省略缓存不等于清空本地 `github/cache`。不含日志、备份目录、`import.pending`、token 和环境变量。备份包含 `user-apps.json`，并且总是包含 GitHub 缓存。服务在 `lanwork_core::backup`，薄命令是 `BackupCommands`。包是只含存储法（compression method 0）的 ZIP。根上的 `manifest.json` 有 `packageSchemaVersion`（当前为 1）、`exportedAt`（调用方传入的 UTC 毫秒）、`appVersion` 和 `counts`。路径拒绝 `..` 和绝对路径，只接受上述白名单。每个 JSON 可解析；缺少 `schemaVersion` 按 1，高于 1 则整包拒绝。自动备份文件名是 `auto-年-月-日-毫秒.zip`，手动是 `manual-毫秒.zip`，导入前的备份是 `import-毫秒.zip`。`backups/auto-day.txt` 记下最近一次自动备份的公历日，不计入 7 份。同一天再次调用自动备份不再写新文件。自动和手动合计超过 7 份时按文件名里的毫秒删除最旧的，不删除 `import-` 和其他文件。手动备份不改 `auto-day.txt`。调用方传入公历日和毫秒，服务不读时钟，也不读环境变量。
- 不实现 Focus 的聚合服务，也不实现从 MayDolist schema 的一次性迁移。

### GitHub 服务

模块在 `lanwork_core::github`。薄命令是 `GithubCommands`。规则在服务里。`crates/core` 不依赖 Slint，也不打开浏览器；`open_url` 只返回 `http` / `https`。测试用可替换的 `GhClient`，不访问网络，也不要求本机已登录。

生产调用是 `ProcessGh`。它只启动本机 `gh`，参数是独立的 argv，不经过 shell。Windows 上 `CreateProcess` 带 `CREATE_NO_WINDOW`（`0x08000000`）。子进程继承当前环境，以便本机 `gh` 使用已有登录；Lanwork 不读取、不保存、不记录这些变量。仓库名以 `-` 开头或含控制字符时不传给 `gh`。这是调用参数的约束，不阻止把这样的字符串记入 watchlist。

`gh version` 的版本写入日志，规则见上面的日志一条。`gh auth status` 退出码为 0 视为已登录。非 0 且不是网络失败或速率限制时视为未登录。不把登录名写入日志或快照。

- `github/watchlist.json` 的字段是 `schemaVersion`、`repos`（字符串数组）、`ignored` 和 `pinned`。后两者的元素是 `repo`、`kind`（`github-pr` 或 `github-issue`）和 `number`。忽略和钉住只被记住，并出现在列表模型上。是否因此隐藏或提前，产品规格没有写。同一仓库字符串出现多次时，刷新和列表只保留第一次。这是避免重复请求的机制，产品规格没有写重复项。
- 读取追踪仓库走这份文件。`add_tracked` 只按产品规格检查 `owner/repo`：恰好一个 `/`，两边都非空。格式不对时返回「正确格式是 owner/repo」，不增加追踪。空白算不算格式错误，产品规格没有写，因此不因此拒绝。同一字符串再次添加是再追加、拒绝还是保持原样，产品规格没有写；当前不写盘，返回内部错误 `DuplicateTracked`，没有界面文案。`remove_tracked` 先把该字符串从 `repos` 写回。这一步失败则快照和待办来源都不变。然后删除快照：`cache_file_id` 失败或文件不存在就跳过，不因此拒绝移除。最后断开 `source.repo` 与该字符串完全相同的待办，含已完成和回收站，待办仍在。删快照或断开来源失败时，仓库已经不在追踪列表里；再次调用会把剩下的步骤做完。三步都没有可做的工作时返回未追踪。不会出现来源已断、仓库仍在追踪。移除后是否清掉忽略和钉住，产品规格没有写，因此不改这两份名单。
- 快照文件名是仓库名中每个 `/` 换成 `%2F`。结果必须仍是存储层允许的 id，否则该仓库记为失败，不调用 `gh`。名字里本来就有 `%2F` 时会和带斜线的名字共用一个文件；产品规格没有写这种输入。
- 快照字段是 `schemaVersion`、`repo`、`fetchedAt`（UTC 毫秒）和 `items`。条目只保存这次读到、状态明确为 open、并且有非空白标题和 `http` / `https` 地址的 PR 与 Issue。Draft 只来自 PR 的 `isDraft` 或 `draft`。可选字段缺失或类型不对时，该项用默认值：不是 Draft，也不标长期未更新。顶层不是 JSON 数组才让这个仓库失败。不认识的 `schemaVersion` 不使用该文件，也不隔离。
- 刷新一个或全部已追踪仓库。某个仓库失败时不写它的快照，其他仓库照常写入。失败后若已有快照，列表仍用它，并标「离线缓存」。这个标记不写进快照文件。成功的刷新清掉它。进程从磁盘载入后、尚未在本次进程里成功刷新时，已有快照也标「离线缓存」。没有快照的失败不标这四个字，只带失败原因。
- 已实现的信号只有长期未更新和 Draft。天数是 0 时不标长期未更新。没有更新时间，或更新时间晚于调用方给出的当前时间时，也不标。否则当 `now - updatedAt >= 天数 × 86400000` 毫秒时标上。一天按 86400000 毫秒，不按日历。需要处理、需要 Review、CI 失败不计算。`SignalExtension` 可以观察已经决定显示的条目，服务不读取它的结果。
- `list` 没有筛选。产品规格已规定多个筛选项按并集，以及「我的」「被分配」「被提及」「参与」「需要处理」「需要 Review」「CI 失败」的含义。CI 进行中、没有检查、无权限读取各自单独显示，不算 CI 失败。快照只有编号、标题、地址、状态、Draft 和更新时间，PR 另有 `mergedAt`，没有作者、被分配、被提及、审查请求和 CI 结果。`list_filtered` 在任一筛选项为真时仍返回错误，不改数据，也不把缺数据的条件猜成未命中。长期未更新和 Draft 只在无筛选列表上标出。
- 列表在读取时跳过「来源类型、仓库字符串、编号都相同，且未完成、`deletedAt` 为空」的待办所关联的条目。完成后或软删除后，快照里仍是 open 就再次出现。永久删除后条目不存在，不再隐藏。隐藏不写 watchlist，也不写快照。
- 转为待办调用待办服务：没有收件箱时先创建，再一次写入条目和来源。标题是 `仓库#编号 原标题`。找不到快照条目，或待办写入失败时，不留下这条待办。收件箱文件若已在同一次调用里创建成功，空的收件箱可以留下。
- 来源同步在该仓库快照提交成功之后进行。只在调用方传入的「关闭来源时自动完成待办」为允许时，把状态明确是 `closed` 或 `merged` 的来源所关联的未完成、未软删除待办标为完成。`mergedAt` 非空且状态是 `closed` 时视为 `merged`，两者都会完成待办。状态缺失、无法识别、条目不在这次响应里、解析失败、网络失败、速率限制、未安装、未登录、仓库不存在、快照没有写成功，都不标完成，也不把待办改回未完成。来源再次变为 open 时也不改回。
- 自动完成逐条调用待办服务的完成。某一条失败时该条保持原状态，错误进入刷新结果；已经完成的其他条不回滚，已经写好的快照也不回滚。设置里的「来源同步」原样出现在刷新结果里。它和「关闭来源时自动完成待办」如何组合，产品规格没有写，因此不单独作为完成条件。
- 刷新间隔由调用方传入，服务不读 `config.json`，也不自建定时器，也不解释 0。
- 单次 `gh pr list` 与 `gh issue list` 使用 `--state all --limit 200`，字段是编号、标题、地址、状态、Draft、更新时间，PR 另加 `mergedAt`。超过 200 条如何分页，产品规格没有写。
- 内存只在对应文件写成功后更新。变更消息由存储层在提交后发布。写盘失败不发布，也不改这份内存。变更回调里可以读列表，不要再调用刷新、转为待办或忽略、钉住。

### 跨文件操作

单文件替换是原子的，跨文件的操作不是。下面三类操作各自保证进程在任意时刻中断后都能恢复到一致状态，不引入通用事务日志。

- **跨清单移动待办**：先写入目标清单（条目带新的 `movedAt` 时间），再从源清单移除。中断后同一 id 出现在两个清单时，加载阶段保留 `movedAt` 较新的一条，并把另一份移除后写回。`movedAt` 相等时保留清单 id 较小的一条；清单也相同则保留靠前的那一行。没有 `movedAt` 的视为更旧。
- **切换当前待办**：先给新条目写上当前标记和 `currentSince` 时间，再清除旧条目的标记。中断后出现多条时，加载阶段保留 `currentSince` 最新的一条，其余清除后写回。`currentSince` 相等时保留清单 id 较小的一条；清单也相同则保留条目 id 较小的一条。没有 `currentSince` 的视为更旧。只有一条当前标记时不改写。

进程仍在、同一批次里后面的写入失败时，先把本批次已经替换的文件写回内存里的操作前内容，然后返回错误。内存保持操作前状态，不发布 `EntityChanged`。这次回滚再失败时，重新读入清单，按上面的 `movedAt` 与 `currentSince` 规则修好后再返回，避免留下重复的条目 id 或两条当前标记。修复成功时返回的仍是原来的写入错误。重新读入失败则返回该读取错误。
- **导入**：由 `BackupCommands` 解压并校验（白名单路径，拒绝 `..` 和绝对路径，每个 JSON 可解析，版本可识别）。校验失败不写 `import.pending`，当前数据不变。通过后，先把当前数据完整备份到 `backups/import-<毫秒>.zip`，再在数据目录写入 `import.pending`（内容是这份备份的路径），然后逐个替换文件、删除导入包中没有的数据文件，最后删除 `import.pending`。包里没有 GitHub 缓存时不删除本地 `github/cache`。替换失败时用这份备份恢复；恢复没有成功则留下 `import.pending`。替换成功后，已打开的配置、待办、便签、收纳和 GitHub 服务从磁盘重新载入，然后才发布 `EntityChanged`。启动时外壳注册的恢复钩子发现 `import.pending`，就用其中记录的备份整体恢复，再删除该文件。`import_recovered` 为真时外壳记日志「导入未完成」，界面尚未显示这句提示。备份文件不存在时不删除 `import.pending`。

加载修复和导入恢复在构建搜索索引、处理通知点击之前完成。`Store::boot` 的顺序是：若存在 `import.pending`，调用导入恢复钩子（备份与导入实现；未注册钩子则启动失败，不加载业务数据）；然后调用加载钩子；然后按注册顺序运行加载修复（`movedAt` 与 `currentSince` 由待办服务实现）。任一钩子返回错误则启动失败，不进入可建索引状态，也不写默认文档。成功之后 `build_index` 与 `handle_notification_click` 才执行调用方。恢复钩子成功且已删除 `import.pending` 时，`BootReport.import_recovered` 为真。外壳记住这个结果并记日志「导入未完成」；界面尚未显示这句提示。导入的 `EntityChanged` 在整次替换完成、已打开的服务重新载入内存之后才发出。永久删除通知是待办服务的领域事件，不是这一层的 `EntityChanged`。永久删除只删除回收站中的条目。从清单删掉之前，把 id 写入数据目录的 `todo-purge-pending.json`（`schemaVersion` 1，字段 `ids`）。这份文件不在 `todos/` 里，避免被当成清单。清单写入成功后发出 `TodoNotice::Purged`。收纳若还没订阅，这条事件不会留下。收纳调用 `watch_todo_notices` 时先订阅，再按这份记录补解除：只解除记录中、且已经不在任何清单里的 id，解除写盘成功后才从记录去掉。记录里的 id 若待办还在，不解除，以免删除尚未落盘时拆掉关联。不在记录里的关联保持原样，读取时也不把残留 id 视为无关联，避免定下「关联到尚不存在的待办 id」。进程在清单写盘之后、收纳写盘之前中断时，下次订阅用同一份记录补上。解除写盘失败时记录还在，下次订阅再试。记录无法读取时不覆盖它，启动加载跳过这一次自动清除，不因此让启动失败。见「收纳」。

待办服务把加载钩子和名为 `movedAt`、`currentSince` 的两条加载修复注册到 `Store::boot`。修复写回使用同一批次，`EntityChanged` 只在该批次提交后发出。永久删除的领域事件是 `TodoNotice::Purged`。服务只删除回收站中的条目。删除前先把 id 写入 `todo-purge-pending.json`，清单写入成功后发出这条事件。启动加载用当时的时间清除已满 30 天的回收站条目；这次清除可能发生在收纳订阅之前，关联靠清除记录补上，不靠重放已经丢掉的事件。

配置至少包括：数据目录、热角、搜索条热键、面板热键（可空）、安静时段、主题、开机启动、GitHub 刷新间隔、长期未更新天数、来源同步、自动完成关联待办、处理模式顺延天数、番茄钟的工作时长、休息时长和长休息时长。不包括玻璃透明度。番茄钟不另存计时记录。GitHub 服务不读 `config.json`。刷新间隔、长期未更新天数和关闭来源时自动完成待办由调用方传入。

配置模块是 `lanwork_core::config`。文件是 `config.json`，经存储层读写。字段用 camelCase，`schemaVersion` 当前为 1。缺少 `schemaVersion` 时按 1 读取。无法解析的 JSON 由存储层隔离，外壳改用默认值，不把默认值写回去。JSON 能解析但热键或范围不合法时不隔离、不覆盖，内存里用默认值。不认识的 `schemaVersion` 不隔离，调用方拒绝。

`dataDir` 只是文件里的记录。启动目录仍按环境变量、引导文件、默认目录解析，不读这个字段。`quietHours` 只接受空。非空值返回「安静时段的格式尚未确定」，不写盘。起止格式产品规格没有写。

下面的缺省不是产品规则，只是文件没写该字段时的值：主题跟随系统，开机启动关闭，来源同步关闭，关闭来源时自动完成待办关闭，刷新间隔 0 毫秒，`dataDir` 空字符串，安静时段空。产品规格已经写明的默认是：热角右上，搜索条热键 `Ctrl+Alt+M`，面板热键空，长期未更新天数 14（0 允许），处理模式顺延天数 3（1 至 30），番茄钟 25、5 和 15 分钟。工作时长 1 至 120，休息时长和长休息时长 1 至 60。错误文字是「工作时长只接受 1 至 120 分钟」「休息时长只接受 1 至 60 分钟」「长休息时长只接受 1 至 60 分钟」「处理模式顺延天数只接受 1 至 30」。

## 搜索

应用索引、待办与便签索引、文件索引三者分开。

应用来源：当前用户和公共开始菜单、注册表 App Paths、PATH、商店应用、用户添加的便携应用和别名。按启动目标去重。快捷方式保留参数和工作目录。启动时先加载缓存，再在后台更新。目录变化合并后再重建；注册表、商店应用和手动刷新按来源更新。

应用索引在 `crates/core` 的 `apps`（`lanwork_core::apps`）。枚举用快捷方式、注册表、`shell:AppsFolder`、`ReadDirectoryChangesW` 和 `ShellExecuteExW`，不创建窗口，所以不放进 `crates/app`。便携应用、别名、隐藏和手动刷新的可见入口已写入产品规格。设置页、右键菜单和快捷键尚未实现。`AppIndex::refresh` 仍只是进程内重建。目标文件不存在的快捷方式不收录，即使它带 `System.AppUserModel.ID`；同一商店应用仍由 `shell:AppsFolder` 按 AUMID 收录。`shell:AppsFolder` 也列出开始菜单快捷方式带来的桌面应用，它们的解析名是 `Chrome` 这类自定 AUMID 或 `{已知文件夹 GUID}\…\chrome.exe`，不是快捷方式的目标路径，按启动目标去重合不上，同一个应用会出现两条。商店来源因此只收打包应用的 AUMID，形如 `<包名>_<13 位发布者 ID>!<应用 ID>`（`rules::is_package_aumid`）；其余项由开始菜单来源收录，被开始菜单过滤掉的（卸载程序、目标不存在、启动文件夹）也不会从这里回来。商店应用的图标键是 `shell:AppsFolder\<AUMID>`，取图仍走 `SHCreateItemFromParsingName` 和 `IShellItemImageFactory`，得到包里的图标。商店枚举中途失败时该来源整次失败，保留上一次快照。`ShellExecuteExW` 带 `SEE_MASK_NOASYNC`，因为这里没有消息泵，宽字符串在调用返回后释放。

下面是当前实现选择，不是产品规则。PATH 上的 UNC 目录跳过，避免一个断开的网络路径挡住其余来源。开始菜单目录变化的安静时间是 400ms，另有 2 秒上限，到点就重建开始菜单来源。`apps.json` 的 `schemaVersion` 不是 1 时，和无法解析一样隔离成 `apps.json.corrupt-<UTC 毫秒>-<序号>` 并记日志，日志不含文件内容。这和数据目录里不认识的 `schemaVersion` 不隔离不同，因为这份缓存可以重建。读取缓存时的 IO 错误只记日志，不改名。`apps.json` 仍是 schemaVersion 1，新增可选字段 `alternateNames`，来源值 `portable` 与 `alias`，以及目标种类 `url`。旧缓存没有这些字段时按空值读取。

开始菜单的 `.lnk` 和收录的 `.url`，显示名用 `SHCreateItemFromParsingName` 得到 `IShellItem`，再 `GetDisplayName(SIGDN_NORMALDISPLAY)`。失败或结果为空白时，用文件名去掉扩展名。快捷方式文件名（含扩展名）和目标 exe 的文件名（含扩展名）作为可搜索备用名，放进匹配引擎的别名字段，不作为结果上的名称。与显示名相同的不重复存。`.url` 没有目标 exe，只加它自己的文件名。

递归扫描开始菜单时，用 `SHGetKnownFolderPath` 取 `FOLDERID_Startup` 和 `FOLDERID_CommonStartup`。目录的规范化路径与其中之一相同则整目录跳过。不按「Startup」或「启动」这些名字判断。测量用的额外目录使用同一组路径，只有扫到这两个已知文件夹才跳过。

卸载程序默认不收录，没有开关。规则对应 Flow Launcher `Plugins/Flow.Launcher.Plugin.Program/Main.cs` 的 `HideUninstallersFilter`，但始终生效。目标文件名（不含目录）忽略大小写等于 `uninst.exe`、`unins000.exe`、`uninst000.exe` 或 `uninstall.exe` 时不收录。目标文件名以该处的前缀开头并且以 `.exe` 结尾时不收录。显示名以前缀开头时不收录。快捷方式文件名以前缀开头并且以 `.lnk` 结尾时不收录。前缀包括 `uninstall`、`卸载`、`卸載`，以及该文件里其他语言的对应词；同一拼写只保留一次。只作用于开始菜单、App Paths 和 PATH。商店应用和用户添加的便携应用不过这道过滤。

收录的 `.url` 只取 `[InternetShortcut]` 段内的第一条非空 `URL=`。地址以 `steam://run/`、`steam://rungameid/` 或 `com.epicgames.launcher://apps/` 开头，且前缀之后还有剩余，才进入索引。比较前缀时忽略 ASCII 大小写。普通 `http` / `https` 以及其他协议不收。去重键是整段地址的 ASCII 小写。启动时把这段地址交给 `ShellExecuteExW`，动词 `open`。不要求本机存在对应的 exe。

`AppSource::ALL` 在四种系统来源之后加上 `Portable` 和 `Alias`。系统来源仍由枚举填入。便携应用和别名来自数据目录的 `user-apps.json`，不由 Windows 枚举产生。便携应用是一条路径目标，来源记为 `Portable`。`kind` 不是路径的便携条目跳过并记日志，不进入索引。别名是一个名称加一个启动目标，来源记为 `Alias`。便携应用与系统来源是同一启动目标时只显示一条，名称用用户填的名称。去重时便携应用不另成一条：系统条目在前时，显示名改成用户填的名称，原来的名称进入备用名；便携应用在前时，后到的系统来源不改显示名。两条系统来源仍保留先出现的名称。别名只并入备用名。别名的目标不在索引里时是否单独显示，待定；最小实现不单独成条。

`user-apps.json` 见「数据」。JSON 的 `schemaVersion` 为 1，字段是 `portable`、`aliases`、`hidden`。缺少 `schemaVersion` 时按 1 读取。文件不存在视为空目录。无法解析，或 `schemaVersion` 更高时返回错误，不改名、不隔离。当前 `load_user_catalog` 和 `save_user_catalog` 仍由调用方传入路径。`AppIndex::open_from_process` 不猜测路径，内存中的目录开始是空的。`set_user_catalog` 替换这份内存并只重建便携应用和别名两个来源。在此之前若后台重建已经用空目录跑完，缓存里残留的便携应用和别名会被清掉。调用方要在打开索引之后把目录设进来。`hide` 和 `restore` 只改这份内存。落盘仍由调用方保存。隐藏按启动目标的去重键。`entries` 仍包含已隐藏的应用。搜索必须走 `query`，那里丢掉隐藏键。`hidden_apps` 把目录里的隐藏项交给设置页；索引里还能找到同一键时，用当前显示名。

有路径的应用可以「以管理员身份运行」和「打开所在文件夹」。前者用 `ShellExecuteExW`，动词 `runas`，掩码与普通启动相同，会交给系统的提权提示。后者返回目标 exe 的父目录，不是快捷方式所在的开始菜单目录；打开这个目录的窗口不在本模块。商店应用的目标只有 AUMID，启动串是 `shell:AppsFolder\<AUMID>`。`runas` 不能用来提权打包应用，枚举也拿不到可以交给资源管理器的普通文件路径，包目录通常在受保护的 WindowsApps 下。因此这两个动作只对有本地文件的路径目标提供。商店应用和游戏链接等没有本地文件的结果不显示「以管理员身份运行」和「打开所在文件夹」，对应快捷键无效。快捷键是 Ctrl+Shift+Enter 与 Ctrl+Enter，见产品规格。界面尚未接这两个入口。

待办与便签索引常驻内存，由写盘成功后的变更消息增量更新，查询时不读盘。只收未完成待办的标题，以及不在回收站中的便签的标题、标签和正文。

预计算中文、英文、别名、英文模糊匹配、拼音和首字母。拼音和首字母只用于应用名、待办标题、便签标题和标签；便签正文只做原文子串匹配。拼音表使用固定版本的 Unicode Unihan，读音取 `kMandarin` 与 `kHanyuPinyin` 的并集，去掉声调后去重，并带许可说明。`kMandarin` 只有常用读音，单独使用会漏掉多音字。不做双拼，不按词义猜测读音。查询使用共享索引。

匹配引擎已接在 `crates/core` 的 `search`。打分和索引机制如下。组内排序和最多 20 条的分组配额已写入 [product.md](product.md)「搜索」（规格缺口 #9 第 7 项已定）。从搜索结果打开时次数加 1，也已写入该节。引擎不保存次数。

### 匹配引擎

应用名、待办标题、便签标题、便签标签和别名用同一种 `PreparedCandidate`。调用方把应用名、待办标题和便签标题标成 `FieldRole::Name`，别名标成 `Alias`，标签标成 `Tag`，便签正文标成 `Body`。文件名不走这张拼音表。应用索引查询应用名时已经调用 `MatchIndex`（内部是 `prepare` 和 `query`）。文件索引在 `lanwork_core::files`，不调用 `prepare`。查询调度在 `lanwork_core::dispatch`，对应用、待办和便签调用同一个 `prepare` 和 `query_prepared`。

拼音表在构建 `lanwork-core` 时生成，查询时不下载、不解析 Unihan 原文，也不把查询里的汉字转成拼音。

- 数据来自 Unicode 18.0.0 的 `Unihan_Readings.txt`，文件内日期 2026-07-31。下载地址是 `https://www.unicode.org/Public/18.0.0/ucd/Unihan.zip`。上游全文 SHA-256 为 `9d39995b5de714e8ce93716ed5d15eaa0792d68e407cdf8a0add2893b8f4150b`。仓库里放的是摘录 `third_party/unihan/kMandarin_kHanyuPinyin.txt`：只留 `kMandarin` 与 `kHanyuPinyin`，且码位在扩展 A 或基本区，数据行原样复制。摘录 SHA-256 为 `a5ad0750009db5a4461efc9c66a87c359b9cf7e6e972a6172ba45c673556fa18`。构建时校验摘录，不一致就失败。重现步骤在 `third_party/unihan/extract.py`。
- 许可是 Unicode License v3，全文在 `third_party/unihan/LICENSE.txt`。
- 覆盖 CJK 扩展 A（U+3400–U+4DBF）和 CJK 统一汉字基本区（U+4E00–U+9FFF）。扩展 B 及以后不收入。这一版覆盖 26,711 个有读音的码位、419 个无声调音节。表里没有的字只参与原文匹配。
- 同一码位先保留 `kMandarin` 的书写顺序，再追加 `kHanyuPinyin` 里去声调后还没有的读音。文件里 `kHanyuPinyin` 行排在前面，生成时不沿用这个行序。两个 `kMandarin` 值都保留。不按词义删除读音，多个读音在能否命中上同等。
- 声调符号去掉。`ü` 和带声调的 `ü` 写成 `v`。`ê` 写成 `e`。不做双拼，也不把查询里的声调字母折成无声调拼音。
- 个别读音在源数据里是两个音节连写、中间没有分隔，例如 U+74F2 的 `túnwǎ`。去声调后仍是一个字符串，不猜测切分。
- 生成表映射进进程的数据是 207,860 字节：音节字节、音节偏移、两段码位索引、读音块。这不是「性能测量」里的 Private Bytes。

每个可拼音字段预计算四样东西：归一化原文、去空白形式、按字存放的音节 id、ASCII 词段。不生成多音字的全组合字符串。

- 归一化：全角 ASCII（U+FF01–U+FF5E）折成半角，全角空格折成普通空格，再做 Unicode 小写。不做其他变音折叠，例如 `café` 不会变成 `cafe`。
- 去空白形式是删掉归一化原文里的全部 Unicode 空白。和原文相同时不另存。
- 连续的 ASCII 字母数字是一个词。有读音的汉字记下全部音节 id。没有读音的汉字，以及假名、谚文和其他非 ASCII 字母，记成间隔；拼音和首字母不能跨过间隔。标点只是分隔。
- 正文不建词段和音节，只留原文。

查询先去掉两端空白。剩下没有非空白字符时返回空列表。`query_prepared` 输出命中类型和分数，不截断到 20 条，也不在各组之间分配名额。排序和名额在 `rank_hits`、`allocate_display`。

| 类型 | 条件 | 分数 |
| --- | --- | --- |
| `Exact` | 归一化原文相等，或去空白后相等 | 去掉首尾空白并归一化之后的查询的 Unicode 标量值个数，内部空白计入 |
| `Prefix` | 查询是上述两种形式之一的前缀 | 同上 |
| `Substring` | 查询是上述两种形式之一的子串 | 同上 |
| `Pinyin` | 从某个词段起，用预存音节把查询对齐完，且至少用到一个汉字。最后一个音节可以只匹配前缀 | 同上 |
| `Initial` | 查询的每个字母对齐连续词段的一个首字母。汉字取其任一读音的首字母，英文词取第一个字母 | 同上 |
| `Fuzzy` | 查询每个字符按顺序在归一化原文里命中一次 | 见下 |

同一字段的原文三类只返回最具体的一种。全拼、首字母和英文模糊只在字段不是正文、且查询去掉空白后只含 ASCII 字母数字时才做。纯英文词的对齐不算全拼。同一字段可以同时返回多种命中。别名和标签用 `FieldRole` 与字段序号区分。

英文模糊的分数只在 `Fuzzy` 内部可比：每个命中字符 +1；与上一命中在原文中相邻再 +2；命中点是词首再 +2。词首指串首，或前一字节不是 ASCII 字母数字。取最高分的对齐。其他类型的分数不和模糊分数比较。

`query_prepared` 的顺序是候选项输入顺序、字段输入顺序，然后按原文、全拼、首字母、模糊枚举。这不是组内排序，上面的分数也不参与组内排序。

`rank_hits` 做组内排序。同一个候选项 id 只留一条，命中类型取完全匹配、前缀、全拼、首字母、子串、模糊里最前的一种。然后按这个类型排序。同一种类型里，调用方经 `FrequencySource` 传入的使用频率高的在前。类型和频率都相同，则保持该候选项在输入里第一次出现的相对顺序。引擎不保存使用次数。次数如何加 1 已写入产品规格。本模块仍不把次数写入磁盘，由调用方传入。已接入的便签服务不保存这个次数。

`allocate_display` 按浏览器、应用、待办、便签、文件、文件夹收已经排好的各组结果。每组先取至多 8 条。20 条还没满时，再按同一顺序把前面组剩下的结果取完，再取后面的组，直到满 20 条或没有更多结果。同一组入选的条目在输出里挨在一起。浏览器组只由调用方在输入是有效 `http` / `https` 时放入。

`cargo bench -p lanwork-core` 用 5,000 个应用名和 10,000 条待办标题测量准备和查询。这组基准不按「性能测量」采样，不能当作应用结果 P95 已经达到。这组语料的 `MatchIndex::heap_bytes()`（各 `Vec` 和 `String` 的 capacity 之和，不含拼音表，不含分配器额外开销）在 Linux 的 debug 构建里是 3,563,603 字节。分配器取整会改变这个数，测试只要求它小于 16 MiB。

文件和文件夹优先使用 Everything，支持 1.4 和 1.5。随包附带 voidtools 的 Everything SDK x64 DLL：1.4 用 SDK 的 DLL，1.5 用 SDK3 的 DLL。两者固定版本，放在程序目录，并附许可说明。每次查询前按 1.5、1.4 的顺序检查状态并探测，不为每次查询启动命令行。

Everything 未运行或未就绪时，退回 Windows Search 索引。查询只匹配文件名，不匹配正文和属性，范围是 Windows 已建立索引的位置。Windows Search 服务不可用时报告文件索引不可用。Everything 恢复就绪后，下一次查询改回 Everything。两种来源都不复制全盘索引。有效 `http` / `https` 成为浏览器动作，无效地址丢弃。打开交给 Windows Shell。

Windows Search 的 spike 当前做法是进程内 ADO `ADODB.Connection`，提供程序 `Search.CollatorDSO.1`，SQL 含 `System.FileName LIKE`，`TOP` 不超过 50。匹配方式（子串、前缀或整名，以及查询里的 `*`、`?` 和 `LIKE` 的 `%`、`_` 是否同义）待定；产品规格只写到按名称。`ISearchQueryHelper::GenerateSQLFromUserQuery` 的默认语句是 `CONTAINS(*)`，会返回正文命中，不作为产品查询。`ISearchManager` 只用来对照 SQL 和列出索引根。这次做法只说明技术验证里测通的调用，不表示「性能测量」的 P95 已经达到。

文件索引服务在 `lanwork_core::files`，薄命令是 `FileCommands`。界面尚未调用。查询队列、60ms 等待、过期序号和分组合并不在这里；`query` 只把调用方传入的序号原样带回。

每次查询前先检查 Everything，再决定要不要问 Windows Search。状态只有就绪、未就绪、未运行。Windows Search 只有可用、不可用。1.5 的 DLL 能连上但数据库还在加载时是未就绪，不再改问 1.4。1.5 未运行时才探测 1.4。1.5 先连未命名实例，管道不存在再连 `1.5a`。已经连上的客户端在下次查询前只调用 `IsDBLoaded`。探测失败按未运行，日志写服务端版本；读不到时写「未知」，不把 `GetLastError` 的 0 改写成管道不存在。DLL 缺失记一条日志，Everything 按未运行。两个 DLL 都按文件名放在程序目录：`Everything3_x64.dll`、`Everything64.dll`。开发构建在程序目录找不到时，再找仓库 `third_party/everything/`。发布构建不回退到编译机路径。不为查询启动进程，也不复制索引。

Windows Search 沿用上面的 ADO 语句，`TOP 50`。连接字符串能从 `ISearchManager` 读到就用那条，否则用 `Provider=Search.CollatorDSO.1`。服务状态不是正在运行时记为不可用，不打开连接。状态读不到时仍尝试查询。查询失败时同样记为不可用，日志可以带 HRESULT；空的异常 `message` 不作为界面说明。两者都不可用时，结果上的说明只有「文件索引不可用」。查询成功但没有命中是空列表，不是这句说明。`System.ItemType` 等于 `Directory` 标成文件夹，其余标成文件。这是当前实现选择。匹配方式仍待定。

状态检查为就绪但这次查询调用失败时，本次改走 Windows Search，并记日志。规格没有单独写这一步。`Everything3_ConnectW` 在数据库加载时可能长时间不返回，规格没有超时，这里不加。只有 `WSearch` 正在运行才当作可用；正在启动算不算可用，规格没写。

程序目录还要带上两份许可，文件名是 `Everything-SDK3-LICENSE.txt`、`Everything-SDK-LICENSE.txt`，来源是 `third_party/everything/` 里对应的 `LICENSE.txt`。安装程序还没做，拷贝不在本服务里。清单是 `PROGRAM_DIRECTORY_FILES`。

搜索条的输入先经 `lanwork_core::search::classify_prefix` 判断收集前缀。该函数只区分空输入、搜索、待办收集和便签收集，并给出前缀之后的剩余文本与是否可提交；不解析日期、不创建记录、不入查询队列。面板搜索框不调用它。待办收集的剩余文本再交给 `parse_todo_due_prefix`，今天的日期由调用方传入。命中前缀时不进入查询队列，尚未返回的查询结果丢弃，只做日期前缀解析和预览；未提交的收集文本保存在内存里，退出进程时不保留。

查询队列只保留一个待处理请求，新输入替换旧请求。每次查询有序号，过期序号的结果必须丢弃。应用、待办和便签结果先返回。最后一次输入后 60ms 内没有新输入，才向 Everything 或 Windows Search 发请求并合并结果。每次最多接收 50 条，界面最多显示 20 条。只为可见项取图标。图标缓存同时不超过 128 项和 8 MiB，搜索和收纳共用。

查询调度的代码在 `lanwork_core::dispatch`，类型是 `Dispatch`。搜索条已调用它，面板还没有。`TodoCommands::boot` 成功并且 `NoteCommands::open` 在导入恢复之后完成，才调用 `Dispatch::build`。`Store::build_index` 在启动未完成时不会建索引。索引读的是这两份服务的内存：未完成且不在回收站的待办标题，以及未软删除便签的标题、标签和正文。之后只在 `EntityChanged` 的种类是待办或便签时，从同一份内存重建对应索引。查询不读盘。交给 `Dispatch` 的必须是正在写入的那一份命令；克隆仍是同一份内存。待办在提交批次之前写入内存。便签在写盘或删文件成功之后、发布变更之前更新内存。订阅者读到消息时，这两份内存已是新值。保存或永久删除便签的回调里不要再调用便签写入，写入锁还被这次操作持有。

`submit` 的时钟读数是结果延迟的起点。搜索条先经 `classify_prefix`。空输入和收集前缀不进入查询队列，并使当前序号失效。面板不分类：空字符串是空输入，其余按搜索。组合中的预编辑不要调用 `submit`。序号从 1 递增。本地分组写好之后，并且距这次 `submit` 已满 60ms、期间没有更新的输入，才调用文件来源。文件查询返回时序号已经变了就不合并。不可用时结果上的说明是「文件索引不可用」，已有的应用、待办和便签行保留。文件服务最多交回 50 条，显示用 `allocate_display` 收到最多 20 条。

有效地址沿用 `is_http_source_url`。应用、待办和便签的组内顺序用 `rank_hits`。`record_open` 把对应键的次数加 1，只放在本进程内存里，下一次查询才参与排序。`LatencyRecord::to_json_line` 的字段与 `tools/sample` 的延迟输入一致。终点由调用方用 `mark_rendered` 填上同序号、同一来源的 `end_ns`。被取代的记录单独标出；已经填了终点的记录不再标为被取代。热召回不在这一层。没有按「性能测量」采样时，这些记录不表示目标已经达到。

图标缓存是 `IconCache`。`Dispatch::icon_cache` 把同一份缓存交给收纳。取图只针对当前可见行。Windows 上用 `IShellItemImageFactory::GetImage`，只要图标并缩放到请求边长，再用 `GetDIBits` 读成 RGBA。非 Windows 上取不到图。下面几项规格没有写死，当前做法如下，不把它们当成产品规则：请求边长是 32 像素；快捷方式图标序号只区分缓存键，取图接口没有这个参数；使用次数不落盘；待办、便签和浏览器行没有可交给壳层的路径，不取图标；文件和文件夹保持文件来源的返回顺序，不按次数重排。

## 收纳

服务在 `lanwork_core::shelves`，薄命令是 `ShelfCommands`。一个分组一个 `shelves/<id>.json`。分组只保存路径引用。服务不调用删除、移动、复制用户文件的接口。界面拖放按本节末尾的原生 OLE 方案。外部拖放的通过条件表见 [roadmap.md](roadmap.md)，七条都已通过。收纳界面仍未写。

文件字段：`schemaVersion`、`id`、`name`、`order`、可选的 `todoId`、`refs`。引用字段：`path`、`name`、`folder`、`addedAt`。`addedAt` 是 Unix 纪元起的 UTC 毫秒。不保存文件内容和图标。

### 路径

引用必须是 Windows 绝对路径。相对路径和盘符相对路径（`C:foo`）拒绝。先规范化，再保存和比较：

- 把 `/` 换成 `\`。
- 展开 `\\?\`。`\\?\UNC\`（`UNC` 不区分大小写）改写成 `\\server\share\...`。其他 `\\?\` 只去掉前缀，留下盘符路径。`\\?\Volume{...}` 和 `\\.\` 拒绝。
- 去掉末尾的 `\`。盘符根保留 `C:\` 里的根分隔符，避免变成盘符相对路径 `C:`。UNC 共享根写成 `\\server\share`。
- `.` 和 `..` 按路径段处理，不用字符串前缀裁剪。空段和 `.` 丢掉。`..` 弹出上一段；已经在盘符根或 UNC 共享根上时留在根上。因此 `C:\A\..\AB\x` 与 `C:\AB\x` 相同，`C:\A` 不是 `C:\AB` 的前缀，段名 `foo..` 也不是上级目录。
- 去重键是规范化结果的逐字符简单大写，近似 NTFS 大写表。每个字符取 `char::to_uppercase`，只有结果恰好是一个字符时才替换，否则保留原字符。不按区域设置折叠，也不做随位置变化的 Σ 小写。因此 `İ` 不与 `i` 合并，`ẞ` 不与 `ß` 合并；`σ`、`ς` 与 `Σ` 合并，`ı` 与 `i` 合并。同一分组内键相同则不新增，保留先加入的那条的大小写、显示名、`folder` 和 `addedAt`。不同分组可以各有一条。
- 落盘的 `path` 用规范化形式，分隔符是 `\`，大小写取第一次加入时的写法。
- 显示名取最后一段。没有更短的段时，盘符根用整个 `C:\`，UNC 共享根用共享名。产品规格没有另写显示名，这是存储时的取值。
- `folder` 由调用方传入。`add_refs` 不探测用户路径，避免网络路径在拖入时挂起。

非 Windows 的测试构建还接受以单个 `/` 开头的绝对路径，用 `/` 做同样的段解析，只为在临时目录里断言原文件仍在。Windows 构建不接受这种路径。

### 写入

`add_refs` 的分组 id 为空，且当前没有分组时，新建名为「收纳」的分组，和引用放在同一个文件里一次写入。写盘失败则磁盘上没有这个文件，内存里也没有这个分组，不发布 `EntityChanged`。已有分组但 id 为空时返回错误，不猜测目标。产品规格没有写未选中分组时拖入到哪里。路径列表为空时不新建分组。一批路径里有一条无法规范化，则整批不写。

名称去掉首尾空白，空白则拒绝。分组名称不要求唯一。

删除分组和移除引用只通过存储层改分组数据文件。

一次操作改多个分组文件时，用存储层批次。后面的写入失败，就把本批次已经替换的文件写回操作前内容，不提交、不发布变更，内存保持操作前状态。回滚再失败时重新读入；读入也失败则清空内存，避免继续用和磁盘不一致的分组。

`EntityChanged` 的 `revision` 为 None。回调在存储写锁释放之后执行，此时内存已是新状态。回调里不要再调用本服务的写入。

### 待办关联

一个分组的 `todoId` 最多一个。已经关联另一条时再次关联返回错误，不替换。产品规格没有写是拒绝还是替换。同一条重复关联不写盘。一条待办可以被多个分组关联。按待办 id 解除时，清掉所有指向它的分组。

关联只校验待办 id 是不是合法的文件名分量，不检查这条待办是否存在。产品规格没有写关联到不存在的 id 时怎么处理。

服务在 `watch_todo_notices` 里先订阅 `TodoNotice::Purged`，再按 `todo-purge-pending.json` 补解除。只解除记录中已经不在任何清单里的待办 id，写盘成功后从记录去掉。待办还在清单里时不解除。不在记录里的关联不动，因此不会把尚不存在的待办 id 当成已永久删除。订阅前发出的 `Purged` 不会留下；进程在待办写盘之后中断时，下次订阅仍能补上。解除写盘失败时记录还在，下次订阅再试。读取时不把残留 id 视为无关联。

待办完成或进入回收站不改变分组。服务不因这两件事解除关联。

### 存在性

`check_exists` 对每条路径在单独的后台线程里调用 `try_exists`。不打开、不复制、不移动、不删除。每条上限是 2 秒（`EXISTS_TIMEOUT`）。查询没有在上限内返回，包括已断开的 UNC，该条视为不存在。本地路径用同一上限，避免断开的盘符把调用挂住。无法规范化的路径也视为不存在。权限错误和超时都不另报。产品规格要求失效路径显示「路径不存在」，超时按这个结果返回。

界面只对可见条目调用，并在界面线程之外拿结果。不监视文件系统。

### 拖放

外部拖入和拖出用原生 OLE。Slint 1.18.1 的窗口内拖放不能代替它，原因见「运行时」。2026-10-09 的资源管理器手测见 [roadmap.md](roadmap.md)。`24b7624` 上长路径拖入失败；同日 10:56（UTC+8）在 `3311ecc` 上拖入通过。同日 14:10–14:24（UTC+8）条件 7 通过，次数是用户估计。`--slint` 拖放没有路径。通过条件表七条都通过，该项按该表通过，产品用 OLE。下面是 `spikes/dnd` 里已经观察到的做法。

- 拖入：事件循环线程调用 `OleInitialize`。本机该线程是 `APTTYPE_MAINSTA`（主 STA），不是 MTA。`MainWindow::new` 之前 COM 尚未初始化；进入 winit 事件循环后由 winit 初始化。拿到 HWND 后 `RegisterDragDrop` 返回 `DRAGDROP_E_ALREADYREGISTERED`，于是 `RevokeDragDrop` 再注册自己的 `IDropTarget`。注册之后窗口属性 `OleDropTargetInterface` 的指针与自己的接口相同。只接受 `CF_HDROP`。指针在收纳区域外返回 `DROPEFFECT_NONE`；区域内只从源提供的效果里选复制，其次快捷方式，不返回移动。验证程序没有标签，用一个矩形代替收纳区域，区域外的判定有单元测试。产品里仍只在收纳标签显示时接受放下。
- 拖出：按下后，物理像素位移严格超过 `SM_CXDRAG` 或 `SM_CYDRAG` 才调用 `DoDragDrop`。`IDataObject` 用 `IShellItemArray::BindToHandler(BHID_DataObject)`。允许的效果只有 `DROPEFFECT_COPY | DROPEFFECT_LINK`。取消由 `IDropSource::QueryContinueDrag` 返回 `DRAGDROP_S_CANCEL`。`DoDragDrop` 的模态循环会吃掉鼠标抬起，列表行的 `TouchArea` 会一直抓着鼠标，下一次按下仍落在刚才那一行。拖出结束（成功、取消或出错）后要向窗口补 `PointerReleased` 和 `PointerExited`。自动化没有调用 `DoDragDrop`：它要等键盘或鼠标状态变化才会第一次询问是否继续，调用会作用到光标下的窗口。自测用 `dispatch_event` 先拖长路径那一行，再拖文件夹那一行，第二次必须是文件夹。
- 不解析 `.lnk`。合成的 `CF_HDROP` 能读回超过 260 个 UTF-16 单元的路径，`DragQueryFileW` 先问长度再分配。Shell 的 `IDataObject` 对 269 个 UTF-16 单元的路径调用 `GetData(CF_HDROP)` 仍返回 `0x8007007A`：外壳用固定 260 单元组 `CF_HDROP`，这时还没有 `HDROP`。失败后改用 `SHCreateShellItemArrayFromDataObject` 和 `IShellItem::GetDisplayName(SIGDN_FILESYSPATH)`，单元测试读回了原路径，快捷方式路径没有被解析成目标。2026-10-09 10:56（UTC+8）在 `3311ecc` 的 `--ole` 上，资源管理器把这条长路径拖进红色矩形，控制台给出完整 `paths=`，`effect=1`，没有再出现 `0x8007007A`。这次走的是上述外壳项退路。
- 路径是否存在只对可见条目调用上面的 `check_exists`，结果回到界面线程更新。不监视文件系统。
- 不实现物理文件夹同步、文件操作撤销栈和桌面嵌入。

## 更新

安装版使用 minisign 签名和 `latest.json`。WinHTTP 分块下载到临时文件，流式验签成功后才启动安装程序。签名错误或下载失败不启动安装。界面提供检查、进度、安装和重启。

便携版不在运行中替换自身，只打开对应的 Release。安装程序不安装 WebView2，也不安装 Windows App Runtime。安装版的开始菜单快捷方式和通知激活器按「通知」注册。签名私钥不进入仓库。没有从旧安装目录完成的一次升级记录时，测试通过不算升级验收。

## 性能测量

下面的数字是测量协议和目标，还没有一次符合协议的记录。空 Slint 窗口的采样记在 [roadmap.md](roadmap.md)，那次没有加载固定数据，不填进下表。在有一次符合协议的记录之前，文档和界面都不写「已达到」。禁止为了凑数字裁剪工作集。Everything 客户端、Everything 服务、Windows Search 索引服务和安装程序单独记录，不计入 Lanwork 进程。随包的 Everything SDK DLL 加载在 Lanwork 进程内，计入。

场景：固定数据为 5,000 个应用、10,000 条待办、1,000 篇约 2 KiB 的便签、20 个收纳分组共 1,000 条引用。预热搜索条和面板搜索，打开收纳标签，在搜索条里各提交一次待办和便签收集，并打开再关闭便签悬浮窗。然后收起全部窗口，连续采样 5 分钟。

| 项 | 目标 |
| --- | --- |
| 进程 Private Bytes 峰值 | ≤ 100 MiB，同时记录工作集 |
| 热召回 P95 | ≤ 100 ms |
| 应用、待办、便签结果 P95 | ≤ 50 ms |
| Everything 就绪时文件结果 P95 | ≤ 150 ms |
| Windows Search 回退时文件结果 P95 | 记录，不设目标 |
| 冷启动、首次索引 | 单独记录，不并入上面的 P95 |
| 收起后空闲 5 分钟 | 记录平均 CPU 和每秒唤醒次数 |
| 同时打开 5 个便签悬浮窗 | 单独记录 Private Bytes 增量 |
| 100 次呼出、查询、开关辅助窗口 | 记录 Private Bytes、USER、GDI 是否持续上升 |

GitHub 快照不放进这组固定数据。刷新 GitHub 时另记内存。

口径：

- **热召回**：从进程收到热键消息，到搜索条第一次渲染完成（Slint `set_rendering_notifier` 的 `AfterRendering`）。不含 DWM 合成到屏幕的时间。
- **结果延迟**：从输入框文本变化（组合中的预编辑不算），到带同一查询序号的结果第一次渲染完成。文件结果包含 60ms 等待。被新输入取代的查询不计入样本，单独记数量。
- **样本**：每项至少 100 个有效样本，排除预热，保存原始时间戳。P95 取升序第 ⌈0.95 × N⌉ 个。Everything 和 Windows Search 分开统计。
- **每秒唤醒次数**：Lanwork 所有线程每秒上下文切换次数之和（性能计数器 `\Thread(lanwork*)\Context Switches/sec`）。
- 记录写明 release 或 debug、Slint 精确版本、渲染器、commit、机器、Windows build、GPU 与驱动、数据规模。
