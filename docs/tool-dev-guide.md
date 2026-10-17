# 工具开发说明

这篇按仓库里现在的代码写，给要做悬浮窗工具的人看。用户能看见的规格在 [tools.md](tools.md)。规格里还没落到代码上的交互，这里不写。和规格不一致的地方，以代码为准。

宿主在 `crates/toolhost`，没有界面。窗口用的是应用里的悬浮窗外壳。应用页和导入的句子也按现在的界面代码写。tools.md 记着这些还没有在 Windows 上验收。

下面用 [examples/api-quota](../examples/api-quota) 串起来。用户导入后，窗口里有一行文本和一条进度条。打开时请求一次，之后这扇窗口开着时每 300 秒再请求。主机名和密钥在导入时确认。示例假定服务商返回 `{"used":20,"limit":100}`，`used` 和 `limit` 是数字，`limit` 大于 0。换成真实服务商时，改 URL 和字段，主机名仍写在 `domains` 里。

另一个不联网的例子是 [examples/countdown](../examples/countdown)，全文在末尾。

## 文件夹

一个工具是一个文件夹。额度示例是：

```text
examples/api-quota/
  manifest.toml
  main.lua
  icon.png
```

`manifest.toml` 是清单。`entry` 指向文件夹里的入口脚本，示例写的是 `main.lua`。`icon.png` 可以不放。倒计时示例就没有图标。

导入时只复制这三样：

| 文件 | 条件 |
| --- | --- |
| `manifest.toml` | 必须有 |
| `entry` 指向的那一个 `.lua` | 必须有，而且是 UTF-8 |
| `icon.png` | 可省略。内容以 PNG 文件头 `\x89PNG\r\n\x1a\n` 开头才复制 |

其它文件不复制，也不执行。多出来的 `.lua`、说明、可执行文件都留在用户原来的文件夹里，副本里没有。入口可以写成子目录里的相对路径，例如 `scripts/main.lua`，但不能含 `..`，也不能是绝对路径，并且必须以 `.lua` 结尾。`icon.png` 只认工具根上的这一份，不认子目录。

没有图标、读不出来或内容不是 PNG 时，不拒绝安装。卡片用名称去掉首尾空白后的第一个字。额度示例的名称是「API 额度」，没有可用图标时卡片上是「A」。图标不是清单字段。

文件夹里的其它文件不影响导入。zip 不一样：包里每个文件都先计入个数和体积，超了整包拒绝，通过之后仍然只留下上面那三样。见「打包」。

装上之后，这一版把副本放在 Lanwork 数据目录的 `tools/<工具 id>/`。存储不在这个文件夹里，见「storage」。tools.md 仍把这两条路径标成待定。

## 清单

编码是 UTF-8。TOML。只认下表这些字段。多写一个，包括版本号，安装被拒绝，界面显示「清单无效」。

额度示例的清单：

```toml
id = "example.api-quota"
name = "API 额度"
description = "服务商接口的剩余额度"
entry = "main.lua"
services = ["timer", "http", "secret"]

[http]
domains = ["api.example.com"]

[[secret]]
id = "api_key"
label = "API 密钥"

[[component]]
id = "summary"
component = "text"
text = "没有数据"

[[component]]
id = "used"
component = "progress"
```

| 字段 | 规则 |
| --- | --- |
| `id` | 工具 id。见下面的 id 规则。额度示例是 `example.api-quota` |
| `name` | 卡片和窗口标题。去掉首尾空白后还要有字，不能含换行 |
| `description` | 卡片上的一行短说明。同样不能含换行，去掉首尾空白后还要有字 |
| `entry` | 入口脚本的相对路径 |
| `services` | 字符串数组。可以是空的。名字只能是下表里的服务。同一个名字写两次，按一次算 |
| `http` | 仅当 `services` 含 `http`。表里可以有 `domains`、`post`、`headers` |
| `secret` | 仅当 `services` 含 `secret`。数组，至少一项。每项有 `id` 和 `label`，还可以有 `header` 和 `prefix` |
| `component` | 数组，必须有这个键，可以是空的。内容区从上到下排 |

`services` 里有 `http` 就必须有 `[http]`，有 `[http]` 就必须声明 `http`。`secret` 一样。对不齐就是「清单无效」。

当前接受的服务：

| 服务 | 确认页上的句子 |
| --- | --- |
| `timer` | 定时刷新 |
| `notify` | 发送通知 |
| `storage` | 保存这个工具自己的数据 |
| `todo.read` | 读取你的待办 |
| `todo.complete` | 完成你的待办 |
| `http` | 每个域名一句「访问 <主机名>」，不另写「联网」。`post = true` 时，每个域名紧接着「向 <主机> 提交数据」。每个固定请求头一句「请求头 <名称>: <值>」 |
| `secret` | 每个密钥一句「使用你填写的<名称>」 |

句子顺序是固定的，不跟着 `services` 数组的书写顺序。先按清单顺序，每个域名一句「访问」，允许提交时紧接着「向 <主机> 提交数据」。然后按清单顺序列出每个固定请求头，再按清单顺序列出每个密钥名称，然后是通知、读取待办、完成待办、保存数据、定时刷新。额度示例的三句是：

```text
访问 api.example.com
使用你填写的 API 密钥
定时刷新
```

确认页上没有密钥的值，也没有密钥 id。固定请求头的值会写在确认页上，不要把密钥填进 `headers`。

更新时确认页仍整份列出这些句子。新版本比上次确认多了密钥、固定请求头、主机名或 `post`，必须再确认一次，新的那几句就在这份列表里。导出包和备份的那一次确认用同一套句子。

### id

工具 id 是单个路径分量，规则与数据文件的 id 相同，另外拒绝以 `.` 开头。

- 长度 1 到 200 字节
- 不能是 `.` 或 `..`，不能以 `.` 开头，不能以 `.` 结尾
- 不能以空格开头或结尾
- 不能含 `/ \ < > : " | ? *`，也不能含控制字符
- 第一个 `.` 之前的那一段不能是 Windows 保留设备名：`CON`、`PRN`、`AUX`、`NUL`、`COM1` 到 `COM9`、`LPT1` 到 `LPT9`。这一段忽略大小写，所以 `con.tool` 不行，`example.con` 可以
- 不能以 `lanwork.` 开头。比较忽略大小写，`Lanwork.tool` 和 `LANWORK.x` 也不行。界面仍是「清单无效」，日志里的说明仍是「id 不能以 lanwork. 开头」

密钥的 `id` 用同一套路径分量规则，不检查 `lanwork.` 前缀。同一个工具里密钥 id 不能重复。

组件 `id` 在这个工具里不能重复。去掉首尾空白后还要有字，不能含换行或控制字符。没有 200 字节这一条。

### 体积

界面上仍然只显示「清单无效」。

| 限制 | 数量 |
| --- | --- |
| 清单、入口脚本、`icon.png` 各自 | 最大 2 MiB |
| zip 文件本身 | 最大 8 MiB |
| zip 解压后的合计 | 最大 8 MiB |
| zip 里的条目 | 最多 64 个，文件夹条目也算 |

`icon.png` 超过 2 MiB 时，即使内容不是 PNG，也是「清单无效」。不到 2 MiB 但不是 PNG，则跳过这个文件，改用名称首字。

### 域名

`domains` 至少一条。每一条是主机名，不是 URL。

- 整段最长 253，用 `.` 分开的每一段 1 到 63
- 只含 ASCII 字母、数字和 `-`。一段不能以 `-` 开头或结尾。整段不能以 `.` 结尾
- 不能含协议、端口、路径、问号、`#`、用户名、空格、通配符
- 不能是 IPv4 或 IPv6。`127.0.0.1` 和 `::1` 都不行
- `localhost` 可以

`api.example.com` 只等于它自己。它不等于 `example.com`，也不等于 `a.api.example.com`。请求时忽略大小写：清单里写 `api.example.com`，URL 用 `API.Example.COM` 仍然算同一台。确认页上的句子保留清单里的写法。

URL 里可以有路径、查询和端口。端口不写进 `domains`。

### 提交和固定请求头

`post` 可以不写，不写等于假。写成真时，脚本可以向 `domains` 里的每一台 POST JSON。不能只给其中一台打开。确认页对每一台都加「向 <主机> 提交数据」。

`headers` 可以不写。每一项只有 `name` 和 `value`。

```toml
[http]
domains = ["api.anthropic.com", "chatgpt.com"]
post = true
headers = [
  { name = "anthropic-beta", value = "oauth-2025-04-20" },
  { name = "anthropic-version", value = "2023-06-01" },
  { name = "X-XAI-Token-Auth", value = "xai-grok-cli" },
  { name = "User-Agent", value = "my-tool" },
]

[[secret]]
id = "token"
label = "访问令牌"
header = "Authorization"
prefix = "Bearer "

[[secret]]
id = "account"
label = "账号"
header = "ChatGPT-Account-Id"
```

名字忽略大小写比较。`Host`、`Cookie`、`Content-Length`、`Content-Type`、`Transfer-Encoding`、`Connection`，以及 `Keep-Alive`、`Proxy-Authenticate`、`Proxy-Authorization`、`Proxy-Connection`、`TE`、`Trailer`、`Upgrade` 都不能写。固定请求头之间不能重名，也不能和某个密钥的 `header` 重名。名字只含字母、数字和 `-`，不以 `-` 开头或结尾，最长 64。值是可打印 ASCII，不含回车换行，去掉首尾空白后还要有字，最长 512。最多 16 条。不合规则时不安装，界面是「清单无效」，日志里有具体原因。

声明了 `User-Agent` 就用这条替换默认的 `Lanwork`，只发出一条。没声明时仍是 `User-Agent: Lanwork`。`Accept` 没声明时仍是 `*/*`。

密钥的 `header` 和 `prefix` 都可以不写。写了 `prefix` 就必须写 `header`。`prefix` 是值前面的字，例如 `Bearer `，可以是空字符串，只含可打印 ASCII，最长 128，不能含回车换行。同一个工具里，密钥声明的请求头也不能彼此重名。

## 组件

`component` 的每一项有 `component` 和 `id`。不认识的种类，或多出来的字段，清单无效。组件排成一列。没有嵌套、分栏和网格。

窗口现在真正画出来的是文本、按钮、计时和进度条。`list`、`input`、`toggle` 可以写进清单，但悬浮窗把它们画成一行文本，脚本也没有对应的函数，点不了，也输入不了。做工具时先别用这三种。

| `component` | 清单里还能写 | 窗口 | 脚本 |
| --- | --- | --- | --- |
| `text` | `text`，可省略，省略是空字符串 | 一行文本，超出省略 | `ui.text(id, s)` |
| `button` | `label`，去掉首尾空白后还要有字 | 按钮，文字用 `label` | `ui.button(id, label)` 改文字。点击调用 `on_click(id)` |
| `timer` | 无 | 剩余秒数，向下取整 | 见下文 |
| `progress` | 无 | 一条 0 到 1 的进度条 | `ui.progress(id, n)` |
| `list` | 无 | 一行文本，内容是空的 | 没有 |
| `input` | `text`，可省略 | 一行文本，内容是清单里的 `text` | 没有 |
| `toggle` | `label`；`on`，可省略，省略是关。`on` 必须是布尔 | 一行文本，形如「名称 开」或「名称 关」 | 没有 |

`ui` 只接受本工具清单里的 id。没有这个 id，这次调用报错，窗口显示「脚本错误（第 N 行）」。往组件上写的时候，种类不对也是这句。`ui.timer(id)` 这种只读取的调用不检查种类：清单里有这个 id，就返回它当前记下的剩余秒数；不是计时组件时，这个数一直是 0。

`ui.text` 只改 `text`。`ui.button` 只改 `button`。`ui.progress` 只改进度条。`n` 小于 0 按 0，大于 1 按 1，不是有限数按 0。

计时组件和 `timer` 服务不是一回事。组件可以短于 30 秒，也不用声明 `timer`。倒计时示例没有声明任何服务。

- `ui.timer(id, seconds)` 把剩余秒数设成这个数。小于 0 或不是有限数按 0。它不开始倒数，也不改「在不在走」
- `ui.timer(id)` 读取剩余秒数，可以带小数。不检查这个 id 是不是计时组件
- `ui.timer_running(id, true)` 从当前剩余秒数开始倒数。`false` 冻在当前秒数
- 窗口开着时大约每秒减一次，一次最多按 2 秒扣。脚本不用自己每秒改数字
- 正在倒数并且减到 0 时调用 `on_timer(id)`。把秒数设成 0，或停掉，都不调用
- 窗口上的数字是剩余秒数向下取整。清单不能给计时组件写初始秒数，打开时是 0

同一次调用里先 `ui.timer` 再 `ui.timer_running(id, false)`，返回前不会走到 0。倒数从这次调用返回之后的下一次窗口刷新才继续。

## 沙箱

每个工具一块 Lua 5.4 状态，库是 `mlua`。入口脚本加载时执行。未授予的服务不放进这块状态。

始终有的基础函数：`assert`、`error`、`ipairs`、`next`、`pairs`、`pcall`、`select`、`tonumber`、`tostring`、`type`、`rawget`、`rawset`、`rawequal`、`setmetatable`、`getmetatable`，以及 `_G`、`_VERSION`。`xpcall` 换成宿主的函数：被保护的函数出错并展开之后才调用处理函数，超时钩子在这两段里都开着。

标准库有 `string`、`table`、`math`、`utf8`。`table.unpack` 在 `table` 里。

另外有 `json` 和 `ui`。`json` 不是服务，不用写进 `services`。`json.decode` 成功返回值，失败返回 `nil` 和错误字符串。`json.encode` 只接受文本能表达的值，空表编成 `[]`。函数、线程编不了。

不放入，读出来是 `nil`：`io`、`os`、`package`、`debug`、`coroutine`，以及 `load`、`loadfile`、`dofile`、`require`、`collectgarbage`、`print`、`warn`、`rawlen`。没有协程。超时钩子按 Lua 线程安装，协程是另一条线程，钩子管不到。入口以外的 `.lua` 不执行。不能加载动态库，也不能启动进程。

一次回调从进入到返回最长 1 秒。超过则中止，窗口显示「脚本超时（第 N 行）」。`pcall` 接住这次中止后，钩子仍会在后面报错，这次回调不能接着跑完。`xpcall` 的处理函数也算在这 1 秒里。

当前宿主还会拦住这些情况。tools.md 把其中一部分字节数标成不是产品规则，脚本这边仍然会碰到：

- `string.find`、`string.match`、`string.gmatch`、`string.gsub` 在宿主的匹配器里按 Lua 5.4 的规则执行。文本最长 1 MiB，模式最长 8 KiB。匹配每 4096 步看一次这 1 秒，到点与钩子一样中止，`pcall` 不能把回调留住
- 这四个函数同时嵌套最多 8 层。`string.rep` 的结果最长 1 MiB。`gsub` 各层还没交还的结果加在一起，跟 Lua 已用内存计入同一块 16 MiB。超过则这次调用报错，`pcall` 可以接住，回调继续
- 不带比较函数的 `table.sort`，表长超过 10 万就拒绝这次调用。带比较函数时，每次比较都回到 Lua，超时钩子仍然有效
- 整块状态的内存大约 16 MiB。到顶后与超时一样中止这次回调

等网络不占这 1 秒。套接字在另一条线程上。

## 生命周期

入口脚本顶层在加载时执行，这时组件已经按清单的初值放好。没有 `on_load` 也可以加载。顶层死循环时，窗口仍然打开，然后底部画出「脚本超时（第 N 行）」。这次不会调用 `on_load`。

| 函数 | 何时 |
| --- | --- |
| `on_load()` | 悬浮窗打开之后。更新导致的重载、保存或清除密钥之后的重载，也会再调用 |
| `on_unload()` | 窗口关闭时，以及重载之前。可以不定义。随后这块状态里的定时器全部取消，还没返回的 HTTP 回调不再调用 |
| `on_click(id)` | 按钮被点击。只有按钮会进来 |
| `on_timer(id)` | 计时组件正在倒数并走到 0 |
| `on_todos_changed()` | 仅当授予了 `todo.read`。待办变更之后。脚本再自己调用 `todo.list` |

窗口已经开着时再点「悬浮窗」，只把焦点给这一扇，不重新加载，也不再跑 `on_load`。关掉再打开才会再跑。

刷新和计时都跟着这扇窗口。大约每秒走一次：先扣计时组件，再看 `timer.every` 和 `timer.after`。窗口关掉，这些就停了，不会在后台继续请求。再打开时由 `on_load` 重新登记。

重载的时机：

- 同一 id 的更新确认之后，窗口还开着：先 `on_unload`，再用新副本和新的授权加载，然后 `on_load`。位置、大小、贴边和钉住不变
- 在「填写密钥」里保存或清除之后，窗口还开着：同样重载，所以额度示例会再请求一次。保存写到一半失败时，已经写上的行仍然生效，开着的窗口也会重载
- 启用开关关掉时，窗口马上关闭，走 `on_unload`

面板收起不关闭工具窗口。关闭窗口只关窗口，不卸载工具。

## 服务

下面的表只在 `services` 声明了对应项时才存在。脚本碰到没声明的服务时，这次调用不执行。这不是 Lua 错误，`pcall` 接不住，回调结束时窗口显示「未授权」。

`secret` 声明了之后会有一个空表 `secret`，上面没有读取函数。没声明时，读 `secret` 也是「未授权」。

### `timer`

要写进 `services`。额度示例用它每 300 秒再请求一次。

| 函数 | 作用 |
| --- | --- |
| `timer.every(seconds, fn)` | 每隔 `seconds` 秒调用 `fn`。第一次在间隔结束时，登记时不立刻调用。返回 id |
| `timer.after(seconds, fn)` | `seconds` 秒后调用 `fn` 一次。返回 id |
| `timer.cancel(id)` | 取消这个 id。不认识的 id 没有动作 |

`timer.every` 的 `seconds` 必须大于或等于 30。短于 30 不登记，窗口显示「间隔过短」。`timer.after` 目前任意正数都可以。两者不是有限的正数时不登记，显示「参数无效」。一次刷新跨过多个间隔，只调用一次。窗口关闭后这些 id 都没了。

计时组件的倒数不走这两个函数。

### `notify`

| 函数 | 作用 |
| --- | --- |
| `notify.send(title, body)` | 发一条系统通知。两个参数都是字符串。没有返回值说明它有没有弹出 |

安静时段内不弹出。时段结束后，错过的几条合成一条。时段是否成立由 Lanwork 的设置决定，脚本不用自己判断。

### `storage`

按键保存这个工具自己的文本。不放密钥。文本不写进工具文件夹。这一版写在数据目录的 `tool-storage/<工具 id>.json`。更新时这份文件还在，卸载时删掉。导出和备份会带走它。

| 函数 | 作用 |
| --- | --- |
| `storage.get(key)` | 返回此前写下的字符串。没有则返回 `nil` |
| `storage.set(key, value)` | 写下字符串。成功返回 `true`。失败返回 `nil` 和「写盘失败」 |

当前宿主在能写盘时限制：键最长 256 个 Unicode 字符，值最长 256 KiB（按字节），最多 256 条。超过，或写盘失败，窗口显示「写盘失败」，原文件保留，内存里的旧值也不改。tools.md 仍把条数和单条大小标成待定，脚本这边现在就会被这三档拦住。

### `todo`

`todo.list` 需要 `todo.read`。`todo.complete` 需要 `todo.complete`。只声明其中一个时，另一个函数不执行，窗口显示「未授权」。

| 函数 | 作用 |
| --- | --- |
| `todo.list()` | 返回数组，从 1 起 |
| `todo.complete(id)` | 完成这一条。成功返回 `true`。失败返回 `nil` 和错误字符串 |

`todo.list` 的每一项：

| 字段 | 含义 |
| --- | --- |
| `id` | 待办 id |
| `listId` | 清单 id |
| `title` | 标题 |
| `completed` | 是否已完成 |
| `due` | 到期日 `YYYY-MM-DD`。没有则是 `nil` |
| `current` | 是否为当前待办 |

不含回收站。读不到便签，也读不到待办正文以外的字段。

`todo.complete` 走待办页上的完成。带周期的按待办自己的规则生成下一次。不能取消完成、新建、改标题或删除。已经完成时返回 `true`，不再写一次。找不到时返回「找不到待办」。这条在回收站里时返回「待办已经在回收站」。写盘失败时返回「写盘失败」。带周期、而这一天在目标月不存在时，返回「每月这一天在目标月不存在，尚未规定落到哪一天」。日期算出去超出范围时，返回「日期超出范围」。这两种都还没写盘。

授予了 `todo.read` 时，待办变更之后宿主调用 `on_todos_changed`。这个回调不写入。脚本要最新列表，就再调用 `todo.list`。

### `http`

只向 `domains` 里声明、并且用户确认过的主机名发请求。额度示例的请求：

```lua
http.request({
  method = "GET",
  url = "https://api.example.com/v1/quota",
  secret = { id = "api_key", header = "Authorization", prefix = "Bearer " },
}, function(result)
end)
```

`http.request` 不等待网络，返回之后宿主再调用那个函数。`secret` 可以省略。写了的话，`id` 必须是清单里的一项，`header` 是头的名字，只含字母、数字和 `-`，`prefix` 是值的前缀，可以是空字符串，但不能含换行。宿主把「前缀 + 密钥原文」放进这个头。脚本读不到原文，也读不到拼好的头。

一次请求要附上多个密钥时，用 `secrets`，不要和 `secret` 同时写。最多 8 个：

```lua
http.request({
  method = "GET",
  url = "https://chatgpt.com/backend-api/wham/usage", -- 路径按服务商文档写
  secrets = {
    { id = "token", header = "Authorization", prefix = "Bearer " },
    { id = "account", header = "ChatGPT-Account-Id", prefix = "" },
  },
}, function(result)
end)
```

清单里该密钥已经写了 `header` 时，脚本可以只写 `id`。两边都写时，名字忽略大小写必须相同，前缀必须逐字相同，发出去的名字用清单里的写法。没在清单里写 `header` 时，脚本必须自己写。同一请求里请求头不能重名，也不能和固定请求头重名，也不能用上面拒绝的那些名字。其中一个密钥没填，这一次整份都不发。

脚本不能另加请求头。表里多出来的字段这次忽略。发出去的请求带 `Host`（主机名，不带端口）、`User-Agent`（清单里的值，否则 `Lanwork`）、`Accept`（清单里的值，否则 `*/*`）和 `Connection: close`。其余固定请求头按清单顺序接在后面，密钥头再接在后面。

`post = true` 时可以 POST。`body` 是 JSON 字符串，可以用 `json.encode` 得到。宿主检查它是合法 JSON，不超过 64 KiB，并自己写上 `Content-Type: application/json` 和 `Content-Length`。正文按脚本给出的字节发出，不重新排版。GET 不能带 `body`。

```lua
http.request({
  method = "POST",
  url = "https://api.example.com/v1/usage",
  body = json.encode({ limit = 1 }),
  secret = { id = "api_key", header = "Authorization", prefix = "Bearer " },
}, function(result)
end)
```

不附带密钥的明文 `http` 不发送，已经定了。重定向一律不跟随，同一台主机上的也不跟随，已经定了。

| 情况 | 结果 |
| --- | --- |
| 方法不是 GET 或 POST，POST 未允许，或 GET 带了 `body` | 不发送。`error` 是「不在范围内」 |
| `body` 不是 JSON 字符串，或 POST 没有 `body` | 不发送。「数据无效」 |
| `body` 超过 64 KiB | 不发送。「数据过大」 |
| URL 解析不了、带了用户名或密码、协议不是 `http` / `https` | 不发送。「地址无效」 |
| 附带密钥但不是 `https` | 不发送。「地址无效」 |
| 这次请求的密钥头不合法、重名，或和固定请求头重名 | 不发送。「地址无效」 |
| 不附带密钥的明文 `http` | 不发送。「不允许明文 http」 |
| 主机名不在已确认的清单里 | 不发送。「域名未允许」 |
| 密钥 id 不在清单里、还没填写，或值是空的 | 不发送。「未填写密钥」 |
| 连接、读取或等待失败 | 「请求失败」。说明里没有密钥，也没有请求头 |

不跟随重定向。同一台主机上的重定向也不跟随。状态码是 3xx 时，`ok` 为假，`error` 是「请求失败」，`status` 仍是那个状态码。

主机名与清单里某一条忽略大小写相等才发送。路径和端口不另做限制。

连接时，域名解析出来的每个地址各自最多等 5 秒，连上一个就用它。读取如果连续 5 秒没有新数据就停，用已经收到的内容解析。正文超过 1 MiB 算失败。`on_load` 返回之后，宿主按发出的顺序等还在飞的请求。每一条单独最多等 5 秒，不是几条合用这 5 秒。某一条没回来，这一条是「请求失败」，然后继续等下一条。这些秒数在代码里是固定的，tools.md 仍标成待定。

`result` 的字段：

| 字段 | 含义 |
| --- | --- |
| `ok` | 状态码是 2xx 时为 `true` |
| `status` | HTTP 状态码。没有发出去时没有这个字段 |
| `body` | 响应正文。没有正文时是空字符串 |
| `error` | `ok` 为假时的说明。上面那些句子 |

响应头不在这个表里。

窗口已经关闭，或正在 `on_unload`，不再调用这个函数。

### `secret`

没有读取函数。密钥只通过 `http.request` 的 `secret` 或 `secrets` 附带。原文不进脚本、存储、日志和确认页。

用户在该工具卡片的「⋯」里点「填写密钥」。每一行左边是 `label`，右边是掩码输入，不回显已保存的值，也不显示密钥 id。关掉再打开，输入是空的。保存只写入填写了的行，空着的行保持原样。某一行没有写上时，界面是「「名称」没有写下」，不含密钥的值；已经写上的行仍然生效。清除删掉这个工具已声明的密钥，不再询问一次。

值在 Windows 凭据管理器里，按工具 id 和密钥 id 分开。不出现在工具文件、存储、导出、备份和日志里。导入或恢复一个包时，如果清单或入口脚本和本机同 id 副本的内容不同，或者本机没有这份副本，会清掉这个工具已经保存的密钥。两边内容相同则保留。

已确认主机的响应里，正文或响应头如果对上密钥的列出形式，正文改成空字符串，`ok` 为假，`error` 为「请求失败」，状态码仍保留。这是尽力检查：密钥被拆开、加密，或用没有列出的写法写出来，仍然可能漏过去。短密钥的编码可能只有几个字符，正文里碰巧出现同样的几个字符时也会整段清空。

检查的形式：密钥原文；`Bearer ` 加原文和 `bearer ` 加原文；这次实际发出的请求头的值；上面每一项的标准 Base64 和 URL 安全 Base64，带填充和不带填充；把 `%XX` 按十六进制解码后再找，十六进制忽略大小写，连续解码最多四次。原文或解码结果里的 `+` 再按空格看一次。

## 导入和更新

入口是应用页的「导入+」。可以选择一个文件夹，或一个 zip。把文件夹或 zip 拖到应用页上，同样开始导入。

先读清单。读到之后，确认页列出名称、一行短说明，以及上面的那些句子。不能改清单，也不能只去掉其中几句。用户整份确认才安装，取消则不安装，数据目录里不留下这份新工具。正在等待确认时再拖入或再点「导入+」，不开始第二次。

新导入的工具启用开关是打开的。程序使用数据目录里的副本，不移动、不删除用户原来的文件夹。

同一 id 已经安装时，再导入视为更新。确认页标明是更新，并有一句「保留数据」。新版本多了密钥、固定请求头、主机名或提交权限时，这些句子出现在同一张确认页上，必须再确认一次。确认后用新副本替换旧副本。同一 id 已有的存储、密钥、窗口位置、大小、贴边和钉住保留。这扇窗口如果开着，按「生命周期」重载。未确认则不替换。

从某一张卡片的「更新」选到的包，id 必须是这张卡片的。不是时显示「不是这个工具的更新包」，不打开确认，也不导入。「导入+」和拖入不按某一张卡片核对 id。

「查看权限」打开同一张卡片，句子与导入确认相同，没有「确认」。关掉之后已经授予的服务不变。

「卸载」先确认，确认框写「会删除这个工具的存储和密钥。」确认后关掉悬浮窗，删除副本、存储、`tool-windows/<工具 id>.json` 和凭据管理器里的密钥。

## 错误

界面上的字只有名称、数量、时间、状态和错误。短说明是卡片上的那一处例外。导入失败时，界面只写「清单无效」，具体原因写在数据目录的 `logs/app.log`，前缀是「工具包没有通过：」。

| 情况 | 界面 |
| --- | --- |
| 清单解析不了，字段、组件或服务不认识，id 不合法，域名不合法，请求头不合法，`http` / `secret` 对不齐，文件过大，不是文件夹或 zip | 「清单无效」。不安装 |
| 副本上的清单超出已经确认的范围，或权限记录读不了 | 窗口显示「未授权」，脚本不跑 |
| 脚本访问未授予的服务 | 「未授权」 |
| 未允许的 POST，或 GET、POST 以外的方法，或 GET 带了正文 | 「不在范围内」 |
| POST 正文不是 JSON | 「数据无效」 |
| POST 正文超过 64 KiB | 「数据过大」 |
| 地址无效，附带密钥但不是 `https`，或这次请求的密钥头不合法 | 「地址无效」 |
| 不附带密钥的明文 `http` | 「不允许明文 http」 |
| 主机名不在已确认的清单里 | 「域名未允许」 |
| 密钥未填写 | 「未填写密钥」 |
| 请求没有完成，或响应里对上密钥 | 「请求失败」 |
| 找不到要完成的待办 | 「找不到待办」 |
| 要完成的待办在回收站 | 「待办已经在回收站」 |
| 定时秒数不是正数 | 「参数无效」 |
| `timer.every` 短于 30 秒 | 「间隔过短」 |
| 一次回调超过 1 秒 | 「脚本超时（第 N 行）」 |
| 回调里运行失败 | 「脚本错误（第 N 行）」 |
| 入口脚本加载失败 | 「脚本错误：…」，后面是 Lua 的说明 |
| `storage.set` 超限或写盘失败 | 「写盘失败」。原文件保留 |
| 打开窗口时存储文件读不了 | 「写盘失败」 |
| 卡片「更新」选到另一个 id | 「不是这个工具的更新包」 |
| 某一行密钥没有写上 | 「「名称」没有写下」 |

窗口底部的红字是最近一条宿主提示，不会自己消失。脚本可以用 `ui.text` 另写一行说明。额度示例在请求失败时两边都会写：红字是上表里的句子，文本组件是脚本自己的字。

## 调试

沙箱里没有 `print`，也没有调试器。把中间结果写到 `text` 组件上。

红字出现之后，对着 `logs/app.log` 看原因。装上之后可以打开数据目录的 `tools/<工具 id>/`，核对里面是不是只有清单、入口脚本和 PNG 图标。

窗口已经开着时，改完脚本再点「悬浮窗」不会重新加载。关掉再打开，或者走一次更新确认，才会跑新的 `on_load`。

`lanwork --float-shell-demo` 只打开一个内容是「示例」的外壳，不是已安装的工具。已经有 Lanwork 在运行时，这条命令不会开窗口。

仓库里可以跑：

```text
cargo test -p lanwork-toolhost
```

额度示例和倒计时示例都在这组测试里。测试会核对开发说明里的代码块和 `examples/` 下的文件是同一份，并让倒计时走完 60 秒。

## 打包

文件夹和 zip 都可以导入。

文件夹：清单放在被选中的那一层。不要选到它的上一级。

zip 有两种摆法：

- 根上就是 `manifest.toml`、入口脚本，以及可选的 `icon.png`
- 根上只有一个文件夹，清单在那个文件夹里

两种都先读清单，再进入确认。根上有两个文件夹，或者找不到清单，是「清单无效」。不要把密钥、说明以外的大文件打进去。那些文件不会被复制，但会计入 64 个文件和 8 MiB 的上限。

加密的 zip、zip64、除「存储」和 deflate 以外的压缩方法，这一版也不接受。

用户原来的文件夹留在原地。确认之后，Lanwork 用的是数据目录里的副本。

## 示例：额度

`manifest.toml`：

```toml
id = "example.api-quota"
name = "API 额度"
description = "服务商接口的剩余额度"
entry = "main.lua"
services = ["timer", "http", "secret"]

[http]
domains = ["api.example.com"]

[[secret]]
id = "api_key"
label = "API 密钥"

[[component]]
id = "summary"
component = "text"
text = "没有数据"

[[component]]
id = "used"
component = "progress"
```

`main.lua`：

```lua
local function show_error(message)
  ui.text("summary", message)
  ui.progress("used", 0)
end

local function apply(body)
  local data = json.decode(body)
  if type(data) ~= "table" or type(data.used) ~= "number" or type(data.limit) ~= "number" or data.limit <= 0 then
    show_error("额度读取失败")
    return
  end
  local used = data.used
  if used < 0 then
    used = 0
  end
  if used > data.limit then
    used = data.limit
  end
  ui.text("summary", string.format("已用 %g / %g", used, data.limit))
  ui.progress("used", used / data.limit)
end

local function refresh()
  http.request({
    method = "GET",
    url = "https://api.example.com/v1/quota",
    secret = { id = "api_key", header = "Authorization", prefix = "Bearer " },
  }, function(result)
    if not result.ok then
      show_error(result.error or "额度读取失败")
      return
    end
    apply(result.body)
  end)
end

function on_load()
  refresh()
  timer.every(300, refresh)
end
```

没填密钥时，`result.error` 是「未填写密钥」，请求不会发出，文本组件上也是这句。脚本里没有能把密钥打出来的调用。在「填写密钥」里保存之后，开着的窗口会重载，`on_load` 再请求一次。

窗口关掉之后，`timer.every` 不再触发。再打开时 `on_load` 重新请求，并再登记 300 秒。

## 示例：倒计时

[examples/countdown](../examples/countdown) 不声明服务，确认页上没有权限句子，但仍然要整份确认。打开时计时显示 0，状态是「准备」。点「开始」后从 60 倒数，走到 0 时状态改成「到了」。

`ui.timer` 只写入秒数。不调用 `ui.timer_running(id, true)` 的话，数字停在 60，不会走，也不会进 `on_timer`。

`manifest.toml`：

```toml
id = "example.countdown"
name = "倒计时"
description = "点开始后倒数 60 秒"
entry = "main.lua"
services = []

[[component]]
id = "status"
component = "text"
text = "准备"

[[component]]
id = "clock"
component = "timer"

[[component]]
id = "start"
component = "button"
label = "开始"
```

`main.lua`：

```lua
function on_load()
  ui.text("status", "准备")
end

function on_click(id)
  if id ~= "start" then
    return
  end
  ui.timer("clock", 60)
  ui.timer_running("clock", true)
  ui.text("status", "倒数")
end

function on_timer(id)
  if id ~= "clock" then
    return
  end
  ui.timer_running("clock", false)
  ui.text("status", "到了")
end
```
