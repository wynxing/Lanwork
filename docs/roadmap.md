# 实现状态

行为以 [product.md](product.md) 为准。本文只记录有没有代码和验证记录。工程骨架已在仓库中。搜索条前缀分类的纯函数在 `crates/core`（`search::classify_prefix`）。匹配引擎也在 `crates/core` 的 `search`。待办收集的日期前缀解析也在 `crates/core`（`parse_todo_due_prefix`）；星期、下周和月底的边界仍等产品规格写明。测量工具的代码已在仓库中，协议记录仍空着。技术验证没有通过记录，搜索条界面和收集提交也还没有代码。界面表里的各项仍全部未实现。

## 测量工具

| 项 | 状态 |
| --- | --- |
| `tools/fixture`、`tools/sample`、[docs/measurements/TEMPLATE.md](measurements/TEMPLATE.md) | 代码已在仓库中 |
| 符合架构「性能测量」协议的记录 | 无 |
| Windows 11 桌面上对空 Slint 窗口采样 5 分钟 | 未测 |

## 技术验证

各项的通过标准见 [architecture.md](architecture.md)「技术验证」。

| 项 | 记录 |
| --- | --- |
| 中文输入法 | 无 |
| 背景 | 无 |
| 渲染器 | 无 |
| 外部拖放 | 无 |
| Everything | 无 |
| Windows Search | 无 |
| 通知 | 无 |
| 热角 | 无 |

## 界面

| 界面 | 规格位置 | 代码 |
| --- | --- | --- |
| 搜索条、快速收集（搜索条内）、面板、搜索、待办、便签、收纳、GitHub、设置 | [product.md](product.md) 对应各节 | 无 |
| 番茄钟 | [product.md](product.md)「番茄钟」 | 无。未定参数补进产品规格之前不写 |

不存在第二份产品范围。新增界面时先改产品规格，再改本表。

## 无界面模块

| 模块 | 代码 |
| --- | --- |
| 匹配引擎 | 有。`crates/core` 的 `search`：拼音表、预计算、命中类型和分数。组内排序和 20 条的分组配额等 [product.md](product.md) 写入规格缺口 #9 第 7 项 |
