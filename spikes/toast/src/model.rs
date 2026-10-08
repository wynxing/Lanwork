//! 与 Win32 无关的注册判定、通知 XML 和模拟列表定位。

pub const AUMID: &str = "Lanwork.Spike.Toast";
pub const ACTIVATOR_CLSID_U128: u128 = 0xDEC1_C43B_2AAC_400F_A0B1_C17A_05F2_B409;
pub const ACTIVATOR_CLSID: &str = "{DEC1C43B-2AAC-400F-A0B1-C17A05F2B409}";
pub const DISPLAY_NAME: &str = "Lanwork 通知验证";
pub const AUMID_KEY: &str = r"Software\Classes\AppUserModelId\Lanwork.Spike.Toast";
pub const CLSID_KEY: &str = r"Software\Classes\CLSID\{DEC1C43B-2AAC-400F-A0B1-C17A05F2B409}";
pub const LOCAL_SERVER_KEY: &str =
    r"Software\Classes\CLSID\{DEC1C43B-2AAC-400F-A0B1-C17A05F2B409}\LocalServer32";
pub const SHORTCUT_FILE_NAME: &str = "Lanwork Toast Spike.lnk";

pub const SAMPLE_TODOS: &[SampleTodo] = &[
    SampleTodo {
        id: "todo-001",
        title: "买牛奶",
    },
    SampleTodo {
        id: "todo-002",
        title: "写周报",
    },
    SampleTodo {
        id: "todo-003",
        title: "交电费",
    },
    SampleTodo {
        id: "todo-004",
        title: "回复邮件",
    },
    SampleTodo {
        id: "todo-005",
        title: "整理桌面",
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SampleTodo {
    pub id: &'static str,
    pub title: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationKind {
    Missing,
    Portable,
    Installed,
    Partial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// 注册缺失时不调用通知 API。
    TrayOnly,
    Toast,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationFacts {
    pub has_aumid_key: bool,
    pub display_name: Option<String>,
    pub custom_activator: Option<String>,
    pub local_server: Option<String>,
    pub shortcut_present: bool,
    pub shortcut_aumid: Option<String>,
    pub shortcut_activator: Option<String>,
    pub exe_path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Locate {
    Waiting,
    Selected(usize),
    Missing,
}

pub fn kind_label(kind: RegistrationKind) -> &'static str {
    match kind {
        RegistrationKind::Missing => "未注册",
        RegistrationKind::Portable => "便携版",
        RegistrationKind::Installed => "安装版",
        RegistrationKind::Partial => "部分注册",
    }
}

pub fn sample_title(id: &str) -> Option<&'static str> {
    locate_index(id).map(|index| SAMPLE_TODOS[index].title)
}

pub fn locate_index(id: &str) -> Option<usize> {
    let id = id.trim();
    if id.is_empty() {
        return None;
    }
    SAMPLE_TODOS.iter().position(|item| item.id == id)
}

/// 收到 launch 参数之后才算定位。空参数保持未定位，未知 id 不选中任何行。
pub fn locate_result(launch: Option<&str>) -> Locate {
    let Some(launch) = launch.map(str::trim).filter(|text| !text.is_empty()) else {
        return Locate::Waiting;
    };
    match locate_index(launch) {
        Some(index) => Locate::Selected(index),
        None => Locate::Missing,
    }
}

pub fn status_line(launch: Option<&str>) -> String {
    match locate_result(launch) {
        Locate::Waiting => "尚未收到 launch 参数。关闭窗口即退出。".to_string(),
        Locate::Selected(index) => {
            let item = &SAMPLE_TODOS[index];
            format!("launch={0}　已定位：{0} {1}", item.id, item.title)
        }
        Locate::Missing => {
            let launch = launch.unwrap_or("").trim();
            format!("launch={launch}　没有这条，未选中任何行")
        }
    }
}

pub fn window_title(launch: Option<&str>) -> String {
    match locate_result(launch) {
        Locate::Waiting => "Lanwork 通知验证 · 模拟面板".to_string(),
        Locate::Selected(index) => {
            format!("Lanwork 通知验证 · 已定位 {}", SAMPLE_TODOS[index].id)
        }
        Locate::Missing => "Lanwork 通知验证 · 未定位".to_string(),
    }
}

pub fn delivery(kind: RegistrationKind) -> Delivery {
    match kind {
        RegistrationKind::Missing => Delivery::TrayOnly,
        RegistrationKind::Portable | RegistrationKind::Installed | RegistrationKind::Partial => {
            Delivery::Toast
        }
    }
}

pub fn classify(facts: &RegistrationFacts) -> RegistrationKind {
    let shortcut_ok = facts.shortcut_present
        && facts.shortcut_aumid.as_deref() == Some(AUMID)
        && facts
            .shortcut_activator
            .as_deref()
            .is_some_and(|value| same_guid(value, ACTIVATOR_CLSID));
    let com_ok = facts
        .custom_activator
        .as_deref()
        .is_some_and(|value| same_guid(value, ACTIVATOR_CLSID))
        && facts
            .local_server
            .as_deref()
            .is_some_and(|value| command_contains_path(value, &facts.exe_path));
    let portable_ok = facts.has_aumid_key
        && facts.display_name.as_deref() == Some(DISPLAY_NAME)
        && facts.custom_activator.is_none()
        && facts.local_server.is_none()
        && !facts.shortcut_present;
    let missing = !facts.has_aumid_key
        && facts.custom_activator.is_none()
        && facts.local_server.is_none()
        && !facts.shortcut_present;

    if shortcut_ok && com_ok && facts.display_name.as_deref() == Some(DISPLAY_NAME) {
        RegistrationKind::Installed
    } else if portable_ok {
        RegistrationKind::Portable
    } else if missing {
        RegistrationKind::Missing
    } else {
        RegistrationKind::Partial
    }
}

pub fn toast_xml(id: &str, heading: &str, body: &str) -> String {
    format!(
        "<toast launch=\"{launch}\" activationType=\"foreground\"><visual><binding template=\"ToastGeneric\"><text>{heading}</text><text>{body}</text></binding></visual></toast>",
        launch = xml_escape(id),
        heading = xml_escape(heading),
        body = xml_escape(body),
    )
}

pub fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

pub fn same_guid(left: &str, right: &str) -> bool {
    normalize_guid(left) == normalize_guid(right)
}

pub fn normalize_guid(text: &str) -> String {
    let hex: String = text.chars().filter(|ch| ch.is_ascii_hexdigit()).collect();
    if hex.len() != 32 {
        return text.trim().to_ascii_uppercase();
    }
    let hex = hex.to_ascii_uppercase();
    format!(
        "{{{a}-{b}-{c}-{d}-{e}}}",
        a = &hex[0..8],
        b = &hex[8..12],
        c = &hex[12..16],
        d = &hex[16..20],
        e = &hex[20..32],
    )
}

pub fn command_contains_path(command: &str, exe_path: &str) -> bool {
    if exe_path.is_empty() {
        return false;
    }
    strip_quotes(command)
        .to_ascii_lowercase()
        .contains(&exe_path.to_ascii_lowercase())
}

fn strip_quotes(text: &str) -> String {
    text.replace('"', "")
}

pub fn format_facts(facts: &RegistrationFacts) -> String {
    format!(
        "判定：{kind}\nAUMID 键：{aumid}\nDisplayName：{display}\nCustomActivator：{activator}\nLocalServer32：{server}\n快捷方式：{shortcut}\n快捷方式 AUMID：{shortcut_aumid}\n快捷方式 CLSID：{shortcut_clsid}",
        kind = kind_label(classify(facts)),
        aumid = yes_no(facts.has_aumid_key),
        display = facts.display_name.as_deref().unwrap_or("（无）"),
        activator = facts.custom_activator.as_deref().unwrap_or("（无）"),
        server = facts.local_server.as_deref().unwrap_or("（无）"),
        shortcut = yes_no(facts.shortcut_present),
        shortcut_aumid = facts.shortcut_aumid.as_deref().unwrap_or("（无）"),
        shortcut_clsid = facts.shortcut_activator.as_deref().unwrap_or("（无）"),
    )
}

fn yes_no(value: bool) -> &'static str {
    if value { "有" } else { "无" }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exe() -> String {
        r"E:\work\toast.exe".to_string()
    }

    fn empty() -> RegistrationFacts {
        RegistrationFacts {
            has_aumid_key: false,
            display_name: None,
            custom_activator: None,
            local_server: None,
            shortcut_present: false,
            shortcut_aumid: None,
            shortcut_activator: None,
            exe_path: exe(),
        }
    }

    #[test]
    fn missing_registration_uses_tray_only() {
        let facts = empty();
        assert_eq!(classify(&facts), RegistrationKind::Missing);
        assert_eq!(delivery(RegistrationKind::Missing), Delivery::TrayOnly);
    }

    #[test]
    fn portable_is_only_the_aumid_key() {
        let mut facts = empty();
        facts.has_aumid_key = true;
        facts.display_name = Some(DISPLAY_NAME.to_string());
        assert_eq!(classify(&facts), RegistrationKind::Portable);
        assert_eq!(delivery(RegistrationKind::Portable), Delivery::Toast);
    }

    #[test]
    fn portable_with_shortcut_is_not_portable() {
        let mut facts = empty();
        facts.has_aumid_key = true;
        facts.display_name = Some(DISPLAY_NAME.to_string());
        facts.shortcut_present = true;
        assert_eq!(classify(&facts), RegistrationKind::Partial);
    }

    #[test]
    fn installed_requires_shortcut_and_com_server() {
        let mut facts = empty();
        facts.has_aumid_key = true;
        facts.display_name = Some(DISPLAY_NAME.to_string());
        facts.custom_activator = Some(ACTIVATOR_CLSID.to_string());
        facts.local_server = Some(format!("\"{}\"", exe()));
        facts.shortcut_present = true;
        facts.shortcut_aumid = Some(AUMID.to_string());
        facts.shortcut_activator = Some(ACTIVATOR_CLSID.to_ascii_lowercase());
        assert_eq!(classify(&facts), RegistrationKind::Installed);
    }

    #[test]
    fn installed_without_local_server_is_partial() {
        let mut facts = empty();
        facts.has_aumid_key = true;
        facts.display_name = Some(DISPLAY_NAME.to_string());
        facts.custom_activator = Some(ACTIVATOR_CLSID.to_string());
        facts.shortcut_present = true;
        facts.shortcut_aumid = Some(AUMID.to_string());
        facts.shortcut_activator = Some(ACTIVATOR_CLSID.to_string());
        assert_eq!(classify(&facts), RegistrationKind::Partial);
    }

    #[test]
    fn xml_carries_escaped_launch_id() {
        let xml = toast_xml("todo-001", "待办到期", "todo-001 买牛奶");
        assert!(xml.contains("launch=\"todo-001\""));
        assert!(xml.contains("activationType=\"foreground\""));
        let quoted = toast_xml("a\"b&c", "标题", "正文");
        assert!(quoted.contains("launch=\"a&quot;b&amp;c\""));
        assert!(!quoted.contains("launch=\"a\"b&c\""));
    }

    #[test]
    fn locate_selects_only_a_known_id() {
        assert_eq!(locate_result(None), Locate::Waiting);
        assert_eq!(locate_result(Some("  ")), Locate::Waiting);
        assert_eq!(locate_result(Some("todo-003")), Locate::Selected(2));
        assert_eq!(locate_result(Some("missing")), Locate::Missing);
        assert!(status_line(Some("todo-001")).contains("已定位"));
        assert!(status_line(Some("missing")).contains("未选中任何行"));
        assert_eq!(sample_title("todo-005"), Some("整理桌面"));
    }

    #[test]
    fn guid_normalization_ignores_braces_and_case() {
        assert!(same_guid(
            ACTIVATOR_CLSID,
            "dec1c43b-2aac-400f-a0b1-c17a05f2b409"
        ));
        assert_eq!(
            normalize_guid("DEC1C43B2AAC400FA0B1C17A05F2B409"),
            ACTIVATOR_CLSID
        );
    }
}
