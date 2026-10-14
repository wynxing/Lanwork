Lanwork __VERSION__

## 更新

自 v0.3.0 以来。

### 检查更新

0.3.0 在后台查看 GitHub 接口上的最新正式版。未登录时这个接口有次数限制，用尽后返回 403，设置页显示「检查更新失败」。

这一版改为读取最新正式版上的 `latest.json`，地址是 `https://github.com/wynxing/Lanwork/releases/latest/download/latest.json`，不再请求那个接口。这份文件给出稳定版本、安装包文件名和 SHA-256。有发行说明时，设置页显示这段说明。已经发布的 v0.3.0 没有这段说明，检查仍能判断有没有更新。

安装版点「更新」仍下载对应的 `Lanwork-Setup-<版本>.exe`，用这份 `latest.json` 核对 SHA-256，通过之后才退出，静默安装，再启动 Lanwork。安装不修改用户数据。校验失败、下载失败或安装起不来时，不替换当前程序，并显示错误。便携版只显示「打开发布页」。网络失败时显示「检查更新失败」，不安装。

### 悬浮窗来源行

展开的便签、收纳和待办悬浮窗，在拖动条下面单独一行显示这一页的来源。这一行比正文小、颜色更淡，按下不拖动窗口。一行放不下时末尾省略号，不换行。收成把手时不显示。

便签是「便签 · 名称」。名称先取标题里第一个有字的行，没有则取正文里第一个有字的行，都没有则是「无标题」。在悬浮窗里改正文，这一行跟着变。

收纳是「收纳 · 分组名」。名称取分组名里第一个有字的行。改名后已经打开的窗口马上换成新名字。分组不在，或名称没有可见字符时，只显示「收纳」。下一行仍是可折叠的「关联待办」，再下面是条目。

待办这一行就是「待办」，没有某一条的标题。

叠在一起时，看得见的那一页显示自己的来源。翻到另一页后换成那一页的来源。页点圆点在内容区外面，不显示来源行。拖动、关闭、最上层、贴边和页点仍在原来的位置。

### 失焦后仍是磨砂

透明效果可用时，便签、收纳和待办悬浮窗在有焦点和失去焦点时是同一种半透明磨砂。叠在一起时，看得见的那一页同样如此。透明效果关闭、节电模式，或这种背景不可用时，仍是与主题一致的纯色，失焦也不换成另一种颜色。收起的把手仍是半透明灰色。页点圆点保持透明，失焦不铺上不透明的底。

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
