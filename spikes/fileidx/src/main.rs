//! `spikes/fileidx` 命令行。标准输出是 JSON。

use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use serde::Serialize;

use std::fmt;

use fileidx::{
    FileIdxError, LoadSnapshot, SdkChoice, WsearchMode, cap_check, compare_modes, indexed_roots,
    mono_ns, poll, private_bytes, probe, probe_one, query_everything, query_windows, sample_load,
};

#[derive(Parser)]
#[command(
    name = "fileidx",
    about = "Everything SDK 与 Windows Search 文件名查询验证。"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 安装情况和 SDK 探测。不启动 Everything。
    Probe(ProbeArgs),
    /// 在一段时间内重复探测，记录状态变化。
    Poll(PollArgs),
    /// 按名称查询 Everything，最多 50 条。
    Everything(QueryArgs),
    /// 用 `*` 检查 50 条上限。只输出数量。
    Cap(CapArgs),
    /// 按文件名查询 Windows Search。
    Wsearch(WsearchArgs),
    /// 对照几种 Windows Search 查询。
    Compare(CompareArgs),
    /// 采样约 1 秒的系统负载。
    Load(LoadArgs),
    /// 列出 Windows Search 已索引的根。
    Scopes,
    /// 文件名查询的 P95 样本。负载明显偏高时退出码 3。
    Bench(BenchArgs),
}

#[derive(Clone, Copy, ValueEnum)]
enum SdkArg {
    Auto,
    Sdk3,
    Sdk14,
}

impl fmt::Display for SdkArg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            SdkArg::Auto => "auto",
            SdkArg::Sdk3 => "sdk3",
            SdkArg::Sdk14 => "sdk14",
        })
    }
}

impl From<SdkArg> for SdkChoice {
    fn from(value: SdkArg) -> Self {
        match value {
            SdkArg::Auto => SdkChoice::Auto,
            SdkArg::Sdk3 => SdkChoice::Sdk3,
            SdkArg::Sdk14 => SdkChoice::Sdk14,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum ModeArg {
    Like,
    HelperFilename,
    HelperContent,
    HelperDefault,
    Contains,
}

impl fmt::Display for ModeArg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ModeArg::Like => "like",
            ModeArg::HelperFilename => "helper-filename",
            ModeArg::HelperContent => "helper-content",
            ModeArg::HelperDefault => "helper-default",
            ModeArg::Contains => "contains",
        })
    }
}

impl From<ModeArg> for WsearchMode {
    fn from(value: ModeArg) -> Self {
        match value {
            ModeArg::Like => WsearchMode::FilenameLike,
            ModeArg::HelperFilename => WsearchMode::HelperFilename,
            ModeArg::HelperContent => WsearchMode::HelperContentProperties,
            ModeArg::HelperDefault => WsearchMode::HelperDefault,
            ModeArg::Contains => WsearchMode::ContentContains,
        }
    }
}

#[derive(Parser)]
struct ProbeArgs {
    #[arg(long, value_enum, default_value_t = SdkArg::Auto)]
    sdk: SdkArg,
    /// 只探这一个实例。省略时 auto/sdk3 会先试未命名实例，失败再试 1.5a。
    #[arg(long)]
    instance: Option<String>,
    /// 调用方已经放了一份客户端（例如便携版目录）。系统里没装时用它区分「未安装」和「已安装但未运行」。
    #[arg(long)]
    client_present: bool,
}

#[derive(Parser)]
struct PollArgs {
    #[arg(long, value_enum)]
    sdk: SdkArg,
    #[arg(long)]
    instance: Option<String>,
    #[arg(long, default_value_t = 15000)]
    millis: u64,
}

#[derive(Parser)]
struct QueryArgs {
    #[arg(long, value_enum, default_value_t = SdkArg::Auto)]
    sdk: SdkArg,
    #[arg(long)]
    instance: Option<String>,
    #[arg(long)]
    text: String,
    #[arg(long, default_value_t = 50)]
    limit: usize,
    #[arg(long)]
    expect: Option<String>,
    #[arg(long)]
    reject: Option<String>,
    #[arg(long)]
    client_present: bool,
}

#[derive(Parser)]
struct CapArgs {
    #[arg(long, value_enum)]
    sdk: SdkArg,
    #[arg(long)]
    instance: Option<String>,
}

#[derive(Parser)]
struct WsearchArgs {
    #[arg(long)]
    text: String,
    #[arg(long, value_enum, default_value_t = ModeArg::Like)]
    mode: ModeArg,
    #[arg(long, default_value_t = 50)]
    limit: usize,
    #[arg(long)]
    expect: Option<String>,
    #[arg(long)]
    reject: Option<String>,
}

#[derive(Parser)]
struct CompareArgs {
    #[arg(long)]
    text: String,
    #[arg(long, default_value_t = 50)]
    limit: usize,
    #[arg(long)]
    expect: Option<String>,
    #[arg(long)]
    reject: Option<String>,
}

#[derive(Parser)]
struct LoadArgs {
    #[arg(long, default_value_t = 1000)]
    interval_ms: u64,
}

#[derive(Parser)]
struct BenchArgs {
    #[arg(long)]
    text: String,
    #[arg(long, default_value_t = 100)]
    samples: u64,
    #[arg(long, default_value_t = 5)]
    warmup: u64,
    #[arg(long)]
    out: PathBuf,
    /// 负载偏高也继续记样本。
    #[arg(long)]
    allow_skew: bool,
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("{err}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ExitCode, FileIdxError> {
    let cli =
        Cli::try_parse_from(env::args_os()).map_err(|err| FileIdxError::new(err.to_string()))?;
    match cli.command {
        Command::Probe(args) => {
            let present = args.client_present.then_some(true);
            let report = if args.instance.is_some() {
                probe_one(args.sdk.into(), args.instance.as_deref(), present)?
            } else {
                probe(args.sdk.into(), present)?
            };
            print_json(&report)?;
        }
        Command::Poll(args) => {
            let report = poll(args.sdk.into(), args.instance.as_deref(), args.millis)?;
            print_json(&report)?;
        }
        Command::Everything(args) => {
            let present = args.client_present.then_some(true);
            let report = query_everything(
                args.sdk.into(),
                args.instance.as_deref(),
                &args.text,
                args.limit,
                args.expect.as_deref(),
                args.reject.as_deref(),
                present,
            )?;
            print_json(&report)?;
        }
        Command::Cap(args) => {
            let report = cap_check(args.sdk.into(), args.instance.as_deref())?;
            print_json(&report)?;
        }
        Command::Wsearch(args) => {
            let report = query_windows(
                args.mode.into(),
                &args.text,
                args.limit,
                args.expect.as_deref(),
                args.reject.as_deref(),
            )?;
            print_json(&report)?;
        }
        Command::Compare(args) => {
            let report = compare_modes(
                &args.text,
                args.limit,
                args.expect.as_deref(),
                args.reject.as_deref(),
            )?;
            print_json(&report)?;
        }
        Command::Load(args) => {
            print_json(&sample_load(args.interval_ms)?)?;
        }
        Command::Scopes => {
            let roots = indexed_roots()?;
            print_json(&roots)?;
        }
        Command::Bench(args) => return bench(args),
    }
    Ok(ExitCode::SUCCESS)
}

fn bench(args: BenchArgs) -> Result<ExitCode, FileIdxError> {
    let load: LoadSnapshot = sample_load(1000)?;
    if load.skewed && !args.allow_skew {
        print_json(&load)?;
        return Ok(ExitCode::from(3));
    }
    let before_com = private_bytes()?;
    let session = fileidx::WsearchSession::open()?;
    let before_query = private_bytes()?;
    let first = session.query(WsearchMode::FilenameLike, &args.text, 50, None, None);
    let after_first = private_bytes()?;
    fs::create_dir_all(&args.out)?;
    let jsonl_path = args.out.join("windows-search.jsonl");
    let mut jsonl = fs::File::create(&jsonl_path)?;
    let mut seq = 1u64;
    if let Some(elapsed) = first.elapsed_ns {
        let end = mono_ns();
        let start = end.saturating_sub(elapsed);
        write_latency(&mut jsonl, seq, start, end, true)?;
        seq += 1;
    }
    for _ in 0..args.warmup {
        let sample = session.query(WsearchMode::FilenameLike, &args.text, 50, None, None);
        if let Some(elapsed) = sample.elapsed_ns {
            let end = mono_ns();
            write_latency(&mut jsonl, seq, end.saturating_sub(elapsed), end, true)?;
            seq += 1;
        } else {
            return Err(FileIdxError::new(format!(
                "预热查询失败：{}",
                sample.message
            )));
        }
    }
    for _ in 0..args.samples {
        let sample = session.query(WsearchMode::FilenameLike, &args.text, 50, None, None);
        if !sample.ok {
            return Err(FileIdxError::new(format!(
                "样本查询失败：{}",
                sample.message
            )));
        }
        let elapsed = sample.elapsed_ns.unwrap_or(0);
        let end = mono_ns();
        write_latency(&mut jsonl, seq, end.saturating_sub(elapsed), end, false)?;
        seq += 1;
    }
    jsonl.flush()?;
    let load_after = sample_load(1000)?;
    let summary = BenchSummary {
        load,
        load_after,
        connection_string: session.connection_string().to_string(),
        sql: first.sql.clone(),
        filename_only: first.filename_only,
        private_bytes_before_open: before_com,
        private_bytes_after_open: before_query,
        private_bytes_after_first: after_first,
        private_bytes_delta_execute: after_first as i64 - before_query as i64,
        private_bytes_delta_including_open: after_first as i64 - before_com as i64,
        first_ok: first.ok,
        first_elapsed_ns: first.elapsed_ns,
        first_returned: first.returned,
        first_hresult: first.hresult,
        warmup: args.warmup,
        samples: args.samples,
        jsonl: jsonl_path.display().to_string(),
    };
    let summary_path = args.out.join("windows-search-bench.json");
    fs::write(
        &summary_path,
        serde_json::to_string_pretty(&summary).map_err(|err| FileIdxError::new(err.to_string()))?,
    )?;
    print_json(&summary)?;
    Ok(ExitCode::SUCCESS)
}

#[derive(Serialize)]
struct BenchSummary {
    load: LoadSnapshot,
    load_after: LoadSnapshot,
    connection_string: String,
    sql: String,
    filename_only: bool,
    private_bytes_before_open: u64,
    private_bytes_after_open: u64,
    private_bytes_after_first: u64,
    private_bytes_delta_execute: i64,
    private_bytes_delta_including_open: i64,
    first_ok: bool,
    first_elapsed_ns: Option<u64>,
    first_returned: usize,
    first_hresult: Option<u32>,
    warmup: u64,
    samples: u64,
    jsonl: String,
}

fn write_latency(
    file: &mut fs::File,
    seq: u64,
    start_ns: u64,
    end_ns: u64,
    warmup: bool,
) -> Result<(), FileIdxError> {
    let line = serde_json::json!({
        "metric": "result",
        "source": "windows_search",
        "seq": seq,
        "start_ns": start_ns,
        "end_ns": end_ns,
        "warmup": warmup,
        "superseded": false
    });
    writeln!(file, "{line}")?;
    Ok(())
}

fn print_json<T: Serialize>(value: &T) -> Result<(), FileIdxError> {
    let text =
        serde_json::to_string_pretty(value).map_err(|err| FileIdxError::new(err.to_string()))?;
    let mut out = io::stdout().lock();
    writeln!(out, "{text}")?;
    Ok(())
}
