# 技术验证记录：中文输入法

日期：2026-10-08。

被测代码 commit：`93b01dba5a0bc64524d49ed45c6c07fd7d286c8e`。

Slint 版本：1.18.1（工作区 `slint = "=1.18.1"`，`cargo tree -p ime` 解析到 `slint v1.18.1`）。

渲染器：未设置 `SLINT_BACKEND`。`slint` 1.18.1 的 default 特性包含 `backend-winit`、`renderer-femtovg` 和 `renderer-software`，不包含 Skia。`i-slint-backend-winit` 1.18.1 在启用 `renderer-femtovg` 且未启用 Skia 时，默认工厂使用 `GlutinFemtoVGRenderer`，默认名称是 `FemtoVG`。本次启动打印为 `SLINT_BACKEND=未设置`。

机器：计算机名 `DESKTOP-7C3P6OG`，交互会话用户 `wynn`，会话 1。任务里的 worker 名是 `lanwork-win`。

Windows build：注册表 `ProductName` 为 `Windows 10 Home China`，`DisplayVersion` 为 `26H2`，`CurrentBuild` 为 `26300`，`UBR` 为 `9550`。`[Environment]::OSVersion` 为 `Microsoft Windows NT 10.0.26300.0`。

GPU 与驱动：

| 名称 | 驱动版本 | 驱动日期 |
| --- | --- | --- |
| Intel(R) UHD Graphics | 32.0.101.6733 | 2025-04-02 |
| Virtual Display Driver | 11.30.4.434 | 2024-12-24 |
| GameViewer Virtual Display Adapter | 15.6.5.199 | 2026-02-28 |

`[System.Windows.Forms.Screen]::AllScreens` 只返回 `\\.\DISPLAY1`，主显示器，边界 1280×800。`GetDpiForMonitor` 的有效 DPI 是 96，即 100%。这次自动检查没有看到第二块显示器，也没有看到 150% 缩放。

构建配置：debug。编译器 `rustc 1.99.0 (b940084d7 2026-09-28)`。并入当前 `origin/main` 之后又跑了 `cargo fmt --all -- --check`、`cargo clippy --all-targets -- -D warnings` 和 `cargo test`。没有设置 `SLINT_BACKEND`。

该项尚未通过。下面每个通过条件仍是「未测」。按 architecture.md，全部条件都通过才算这项通过。

## 通过条件

把「未测」改成「通过」或「失败」之前，按下一节亲手输入并看屏幕。没有亲眼看到的条件保持「未测」。

| 编号 | 条件 | 结果 | 观察 |
| --- | --- | --- | --- |
| 1 | 微软拼音在单行框里能组合并上屏，包括中英文混排、长句和翻页选词 | 未测 | |
| 2 | 微软拼音在多行框里能组合并上屏，包括中英文混排、长句和翻页选词 | 未测 | |
| 3 | 两个框的候选窗都贴在光标处，翻页时也贴着 | 未测 | |
| 4 | 拖动窗口后，候选窗仍贴在光标处 | 未测 | |
| 5 | 窗口在 100% 与 150% 缩放的两块显示器之间移动后，组合和候选窗仍然正确 | 未测 | |
| 6 | 组合未上屏时按 Enter，搜索条的 `accepted` 不增加 | 未测 | |
| 7 | 组合未上屏时按数字键 1 到 5，两个框的「应用看到 1–5」都不增加，候选由输入法处理 | 未测 | |

架构文档的通过标准是：微软拼音在 Slint 文本框里能组合、上屏，候选窗贴在光标处；组合期间 Enter 不触发提交。条件 1 到 4 和 6 对应这句话。条件 5 和 7 来自 issue #3 的边界和异常标准（150% 显示器，以及处理模式要用的数字键）。第三方输入法不在这张表里。

## 人工步骤

在包含本记录的仓库根目录执行。不要设置 `SLINT_BACKEND`。

```text
cargo run -p ime
```

这是 debug 构建。窗口标题文字是「中文输入法验证」。它没有系统标题栏。拖顶部那一行可以移动窗口。点「关闭」退出。每次启动会清空 `%TEMP%\lanwork-ime-spike.log`，窗口最下方有这份文件的路径。界面日志只保留最新 40 行，完整内容在该文件里。

当前用户的 `zh-Hans-CN` 输入法提示是 `0804:{81D4E9C9-1D3B-41BC-9E6C-4B40BF79E35E}{FA550B04-5AD7-411F-A5AC-CA038EC515D7}`，注册表描述为 `Microsoft Pinyin`。自动检查没有打开输入法界面。开始前仍要在屏幕上确认：

1. 按 `Win+空格`，直到指示器显示微软拼音。
2. 指示器是「中」而不是「英」。按 `Shift` 可以切换。
3. 翻页用 `-` 和 `=`，或 `PageUp` 和 `PageDown`。如果这台机器改过翻页键，用实际按键，并写进观察栏。

计数的含义：数字增加表示按键到达了应用。程序收到后仍把按键交回文本框，不拦截输入法。`preedit：` 为「（空）」表示没有未上屏的组合。

先做两次对照，确认计数有效：

1. 点「搜索条（单行 TextInput）」。不要开组合。输入已上屏的文字后按 `Enter`。`accepted` 应加 1，日志有一行 `搜索条 accepted`。
2. 组合为「（空）」时按 `1`。`应用看到 1–5` 应加 1，框里出现字符 `1`。

对照失败时，不要把条件 6 或 7 写成通过。把日志原文写进观察栏，结果写「失败」。

### 条件 1

1. 点搜索条，清空文字。
2. 中文状态下输入 `nihao`，先不要按空格。框里应出现带下划线的组合，`preedit：` 不再是「（空）」。
3. 按空格。文字应变成「你好」，`preedit：` 回到「（空）」。
4. 按 `Shift` 切到英文，输入 `Lanwork`，再按 `Shift` 回到中文。
5. 输入 `ceshi`，按空格，应再出现「测试」。
6. 输入 `jintianxiawusandiankaihui`。候选出现后按 `-` 或 `PageDown` 翻一页，再用数字键选词，直到整句上屏。长句应留在单行框里，光标仍在可见范围内。

三项都发生则结果写「通过」，并写上屏后的实际文字。任一项没有发生则写「失败」，并写停在哪一步。

### 条件 2

在「便签正文（多行 TextInput）」重复条件 1 的第 2 到 6 步。上屏后再按一次 `Enter`（此时 `preedit：` 必须是「（空）」），应出现第二行。多行框没有 `accepted` 计数。

### 条件 3

在条件 1 第 2 步和条件 2 的同一步，以及翻页时看候选窗。它应挨着光标，而不是停在屏幕角落，或相对窗口固定在别处。两个框都要看。把候选窗相对光标的位置写进观察栏。建议截图 `docs/verification/ime/search-candidate.png` 和 `docs/verification/ime/note-candidate.png`。

### 条件 4

在搜索条输入 `nihao` 并且不按空格，候选窗还在时，拖「中文输入法验证」那一行，把窗口挪开大约 200 像素。候选窗应跟着光标走。松手后再看一次。便签框同样做一次。建议截图 `docs/verification/ime/candidate-after-move.png`。

### 条件 5

这次会话只有一块 100% 的显示器，所以这一项现在不能测。准备好一块 100% 和一块 150% 的显示器后再测：在搜索条保持 `nihao` 未上屏，把窗口拖到另一块显示器上。组合应仍在，候选窗仍贴着光标，文字大小应跟着那块屏幕的缩放。两块缩放都看过才把结果从「未测」改掉。建议截图 `docs/verification/ime/candidate-after-dpi.png`。

### 条件 6

1. 记下搜索条的 `accepted` 和「应用看到 Enter」。
2. 输入 `nihao`，候选还在、`preedit：` 不是「（空）」时按 `Enter`。
3. 看计数和日志最上面几行。

结果写「通过」的条件：输入法自己确认或取消了这次组合，并且 `accepted` 没有因为这次 `Enter` 增加。日志里如果出现 `搜索条 accepted`，且同一时刻 `preedit` 不是「（空）」，结果写「失败」。把相关日志行原文贴进观察栏。建议截图 `docs/verification/ime/enter-during-compose.png`。

### 条件 7

1. 记下两个框的「应用看到 1–5」。
2. 在搜索条输入 `yi`，让候选出现，`preedit：` 不是「（空）」。
3. 按 `1`。若这次按键把词上屏了，再输入 `yi`，依次按 `2`、`3`、`4`、`5`。每次按之前组合都要还在。
4. 在便签框重复第 2 步和第 3 步。

结果写「通过」的条件：数字键被输入法用来选词，两个框的「应用看到 1–5」都没有在 `preedit：` 非空时增加。计数增加了，或候选没有被选中而字符 `1` 到 `5` 直接进了正文，结果写「失败」。把日志里对应的 `按键` 行贴进观察栏。建议截图 `docs/verification/ime/digits-during-compose.png`。

截图放进 `docs/verification/ime/` 后再提交。本记录现在没有截图。

## 第三方输入法

不作为通过条件。结果：未测。

装好搜狗输入法或微信输入法后，用 `Win+空格` 切过去，把条件 1、2、3、6、7 各做一遍。只把结果写在这里，不要改上面的通过条件表。

| 输入法 | 结果 | 观察 |
| --- | --- | --- |
| 搜狗或微信，写明实际名称和版本 | 未测 | |

## 已自动核对

这些不是通过条件，不使这项变成通过。

| 核对 | 结果 |
| --- | --- |
| `cargo fmt --all -- --check` | 退出码 0。spike commit 上一次，并入 `origin/main` 的 merge commit `5c873138ddd9062db0a68802d90712a656e6f833` 上又一次 |
| `cargo clippy --all-targets -- -D warnings` | 退出码 0。同上，两次 |
| `cargo test`（spike commit `93b01db`） | 退出码 0。`ime` 4 项通过，当时的 `lanwork-core` 1 项通过 |
| `cargo test`（merge commit 之后的工作区） | 本机：`ime` 4 项通过，其余测试通过。`crates/core/tests/disk_full.rs` 未在本机跑完：这次会话不是管理员，`diskpart` 返回错误 740（请求的操作需要提升），测试进程以 `0xc0000409` 退出。该测试来自 main，要提升权限才能创建 VHD。Windows CI 作业 `fmt, clippy, test`（run `37788220029`）里的 `cargo test` 步骤成功，那次 runner 有管理员权限 |
| `LANWORK_IME_SPIKE_SMOKE=1` 运行 `target\debug\ime.exe` | 退出码 0。标准错误打印 `启动 Slint 1.18.1 构建 debug SLINT_BACKEND=未设置`。进程创建了窗口并离开事件循环。没有看屏幕，没有输入文字 |

`TextInput.preedit-text` 能在 `.slint` 里绑定：`spikes/ime/ui/main.slint` 绑定了它，并且 `cargo build -p ime` 成功。Slint 1.18.1 的内建元素把该属性标成内部、未写入文档、只为输入法暴露。

`LineEdit` 和 `TextEdit` 在 1.18.1 里用 `text-input.preedit-text` 决定占位文字是否显示，但没有把该属性声明成自己的属性。用这两个控件的应用代码读不到组合文本。本 spike 直接使用 `TextInput`，单行的横向滚动和多行的 `ScrollView` 按 1.18.1 控件源码里的同一做法接上，这样长句时光标还能留在可见区域，同时界面能显示 `preedit`。

单行 `TextInput` 收到 `KeyPressed` 且文本是换行（`U+000A`）时调用 `accepted`。这段代码不看 `preedit-text`。组合期间 `Enter` 会不会变成这次按键，取决于 Windows 和 winit 是否把按键交给应用。spike 只计数，不吞掉按键。数字键 1 到 5 同样没有在组合期间被 Slint 滤掉。

winit 后端在 `input_method_request` 里调用 `set_ime_cursor_area`，传入光标矩形。候选窗是否真的贴在光标上，要靠条件 3、4、5 看屏幕。

## 架构文档

没有改 `docs/architecture.md`。行为条件都还是未测，不能据此把某个 Slint 版本或后端写成必需设置。

如果条件 6 失败，后续界面在提交前必须读到组合状态。`LineEdit` 没有转发 `preedit-text`，那时要在架构文档写明搜索条使用能读到该属性的文本框。在条件 6 有观察结果之前不写。
