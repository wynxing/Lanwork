---
name: lanwork-tool
description: >-
  为 Lanwork（Windows 11，Rust + Slint）编写、修改或打包可导入的悬浮窗工具（manifest.toml 与 Lua 5.4）。
  用户要做工具、问清单字段、组件、权限、http/密钥、导入错误或打包方式时使用。
  先读 docs/tool-dev-guide.md，再按最小权限写能通过导入校验的文件夹或 zip。
  不要用在 Lanwork 本体（搜索、待办页、便签、收纳、设置）上，也不要发明指南里没有的 API。
---

# Lanwork 悬浮窗工具

细节以仓库里的 [docs/tool-dev-guide.md](../../docs/tool-dev-guide.md) 为准。那篇按当前代码写。这里只留工作顺序、模板和会让导入或运行失败的限制。规格 [docs/tools.md](../../docs/tools.md) 里还没实现的交互，不要写进工具。

## 工作顺序

1. 先问清这扇窗口要显示什么、用户点什么、要不要记住、要不要待办、通知、网络和密钥。用户没说的权限不要加。
2. 只声明用到的服务。能用计时组件完成的倒数，不要声明 `timer`。不联网就不要 `http` 和 `secret`。
3. 按下面的模板写 `manifest.toml` 和入口脚本。组件只用 `text`、`button`、`timer`、`progress`。
4. 打包成文件夹，或根上带清单的 zip。告诉用户怎么导入，见文末。
5. 用文末的自检清单过一遍，再把文件交给用户。

参考实现：联网的额度工具在 `examples/api-quota`，不联网的倒计时在 `examples/countdown`。倒计时的 `services` 是空数组，确认页上没有权限句子。

## 清单模板

删掉用不到的 `http`、`secret` 和 `services` 项。`component` 这个键必须留着。不要写 `version` 或其它字段。

```toml
id = "example.my-tool"
name = "工具名称"
description = "一行短说明"
entry = "main.lua"
services = []

[[component]]
id = "summary"
component = "text"
text = ""

[[component]]
id = "action"
component = "button"
label = "开始"
```

需要网络时改成：

```toml
services = ["http", "secret"]

[http]
domains = ["api.example.com"]

[[secret]]
id = "api_key"
label = "API 密钥"
```

`domains` 只写主机名，不写 `https://`、端口、路径和通配符。密钥的 `label` 是确认页和「填写密钥」里给用户看的名称。

## 脚本模板

```lua
function on_load()
  ui.text("summary", "准备")
end

function on_click(id)
  if id ~= "action" then
    return
  end
  ui.text("summary", "已点")
end
```

计时组件要自己开始倒数：

```lua
ui.timer("clock", 60)
ui.timer_running("clock", true)
```

只调用 `ui.timer` 时数字会改，但不会走，也不会进 `on_timer`。`on_timer(id)` 在正在倒数并走到 0 时调用。

定时刷新：

```lua
timer.every(300, refresh)
```

间隔必须大于或等于 30。窗口关掉后定时器取消，不会在后台继续跑。再打开时靠 `on_load` 重新登记。

## API 速查

未声明的服务不要调用。调用了窗口会显示「未授权」，`pcall` 接不住。

| 声明 | 脚本 | 确认页句子 |
| --- | --- | --- |
| 始终有 | `ui.text` `ui.button` `ui.timer` `ui.timer_running` `ui.progress` `json.decode` `json.encode` | 无 |
| `timer` | `timer.every` `timer.after` `timer.cancel` | 定时刷新 |
| `notify` | `notify.send(title, body)` | 发送通知 |
| `storage` | `storage.get` `storage.set` | 保存这个工具自己的数据 |
| `todo.read` | `todo.list` `on_todos_changed` | 读取你的待办 |
| `todo.complete` | `todo.complete(id)` | 完成你的待办 |
| `http` | `http.request({ method = "GET", url = "...", secret = 可省略 }, fn)` | 每个域名一句「访问 <主机名>」 |
| `secret` | 没有读取函数。只在 `http.request` 里写 `secret.id` | 每个密钥一句「使用你填写的<名称>」 |

`http` 当前只发送已声明主机上的 `https` GET，不跟随重定向。附带密钥时必须是 `https`。没填密钥时回调里的 `error` 是「未填写密钥」，请求不会发出。

标准库只有 `string`、`table`、`math`、`utf8`，外加基础函数。没有 `io`、`os`、`package`、`debug`、`coroutine`、`require`、`load`、`print`。

回调：`on_load`、`on_unload`、`on_click`、`on_timer`、`on_todos_changed`。一次回调最长约 1 秒。

## 硬性限制

- `id` 是单个路径分量，1 到 200 字节，不能以 `.` 或 `lanwork.` 开头，不能是 Windows 设备名。
- 不认识的字段、组件、服务，或 `http` / `secret` 和 `services` 对不齐，界面只显示「清单无效」。
- 域名不能是 IP，不能带协议、端口、路径、通配符。
- `list`、`input`、`toggle` 现在只显示成一行文本，没有点击、输入和对应的 `ui` 函数。不要用。
- 导入只复制清单、入口 `.lua`，以及文件头是 PNG 的根上 `icon.png`。其它文件不复制。
- 单个文件最大 2 MiB。zip 最大 8 MiB，解压后合计最大 8 MiB，最多 64 个文件。
- `storage` 当前大约是：键 256 个字符，值 256 KiB，最多 256 条。超出时「写盘失败」，原文件保留。
- `timer.every` 短于 30 秒显示「间隔过短」，不登记。
- 密钥不能写进脚本、清单或 `storage`。脚本读不到原文。

## 交给用户

导入：打开面板的「应用」，点「导入+」，选这个文件夹或 zip；也可以把文件夹或 zip 拖到应用页上。确认页会列出名称、短说明和权限句子，要整份接受。取消则不安装。

同一 id 再导入是更新，确认页有「保留数据」。存储和密钥还在。

声明了密钥时，卡片「⋯」里有「填写密钥」。掩码输入，不回显。保存之后，已经打开的窗口会重载并再跑 `on_load`。

窗口已经开着时再点「悬浮窗」不会重新加载。要看新脚本，先关掉再打开，或再导入一次。

## 自检

- [ ] 只声明了脚本真正调用的服务
- [ ] 没有 `version` 和其它未列出的字段
- [ ] `id` 不以 `lanwork.` 开头
- [ ] 有 `http` 就有 `domains`，有 `secret` 就至少有一项 `id` 和 `label`，两边都和 `services` 对齐
- [ ] 域名是纯主机名
- [ ] 组件只用 `text`、`button`、`timer`、`progress`，id 不重复
- [ ] 计时组件在需要倒数时调用了 `ui.timer_running`
- [ ] `timer.every` 的秒数大于或等于 30
- [ ] 请求是 `https` GET，主机名写在 `domains` 里，密钥只按 id 引用
- [ ] 没有 `print`、`require`、`io`、`os`、协程
- [ ] 文件夹根上有 `manifest.toml` 和入口脚本；zip 根上有清单，或只有一个顶层文件夹
- [ ] 告诉了用户从「应用」页导入，以及密钥要在「填写密钥」里填
