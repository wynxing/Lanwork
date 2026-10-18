# Claude 用量

Anthropic 的条款把订阅令牌限制在 Claude Code 和 claude.ai。这个示例读取本机 `~/.claude/.credentials.json` 里的令牌，去请求 `https://api.anthropic.com/api/oauth/usage`。使用由你自行承担风险。

`resets_at` 是 RFC3339 字符串。登录描述原样把它交给脚本，时刻由 `main.lua` 换算成 UTC。
