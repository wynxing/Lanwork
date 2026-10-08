use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

use lanwork_fixture::{
    DEFAULT_LISTS, GenerateRequest, PROTOCOL_NOTE_BYTES, PROTOCOL_NOTES, PROTOCOL_REFS,
    PROTOCOL_SHELVES, PROTOCOL_SHORTCUTS, PROTOCOL_TODOS, Scale, generate,
};

#[derive(Parser)]
#[command(
    name = "lanwork-fixture",
    about = "在隔离目录生成性能测量用的固定数据。默认规模等于协议。"
)]
struct Cli {
    /// 夹具根目录。数据在其子目录 data，不会读取 `LANWORK_DATA_DIR`。
    #[arg(long)]
    out: PathBuf,
    #[arg(long, default_value_t = PROTOCOL_TODOS)]
    todos: usize,
    /// 清单个数。协议没有规定，默认 20。
    #[arg(long, default_value_t = DEFAULT_LISTS)]
    lists: usize,
    #[arg(long, default_value_t = PROTOCOL_NOTES)]
    notes: usize,
    /// 每篇便签正文的字节数。默认 2048，即约 2 KiB。
    #[arg(long, default_value_t = PROTOCOL_NOTE_BYTES)]
    note_bytes: usize,
    #[arg(long, default_value_t = PROTOCOL_SHELVES)]
    shelves: usize,
    #[arg(long, default_value_t = PROTOCOL_REFS)]
    refs: usize,
    #[arg(long, default_value_t = PROTOCOL_SHORTCUTS)]
    shortcuts: usize,
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

fn run() -> Result<(), lanwork_fixture::ToolError> {
    let cli = Cli::try_parse_from(env::args_os())
        .map_err(|err| lanwork_fixture::ToolError::new(err.to_string()))?;
    let report = generate(&GenerateRequest {
        out: cli.out,
        scale: Scale {
            todos: cli.todos,
            lists: cli.lists,
            notes: cli.notes,
            note_bytes: cli.note_bytes,
            shelves: cli.shelves,
            refs: cli.refs,
            shortcuts: cli.shortcuts,
        },
        user_profile: env::var_os("USERPROFILE").map(PathBuf::from),
    })?;
    serde_json::to_writer_pretty(std::io::stdout(), &report)?;
    println!();
    Ok(())
}
