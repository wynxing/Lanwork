use std::env;
use std::fs::File;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};

use lanwork_sample::{
    SampleRequest, SampleTarget, ToolError, parse_latency_jsonl, run_sample, summarize,
};

#[derive(Parser)]
#[command(
    name = "lanwork-sample",
    about = "按性能测量协议采样进程，或汇总延迟原始记录。"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 按 PID 或进程名每秒采样。进程中途退出时结束并保留已有 CSV。
    Sample(SampleArgs),
    /// 读取 JSONL 原始时间戳，排除预热和被取代的查询，计算 P95。
    Latency(LatencyArgs),
}

#[derive(Parser)]
struct SampleArgs {
    /// 进程 ID。与 --name 二选一。
    #[arg(long)]
    pid: Option<u32>,
    /// 进程名，例如 lanwork。匹配到多个进程时改用 --pid。
    #[arg(long)]
    name: Option<String>,
    /// 采样时长（秒）。协议里的空闲采样是 300。
    #[arg(long)]
    duration_secs: u64,
    /// 采样间隔（毫秒）。协议是 1000。
    #[arg(long, default_value_t = 1000)]
    interval_ms: u64,
    /// CSV 输出路径。
    #[arg(long)]
    out: PathBuf,
}

#[derive(Parser)]
struct LatencyArgs {
    /// JSONL 原始记录。
    #[arg(long)]
    input: PathBuf,
    /// 汇总 JSON。省略时写到标准输出。
    #[arg(long)]
    out: Option<PathBuf>,
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

fn run() -> Result<(), ToolError> {
    let cli = Cli::try_parse_from(env::args_os()).map_err(|err| ToolError::new(err.to_string()))?;
    match cli.command {
        Command::Sample(args) => sample(args),
        Command::Latency(args) => latency(args),
    }
}

fn sample(args: SampleArgs) -> Result<(), ToolError> {
    let target = match (args.pid, args.name) {
        (Some(pid), None) => SampleTarget::Pid(pid),
        (None, Some(name)) => SampleTarget::Name(name),
        _ => return Err(ToolError::new("必须指定 --pid 或 --name 中的一个")),
    };
    let run = run_sample(&SampleRequest {
        target,
        duration: Duration::from_secs(args.duration_secs),
        interval: Duration::from_millis(args.interval_ms),
        out: args.out,
        user_profile: env::var_os("USERPROFILE").map(PathBuf::from),
    })?;
    let stop = match run.stop {
        lanwork_sample::StopReason::DurationReached { .. } => "duration",
        lanwork_sample::StopReason::ProcessExited { .. } => "process_exited",
    };
    serde_json::to_writer_pretty(
        io::stdout(),
        &serde_json::json!({
            "samples": run.samples,
            "stop": stop,
            "out": run.out,
            "pid": run.pid,
            "counter": run.counter,
        }),
    )?;
    println!();
    Ok(())
}

fn latency(args: LatencyArgs) -> Result<(), ToolError> {
    let text = std::fs::read_to_string(&args.input)?;
    let records = parse_latency_jsonl(&text)?;
    let report = summarize(&records)?;
    if let Some(path) = args.out {
        let mut file = File::create(path)?;
        serde_json::to_writer_pretty(&mut file, &report)?;
        file.write_all(b"\n")?;
    } else {
        serde_json::to_writer_pretty(io::stdout(), &report)?;
        println!();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn help_does_not_offer_working_set_trimming() {
        let mut command = Cli::command();
        let help = command.render_long_help().to_string().to_ascii_lowercase();
        assert!(!help.contains("emptyworkingset"));
        assert!(!help.contains("setprocessworkingsetsize"));
    }
}
