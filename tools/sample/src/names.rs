use crate::ToolError;

pub fn exe_names_match(query: &str, exe_file: &str) -> bool {
    let query = strip_exe(base_name(query.trim()));
    let exe = strip_exe(base_name(exe_file.trim()));
    query.eq_ignore_ascii_case(exe)
}

pub fn counter_process_name(exe_file: &str) -> Result<String, ToolError> {
    let stem = strip_exe(base_name(exe_file.trim()));
    if stem.is_empty() {
        return Err(ToolError::new("进程名为空，无法构造唤醒计数器"));
    }
    if stem.contains(['*', '?', '(', ')']) {
        return Err(ToolError::new(format!(
            "进程名 {stem} 含有计数器通配符，无法构造 \\Thread 路径"
        )));
    }
    // 性能计数器的进程名最多 15 个字符。架构里的例子 `lanwork` 不受影响。
    let truncated: String = stem.chars().take(15).collect();
    Ok(truncated.to_ascii_lowercase())
}

pub fn context_switch_counter(process_name: &str) -> String {
    format!(r"\Thread({process_name}*)\Context Switches/sec")
}

pub fn id_process_counter(process_name: &str) -> String {
    format!(r"\Thread({process_name}*)\ID Process")
}

pub fn select_one_pid(label: &str, pids: &[u32]) -> Result<u32, ToolError> {
    match pids {
        [pid] => Ok(*pid),
        [] => Err(ToolError::new(format!("没有找到进程 {label}"))),
        many => {
            let list = many
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            Err(ToolError::new(format!(
                "进程名 {label} 匹配了 {} 个进程：{list}。请改用 --pid",
                many.len()
            )))
        }
    }
}

fn base_name(name: &str) -> &str {
    name.rsplit(['\\', '/']).next().unwrap_or(name)
}

fn strip_exe(name: &str) -> &str {
    if name.len() > 4 && name[name.len() - 4..].eq_ignore_ascii_case(".exe") {
        &name[..name.len() - 4]
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lanwork_counter_matches_the_architecture_path() {
        let name = counter_process_name(r"C:\Program Files\Lanwork\lanwork.exe").unwrap();
        assert_eq!(name, "lanwork");
        assert_eq!(
            context_switch_counter(&name),
            r"\Thread(lanwork*)\Context Switches/sec"
        );
        assert_eq!(id_process_counter(&name), r"\Thread(lanwork*)\ID Process");
    }

    #[test]
    fn exe_name_matching_ignores_directory_extension_and_case() {
        assert!(exe_names_match("lanwork", r"C:\App\Lanwork.exe"));
        assert!(exe_names_match("LANWORK.EXE", "lanwork.exe"));
        assert!(!exe_names_match("lanwork", "lanwork-helper.exe"));
    }

    #[test]
    fn long_process_names_are_truncated_to_the_counter_limit() {
        let name = counter_process_name("lanwork-sample-extra.exe").unwrap();
        assert_eq!(name.chars().count(), 15);
        assert_eq!(name, "lanwork-sample-");
    }

    #[test]
    fn select_one_pid_rejects_zero_and_many() {
        assert_eq!(select_one_pid("lanwork", &[7]).unwrap(), 7);
        assert!(select_one_pid("lanwork", &[]).is_err());
        let err = select_one_pid("lanwork", &[3, 9]).unwrap_err();
        assert!(err.to_string().contains("3"));
        assert!(err.to_string().contains("9"));
        assert!(err.to_string().contains("--pid"));
    }
}
