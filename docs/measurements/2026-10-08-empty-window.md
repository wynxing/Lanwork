# 测量记录

按 [TEMPLATE.md](TEMPLATE.md) 填写。空着的格子写「未测」。

技术验证里的测量只说明当时的场景，不算「性能测量」的验收。没有一份符合协议的记录之前，文档和界面都不写「已达到」。有效样本不足 100 的项标为未完成。偏离了协议的记录要写明偏离之处，不能作为「已达到」的依据。

禁止为了凑数字裁剪工作集。`lanwork-sample` 没有 `EmptyWorkingSet` 或缩小工作集的参数。

## 环境

这些字段是 architecture.md「技术验证」和「性能测量」都要求写入记录的。

| 字段 | 值 |
| --- | --- |
| 日期 | 2026-10-08 |
| commit | 2b06db233162119c0056398a88b56b81bd276abc |
| 构建配置 | release |
| Slint 精确版本 | 1.18.1 |
| 渲染器 | Slint 1.18.1 默认 features。技术验证尚未选定 FemtoVG、Skia 或软件渲染 |
| 机器 | XIAOMI REDMI Book 14 2025(FHD+); 13th Gen Intel(R) Core(TM) i5-13420H; logical CPUs 12; RAM 15.7 GiB; 显示缩放 100%（LOGPIXELSX/Y = 96） |
| Windows build | Microsoft Windows 11 家庭版 中文版，版本 10.0.26300，64 位。注册表 `ProductName` / `DisplayVersion` / `CurrentBuild`.`UBR` 为 Windows 10 Home China 26H2 build 26300.9550 |
| GPU 与驱动 | Virtual Display Driver driver 11.30.4.434; GameViewer Virtual Display Adapter driver 15.6.5.199; Intel(R) UHD Graphics driver 32.0.101.6733 |
| 数据规模 | 偏离协议：空 Slint 窗口，没有加载固定数据 |

测量用数据目录通过环境变量 `LANWORK_DATA_DIR` 覆盖（见 architecture.md「数据」），指向 `lanwork-fixture --out <目录>` 打印的 `data_dir`。相对路径按当前工作目录补成绝对路径。这个目录必须和正式数据目录隔开，不能是 `%USERPROFILE%\Documents\Lanwork`，也不读取 `%USERPROFILE%\Documents\MayDolist`。

夹具不读 `LANWORK_DATA_DIR`，输出位置只由 `--out` 决定。空 Slint 窗口的 5 分钟采样也不设置这个变量。`data` 里的待办、便签和收纳 JSON 带 `schemaVersion` 1，字段仍按 #11、#12、#13、#14 已经列出的模型来写。architecture.md 里的示例模型是 `ExampleDocument`，不是这批测量数据的模型。

应用索引要读的 5,000 个快捷方式在夹具打印的 `shortcuts_dir`。追加这个目录的环境变量名由 #18 决定，本模板不另起一个名字。

本次没有设置 `LANWORK_DATA_DIR`。空窗口也不读它。

## 技术验证

每个通过条件分别标「通过」「失败」或「未测」。只有全部条件都通过，这一项才算通过。

| 项 | 通过条件 | 结果 | 附带的测量 |
| --- | --- | --- | --- |
| 中文输入法 | 微软拼音能组合、上屏，候选窗贴在光标处 | 未测 | |
| 中文输入法 | 组合期间 Enter 不触发提交 | 未测 | |
| 背景 | 选定渲染器下，透明窗口加 DWM Acrylic 正确显示 | 未测 | |
| 背景 | 透明失败时能检测到并退回纯色 | 未测 | |
| 渲染器 | FemtoVG、Skia、软件渲染各有一个空面板和一个 200 行列表 | 未测 | Private Bytes、工作集、中文字形、二进制体积 |
| 外部拖放 | 从资源管理器拖入多个文件能拿到路径 | 未测 | |
| 外部拖放 | 拖出到资源管理器时只有复制和创建快捷方式 | 未测 | |
| Everything | 1.4 和 1.5 都能返回结果 | 未测 | |
| Everything | 能区分「未运行」和「未就绪」 | 未测 | |
| Windows Search | 只按文件名查询，正文命中不返回 | 未测 | 结果 P95、查询带来的 Private Bytes 增量 |
| 通知 | 到期通知能显示，点击后打开面板并定位该条 | 未测 | 安装版和便携版分开记 |
| 热角 | 光标移动 5 分钟、静止 5 分钟 | 未测 | CPU 占用、每秒唤醒次数 |

渲染器、Windows Search 和热角使用本目录的采样与延迟汇总。原始 CSV、JSONL 和汇总 JSON 的路径写在「附带的测量」里。

## 性能测量

场景：固定数据为 5,000 个应用、10,000 条待办、1,000 篇约 2 KiB 的便签、20 个收纳分组共 1,000 条引用。预热搜索条和面板搜索，打开收纳标签，在搜索条里各提交一次待办和便签收集，并打开再关闭便签悬浮窗。然后收起全部窗口，连续采样 5 分钟。

GitHub 快照不放进这组固定数据。刷新 GitHub 时另记内存。Everything 客户端、Everything 服务、Windows Search 索引服务和安装程序单独记录，不计入 Lanwork 进程。随包的 Everything SDK DLL 加载在 Lanwork 进程内，计入。

| 项 | 目标 | 结果 | 有效样本数 | 是否足 100 | P95 | 被取代查询数 | 原始时间戳 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 进程 Private Bytes 峰值 | ≤ 100 MiB | 未测 | | | | | 同时记录工作集 |
| 热召回 | ≤ 100 ms | 未测 | | | | | |
| 应用、待办、便签结果 | ≤ 50 ms | 未测 | | | | | |
| Everything 就绪时的文件结果 | ≤ 150 ms | 未测 | | | | | |
| Windows Search 回退时的文件结果 | 只记录，不设目标 | 未测 | | | | | |
| 冷启动、首次索引 | 单独记录，不并入上面的 P95 | 未测 | | | | | |
| 收起后空闲 5 分钟 | 记录平均 CPU 和每秒唤醒次数 | 未测 | | | | | |
| 同时打开 5 个便签悬浮窗 | 单独记录 Private Bytes 增量 | 未测 | | | | | |
| 100 次呼出、查询、开关辅助窗口 | 记录 Private Bytes、USER、GDI 是否持续上升 | 未测 | | | | | |

口径，记录时不要改：

- 热召回：从进程收到热键消息，到搜索条第一次 `AfterRendering`。不含 DWM 合成到屏幕的时间。
- 结果延迟：从输入框文本变化（组合中的预编辑不算），到带同一查询序号的结果第一次 `AfterRendering`。文件结果包含 60ms 等待。
- 被新输入取代的查询不计入样本，单独记数量。预热排除。
- P95 取升序第 ⌈0.95 × N⌉ 个。每项至少 100 个有效样本。Everything 和 Windows Search 分开统计。
- 每秒唤醒次数：目标进程所有线程的 `\Thread(<进程名>*)\Context Switches/sec` 之和。采样器再按 PID 滤掉同名的其他进程。
- 平均 CPU 用 CSV 的 `cpu_time_100ns` 计算：（最后一行 − 第一行）÷ 这段墙钟时间。`cpu_percent` 是每个间隔的进程 CPU，分子是全部线程的 kernel+user，可以超过 100。架构没有规定百分比的分母，验收以累计 CPU 时间为主。

偏离协议：

空 Slint 窗口采样。没有加载协议规定的固定数据，没有热召回，也没有结果延迟。上面的性能表保持「未测」。这份记录不能作为「已达到」的依据。

`tools/README.md` 里的 PowerShell 没有改。它在采样器退出码为 0 之后还要求 CSV 至少 290 行、首尾至少隔 290 秒。两次跑满时长的结果都被这个行数检查拒绝，所以脚本没有写出 `measurements-out\empty-window.md`，上面的「实机」勾选也保持未勾。

归档的是第一次跑满时长的 CSV：[2026-10-08-empty-window.csv](2026-10-08-empty-window.csv)。采样器 JSON：`samples` 272，`stop` 为 `duration`，计数器 `\Thread(lanwork*)\Context Switches/sec`，PID 24504。每一列都有值。首尾 `2026-10-08T05:43:57.338Z` 到 `2026-10-08T05:48:56.300Z`，间隔 298.962 秒。行间隔平均 1103.2 ms（最小 909 ms，最大 1494 ms）。

- Private Bytes 峰值：49,844,224 字节（47.54 MiB）。最小 49,557,504 字节。
- 工作集峰值：77,705,216 字节（74.11 MiB）。最小 77,467,648 字节。
- 平均 CPU：（最后一行 `cpu_time_100ns` 14,062,500 − 第一行 4,531,250）÷ 墙钟 = 0.319%。
- `wakeups_per_sec` 平均：96.629。
- 句柄：325 到 339。
- USER 对象：23 到 24。
- GDI 对象：17。

协议里 Private Bytes ≤ 100 MiB 是固定数据场景的目标。这次是空窗口，该行保持「未测」，不记通过或失败。47.54 MiB 只是这次读数，低于 100 MiB 这个数字本身，不能当成验收。

同一脚本再跑一次（仍是 `2b06db2`，没有改脚本）：`samples` 267，`stop` 为 `duration`，PID 17112，首尾间隔 299.018 秒，Private Bytes 峰值 49,467,392 字节，工作集峰值 77,455,360 字节，平均 CPU 0.199%，`wakeups_per_sec` 平均 97.202。脚本同样抛出「CSV 只有 267 行数据，5 分钟采样不完整。」这次 CSV 不入库。

在 `5d7c133` 上、采样器还把 `PDH_CALC_NEGATIVE_VALUE`（`0x800007D8`）当成整次失败时，同一脚本先中止过两次：一次 24 行后退出，一次 47 行后退出，错误都是「读取计数器数组失败：0x800007D8」。那两次不是这份 5 分钟记录。`2b06db2` 让数组级的 `PDH_CALC_NEGATIVE_*` 继续返回实例，坏实例仍按自己的 `CStatus` 丢掉。

采样期间还有别的工作树在编译。第一次（本 CSV）开始前，机器上有 `cargo test`、`cargo check -p lanwork-render-spike`，以及 issue-5 工作树的 `cargo run -p dnd -- --self-test`，rustc 在编 `windows`、`image`、`lanwork_core`、`criterion`、`usvg`。采样约 1 分钟时的 3 秒 CPU 差里，前排是 powershell、任务管理器、微信、audiodg、Edge WebView2、Cursor；`lanwork-sample` 约 0.125 秒，`lanwork.exe` 没有进前 15。再跑那一次的中途，issue-18 在编 `i-slint-compiler` / `i-slint-core`，issue-7 在编 FemtoVG 和软件渲染器，issue-5 在跑 clippy，另有 `cargo test --locked`。那 3 秒里 rustc 最高约 5.4 秒 CPU。桌面上一直有 Cursor、微信、Edge、GameViewer。

行数过不了 290 的原因在采样循环，不在脚本门槛被改过：循环先睡满 1000 ms，再读性能计数器，时长时钟把这次读取算进去。这台机器上一次读取大约 100 ms，所以 300 秒时长写出 270 行上下，首尾却仍接近 299 秒。单独的改法是读完之后只睡完这一秒里剩下的时间，让读取短于 1 秒时仍然大约每秒一行。这次没有改 `tools/README.md`，也没有改这个循环。

## 实机

下面这项要在 Windows 11 桌面上做。仓库里的单元测试和 Windows CI 不能代替。没做之前保持未勾选，对应格子填「未测」。

- [ ] 对空 Slint 窗口采样 5 分钟，得到每一列都有值的 CSV。命令见 `tools/README.md`。采样器两次都跑满 `duration`（272 行和 267 行），每一列都有值，首尾都超过 290 秒。脚本因行数少于 290 拒绝，所以这里不勾。CSV：[2026-10-08-empty-window.csv](2026-10-08-empty-window.csv)。
