# 视觉刷新实现计划

基线是 `main` 的 `3731b90b694438a4edf02d6971458c2d60986838`。界面仍是 Rust + Slint 1.18.1。不迁 GPUI，不引入 WebView 或 HTML 运行时。不升版本，不发 release，不打 tag，不改更新源。

## 范围

改面板左侧标签、纯色回退和各页共用的配色。搜索、待办、便签、收纳、应用、设置、悬浮窗和搜索条沿用同一套颜色。领域逻辑、排序、热键、拖放、Lua 和插件不改。

贴边把手仍是 3×40 逻辑像素、没有图标。叠页仍是规格里的彩色圆点，点圆点换页，拖出圆点拆页。稿里的页名和加宽把手不进这一版：那两处会改掉已经写明的命中范围和行高。搜索结果仍显示类型。悬浮窗来源行仍是原来的文字。

## 文件

- `crates/core/src/shell/bar.rs`：纯色和强调策略着色。
- `crates/app/ui/widgets.slint`：共用配色。
- `crates/app/ui/panel.slint`：左侧图标标签。
- `crates/app/ui/searchbar.slint`、`appwall.slint`、`float_shell.slint`、`removal.slint`，以及各窗口的纯色默认值。
- `crates/app/src/tab_rail_tests.rs`：标签点击、悬停名称、键盘焦点。
- `crates/app/src/notes_layout_tests.rs`、`settings_layout_tests.rs`、`triage_layout_tests.rs`、`shelves_layout_tests.rs`、`apps_layout_tests.rs`：内容区左移后的点击坐标。
- `docs/product.md`、`docs/architecture.md`：只改已经变了的可见内容和纯色值。

不改 `spikes/`。

## 任务

1. 在 `tab_rail_tests` 里写失败测试：五个标签可点并回调下标；没有悬停和焦点时名称为空；悬停或 `focus-tabs` 后名称与 `tab-titles` 一致；从面板焦点按 Tab 进入当前标签；上下键只移动名称，Enter 才切换；Esc 仍收起。热区宽高至少 40。
2. 跑这个测试，确认先失败。
3. `solid_rgb` 浅色改为 `#FBF6EE`，暗色改为 `#1C1916`。强调策略着色是 `0xAABBGGRR`：浅色 `0x48EEF6FB`，暗色 `0x4816191C`。透明度仍是 `0x48`。测试按通道比对红、绿、蓝，不要求低 24 位等于 RGB 数值。
4. `LanworkTheme.palette` 提供暖灰和柿色。逾期和错误仍用浅色 `#c42b1c`、暗色 `#ff99a4`。面板、悬浮窗、搜索条、应用墙、卸载清单都用这一套。
5. 面板左侧宽 64。标签只有图标，当前项用浅底、柿色图标和短标记。名称不写进侧栏。悬停片刻或键盘焦点时，短提示浮在按钮旁，不改变栏宽，也不另开窗口。图标保持居中。顺序仍是待办、便签、收纳、应用、设置。空白处拖动窗口。关闭仍收起。内容从 x=64、y=0 开始。收纳放下区域跟着内容区走。侧栏上的 Esc 走面板同一层退出顺序。
6. 搜索里的便签图形改成两张叠页，文件仍是右上折角的竖页。类型文字保留。
7. 修正布局测试里写死的窗口坐标，使原有点击仍落在原来的控件上。
8. `cargo fmt`、`cargo clippy --all-targets -- -D warnings`、`cargo test`。Windows 安装包和便携包交给现有 CI 的 package 作业，不在 Linux 上宣称 Windows 效果已经看过。

## 验收

- 标签点击、悬停名称、键盘焦点、Enter 切换、Esc 收起有测试。
- 纯色值有测试。强调策略着色按 `0xAABBGGRR` 解出的红、绿、蓝与纯色相同。
- 原有布局测试通过。
- 把手尺寸、叠页圆点、搜索类型、来源行的行为测试不改断言含义。
