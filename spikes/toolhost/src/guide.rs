//! 从开发说明里取出示例的清单和入口脚本。验证跑的就是这份文本。

pub const GUIDE: &str = include_str!("../../../docs/tool-dev-guide.md");

pub fn example(heading: &str) -> Result<(String, String), String> {
    example_in(GUIDE, heading)
}

fn example_in(guide: &str, heading: &str) -> Result<(String, String), String> {
    // Windows 检出可能把说明转成 CRLF。代码块标记按 LF 查找。
    let guide = guide.replace("\r\n", "\n");
    let start = guide
        .find(heading)
        .ok_or_else(|| format!("开发说明里没有 {heading}"))?;
    let rest = &guide[start + heading.len()..];
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

#[cfg(test)]
mod tests {
    use super::{GUIDE, example_in};

    #[test]
    fn crlf_guide_still_yields_pomodoro() {
        let crlf = GUIDE.replace("\r\n", "\n").replace('\n', "\r\n");
        let (manifest, script) = example_in(&crlf, "## 示例：番茄钟").expect("番茄钟示例");
        assert!(manifest.contains("id = \"lanwork.pomodoro\""));
        assert!(script.contains("function on_timer"));
        assert!(!manifest.contains('\r'));
        assert!(!script.contains('\r'));
    }
}
