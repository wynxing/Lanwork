//! 模型、服务与存储。
//!
//! 不依赖 Slint，也不依赖 Win32 窗口 API，以便在 CI 上直接测试。
//! 具体模块随后续功能加进来。

#[cfg(test)]
mod tests {
    #[test]
    fn package_name() {
        let name = env!("CARGO_PKG_NAME").to_owned();
        assert_eq!(name, "lanwork-core");
    }
}
