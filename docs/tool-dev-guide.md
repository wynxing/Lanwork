# 工具开发说明

本文说明悬浮窗工具的清单、组件、Lua 调用和两份完整示例。可见行为以 [tools.md](tools.md) 为准。待办和番茄钟的规则以 [product.md](product.md) 为准。与 tools.md 冲突时，以 tools.md 为准。现在没有这部分代码。没有通过记录之前，不写依赖该规格的界面。

示例写成一个文件夹。导入时文件夹和 zip 都接受，也可以拖入「自定义」页，见 tools.md「导入」。

## 文件夹

```text
my-tool/
  manifest.toml
  main.lua
```

`manifest.toml` 是清单。`entry` 指向文件夹内的入口脚本，示例用 `main.lua`。路径不含 `..`，不是绝对路径，以 `.lua` 结尾，文件必须在。入口以外的 `.lua` 不执行。

## 清单

编码是 UTF-8。TOML。当前列出的字段如下。版本字段还没定，不要写。写了未列出的字段，安装被拒绝，界面显示「清单无效」。

| 字段 | 含义 |
| --- | --- |
| `id` | 单个路径分量，规则与数据文件的 id 相同。导入的 id 不能以 `lanwork.` 开头。内置番茄钟是 `lanwork.pomodoro` |
| `name` | 卡片上的名称。去掉首尾空白后还要有字 |
| `description` | 卡片上的一行短说明。不能含换行。去掉首尾空白后还要有字 |
| `entry` | 入口脚本的相对路径 |
| `services` | 字符串数组。第一批只接受 tools.md「服务」表里的名字 |
| `http` | 仅当 `services` 含 `http`。表内字段 `domains` 是主机名数组，至少一条 |
| `secret` | 数组。仅当 `services` 含 `secret`，且至少一项。每项有 `id` 和 `label` |
| `component` | 数组。内容区从上到下的组件 |

`domains` 的每一条是主机名。不含协议、端口、路径、问号和通配符。`label` 是设置里给用户看的名称。密钥的 `id` 只在脚本和清单里使用，确认页不显示这个 id，显示 `label`。

`http` 与 `secret` 和 `services` 对不齐时，清单无效，见 tools.md「错误」。

## 组件

`component` 的每一项有 `component` 和 `id`。`id` 在这个工具里不重复。不认识的 `component`，或没列出的字段，清单无效。

| `component` | 清单字段 | 脚本 | 回调 |
| --- | --- | --- | --- |
| `text` | `text`，可省略，省略是空字符串 | `ui.text(id, s)` | 无 |
| `button` | `label`，去掉首尾空白后还要有字 | `ui.button(id, label)` | `on_click(id)` |
| `timer` | 无 | `ui.timer(id, seconds)` 设定剩余秒数，宿主开始倒数；`ui.timer(id)` 读取当前剩余秒数；`ui.timer_running(id, on)` 继续或冻结 | `on_timer(id)`，正在倒数并走到 0 时 |
| `progress` | 无 | `ui.progress(id, n)` | 无 |
| `list` | 无 | `ui.list(id, items)` | `on_list_click(id, index)` |
| `input` | `text`，可省略 | `ui.input(id)` 读取，`ui.input_set(id, s)` 写入 | `on_input(id, s)` |
| `toggle` | `label`；`on`，可省略，省略是关 | `ui.toggle(id)` 读取，`ui.toggle_set(id, on)` 写入 | `on_toggle(id, on)` |

`ui.timer(id, seconds)` 把剩余秒数交给宿主。宿主逐秒重画，脚本不用自己每秒改。小于 0 按 0。这次设定会换掉上一次倒数。正在倒数并且走到 0 时调用 `on_timer(id)`。把秒数设成 0，或 `ui.timer_running(id, false)`，都不调用 `on_timer`。`ui.timer_running(id, true)` 从当前剩余秒数继续。显示格式待定。这条回调不受 30 秒刷新下限。

`ui.progress` 的 `n` 是 0 到 1。小于 0 按 0，大于 1 按 1。

`ui.list` 的 `items` 是字符串数组，从 1 起。`on_list_click` 的 `index` 也从 1 起。

`on_input` 在组合文字上屏之后调用。组合未上屏时不调用。每个键都调用，还是停一段时间再调用，待定。Enter 是否另算提交，待定。

组件排成一列。嵌套、分栏和网格待定。

## 沙箱

宿主为每个工具建一块 Lua 5.4 状态，用 `mlua` 执行入口脚本。未授予的服务不放进这块状态。

放入的基础函数：`assert`、`error`、`ipairs`、`next`、`pairs`、`pcall`、`xpcall`、`select`、`tonumber`、`tostring`、`type`、`rawget`、`rawset`、`rawequal`、`setmetatable`、`getmetatable`，以及 `table.unpack`。

`xpcall` 由宿主换成自己的函数。被保护的函数出错并展开之后，才调用处理函数，超时钩子在这两段里都开着。处理函数里的死循环会在 1 秒后中止，窗口显示「脚本超时（第 N 行）」。Lua 自带的 `xpcall` 会在钩子关掉时调用处理函数，所以没有放进沙箱。

放入的标准库：`string`、`table`、`math`、`utf8`。

`string.find`、`string.match`、`string.gmatch`、`string.gsub` 在宿主的匹配器里按 Lua 5.4 的规则执行，并按步对照这 1 秒。到点与钩子一样中止，窗口显示「脚本超时（第 N 行）」，`pcall` 不能把这次回调留住。文本、模式和 `string.rep` 的结果仍有临时长度上限；超过则这次调用报错，可以用 `pcall` 接住，这次回调继续。`gsub` 的结果在每次扩容前计入同一块临时内存预算，各层还没交还的缓冲区加在一起算，超过同样可以用 `pcall` 接住；复制大约每 64 KiB 再对照这 1 秒，到点与钩子一样中止。这四个模式函数同时嵌套最多 8 层，超过可以用 `pcall` 接住；到顶之后，这次调用展开完之前不能再进。`xpcall` 和 `table.sort` 从宿主再进 Lua 时还有重入上限，到顶之后同样要等这次调用展开完。字节数和层数仍待定，见 tools.md「待定」第 2 条。

另放入 `json.decode` 和 `json.encode`。两者只处理文本。`json.decode` 成功时返回值，失败时返回 `nil` 和错误字符串。这不是服务，不必写进 `services`。

`ui` 始终放入。它只碰到本工具清单里的组件 id。

不放入 `io`、`os`、`package`、`debug`、`load`、`loadfile`、`dofile`、`require`、`collectgarbage`，也不放入协程。超时钩子按 Lua 线程安装，协程是另一条线程，钩子管不到。`require` 和多个 Lua 文件是否允许，见 tools.md「待定」。示例不使用协程。

## 生命周期

宿主在悬浮窗打开后调用 `on_load`。窗口关闭时调用 `on_unload`。没有定义这两个函数也可以加载。

回调：

| 函数 | 何时 |
| --- | --- |
| `on_load()` | 窗口已打开，组件已按清单的初值摆好 |
| `on_unload()` | 窗口关闭。随后这块状态里的定时器全部取消，尚未返回的 HTTP 回调不再调用 |
| `on_click(id)` | 按钮被点击 |
| `on_input(id, s)` | 输入框上屏后的文本 |
| `on_toggle(id, on)` | 开关变化。`on` 是布尔 |
| `on_list_click(id, index)` | 列表的一项被点击 |
| `on_todos_changed()` | 仅当授予了 `todo.read`。待办变更之后。脚本再调用 `todo.list` |
| `on_timer(id)` | 计时组件正在倒数并走到 0 |

函数抛错时，悬浮窗显示「脚本错误（第 N 行）」，N 是行号，不含密钥。一次回调从进入到返回超过 1 秒，宿主中止这次调用，显示「脚本超时（第 N 行）」，不含密钥。`xpcall` 的处理函数也算在这 1 秒里。

## 服务函数

下面的表只在 `services` 声明了对应项时才存在。

### `timer`

| 函数 | 作用 |
| --- | --- |
| `timer.every(seconds, fn)` | 每隔 `seconds` 秒调用 `fn`。第一次在间隔结束时，不会在登记时立刻调用。返回 id |
| `timer.after(seconds, fn)` | `seconds` 秒后调用 `fn` 一次。返回 id |
| `timer.cancel(id)` | 取消这个 id。不认识的 id 没有动作 |

`timer.every` 的 `seconds` 必须是大于或等于 30 的数。短于 30 则不登记，显示「间隔过短」。`timer.after` 的 `seconds` 必须是正数；最短是多少，待定。两者都不是正数则不登记，显示「参数无效」。定时器数量、睡眠或关机期间如何补，待定。`on_unload` 取消本工具全部定时器。计时组件的倒数不走这两个函数，可以短于 30 秒。

### `notify`

| 函数 | 作用 |
| --- | --- |
| `notify.send(title, body)` | 发一条系统通知。两个参数都是字符串 |

内置工具和导入的工具都遵守安静时段：时段内不弹出；时段结束后，错过的多条按产品规格「待办」合并补发。

### `storage`

| 函数 | 作用 |
| --- | --- |
| `storage.get(key)` | 返回这个工具此前写下的字符串。没有则返回 `nil` |
| `storage.set(key, value)` | 写下字符串。写盘失败时原文件保留，窗口显示错误，返回 `nil` 和错误字符串 |

`key` 和 `value` 是不含密钥的文本。大小和条数待定。这些文本进入导出和备份。番茄钟不调用这两函数。密钥不进入导出和备份。

### `todo`

| 函数 | 作用 |
| --- | --- |
| `todo.list()` | 返回数组。需要 `todo.read` |
| `todo.complete(id)` | 完成这一条。需要 `todo.complete`。成功返回 `true`。失败返回 `nil` 和错误字符串 |

`todo.list` 的每一项：

| 字段 | 含义 |
| --- | --- |
| `id` | 待办 id |
| `listId` | 清单 id |
| `title` | 标题 |
| `completed` | 是否已完成 |
| `due` | 到期日 `YYYY-MM-DD`。没有则是 `nil` |
| `current` | 是否为当前待办 |

不含回收站。不含正文以外的便签字段。这里也读不到便签。

`todo.complete` 的规则与在待办页完成相同。冲突时返回的错误字符串是「待办保存冲突」，不写盘。找不到时是「找不到待办」。

### `http`

```lua
http.request({
  method = "GET",
  url = "https://api.example.com/v1/quota",
  secret = { id = "api_key", header = "Authorization", prefix = "Bearer " },
}, function(result)
end)
```

`secret` 可以省略。省略则不附带密钥。写了 `secret` 时，`id` 必须是清单里的一项，`header` 是头的名字，`prefix` 是值的前缀，可以是空字符串。宿主把「前缀 + 密钥原文」放进这个头。脚本读不到这个值。调用 `http.request` 的函数不等待网络；宿主在连接和读取结束后再调用下面的函数。

`result` 在请求结束时传入：

| 字段 | 含义 |
| --- | --- |
| `ok` | 状态码是 2xx 时为 `true` |
| `status` | HTTP 状态码。没有发出去时没有这个字段 |
| `body` | 响应正文，字符串。没有正文时是空字符串。正文或响应头对上密钥的列出形式时，正文改成空字符串，`ok` 为假，`error` 为「请求失败」。哪些形式、以及这是对已确认主机的尽力检查，见 tools.md「密钥」。响应头本身不在这个表里 |
| `error` | `ok` 为 `false` 时的说明，是 tools.md「错误」里的句子 |

第一批纳入的是 `https` 的 GET。其它方法显示「不在范围内」，是否以后纳入见 tools.md「待定」。附带密钥但不是 `https`、URL 无法解析，或协议不是 `http` / `https` 时，显示「地址无效」，不发请求。不附带密钥的明文 `http` 是否允许，待定。主机名未确认时显示「域名未允许」。密钥未填写时显示「未填写密钥」，并且不发请求。这些说明里没有密钥，也没有请求头。

窗口已经关闭时，不再调用这个函数。

### `secret`

没有读取函数。密钥只通过 `http.request` 的 `secret.id` 附带。

### 番茄钟的时长

只注入给 id 为 `lanwork.pomodoro` 的那块状态：

```lua
local work_min, break_min, long_min = pomodoro.durations()
```

三个返回值是设置里的分钟数，范围仍按产品规格。这不是第一批服务，其它工具的状态里没有 `pomodoro`。

## 示例：额度

用户导入后，悬浮窗用文本和进度条显示额度。打开时请求一次，之后每 300 秒再请求。主机名和密钥在导入时确认。示例假定服务商返回 `{"used":20,"limit":100}`，`used` 和 `limit` 是数字，`limit` 大于 0。真实工具换成该服务商的 URL 和字段；主机名仍写在 `domains` 里。

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

没填密钥时，`result.error` 是「未填写密钥」，请求不会发出。脚本里没有能把密钥打出来的调用。

## 示例：番茄钟

内置工具。计时规则以产品规格「番茄钟」为准。本示例只演示这些规则如何落在清单和 Lua 上。

阶段是工作、休息、长休息。卡片上的「悬浮窗」是打开并开始，还是只打开窗口，仍待定，见 tools.md「待定」。没有当前待办时这个按钮仍可打开，标题显示「没有当前待办」，不能开始。本示例不自动开始：打开并且存在当前待办时，阶段停在工作，按钮是「继续」，倒数不走；点「继续」才开始这一轮。工作结束并计为一轮后进入休息；已完成的工作满 4 轮时，这一次休息是长休息。休息或长休息结束之后回到工作。跳过进入下一阶段，工作被跳过时不记为完成，因此进入的是普通休息。暂停冻结剩余秒数。产品规格里的「完成」在本示例里只在工作阶段把本轮记为完成并进入休息，不发到点通知。到点才调用 `notify.send`。通知的用字产品规格没有写，示例用名称「番茄钟」和正文「到点」。

按钮是「继续」「跳过」「完成」。点「继续」之后改为「暂停」，再点回到「继续」。这些按钮是否放进窗口，见 tools.md「待定」。休息阶段点「完成」没有动作，这一条也待定。

当前待办换成另一条，或当前标记被清除时，停止计时，剩余显示为 0，不再留着上一条的剩余时间，也不自动开始下一次。倒数由计时组件完成，走到 0 时调用 `on_timer`。不使用 `timer.every`，因此不受「最短 30 秒」的刷新下限。暂停用 `ui.timer_running` 冻结宿主上的剩余秒数。关闭窗口时 `on_unload` 把倒数停掉。不调用 `storage`。

`manifest.toml`：

```toml
id = "lanwork.pomodoro"
name = "番茄钟"
description = "当前待办的计时"
entry = "main.lua"
services = ["notify", "todo.read"]

[[component]]
id = "title"
component = "text"

[[component]]
id = "status"
component = "text"

[[component]]
id = "clock"
component = "timer"

[[component]]
id = "pause"
component = "button"
label = "继续"

[[component]]
id = "skip"
component = "button"
label = "跳过"

[[component]]
id = "done"
component = "button"
label = "完成"
```

`main.lua`：

```lua
local phase = "work"
local running = false
local completed_work = 0
local started_id = nil

local function current_item()
  local items = todo.list()
  for _, item in ipairs(items) do
    if item.current and not item.completed then
      return item
    end
  end
  return nil
end

local function show_title(item)
  if item then
    ui.text("title", item.title)
  else
    ui.text("title", "没有当前待办")
  end
end

local function phase_seconds(name)
  local work_min, break_min, long_min = pomodoro.durations()
  if name == "work" then
    return work_min * 60
  end
  if name == "long" then
    return long_min * 60
  end
  return break_min * 60
end

local function status_text(name)
  if name == "work" then
    return "工作"
  end
  if name == "long" then
    return "长休息"
  end
  return "休息"
end

local function enter(name)
  phase = name
  running = true
  ui.text("status", status_text(name))
  ui.button("pause", "暂停")
  ui.timer("clock", phase_seconds(name))
  ui.timer_running("clock", true)
end

local function stop_session()
  running = false
  started_id = nil
  ui.text("status", "")
  ui.button("pause", "暂停")
  ui.timer("clock", 0)
  ui.timer_running("clock", false)
end

local function finish_phase(from_timer)
  if phase == "work" then
    completed_work = completed_work + 1
    if completed_work % 4 == 0 then
      enter("long")
    else
      enter("break")
    end
  else
    enter("work")
  end
  if from_timer then
    notify.send("番茄钟", "到点")
  end
end

function on_timer(id)
  if id ~= "clock" or not started_id then
    return
  end
  finish_phase(true)
end

function on_load()
  local item = current_item()
  show_title(item)
  if not item then
    ui.timer("clock", 0)
    ui.timer_running("clock", false)
    return
  end
  started_id = item.id
  completed_work = 0
  phase = "work"
  running = false
  ui.text("status", status_text("work"))
  ui.button("pause", "继续")
  ui.timer("clock", phase_seconds("work"))
  ui.timer_running("clock", false)
end

function on_unload()
  stop_session()
end

function on_todos_changed()
  if not started_id then
    return
  end
  local item = current_item()
  if item and item.id == started_id then
    show_title(item)
    return
  end
  show_title(item)
  stop_session()
end

function on_click(id)
  if not started_id then
    return
  end
  if id == "pause" then
    running = not running
    ui.timer_running("clock", running)
    if running then
      ui.button("pause", "暂停")
    else
      ui.button("pause", "继续")
    end
    return
  end
  if id == "skip" then
    if phase == "work" then
      enter("break")
    else
      enter("work")
    end
    return
  end
  if id == "done" and phase == "work" then
    finish_phase(false)
  end
end
```

时长在每次 `enter` 时读取，所以设置里改完之后，下一次进入该阶段用新的分钟数。已经走着的这一段剩余时间不改。这一段要不要立刻改用新时长，产品规格没有写。

宿主在这次 Lua 调用返回之后才继续倒数。同一回调里先 `ui.timer` 再 `ui.timer_running(id, false)`，返回前不会走到 0。暂停时冻结剩余秒数，继续时从冻结的值接着走。走到 0 才进入 `on_timer`。这不经过 `timer.every`，所以可以短于 30 秒。登记短于 30 秒的 `timer.every` 会被拒绝。`timer.after` 的最短秒数待定。
