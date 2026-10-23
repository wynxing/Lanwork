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

参考实现：联网的额度工具在 `examples/api-quota`，不联网的倒计时在 `examples/countdown`。本机登录用量都写成 `[[login]]`：`examples/chatgpt-usage`、`examples/claude-usage`、`examples/grok-usage`。倒计时的 `services` 是空数组，确认页上没有权限句子。

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
header = "Authorization"
prefix = "Bearer "
```

要提交 JSON 时在 `[http]` 里写 `post = true`。确认页对每个域名加一句「向 <主机> 提交数据」。固定请求头写在 `headers` 里，确认页会显示名称和值，不要把密钥写进去：

```toml
[http]
domains = ["api.anthropic.com"]
headers = [
  { name = "anthropic-beta", value = "oauth-2025-04-20" },
  { name = "anthropic-version", value = "2023-06-01" },
]
```

`domains` 只写主机名，不写 `https://`、端口、路径和通配符。密钥的 `label` 是确认页和「填写密钥」里给用户看的名称。每个要发出的密钥都必须写 `header`，`prefix` 可以不写。脚本只按 id 附带，不能另选请求头。多个密钥各用各的头，名字不能重复，也不能和 `headers` 重名。`User-Agent` 写进 `headers` 就替换默认的 `Lanwork`。

清单里写 `usage` 是不认识的字段，界面是「清单无效」，日志写「不认识的字段 usage」。没有 `usage.fetch`。

本机登录写成 `[[login]]`。同时声明 `http`，并把主机名放进 `domains`。ChatGPT、Claude、Grok 的完整示例分别在 `examples/chatgpt-usage`、`examples/claude-usage`、`examples/grok-usage`。Claude 的订阅令牌按 Anthropic 的条款只用于 Claude Code 和 claude.ai，使用该示例由用户自行承担风险，见该目录的 README。最小写法：

```toml
services = ["http", "timer"]

[http]
domains = ["chatgpt.com"]

[[login]]
id = "chatgpt"
label = "ChatGPT 登录"
file = ".codex/auth.json"

[[login.fields]]
name = "token"
path = "tokens.access_token"
header = "Authorization"
prefix = "Bearer "

[login.expires]
field = "token"
format = "jwt_exp"

[login.request]
method = "GET"
url = "https://chatgpt.com/backend-api/wham/usage"

[[login.extract]]
name = "five_percent"
path = "rate_limit.primary_window.used_percent"
type = "number"
```

`file` 相对家目录，只接受 JSON。不能写 `..`、绝对路径、盘符或 `:`，也不能指向 `.ssh`、`.gnupg`、浏览器用户数据目录或 Lanwork 自己的数据目录。插件描述不能用环境变量换目录。宿主只打开一次，从句柄上的最终路径做检查，硬链接数大于 1 的文件不读。`path` 是点号加 `[下标]`。顶层键要用前缀挑选时，每个字段写一个 `key_prefix`：键等于这个前缀，或者后面紧跟 `/`、`:`，才算这一项。写了的前缀都必须命中。过期格式是 `epoch_s`、`epoch_ms`、`rfc3339` 或 `jwt_exp`。地址的主机、路径和查询都写死，脚本改不了。POST 把静态 JSON 写在 `request.body`。确认页会写出文件路径、发往的地址、请求头和交给脚本的字段名。改了描述的任何一处都要再确认。少了一项不算扩大。脚本用 `login.fetch("chatgpt", fn)`，只拿到 `values` 里点名的字段。字符串提取会做尽力回显检查（含 Base64、百分号编码、十六进制，以及按声明顺序拼起来的字符串）；数字和布尔不查。同一份描述 60 秒内不重复请求，描述改了就不再用上次的结果。登录过期是「登录已过期」。

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
| `http` | `http.request({ method = "GET" 或 "POST", url = "...", secret 或 secrets = 可省略, body = POST 时的 JSON 字符串 }, fn)` | 每个域名一句「访问 <主机名>」。`post = true` 时再加「向 <主机> 提交数据」。每个固定请求头一句「请求头 <名称>: <值>」 |
| `secret` | 没有读取函数。只在 `http.request` 里按 id 附带 | 「使用你填写的 <名称>，请求头 <名称>」。有前缀再接「，前缀 <前缀>」 |
| `[[login]]` | `login.fetch("id", fn)`。不是 `services` 里的名字。回调里是 `values` | 「读取本机文件 ~/路径 中的 名称，发往 地址」，加上请求头和「交给脚本：字段名」 |

`http` 只发往已声明的主机。GET 可以发。POST 要清单里 `post = true`，`body` 用 `json.encode` 得到的 JSON 字符串，不超过 64 KiB。宿主写 `Content-Type: application/json`。不跟随重定向，同一台主机上的也不跟随。不附带密钥的明文 `http` 不发送，回调里的 `error` 是「不允许明文 http」。附带密钥时必须是 `https`。没填密钥时回调里的 `error` 是「未填写密钥」，请求不会发出。一次请求可以带多个密钥，最多 8 个，每个进清单里写死的头。脚本不能另选请求头，也不能另加请求头。`login.fetch` 登录过期是「登录已过期」，正文解析不了或提取类型对不上是「数据无效」。

标准库只有 `string`、`table`、`math`、`utf8`，外加基础函数。没有 `io`、`os`、`package`、`debug`、`coroutine`、`require`、`load`、`print`。

回调：`on_load`、`on_unload`、`on_click`、`on_timer`、`on_todos_changed`。一次回调最长约 1 秒。

## 硬性限制

- `id` 是单个路径分量，1 到 200 字节，不能以 `.` 开头，也不能以 `lanwork.` 开头（忽略大小写，`Lanwork.tool` 和 `LANWORK.x` 都不行），不能是 Windows 设备名。
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

同一 id 再导入是更新，确认页有「保留数据」。存储、密钥，以及窗口的位置、大小、贴边、钉住和叠页还在。

声明了密钥时，这一行的「⋯」里有「填写密钥」。掩码输入，不回显。保存之后，已经打开的窗口会重载并再跑 `on_load`。

悬浮窗可以拖动、钉住、贴边，并和便签、收纳、待办叠页。展开时来源行是「应用 · 工具名」。窗口已经开着时再点「悬浮」不会重新加载。要看新脚本，先关掉再打开，或再导入一次。

## 自检

- [ ] 只声明了脚本真正调用的服务
- [ ] 没有 `version` 和其它未列出的字段
- [ ] `id` 不以 `lanwork.` 开头，大小写不论
- [ ] 有 `http` 就有 `domains`，有 `secret` 就至少有一项 `id`、`label` 和 `header`，两边都和 `services` 对齐。本机登录用 `[[login]]`，主机名写进 `domains`，文件是家目录下的 JSON，地址整段写死。不要写 `usage`
- [ ] 域名是纯主机名
- [ ] 固定请求头的名字不与密钥头重名，也不用 `Host`、`Cookie`、`Content-Type` 这些被拒绝的名字；`User-Agent` 只有在要替换 `Lanwork` 时才写
- [ ] 组件只用 `text`、`button`、`timer`、`progress`，id 不重复
- [ ] 计时组件在需要倒数时调用了 `ui.timer_running`
- [ ] `timer.every` 的秒数大于或等于 30
- [ ] 请求的主机名写在 `domains` 里。GET 直接发；POST 先写 `post = true`，正文是 JSON 字符串。密钥只按 id 引用，请求头用清单里的。明文 `http` 不发，窗口是「不允许明文 http」，也不跟随重定向。插件登录只用 `login.fetch`，不自己拼令牌，也不改地址
- [ ] 没有 `print`、`require`、`io`、`os`、协程
- [ ] 文件夹根上有 `manifest.toml` 和入口脚本；zip 根上有清单，或只有一个顶层文件夹
- [ ] 告诉了用户从「应用」页导入，以及密钥要在「填写密钥」里填
