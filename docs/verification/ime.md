# 技术验证记录：中文输入法

日期：2026-10-08。人工测试是 2026-10-08～09。条件 5 的失败观察是 2026-10-10。

被测代码 commit：`da42398c94a79bdbf336af5548fe0ca45f689dc5`（`spikes/ime` 与变基前的 `93b01dba5a0bc64524d49ed45c6c07fd7d286c8e` 相同）。窗口标题是 `Lanwork IME`。

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

该项尚未通过。条件 5 在 2026-10-10 记为「未通过待复测」。按 architecture.md，全部条件都通过才算这项通过。

## 通过条件

2026-10-08～09，用户在 `DESKTOP-7C3P6OG` 上手测标题为 `Lanwork IME` 的窗口。用户报告当时是 Windows 11、微软拼音、100% 缩放。下面只记这份报告里的结果，不补报告里没有的上屏文字、计数或候选窗位置。

| 编号 | 条件 | 结果 | 观察 |
| --- | --- | --- | --- |
| 1 | 微软拼音在单行框里能组合并上屏，包括中英文混排、长句和翻页选词 | 通过 | 用户报告单行框输入没有问题 |
| 2 | 微软拼音在多行框里能组合并上屏，包括中英文混排、长句和翻页选词 | 通过 | 用户报告多行框输入没有问题 |
| 3 | 两个框的候选窗都贴在光标处，翻页时也贴着 | 通过 | 用户报告候选窗位置没有问题 |
| 4 | 拖动窗口后，候选窗仍贴在光标处 | 通过 | 用户报告拖动窗口后候选窗位置没有问题 |
| 5 | 窗口在 100% 与 150% 缩放的两块显示器之间移动后，组合和候选窗仍然正确 | 未通过待复测 | 2026-10-10，`DESKTOP-HDJS01V`，主屏 100%、副屏 150%。100%→150% 时组合和候选窗都在。150%→100% 时窗口被拉大，内容缩在左上角，候选窗消失，只剩拼音 preedit。绕过已写进 spike，还没有在这台机器上复测 |
| 6 | 组合未上屏时按 Enter，搜索条的 `accepted` 不增加 | 通过 | 用户报告 Enter 没有问题 |
| 7 | 组合未上屏时按数字键 1 到 5，两个框的「应用看到 1–5」都不增加，候选由输入法处理 | 通过 | 用户报告数字键选词没有问题 |

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

2026-10-10，在 `DESKTOP-HDJS01V`（Windows 11 build 26300）上，主屏 100%、副屏 150%。搜索条保持 `nihao` 未上屏，拖标题为「中文输入法验证」的窗口。这次没有把截图放进仓库。当时的日志在 `C:\Users\yumiw\AppData\Local\Temp\lanwork-ime-spike.log`。

| 方向 | 观察 |
| --- | --- |
| 100% → 150% | 组合还在，候选窗也在 |
| 150% → 100% | 整个窗口被拉大拉长，内容缩在左上角，外框没有按新的 DPI 缩小。候选窗消失，只剩拼音 preedit |

结果是「未通过待复测」。两个方向都要再看过，才能改成通过。

窗口逻辑尺寸固定为 720×760。变大之后缩放已是 1.0，所以内容画在客户区左上角，其余是空的。

原因在 winit 0.30.13，由 `i-slint-backend-winit` 1.18.1 带进来。`WM_DPICHANGED` 的处理自己用旧物理尺寸乘新缩放、除旧缩放来算外框，`lParam` 里的建议矩形没有参加这次计算。拖动过程中 `MonitorFromWindow` 仍返回正在离开的显示器，窗口被推回去，系统再发一次 DPI 消息。从 150% 回到 100% 时，第二次计算看见的缩放已经是 96，物理像素还是上一档的。720 逻辑像素在 144 DPI 上是 1080 物理像素；这 1080 再按从 96 到 144 乘回去，得到 1620，窗口变大。winit 问题 [4041](https://github.com/rust-windowing/winit/issues/4041) 和 [4600](https://github.com/rust-windowing/winit/issues/4600) 记的是同一条路径，0.30.13 里仍在。

Slint 1.18.1 的 `WinitWindowAdapter` 收到 `ScaleFactorChanged` 后不调用 `set_ime_cursor_area`。源码里留着保持逻辑尺寸的 TODO。输入法光标还停在旧的物理坐标上，微软拼音就不再显示候选窗。预编辑由 `TextInput` 自己画，所以还在。固定尺寸窗口在两块缩放不同的屏幕之间被拖大，也记在 Slint 问题 [11073](https://github.com/slint-ui/slint/issues/11073)。

绕过只在 `spikes/ime`，winit 仍是 0.30.13，`Cargo.lock` 里已有的版本没有因此改动。窗口装上 `SetWindowSubclass`。拖动期间，最外层的 `WM_DPICHANGED` 锁住建议矩形，在 `WM_WINDOWPOSCHANGING` 里用它换掉 winit 算出的外框。外框仍对不上时再 `SetWindowPos`，带 `SWP_NOACTIVATE`，避免输入法失焦。这次消息返回之后，拖动还没结束时只保住该尺寸，位置仍跟着光标。松手后按当前 DPI 再收一次外框，并用逻辑光标乘 `dpi/96`（四舍五入，与 winit 的 `to_physical` 相同）调用 `ImmSetCompositionWindow`（`CFS_POINT`）和 `ImmSetCandidateWindow`（`CFS_EXCLUDE`）。`HIMC` 只 `ImmReleaseContext`。windows 0.62 里 `HIMC` 的 `Free` 会调用 `ImmDestroyContext`，这里不走那条路。事件循环回到 Slint 之后再做一次，并在 50 毫秒后再做一次，躲开拖动模态循环里被缓住的缩放和尺寸事件。

复测仍用下面的 `cargo run -p ime`，不要设置 `SLINT_BACKEND`。启动日志应有一行 `DPI 子类已装上`。若出现 `DPI 子类未装上`，把那一行原文记下来，不要把条件 5 写成通过。

1. 主屏 100%，副屏 150%。微软拼音，指示器是「中」。
2. 搜索条输入 `nihao`，不要上屏。从 100% 拖到 150%。组合和候选窗都要还在，文字大小跟着那块屏幕，内容铺满窗口。
3. 再从 150% 拖回 100%。外框应随新 DPI 缩小，内容铺满窗口。候选窗仍贴着光标，preedit 仍在。
4. 便签框同样做这两个方向。
5. 日志里应能看到 `DPI 拖动` 和 `IME 光标`。把这两行原文留在观察栏。

两个方向都由人看过，才把结果从「未通过待复测」改成通过。建议截图 `docs/verification/ime/candidate-after-dpi.png`。

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

winit 后端在 `input_method_request` 里调用 `set_ime_cursor_area`，传入光标矩形。条件 3 和 4 的用户报告是候选窗位置没有问题。条件 5 里，缩放变化后 Slint 1.18.1 不更新这块区域；spike 在拖动跨 DPI 之后自己重设组合窗和候选窗。这一项仍是未通过待复测。

## 架构文档

没有改 `docs/architecture.md`。条件 5 未通过待复测，这项还没有通过，不能据此把某个 Slint 版本或后端写成必需设置。

条件 6 的用户报告是 Enter 没有问题，所以仍然不把文本框写法写进架构文档。若以后观察到组合期间 Enter 增加了 `accepted`，再写搜索条如何读到组合状态。`LineEdit` 没有转发 `preedit-text`。
