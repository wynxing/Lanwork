<div align="center">

<img src="crates/app/assets/lanwork-256.png" alt="Lanwork 图标" width="128" height="128">

# Lanwork

**一个热键呼出的 Windows 11 桌面效率工具：搜索、待办、便签、收纳，都在手边。**

用 Rust 和 [Slint](https://slint.dev/) 原生编写，不带 Vue、Tauri 或 WebView2。

[![Release](https://img.shields.io/github/v/release/wynxing/Lanwork?include_prereleases&sort=semver&label=release&logo=github)](https://github.com/wynxing/Lanwork/releases/latest)
[![下载量](https://img.shields.io/github/downloads/wynxing/Lanwork/total?label=downloads&logo=github)](https://github.com/wynxing/Lanwork/releases)
[![CI](https://img.shields.io/github/actions/workflow/status/wynxing/Lanwork/ci.yml?branch=main&label=CI&logo=githubactions&logoColor=white)](https://github.com/wynxing/Lanwork/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/github/license/wynxing/Lanwork?label=license)](LICENSE)
[![Windows 11 x64](https://img.shields.io/badge/Windows-11%20x64-0078D4?logo=windows11&logoColor=white)](#requirements)
[![Rust 1.99.0](https://img.shields.io/badge/Rust-1.99.0-DEA584?logo=rust&logoColor=white)](rust-toolchain.toml)
[![Slint 1.18.1](https://img.shields.io/badge/Slint-1.18.1-2379F4)](https://slint.dev/)

[下载](#download) · [功能](#features) · [快速上手](#quickstart) · [路线图](#roadmap) · [从源码构建](#dev) · [文档](#docs)

</div>

---

<!--
  TODO（待补）：界面截图。目前仓库里没有截图，不要放示意图或拼接图。
  建议补充，放在 docs/images/ 下，并用相对路径引用：
    1. 搜索条（结果列表展开，含 Alt+数字提示）
    2. 面板：待办页
    3. 便签悬浮窗（始终在最上层，可贴边收成把手）
    4. 收纳分组悬浮窗
  截图应取自 Windows 11 上的真实运行界面，浅色和深色各一张更好。
-->

<a id="why"></a>

## ✨ 为什么是 Lanwork

- **一个热键，直达想要的东西。** 按下 `Ctrl+Alt+M`，居中的搜索条一次搜遍应用、文件、待办、便签和收纳，也能直接打开网址或搜索网页。
- **轻量原生。** 界面由 Slint 绘制，亚克力背景由 Windows 系统提供。没有内嵌浏览器。
- **数据留在本机。** 数据放在 Lanwork 自己的目录，默认 `%USERPROFILE%\Documents\Lanwork`，格式是 JSON。没有云同步，没有账号。
- **界面只说事实。** 只显示名称、数量、时间和状态。没有欢迎语和功能介绍，没有「今日」页。
- **中文友好。** 应用名、待办标题、便签标题和标签支持全拼和拼音首字母，中文输入法的组合输入期间不会误触 Enter。

<a id="features"></a>

## 🧰 功能

> 状态以 [docs/roadmap.md](docs/roadmap.md) 和仓库中的代码为准。尚未实现的部分列在 [路线图](#roadmap)。

### 🔍 搜索条

按热键呼出，再按一次收起。无边框圆角窗口，使用系统亚克力背景（系统不支持或关闭透明效果时自动改为纯色）。

| | 说明 |
| --- | --- |
| 🧭 **多种来源** | 应用（开始菜单、商店应用，以及 Steam 和 Epic 的游戏链接）、文件、文件夹、待办、便签、收纳、网址、网页搜索。收纳组排在便签之后、本机文件之前，包含分组里的文件、文件夹和网页引用，名称下方是「收纳 · 分组名」；已经出现在这一组的文件不再在「文件」组重复 |
| 🔤 **拼音匹配** | 应用名、待办标题、便签标题和标签支持全拼、拼音首字母、子串和模糊匹配；便签正文只做原文子串匹配，文件只按文件名匹配。按命中类型和使用频率排序，同一条结果只出现一次 |
| ⚡ **Alt+数字直达** | 前 9 条结果右侧显示 `Alt+1` 至 `Alt+9`，按下即打开 |
| 🌐 **打开网址** | 输入像网址时，第一条是「打开网址」，没写协议会补 `https://`。结果最后总有一条网页搜索，默认 Google，可在配置里改为必应或百度 |
| 📁 **文件搜索** | 优先使用本机已运行的 [Everything](https://www.voidtools.com/)（1.4 和 1.5），未运行或未就绪时回退到 Windows Search 索引。两者都只按文件名匹配 |
| ⌨️ **键盘操作** | `Enter` 打开；文件和文件夹上 `Ctrl+Enter` 打开所在目录；应用上 `Ctrl+Enter` 或 `Ctrl+Shift+Enter` 以管理员身份运行；便签结果上 `Ctrl+Enter` 悬浮该便签 |
| 🖱️ **可拖动** | 拖动搜索条到喜欢的位置，下次呼出（包括重启程序后）仍在那里；拖动不影响输入框里的文字选择 |
| 🖱️ **应用菜单** | 在应用结果上右键：以其他用户身份运行、以管理员身份运行、打开文件所在文件夹、打开目标文件夹、隐藏。菜单打开时 `Alt+1` 至 `Alt+5` 选这些项。隐藏后可在设置页恢复 |
| 🈶 **输入法友好** | 组合输入未上屏时，Enter 和 Alt+数字都不会执行结果 |

### 🗂️ 面板

标签依次为待办、便签、收纳、设置。从托盘菜单、热角或搜索条里的待办、便签结果打开。设置页分成常规、待办、数据、其他四组：可以改开机启动、热角、搜索条热键、主题、网页搜索引擎、安静时段和「稍后」天数，也可以更换数据目录、备份、导出、导入，以及恢复隐藏的应用、检查更新。收起面板不会退出程序。

### ✅ 待办

- 清单与系统收件箱，新建、改标题、上移下移排序
- 完成与取消完成；已完成条目取消后回到原位置
- 到期日、提醒时间，以及周期：每天、每周、每两周、每月（可设重复截止日；每月 31 日在小月落到月末，下一次仍回到原来的日子）
- 设为「当前待办」，重启后保留
- 软删除进回收站，可恢复；回收站里满 30 天自动清除，永久删除只能在回收站里进行
- 托盘图标右下角用数字徽章显示逾期数量
- **到期提醒**：到点弹出系统通知。错过的多条合并成一条，可以展开查看。安静时段内不弹出。安装版点击单条会打开面板并定位；多条合并的点击打开收件箱。便携版只显示提醒，点击不定位
- **处理模式**：收件箱有未完成条目时，清单顶部有「处理 N 条」。一次一张卡片：1 今天、2 稍后、3 移动、4 完成、5 删除，Esc 退出。最后一条之后显示「收件箱已清空」
- 条目上的标记：到期日、周期、来源、关联收纳；有关联收纳分组时可一键「打开关联收纳」
- **悬浮窗**：待办页上的「悬浮」打开唯一的窗口，列出全部清单里的待办，含已完成。勾选完成或取消完成；逾期条目整行文字为红色。不能在窗口里新建或修改。始终在最上层，可贴边，可与便签、收纳叠页。展开时拖动条下面一行是「待办」。失去焦点后仍是半透明磨砂

### 📝 便签

- 新建、编辑、标签、置顶、按标签筛选；停止编辑 1 秒后自动保存
- 输入法组合期间不保存；保存失败时可重试或放弃修改；面板和悬浮窗同时改同一篇时，由你选择保留哪个版本
- **悬浮窗**：把一篇便签变成独立小窗，同时可开多个；每篇各自记住位置和大小；窗口始终在其他窗口之上，没有钉住按钮。展开时拖动条下面一行是「便签 · 名称」：先取标题里第一个有字的行，否则取正文首行，都没有则是「无标题」。失去焦点后仍是半透明磨砂
- **贴边与工作区**：不贴边时，拖动松手、再打开、改大小或内容把窗口撑大，整扇都留在所在显示器的工作区里。拖到屏幕外侧的左、右或上边松开，收成半透明细把手；拖出这三条边再松开，先回到工作区边缘，然后收成把手。底边拖出去只回到工作区
- **叠页**：两个悬浮窗重叠超过一半后松开，合成一叠；左右方向键翻页。展开并且至少两页时，窗口下方靠左有一排彩色圆点，按页序循环红、黄、绿、蓝、紫、橙、粉、青。点一个换页，拖一个可以把那一页拆出去
- 删除进回收站，保留 30 天，可恢复
- **多选**：列表顶部「选择」，可勾选多篇，批量浮出，或确认后删除进回收站。Ctrl+点击、Shift+点击也可以开始多选。回收站和悬浮窗没有多选

### 📎 收纳

只保存引用。文件和文件夹记**路径**，不移动、不复制原文件。网页记网址、标题和网站图标，不关闭、不移动浏览器标签。

- 从资源管理器**拖入**文件或文件夹（可一次多个），同一分组内同一路径只保留一条
- **拖出**到资源管理器或其他应用，只允许复制和创建快捷方式，不允许移动；`Esc` 取消
- 分组可新建、重命名、排序，删除分组只删引用，原文件不受影响
- 路径失效时显示「路径不存在」
- **关联待办**：搜索并点击即可关联，一个分组可关联多条待办，一条待办也可被多个分组关联
- **网页**：从 Chrome 或 Edge 把地址栏锁图标或页面链接拖进收纳页或收纳悬浮窗。全部收纳里同一网址只保留一条。搜索里归入「收纳」组。打开后用默认浏览器重新打开。网站图标在后台获取，取不到时图标位留空；这份缓存不进入导出和备份
- **多选**：分组列表有「选择」，可一次悬浮或确认后删除多个分组，删除只去掉引用。分组内的条目可以 Ctrl、Shift 或 Ctrl+A 多选，再移除、打开，或拖到另一个分组。悬浮窗里没有这一套多选
- **悬浮窗**：每个分组可以独立悬浮，同样支持拖入、拖出，始终在最上层，可贴边、可与便签和待办叠页，并记住位置和大小；窗口里有可折叠的「关联待办」，可直接勾选完成，或点标题回到面板定位该待办。展开时拖动条下面一行是「收纳 · 分组名」。失去焦点后仍是半透明磨砂

### 🛠️ 更多

| | 说明 |
| --- | --- |
| 🧷 **托盘常驻** | 单实例；托盘菜单有打开面板、设置、新建便签、退出；退出前先保存未写入的便签 |
| 🎨 **主题** | 明、暗、跟随系统；亚克力背景由系统绘制，不提供透明度滑块 |
| 💾 **备份、导出与导入** | 每天第一次启动自动备份一次。设置页可以立即备份、导出和导入。自动与手动备份合计最多保留 7 份。导入先看概览，确认后才替换；中断后下次启动恢复为导入前的数据 |
| 🔒 **安全的写盘** | 数据先原子写入，失败时保留原文件并在触发动作的界面显示错误 |

<a id="roadmap"></a>

## 🚧 路线图

以下内容规格已经写明，但**还没有可用的界面**，不属于 v0.1.0 的承诺。其中不少已有 `lanwork-core` 里的服务和测试，只差把界面接上。

| 方向 | 现状 |
| --- | --- |
| ⚡ **快速收集** | 搜索条里以 `/note` 开头快速收集便签。待办快速收集已放弃；便签快速收集提交动作尚未接入搜索条 |
| ⚙️ **设置页其余项** | 便携应用、别名和手动刷新的界面入口仍待定。设置不另设「应用」页 |
| 🍅 **番茄钟** | 计时规则已写入产品规格。作为应用页的内置工具，用清单和沙箱 Lua 来写，排在 v0.4.0 之后。产品界面还没有。宿主在 `crates/toolhost`。窗口验证程序在 `spikes/toolhost`。技术验证「工具宿主」已通过，凭据管理器界面待用户用 `--toolhost-cred` 确认 |
| 🧪 **验收与测量** | 背景、渲染器等技术验证项还有没记成通过的部分；没有符合测量协议的性能记录，因此这里不给出内存和延迟数字 |

### v0.4.0 之后

| 方向 | 现状 |
| --- | --- |
| 🤖 **外部 AI** | 本机 MCP，默认关闭，在设置里打开。只改待办和便签。规格见 [docs/external-ai.md](docs/external-ai.md)。还没有代码 |
| 🧩 **应用页与自定义工具** | 面板上的「应用」标签，分类是「内置」和「自定义」。悬浮窗外壳加 TOML 清单和沙箱 Lua。番茄钟是内置工具。规格见 [docs/tools.md](docs/tools.md)。应用页有代码，未在 Windows 上验收；已安装工具还是空的，导入、卸载、更新、查看权限和番茄钟迁移还没有。宿主在 `crates/toolhost`。窗口验证程序在 `spikes/toolhost`。技术验证「工具宿主」已通过，凭据管理器界面待用户用 `--toolhost-cred` 确认。一键把网页收进收纳的浏览器扩展也还没有，先做 Chrome |

不做的事：「今日」聚合页、GitHub PR/Issue 追踪、由搜索词自动创建待办、云同步、文件内容搜索。完整清单见 [产品规格](docs/product.md)「明确不做」。

<a id="download"></a>

## 📥 下载安装

前往 **[Releases › Latest](https://github.com/wynxing/Lanwork/releases/latest)** 下载：

| 版本 | 适合 |
| --- | --- |
| **安装版（`.exe`）** | 推荐。常规安装 |
| **便携版（`.zip`）** | 解压即用 |

> 当前版本是 v0.4.0。自 v0.3.1 以来的变化见 [更新说明](CHANGELOG.md)。如果 Releases 页面暂时还没有这一版的文件，说明正在发布中。

安装或解压后运行 `lanwork.exe`，托盘区会出现 Lanwork 图标。

<a id="requirements"></a>

## 💻 系统要求

- Windows 11，x64
- 亚克力背景要求 Windows 11 Build 22621 或更高；低于此版本、关闭系统「透明效果」或处于节电模式时，自动改用纯色背景，功能不受影响
- 可选：[Everything](https://www.voidtools.com/) 1.4 或 1.5，需要在本机安装并**正在运行**，Lanwork 才会用它搜索文件。Lanwork 通过 SDK 的 DLL 连接它，发布包会把 DLL 放在 `lanwork.exe` 旁边。Everything 不可用时回退到 Windows Search，只搜 Windows 已建立索引的位置；两者都不可用时，文件结果显示「文件索引不可用」，其余结果不受影响

非 Windows 系统上，程序会打印「Lanwork 只支持 Windows 11 x64」并退出。

<a id="quickstart"></a>

## 🚀 快速上手

1. 运行 `lanwork.exe`，程序驻留在托盘。
2. 按 **`Ctrl+Alt+M`** 呼出搜索条，输入应用名、文件名、待办标题或便签内容。应用名、待办标题、便签标题和标签可以用全拼或拼音首字母；文件按文件名匹配；便签正文只做原文子串匹配。
3. 用 `↑` `↓` 选择后按 `Enter` 打开，或直接按 `Alt+1` 至 `Alt+9`；`Esc` 收起。
4. 在搜索条里找到待办或便签，按 `Enter` 会打开面板并定位到该条。
5. 右键托盘图标，打开面板或新建便签。收纳页里把文件拖进来即可建立引用。

| 默认值 | |
| --- | --- |
| 搜索条热键 | `Ctrl+Alt+M`（再按一次收起） |
| 热角 | 默认右上。光标停留 350 毫秒后打开面板；光标所在屏幕有全屏窗口时不触发。可在设置里改为左上、左下、右下或关闭 |
| 数据目录 | `%USERPROFILE%\Documents\Lanwork`，可在设置页更换 |
| 网页搜索引擎 | Google |

开机启动、热角、搜索条热键、主题、网页搜索引擎、安静时段、「稍后」天数、数据目录、备份、导出、导入、隐藏应用的恢复和检查更新可以在设置页里改。便携应用和别名等其余项仍只能手改数据目录里的 `config.json`。热键格式例如 `Ctrl+Alt+M`。除 `F1`–`F24` 可以单独使用外，主键要带上 `Ctrl`、`Alt`、`Shift`、`Win` 中的至少一个；主键还可以是空格、方向键、标点、小键盘和导航键。手改出错时：JSON 无法解析，文件会被改名为 `config.json.corrupt-*`，本次启动使用默认配置；JSON 能解析但字段不合法，原文件保持不动，本次启动使用默认配置；`schemaVersion` 不是程序认识的版本，程序会拒绝启动。

<a id="dev"></a>

## 🛠️ 开发

### 从源码构建

需要 Windows 11 x64 和 MSVC 工具链。Rust 版本由 [`rust-toolchain.toml`](rust-toolchain.toml) 固定为 1.99.0（含 `clippy`、`rustfmt`）。

```powershell
git clone https://github.com/wynxing/Lanwork.git
cd Lanwork

cargo build --release -p lanwork
.\target\release\lanwork.exe
```

开发时可直接 `cargo run -p lanwork`。

**Everything 的 DLL。** 发布构建（`--release`）只从 `lanwork.exe` 所在目录加载 Everything 的 DLL，不会回到仓库里找。要在自己编译的 release 版里用 Everything 搜索，需要把 `third_party/everything/sdk3/Everything3_x64.dll`（对应 Everything 1.5）和 `third_party/everything/sdk/Everything64.dll`（对应 Everything 1.4）复制到 `target\release\` 下，并且本机已运行 Everything。没有 DLL 或 Everything 没运行时，文件搜索自动回退到 Windows Search，不影响其他功能。debug 构建（`cargo run`）会直接使用仓库里的 DLL。

与 CI 一致的检查：

```powershell
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

CI 在 `windows-latest` 上运行，还会检查 `lanwork-core` 的依赖树里没有 Slint。

### 项目结构

| 路径 | 内容 |
| --- | --- |
| `crates/core` | 包名 `lanwork-core`。模型、服务、存储、搜索匹配与调度、备份等，不依赖 Slint，也不依赖 Win32 窗口 API |
| `crates/app` | 包名 `lanwork`，产物 `lanwork.exe`。程序外壳（单实例、托盘、热键、主题、开机启动）、搜索条、面板、便签与收纳悬浮窗；Slint 界面文件在 `ui/` |
| `spikes/` | 技术验证程序：渲染与背景（`lanwork-render-spike`）、热角（`hotcorner`）、工具宿主（`lanwork-toolhost-spike`）。已通过的 spike 已归档清理 |
| `tools/` | 测量工具：固定数据夹具 `lanwork-fixture`、进程采样 `lanwork-sample`，以及图标渲染脚本 |
| `third_party/` | 第三方许可与随附文件：Unihan 读音摘录、Everything SDK |
| `docs/` | 产品规格、架构、实现状态。技术验证结论在 `docs/roadmap.md`，测量模板在 `docs/measurements/TEMPLATE.md` |

### 数据目录

按顺序解析：环境变量 `LANWORK_DATA_DIR`，其次 `%LOCALAPPDATA%\Lanwork\bootstrap.json`，最后默认的 `%USERPROFILE%\Documents\Lanwork`。开发和测量时可设置 `LANWORK_DATA_DIR` 指向临时目录，避免动到自己的数据。

### 固定数据与采样

```powershell
cargo run -p lanwork-fixture -- --out <目录>
cargo run -p lanwork-sample -- sample --name lanwork --duration-secs 300 --interval-ms 1000 --out sample.csv
```

用法、数据格式和测量协议见 [tools/README.md](tools/README.md) 和 [architecture.md](docs/architecture.md)「性能测量」。

<details>
<summary>技术验证程序（spikes）</summary>

```powershell
cargo run -p hotcorner -- probe --out probe.json
```

渲染器和背景的验证程序是 `lanwork-render-spike`，各渲染器的 feature 互斥。工具宿主的验证程序是 `lanwork-toolhost-spike`。详见 [spikes/README.md](spikes/README.md)。

</details>

<a id="docs"></a>

## 📚 文档

| 文档 | 内容 |
| --- | --- |
| [产品规格](docs/product.md) | 每个界面的入口、内容、动作、禁止项和验收。可见行为以它为准 |
| [架构](docs/architecture.md) | Slint、Rust、数据与性能测量。技术验证结论见实现状态。它描述目标，不是现有代码的地图 |
| [实现状态](docs/roadmap.md) | 哪些界面已有代码、哪些没有，以及技术验证记录 |
| [外部 AI](docs/external-ai.md) | 本机 MCP。排在 v0.4.0 之后，还没有代码 |
| [应用页与自定义工具](docs/tools.md) | 悬浮窗工具。原先的扩展页已并入。排在 v0.4.0 之后。应用页有代码，未在 Windows 上验收；导入、卸载、更新、查看权限和番茄钟迁移还没有。宿主在 `crates/toolhost`。窗口验证见 `spikes/toolhost`。技术验证「工具宿主」已通过，凭据管理器界面待用户用 `--toolhost-cred` 确认。开发说明见 [docs/tool-dev-guide.md](docs/tool-dev-guide.md) |
| [测量模板](docs/measurements/TEMPLATE.md) | 空白模板，不是实测。技术验证结论在实现状态 |

<a id="credits"></a>

## 🙏 致谢与第三方许可

- **[Slint](https://slint.dev/)**：本项目的界面框架，按 Slint 的 Royalty-free 许可证使用。

  <a href="https://slint.dev/"><img src="https://raw.githubusercontent.com/slint-ui/slint/master/logo/MadeWithSlint-logo-whitebg.png" alt="Made with Slint" height="48"></a>

- **[Everything SDK](https://www.voidtools.com/support/everything/sdk/)**（voidtools）：文件搜索通过动态加载 Everything 1.4 的 SDK 和 1.5 的 SDK3 的 x64 DLL 连接本机运行的 Everything。许可全文见 [third_party/everything/](third_party/everything/)。Everything 程序本身不随 Lanwork 分发，需要时请自行安装并运行。
- **[Unicode Unihan](https://www.unicode.org/charts/unihan.html)**：拼音匹配表由 Unicode 18.0.0 Unihan 的 `kMandarin` 和 `kHanyuPinyin` 读音生成，遵循 Unicode License v3，说明和许可见 [third_party/unihan/](third_party/unihan/)。
- 其余 Rust 依赖（如 `windows`、`serde` 等）各自遵循它们的开源许可，见各自的 crate 与 `Cargo.lock`。

<a id="license"></a>

## 📄 License

[MIT](LICENSE) © 2026 Wynn
