//! 在 Windows 桌面会话里比较几种置前办法。
//!
//! 产品路径只用「先直接置前，不行再送一次相对位移 0 的鼠标移动，等它被取走后再置前」。
//! 这里把那一套和不等待的对照、绝对坐标，以及可选的 Alt 抬起、`AttachThreadInput` 分开跑。
//! 不代替 Windows 验收。

use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    match lanwork_foreground::parse(env::args().skip(1)) {
        Ok(lanwork_foreground::Args::Help) => {
            println!("{}", lanwork_foreground::USAGE);
            ExitCode::SUCCESS
        }
        Ok(lanwork_foreground::Args::Run(methods)) => run(&methods),
        Err(err) => {
            eprintln!("{err}");
            ExitCode::from(1)
        }
    }
}

#[cfg(windows)]
fn run(methods: &[lanwork_foreground::ProbeMethod]) -> ExitCode {
    probe::run(methods)
}

#[cfg(not(windows))]
fn run(_methods: &[lanwork_foreground::ProbeMethod]) -> ExitCode {
    eprintln!("lanwork-foreground 只在 Windows 桌面会话里运行。");
    ExitCode::from(1)
}

#[cfg(windows)]
mod probe;
