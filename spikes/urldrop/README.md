# 网址拖放

独立的最小程序，用来验证从 Chrome 或 Edge 拖进来的网址。不进 `lanwork` 的依赖，不进发布包，也不是收纳窗口。

它做的事：

- 接受放到窗口上的拖放。
- 从 `CF_HTML`（注册名 `HTML Format`）、`text/uri-list` 和纯文本 `CF_UNICODETEXT` 里取出网址。`CF_HTML` 里如果有标题，一并记下来。
- 同一次拖放里的虚拟 `.url`（`FileGroupDescriptor` / `FileContents`）只记下文件名。不读取 `FileContents`，不把文件写进磁盘，也不存成路径。
- 放下时只回复复制或快捷方式。对方只允许移动时，这次放下被拒绝。
- 不调用会关闭或移动浏览器标签的接口。

几种格式同时出现时用哪一个、纯文本怎样才算网址、`CF_TEXT` 和 `UniformResourceLocator` 收不收，产品规格还没定。程序把读到的都写进日志，不替规格做选择。

手测通过之前，收纳页面不识别网址。

## 在 Windows 上运行

在仓库根目录：

```text
cargo run -p urldrop -- self-test
cargo run -p urldrop -- drop
```

`self-test` 不打开窗口，也不用浏览器。它用程序里准备的拖放数据检查解析，并确认没有去读虚拟 `.url` 的内容。这不是手测通过记录。

`drop` 打开标题为「Lanwork 网址拖放验证」的窗口。需要本机已安装的 Chrome 和 Microsoft Edge。用调试构建即可，`cargo run` 的输出在这个终端里。也可以先 `cargo build -p urldrop`，再运行 `target\debug\urldrop.exe drop`。

日志文件是 `%TEMP%\lanwork-spike-urldrop.log`。每次启动 `drop` 都会重写这个文件。窗口里的文字和这个文件、终端输出是同一份。

## 手测

做的时候不要关原来的网页。Chrome 和 Edge 都要做。

1. 启动 `cargo run -p urldrop -- drop`，等窗口出现。
2. 用 Chrome 打开一个普通网页。按住地址栏左边的锁图标，拖进验证窗口，松开。没有锁、只有网站小图标或调音图标时，拖的就是那一个，它和锁在同一位置。
3. 看原来的标签还在不在，有没有被关掉，有没有换到别的位置。
4. 在同一个网页里按住一个链接，拖进验证窗口，松开。再看一次标签。
5. 换 Microsoft Edge，重复第 2 步到第 4 步。
6. 关掉验证窗口。
7. 把窗口里曾经显示的字全选复制，或者用记事本打开 `%TEMP%\lanwork-spike-urldrop.log`，把整段贴回去。

贴回去的内容里，每次拖放都有一段 `kind=drop`。里面会有网址、能读到的标题、对方提供了哪些格式，以及 `file-contents-read=false`。`self-test` 的输出不要当作这次手测结果。
