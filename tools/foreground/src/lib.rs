//! 前台探测的参数，以及「别的程序已在前台并静默」的判定。窗口和 `SendInput` 在 Windows 上的 `main` 里。

use lanwork_core::shell::tick_after;

/// 前台不是本进程，且最后输入时刻已经更新之后，还要保持这么久才开始一项。
pub const COLD_QUIET_MS: u64 = 2000;

/// 命令行说明。产品路径不用这里的 Alt 和 `AttachThreadInput`。
pub const USAGE: &str = "\
用法: lanwork-foreground [--method <名称>]... [--include-alt] [--include-attach]

在别的程序处于前台、并且本进程有一会儿没收到输入时，逐项试能不能把本窗口拿到前台。
每一项开始前都会等：前台不是本进程，而且出现了比这一项开始时更新的最后输入时刻，
然后这个前台和这个时刻保持约 2 秒。请点击别的程序（不要点本探测窗口），再松开鼠标和键盘。
不要在本进程里按键来开始：那一下会让本进程变成最后一次输入的接收者，plain 就会误判为成功。
上一项若已经把本窗口带到前台，下一项会再等你点击别处。

默认依次试：
  plain                直接 SetForegroundWindow
  relative-immediate   相对位移 0 的鼠标移动，立刻置前（不等待，用来看和输入线程的竞争）
  relative-wait        同上，但等到最后输入时刻推进后再置前。Lanwork 用这个
  absolute-wait        绝对坐标移到当前像素（回投必须仍是同一像素才发），等待后再置前

可选，产品路径不用：
  --include-alt        只送 Alt 键抬起再立刻置前。可能打开前台程序的菜单，或打断中文输入
  --include-attach     AttachThreadInput 到前台线程再置前，随即解开。可能死锁；卡住就结束进程

--method 可重复，用来只跑列出的项。名称就是上面的 plain、relative-immediate、relative-wait、
absolute-wait、alt、attach。--include-alt 和 --include-attach 会追加到列表末尾。
";

/// 一次要试的置前办法。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeMethod {
    Plain,
    RelativeImmediate,
    RelativeWait,
    AbsoluteWait,
    Alt,
    Attach,
}

impl ProbeMethod {
    pub fn name(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::RelativeImmediate => "relative-immediate",
            Self::RelativeWait => "relative-wait",
            Self::AbsoluteWait => "absolute-wait",
            Self::Alt => "alt",
            Self::Attach => "attach",
        }
    }

    pub fn summary(self) -> &'static str {
        match self {
            Self::Plain => "直接 SetForegroundWindow",
            Self::RelativeImmediate => "相对位移 0，立刻置前",
            Self::RelativeWait => "相对位移 0，等到输入被取走再置前",
            Self::AbsoluteWait => "绝对坐标零位移，等到输入被取走再置前",
            Self::Alt => "只送 Alt 抬起，立刻置前",
            Self::Attach => "AttachThreadInput 后置前，随即解开",
        }
    }
}

/// 解析后的参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Args {
    Help,
    Run(Vec<ProbeMethod>),
}

/// 解析进程参数，不含程序名。
pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let args: Vec<String> = args.into_iter().collect();
    let mut methods = Vec::new();
    let mut explicit = false;
    let mut include_alt = false;
    let mut include_attach = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "-h" | "--help" => {
                if args.len() != 1 {
                    return Err(usage_error("帮助不能和其他参数一起用"));
                }
                return Ok(Args::Help);
            }
            "--include-alt" => include_alt = true,
            "--include-attach" => include_attach = true,
            "--method" => {
                index += 1;
                let name = args.get(index).ok_or_else(|| usage_error("缺少方法名"))?;
                methods.push(method_from(name)?);
                explicit = true;
            }
            other => return Err(usage_error(&format!("不认识的参数 {other}"))),
        }
        index += 1;
    }
    if !explicit {
        methods = vec![
            ProbeMethod::Plain,
            ProbeMethod::RelativeImmediate,
            ProbeMethod::RelativeWait,
            ProbeMethod::AbsoluteWait,
        ];
    }
    if include_alt && !methods.contains(&ProbeMethod::Alt) {
        methods.push(ProbeMethod::Alt);
    }
    if include_attach && !methods.contains(&ProbeMethod::Attach) {
        methods.push(ProbeMethod::Attach);
    }
    if methods.is_empty() {
        return Err(usage_error("没有要试的方法"));
    }
    Ok(Args::Run(methods))
}

fn method_from(name: &str) -> Result<ProbeMethod, String> {
    match name {
        "plain" => Ok(ProbeMethod::Plain),
        "relative-immediate" => Ok(ProbeMethod::RelativeImmediate),
        "relative-wait" => Ok(ProbeMethod::RelativeWait),
        "absolute-wait" => Ok(ProbeMethod::AbsoluteWait),
        "alt" => Ok(ProbeMethod::Alt),
        "attach" => Ok(ProbeMethod::Attach),
        other => Err(usage_error(&format!("不认识的方法 {other}"))),
    }
}

fn usage_error(reason: &str) -> String {
    format!("{reason}\n{USAGE}")
}

/// 一项开始前的等待。`token` 区分前台窗口，`tick` 是 `GetLastInputInfo` 的时刻。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColdWatch {
    started: Option<u32>,
    held: Option<ColdHeld>,
    since_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ColdHeld {
    token: u64,
    tick: Option<u32>,
}

/// 这一拍还在等，或者可以开始置前。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColdPoll {
    Waiting,
    Ready,
}

impl ColdWatch {
    /// `started` 是这一项开始时的最后输入时刻。读不到时传 `None`，那时只要求别的程序在前台。
    pub fn new(started: Option<u32>) -> Self {
        Self {
            started,
            held: None,
            since_ms: None,
        }
    }

    /// 已经开始计静默。这段时间里不要再提示用户去点击。
    pub fn arming(&self) -> bool {
        self.since_ms.is_some()
    }

    /// `now_ms` 是单调时钟的毫秒，只和上一次相比，不必是 Unix 时间。
    pub fn poll(&mut self, now_ms: u64, foreign: bool, token: u64, tick: Option<u32>) -> ColdPoll {
        let newer = match (self.started, tick) {
            (Some(previous), Some(current)) => tick_after(previous, current),
            (None, _) => true,
            (Some(_), None) => false,
        };
        let same = self
            .held
            .is_some_and(|held| held.token == token && held.tick == tick);
        if !foreign || !newer || !same {
            let arm = foreign && newer;
            self.held = arm.then_some(ColdHeld { token, tick });
            self.since_ms = arm.then_some(now_ms);
            return ColdPoll::Waiting;
        }
        let ready = self
            .since_ms
            .is_some_and(|start| now_ms.saturating_sub(start) >= COLD_QUIET_MS);
        if ready {
            ColdPoll::Ready
        } else {
            ColdPoll::Waiting
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_slice(args: &[&str]) -> Result<Args, String> {
        parse(args.iter().map(|item| (*item).to_owned()))
    }

    #[test]
    fn default_methods_skip_alt_and_attach() {
        let Args::Run(methods) = parse_slice(&[]).unwrap() else {
            panic!("expected run");
        };
        assert_eq!(
            methods,
            vec![
                ProbeMethod::Plain,
                ProbeMethod::RelativeImmediate,
                ProbeMethod::RelativeWait,
                ProbeMethod::AbsoluteWait,
            ]
        );
    }

    #[test]
    fn method_flag_replaces_the_default_list() {
        let Args::Run(methods) =
            parse_slice(&["--method", "relative-wait", "--method", "plain"]).unwrap()
        else {
            panic!("expected run");
        };
        assert_eq!(methods, vec![ProbeMethod::RelativeWait, ProbeMethod::Plain]);
    }

    #[test]
    fn include_flags_append() {
        let Args::Run(methods) = parse_slice(&["--include-attach", "--include-alt"]).unwrap()
        else {
            panic!("expected run");
        };
        assert_eq!(methods[4], ProbeMethod::Alt);
        assert_eq!(methods[5], ProbeMethod::Attach);
        assert_eq!(methods.len(), 6);
    }

    #[test]
    fn help_and_unknown_args() {
        assert!(matches!(parse_slice(&["--help"]), Ok(Args::Help)));
        assert!(parse_slice(&["--help", "--method", "plain"]).is_err());
        assert!(parse_slice(&["--method"]).is_err());
        assert!(parse_slice(&["--method", "key"]).is_err());
        assert!(parse_slice(&["--nope"]).is_err());
    }

    #[test]
    fn cold_watch_needs_a_newer_tick_then_quiet() {
        let mut watch = ColdWatch::new(Some(10));
        assert_eq!(watch.poll(0, true, 1, Some(10)), ColdPoll::Waiting);
        assert!(!watch.arming());
        assert_eq!(watch.poll(0, true, 1, Some(11)), ColdPoll::Waiting);
        assert!(watch.arming());
        assert_eq!(
            watch.poll(COLD_QUIET_MS - 1, true, 1, Some(11)),
            ColdPoll::Waiting
        );
        assert_eq!(
            watch.poll(COLD_QUIET_MS, true, 1, Some(11)),
            ColdPoll::Ready
        );
    }

    #[test]
    fn cold_watch_resets_when_the_user_moves_or_comes_back() {
        let mut watch = ColdWatch::new(Some(10));
        assert_eq!(watch.poll(0, true, 1, Some(20)), ColdPoll::Waiting);
        assert_eq!(watch.poll(500, false, 1, Some(20)), ColdPoll::Waiting);
        assert!(!watch.arming());
        assert_eq!(watch.poll(500, true, 1, Some(20)), ColdPoll::Waiting);
        assert_eq!(watch.poll(1_000, true, 2, Some(20)), ColdPoll::Waiting);
        assert_eq!(watch.poll(1_500, true, 2, Some(21)), ColdPoll::Waiting);
        assert_eq!(
            watch.poll(1_500 + COLD_QUIET_MS, true, 2, Some(21)),
            ColdPoll::Ready
        );
    }
}
