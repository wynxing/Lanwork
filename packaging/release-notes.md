Lanwork __VERSION__

## 更新

自 v0.5.6 以来。

### 悬浮窗

便签、收纳、待办和工具的悬浮窗，下次启动打开上次留在屏幕上的那一组。收成把手也算留着。关掉的窗口不再打开。叠页里其他页还记着，也不会把已经关掉的这一叠再打开。单独打开的窗口同样会回来。打开或关掉时马上记下。

从旧版升级后，便签、收纳和待办如果还没有记下是否留在屏幕上，要先手动打开一次，之后才会按留下的窗口恢复。工具仍按原来是否开着来恢复，不用全部重新打开。

### 工具刷新

工具按钮刷新时界面不卡住，可以继续拖动和关闭。结果按请求发出的顺序回来。窗口已经关掉、卸载或重载之后，迟到的结果不再写入。

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
