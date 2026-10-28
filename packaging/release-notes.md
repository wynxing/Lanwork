Lanwork __VERSION__

## 更新

自 v0.5.2 以来。

### 应用墙

搜索条右侧是应用墙按钮。点一下，在搜索条所在的那块显示器上铺满一块应用墙。墙上是开始菜单里的应用。开始菜单里收录的 Steam、Epic 游戏链接也列在墙上。设置页「隐藏的应用」里的应用不出现。没有可列出的应用时显示「没有应用」。

应用墙打开时，搜索条隐藏，右侧的应用墙按钮随搜索条一起离开屏幕，输入框里的文字留着。每次打开时，墙上的过滤是空的。过滤按名称匹配，别名也参与，包括拼音全拼和首字母。点在墙的空白处，焦点回到过滤框，应用墙留着。点在格子或过滤框上，应用墙留着。点在应用墙以外，搜索条和应用墙一起收起。

按 Esc，应用墙收起，搜索条重新出现在原来的位置，应用墙按钮在输入框右侧，焦点回到输入框，已输入的文字还在。输入法正在组字时，Esc 交给输入法。在应用墙上按 Alt+F4 同样收起应用墙，回到搜索条，Lanwork 仍在运行。应用墙没有单独的热键。

单击格子启动该应用。启动成功后，应用墙和搜索条一起收起。取消系统的确认对话框时不显示错误。启动失败时，错误显示在应用墙上，应用墙留着。右键菜单与搜索条里的应用结果相同：以其他用户身份运行、以管理员身份运行、打开文件所在文件夹、打开目标文件夹、隐藏。隐藏后这一条从应用墙和搜索结果里消失。设置里点「恢复」后，开着的应用墙按当前过滤重新列出。

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
