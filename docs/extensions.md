# 应用页

可见行为以 [tools.md](tools.md) 为准，开发说明见 [tool-dev-guide.md](tool-dev-guide.md)。

面板标签是待办、便签、收纳、应用、设置。工具排成一列，「导入+」在最后。

清单里出现 `usage` 时不安装。用量来源写在清单的 `[[login]]` 里。

应用页有代码，未在 Windows 上验收。导入、卸载、更新、查看权限和填写密钥有代码，未在 Windows 上验收。副本在数据目录的 `tools/<工具 id>/`，路径仍待定。宿主在 `crates/toolhost`。技术验证「工具宿主」在 [roadmap.md](roadmap.md)，2026-10-15 通过。窗口验证程序在 `spikes/toolhost`。凭据管理器界面待用户用 `lanwork --toolhost-cred` 确认。
