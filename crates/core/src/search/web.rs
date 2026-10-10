//! 网页搜索结果：搜索引擎、显示名和搜索页地址。

use serde::{Deserialize, Serialize};

/// 网页搜索用的搜索引擎。缺省 Google。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum WebSearchEngine {
    #[default]
    Google,
    Bing,
    Baidu,
}

impl WebSearchEngine {
    /// 结果行里的名称：「搜索 Google：…」「搜索必应：…」「搜索百度：…」。
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Google => "Google",
            Self::Bing => "必应",
            Self::Baidu => "百度",
        }
    }

    const fn base_url(self) -> &'static str {
        match self {
            Self::Google => "https://www.google.com/search?q=",
            Self::Bing => "https://www.bing.com/search?q=",
            Self::Baidu => "https://www.baidu.com/s?wd=",
        }
    }

    /// 搜索页地址。查询去掉首尾空白，按 UTF-8 百分号编码，只保留 RFC 3986 的非保留字符。
    #[must_use]
    pub fn search_url(self, query: &str) -> String {
        let mut url = self.base_url().to_owned();
        for byte in query.trim().bytes() {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
                url.push(char::from(byte));
            } else {
                url.push_str(&format!("%{byte:02X}"));
            }
        }
        url
    }

    /// 结果行的名称。
    #[must_use]
    pub fn label(self, query: &str) -> String {
        let name = self.display_name();
        let query = query.trim();
        if name.is_ascii() {
            format!("搜索 {name}：{query}")
        } else {
            format!("搜索{name}：{query}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::WebSearchEngine;

    #[test]
    fn urls_encode_the_trimmed_query_as_utf8() {
        assert_eq!(
            WebSearchEngine::Google.search_url("  rust slint "),
            "https://www.google.com/search?q=rust%20slint"
        );
        assert_eq!(
            WebSearchEngine::Bing.search_url("a&b=c#d+e/?"),
            "https://www.bing.com/search?q=a%26b%3Dc%23d%2Be%2F%3F"
        );
        assert_eq!(
            WebSearchEngine::Baidu.search_url("微信"),
            "https://www.baidu.com/s?wd=%E5%BE%AE%E4%BF%A1"
        );
        assert_eq!(
            WebSearchEngine::Google.search_url("A-z_0.9~"),
            "https://www.google.com/search?q=A-z_0.9~"
        );
    }

    #[test]
    fn labels_name_the_engine() {
        assert_eq!(WebSearchEngine::Google.label(" chat "), "搜索 Google：chat");
        assert_eq!(WebSearchEngine::Bing.label("chat"), "搜索必应：chat");
        assert_eq!(WebSearchEngine::Baidu.label("微信"), "搜索百度：微信");
    }

    #[test]
    fn default_is_google_and_names_are_lowercase_in_json() {
        assert_eq!(WebSearchEngine::default(), WebSearchEngine::Google);
        assert_eq!(
            serde_json::to_string(&WebSearchEngine::Baidu).unwrap(),
            "\"baidu\""
        );
        assert_eq!(
            serde_json::from_str::<WebSearchEngine>("\"bing\"").unwrap(),
            WebSearchEngine::Bing
        );
        assert!(serde_json::from_str::<WebSearchEngine>("\"duckduckgo\"").is_err());
    }
}
