# Grok 用量

登录文件是 `~/.grok/auth.json`。`key_prefix` 写成 `https://auth.x.ai`：顶层键等于这个前缀，或者后面紧跟 `/`、`:`，才算这一项。有多项时取字典序最小、字段都能读到、并且还没过期的那一个。

每个字段只能写一个 `key_prefix`。要改用 `https://accounts.x.ai/sign-in`，把字段和 `expires` 上的 `key_prefix` 都换成那个字符串。两条不能同时写上：写了的前缀都必须命中，否则整份是「登录已过期」。

`expires_at` 的格式并不统一。这个示例写成 `epoch_ms`（毫秒整数或整数字符串）。若本机文件里是 RFC3339 字符串，把 `format` 改成 `rfc3339`；若是秒，改成 `epoch_s`。格式对不上时窗口是「登录已过期」，请求不会发出。

重置时间优先用 `config.currentPeriod.end`。它按字符串提取，脚本接受 RFC3339 或数字字符串。解析不了时用 `config.billingPeriodEnd`（数字；大于等于 1e12 当作毫秒，大于等于 1e9 当作秒）。若 `currentPeriod.end` 在 JSON 里是数字而不是字符串，类型对不上，整份是「数据无效」；把该项的 `type` 改成 `number`。没有 `creditUsagePercent` 时用 `onDemandUsed` 和 `onDemandCap` 相除的算法，登录描述表达不了，这里不包含。
