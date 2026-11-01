Lanwork __VERSION__

## 更新

自 v0.5.3 以来。

### 应用图标

应用图标取出之后留在本机缓存里，搜索条和应用墙用同一份。这份缓存可以整份删掉，之后会重新取。源文件变了就显示新图标。进程空闲，并且搜索条和应用墙都没在屏幕上时，后台预先取出当前这面墙第一屏的图标。打开应用墙时，第一屏从开头算，内存里放得下的图标直接显示。第一屏多出来的格子，以及后来滚进画面的格子，按当时看得到的范围再取。

### 搜索结果菜单

文件和文件夹结果的菜单是「打开所在文件夹」「复制路径」「复制文件」「加入收纳…」。便签结果的菜单是「悬浮」。网页引用的菜单是「复制网址」。应用结果的菜单是「以其他用户身份运行」「以管理员身份运行」「打开文件所在文件夹」「打开目标文件夹」「隐藏」。光标在已输入文字的末尾时，右方向键打开当前选中结果的菜单。组合输入期间，右方向键交给输入法。「复制文件」按资源管理器「复制」的格式放入剪贴板，在资源管理器里粘贴会复制这一条文件或文件夹。「加入收纳…」把这条路径引用放进所选分组，和把文件或文件夹拖进该分组相同。分组列表为空时，菜单里是「收纳」，选中后创建这个分组。

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
