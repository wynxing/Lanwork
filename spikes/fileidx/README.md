# fileidx

Everything SDK 和 Windows Search 文件名查询的技术验证程序。不进 `lanwork` 的依赖，也不进发布包。

DLL 在 `third_party/everything/`。用 `LoadLibraryW` 加载，先探测 SDK3（1.5），再探测 SDK（1.4）。SDK3 先连未命名实例，管道不存在再连 `1.5a`。1.4 的 DLL 只找窗口类 `EVERYTHING_TASKBAR_NOTIFICATION`。

Windows Search 的产品查询是进程内 ADO（`ADODB.Connection`）连接 `Search.CollatorDSO`，SQL 只匹配 `System.FileName`。`ISearchManager` 要用 `CLSCTX_ALL` 创建，`ISearchQueryHelper` 和正文 `CONTAINS` 只作对照。最多 50 条。

SDK3 在 `Everything3_Search` 之前请求 `EVERYTHING3_PROPERTY_ID_NAME`。不请求时，结果条数是对的，`GetResultNameW` 得到空串。1.5 beta 便携版的实例名是 `1.5a`，管道是 `\\.\pipe\Everything IPC (1.5a)`。探测先试未命名实例，管道不存在再试 `1.5a`。

标准输出是 JSON。

```text
cargo run -p fileidx -- probe
cargo run -p fileidx -- probe --sdk sdk14 --client-present
cargo run -p fileidx -- poll --sdk sdk14 --millis 20000
cargo run -p fileidx -- everything --sdk sdk14 --text "名称" --expect 名称.txt --reject 正文.txt
cargo run -p fileidx -- cap --sdk sdk14
cargo run -p fileidx -- wsearch --text "名称" --expect 名称.txt --reject 正文.txt
cargo run -p fileidx -- compare --text "标记" --expect 标记.txt --reject 正文.txt
cargo run -p fileidx -- scopes
cargo run -p fileidx -- load
cargo run -p fileidx -- bench --text "名称" --samples 100 --warmup 5 --out docs/measurements/fileidx
```

`bench` 先采样 1 秒系统 CPU。CPU 不低于 70%，或者同时有 `rustc.exe`、`link.exe`、`cl.exe` 时退出码 3，不写样本。确认可以照常记时再加 `--allow-skew`。

Private Bytes 用 `PROCESS_MEMORY_COUNTERS_EX.PrivateUsage`，和 `lanwork-sample` 的 `private_bytes` 相同。`private_bytes_delta_including_open` 含第一次打开搜索连接。`private_bytes_delta_execute` 只含第一次 `Execute`。

P95 用 `lanwork-sample latency` 读 `windows-search.jsonl`。这里的时间是进程内查询，不含架构里的 60ms 等待，也不含界面渲染。不能当作「性能测量」已经达到。

## 四种状态

本机没有安装 Everything 时，`probe` 的 `state` 是 `not_installed`。SDK3 的 `last_error` 是 `0xE0000002`，SDK 1.4 的 `last_error` 是 `2`。

便携版放在 `spikes/fileidx/.portable/`，这个目录被 git 忽略。不要用安装包，也不要启动或停止 Windows 服务。

已安装但未运行：解压便携版，先不要启动，再执行 `probe --sdk sdk14 --client-present` 或 `probe --sdk sdk3 --client-present`。

数据库还在加载：删掉便携目录里的 `Everything.db`，启动 `Everything.exe`，立刻 `poll`。看见 `not_ready` 才算观察到「未就绪」。1.4 的未就绪是窗口已经在、`IsDBLoaded` 为假、`LastError` 为 0。SDK3 是已经连上管道、`IsDBLoaded` 为假。

就绪：`poll` 或 `everything` 的 `kind` 为 `ready`，并且查询能返回文件名。

用完之后执行 `Everything.exe -exit`，不要留着进程。

## WSearch 停止

没有管理员权限时不要停止 `WSearch`。需要人在提升过的 PowerShell 里做，做完要恢复：

```powershell
Get-Service WSearch | Format-List Name,Status,StartType
Stop-Service WSearch
# 在仓库根目录：
cargo run -p fileidx -- wsearch --text notepad
Start-Service WSearch
Get-Service WSearch | Format-List Name,Status,StartType
```

把 JSON 里的 `hresult` 和 `message` 记进测量记录。停止之前先记下原来的 `StartType`，不要改它。
