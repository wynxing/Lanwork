//! 技术验证入口。`run` 打开软件渲染的悬浮窗。`measure` 在无桌面的软件窗口上记一帧，再让进程睡一会儿。
//! 不写产品数据目录，也不读 MayDolist。

use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use lanwork_toolhost_spike::{
    MeasureReport, Options, Session, attach_for_test, green_pixels, idle_cpu_percent, memory_note,
    platform_lock, process_memory, run_window, write_example,
};

fn main() {
    let mut args = std::env::args().skip(1);
    let command = args.next().unwrap_or_default();
    let rest: Vec<String> = args.collect();
    let result = match command.as_str() {
        "run" => run(&rest),
        "measure" => measure(&rest),
        _ => Err(usage()),
    };
    if let Err(err) = result {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

fn usage() -> String {
    "用法：lanwork-toolhost-spike run --example quota\n      lanwork-toolhost-spike measure --example quota --secs 5 --out measure.json".to_string()
}

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].as_str())
}

fn run(args: &[String]) -> Result<(), String> {
    let example = flag(args, "--example").unwrap_or("quota");
    let dir = scratch("run");
    write_example(&dir, example)?;
    let session = Rc::new(open_example(&dir, example)?);
    run_window(session).map_err(|err| err.to_string())
}

fn measure(args: &[String]) -> Result<(), String> {
    let example = flag(args, "--example").unwrap_or("quota");
    let secs: f64 = flag(args, "--secs")
        .unwrap_or("5")
        .parse()
        .map_err(|_| "secs 不是数字".to_string())?;
    let out = flag(args, "--out").ok_or("缺少 --out")?;
    let dir = scratch("measure");
    write_example(&dir, example)?;
    let started = Instant::now();
    let session = Rc::new(open_example(&dir, example)?);
    session.on_load();
    let _guard = platform_lock();
    let ui = attach_for_test(Rc::clone(&session));
    let frame_started = Instant::now();
    let _green = green_pixels(&ui);
    let frame_ms = frame_started.elapsed().as_millis();
    let startup_ms = started.elapsed().as_millis();
    drop(ui);
    drop(_guard);
    let (rss_bytes, private_bytes) = process_memory();
    let report = MeasureReport {
        startup_ms,
        frame_ms,
        rss_bytes,
        private_bytes,
        idle_cpu_percent: idle_cpu_percent(secs),
        idle_secs: secs,
        renderer: "software".to_string(),
        lua: "5.4".to_string(),
        slint: "1.18.1".to_string(),
        memory_note: memory_note().to_string(),
    };
    let json = serde_json::to_string_pretty(&report).map_err(|err| err.to_string())?;
    std::fs::write(out, json).map_err(|err| err.to_string())?;
    Ok(())
}

fn open_example(dir: &std::path::Path, _example: &str) -> Result<Session, String> {
    Session::open(dir, Options::default())
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "lanwork-toolhost-spike-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}
