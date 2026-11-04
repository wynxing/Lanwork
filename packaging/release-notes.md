Lanwork __VERSION__

## 更新

自 v0.5.4 以来。

### 应用菜单

搜索条的应用结果和应用墙使用同一份菜单。「清理开始菜单入口」删掉这一条索引所用的开始菜单快捷方式或游戏链接。安装目录和卸载登记保持原样。没有可删的开始菜单入口时，这一项留在菜单里，不能点。删除成功后重新读取开始菜单索引，开着的应用墙和搜索结果按刷新后的索引列出。

「卸载…」先出现确认：标题是应用名称，正文是「会启动这个应用自己的卸载程序。」确认后启动这条卸载登记里的官方卸载程序，可以是卸载字符串、Windows Installer 产品或商店包。卸载字符串里的可执行文件名是 `Update.exe`（忽略大小写）时，参数里独立的 `--uninstall` 在启动前写成子命令 `uninstall`。没有可用的卸载命令时，这一项留在菜单里，不能点。

卸载进程结束后刷新索引。允许扫描时自动扫描和这个应用对应的残留，并打开清单。条目已经从对应索引里消失时扫描。条目仍在索引里、且卸载进程已成功退出时同样扫描。用户在系统对话框或卸载程序里取消、且这一条仍在索引里时，不扫描。卸载进程的退出码表示失败、且这一条仍在索引里时，显示「卸载没有完成」，不扫描。扫描之后没有可清理的残留时，显示「没有发现可清理的残留」，不打开空清单：从搜索条卸载时写在状态行，从应用墙卸载时写在应用墙上。确认后只删除勾选的项。

扫描这些位置：登记的安装目录；安装目录同一层里，名称以显示名开头、后面是「 Data」「Data」「 Cache」或「Cache」的文件夹；开始菜单和桌面上的快捷方式；用户主目录下一层里，名为「.」加上显示名或可执行文件主名的文件夹（忽略大小写；显示名去掉空格后也算；显示名分成几段时，第一段也算）；%AppData% 和 %LocalAppData% 的下一层里，名称等于显示名、显示名最后一段或可执行文件主名的文件夹，以及主名后面接「-updater」或「 updater」的文件夹；这条卸载登记，以及软件名下叶子名等于显示名、显示名最后一段或可执行文件主名的注册表键。清单里每一行是完整路径或完整注册表键。安装目录还在并且目标程序已经不在、快捷方式仍指向这个安装目录或目标程序、卸载子键还在，这些默认勾选。文件夹名与完整显示名一致，并且不直接放在 Program Files、Program Files (x86)、ProgramData、AppData、文档、桌面或用户主目录里，也默认勾选。

卸载登记的 `SystemComponent` 或 `NoRemove` 不是 0，路径落在 `Microsoft\Edge` 目录下，卸载命令带独立参数 `--msedge`，以及没有卸载信息且路径都在系统目录里时，「清理开始菜单入口」和「卸载…」留在菜单里，不能点。卸载登记的显示名去掉末尾版本号后，可以和开始菜单入口的名称对上。没有 `InstallLocation` 时，卸载程序所在的目录当作安装目录。卸载进程退出、启动失败，或结果没有回到界面之后，占用放开，其它应用的「卸载…」可以继续用。残留清单还开着时，占用留到清单关闭。

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
