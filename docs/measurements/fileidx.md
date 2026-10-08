# fileidx 技术验证记录

`spikes/fileidx` 在这台 Windows 桌面上的观察。技术验证里的测量只说明这次场景，不算 architecture.md「性能测量」已经达到。空白的 [TEMPLATE.md](TEMPLATE.md) 没有改。

## 环境

| 字段 | 值 |
| --- | --- |
| 日期 | 2026-10-08 |
| commit | `46329e23be6edf46562c88310253abd184e0099e` |
| 构建配置 | release |
| Slint 精确版本 | 1.18.1。这次程序不创建 Slint 窗口 |
| 渲染器 | 未使用。技术验证尚未选定渲染器 |
| 机器 | XIAOMI REDMI Book 14 2025(FHD+); logical CPUs 12; RAM 15.7 GiB |
| Windows build | 注册表 `ProductName` 为 Windows 10 Home China，`DisplayVersion` 26H2，build 26300.9550 |
| GPU 与驱动 | Intel(R) UHD Graphics driver 32.0.101.6733；另有 Virtual Display Driver 11.30.4.434 与 GameViewer Virtual Display Adapter 15.6.5.199 |
| 数据规模 | 偏离协议。没有加载 `lanwork-fixture` 的固定数据。Windows Search 的延迟样本是对已索引文件名 `notepad` 的 100 次进程内查询 |

## 技术验证

| 项 | 通过条件 | 结果 | 附带的测量 |
| --- | --- | --- | --- |
| Everything | 1.4 和 1.5 都能返回结果 | 通过 | 见下。原始 JSON 在 `docs/measurements/fileidx/` |
| Everything | 能区分「未运行」和「未就绪」 | 通过 | 1.4 与 1.5 都观察到了 |
| Windows Search | 只按文件名查询，正文命中不返回 | 通过 | P95 与 Private Bytes 见下 |
| 其余技术验证项 | 见模板 | 未测 | |

`WSearch` 停止时的错误码是 issue 里的异常项，这次未测。没有管理员权限，没有停止服务。

## Windows Search

选定的调用方式是进程内 ADO：`ADODB.Connection`，提供程序 `Search.CollatorDSO.1`。产品 SQL 只含 `System.FileName LIKE`，`TOP` 不超过 50。`ISearchQueryHelper` 默认生成 `CONTAINS(*)`，对照里会同时命中文件名和正文，不作为产品查询。`ISearchManager` 用 `CLSCTX_ALL` 创建；只用 `CLSCTX_INPROC_SERVER` 时返回 `0x80040154`。

索引根（`scopes`）：当前用户的 `defaultroot://` 与 `winrt://`，以及 `file:///C:\`。

在 `%USERPROFILE%\Documents\LanworkFileidxProbe` 放了探针文件，查完后已删除。

| 查询 | 观察 |
| --- | --- |
| `Fileidx Space` | `LIKE '%Fileidx Space%'`，命中 `Lanwork Fileidx Space.txt` |
| `兰工文件名` | 命中 `兰工文件名.txt` |
| `lanworkprobe?mid.txt` | `LIKE 'lanworkprobe_mid.txt'`，命中 `lanworkprobeXmid.txt` |
| `LanworkFileidx*` | `LIKE 'LanworkFileidx%'`，命中 `LanworkFileidxBodyOnly.txt` |
| `*`，请求 80 条 | SQL 被夹到 `TOP 50`，返回 50 条 |
| 文件名 `LWXCONTENT9F3A2C` | `LIKE` 只返回 `LWXCONTENT9F3A2C-name.txt`，不返回 `LanworkFileidxBodyOnly.txt` |
| 正文 `CONTAINS('LWXCONTENT9F3A2C')` | 只返回 `LanworkFileidxBodyOnly.txt`。该文件的正文含这个记号，文件名不含 |
| `ISearchQueryHelper` 默认 | `CONTAINS(*)`，同一次查询同时返回上面两个文件 |

延迟样本是 `bench --text notepad --samples 100 --warmup 5`。查询前系统 CPU 60.6%，`rustc`/`link`/`cl` 为 0，`cargo` 为 2（另一项测量正在跑 `lanwork-sample`，不是编译）。查询后系统 CPU 16.8%，同样没有编译进程。两次都没有被标成明显干扰。更早几次查询后 CPU 超过 70%，那些样本没有保留。

`lanwork-sample latency` 的结果：有效样本 100，预热排除 6，被取代 0，`sufficient` 为 true，`sample_status` 为「足够」，P95 是第 95 个，`p95_ns` 为 52242300（52.242 ms）。原始时间戳是 `windows-search.jsonl`，汇总是 `windows-search-summary.json`。

Private Bytes 用 `PROCESS_MEMORY_COUNTERS_EX.PrivateUsage`：

| 点 | 字节 |
| --- | --- |
| 打开连接之前 | 1286144 |
| 打开连接之后、第一次 `Execute` 之前 | 2400256 |
| 第一次 `Execute` 之后 | 3018752 |
| 只含第一次 `Execute` 的增量 | 618496 |
| 含打开连接的增量 | 1732608 |

这次 P95 不含架构里的 60 ms 等待，也不含界面渲染。不能当作文件结果 P95 已经达到。

当时 `WSearch` 的状态是 Running，`StartType` 是 Manual。

## Everything

本机没有系统安装：`HKLM` 和 `HKCU` 的 `SOFTWARE\voidtools\Everything` 都不存在，没有 Everything 服务，也没有正在运行的 `Everything.exe`。`probe` 的 `state` 是 `not_installed`。SDK3 的 `last_error` 是 `0xE0000002`，SDK 1.4 的 `last_error` 是 `2`。

便携版只放在 git 忽略的 `spikes/fileidx/.portable/`，没有安装，也没有写入 `%APPDATA%\Everything`。用完后已执行 `-exit`，进程已退出。注册表键仍然不存在。

| 包 | 官方 URL | SHA-256 |
| --- | --- | --- |
| Everything 1.4.1.1032 x64 便携 zip | https://www.voidtools.com/Everything-1.4.1.1032.x64.zip | `698df475ec44e638f66f1b6a32d28fea613cec78d3b6310e6abe53431eeb940c` |
| Everything 1.5.0.1423b x64 便携 zip | https://www.voidtools.com/Everything-1.5.0.1423b.x64.zip | `a218cebffccfd9dfaa5aa4bbb665d6e4b5533049b1b9ca7e3828385b5cb8142c` |

这两份哈希与官网 `Everything-1.4.1.1032.sha256`、`Everything-1.5.0.1423b.sha256` 里的对应行一致。压缩包不进仓库。随包 DLL 的版本和许可在 `third_party/everything/README.md`。

1.5.0.1423b 用 `-instance 1.5a` 启动。管道名是 `\\.\pipe\Everything IPC (1.5a)`。未命名实例返回 `0xE0000002`，接着连上 `1.5a`。就绪时 SDK3 报告的服务端版本是 `1.5.0.1423`。

| 状态 | 1.4 | 1.5 |
| --- | --- | --- |
| 未安装 | `probe` 的 `state` 为 `not_installed`，`last_error` 为 2 | 同一次探测里 SDK3 为 `0xE0000002` |
| 已安装但未运行 | 便携目录在，进程没启动，`--client-present` 后 `state` 为 `installed_not_running`，`last_error` 为 2 | `--client-present` 后同样是 `installed_not_running`，未命名实例和 `1.5a` 都是 `0xE0000002` |
| 运行中但数据库还在加载 | `everything-14-poll.json`：t=1251 ms，`is_db_loaded` 为 false，`last_error` 为 0，`kind` 为 `not_ready` | `everything-15-not-ready.json`：已连上 `1.5a`，`is_db_loaded` 为 false，`last_error` 为 0，`kind` 为 `not_ready`，`state` 为 `running_db_loading` |
| 就绪 | 同一份 poll 在 t=1263 ms 变为 `ready`，随后查询能返回文件名 | `everything-15-became-ready.json` 的 `server_version` 为 `1.5.0.1423`，`is_db_loaded` 为 true |

1.5 的「未就绪」是在把文件夹索引指到 `C:\Windows\System32` 的启动过程里看到的：约 85 ms、215 ms、254 ms、293 ms、331 ms 为 `not_ready`，386 ms 为 `ready`。只索引探针目录时，第一次探测（约 112–149 ms）已经是 `ready`，没有留下 `not_ready` 样本。看到 `ready` 之后已退出进程，没有改 Windows Search，也没有安装服务。

就绪后的文件名查询（1.4 这次的系统 CPU 23.4%，没有标成干扰）：

| SDK | 查询 | 结果 | 耗时 |
| --- | --- | --- | --- |
| 1.4 | `兰工文件名` | 命中 `兰工文件名.txt` | 6.214 ms |
| 1.4 | `Fileidx Space` | 命中 `Lanwork Fileidx Space.txt` | 5.495 ms |
| 1.4 | `lanworkprobe?mid.txt` | 命中 `lanworkprobeXmid.txt` | 4.929 ms |
| 1.4 | `LanworkFileidx*` | 命中 `LanworkFileidxBodyOnly.txt` | 6.013 ms |
| 1.4 | `capfile`，请求 80 条 | 返回 50，`total` 为 60，`limit` 为 50 | 7.331 ms |
| 1.5 `1.5a` | `兰工文件名` | 命中 `兰工文件名.txt` | 0.283 ms |
| 1.5 `1.5a` | `Fileidx Space` | 命中 `Lanwork Fileidx Space.txt` | 0.506 ms |
| 1.5 `1.5a` | `lanworkprobe?mid.txt` | 命中 `lanworkprobeXmid.txt` | 0.467 ms |
| 1.5 `1.5a` | `LanworkFileidx*` | 命中 `LanworkFileidxBodyOnly.txt` | 0.262 ms |
| 1.5 `1.5a` | `capfile`，请求 80 条 | 返回 50，`limit` 为 50，命中 `capfile-01.txt` | 2.549 ms |

`cap` 子命令会向 SDK 要 51 条，用来看 SDK 自己是否截断。1.4 在上限 50 时返回 50，上限 51 时返回 51，`total` 为 66。1.5 同样是 50 和 51。产品查询在调用 SDK 之前把条数夹到 50，所以上面「请求 80 条」的结果是 50。

1.4 与 1.5 的单次查询都远低于架构里 Everything 文件结果 150 ms 的目标，但样本不是 100 次，也不含 60 ms 等待和界面渲染。这里不写「已达到」。

## 给人做的 WSearch 停止检查

这次没有管理员权限，没有停止 `WSearch`。当时服务是 Running，启动类型是 Manual。请在提升过的 PowerShell 里做，做完恢复，不要改 `StartType`：

```powershell
Get-Service WSearch | Format-List Name,Status,StartType
Stop-Service WSearch
Set-Location E:\My_project\Lanwork-wt\issue-6
cargo run -p fileidx --release -- wsearch --text notepad
Start-Service WSearch
Get-Service WSearch | Format-List Name,Status,StartType
```

把 JSON 里的 `hresult` 和 `message` 补进这份记录。停止期间如果查询挂起或崩溃，记下来，不要把它写成已经观察到的错误码。
