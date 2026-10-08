# 实现状态

行为以 [product.md](product.md) 为准。本文只记录有没有代码和验证记录。工程骨架已在仓库中。搜索条前缀分类的纯函数在 `crates/core`（`search::classify_prefix`）。匹配引擎也在 `crates/core` 的 `search`。待办收集的日期前缀解析也在 `crates/core`（`parse_todo_due_prefix`），包括已写入产品规格的星期、下周和月底边界。规格缺口 #9 的第 4 项和第 7 项已定，其余项仍待定。便签服务在 `crates/core`（`lanwork_core::notes`）。待办服务在 `crates/core`（`lanwork_core::todos`）：清单、收件箱、周期、软删除与恢复、当前标记、处理模式和跨清单移动已有代码；永久删除、原清单不存在时的恢复，以及短月没有对应日、在截止日当天完成是否再生成，仍等产品规格。收纳服务在 `crates/core`（`lanwork_core::shelves`）：分组、路径引用、去重、待办关联和存在性检查已有代码。已有分组但未指定目标、再次关联另一条待办、关联到尚不存在的待办 id，仍等产品规格。残留关联 id 在读取时视为无关联，并在下次写该分组时清除；该清除在永久删除落地时实现。GitHub 服务在 `crates/core`（`lanwork_core::github`）：本机 `gh` 的探测与调用、watchlist、刷新与快照、长期未更新和 Draft、转为待办、隐藏规则和来源同步已有代码。追踪仓库的添加与移除等 #9 第 17 项，以及需要处理、需要 Review、CI 失败和筛选组合等 #9 第 18 项，仍等产品规格。应用索引在 `crates/core` 的 `apps`。便携应用、别名和手动刷新入口仍等规格。测量工具的代码已在仓库中，协议记录仍空着。技术验证没有通过记录，搜索条界面、待办界面、收纳界面和收集提交也还没有代码。界面表里的各项仍全部未实现。

## 测量工具

| 项 | 状态 |
| --- | --- |
| `tools/fixture`、`tools/sample`、[docs/measurements/TEMPLATE.md](measurements/TEMPLATE.md) | 代码已在仓库中 |
| 符合架构「性能测量」协议的记录 | 无 |
| Windows 11 桌面上对空 Slint 窗口采样 5 分钟 | 2026-10-08，commit `0b7fd5f`，release。脚本通过：300 行，首尾 298.989 秒。Private Bytes 峰值 49,729,536 字节，工作集峰值 77,799,424 字节，平均 CPU 0.204%，唤醒平均 69.150/秒，句柄 326–343，USER 23，GDI 17。见 [measurements/2026-10-08-empty-window.md](measurements/2026-10-08-empty-window.md)。不是性能协议验收 |

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
| 待办服务 | 有。`crates/core` 的 `todos`：清单、收件箱、周期、软删除与恢复、当前标记、处理模式、跨清单移动。永久删除、原清单不存在时的恢复，以及短月没有对应日、在截止日当天完成是否再生成，仍等产品规格 |
| 收纳服务 | 有。`lanwork_core::shelves`：分组、路径引用、去重、待办关联、存在性检查。服务不移动、不复制、不删除用户文件。已有分组但未指定目标、再次关联另一条待办是拒绝还是替换、关联到尚不存在的待办 id，仍待定。残留关联 id 在读取时视为无关联，并在下次写该分组时清除；该清除在永久删除落地时实现。存在性检查每条路径的上限是 2 秒 |
| GitHub 服务 | 有。`lanwork_core::github`：`gh` 探测与调用、watchlist、刷新与快照、长期未更新、Draft、转为待办、隐藏规则、来源同步。添加和移除追踪仓库等 #9 第 17 项，需要处理、需要 Review、CI 失败和筛选组合等 #9 第 18 项，仍等产品规格。界面仍无 |
| 应用索引 | 有。`crates/core` 的 `apps`：开始菜单、App Paths、PATH、商店应用、去重、缓存和启动。便携应用和别名、手动刷新的界面入口等规格缺口 #9 第 1、2 项 |
