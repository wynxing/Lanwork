# fileidx 技术验证记录

`spikes/fileidx` 在这台 Windows 桌面上的观察。技术验证里的测量只说明这次场景，不算 architecture.md「性能测量」已经达到。空白的 [TEMPLATE.md](TEMPLATE.md) 没有改。

这次数字来自修复 VARIANT 双重释放和 COM 卸载顺序之后的 release 构建。修复前那次 Windows Search 的 P95 和 Private Bytes 作废，不沿用。

## 环境

| 字段 | 值 |
| --- | --- |
| 日期 | 2026-10-09 |
| commit | `9b82c6b71e8a7303f74b872456e8c716ba0d9149` |
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

`WSearch` 停止时的结果见文末「人工停止 WSearch 结果」。`message` 是空字符串，错误提示文案不能依赖它，列为后续。

`loadmon` 把系统 CPU 不低于 70%，或同时有 `rustc`、`link`、`cl`，标成明显干扰。这是 spike 自己的筛选，用来决定这次计时要不要重跑。架构「性能测量」协议里没有这条门槛。

## Windows Search

spike 当前用进程内 ADO：`ADODB.Connection`，提供程序 `Search.CollatorDSO.1`，SQL 含 `System.FileName LIKE`，`TOP` 不超过 50。匹配方式（子串、前缀或整名，以及 `*`、`?` 与 `LIKE` 通配符是否同义）待定，产品规格只写到按名称。下面的 SQL 只说明这次程序实际发出的语句。

`ISearchQueryHelper` 默认生成 `CONTAINS(*)`，对照里会同时命中文件名和正文，不作为产品查询。`CSearchManager` 用 `CLSCTX_INPROC_SERVER` 创建失败，用 `CLSCTX_ALL` 成功。见 `windows-search-manager.json`：`inproc_server` 的 `hresult` 是 2147746132（`0x80040154`），`message` 是「没有注册类」；`all` 的 `ok` 为 true。

索引根没有在这次重测里重新枚举。

在 `%USERPROFILE%\Documents\LanworkFileidxProbe` 放了探针文件，查完后已删除。

| 查询 | JSON | 观察 |
| --- | --- | --- |
| `Fileidx Space` | `windows-search-space.json` | `LIKE '%Fileidx Space%'`，命中 `Lanwork Fileidx Space.txt` |
| `兰工文件名` | `windows-search-chinese.json` | 命中 `兰工文件名.txt` |
| `lanworkprobe?mid.txt` | `windows-search-wild.json` | 这次程序写成 `LIKE 'lanworkprobe_mid.txt'`，命中 `lanworkprobeXmid.txt` |
| `LanworkFileidx*` | `windows-search-star.json` | 这次程序写成 `LIKE 'LanworkFileidx%'`，返回 2 条，其中有 `LanworkFileidxBodyOnly.txt` |
| `*`，请求 80 条 | `windows-search-cap.json` | SQL 被夹到 `TOP 50`，`returned` 为 50，`limit` 为 50 |
| 文件名 `LWXCONTENT9F3A2C` | `windows-search-filename-token.json` | `LIKE` 只返回 `LWXCONTENT9F3A2C-name.txt`，`reject_hit` 为 false，不返回 `LanworkFileidxBodyOnly.txt` |
| 正文 `CONTAINS('LWXCONTENT9F3A2C')` | `windows-search-content.json` | 只返回 `LanworkFileidxBodyOnly.txt`。该文件的正文含这个记号，文件名不含 |
| `ISearchQueryHelper` 默认 | `windows-search-compare.json` 的 `helper_default` | `CONTAINS(*)`，同一次查询返回上面两个文件 |

延迟样本是 `bench --text notepad --samples 100 --warmup 5`。查询前系统 CPU 41.0%，`rustc`/`link`/`cl`/`cargo` 都是 0。查询后系统 CPU 13.6%，同样没有这些进程。两次都没有被标成明显干扰。当时另有 agent 在别的工作树，但这次采样里没有编译进程。

`bench` 把第一次 `Execute` 另记成一条 `warmup: true`，然后再跑 `--warmup 5`。所以 JSONL 有 106 行：预热 6 行，有效样本 100 行。多出的那一条就是第一次 `Execute`，它同时用来量 Private Bytes，不进入 P95。

`lanwork-sample latency` 的结果：有效样本 100，预热排除 6，被取代 0，`sufficient` 为 true，`sample_status` 为「足够」，P95 是第 95 个，`p95_ns` 为 52705000（52.705 ms）。原始时间戳是 `windows-search.jsonl`，汇总是 `windows-search-summary.json`，bench 元数据是 `windows-search-bench.json`。

Private Bytes 用 `PROCESS_MEMORY_COUNTERS_EX.PrivateUsage`：

| 点 | 字节 |
| --- | --- |
| 打开连接之前 | 1277952 |
| 打开连接之后、第一次 `Execute` 之前 | 2330624 |
| 第一次 `Execute` 之后 | 2785280 |
| 只含第一次 `Execute` 的增量 | 454656 |
| 含打开连接的增量 | 1507328 |

这次 P95 不含架构里的 60 ms 等待，也不含界面渲染。不能当作文件结果 P95 已经达到。

当时 `WSearch` 的状态是 Running，`StartType` 是 Manual。

## Everything

本机没有系统安装：`HKLM` 和 `HKCU` 的 `SOFTWARE\voidtools\Everything` 都不存在，没有 Everything 服务，也没有正在运行的 `Everything.exe`。`everything-not-installed.json` 的 `state` 是 `not_installed`。SDK3 未命名实例和 `1.5a` 的 `last_error` 都是原始值 `0xE0000002`。SDK 1.4 的 `last_error` 是 `2`。连接失败且 `GetLastError` 为 0 时，程序不改写成 `0xE0000002`；这次观察到的不是 0。

便携版只放在 git 忽略的 `spikes/fileidx/.portable/`，没有安装，也没有写入 `%APPDATA%\Everything`。用完后已执行 `-exit`，进程已退出。注册表键仍然不存在。

| 包 | 官方 URL | SHA-256 |
| --- | --- | --- |
| Everything 1.4.1.1032 x64 便携 zip | https://www.voidtools.com/Everything-1.4.1.1032.x64.zip | `698df475ec44e638f66f1b6a32d28fea613cec78d3b6310e6abe53431eeb940c` |
| Everything 1.5.0.1423b x64 便携 zip | https://www.voidtools.com/Everything-1.5.0.1423b.x64.zip | `a218cebffccfd9dfaa5aa4bbb665d6e4b5533049b1b9ca7e3828385b5cb8142c` |

这两份哈希与官网 `Everything-1.4.1.1032.sha256`、`Everything-1.5.0.1423b.sha256` 里的对应行一致。压缩包不进仓库。随包 DLL 的版本和许可在 `third_party/everything/README.md`。

1.5.0.1423b 用 `-instance 1.5a` 启动。管道名是 `\\.\pipe\Everything IPC (1.5a)`。未命名实例返回原始 `0xE0000002`，接着连上 `1.5a`。就绪时 SDK3 报告的服务端版本是 `1.5.0.1423`。

| 状态 | 1.4 | 1.5 |
| --- | --- | --- |
| 未安装 | `everything-not-installed.json`：`last_error` 为 2，`kind` 为 `not_running`，`state` 为 `not_installed` | 同一份文件里 SDK3 未命名实例和 `1.5a` 都是 `0xE0000002`，`kind` 为 `not_running` |
| 已安装但未运行 | `everything-14-installed-not-running.json`：便携目录在，进程没启动，`--client-present` 后 `state` 为 `installed_not_running`，`last_error` 为 2 | `everything-15-not-running.json`：`--client-present` 后 `state` 为 `installed_not_running`，未命名实例和 `1.5a` 都是 `0xE0000002` |
| 运行中但数据库还在加载 | `everything-14-poll.json`：t=68 ms，`is_db_loaded` 为 false，`last_error` 为 0，`kind` 为 `not_ready` | `everything-15-not-ready.json`：已连上 `1.5a`，`is_db_loaded` 为 false，`last_error` 为 0，`kind` 为 `not_ready`，`state` 为 `running_db_loading` |
| 就绪 | 同一份 poll 在 t=75 ms 变为 `ready` | `everything-15-became-ready.json` 的 `server_version` 为 `1.5.0.1423`，`is_db_loaded` 为 true |

1.4 这份 poll 之前系统 CPU 8.4%，之后 21.6%，`rustc`/`link`/`cl`/`cargo` 都是 0，两次都没有被标成明显干扰。更早一次 poll 结束后 CPU 到了 74.5%，那次样本没有保留。

1.5 的「未就绪」是在只索引探针目录的启动里用单次 `probe` 看到的。没有一份完成的 poll JSON，所以这里不写毫秒。把文件夹指到 `C:\Windows\System32` 时，管道出现之后 `Everything3_ConnectW` 在 1.2 秒内没有返回，那些探测被杀掉了，没有留下 `not_ready` 样本。`everything-15-not-ready.json` 的 `client_present` 是 false，因为那次命令没有加 `--client-present`；`state` 仍是 `running_db_loading`，依据是 `kind` 为 `not_ready`。看到 `ready` 之后已退出进程，没有改 Windows Search，也没有安装服务。

就绪后的文件名查询。1.4 查询前系统 CPU 60.9%，没有编译进程，没有被标成明显干扰。1.5 查询前 6.5%，查询后 16.0%，同样没有编译进程，两次都没有被标成明显干扰。

| SDK | 查询 | JSON | 结果 | 耗时 |
| --- | --- | --- | --- | --- |
| 1.4 | `兰工文件名` | `everything-14-chinese.json` | 命中 `兰工文件名.txt` | 7.212 ms |
| 1.4 | `Fileidx Space` | `everything-14-space.json` | 命中 `Lanwork Fileidx Space.txt` | 7.292 ms |
| 1.4 | `lanworkprobe?mid.txt` | `everything-14-wild.json` | 命中 `lanworkprobeXmid.txt` | 6.740 ms |
| 1.4 | `LanworkFileidx*` | `everything-14-star.json` | 命中 `LanworkFileidxBodyOnly.txt` | 10.914 ms |
| 1.4 | `capfile`，请求 80 条 | `everything-14-cap-query.json` | 返回 50，`total` 为 60，`limit` 为 50 | 8.782 ms |
| 1.5 `1.5a` | `兰工文件名` | `everything-15-chinese.json` | 命中 `兰工文件名.txt` | 1.576 ms |
| 1.5 `1.5a` | `Fileidx Space` | `everything-15-space.json` | 命中 `Lanwork Fileidx Space.txt` | 0.395 ms |
| 1.5 `1.5a` | `lanworkprobe?mid.txt` | `everything-15-wild.json` | 命中 `lanworkprobeXmid.txt` | 0.673 ms |
| 1.5 `1.5a` | `LanworkFileidx*` | `everything-15-star.json` | 命中 `LanworkFileidxBodyOnly.txt` | 0.269 ms |
| 1.5 `1.5a` | `capfile`，请求 80 条 | `everything-15-cap-query.json` | 返回 50，`limit` 为 50，命中 `capfile-01.txt` | 1.444 ms |

`cap` 子命令会向 SDK 要 51 条，用来看 SDK 自己是否截断。1.4（`everything-14-cap.json`）在上限 50 时返回 50，上限 51 时返回 51，`total` 为 66。1.5（`everything-15-cap.json`）同样是 50 和 51，`total` 为空。产品查询在调用 SDK 之前把条数夹到 50，所以上面「请求 80 条」的结果是 50。

1.4 与 1.5 的单次查询都远低于架构里 Everything 文件结果 150 ms 的目标，但样本不是 100 次，也不含 60 ms 等待和界面渲染。这里不写「已达到」。

## 人工停止 WSearch 结果

2026-10-09 14:24（UTC+8），DESKTOP-7C3P6OG，issue-6 工作树 `08b28fe35deb47b0ac9278e4700bf6e51af7bad1`，release 构建。管理员 PowerShell 脚本依次执行了下面这些。

初始 `WSearch`：Status 为 Running，StartType 为 Manual。

`Stop-Service WSearch -Force` 之后：Status 为 Stopped。

然后运行 `fileidx.exe wsearch --text notepad`。程序没有崩溃，也没有卡住。输出 JSON：

| 字段 | 值 |
| --- | --- |
| mode | `filename_like` |
| ok | false |
| hresult | 2147614729（`0x80020009`，`DISP_E_EXCEPTION`） |
| message | 空字符串 |
| elapsed_ns | 5322000（约 5.3 ms） |
| returned | 0 |
| matching_names | `[]` |

随后 `Start-Service` 恢复 `WSearch`。最终 Status 为 Running，StartType 为 Manual。

`message` 是空字符串。错误提示文案不能依赖它。这可以列为后续。
