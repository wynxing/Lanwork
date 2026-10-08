# 实现状态

行为以 [product.md](product.md) 为准。本文只记录有没有代码和验证记录。工程骨架已在仓库中。搜索条前缀分类的纯函数在 `crates/core`（`search::classify_prefix`）。匹配引擎也在 `crates/core` 的 `search`。待办收集的日期前缀解析也在 `crates/core`（`parse_todo_due_prefix`），包括已写入产品规格的星期、下周和月底边界。规格缺口 #9 的第 4 项和第 7 项已定，其余项仍待定。便签服务在 `crates/core`（`lanwork_core::notes`）。待办服务在 `crates/core`（`lanwork_core::todos`）：清单、收件箱、周期、软删除与恢复、当前标记、处理模式和跨清单移动已有代码；永久删除、原清单不存在时的恢复，以及短月没有对应日、在截止日当天完成是否再生成，仍等产品规格。收纳服务在 `crates/core`（`lanwork_core::shelves`）：分组、路径引用、去重、待办关联和存在性检查已有代码。已有分组但未指定目标、再次关联另一条待办、关联到尚不存在的待办 id，仍等产品规格。残留关联 id 在读取时视为无关联，并在下次写该分组时清除；该清除在永久删除落地时实现。GitHub 服务在 `crates/core`（`lanwork_core::github`）：本机 `gh` 的探测与调用、watchlist、刷新与快照、长期未更新和 Draft、转为待办、隐藏规则和来源同步已有代码。追踪仓库的添加与移除等 #9 第 17 项，以及需要处理、需要 Review、CI 失败和筛选组合等 #9 第 18 项，仍等产品规格。应用索引在 `crates/core` 的 `apps`。便携应用、别名和手动刷新入口仍等规格。测量工具的代码已在仓库中，协议记录仍空着。Everything 与 Windows Search 的记录见下表，其余技术验证项没有通过记录。搜索条界面、待办界面、收纳界面和收集提交也还没有代码。界面表里的各项仍全部未实现。

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
| 中文输入法 | 无 |
| 背景 | 部分记录，未通过。见 [render-2026-10-08.md](measurements/render-2026-10-08.md)。2026-10-09 在 DESKTOP-7C3P6OG（Windows 11，屏幕 1280×800）上，FemtoVG、软件渲染、Skia 软件、Skia OpenGL 的窗口都是磨砂、能透出背景、没有黑色客户区。系统浅色/深色主题切换通过，深色下也清楚。关闭/打开「透明效果」通过。节电模式通过。渲染器未选定，这一项仍未通过。真实透明渲染失败没有新的记录。独立显卡机器未测。观察（不是失败判定）：自定义标题栏导致窗口拖不动；有两个关闭按钮 |
| 渲染器 | 部分记录，未通过。同上。2026-10-09 四种窗口的文字清楚（含中文和 0OIl1）。没有选定渲染器，也没有写进 architecture.md「运行时」。独立显卡机器未测 |
| 外部拖放 | 无 |
| Everything | [fileidx.md](measurements/fileidx.md)：1.4 与 1.5 都返回了文件名，并区分了未运行和未就绪 |
| Windows Search | [fileidx.md](measurements/fileidx.md)：文件名查询、正文不返回、P95 和 Private Bytes 已记下。`WSearch` 停止时的错误码未测 |
| 通知 | 有记录，未通过。见下方「通知（#7）」 |
| 热角 | 2026-10-08，`spikes/hotcorner`，release。停留一次、离开约 340ms、离开后再进入，以及静止 5 分钟的三种采样，在 1920×1200 的屏幕上有记录。2026-10-09 在 DESKTOP-7C3P6OG（Windows 11，屏幕 1280×800）上，钩子和 50ms 轮询各做了 5 分钟鼠标正常移动的采样（各 300 行）；100ms 轮询未测。拖动窗口、多显示器、全屏未测。没有选定机制，不算通过。见 [measurements/2026-10-08-hotcorner.md](measurements/2026-10-08-hotcorner.md) |

## 通知（#7）

这项没有通过。下面有任何一条不是「通过」，整项就不通过。自动检查只证明 API 返回值和注册表读回；屏幕上有没有通知、点下去有没有定位，都还空着，等所有者填「所有者观察」。

### 环境

| 字段 | 值 |
| --- | --- |
| 日期 | 2026-10-08 |
| commit | 与 spike 同一提交。精确哈希见紧接着的下一笔提交；自动检查跑在写入本记录之前的同一棵源码上 |
| Slint 精确版本 | 1.18.1。本 spike 不链接 Slint |
| 渲染器 | 不适用 |
| 机器 | 13th Gen Intel Core i5-13420H |
| Windows build | Windows 11 家庭中文版 64 位，DisplayVersion 26H2，10.0.26300.9550（CurrentBuild 26300，UBR 9550）。注册表 ProductName 仍写着 Windows 10 Home China |
| GPU 与驱动 | Intel UHD Graphics，驱动 32.0.101.6733（2025-04-02）；另有 Virtual Display Driver 11.30.4.434（2024-12-24）、GameViewer Virtual Display Adapter 15.6.5.199（2026-02-28） |
| 构建配置 | `cargo test` 为 debug。自动检查、未注册时的 `show` 回退为 release：`target\release\toast.exe` |

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

### 通过条件

| 条件 | 结果 | 所有者观察 |
| --- | --- | --- |
| 安装版：通知能显示 | 未测 | |
| 安装版：进程运行中点击，面板定位到该条，并收到待办 id | 未测 | |
| 安装版：进程已退出后点击，重新打开并定位到该条，并收到待办 id | 未测 | |
| 便携版：通知能显示 | 未测 | |
| 便携版：进程运行中点击，面板定位到该条，并收到待办 id | 未测 | |
| 便携版：进程已退出后点击，重新打开并定位到该条，并收到待办 id | 未测 | |
| 同一待办多次提醒时，系统是否把通知叠放 | 未测 | |
| 请勿打扰打开时，通知如何表现 | 未测 | |
| 缺失注册时不崩溃，并退回托盘逾期徽标 | 未测 | 命令路径已退出码 0，且 API 返回 TRUE；徽标有没有出现在托盘上还没看 |
| 托盘徽标代替到期通知 | 失败（不能代替） | 不把徽标算成通知通过 |

模拟面板在单元测试里会按 id 选中对应行：`todo-003` 选中「交电费」，未知 id 不选中，空参数保持等待。这只说明函数行为。点击之后窗口里有没有选中那一行，仍以上表为准。正式待办页的定位不在本记录里，属于 #25。

便携版点击定位还没有观察结果，所以没有改 [product.md](product.md)。

### 所有者要亲手做的步骤

在本 PR 的目录里执行。命令都用 release，这样快捷方式里的程序路径和正在跑的程序是同一个。换过构建目录或重新编译后，先重新 `register`。做完或中途停下，都执行文末的清理。

1. `cargo build -p toast --release`
2. 安装版显示：`cargo run -p toast --release -- register installed`，然后 `status`，确认判定是「安装版」。再 `cargo run -p toast --release -- show todo-001`。看通知中心或右下角有没有「待办到期」，正文含 `todo-001` 和「买牛奶」。把看到的写进「安装版：通知能显示」。控制台里的 `S_OK` 不算。
3. 安装版、进程还在时点击：另开一个终端，`cargo run -p toast --release -- serve`，让模拟面板保持打开。再 `cargo run -p toast --release -- show todo-003`。点击那条通知。要同时看到：面板到前台；标题含「已定位 todo-003」；列表选中「todo-003 交电费」；运行 `serve` 的终端有 `COM Activate launch=todo-003`。只收到这串字、列表没有选中该行，定位不通过。
4. 安装版、进程退出后点击：关掉模拟面板，确认 `serve` 已经退出。上一条通知会被点击消掉，所以再 `cargo run -p toast --release -- show todo-002`。点击它。应重新出现面板，并选中「todo-002 写周报」，终端有 `launch=todo-002`。冷启动时先有控制台窗口，这是 spike 用了控制台子系统，单独记一笔，不因此改判定。
5. 便携版不要沿用安装版的结果。先 `cargo run -p toast --release -- unregister`，再 `register portable`，`status` 必须是「便携版」，且没有快捷方式、CustomActivator、LocalServer32。然后按第 2、3、4 步各做一次，分别填便携版的显示、运行中点击、退出后点击。退出后点击如果拉不起进程，就记失败，不要补快捷方式再试。
6. 叠放：在安装版注册下执行 `cargo run -p toast --release -- show todo-001 --repeat 3`，看通知中心是三条还是被收成一条。再执行 `show todo-001 --repeat 3 --tag todo-001`，再看一次。便携版再各做一遍。历史条数已经写在自动观察里，这里只填眼睛看到的。
7. 请勿打扰：打开「设置 → 系统 → 通知」，打开「请勿打扰」。在安装版注册下 `show todo-001`。写下通知有没有出现、有没有进通知中心。然后关掉请勿打扰。不要改 Focus Assist 的注册表。
8. 缺失注册的托盘：`unregister` 之后 `show todo-001`。确认进程退出码是 0，没有弹出到期通知，托盘上出现带「1」的图标，提示「逾期 1」，大约 15 秒后消失。看到图标后再把这一行改成通过或失败。没看到就保持未测。

### 清理

自动检查结束时，这台机器上的 spike 注册已经去掉。若通知中心还留着「Lanwork 自动检查」或「待办到期」，在通知中心里清掉，或运行 `cargo run -p toast --release -- clear-history`。

所有者做完步骤后执行：

```text
cargo run -p toast --release -- unregister
cargo run -p toast --release -- status
```

`status` 应为「未注册」。这会删除上面的快捷方式、AUMID 键和 CLSID 键，并尝试清除该 AUMID 的通知历史。不删除其他程序的通知。卸载产品时的清理属于 #31，不在这里做。

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
