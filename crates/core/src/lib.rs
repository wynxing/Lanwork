//! 模型、服务与存储。
//!
//! 不依赖 Slint，也不依赖 Win32 窗口 API，以便在 CI 上直接测试。
//! 存储规则在 [`storage`] 模块。待办、便签、收纳等服务随后续 issue 加进来。

pub mod storage;

#[cfg(test)]
mod tests {
    #[test]
    fn package_name() {
        let name = env!("CARGO_PKG_NAME").to_owned();
        assert_eq!(name, "lanwork-core");
    }
}
