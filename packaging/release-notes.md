Lanwork __VERSION__

## 更新

自 v0.5.0 以来。

### 搜索条的选中

搜索条打开时选中第一条候选。输入变化后，结果刷新时选中项是第一条候选。同一段输入后面到达的结果保留已经选中的那一条；那一条已经不在列表里时回到第一条。本机路径这一组接到最前面时，选中项改为这一组的第一条。

指针在这次打开之后、并且在最近一次结果刷新之后移动过，悬停在一条结果上才选中该条。指针停在原处，或结果出现在静止的指针下面，选中项保持当前这一条。单击直接打开。

### 本机路径

输入以盘符、冒号和斜杠开头时，按本机路径列出该文件夹的一层，例如 `C:\`、`d:/`。以 `\\`、`~\`、`~/` 或 `%变量名%` 开头的输入同样列出一层。`~` 展开成用户文件夹。输入里每一处 `%变量名%` 展开成对应的系统环境变量。`/` 按 `\` 处理。`a:b` 和 `C:` 按普通文字搜索。

这一层文件夹在前、文件在后。最后一个分隔符之后的文字按名称过滤，顺序是完全匹配、前缀、全拼、首字母、子串、模糊；同一种命中里，这次输入的次数高的在前，次数相同再按总次数。读目录时略过隐藏项和系统项。过滤、排序之后保留前 200 条。1.5 秒到点时，这一组显示一行「读取超时」。展开后的路径是已经存在的文件时，第一条名称是「打开这个文件」。

当前输入是本机路径时，Tab 把选中的文件夹补进输入框，末尾带 `\`，并列出下一层。Ctrl+Backspace 把输入换成当前列出的文件夹的上一层；在盘符根或 UNC 共享根时，输入保持不变。路径指向文件时，Ctrl+Backspace 列出该文件所在的文件夹。

### 使用次数

从搜索结果打开一项后，该项的总使用次数加 1，同时这次输入对应的次数加 1。同一种命中里，这次输入的次数高的排在前面，次数相同再按总次数。

次数写在数据目录的 `search-usage.json`，带 `schemaVersion`。每条记录保存条目的键（id 或路径）。按输入分开的次数以规范化查询的 SHA-256 为键。规范化是去掉首尾空白并转成小写。启动时读入。次数变化后约 30 秒写入一次，退出时把内存里待写入的次数再写一次。这份文件进入导出、备份和导入，从备份恢复时写回。

缺少 `schemaVersion` 时按 1 读取。`schemaVersion` 为 1 的文件按记录写入备份。`schemaVersion` 不是 1 时，备份按原字节带上这份文件。文件无法解析成 JSON 时，它留在数据目录，其余数据照常写入备份。

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
