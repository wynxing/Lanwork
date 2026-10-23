Lanwork __VERSION__

## 更新

自 v0.5.1 以来。

### 应用页

应用页把已安装的工具排成一列扁列表。每一行从左到右是图标、名称、一行短说明、「悬浮」、启用开关和「⋯」。「导入+」是同样高度的最后一行。点它，或把文件夹、zip 拖到应用页上，开始导入。列表为空时，这一页就是「导入+」这一行。

图标用工具文件夹里的 `icon.png`。文件读得出并且内容是 PNG 时用这张图；其余情况用名称去掉首尾空白后的第一个字。

「⋯」里是「更新」「卸载」「查看权限」。清单里声明了密钥时，再加「填写密钥」。

「悬浮」打开该工具的窗口。窗口已经开着时再点，焦点回到这一扇。启用开关关掉时，这一扇马上关闭，「悬浮」保持不可用，重启后开关仍是关闭。

### 工具悬浮窗

工具悬浮窗和便签、收纳、待办共用贴边和叠页。拖到其中一扇上，松手时重叠超过一半，就合成一叠，各页共用位置和大小。展开并且至少两页时，窗口下方靠左有一排彩色圆点。左右方向键翻页。工具内容占用这两个键时，键留给内容。

展开时，拖动条下面一行是「应用 · 工具名」。工具名取清单名称里第一个去掉首尾空白后还有字的行。整段名称都是空白时，这一行是「应用」。来源行在展开时显示，收成把手时只留下把手。

钉住只出现在工具悬浮窗上。开着时在其他普通窗口之上，关掉时是普通窗口层级，默认开着。关掉钉住并且这扇窗口展开在一叠里时，彩色圆点和这扇窗口同一层，别的窗口到前面时圆点留在这扇窗口旁边。

位置、大小、贴边、钉住和叠页记在数据目录的 `tool-windows/<工具 id>.json`，并进入导出和备份。同一工具再导入并确认后，这些和存储、密钥留在原处；窗口开着时，就在原来的位置用新副本重载。退出时窗口开着、启用开关也开着、并且不在叠里时，下次启动再打开。已经叠在一起时，启动打开上次那一页。

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
