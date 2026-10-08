# 实现状态

行为以 [product.md](product.md) 为准。本文只记录有没有代码和验证记录。工程骨架已在仓库中。搜索条前缀分类的纯函数在 `crates/core`（`search::classify_prefix`）。匹配引擎也在 `crates/core` 的 `search`。待办收集的日期前缀解析也在 `crates/core`（`parse_todo_due_prefix`），包括已写入产品规格的星期、下周和月底边界。规格缺口 #9 的第 4 项和第 7 项已定，其余项仍待定。便签服务在 `crates/core`（`lanwork_core::notes`）。测量工具的代码已在仓库中，协议记录仍空着。技术验证没有通过记录，搜索条界面和收集提交也还没有代码。界面表里的各项仍全部未实现。

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
| 匹配引擎 | 有。`crates/core` 的 `search`：拼音表、预计算、命中类型和分数，以及产品规格里的组内排序和 20 条分组配额。使用频率只作为排序输入，不在这里保存。频率如何增减仍待定 |
| 便签服务 | 有。`lanwork_core::notes`：模型、保存、标签、置顶、`revision` 冲突和软删除恢复。删除的界面入口和冲突后的选择等 [product.md](product.md) 写入 #9 第 10、16 项。测试 `one_mib_body_roundtrip_records_write_time` 打印 `note_write_1mib_ms`，只记录 1 MiB 正文的写盘耗时，不是性能测量验收 |
