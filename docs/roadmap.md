# 实现状态

行为以 [product.md](product.md) 为准。本文只记录有没有代码和验证记录。工程骨架已在仓库中。搜索条前缀分类的纯函数在 `crates/core`（`search::classify_prefix`）。匹配引擎也在 `crates/core` 的 `search`。待办收集的日期前缀解析也在 `crates/core`（`parse_todo_due_prefix`），包括已写入产品规格的星期、下周和月底边界。规格缺口 #9 的第 4 项和第 7 项已定，其余项仍待定。便签服务在 `crates/core`（`lanwork_core::notes`）。待办服务在 `crates/core`（`lanwork_core::todos`）：清单、收件箱、周期、软删除与恢复、当前标记、处理模式和跨清单移动已有代码；永久删除、原清单不存在时的恢复，以及短月没有对应日、在截止日当天完成是否再生成，仍等产品规格。收纳服务在 `crates/core`（`lanwork_core::shelves`）：分组、路径引用、去重、待办关联和存在性检查已有代码。已有分组但未指定目标、再次关联另一条待办、关联到尚不存在的待办 id，仍等产品规格。残留关联 id 在读取时视为无关联，并在下次写该分组时清除；该清除在永久删除落地时实现。GitHub 服务在 `crates/core`（`lanwork_core::github`）：本机 `gh` 的探测与调用、watchlist、刷新与快照、长期未更新和 Draft、转为待办、隐藏规则和来源同步已有代码。追踪仓库的添加与移除等 #9 第 17 项，以及需要处理、需要 Review、CI 失败和筛选组合等 #9 第 18 项，仍等产品规格。应用索引在 `crates/core` 的 `apps`。便携应用、别名和手动刷新入口仍等规格。测量工具的代码已在仓库中，协议记录仍空着。Everything 与 Windows Search 的记录见下表。外部拖放有一份未通过的验证记录，资源管理器方向的通过条件留空。其余技术验证项没有通过记录。搜索条界面、待办界面、收纳界面和收集提交也还没有代码。界面表里的各项仍全部未实现。

## 测量工具

| 项 | 状态 |
| --- | --- |
| `tools/fixture`、`tools/sample`、[docs/measurements/TEMPLATE.md](measurements/TEMPLATE.md) | 代码已在仓库中 |
| 符合架构「性能测量」协议的记录 | 无 |
| Windows 11 桌面上对空 Slint 窗口采样 5 分钟 | 2026-10-08，commit `0b7fd5f`，release。脚本通过：300 行，首尾 298.989 秒。Private Bytes 峰值 49,729,536 字节，工作集峰值 77,799,424 字节，平均 CPU 0.204%，唤醒平均 69.150/秒，句柄 326–343，USER 23，GDI 17。见 [measurements/2026-10-08-empty-window.md](measurements/2026-10-08-empty-window.md)。不是性能协议验收 |

## 技术验证

各项的通过标准见 [architecture.md](architecture.md)「技术验证」。

| 项 | 记录 |
| --- | --- |
| 中文输入法 | [记录](verification/ime.md)。100% 缩放下用户报告单行、多行、候选窗、拖动后的候选窗、Enter 和数字键没有问题。150% 未测，该项未通过 |
| 背景 | 部分记录，未通过。见 [render-2026-10-08.md](measurements/render-2026-10-08.md)。2026-10-09 在 DESKTOP-7C3P6OG（Windows 11，屏幕 1280×800）上，FemtoVG、软件渲染、Skia 软件、Skia OpenGL 的窗口都是磨砂、能透出背景、没有黑色客户区。系统浅色/深色主题切换通过，深色下也清楚。关闭/打开「透明效果」通过。节电模式通过。渲染器未选定，这一项仍未通过。真实透明渲染失败没有新的记录。独立显卡机器未测。观察（不是失败判定）：自定义标题栏导致窗口拖不动；有两个关闭按钮 |
| 渲染器 | 部分记录，未通过。同上。2026-10-09 四种窗口的文字清楚（含中文和 0OIl1）。没有选定渲染器，也没有写进 architecture.md「运行时」。独立显卡机器未测 |
| 外部拖放 | 未通过。记录见下方「外部拖放」。资源管理器方向的通过条件留空，等人工填写。 |
| Everything | [fileidx.md](measurements/fileidx.md)：1.4 与 1.5 都返回了文件名，并区分了未运行和未就绪 |
| Windows Search | [fileidx.md](measurements/fileidx.md)：文件名查询、正文不返回、P95 和 Private Bytes 已记下。`WSearch` 停止时的错误码未测 |
| 通知 | 有记录，未通过。见下方「通知（#7）」 |
| 热角 | 2026-10-08，`spikes/hotcorner`，release。停留一次、离开约 340ms、离开后再进入，以及静止 5 分钟的三种采样，在 1920×1200 的屏幕上有记录。2026-10-09 在 DESKTOP-7C3P6OG（Windows 11，屏幕 1280×800）上，钩子和 50ms 轮询各做了 5 分钟鼠标正常移动的采样（各 300 行）；100ms 轮询未测。拖动窗口、多显示器、全屏未测。没有选定机制，不算通过。见 [measurements/2026-10-08-hotcorner.md](measurements/2026-10-08-hotcorner.md) |

## 通知（#7）

这项没有通过。下面有任何一条不是「通过」，整项就不通过。2026-10-09 所有者在 `2758d18` 的 release、正常 `target` 上亲手做完步骤。安装版的显示和两种点击通过；便携版能显示，但运行中点击和退出后点击失败。因此整项仍不通过，#7 不关闭。便携版点击不可用要不要写进产品规格，由所有者决定，现为待定。本次没有改 [product.md](product.md)。

### 环境

| 字段 | 值 |
| --- | --- |
| 日期 | 2026-10-08 的自动检查；2026-10-09 激活修复的自动检查，以及同日所有者的人工步骤 |
| commit | 人工步骤跑在 `2758d1849ff4bd72ab1e1e7a5600c84376c481aa`，release，正常 `target`。该提交只补了记录里的哈希，程序与 `fa20baa2cf82952c4c0641bffe0e7431a90d42a3` 相同。spike 源码 rebase 之后是 `b598419`（rebase 前 `07888db93f9b90a8dee0a303783b57a5be3fbddb`）。2026-10-08 的自动检查跑在 rebase 前的那棵源码上 |
| Slint 精确版本 | 1.18.1。本 spike 不链接 Slint |
| 渲染器 | 不适用 |
| 机器 | 13th Gen Intel Core i5-13420H |
| Windows build | Windows 11 家庭中文版 64 位，DisplayVersion 26H2，10.0.26300.9550（CurrentBuild 26300，UBR 9550）。注册表 ProductName 仍写着 Windows 10 Home China |
| GPU 与驱动 | Intel UHD Graphics，驱动 32.0.101.6733（2025-04-02）；另有 Virtual Display Driver 11.30.4.434（2024-12-24）、GameViewer Virtual Display Adapter 15.6.5.199（2026-02-28） |
| 构建配置 | `cargo test` 为 debug。2026-10-08 的自动检查、未注册时的 `show` 回退为 release：`target\release\toast.exe`。2026-10-09 修复后的自动 `activation-check` 用了另一个 `CARGO_TARGET_DIR`。同日人工步骤用的是 `2758d18` 的 release、正常 `target` |

标识只属于这个 spike：AUMID `Lanwork.Spike.Toast`，CLSID `{DEC1C43B-2AAC-400F-A0B1-C17A05F2B409}`，显示名「Lanwork 通知验证」。快捷方式是 `%APPDATA%\Microsoft\Windows\Start Menu\Programs\Lanwork Toast Spike.lnk`。日志是 `%TEMP%\lanwork-spike-toast.log`。只写 HKCU，不写 HKLM，不用管理员。

### 自动观察

2026-10-08 在上述 release 程序上运行 `cargo run -p toast --release -- self-check`，进程退出码 0。随后 `status` 为「未注册」，快捷方式文件不存在，`HKCU\Software\Classes\AppUserModelId\Lanwork.Spike.Toast` 和 `HKCU\Software\Classes\CLSID\{DEC1C43B-2AAC-400F-A0B1-C17A05F2B409}` 都不存在。`clear-history` 再执行一次，打印「已清除该 AUMID 的通知历史」。这些都不是屏幕上看到通知。

| 观察 | 结果 |
| --- | --- |
| `SHQueryUserNotificationState` | 5（`QUNS_ACCEPTS_NOTIFICATIONS`）。这不是 Windows 11「请勿打扰」的测试，也没有改请勿打扰 |
| 清理后的判定 | 未注册 |
| 未注册时直接调用 `Show`（只在 self-check 里探测，不是 `show` 命令的路径） | `Show[0] = S_OK`，约 400ms 后 `GetHistoryWithId` 条数 = 1。没有看屏幕 |
| 未注册时 `Shell_NotifyIcon` ADD / MODIFY / DELETE | 三次都返回 TRUE。徽标只做了往返，没有停留，没有看托盘 |
| 安装版读回 | 判定为安装版。DisplayName、CustomActivator、LocalServer32（带引号的当前 exe）、快捷方式 AUMID 和快捷方式 CLSID 都与写入一致 |
| 安装版单次 `Show`，无 Tag | `S_OK`，历史条数 = 1 |
| 安装版同一 id 连续 3 次，无 Tag | 三次 `S_OK`，历史条数 = 3 |
| 安装版同一 id 连续 3 次，Tag=`todo-001`，Group=`lanwork-spike` | 三次 `S_OK`，历史条数 = 1 |
| 安装版 `unregister` 之后 | 未注册 |
| 便携版读回 | 判定为便携版。有 DisplayName。没有 CustomActivator、LocalServer32、快捷方式 |
| 便携版单次 `Show`，无 Tag | `S_OK`，历史条数 = 1 |
| 便携版同一 id 连续 3 次，无 Tag | 三次 `S_OK`，历史条数 = 3 |
| 便携版同一 id 连续 3 次，带上述 Tag/Group | 三次 `S_OK`，历史条数 = 1 |
| 便携版 `unregister` 之后 | 未注册 |
| 未注册时运行 `show todo-001` | 退出码 0。打印「注册缺失，不调用 ToastNotificationManager.Show，只显示托盘徽标」，约 15 秒后打印「托盘徽标已移除」。没有看托盘 |

历史条数是 `GetHistoryWithId` 的返回，不是通知中心里肉眼看到的叠放。`S_OK` 也不等于通知出现在屏幕上。

### 2026-10-09 人工点击时暴露的 spike 缺陷

所有者在安装版注册下开着 `serve`，`show todo-001` 的通知能显示。点击之后面板没有出现。`%TEMP%\lanwork-spike-toast.log` 没有 `COM Activate` 行。DcomLaunch 另外拉起了 `toast.exe -Embedding`。那个进程也只记了注册和「模拟面板启动」，没有 `Activate`。两个 `LanworkToastSpikePanel` 的 `IsWindowVisible` 都是 false，窗口样式是 `0x04CF0000`，没有 `WS_VISIBLE`。`status` 读快捷方式时报 `CoCreateInstance: 尚未调用 CoInitialize (0x800401F0)`。这些是 spike 自己的缺陷。下面的通过条件表没有因此改成通过。

根因有两处。类工厂和回调用了 `#[implement]` 的默认 agile，也就是 `IAgileObject` 加自由线程封送。通知平台在另一个进程里 `CoCreateInstance`。自由线程封送的数据包是进程内指针，跨进程解封送失败，所以正在运行的 `serve` 进不了 `CreateInstance` / `Activate`。SCM 再按 `LocalServer32` 启动 `-Embedding`，新进程是同一套工厂，`Activate` 还是到不了。`-Embedding` 确实进了 `serve`。AUMID、CLSID 和快捷方式里的 `ToastActivatorCLSID` 当时是一致的。另外，注册只用了 `REGCLS_MULTIPLEUSE`，发生在窗口和消息循环之前；STA 上的调用要等消息泵。面板只调用了一次 `ShowWindow`。控制台程序和 COM 拉起的本地服务器经常带 `STARTF_USESHOWWINDOW` 且 `wShowWindow = SW_HIDE`，第一次 `ShowWindow` 会改用这个值，窗口就一直没有 `WS_VISIBLE`。`status` 则是在没有 `CoInitializeEx` 的线程上 `CoCreateInstance`。

修复之后，激活器不再 agile。`CoRegisterClassObject` 使用 `REGCLS_MULTIPLEUSE | REGCLS_SUSPENDED`，窗口显示后 `CoResumeClassObjects`，再进入消息循环。`Activate` 里先写 `COM Activate launch=...`，再 `ShowWindow` 两次、`SetWindowPos(SWP_SHOWWINDOW)` 和 `SetForegroundWindow`。面板标题和状态含「已定位 todo-00N」。`status` 读快捷方式前初始化 COM；线程上已经有套间时不再多调用一次 `CoUninitialize`。

### 激活修复的自动观察

2026-10-09 在独立构建目录的 release `toast.exe` 上运行 `activation-check`，退出码 0。当时所有者的 `toast.exe` 仍在运行（`serve` 和 `-Embedding`），所以这条自检没有向 SCM 注册产品 CLSID，也没有改 HKCU。它注册的是只活在该进程里的探测 CLSID `B7E3A1C2-4D55-4E18-9A60-2F6C8D0E11A7`。

同一程序再用 `STARTF_USESHOWWINDOW`、`wShowWindow = 0`（`SW_HIDE`）启动一次，退出码仍是 0。日志里第一次 `ShowWindow` 之前窗口是隐藏的（两次 `ShowWindow` 的返回值都是 0），调用之后 `IsWindowVisible=true`，样式为 `0x14CF0000`。

| 观察 | 结果 |
| --- | --- |
| 进程内 `CreateInstance` + `Activate("todo-001")` | 日志有 `COM Activate launch=todo-001`。标题含「已定位 todo-001」，状态是「已定位 todo-001　买牛奶」。`IsWindowVisible=true`。这不是点击通知 |
| 另一个进程 `CoCreateInstance` 探测 CLSID，再 `Activate("todo-003")` | 日志有 `COM Activate launch=todo-003`。标题含「已定位 todo-003」，状态是「已定位 todo-003　交电费」。`IsWindowVisible=true`。产品 CLSID 这次没有注册，因为已有 `toast.exe` |
| `SW_HIDE` 启动后的窗口 | `activation-check` 退出码 0，创建后 `IsWindowVisible=true` |
| `status` | 退出码 0。日志有 `CoInitializeEx STA hr=0x00000000`，没有 `0x800401F0`。快捷方式的 AUMID 和 CLSID 能读回。判定是「部分注册」：`LocalServer32` 仍指向所有者正在运行的那个 exe，当前程序在另一个构建目录，路径对不上。没有改注册表 |

这次没有跑完整的 `self-check`，因为它会注销并重写 HKCU。所有者机器上的安装版注册保持原样。

### 通过条件

2026-10-09，所有者，`2758d18`，release，正常 `target`。安装版先 `register installed`，`status` 为「安装版」，`activation-check` 退出码 0。便携版是 `unregister` 之后 `register portable`，`status` 为「便携版」，没有 CustomActivator、LocalServer32、快捷方式。做完后已 `unregister`，`status` 为「未注册」。

| 条件 | 结果 | 所有者观察 |
| --- | --- | --- |
| 安装版：通知能显示 | 通过 | 所有者看到通知 |
| 安装版：进程运行中点击，面板定位到该条，并收到待办 id | 通过 | `serve` 运行中点击 `todo-001`。面板到前台，标题含「已定位 todo-001」，该行选中。日志 `02:40:12.576 COM Activate launch=todo-001` |
| 安装版：进程已退出后点击，重新打开并定位到该条，并收到待办 id | 通过 | 点击 `todo-002`。日志 `02:40:46.356` 的 `serve` 参数含 `-Embedding`，`02:40:46.450 COM Activate launch=todo-002`。所有者看到拉起并定位 |
| 便携版：通知能显示 | 通过 | 有横幅 |
| 便携版：进程运行中点击，面板定位到该条，并收到待办 id | 失败 | 面板没有定位，没有选中。日志没有任何 `COM Activate` |
| 便携版：进程已退出后点击，重新打开并定位到该条，并收到待办 id | 失败 | 点击 `todo-002` 没有拉起进程 |
| 同一待办多次提醒时，系统是否把通知叠放 | 已观察 | 安装版：无 Tag 的 `--repeat 3` 在通知中心是 3 条；`--tag todo-001` 是 1 条。便携版：无 Tag 3 条；带 Tag 1 条 |
| 请勿打扰打开时，通知如何表现 | 已观察 | 请勿打扰开启时，安装版 `show todo-003`：不弹横幅，通知中心里有这条 |
| 缺失注册时不崩溃，并退回托盘逾期徽标 | 通过 | `show todo-001` 的日志是「注册缺失，不调用 ToastNotificationManager.Show，只显示托盘徽标」，约 15 秒后「托盘徽标已移除，进程正常退出」。截图里托盘有蓝底、写着「1」的小图标。文字「逾期 1」是否在提示里，没有确认 |
| 托盘徽标代替到期通知 | 失败（不能代替） | 不把徽标算成通知通过 |

模拟面板在单元测试里会按 id 选中对应行：`todo-003` 选中「交电费」，未知 id 不选中，空参数保持等待。这只说明函数行为。安装版点击时，所有者看到了对应行被选中。便携版运行中点击没有选中。正式待办页的定位不在本记录里，属于 #25。

便携版运行中点击和退出后点击都失败。是否把「便携版不能点击定位」写进 [product.md](product.md)，由所有者决定，现为待定。这次没有改产品规格。

### 重测时要先卸掉旧的本地服务器

2026-10-09 第一次按新程序重编译时，一个旧的 `toast.exe -Embedding` 被 DcomLaunch 反复拉起，占住 exe，链接报 `os error 5`。先 `unregister`，再结束那个进程，然后才能编译。不要留着旧的 `-Embedding` 再 `register`。换过构建目录后也要重新 `register`，否则 `LocalServer32` 仍指向旧 exe。

上面的人工步骤已经在 `2758d18` 做过，结果见「通过条件」。若再做一遍，命令仍用 release。`activation-check` 不能代替亲眼看到的显示、点击、叠放、请勿打扰和托盘图标。

1. `cargo build -p toast --release`
2. 安装版显示：`cargo run -p toast --release -- register installed`，然后 `status`，确认判定是「安装版」。再 `cargo run -p toast --release -- show todo-001`。看通知中心或右下角有没有「待办到期」，正文含 `todo-001` 和「买牛奶」。把看到的写进「安装版：通知能显示」。控制台里的 `S_OK` 不算。
3. 安装版、进程还在时点击：另开一个终端，`cargo run -p toast --release -- serve`，让模拟面板保持打开。再 `cargo run -p toast --release -- show todo-003`。点击那条通知。要同时看到：面板可见并到前台；标题含「已定位 todo-003」；列表选中「todo-003 交电费」；`%TEMP%\lanwork-spike-toast.log` 有 `COM Activate launch=todo-003`。只收到这串字、列表没有选中该行，定位不通过。不要再出现第二个没有 `Activate` 日志的 `-Embedding` 进程。
4. 安装版、进程退出后点击：关掉模拟面板，确认 `serve` 已经退出。上一条通知会被点击消掉，所以再 `cargo run -p toast --release -- show todo-002`。点击它。应重新出现面板，并选中「todo-002 写周报」，终端有 `launch=todo-002`。冷启动时先有控制台窗口，这是 spike 用了控制台子系统，单独记一笔，不因此改判定。
5. 便携版不要沿用安装版的结果。先 `cargo run -p toast --release -- unregister`，再 `register portable`，`status` 必须是「便携版」，且没有快捷方式、CustomActivator、LocalServer32。然后按第 2、3、4 步各做一次，分别填便携版的显示、运行中点击、退出后点击。退出后点击如果拉不起进程，就记失败，不要补快捷方式再试。
6. 叠放：在安装版注册下执行 `cargo run -p toast --release -- show todo-001 --repeat 3`，看通知中心是三条还是被收成一条。再执行 `show todo-001 --repeat 3 --tag todo-001`，再看一次。便携版再各做一遍。历史条数已经写在自动观察里，这里只填眼睛看到的。
7. 请勿打扰：打开「设置 → 系统 → 通知」，打开「请勿打扰」。在安装版注册下 `show todo-001`。写下通知有没有出现、有没有进通知中心。然后关掉请勿打扰。不要改 Focus Assist 的注册表。
8. 缺失注册的托盘：`unregister` 之后 `show todo-001`。确认进程退出码是 0，没有弹出到期通知，托盘上出现蓝底、写着「1」的小图标，大约 15 秒后消失。文字「逾期 1」是否在提示里，没有确认。

第 2 到第 8 步已在 2026-10-09 做完。实际点的是哪条、通知中心里有几条、请勿打扰和托盘各看到什么，以「通过条件」表为准。表里没有写成通过的，不要补成通过。

### 清理

自动检查结束时，这台机器上的 spike 注册已经去掉。若通知中心还留着「Lanwork 自动检查」或「待办到期」，在通知中心里清掉，或运行 `cargo run -p toast --release -- clear-history`。

2026-10-09 所有者做完步骤后已经 `unregister`，`status` 为「未注册」。这会删除上面的快捷方式、AUMID 键和 CLSID 键，并尝试清除该 AUMID 的通知历史。不删除其他程序的通知。卸载产品时的清理属于 #31，不在这里做。

## 界面

| 界面 | 规格位置 | 代码 |
| --- | --- | --- |
| 搜索条、快速收集（搜索条内）、面板、搜索、待办、便签、收纳、GitHub、设置 | [product.md](product.md) 对应各节 | 无 |
| 番茄钟 | [product.md](product.md)「番茄钟」 | 无。未定参数补进产品规格之前不写 |

不存在第二份产品范围。新增界面时先改产品规格，再改本表。

## 无界面模块

| 模块 | 代码 |
| --- | --- |
| 匹配引擎 | 有。`crates/core` 的 `search`：拼音表、预计算、命中类型和分数，以及产品规格里的组内排序和 20 条分组配额。使用频率只作为排序输入，不在这里保存。频率如何增减仍待定 |
| 便签服务 | 有。`lanwork_core::notes`：模型、保存、标签、置顶、`revision` 冲突和软删除恢复。删除的界面入口和冲突后的选择等 [product.md](product.md) 写入 #9 第 10、16 项。测试 `one_mib_body_roundtrip_records_write_time` 打印 `note_write_1mib_ms`，只记录 1 MiB 正文的写盘耗时，不是性能测量验收 |
| 待办服务 | 有。`crates/core` 的 `todos`：清单、收件箱、周期、软删除与恢复、当前标记、处理模式、跨清单移动。永久删除、原清单不存在时的恢复，以及短月没有对应日、在截止日当天完成是否再生成，仍等产品规格 |
| 收纳服务 | 有。`lanwork_core::shelves`：分组、路径引用、去重、待办关联、存在性检查。服务不移动、不复制、不删除用户文件。已有分组但未指定目标、再次关联另一条待办是拒绝还是替换、关联到尚不存在的待办 id，仍待定。残留关联 id 在读取时视为无关联，并在下次写该分组时清除；该清除在永久删除落地时实现。存在性检查每条路径的上限是 2 秒 |
| GitHub 服务 | 有。`lanwork_core::github`：`gh` 探测与调用、watchlist、刷新与快照、长期未更新、Draft、转为待办、隐藏规则、来源同步。添加和移除追踪仓库等 #9 第 17 项，需要处理、需要 Review、CI 失败和筛选组合等 #9 第 18 项，仍等产品规格。界面仍无 |
| 应用索引 | 有。`crates/core` 的 `apps`：开始菜单、App Paths、PATH、商店应用、去重、缓存和启动。便携应用和别名、手动刷新的界面入口等规格缺口 #9 第 1、2 项 |

## 外部拖放

结论：**未通过**。七条通过条件都要人工用鼠标对资源管理器操作，结果栏留空。自动化观察到的内容写在后面，不充当这七条的「通过」。按 [#33](https://github.com/wynxing/Lanwork/issues/33)，不是每一条都通过就不算该项通过，因此不关闭 [#5](https://github.com/wynxing/Lanwork/issues/5)。

- 日期：2026-10-08 首次自动化；2026-10-09 修验证程序拖错文件
- commit：`COMMIT_HASH_HERE`。修过抓取之后，`cargo run -p dnd -- --self-test` 退出码 0。2026-10-09 之前的手测结果不算数
- Slint：1.18.1（workspace 依赖 `=1.18.1`）。验证程序额外打开 feature `raw-window-handle-06`。默认 features 含 `backend-winit`、`renderer-femtovg`、`renderer-software`，不代表产品已选定渲染器
- 渲染器：`GraphicsAPI::NativeOpenGL`（FemtoVG 的 OpenGL 路径被选中）。窗口缩放 1.5
- 机器：DESKTOP-7C3P6OG，XIAOMI REDMI Book 14 2025 (FHD+)
- Windows：标题为 Windows 11 家庭中文版。`RtlGetVersion` 返回 10.0.26300，status=0。注册表 `DisplayVersion=26H2`，`CurrentBuild=26300`，`UBR=9550`。`ProductName` 仍是 Windows 10 Home China
- GPU 与驱动：PCI 设备 Intel(R) UHD Graphics，驱动 32.0.101.6733，日期 2025-04-02。同机还有两块虚拟显示：Virtual Display Driver 11.30.4.434（2024-12-24），GameViewer Virtual Display Adapter 15.6.5.199（2026-02-28）。没有判断哪一块在合成这个窗口
- 构建：debug。命令 `cargo run -p dnd -- --self-test`，退出码 0。Rust 1.99.0（b940084d7 2026-09-28），host `x86_64-pc-windows-msvc`

### 通过条件

结果留空。请按步骤做完后，只把亲眼看到的写成「通过」或「失败」。

| 条件 | 结果 |
| --- | --- |
| 拖入的多个路径全部拿到 |  |
| 拖出到资源管理器产生副本或快捷方式 |  |
| 同盘拖出不移动原文件 |  |
| 拖入 .lnk 时保留快捷方式本身的路径 |  |
| 超过 260 字符的长路径也能拿到 |  |
| 拖出过程中按 Esc 取消，原文件不变、没有残留 |  |
| 反复拖放 100 次，句柄数不持续增加 |  |

另有一条不在上面七条里，同样留空：从资源管理器把文件拖到 Slint 的「Slint 放下」区域，Slint 是否拿到路径。源码上 winit 后端不把外部放下交给 `DropArea`，但没有用鼠标确认。

### 2026-10-09 手测发现验证程序拖错文件

旧版验证程序先从列表拖出嵌套的 `long.txt` 之后，再按住 `folder` 那一行拖到样本目录的 `folder`，拷出来的是 `long.txt`。本地 `%TEMP%\lanwork-dnd-spike\folder\long.txt` 在 08:27 被创建，4 字节。窗口「记录」也不再出现新行。进程仍在，窗口仍响应。

根因：`DoDragDrop` 的模态循环吃掉鼠标抬起，Slint 没收到 release。第 4 行（`long.txt`，下标 3）的 `TouchArea` 一直处于 pressed 并抓着鼠标，下一次在任何行按下都被派给这一行。拖出结束时也没有清掉这份抓取。窗口「记录」以前只由定时器拷贝放下日志，拖出只写控制台；文本从顶部排，新行落在 140px 下面被裁掉，所以看起来停了。

已修：拖出成功、取消或出错之后都派发 `PointerReleased` 和 `PointerExited`。`ole-drag-start` 带行号和路径。拖入和拖出都写进窗口记录，只保留最后 12 行并贴在底部。修之前的手测不要填进通过条件。

自测覆盖的是同一条清理：`dispatch_event` 先在第 4 行按下并移动，再在第 2 行（`folder`，下标 1）按下并移动，中间不另发抬起。第二次必须是 `index=1`，路径以 `\folder` 结尾。这条自测不调用 `DoDragDrop`，不能代替下面的资源管理器手测。

### 人工步骤

先关掉已经打开的「拖放验证」窗口，再在仓库根目录执行下面的命令。旧进程占着 `dnd.exe` 时，新的 `cargo run` 覆盖不了它。样本由程序写到 `%TEMP%\lanwork-dnd-spike`，不要改用户文档目录。控制台每一行以 `dnd:` 开头。窗口「记录」应在每次拖入和拖出后出现新行。本机 `SM_CXDRAG=4`、`SM_CYDRAG=4`，缩放 1.5，所以要拖过大约 4 个物理像素才开始 OLE 拖出。

0. `cargo run -p dnd -- --ole`。先把列表里的 `long.txt` 拖到样本目录的 `folder` 里（复制即可）。再按住列表里的 `folder` 拖进同一个 `folder`。第二次控制台必须是 `ole-drag-start index=1 path=...\folder`，不能再是 `long.txt`。窗口「记录」里这两次都要出现。`folder` 里如果又多出一个 `long.txt`，这次修复失败，通过条件不要填。这一步只确认拖的是哪一行，不代替下面七条。
1. 同一个窗口里应看到 `registration-how=RevokeDragDrop then RegisterDragDrop`，且 `our-pointer` 与 `prop` 相同。
2. 打开 `%TEMP%\lanwork-dnd-spike`。同时选中 `readme.txt` 和 `folder`，拖进红色「OLE 拖入」矩形。控制台应有一行 `drop effect=1 paths=...`，两个路径都在，顺序不限。矩形外松手应没有这条 drop，光标不是复制。把看到的写入第一条。
3. 在窗口下方列表按住 `readme.txt`，拖到同一磁盘的一个文件夹，松手时选择复制。原文件还在样本目录，目标里有副本。再拖一次，松手时选择创建快捷方式。目标里出现快捷方式，原文件仍在。不要选移动；如果资源管理器只给出移动，记失败。写入第二、三条。
4. 把 `shortcut.lnk` 拖进红色矩形。`paths=` 里必须是 `shortcut.lnk` 自己的路径，不能变成 `readme.txt`。写入第四条。
5. 把 `long.txt` 拖进红色矩形。控制台里的路径应仍是样本目录下那段嵌套路径，UTF-16 长度大于 260。若拖出列表里的 `long.txt` 时控制台出现 `ole-drag-error` 或 `0x8007007A`，把原文记下，第五条不要写成通过。写入第五条。
6. 按住 `readme.txt` 拖出窗口，还没松手时按 Esc。样本目录里的 `readme.txt` 还在，目标里没有多出来的文件或半成品。写入第六条。
7. 在红色矩形和同一文件夹之间来回拖放，合计 100 次（拖入和拖出都算）。开始前抄下控制台里的 `handles=`，每 20 次再抄一次。数字可以波动，但不能一路涨。写入第七条。
8. 关掉窗口。另开 `cargo run -p dnd -- --slint`。从资源管理器把 `readme.txt` 拖到「Slint 放下」。窗口「记录」和 `paths` 有没有出现路径，写在七条之外的那一行。这一步不代替上面的 OLE 拖入。

### 自动化已经看到的（不是上面七条的通过）

`cargo run -p dnd -- --self-test` 退出码 0。

- 调用 `OleInitialize` 之前，以及 `--probe-no-ole` 在 `MainWindow::new` 之后、进入事件循环之前：`CoGetApartmentType` 为 `0x800401F0`（尚未 `CoInitialize`）。
- 进入 winit 事件循环后，以及本进程调用 `OleInitialize` 之后：`APTTYPE(3)` 即 `APTTYPE_MAINSTA`，qualifier 0。这是主 STA，不是 MTA。
- 未替换目标时，`RegisterDragDrop` 返回 already-registered。winit 0.30.13 的 `create_window_data` 在 `drag_and_drop` 为真时 `OleInitialize` 并注册 `FileDropHandler`。
- 替换后：`registration-how=RevokeDragDrop then RegisterDragDrop`，`our-pointer` 与 `prop` 同为 `0x20b49c4aab8`，再次注册仍是 already-registered。这个地址每次进程不同，几次自测里两边都相等。
- `SM_CXDRAG=4`，`SM_CYDRAG=4`。判定是物理像素位移严格大于这两个值。Slint 窗口内阈值是另一套：`DISTANCE_THRESHOLD` 为 8 逻辑像素。
- Shell `IDataObject` 往返：`readme.txt`、`folder`、`shortcut.lnk` 三个路径原样返回，快捷方式没有被解析成目标。
- 同一接口对 269 个 UTF-16 单元的 `long.txt` 调用 `GetData(CF_HDROP)` 失败：`0x8007007A`（数据区域太小）。
- 自己组的 `CF_HDROP` 能读回该长路径和 `shortcut.lnk`，长路径未被截断。
- 连续 100 次创建 Shell `IDataObject` 并读回 `CF_HDROP`：这次 `handles=305` 前后相同，`gdi=0`，`user=2`。这不是资源管理器拖放 100 次。
- `IDropSource::QueryContinueDrag` 在强制取消时返回 `0x40101`（`DRAGDROP_S_CANCEL`）。没有调用 `DoDragDrop`。
- 窗口内 `dispatch_event`：从「Slint 拖出」拖到「Slint 放下」得到 `got=true`、`action=Copy`、文本为 `readme.txt`。目标改要移动后 `got=false`、`action=None`。这不是拖到资源管理器。
- 同一窗口里 `dispatch_event` 先拖列表第 4 行再拖第 2 行，中间不发送抬起：`ole-drag-start index=3` 的路径是 `long.txt`，接着 `index=1` 的路径是 `...\folder`。窗口记录里同时有 `index=3` 和 `index=1`。这条不调用 `DoDragDrop`。

### 和 #27 的选择

收纳的外部拖放用原生 OLE，不改用 Slint `DragArea` / `DropArea`。窗口内拖放可用，但 winit 不实现 `start_drag`，窗口句柄上的放下目标也是 winit 的。替换目标的做法已经在本机注册成功。资源管理器是否给出复制和快捷方式、是否会在同盘变成移动、长路径和 Esc、句柄是否上涨，都还要上面的人工步骤。`crates/app` 里不写收纳界面。
