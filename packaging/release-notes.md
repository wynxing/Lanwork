Lanwork __VERSION__

## 更新

自 v0.5.5 以来。

### 界面

界面用 Rust 与 Slint 绘制。浅色和暗色是暖灰和柿色。

面板左侧五个标签是固定居中的图标，顺序仍是待办、便签、收纳、应用、设置。名称不写在侧栏里。指针停下片刻，或键盘焦点落在标签上时，名称出现在按钮旁边。提示不接收点击，离开或失焦后消失，并留在窗口内。图标在静止、选中、悬停和焦点时不移动、不缩小。

搜索条右侧的应用墙入口同样是固定居中的图标，名称是按钮左侧的短提示。点一下仍在所在显示器上铺满应用墙。

收纳页右侧是取用格。格子上是握点、图标或类型图形、名称。网页还没有图标时写「网页」，失效路径写「路径不存在」。分组的重命名、上移、下移、关联待办、悬浮和删除在「⋯」里。选中条目后，底部是路径或「已选 N 条」，以及「打开」「打开所在目录」「移除引用」。

搜索结果仍显示图标、名称和类型。名称里与输入相同的一段用柿色加粗。工具悬浮窗和便签、收纳、待办共用贴边和叠页。展开时来源行是「类型 · 名称」。工具窗口上的按钮写「置顶」。

收纳里按 Esc，先关分组「⋯」，再关右键菜单，再取消重命名，再撤回确认，再清选中，再退出分组多选，然后才收起面板。输入法正在组字时，Esc 交给输入法。

悬浮窗失焦后仍在的亚克力用这套暖色。浅色和暗色各自对应当前底色。

导航和紧凑的常用入口只用图标，名称用独立的短提示。正文、菜单项、搜索结果的类型和设置里的说明仍在。

## 安装版

主要下载：`Lanwork-Setup-__VERSION__.exe`

按当前用户安装，不需要管理员权限。不安装 WebView2，也不安装 Windows App Runtime。开始菜单里的 Lanwork 快捷方式带通知用的 AppUserModelID 和激活 CLSID。卸载会去掉这些注册项，以及开机启动里名为 Lanwork 的项，不会删除文档中的 Lanwork 数据目录。

## 便携版

附加下载：`Lanwork-__VERSION__-portable-x64.zip`

压缩包里是同一套程序文件，不含安装程序。

## 第三方许可

安装目录和便携版压缩包里有这些文件，仓库里也有原文：

- [Everything-SDK-LICENSE.txt](https://github.com/wynxing/Lanwork/blob/__TAG__/third_party/everything/sdk/LICENSE.txt)、[Everything-SDK3-LICENSE.txt](https://github.com/wynxing/Lanwork/blob/__TAG__/third_party/everything/sdk3/LICENSE.txt)：voidtools Everything SDK。可以免费使用、复制、修改、合并、发布、分发、再许可和销售，须保留版权声明和许可声明。
- [Unicode-LICENSE.txt](https://github.com/wynxing/Lanwork/blob/__TAG__/third_party/unihan/LICENSE.txt)：Unicode License v3。拼音数据来自 Unicode 18.0.0 Unihan。

## 界面

界面使用 Slint 1.18.1，许可证是 [Slint Royalty-free 2.0](https://github.com/slint-ui/slint/blob/v1.18.1/LICENSES/LicenseRef-Slint-Royalty-free-2.0.md)。

[![Made with Slint](https://github.com/wynxing/Lanwork/raw/__TAG__/third_party/slint/MadeWithSlint-logo-whitebg.png)](https://slint.dev)

[Slint](https://slint.dev)

这个安装包没有代码签名。设置里可以检查更新；不会自动下载，也不会自动安装。
