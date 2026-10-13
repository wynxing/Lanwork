//! 网址拖放技术验证。
//!
//! 不进 `lanwork` 的依赖，也不进发布包。不写收纳界面。
//! 运行说明在 `spikes/urldrop/README.md`。

mod parse;

#[cfg(windows)]
mod ole;
#[cfg(windows)]
mod window;

pub use parse::session_header;

pub fn self_test() -> Result<(), String> {
    parse::check_parsers()?;
    #[cfg(windows)]
    ole::check_synthetic_drop()?;
    #[cfg(not(windows))]
    eprintln!("urldrop: com=skipped-not-windows");
    #[cfg(windows)]
    eprintln!("urldrop: com=checked");
    Ok(())
}

pub fn run_drop() -> Result<(), String> {
    #[cfg(windows)]
    {
        window::run()
    }
    #[cfg(not(windows))]
    {
        Err("urldrop drop 只在 Windows 上运行".to_string())
    }
}
