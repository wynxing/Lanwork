# 测量记录

按 [TEMPLATE.md](TEMPLATE.md) 填写。空着的格子写「未测」。

技术验证里的测量只说明当时的场景，不算「性能测量」的验收。没有一份符合协议的记录之前，文档和界面都不写「已达到」。有效样本不足 100 的项标为未完成。偏离了协议的记录要写明偏离之处，不能作为「已达到」的依据。

禁止为了凑数字裁剪工作集。`lanwork-sample` 没有 `EmptyWorkingSet` 或缩小工作集的参数。这次没有对 `hotcorner.exe` 调用它。

## 环境

这些字段是 architecture.md「技术验证」和「性能测量」都要求写入记录的。

| 字段 | 值 |
| --- | --- |
| 日期 | 2026-10-08。本机时钟 17:07 +08:00 时，下面的 release 自测已经结束 |
| commit | aeab635c6b192b6062e35b07fd16b252a00e78e2 |
| 构建配置 | release。`rustc 1.99.0 (b940084d7 2026-09-28)` |
| Slint 精确版本 | 未使用。这个 spike 不依赖 Slint |
| 渲染器 | 未使用 |
| 机器 | XIAOMI REDMI Book 14 2025(FHD+); 13th Gen Intel(R) Core(TM) i5-13420H; logical CPUs 12; RAM 16,891,371,520 字节（15.7 GiB）; `GetDpiForMonitor` 96×96 |
| Windows build | 注册表 `ProductName` / `DisplayVersion` / `CurrentBuild` / `UBR` 为 Windows 10 Home China、26H2、26300、9550。`OSVersion` 为 Microsoft Windows NT 10.0.26300.0 |
| GPU 与驱动 | Virtual Display Driver driver 11.30.4.434; GameViewer Virtual Display Adapter driver 15.6.5.199; Intel(R) UHD Graphics driver 32.0.101.6733 |
| 数据规模 | 偏离协议：没有加载固定数据，也没有设置 `LANWORK_DATA_DIR` |

测量用数据目录通过环境变量 `LANWORK_DATA_DIR` 覆盖（见 architecture.md「数据」），指向 `lanwork-fixture --out <目录>` 打印的 `data_dir`。相对路径按当前工作目录补成绝对路径。这个目录必须和正式数据目录隔开，不能是 `%USERPROFILE%\Documents\Lanwork`，也不读取 `%USERPROFILE%\Documents\MayDolist`。

夹具不读 `LANWORK_DATA_DIR`，输出位置只由 `--out` 决定。这次热角程序也不读它。

本次没有设置 `LANWORK_DATA_DIR`。

原始记录：[2026-10-08-hotcorner-probe.json](2026-10-08-hotcorner-probe.json)、[2026-10-08-hotcorner-self-test.json](2026-10-08-hotcorner-self-test.json)。更早一次 debug 自测没有当作这份记录。

## 技术验证

每个通过条件分别标「通过」「失败」或「未测」。只有全部条件都通过，这一项才算通过。热角这一项没有全部通过。

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
| 热角 | 光标移动 5 分钟、静止 5 分钟 | 未完成。静止三种有采样，移动没有 | 静止 CSV 见下方。移动未测 |

渲染器、Windows Search 和热角使用本目录的采样与延迟汇总。原始 CSV、JSONL 和汇总 JSON 的路径写在「附带的测量」里。这次只有静止采样的 CSV，没有移动采样，也没有延迟 JSONL。

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

这次不是 `lanwork.exe`，没有固定数据，也没有热召回或结果延迟。上面的性能表保持「未测」，不能作为「已达到」的依据。热角进程名是 `hotcorner.exe`。若对它采样，计数器是 `\Thread(hotcorner*)\Context Switches/sec`，不是架构里写的 `lanwork*`。

## 热角

程序是 `spikes/hotcorner`。`cargo test` 不安装钩子，也不移动光标。下面的实机结果来自 release 的 `self-test --corner-px 32`。自测会短时间移动光标，结束时放回原处，并卸下钩子。结束后没有 `hotcorner.exe`。

产品规格写了：热角默认右上，可设为左上、左下、右下或关闭；进入所选角并连续停留 350ms 后触发；离开该角后才允许再次触发；停留期间不重复触发。这次停留用的就是 350ms，角落四种和关闭都跑了。

待定，规格没有写，这次只是参数：

- 角的像素边长。这次是 32。瞄准点是这块区域的中心，右上为 (1904, 15)。
- 多显示器认主屏还是认光标所在屏。这次自测传的是主屏。点不在任何显示器内时，光标所在屏模式退回主屏。这块机器只有一块屏，空隙没有实机样本。
- 全屏游戏或视频时是否抑制。程序只探测前台窗口，不因此改触发。
- 启动时光标已经在角内。状态机把开始看成在角外，第一拍若已在角内会开始计时。自测每次都先把光标移出角，所以这条没有实机样本。
- 关闭时检测是否还在跑。这次关闭后检测仍在跑，角测试恒为假。规格只要求不打开面板。

不调用 `timeBeginPeriod`。不写注册表。`HKCU\Control Panel\Desktop\LowLevelHooksTimeout` 读取结果是 `found: false`，`error: ERROR_FILE_NOT_FOUND`。没有写入，也没有恢复动作。

显示器一块，矩形 [0, 0, 1920, 1200]，主屏，DPI 96。虚拟屏幕同为 1920×1200，原点 (0, 0)。自测开始时前台是 `CabinetWClass`，`C:\Windows\explorer.exe`，pid 9032，矩形 [120, 0, 1320, 752]，`covers_monitor` 为 false。

`arrived_to_trigger_us` 是光标确认进角到触发的微秒。`move_to_trigger_us` 含移入角的时间。每次停留只应有一条触发。不另设「离 350ms 允许多少」的通过线。

### 方案 A：`WH_MOUSE_LL`

回调在钩子线程里比较已发布的矩形并 `PostMessageW`。计时在窗口线程。`disagree` 0，`timer_fail` 0，`post_fail` 0，`hook_hits` 60，工作线程没有 panic。

停留，每次 1 次触发，spike 标为通过：

| 角 | `arrived_to_trigger_us` | `move_to_trigger_us` |
| --- | --- | --- |
| 右上 | 354710，361281，363620 | 361022，363375，365189 |
| 左上 | 362582 | 363701 |
| 左下 | 360479 | 361825 |
| 右下 | 350069 | 350928 |

离开：在角内 340ms 后移出，触发 0 次，spike 标为通过。`inside_ms` 含移出的时间。

再次进入：第一次 `first_trigger_us` 369651，按住期间多出的触发 0，离开后再进入 `second_trigger_us` 355863。spike 标为通过。

关闭：在右上角停 500ms，触发 0 次。spike 标为通过。这是「角测试为假」，不是「检测线程已经停」。待定。

### 方案 B：50ms

停留，每次 1 次触发，spike 标为通过：

| 角 | `arrived_to_trigger_us` | `move_to_trigger_us` |
| --- | --- | --- |
| 右上 | 352768，396569，395443 | 354167，399288，397405 |
| 左上 | 394202 | 395788 |
| 左下 | 401120 | 401539 |
| 右下 | 400087 | 400751 |

离开：`inside_ms` 342，触发 0 次，spike 标为通过。

再次进入：`first_trigger_us` 362716，按住期间多出的触发 0，`second_trigger_us` 355864。spike 标为通过。

关闭：停 500ms，触发 0 次。spike 标为通过。待定，同上。

### 方案 B：100ms

停留，每次 1 次触发，spike 标为通过：

| 角 | `arrived_to_trigger_us` | `move_to_trigger_us` |
| --- | --- | --- |
| 右上 | 499781，492286，494199 | 502367，493416，495849 |
| 左上 | 496480 | 498128 |
| 左下 | 499572 | 499886 |
| 右下 | 497879 | 498184 |

离开：`inside_ms` 340，触发 0 次，spike 标为通过。

再次进入：`first_trigger_us` 409667，按住期间多出的触发 0，`second_trigger_us` 449600。spike 标为通过。

关闭：停 500ms，触发 0 次。spike 标为通过。待定，同上。

### 钩子线程阻塞

没有人眼看指针。不把某一毫秒数写成卡顿阈值。`returned_before_sleep_finished` 在 `SendInput` 返回的当时读取，读完才把光标移回去。

| 步骤 | `send_ms` | 返回时 Sleep 已结束 | 光标移动 | 起点 | 终点 |
| --- | --- | --- | --- | --- | --- |
| 基线，没有 Sleep | 0 | 不适用（`block_done_at_send_return` false） | true | [1764, 155] | [1710, 155] |
| 窗口线程 `Sleep(400)` | 0 | false，也就是返回时还在 Sleep | true | [1764, 155] | [1678, 155] |
| 钩子线程消息循环 `Sleep(400)` | 303 | false，也就是返回时还在 Sleep | true | [1764, 155] | [1678, 155] |

钩子线程这次 Sleep 之后，`hook_installed_after` 为 true。相对位移请求是 40 或 -40 像素；上表终点是 `GetCursorPos` 读到的位置，和请求的差值不完全相同。

人眼是否觉得卡顿：未测。

### 钩子不再收到后续移动

`LowLevelHooksTimeout` 键不存在，没有改它。回调里一次性 `Sleep(1000)`，这只出现在这次移除试验。`slow_send_ms` 311，`SendInput` 没有错误。`HOOK_HITS` 从 54 到 55，说明这次慢回调跑过。随后一次相对移动在 500ms 内没有再增加 `HOOK_HITS`（`hook_called_after_slow` false）。程序把这记为检测不到后续调用。没有单独的系统错误码说「钩子已被移除」。

然后向钩子线程投递重新安装。`rehook_finished` true，`rehook_result` 为 `ok`。之后的移动能进回调。重新安装后的停留：`arrived_to_trigger_us` 357270，1 次触发，spike 标为通过。

接着卸下钩子，`unhooked` true。退回方案 B 的 50ms 再停留一次：`arrived_to_trigger_us` 362035，1 次触发，spike 标为通过。开始这次试验时钩子还在，`restored_before_slow` 为 null。

### 拖动窗口

未测。三种跑法都创建了 `WS_EX_TOPMOST` 的 `hotcorner-spike` 窗口，标题栏按下并移动，但窗口矩形前后都是 [840, 530, 1080, 670]。矩形没变，不能把这次当成拖过窗口。`input_error` 和 `title_error` 都是 null。

按住那一次（约 550–562ms）每种方案的 `triggers` 都是 1，快扫那一次是 0。窗口没动，这个 1 只说明光标在角里停过，不是拖动窗口的结果。

### 多显示器

未测。`monitor_count` 1。两种选屏都写在 `select_monitor` 里：主屏模式只用主屏矩形；光标所在屏模式用包含该点的矩形，点不在任何一块里时退回主屏。这块机器看不到两者的差别。待定。

### 全屏

未测。自测当时的前台窗口没有盖住显示器。程序没有按全屏改变触发。是否抑制，待定。

### 5 分钟负载

移动 5 分钟：未测。没有把光标按住移动 5 分钟。

静止 5 分钟做了。光标停在 (960, 600)，不在 32 像素角里。三种的 `events` 文件都是 0 字节。外部 `GetCursorPos` 各记了 298 行，全部是 `960,600`。进程采样各 300 行。采样器按 PID 滤线程。进程名是 `hotcorner.exe`，计数器按采样器规则是 `\Thread(hotcorner*)\Context Switches/sec`，不是架构里的 `lanwork*`。这次子进程没有把采样器自己的 JSON 行存下来。更早一次方案 A 采样作废：那时的坐标日志写出来是空的，不能拿来证明没动。那份 CSV 的数字不写在这里。

平均 CPU 是（最后一行 `cpu_time_100ns` − 第一行）÷ 首尾 `utc` 的墙钟。百分比是这段 CPU 时间占墙钟的比例。

| 方案 | PID | 首尾 UTC | 墙钟 | `cpu_time_100ns` | 平均 CPU | `wakeups_per_sec` | Private Bytes | 工作集 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| A 钩子 | 16628 | 2026-10-08T09:14:43.623Z 到 2026-10-08T09:19:42.628Z | 299.005 秒 | 312500 到 312500 | 0% | 300 行都是 0，平均 0 | 1,261,568–1,351,680 | 7,491,584–7,610,368 |
| B 50ms | 26704 | 2026-10-08T09:20:44.118Z 到 2026-10-08T09:25:43.114Z | 298.996 秒 | 312500 到 2187500 | 0.0627% | 平均 22.478，最小 18.996205，最大 30.716574 | 1,224,704–1,318,912 | 7,348,224–7,454,720 |
| B 100ms | 24752 | 2026-10-08T09:26:44.413Z 到 2026-10-08T09:31:43.420Z | 299.007 秒 | 156250 到 937500 | 0.0261% | 平均 11.257，最小 8.718673，最大 17.005032 | 1,208,320–1,302,528 | 7,344,128–7,450,624 |

0.0627% 来自 (2,187,500 − 312,500) / 10,000,000 / 298.996。0.0261% 来自 (937,500 − 156,250) / 10,000,000 / 299.007。唤醒平均是 300 个 `wakeups_per_sec` 的算术平均，四舍五入到三位小数。

句柄、USER、GDI：方案 A 为 110、5、0。方案 B 50ms 句柄 111–112，USER 1，GDI 0。方案 B 100ms 句柄 113，USER 1，GDI 0。方案 B 各行的 `cpu_percent`：50ms 从 0 到 1.563342，100ms 从 0 到 1.562828。方案 A 各行都是 0。

脚本没有读到进程退出码（记下来是 null）。没有对这三个进程调用 `Stop-Process`。三次结束后进程列表里没有 `hotcorner.exe`。

CSV：[2026-10-08-hotcorner-still-hook.csv](2026-10-08-hotcorner-still-hook.csv)、[2026-10-08-hotcorner-still-poll50.csv](2026-10-08-hotcorner-still-poll50.csv)、[2026-10-08-hotcorner-still-poll100.csv](2026-10-08-hotcorner-still-poll100.csv)。坐标：[2026-10-08-hotcorner-still-hook-cursor.csv](2026-10-08-hotcorner-still-hook-cursor.csv)、[2026-10-08-hotcorner-still-poll50-cursor.csv](2026-10-08-hotcorner-still-poll50-cursor.csv)、[2026-10-08-hotcorner-still-poll100-cursor.csv](2026-10-08-hotcorner-still-poll100-cursor.csv)。

这不是「性能测量」的空闲场景，也不能写成已经达到内存或 CPU 目标。光标移动的 5 分钟仍然没有。机制没有选定。

要补移动 5 分钟：人在采样的 5 分钟里持续移动光标，不要只停在角上。检测先跑起来，采样 300 秒，检测用 360 秒自己退出。不要 `Stop-Process`。

```
cargo run -p hotcorner --release -- run --scheme hook --monitor primary --corner top-right --corner-px 32 --dwell-ms 350 --duration-secs 360 --events events-hook.jsonl
cargo run -p lanwork-sample --release -- sample --pid <上面打印的 pid> --duration-secs 300 --interval-ms 1000 --out hook-move.csv
```

方案 B 把第一行换成：

```
cargo run -p hotcorner --release -- run --scheme poll --interval-ms 50 --monitor primary --corner top-right --corner-px 32 --dwell-ms 350 --duration-secs 360 --events events-poll50.jsonl
cargo run -p hotcorner --release -- run --scheme poll --interval-ms 100 --monitor primary --corner top-right --corner-px 32 --dwell-ms 350 --duration-secs 360 --events events-poll100.jsonl
```

拖动窗口要人手做：用上面的 `run`，`--duration-secs 120`，自己把一个普通窗口拖过右上 32 像素区域，再看 `events` 文件。自动化那次矩形没有变化。

多显示器要再接一块屏，然后重跑 `self-test --corner-px 32`。全屏要打开一个盖住该屏的视频或游戏，再跑 `probe`，并在该状态下做一次停留。这两项规格都还没定。人眼是否觉得钩子线程阻塞时卡顿，也还没有看。

## 实机

- [x] release 自测：停留、离开、再次进入、关闭、钩子慢回调后的重新安装和 50ms 退回。JSON 见 [2026-10-08-hotcorner-self-test.json](2026-10-08-hotcorner-self-test.json)。
- [x] 光标静止 5 分钟，方案 A、方案 B 50ms、方案 B 100ms。外部坐标 298 行都是 960,600。
- [ ] 光标移动 5 分钟，同样三种。
- [ ] 人手拖动窗口经过角。
- [ ] 两块显示器上比较两种选屏。
- [ ] 全屏前台下的探测和停留。人眼是否觉得钩子线程阻塞时卡顿，也还没有看。
