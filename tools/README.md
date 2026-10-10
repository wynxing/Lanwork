# 测量工具

给 #4、#6、#8、#20、#21、#32 用。可见行为仍以产品规格为准。这里只规定工具怎么调用，以及它和后续 issue 交换的文件格式。

## 固定数据

```text
cargo run -p lanwork-fixture -- --out <目录>
```

默认规模是协议规模：10,000 条待办、1,000 篇正文恰好 2,048 字节的便签、20 个收纳分组共 1,000 条引用、5,000 个快捷方式。`--todos`、`--lists`、`--notes`、`--note-bytes`、`--shelves`、`--refs`、`--shortcuts` 可以改规模。协议没有规定清单个数，`--lists` 默认 20，这是夹具参数，不是存储布局。

`--out` 必填。夹具不读 `LANWORK_DATA_DIR`。它拒绝写入 `%USERPROFILE%\Documents\Lanwork` 和 `%USERPROFILE%\Documents\MayDolist`。

输出：

```text
<目录>/
├── manifest.json
├── data/                 测量时把 LANWORK_DATA_DIR 指到这里
│   ├── todos/
│   ├── notes/
│   └── shelves/
├── files/                收纳引用指向的生成文件
└── shortcuts/            给 #18 的测量来源读
    ├── App 0001.lnk
    └── targets/
```

引用放在夹具目录里，不放进系统 Temp，这样测量期间路径一直有效。快捷方式按 MS-SHLLINK 写成 `.lnk`，目标是同目录下生成的空文件，参数是 `fixture`，工作目录是 `targets`。盘符路径会带上 IDList，`IShellLinkW::GetPath` 才能读回目标。#18 规定指向不存在目标的快捷方式不进索引，所以这些目标文件要留着。应用索引读取环境变量 `LANWORK_EXTRA_SHORTCUT_DIR`。测量时把它指到上面的 `shortcuts` 目录。界面不暴露这个变量。

`data` 里的 JSON 带 `schemaVersion` 1。字段按 #11、#12、#13、#14 已经列出的模型来写。architecture.md 要求每个 JSON 对象带 `schemaVersion`，示例模型是 `ExampleDocument`，不是待办、便签或收纳。那些 issue 落地时如果改了字段，夹具要跟着改。不生成 `config.json`，也不生成 GitHub 快照。待办都未完成，便签都不在回收站，这样后续索引能看见协议里的全部条数。时间戳固定为 `2026-01-01T00:00:00Z`，同一组参数会写出同一批字节。

## 采样

```text
cargo run -p lanwork-sample -- sample --pid <pid> --duration-secs 300 --interval-ms 1000 --out sample.csv
cargo run -p lanwork-sample -- sample --name lanwork --duration-secs 300 --interval-ms 1000 --out sample.csv
```

只能在 Windows 上跑。每秒一行。进程在采样中途退出时，命令结束，已经写出的行留在 CSV 里。`--name` 匹配到多个进程时要改用 `--pid`。

CSV 列：

```text
utc,pid,private_bytes,working_set_bytes,handles,user_objects,gdi_objects,cpu_time_100ns,cpu_percent,wakeups_per_sec
```

- `private_bytes` 是 `PROCESS_MEMORY_COUNTERS_EX.PrivateUsage`。
- `working_set_bytes` 是 `WorkingSetSize`。
- `handles` 是 `GetProcessHandleCount`。
- `user_objects` 和 `gdi_objects` 是 `GetGuiResources`。
- `cpu_time_100ns` 是 kernel 与 user 时间之和，单位 100 纳秒，累计值。
- `cpu_percent` 是相邻两行之间的进程 CPU。架构没写百分比公式，空闲 5 分钟的平均 CPU 用 `cpu_time_100ns` 的差值除以墙钟。
- `wakeups_per_sec` 是 `\Thread(<进程名>*)\Context Switches/sec` 里 `ID Process` 等于目标 PID 的线程之和。`lanwork.exe` 对应架构里的 `\Thread(lanwork*)\Context Switches/sec`。Windows 计数器的进程名最多 15 个字符，更长的名字只用前 15 个，再用 PID 过滤。

第一行数据之前会先读一次基线，那一次不写进 CSV。没有裁剪工作集的参数。

对空 Slint 窗口采样 5 分钟要在 Windows 11 桌面会话里做。仓库里的单元测试和 Windows CI 不能代替，这项仍是未测。在仓库根目录打开 PowerShell，整段粘贴。大约 5 分钟后结束。它会启动 release 版空窗口，按 PID 采样 300 秒，然后关掉窗口。CSV 和模板的填写副本写到仓库下的 `measurements-out\`，不写入 `%USERPROFILE%\Documents\Lanwork`，也不读取 MayDolist。空白的 `docs/measurements/TEMPLATE.md` 保持不动。

```powershell
$ErrorActionPreference = 'Stop'
Set-Location ((git rev-parse --show-toplevel).Trim())

cargo build -p lanwork -p lanwork-sample --release
if ($LASTEXITCODE -ne 0) { throw "cargo build 失败：$LASTEXITCODE" }

$outDir = Join-Path (Get-Location) 'measurements-out'
New-Item -ItemType Directory -Force -Path $outDir | Out-Null
$csv = Join-Path $outDir 'empty-window.csv'
$record = Join-Path $outDir 'empty-window.md'
if (Test-Path $csv) { Remove-Item $csv }

$exe = Join-Path (Get-Location) 'target\release\lanwork.exe'
$app = Start-Process -FilePath $exe -WorkingDirectory (Get-Location) -PassThru
try {
    Start-Sleep -Seconds 2
    if ($app.HasExited) { throw "lanwork.exe 启动后立即退出，退出码 $($app.ExitCode)。需要有桌面的 Windows 11 会话。" }
    & cargo run -p lanwork-sample --release -- sample --pid $app.Id --duration-secs 300 --interval-ms 1000 --out $csv
    if ($LASTEXITCODE -ne 0) { throw "采样失败：$LASTEXITCODE" }
}
finally {
    if (-not $app.HasExited) { Stop-Process -Id $app.Id -Force }
}

$rows = Import-Csv -Path $csv
$columns = @('utc','pid','private_bytes','working_set_bytes','handles','user_objects','gdi_objects','cpu_time_100ns','cpu_percent','wakeups_per_sec')
if ($rows.Count -lt 290) { throw "CSV 只有 $($rows.Count) 行数据，5 分钟采样不完整。" }
foreach ($row in $rows) {
    foreach ($name in $columns) {
        if ([string]::IsNullOrWhiteSpace($row.$name)) { throw "CSV 列 $name 有空值。" }
    }
}
$firstUtc = [datetimeoffset]::Parse([string]$rows[0].utc)
$lastUtc = [datetimeoffset]::Parse([string]$rows[-1].utc)
$span = ($lastUtc - $firstUtc).TotalSeconds
if ($span -lt 290) { throw "CSV 首尾只隔 $span 秒。" }

$peakPrivate = ($rows | ForEach-Object { [int64]$_.private_bytes } | Measure-Object -Maximum).Maximum
$peakWorking = ($rows | ForEach-Object { [int64]$_.working_set_bytes } | Measure-Object -Maximum).Maximum
$cpu0 = [int64]$rows[0].cpu_time_100ns
$cpu1 = [int64]$rows[-1].cpu_time_100ns
$cpuPercent = [math]::Round((($cpu1 - $cpu0) / 10000000.0) / $span * 100, 3)
$wakeAvg = [math]::Round((($rows | ForEach-Object { [double]$_.wakeups_per_sec } | Measure-Object -Average).Average), 3)

$commit = (git rev-parse HEAD).Trim()
$date = Get-Date -Format 'yyyy-MM-dd'
$cv = Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion'
$windowsBuild = "$($cv.ProductName) $($cv.DisplayVersion) build $($cv.CurrentBuild).$($cv.UBR)"
$cs = Get-CimInstance Win32_ComputerSystem
$machine = "$($cs.Manufacturer) $($cs.Model); logical CPUs $($cs.NumberOfLogicalProcessors); RAM $([math]::Round($cs.TotalPhysicalMemory / 1GB, 1)) GiB"
$gpu = (Get-CimInstance Win32_VideoController | ForEach-Object { "$($_.Name) driver $($_.DriverVersion)" }) -join '; '
$machine = $machine.Replace('|', '/')
$gpu = $gpu.Replace('|', '/')
$windowsBuild = $windowsBuild.Replace('|', '/')

$templatePath = Join-Path (Get-Location) 'docs\measurements\TEMPLATE.md'
$text = [System.IO.File]::ReadAllText($templatePath)
$text = $text.Replace('| 日期 | |', "| 日期 | $date |")
$text = $text.Replace('| commit | |', "| commit | $commit |")
$text = $text.Replace('| 构建配置 | release 或 debug |', '| 构建配置 | release |')
$text = $text.Replace('| Slint 精确版本 | |', '| Slint 精确版本 | 1.18.1 |')
$text = $text.Replace('| 渲染器 | 技术验证选定之前，写这次实际使用的渲染器。空窗口的默认 features 不代表已经选定 |', '| 渲染器 | Slint 1.18.1 默认 features。技术验证尚未选定 FemtoVG、Skia 或软件渲染 |')
$text = $text.Replace('| 机器 | |', "| 机器 | $machine |")
$text = $text.Replace('| Windows build | |', "| Windows build | $windowsBuild |")
$text = $text.Replace('| GPU 与驱动 | |', "| GPU 与驱动 | $gpu |")
$text = $text.Replace('| 数据规模 | 协议规模，或写明哪里偏离了 |', '| 数据规模 | 偏离协议：空 Slint 窗口，没有加载固定数据 |')
$text = $text.Replace('| 中文输入法 | 微软拼音能组合、上屏，候选窗贴在光标处 | | |', '| 中文输入法 | 微软拼音能组合、上屏，候选窗贴在光标处 | 未测 | |')
$text = $text.Replace('| 中文输入法 | 组合期间 Enter 不触发提交 | | |', '| 中文输入法 | 组合期间 Enter 不触发提交 | 未测 | |')
$text = $text.Replace('| 背景 | 选定渲染器下，透明窗口加 DWM Acrylic 正确显示 | | |', '| 背景 | 选定渲染器下，透明窗口加 DWM Acrylic 正确显示 | 未测 | |')
$text = $text.Replace('| 背景 | 透明失败时能检测到并退回纯色 | | |', '| 背景 | 透明失败时能检测到并退回纯色 | 未测 | |')
$text = $text.Replace('| 渲染器 | FemtoVG、Skia、软件渲染各有一个空面板和一个 200 行列表 | | Private Bytes、工作集、中文字形、二进制体积 |', '| 渲染器 | FemtoVG、Skia、软件渲染各有一个空面板和一个 200 行列表 | 未测 | Private Bytes、工作集、中文字形、二进制体积 |')
$text = $text.Replace('| 外部拖放 | 从资源管理器拖入多个文件能拿到路径 | | |', '| 外部拖放 | 从资源管理器拖入多个文件能拿到路径 | 未测 | |')
$text = $text.Replace('| 外部拖放 | 拖出到资源管理器时只有复制和创建快捷方式 | | |', '| 外部拖放 | 拖出到资源管理器时只有复制和创建快捷方式 | 未测 | |')
$text = $text.Replace('| Everything | 1.4 和 1.5 都能返回结果 | | |', '| Everything | 1.4 和 1.5 都能返回结果 | 未测 | |')
$text = $text.Replace('| Everything | 能区分「未运行」和「未就绪」 | | |', '| Everything | 能区分「未运行」和「未就绪」 | 未测 | |')
$text = $text.Replace('| Windows Search | 只按文件名查询，正文命中不返回 | | 结果 P95、查询带来的 Private Bytes 增量 |', '| Windows Search | 只按文件名查询，正文命中不返回 | 未测 | 结果 P95、查询带来的 Private Bytes 增量 |')
$text = $text.Replace('| 通知 | 到期通知能显示，点击后打开面板并定位该条 | | 安装版和便携版分开记 |', '| 通知 | 到期通知能显示，点击后打开面板并定位该条 | 未测 | 安装版和便携版分开记 |')
$text = $text.Replace('| 热角 | 光标移动 5 分钟、静止 5 分钟 | | CPU 占用、每秒唤醒次数 |', '| 热角 | 光标移动 5 分钟、静止 5 分钟 | 未测 | CPU 占用、每秒唤醒次数 |')
$text = $text.Replace('| 进程 Private Bytes 峰值 | ≤ 100 MiB | | | | | | 同时记录工作集 |', '| 进程 Private Bytes 峰值 | ≤ 100 MiB | 未测 | | | | | 同时记录工作集 |')
$text = $text.Replace('| 热召回 | ≤ 100 ms | | | | | | |', '| 热召回 | ≤ 100 ms | 未测 | | | | | |')
$text = $text.Replace('| 应用、待办、便签结果 | ≤ 50 ms | | | | | | |', '| 应用、待办、便签结果 | ≤ 50 ms | 未测 | | | | | |')
$text = $text.Replace('| Everything 就绪时的文件结果 | ≤ 150 ms | | | | | | |', '| Everything 就绪时的文件结果 | ≤ 150 ms | 未测 | | | | | |')
$text = $text.Replace('| Windows Search 回退时的文件结果 | 只记录，不设目标 | | | | | | |', '| Windows Search 回退时的文件结果 | 只记录，不设目标 | 未测 | | | | | |')
$text = $text.Replace('| 冷启动、首次索引 | 单独记录，不并入上面的 P95 | | | | | | |', '| 冷启动、首次索引 | 单独记录，不并入上面的 P95 | 未测 | | | | | |')
$text = $text.Replace('| 收起后空闲 5 分钟 | 记录平均 CPU 和每秒唤醒次数 | | | | | | |', '| 收起后空闲 5 分钟 | 记录平均 CPU 和每秒唤醒次数 | 未测 | | | | | |')
$text = $text.Replace('| 同时打开 5 个便签悬浮窗 | 单独记录 Private Bytes 增量 | | | | | | |', '| 同时打开 5 个便签悬浮窗 | 单独记录 Private Bytes 增量 | 未测 | | | | | |')
$text = $text.Replace('| 100 次呼出、查询、开关辅助窗口 | 记录 Private Bytes、USER、GDI 是否持续上升 | | | | | | |', '| 100 次呼出、查询、开关辅助窗口 | 记录 Private Bytes、USER、GDI 是否持续上升 | 未测 | | | | | |')
$note = @"
空 Slint 窗口采样 $($rows.Count) 行，首尾间隔 $([math]::Round($span, 1)) 秒。没有加载协议规定的固定数据，没有热召回，也没有结果延迟。上面的性能表保持「未测」，这份副本不能作为「已达到」的依据。

- CSV：$csv
- Private Bytes 峰值：$peakPrivate 字节
- 工作集峰值：$peakWorking 字节
- 平均 CPU：（最后一行 cpu_time_100ns − 第一行）÷ 墙钟 = $cpuPercent%
- wakeups_per_sec 平均：$wakeAvg
- 本次没有设置 LANWORK_DATA_DIR。空窗口也不读它。

"@
$text = $text.Replace("（没有就写「无」。有的话，这份记录不能作为「已达到」的依据。）", $note.TrimEnd())
$text = $text.Replace('- [ ] 对空 Slint 窗口采样 5 分钟，得到每一列都有值的 CSV。命令见 `tools/README.md`。', "- [x] 对空 Slint 窗口采样 5 分钟，得到每一列都有值的 CSV。命令见 ``tools/README.md``。CSV：$csv")
$utf8 = New-Object System.Text.UTF8Encoding $false
[System.IO.File]::WriteAllText($record, $text, $utf8)
Write-Output "CSV: $csv"
Write-Output "记录: $record"
```

填写副本是 `measurements-out\empty-window.md`。环境、空窗口勾选和上面算出的 Private Bytes、工作集、平均 CPU、唤醒次数会写进去。技术验证和「性能测量」协议表填「未测」。这次不设置 `LANWORK_DATA_DIR`：空窗口不读它，这次也不加载夹具数据。

## 延迟汇总

#20 写结果延迟的起点和查询序号，#21 写热召回的起点，并在 `AfterRendering` 写同序号的终点。两边都把原始记录追加成 JSONL，本工具只读这个文件。`lanwork.exe` 在环境变量 `LANWORK_LATENCY_OUT` 非空时把这些记录追加到它指向的文件；搜索条每次收起时写出结果记录，热召回在渲染后立即写出。进程启动后的第一次热召回和第一个查询序号标为 `warmup: true`。

```powershell
$env:LANWORK_LATENCY_OUT = "$PWD\measurements-out\raw.jsonl"
.\target\release\lanwork.exe
```

```text
cargo run -p lanwork-sample -- latency --input measurements-out\raw.jsonl --out summary.json
```

一行一条。不认识的字段会忽略。时间戳用同一种时钟的纳秒，工具只做减法，不把它当成 Unix 时间。

```json
{"metric":"hot_recall","seq":1,"start_ns":100,"end_ns":180,"warmup":false,"superseded":false}
{"metric":"result","source":"local","seq":2,"start_ns":200,"end_ns":240,"warmup":false,"superseded":false}
{"metric":"result","source":"everything","seq":2,"start_ns":200,"end_ns":340,"warmup":false,"superseded":false}
{"metric":"result","source":"windows_search","seq":3,"start_ns":400,"end_ns":null,"warmup":false,"superseded":true}
```

- `hot_recall` 不带 `source`。
- `result` 的 `source` 只能是 `local`、`everything`、`windows_search`。应用、待办和便签在架构里共用一个 P95，所以都写 `local`。
- `warmup: true` 的记录排除。
- `superseded: true` 的记录不进样本，计入被取代数。同时标了预热时，算被取代，不算预热排除。
- 没有 `end_ns` 且没有被取代，算未完成，不进样本。
- `end_ns` 早于 `start_ns` 算无效，不进样本。

P95 是有效时长升序后的第 ⌈0.95 × N⌉ 个，从 1 起计。1 到 100 的结果是 95。有效样本少于 100 时，`sample_status` 为「不足」，`sufficient` 为 false；P95 仍按同一公式给出，但不能当成验收。汇总里保留每条有效样本的 `seq`、`start_ns`、`end_ns`。

记录抄到 [docs/measurements/TEMPLATE.md](../docs/measurements/TEMPLATE.md)。模板不是实测。
