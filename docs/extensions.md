# 应用页

2026-10-15 补充决定：原来的扩展页并入悬浮窗工具方案，页面统一叫「应用」。可见行为以 [tools.md](tools.md) 为准，开发说明见 [tool-dev-guide.md](tool-dev-guide.md)。番茄钟是用该格式写成的内置工具，计时规则以 [product.md](product.md)「番茄钟」为准。

面板标签是待办、便签、收纳、应用、设置。没有「扩展」标签。不再规定另一套不带脚本的声明式插件，也不再把番茄钟写成官方插件。

产品界面还没有代码。宿主在 `crates/toolhost`，没有界面。技术验证「工具宿主」在 [roadmap.md](roadmap.md)，2026-10-15 通过。窗口验证程序在 `spikes/toolhost`。凭据管理器界面待用户用 `lanwork --toolhost-cred` 确认。
