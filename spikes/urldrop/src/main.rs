//! `spikes/urldrop` 命令行。说明在 `spikes/urldrop/README.md`。

use std::process::ExitCode;

fn main() -> ExitCode {
    match parse_args() {
        Arg::Help => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
        Arg::SelfTest => match urldrop::self_test() {
            Ok(()) => {
                println!("urldrop: self-test ok");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("urldrop: {error}");
                ExitCode::from(1)
            }
        },
        Arg::Drop => match urldrop::run_drop() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("urldrop: {error}");
                ExitCode::from(1)
            }
        },
    }
}

const USAGE: &str = "\
用法:
  cargo run -p urldrop -- self-test
  cargo run -p urldrop -- drop

self-test 不打开窗口，用程序里准备的拖放数据检查解析。
drop 打开窗口，等你从 Chrome 或 Edge 把网址拖进来。
步骤写在 spikes/urldrop/README.md。";

enum Arg {
    Help,
    SelfTest,
    Drop,
}

fn parse_args() -> Arg {
    let mut args = std::env::args().skip(1);
    let Some(arg) = args.next() else {
        return Arg::Help;
    };
    if args.next().is_some() {
        return Arg::Help;
    }
    match arg.as_str() {
        "self-test" | "--self-test" => Arg::SelfTest,
        "drop" | "--drop" => Arg::Drop,
        _ => Arg::Help,
    }
}
