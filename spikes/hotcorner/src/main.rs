use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand, ValueEnum};

use hotcorner::{Corner, MonitorPick, RunOpts, SchemeKind};

#[derive(Parser)]
#[command(name = "hotcorner", about = "热角检测技术验证。不改系统设置。")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 打印显示器、DPI、低级钩子超时和前台窗口。不安装钩子。
    Probe {
        #[arg(long)]
        out: PathBuf,
    },
    /// 短时间移动光标，检查停留、离开和重新武装。结束时卸下钩子。
    SelfTest {
        #[arg(long)]
        out: PathBuf,
        /// 待定。产品规格没有写角的像素边长。
        #[arg(long)]
        corner_px: u32,
    },
    /// 运行一种检测，到时退出。不移动光标。
    Run(RunArgs),
}

#[derive(Parser)]
struct RunArgs {
    #[arg(long, value_enum)]
    scheme: SchemeArg,
    /// 方案 B 只接受 50 或 100。方案 A 不要传。
    #[arg(long)]
    interval_ms: Option<u64>,
    #[arg(long, value_enum, default_value_t = CornerArg::TopRight)]
    corner: CornerArg,
    /// 待定。产品规格没有写角的像素边长。
    #[arg(long)]
    corner_px: u32,
    /// 待定。产品规格没有写多显示器认哪块屏幕。
    #[arg(long, value_enum)]
    monitor: MonitorArg,
    /// 产品规格是 350。别的值只用于这次 spike，不是新的产品规则。
    #[arg(long, default_value_t = 350)]
    dwell_ms: u64,
    /// 到时进程退出并卸下钩子。
    #[arg(long)]
    duration_secs: u64,
    /// 每次触发追加一行 JSON。静止且不进角时不会写。
    #[arg(long)]
    events: Option<PathBuf>,
}

#[derive(Clone, Copy, ValueEnum)]
enum SchemeArg {
    Hook,
    Poll,
}

#[derive(Clone, Copy, ValueEnum)]
enum CornerArg {
    #[value(name = "top-left")]
    TopLeft,
    #[value(name = "top-right")]
    TopRight,
    #[value(name = "bottom-left")]
    BottomLeft,
    #[value(name = "bottom-right")]
    BottomRight,
    Off,
}

#[derive(Clone, Copy, ValueEnum)]
enum MonitorArg {
    /// 只认主显示器的角。待定。
    Primary,
    /// 认光标所在显示器的角。待定。
    Cursor,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let cli = Cli::try_parse().map_err(|err| err.to_string())?;
    match cli.command {
        Command::Probe { out } => hotcorner::plat::probe(&out),
        Command::SelfTest { out, corner_px } => hotcorner::plat::self_test(corner_px, &out),
        Command::Run(args) => {
            let opts = RunOpts {
                scheme: match args.scheme {
                    SchemeArg::Hook => SchemeKind::Hook,
                    SchemeArg::Poll => SchemeKind::Poll,
                },
                interval: args.interval_ms.map(Duration::from_millis),
                corner: match args.corner {
                    CornerArg::TopLeft => Corner::TopLeft,
                    CornerArg::TopRight => Corner::TopRight,
                    CornerArg::BottomLeft => Corner::BottomLeft,
                    CornerArg::BottomRight => Corner::BottomRight,
                    CornerArg::Off => Corner::Off,
                },
                corner_px: args.corner_px,
                pick: match args.monitor {
                    MonitorArg::Primary => MonitorPick::Primary,
                    MonitorArg::Cursor => MonitorPick::Cursor,
                },
                dwell: Duration::from_millis(args.dwell_ms),
                duration: Duration::from_secs(args.duration_secs),
                events: args.events,
            };
            hotcorner::plat::run(&opts)
        }
    }
}
