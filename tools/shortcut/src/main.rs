//! 安装打包用的小程序：暂存发布文件，或写入开始菜单快捷方式属性。

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use lanwork_shortcut::{read_shortcut, stage, write_shortcut};

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let command = args.next();
    let result = match command.as_deref() {
        Some("stage") => two(&mut args).and_then(|(exe, dest)| stage(&exe, &dest)),
        Some("write") => two(&mut args).and_then(|(exe, lnk)| write_shortcut(&exe, &lnk)),
        Some("read") => one(&mut args).and_then(|lnk| read_shortcut(&lnk)),
        _ => Err(usage()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err}");
            ExitCode::from(1)
        }
    }
}

fn one(args: &mut impl Iterator<Item = String>) -> Result<PathBuf, String> {
    let path = args.next().ok_or_else(usage)?;
    if args.next().is_some() {
        return Err(usage());
    }
    Ok(PathBuf::from(path))
}

fn two(args: &mut impl Iterator<Item = String>) -> Result<(PathBuf, PathBuf), String> {
    let first = args.next().ok_or_else(usage)?;
    let second = args.next().ok_or_else(usage)?;
    if args.next().is_some() {
        return Err(usage());
    }
    Ok((PathBuf::from(first), PathBuf::from(second)))
}

fn usage() -> String {
    "用法: lanwork-shortcut stage <exe> <目录> | write <exe> <lnk> | read <lnk>".to_string()
}
