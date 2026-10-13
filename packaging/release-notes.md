Lanwork __VERSION__

## 更新

自 v0.1.0 以来。

### 不再追踪 GitHub

Lanwork 不再跟踪 GitHub 的 Pull Request 和 Issue。配置里以前留下的相关选项，打开时会被忽略，保存时不再写回去。来源记成 GitHub PR 或 Issue 的旧待办仍然可以打开。带有旧关注列表或缓存的备份包仍然可以导入。

### 热角

光标停在所选屏幕角上并连续停留 350 毫秒后，打开面板。每一块显示器上的这个角都会触发。光标所在的那块屏幕上有全屏窗口时，热角不会打开面板。在设置里可以把角改成左上、左下、右下，或关掉。关掉之后，搜索条热键仍然可用。

热角把面板叫到前面时，如果面板没能来到前台，面板会收起。

### 设置

设置页可以开关开机启动、选择热角、修改搜索条热键。改热键时按下新的组合即可。组合不合法、已被占用，或与面板热键相同时，原来的热键保持不变，并显示错误。

### 热键

搜索条热键和面板热键可以用更多键做主键：空格、方向键、标点符号、小键盘数字和运算符，以及 Home、End、PageUp、PageDown、Insert、Delete。除 F1 到 F24 可以单独使用外，其余主键都要带上 Ctrl、Alt、Shift、Win 中的至少一个。以前保存过、并且符合这些规则的热键照常使用。

热键被触发时，系统不区分小键盘和对应的导航键。NumLock 关闭时，存成小键盘数字的热键可能不会触发，存成方向键的热键也可能被小键盘按下。录入热键时可以区分这些键。

### 悬浮窗始终在最上层

便签悬浮窗和收纳悬浮窗始终在其他窗口之上。钉住按钮已去掉。以前钉住过的旧数据仍能打开，窗口同样在最上层。

### 贴边收成把手

把悬浮窗拖到某块屏幕外侧的左边、右边或上边，松开后收成贴在这条边上的细把手。把手厚 3、长 40，两端圆角，半透明灰色，放在贴住位置的正中。鼠标停在上面时更亮。能点击和悬停的范围就是这块把手。两块屏幕共用的边，以及底边，不会贴上。

点把手展开窗口；再点把手，或窗口失去焦点，就收回。把展开的窗口拖离这条边再松开，就取消贴边。贴住的边、沿边的位置，以及当前是展开还是收起，下次再打开时仍在。

### 显示桌面

按 Win+D 显示桌面之后，收起的悬浮窗仍是这块半透明把手。

### 叠页

把一个悬浮窗拖到另一个上面，重叠超过一半后松开，就合成一叠。便签和收纳可以叠在一起。一叠同一时刻只显示一页，各页共用位置和大小。

窗口有焦点、正文输入框没有焦点时，按左方向键或右方向键翻页。正文输入框有焦点时，这两个键留给文字。

展开并且至少有两页时，窗口下方正中有一排小圆点，一页一个，当前页更亮。点一个点就换到那一页。把某一个点拖出去，那一页就拆成独立的悬浮窗。拖标题栏只移动整叠。

整叠一起贴边。收起的把手上用小点表示有几页。两条已经收起的悬浮窗贴在同一条屏幕边上、彼此靠近时，会自动合成一叠。

重启之后，已经叠在一起的悬浮窗会再打开，仍是一叠，只显示上次那一页。收起时仍是一个把手，页点在把手上；展开时页点在窗口下方。记下的当前页缺失或重复时，打开第一页。页的序号有空档也不会少掉某一页。没有叠起来的悬浮窗不会因此被打开。没有叠页记录的旧文件照常打开。

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

这个安装包没有代码签名，也没有自动更新。
