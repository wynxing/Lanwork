//! 从开发说明里取出示例的清单和入口脚本。验证跑的就是这份文本。

pub const GUIDE: &str = include_str!("../../../docs/tool-dev-guide.md");

pub fn example(heading: &str) -> Result<(String, String), String> {
    let start = GUIDE
        .find(heading)
        .ok_or_else(|| format!("开发说明里没有 {heading}"))?;
    let rest = &GUIDE[start + heading.len()..];
    let end = rest.find("\n## ").unwrap_or(rest.len());
    let section = &rest[..end];
    let manifest = fence(section, "toml")?;
    let script = fence(section, "lua")?;
    Ok((manifest, script))
}

fn fence(section: &str, lang: &str) -> Result<String, String> {
    let marker = format!("```{lang}\n");
    let start = section
        .find(&marker)
        .ok_or_else(|| format!("缺少 {lang} 代码块"))?;
    let body = &section[start + marker.len()..];
    let end = body
        .find("\n```")
        .ok_or_else(|| format!("{lang} 代码块没有结束"))?;
    Ok(body[..end].trim_end_matches('\n').to_string())
}

pub fn pomodoro() -> Result<(String, String), String> {
    example("## 示例：番茄钟")
}

pub fn quota() -> Result<(String, String), String> {
    example("## 示例：额度")
}

pub fn write_example(dir: &std::path::Path, which: &str) -> Result<(), String> {
    let (manifest, script) = match which {
        "pomodoro" => pomodoro()?,
        "quota" => quota()?,
        _ => return Err(format!("没有这个示例：{which}")),
    };
    crate::manifest::materialize(dir, &manifest, &script)
}
