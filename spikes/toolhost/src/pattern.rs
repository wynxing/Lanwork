//! Lua 5.4.9 `lstrlib.c` 的模式匹配。`find`、`match`、`gmatch`、`gsub` 的结果与原版一致。
//!
//! 未锚定的查找会从每个起点重试，单个 `.*` 就是平方时间。匹配循环里每 4096 步看一次这次回调的截止时间。
//! 到点则置上与钩子相同的中止标记，并抛出「脚本超时」。`pcall` 接住这一次之后，下一条指令仍会被钩子拦住。
//! 模式写错、捕获写错、嵌套过深仍是普通脚本错误，不置中止标记。
//!
//! 文本和模式另有长度上限，只是验证程序的临时天花板，不是产品规则。
//! 同一块 Lua 状态上，所有还没交还的 `gsub` 结果缓冲区加在一起，跟 Lua 已用内存计入同一块 16 MiB 预算。
//! 超过则这次调用报错，不置中止标记。缓冲区释放时把占用减回去。
//! 复制进结果的字节大约每 64 KiB 再看一次截止时间。
//! 模式函数最多嵌套 8 层。从 Rust 再进 Lua 的宿主函数另有重入上限。
//! Lua 5.4 的 `lua_setcstacklimit` 不改变编译进去的 C 栈上限，所以这层限制放在宿主里。
//! 超过是普通脚本错误，不置中止标记。

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use mlua::{Function, Lua, MultiValue, Table, Value};

const MAX_CAPTURES: usize = 32;
const MAX_CALLS: i32 = 200;
const CAP_UNFINISHED: i64 = -1;
const CAP_POSITION: i64 = -2;
const STEP_INTERVAL: u32 = 4_096;
/// 临时上限。64 KiB 的 `.*` 必须还能跑进匹配器，由步数时钟拦住。
const SUBJECT_CAP: usize = 1024 * 1024;
const PATTERN_CAP: usize = 8 * 1024;
/// 与宿主的临时内存天花板相同。`gsub` 的结果还在 Rust 缓冲区里时，Lua 分配器看不见它。
const RESULT_BUDGET: usize = 16 * 1024 * 1024;
/// 最后交给 Lua 的字符串还有一个小头。留出来，避免刚通过预算检查就在 `create_string` 上失败。
const STRING_OVERHEAD: usize = 64;
/// 复制进结果时，每这么多字节约看一次截止时间。
const COPY_STEP: usize = 64 * 1024;
/// `find` / `match` / `gmatch` / `gsub` 同时还在栈上的层数。再深就报可接住的错误。
const PATTERN_REENTRY_CAP: u32 = 8;
/// 从 Rust 回调再进入 Lua 的层数。`xpcall`、`table.sort` 和模式函数都算。
/// 默认的 200 层 C 调用会把 Rust 栈撑爆，而 `lua_setcstacklimit` 改不了这个上限。
const HOST_REENTRY_CAP: u32 = 16;

/// 挂在 Lua 状态上。嵌套的 `gsub` 看见的是同一份计数。
struct Nest {
    buffers: Cell<usize>,
    pattern_depth: Cell<u32>,
    pattern_overflow: Cell<bool>,
    host_depth: Cell<u32>,
    /// 到顶之后保持拒绝，直到这一层全部展开。否则处理函数会再次整棵重试。
    host_overflow: Cell<bool>,
}

fn nest(lua: &Lua) -> Rc<Nest> {
    if let Some(existing) = lua.app_data_ref::<Rc<Nest>>() {
        return Rc::clone(&existing);
    }
    let created = Rc::new(Nest {
        buffers: Cell::new(0),
        pattern_depth: Cell::new(0),
        pattern_overflow: Cell::new(false),
        host_depth: Cell::new(0),
        host_overflow: Cell::new(false),
    });
    lua.set_app_data(Rc::clone(&created));
    created
}

#[must_use]
pub(crate) struct HostGuard {
    nest: Rc<Nest>,
}

impl Drop for HostGuard {
    fn drop(&mut self) {
        release(&self.nest.host_depth, &self.nest.host_overflow);
    }
}

/// 进入一个会再调用 Lua 的宿主函数。超过上限是普通脚本错误，不置中止标记。
pub(crate) fn enter_host(lua: &Lua) -> mlua::Result<HostGuard> {
    let nest = nest(lua);
    if nest.host_overflow.get() || nest.host_depth.get() >= HOST_REENTRY_CAP {
        nest.host_overflow.set(true);
        return Err(mlua::Error::runtime("调用过深"));
    }
    nest.host_depth.set(nest.host_depth.get() + 1);
    Ok(HostGuard { nest })
}

#[must_use]
struct PatternGuard {
    nest: Rc<Nest>,
}

impl Drop for PatternGuard {
    fn drop(&mut self) {
        release(&self.nest.pattern_depth, &self.nest.pattern_overflow);
    }
}

fn enter_pattern(lua: &Lua) -> mlua::Result<PatternGuard> {
    let nest = nest(lua);
    if nest.pattern_overflow.get() || nest.pattern_depth.get() >= PATTERN_REENTRY_CAP {
        nest.pattern_overflow.set(true);
        return Err(mlua::Error::runtime("模式调用过深"));
    }
    nest.pattern_depth.set(nest.pattern_depth.get() + 1);
    Ok(PatternGuard { nest })
}

fn release(depth: &Cell<u32>, overflow: &Cell<bool>) {
    let next = depth.get().saturating_sub(1);
    depth.set(next);
    if next == 0 {
        overflow.set(false);
    }
}

/// 一层 `gsub` 结果缓冲区占着的字节。丢弃时从共享计数里减掉，出错路径也走这里。
struct BufferCharge {
    nest: Rc<Nest>,
    charged: Cell<usize>,
}

impl BufferCharge {
    fn new(nest: Rc<Nest>) -> Self {
        Self {
            nest,
            charged: Cell::new(0),
        }
    }

    fn note(&self, capacity: usize) {
        let old = self.charged.get();
        if capacity <= old {
            return;
        }
        let delta = capacity - old;
        self.nest.buffers.set(self.nest.buffers.get() + delta);
        self.charged.set(capacity);
    }
}

impl Drop for BufferCharge {
    fn drop(&mut self) {
        let left = self.nest.buffers.get().saturating_sub(self.charged.get());
        self.nest.buffers.set(left);
    }
}

pub(crate) trait CpuWatch: Send + Sync {
    fn deadline_ms(&self) -> u64;
    fn elapsed_ms(&self) -> u64;
    fn aborted(&self) -> bool;
    fn abort(&self, line: i64);
}

#[derive(Clone, Copy)]
enum Call {
    Find,
    Match,
    Gmatch,
    Gsub,
}

impl Call {
    fn name(self) -> &'static str {
        match self {
            Call::Find => "find",
            Call::Match => "match",
            Call::Gmatch => "gmatch",
            Call::Gsub => "gsub",
        }
    }
}

pub(crate) fn install<W>(lua: &Lua, watch: Arc<W>) -> Result<(), String>
where
    W: CpuWatch + 'static,
{
    lua.set_app_data(nest(lua));
    let library: Table = lua
        .globals()
        .get("string")
        .map_err(|err| format!("脚本错误：{err}"))?;
    for kind in [Call::Find, Call::Match, Call::Gmatch, Call::Gsub] {
        let watch = Arc::clone(&watch);
        let function = lua
            .create_function(move |lua, args: MultiValue| {
                dispatch(lua, Arc::clone(&watch), kind, args)
            })
            .map_err(|err| format!("脚本错误：{err}"))?;
        library
            .set(kind.name(), function)
            .map_err(|err| format!("脚本错误：{err}"))?;
    }
    Ok(())
}

fn dispatch<W>(lua: &Lua, watch: Arc<W>, kind: Call, args: MultiValue) -> mlua::Result<MultiValue>
where
    W: CpuWatch + 'static,
{
    if watch.aborted() {
        return Err(mlua::Error::runtime("脚本超时"));
    }
    let _host = enter_host(lua)?;
    let _pattern = enter_pattern(lua)?;
    match kind {
        Call::Find => find(lua, watch.as_ref(), &args, true),
        Call::Match => find(lua, watch.as_ref(), &args, false),
        Call::Gmatch => gmatch(lua, watch, &args),
        Call::Gsub => gsub(lua, watch.as_ref(), &args),
    }
}

fn find<W: CpuWatch>(
    lua: &Lua,
    watch: &W,
    args: &MultiValue,
    is_find: bool,
) -> mlua::Result<MultiValue> {
    let subject = lua_bytes(lua, required(args, 0)?)?;
    let pattern = lua_bytes(lua, required(args, 1)?)?;
    check_lengths(&subject, &pattern)?;
    let init = rel_index(opt_i64(lua, args.get(2), 1)?, subject.len());
    if init > subject.len() {
        return Ok(just_nil());
    }
    let plain = is_find && args.get(3).is_some_and(truthy);
    if is_find && (plain || nospecials(&pattern)) {
        let mut steps = 0u32;
        let found = lmemfind(lua, watch, &mut steps, &subject[init..], &pattern)?;
        return Ok(match found {
            Some(rel) => {
                let offset = init + rel;
                MultiValue::from_vec(vec![
                    Value::Integer((offset + 1) as i64),
                    Value::Integer((offset + pattern.len()) as i64),
                ])
            }
            None => just_nil(),
        });
    }
    let anchor = pattern.first() == Some(&b'^');
    let pat = if anchor { &pattern[1..] } else { &pattern };
    let mut state = MatchState::new(lua, watch, &subject, pat, None);
    let mut at = init;
    loop {
        state.reprep();
        if let Some(end) = state.exec(at, 0)? {
            if is_find {
                let mut values = vec![Value::Integer((at + 1) as i64), Value::Integer(end as i64)];
                if state.level > 0 {
                    values.extend(state.capture_values(at, end)?);
                }
                return Ok(MultiValue::from_vec(values));
            }
            return Ok(MultiValue::from_vec(state.capture_values(at, end)?));
        }
        if anchor || at >= subject.len() {
            break;
        }
        at += 1;
    }
    Ok(just_nil())
}

struct Cursor {
    src: usize,
    last: Option<usize>,
}

fn gmatch<W>(lua: &Lua, watch: Arc<W>, args: &MultiValue) -> mlua::Result<MultiValue>
where
    W: CpuWatch + 'static,
{
    let subject = lua_bytes(lua, required(args, 0)?)?;
    let pattern = lua_bytes(lua, required(args, 1)?)?;
    check_lengths(&subject, &pattern)?;
    let mut init = rel_index(opt_i64(lua, args.get(2), 1)?, subject.len());
    if init > subject.len() {
        init = subject.len() + 1;
    }
    let cursor = Rc::new(RefCell::new(Cursor {
        src: init,
        last: None,
    }));
    let iter = lua.create_function(move |lua, _: MultiValue| {
        let _host = enter_host(lua)?;
        let _pattern = enter_pattern(lua)?;
        gmatch_next(lua, watch.as_ref(), &subject, &pattern, &cursor)
    })?;
    Ok(MultiValue::from_vec(vec![Value::Function(iter)]))
}

fn gmatch_next<W: CpuWatch>(
    lua: &Lua,
    watch: &W,
    subject: &[u8],
    pattern: &[u8],
    cursor: &RefCell<Cursor>,
) -> mlua::Result<MultiValue> {
    if watch.aborted() {
        return Err(mlua::Error::runtime("脚本超时"));
    }
    let mut slot = cursor.borrow_mut();
    let mut src = slot.src;
    let mut state = MatchState::new(lua, watch, subject, pattern, None);
    while src <= subject.len() {
        state.reprep();
        if let Some(end) = state.exec(src, 0)?
            && Some(end) != slot.last
        {
            slot.src = end;
            slot.last = Some(end);
            return Ok(MultiValue::from_vec(state.capture_values(src, end)?));
        }
        if src >= subject.len() {
            break;
        }
        src += 1;
    }
    slot.src = subject.len().saturating_add(1);
    Ok(MultiValue::new())
}

enum Repl {
    Bytes(Vec<u8>),
    Func(Function),
    Table(Table),
}

fn gsub<W: CpuWatch>(lua: &Lua, watch: &W, args: &MultiValue) -> mlua::Result<MultiValue> {
    let original = required(args, 0)?.clone();
    let subject = lua_bytes(lua, &original)?;
    let pattern = lua_bytes(lua, required(args, 1)?)?;
    check_lengths(&subject, &pattern)?;
    let repl = match required(args, 2)? {
        Value::Function(function) => Repl::Func(function.clone()),
        Value::Table(table) => Repl::Table(table.clone()),
        Value::Integer(_) | Value::Number(_) | Value::String(_) => {
            Repl::Bytes(lua_bytes(lua, required(args, 2)?)?)
        }
        other => {
            return Err(mlua::Error::runtime(format!(
                "bad argument #3 (string/function/table expected, got {})",
                type_name(other)
            )));
        }
    };
    let max_s = opt_i64(
        lua,
        args.get(3),
        i64::try_from(subject.len())
            .unwrap_or(i64::MAX)
            .saturating_add(1),
    )?;
    let anchor = pattern.first() == Some(&b'^');
    let pat: &[u8] = if anchor { &pattern[1..] } else { &pattern };
    let charge = BufferCharge::new(nest(lua));
    let mut state = MatchState::new(lua, watch, &subject, pat, Some(&charge));
    let mut buf = Vec::new();
    let mut src = 0usize;
    let mut last = None;
    let mut count = 0i64;
    let mut changed = false;
    while count < max_s {
        state.reprep();
        let matched = state.exec(src, 0)?;
        if let Some(end) = matched
            && Some(end) != last
        {
            count += 1;
            if state.add_value(&mut buf, src, end, &repl)? {
                changed = true;
            }
            src = end;
            last = Some(end);
            if anchor {
                break;
            }
            continue;
        }
        if src < subject.len() {
            state.write_span(&mut buf, src, src + 1)?;
            src += 1;
        } else {
            break;
        }
        if anchor {
            break;
        }
    }
    // `luaL_checklstring` 会把数字参数就地收成字符串。没有替换时返回的也是这个字符串。
    let text = if changed {
        state.write_span(&mut buf, src, subject.len())?;
        Value::String(lua.create_string(&buf)?)
    } else if matches!(original, Value::String(_)) {
        original
    } else {
        Value::String(lua.create_string(&subject)?)
    };
    Ok(MultiValue::from_vec(vec![text, Value::Integer(count)]))
}

fn check_lengths(subject: &[u8], pattern: &[u8]) -> mlua::Result<()> {
    if pattern.len() > PATTERN_CAP {
        return Err(mlua::Error::runtime("模式过于复杂"));
    }
    if subject.len() > SUBJECT_CAP {
        return Err(mlua::Error::runtime("文本过长"));
    }
    Ok(())
}

fn required(args: &MultiValue, index: usize) -> mlua::Result<&Value> {
    args.get(index)
        .ok_or_else(|| mlua::Error::runtime("bad argument (value expected)"))
}

fn lua_bytes(lua: &Lua, value: &Value) -> mlua::Result<Vec<u8>> {
    match value {
        Value::String(text) => Ok(text.as_bytes().to_vec()),
        Value::Integer(_) | Value::Number(_) => match lua.coerce_string(value.clone())? {
            Some(text) => Ok(text.as_bytes().to_vec()),
            None => Err(mlua::Error::runtime("bad argument (string expected)")),
        },
        _ => Err(mlua::Error::runtime("bad argument (string expected)")),
    }
}

fn opt_i64(lua: &Lua, value: Option<&Value>, default: i64) -> mlua::Result<i64> {
    match value {
        None | Some(Value::Nil) => Ok(default),
        Some(other) => match lua.coerce_integer(other.clone())? {
            Some(number) => Ok(number),
            None => Err(mlua::Error::runtime("bad argument (number expected)")),
        },
    }
}

fn truthy(value: &Value) -> bool {
    !matches!(value, Value::Nil | Value::Boolean(false))
}

fn just_nil() -> MultiValue {
    MultiValue::from_vec(vec![Value::Nil])
}

/// 1-based Lua index to a 0-based byte index. Past-the-end stays greater than `len`.
fn rel_index(pos: i64, len: usize) -> usize {
    let one = if pos > 0 {
        usize::try_from(pos).unwrap_or(usize::MAX)
    } else if pos == 0 {
        1
    } else {
        let Ok(len_i) = i64::try_from(len) else {
            return 0;
        };
        if pos < -len_i {
            1
        } else {
            usize::try_from(len_i + pos + 1).unwrap_or(0)
        }
    };
    one.saturating_sub(1)
}

fn nospecials(pattern: &[u8]) -> bool {
    const SPECIALS: &[u8] = b"^$*+?.([%-";
    let mut upto = 0;
    loop {
        if upto > pattern.len() {
            return true;
        }
        let rest = &pattern[upto..];
        let head = rest
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(rest.len());
        if pattern[upto..upto + head]
            .iter()
            .any(|byte| SPECIALS.contains(byte))
        {
            return false;
        }
        upto += head + 1;
    }
}

fn lmemfind<W: CpuWatch>(
    lua: &Lua,
    watch: &W,
    steps: &mut u32,
    hay: &[u8],
    needle: &[u8],
) -> mlua::Result<Option<usize>> {
    if needle.is_empty() {
        return Ok(Some(0));
    }
    if needle.len() > hay.len() {
        return Ok(None);
    }
    let tail = needle.len() - 1;
    let mut window = hay.len() - tail;
    let mut base = 0usize;
    while window > 0 {
        charge(lua, watch, steps)?;
        let search = &hay[base..base + window];
        let Some(rel) = search.iter().position(|byte| *byte == needle[0]) else {
            return Ok(None);
        };
        let after = rel + 1;
        if hay[base + after..base + after + tail] == needle[1..] {
            return Ok(Some(base + rel));
        }
        window -= after;
        base += after;
    }
    Ok(None)
}

fn charge<W: CpuWatch>(lua: &Lua, watch: &W, steps: &mut u32) -> mlua::Result<()> {
    *steps = steps.wrapping_add(1);
    if !steps.is_multiple_of(STEP_INTERVAL) {
        return Ok(());
    }
    if watch.aborted() {
        return Err(mlua::Error::runtime("脚本超时"));
    }
    let deadline = watch.deadline_ms();
    if deadline == 0 || watch.elapsed_ms() < deadline {
        return Ok(());
    }
    watch.abort(script_line(lua));
    Err(mlua::Error::runtime("脚本超时"))
}

fn script_line(lua: &Lua) -> i64 {
    for level in 0..16 {
        match lua.inspect_stack(level, |debug| debug.current_line()) {
            Some(Some(line)) => return line as i64,
            Some(None) => continue,
            None => break,
        }
    }
    -1
}

#[derive(Clone, Copy)]
struct Capture {
    init: usize,
    len: i64,
}

struct MatchState<'a, W: CpuWatch> {
    lua: &'a Lua,
    watch: &'a W,
    src: &'a [u8],
    pat: &'a [u8],
    steps: u32,
    /// 自从上次看截止时间以来，写进 `gsub` 结果的字节数。
    copied: Cell<usize>,
    charge: Option<&'a BufferCharge>,
    depth: i32,
    level: usize,
    capture: [Capture; MAX_CAPTURES],
}

enum Flow {
    Done(Option<usize>),
    Continue { s: usize, p: usize },
}

impl<'a, W: CpuWatch> MatchState<'a, W> {
    fn new(
        lua: &'a Lua,
        watch: &'a W,
        src: &'a [u8],
        pat: &'a [u8],
        charge: Option<&'a BufferCharge>,
    ) -> Self {
        Self {
            lua,
            watch,
            src,
            pat,
            steps: 0,
            copied: Cell::new(0),
            charge,
            depth: MAX_CALLS,
            level: 0,
            capture: [Capture { init: 0, len: 0 }; MAX_CAPTURES],
        }
    }

    fn reprep(&mut self) {
        self.depth = MAX_CALLS;
        self.level = 0;
    }

    fn charge(&mut self) -> mlua::Result<()> {
        charge(self.lua, self.watch, &mut self.steps)
    }

    fn check_deadline(&self) -> mlua::Result<()> {
        if self.watch.aborted() {
            return Err(mlua::Error::runtime("脚本超时"));
        }
        let deadline = self.watch.deadline_ms();
        if deadline == 0 || self.watch.elapsed_ms() < deadline {
            return Ok(());
        }
        self.watch.abort(script_line(self.lua));
        Err(mlua::Error::runtime("脚本超时"))
    }

    /// 扩容前把各层还没交还的结果缓冲区加进 16 MiB 预算。超过是普通脚本错误，不置中止标记。
    fn begin_write(&self, buf: &mut Vec<u8>, len: usize) -> mlua::Result<()> {
        if len == 0 {
            return Ok(());
        }
        let new_len = buf.len().checked_add(len).ok_or_else(memory_error)?;
        if buf.capacity() >= new_len {
            return Ok(());
        }
        let Some(charge) = self.charge else {
            return Err(memory_error());
        };
        let used = self.lua.used_memory();
        let held = charge.nest.buffers.get();
        let room = RESULT_BUDGET
            .saturating_sub(used)
            .saturating_sub(STRING_OVERHEAD)
            .saturating_sub(held);
        if len > room {
            return Err(memory_error());
        }
        let extra = len.max(COPY_STEP).min(room);
        buf.try_reserve_exact(extra).map_err(|_| memory_error())?;
        charge.note(buf.capacity());
        Ok(())
    }

    fn account_copy(&self, n: usize) -> mlua::Result<()> {
        let pending = self.copied.get() + n;
        if pending >= COPY_STEP {
            self.copied.set(0);
            self.check_deadline()?;
        } else {
            self.copied.set(pending);
        }
        Ok(())
    }

    fn write_bytes(&self, buf: &mut Vec<u8>, mut data: &[u8]) -> mlua::Result<()> {
        self.begin_write(buf, data.len())?;
        while !data.is_empty() {
            let room = COPY_STEP - self.copied.get();
            let n = room.min(data.len());
            buf.extend_from_slice(&data[..n]);
            data = &data[n..];
            self.account_copy(n)?;
        }
        Ok(())
    }

    fn write_span(&self, buf: &mut Vec<u8>, from: usize, to: usize) -> mlua::Result<()> {
        let start = from.min(self.src.len());
        let end = to.min(self.src.len());
        if end <= start {
            return Ok(());
        }
        self.begin_write(buf, end - start)?;
        let mut off = start;
        while off < end {
            let room = COPY_STEP - self.copied.get();
            let n = room.min(end - off);
            buf.extend_from_slice(&self.src[off..off + n]);
            off += n;
            self.account_copy(n)?;
        }
        Ok(())
    }

    fn exec(&mut self, s: usize, p: usize) -> mlua::Result<Option<usize>> {
        if self.depth == 0 {
            return Err(mlua::Error::runtime("pattern too complex"));
        }
        self.depth -= 1;
        let result = self.exec_at(s, p);
        if result.is_ok() {
            self.depth += 1;
        }
        result
    }

    fn exec_at(&mut self, mut s: usize, mut p: usize) -> mlua::Result<Option<usize>> {
        loop {
            self.charge()?;
            if p == self.pat.len() {
                return Ok(Some(s));
            }
            match self.pat_byte(p) {
                b'(' => {
                    let (next, kind) = if self.pat_byte(p + 1) == b')' {
                        (p + 2, CAP_POSITION)
                    } else {
                        (p + 1, CAP_UNFINISHED)
                    };
                    return self.start_capture(s, next, kind);
                }
                b')' => return self.end_capture(s, p + 1),
                b'$' if p + 1 == self.pat.len() => {
                    return Ok(if s == self.src.len() { Some(s) } else { None });
                }
                b'%' => match self.pat_byte(p + 1) {
                    b'b' => match self.match_balance(s, p + 2)? {
                        Some(next) => {
                            s = next;
                            p += 4;
                        }
                        None => return Ok(None),
                    },
                    b'f' => {
                        let bracket = p + 2;
                        if self.pat_byte(bracket) != b'[' {
                            return Err(mlua::Error::runtime("missing '[' after '%f' in pattern"));
                        }
                        let ep = self.class_end(bracket)?;
                        let previous = if s == 0 { 0 } else { self.src[s - 1] };
                        let current = self.src_byte(s);
                        let end = ep.saturating_sub(1);
                        if !self.match_bracket(previous, bracket, end)
                            && self.match_bracket(current, bracket, end)
                        {
                            p = ep;
                        } else {
                            return Ok(None);
                        }
                    }
                    digit @ b'0'..=b'9' => match self.match_capture(s, digit)? {
                        Some(next) => {
                            s = next;
                            p += 2;
                        }
                        None => return Ok(None),
                    },
                    _ => match self.item(s, p)? {
                        Flow::Done(result) => return Ok(result),
                        Flow::Continue {
                            s: next_s,
                            p: next_p,
                        } => {
                            s = next_s;
                            p = next_p;
                        }
                    },
                },
                _ => match self.item(s, p)? {
                    Flow::Done(result) => return Ok(result),
                    Flow::Continue {
                        s: next_s,
                        p: next_p,
                    } => {
                        s = next_s;
                        p = next_p;
                    }
                },
            }
        }
    }

    fn item(&mut self, s: usize, p: usize) -> mlua::Result<Flow> {
        let ep = self.class_end(p)?;
        let suffix = self.pat_byte(ep);
        if !self.single(s, p, ep) {
            if matches!(suffix, b'*' | b'?' | b'-') {
                return Ok(Flow::Continue { s, p: ep + 1 });
            }
            return Ok(Flow::Done(None));
        }
        match suffix {
            b'?' => {
                if let Some(end) = self.exec(s + 1, ep + 1)? {
                    Ok(Flow::Done(Some(end)))
                } else {
                    Ok(Flow::Continue { s, p: ep + 1 })
                }
            }
            b'+' => Ok(Flow::Done(self.max_expand(s + 1, p, ep)?)),
            b'*' => Ok(Flow::Done(self.max_expand(s, p, ep)?)),
            b'-' => Ok(Flow::Done(self.min_expand(s, p, ep)?)),
            _ => Ok(Flow::Continue { s: s + 1, p: ep }),
        }
    }

    fn max_expand(&mut self, s: usize, p: usize, ep: usize) -> mlua::Result<Option<usize>> {
        let mut extra = 0usize;
        while s.saturating_add(extra) < self.src.len() && self.single(s + extra, p, ep) {
            self.charge()?;
            extra += 1;
        }
        loop {
            self.charge()?;
            if let Some(end) = self.exec(s + extra, ep + 1)? {
                return Ok(Some(end));
            }
            if extra == 0 {
                return Ok(None);
            }
            extra -= 1;
        }
    }

    fn min_expand(&mut self, mut s: usize, p: usize, ep: usize) -> mlua::Result<Option<usize>> {
        loop {
            self.charge()?;
            if let Some(end) = self.exec(s, ep + 1)? {
                return Ok(Some(end));
            }
            if self.single(s, p, ep) {
                s += 1;
            } else {
                return Ok(None);
            }
        }
    }

    fn match_balance(&mut self, s: usize, p: usize) -> mlua::Result<Option<usize>> {
        if p >= self.pat.len().saturating_sub(1) {
            return Err(mlua::Error::runtime(
                "malformed pattern (missing arguments to '%b')",
            ));
        }
        let open = self.pat[p];
        let close = self.pat[p + 1];
        if self.src_byte(s) != open {
            return Ok(None);
        }
        let mut depth = 1i32;
        let mut index = s;
        loop {
            index += 1;
            if index >= self.src.len() {
                return Ok(None);
            }
            self.charge()?;
            let byte = self.src[index];
            if byte == close {
                depth -= 1;
                if depth == 0 {
                    return Ok(Some(index + 1));
                }
            } else if byte == open {
                depth = depth.saturating_add(1);
            }
        }
    }

    fn start_capture(&mut self, s: usize, p: usize, kind: i64) -> mlua::Result<Option<usize>> {
        if self.level >= MAX_CAPTURES {
            return Err(mlua::Error::runtime("too many captures"));
        }
        let level = self.level;
        self.capture[level] = Capture { init: s, len: kind };
        self.level = level + 1;
        let matched = self.exec(s, p)?;
        if matched.is_none() {
            self.level -= 1;
        }
        Ok(matched)
    }

    fn end_capture(&mut self, s: usize, p: usize) -> mlua::Result<Option<usize>> {
        let level = self.capture_to_close()?;
        let init = self.capture[level].init;
        self.capture[level].len = s as i64 - init as i64;
        let matched = self.exec(s, p)?;
        if matched.is_none() {
            self.capture[level].len = CAP_UNFINISHED;
        }
        Ok(matched)
    }

    fn capture_to_close(&self) -> mlua::Result<usize> {
        let mut level = self.level;
        while level > 0 {
            level -= 1;
            if self.capture[level].len == CAP_UNFINISHED {
                return Ok(level);
            }
        }
        Err(mlua::Error::runtime("invalid pattern capture"))
    }

    fn check_capture(&self, digit: u8) -> mlua::Result<usize> {
        let index = i64::from(digit) - i64::from(b'1');
        if index < 0
            || index as usize >= self.level
            || self.capture[index as usize].len == CAP_UNFINISHED
        {
            return Err(mlua::Error::runtime(format!(
                "invalid capture index %{}",
                index + 1
            )));
        }
        Ok(index as usize)
    }

    fn match_capture(&self, s: usize, digit: u8) -> mlua::Result<Option<usize>> {
        let index = self.check_capture(digit)?;
        let len = self.capture[index].len;
        if len < 0 || s > self.src.len() {
            return Ok(None);
        }
        let len = len as usize;
        let start = self.capture[index].init;
        if self.src.len() - s < len || start.saturating_add(len) > self.src.len() {
            return Ok(None);
        }
        if self.src[start..start + len] == self.src[s..s + len] {
            Ok(Some(s + len))
        } else {
            Ok(None)
        }
    }

    fn class_end(&self, p: usize) -> mlua::Result<usize> {
        match self.pat_byte(p) {
            b'%' => {
                if p + 1 >= self.pat.len() {
                    return Err(mlua::Error::runtime("malformed pattern (ends with '%')"));
                }
                Ok(p + 2)
            }
            b'[' => {
                let mut index = p + 1;
                if self.pat.get(index) == Some(&b'^') {
                    index += 1;
                }
                loop {
                    if index >= self.pat.len() {
                        return Err(mlua::Error::runtime("malformed pattern (missing ']')"));
                    }
                    if self.pat[index] == b'%' && index + 1 < self.pat.len() {
                        index += 2;
                    } else {
                        index += 1;
                    }
                    if self.pat.get(index).copied() == Some(b']') {
                        return Ok(index + 1);
                    }
                }
            }
            _ => Ok(p + 1),
        }
    }

    fn single(&self, s: usize, p: usize, ep: usize) -> bool {
        if s >= self.src.len() {
            return false;
        }
        let byte = self.src[s];
        match self.pat_byte(p) {
            b'.' => true,
            b'%' => match_class(byte, self.pat_byte(p + 1)),
            b'[' => self.match_bracket(byte, p, ep.saturating_sub(1)),
            _ => self.pat_byte(p) == byte,
        }
    }

    fn match_bracket(&self, byte: u8, mut p: usize, end: usize) -> bool {
        let mut accept = true;
        if self.pat_byte(p + 1) == b'^' {
            accept = false;
            p += 1;
        }
        loop {
            p += 1;
            if p >= end {
                break;
            }
            if self.pat_byte(p) == b'%' {
                p += 1;
                if match_class(byte, self.pat_byte(p)) {
                    return accept;
                }
            } else if self.pat_byte(p + 1) == b'-' && p + 2 < end {
                let start = self.pat_byte(p);
                p += 2;
                let last = self.pat_byte(p);
                if start <= byte && byte <= last {
                    return accept;
                }
            } else if self.pat_byte(p) == byte {
                return accept;
            }
        }
        !accept
    }

    fn capture_values(&self, from: usize, to: usize) -> mlua::Result<Vec<Value>> {
        if self.level == 0 {
            return Ok(vec![self.slice_string(from, to)?]);
        }
        let mut values = Vec::with_capacity(self.level);
        for index in 0..self.level {
            values.push(self.one_capture(index, from, to)?);
        }
        Ok(values)
    }

    fn one_capture(&self, index: usize, from: usize, to: usize) -> mlua::Result<Value> {
        if index >= self.level {
            if index != 0 {
                return Err(mlua::Error::runtime(format!(
                    "invalid capture index %{}",
                    index + 1
                )));
            }
            return self.slice_string(from, to);
        }
        let capture = self.capture[index];
        if capture.len == CAP_UNFINISHED {
            return Err(mlua::Error::runtime("unfinished capture"));
        }
        if capture.len == CAP_POSITION {
            return Ok(Value::Integer(capture.init as i64 + 1));
        }
        let len = usize::try_from(capture.len).unwrap_or(0);
        self.slice_string(capture.init, capture.init.saturating_add(len))
    }

    fn slice_string(&self, from: usize, to: usize) -> mlua::Result<Value> {
        let end = to.min(self.src.len());
        let start = from.min(end);
        Ok(Value::String(
            self.lua.create_string(&self.src[start..end])?,
        ))
    }

    fn add_value(
        &self,
        buf: &mut Vec<u8>,
        from: usize,
        to: usize,
        repl: &Repl,
    ) -> mlua::Result<bool> {
        match repl {
            Repl::Bytes(text) => {
                self.add_replacement(buf, from, to, text)?;
                Ok(true)
            }
            Repl::Func(function) => {
                let args = MultiValue::from_vec(self.capture_values(from, to)?);
                let value: Value = function.call(args)?;
                self.add_repl_value(buf, from, to, &value)
            }
            Repl::Table(table) => {
                let key = self.one_capture(0, from, to)?;
                let value: Value = table.get(key)?;
                self.add_repl_value(buf, from, to, &value)
            }
        }
    }

    fn add_repl_value(
        &self,
        buf: &mut Vec<u8>,
        from: usize,
        to: usize,
        value: &Value,
    ) -> mlua::Result<bool> {
        if matches!(value, Value::Nil | Value::Boolean(false)) {
            self.write_span(buf, from, to)?;
            return Ok(false);
        }
        let Some(text) = self.lua.coerce_string(value.clone())? else {
            return Err(mlua::Error::runtime(format!(
                "invalid replacement value (a {})",
                type_name(value)
            )));
        };
        self.write_bytes(buf, &text.as_bytes())?;
        Ok(true)
    }

    fn add_replacement(
        &self,
        buf: &mut Vec<u8>,
        from: usize,
        to: usize,
        repl: &[u8],
    ) -> mlua::Result<()> {
        let mut index = 0;
        while index < repl.len() {
            if repl[index] != b'%' {
                let start = index;
                while index < repl.len() && repl[index] != b'%' {
                    index += 1;
                }
                self.write_bytes(buf, &repl[start..index])?;
                continue;
            }
            index += 1;
            if index >= repl.len() {
                return Err(mlua::Error::runtime(
                    "invalid use of '%' in replacement string",
                ));
            }
            match repl[index] {
                b'%' => self.write_bytes(buf, b"%")?,
                b'0' => self.write_span(buf, from, to)?,
                digit @ b'1'..=b'9' => {
                    let value = self.one_capture((digit - b'1') as usize, from, to)?;
                    self.append_capture_text(buf, &value)?;
                }
                _ => {
                    return Err(mlua::Error::runtime(
                        "invalid use of '%' in replacement string",
                    ));
                }
            }
            index += 1;
        }
        Ok(())
    }

    fn append_capture_text(&self, buf: &mut Vec<u8>, value: &Value) -> mlua::Result<()> {
        match value {
            Value::String(text) => self.write_bytes(buf, &text.as_bytes()),
            Value::Integer(number) => self.write_bytes(buf, number.to_string().as_bytes()),
            Value::Number(number) => self.write_bytes(buf, number.to_string().as_bytes()),
            _ => Err(mlua::Error::runtime(
                "invalid use of '%' in replacement string",
            )),
        }
    }

    fn pat_byte(&self, index: usize) -> u8 {
        self.pat.get(index).copied().unwrap_or(0)
    }

    fn src_byte(&self, index: usize) -> u8 {
        self.src.get(index).copied().unwrap_or(0)
    }
}

fn memory_error() -> mlua::Error {
    mlua::Error::runtime("not enough memory")
}

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Nil => "nil",
        Value::Boolean(_) => "boolean",
        Value::Integer(_) | Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Table(_) => "table",
        Value::Function(_) => "function",
        Value::Thread(_) => "thread",
        Value::LightUserData(_) | Value::UserData(_) | Value::Error(_) | Value::Other(_) => {
            "userdata"
        }
    }
}

fn match_class(byte: u8, class: u8) -> bool {
    let subject = i32::from(byte);
    let symbol = i32::from(class);
    let folded = unsafe { tolower(symbol) } as u8;
    let matched = match folded {
        b'a' => unsafe { isalpha(subject) != 0 },
        b'c' => unsafe { iscntrl(subject) != 0 },
        b'd' => unsafe { isdigit(subject) != 0 },
        b'g' => unsafe { isgraph(subject) != 0 },
        b'l' => unsafe { islower(subject) != 0 },
        b'p' => unsafe { ispunct(subject) != 0 },
        b's' => unsafe { isspace(subject) != 0 },
        b'u' => unsafe { isupper(subject) != 0 },
        b'w' => unsafe { isalnum(subject) != 0 },
        b'x' => unsafe { isxdigit(subject) != 0 },
        b'z' => byte == 0,
        _ => return byte == class,
    };
    if unsafe { islower(symbol) != 0 } {
        matched
    } else {
        !matched
    }
}

unsafe extern "C" {
    fn isalpha(c: i32) -> i32;
    fn iscntrl(c: i32) -> i32;
    fn isdigit(c: i32) -> i32;
    fn isgraph(c: i32) -> i32;
    fn islower(c: i32) -> i32;
    fn ispunct(c: i32) -> i32;
    fn isspace(c: i32) -> i32;
    fn isupper(c: i32) -> i32;
    fn isalnum(c: i32) -> i32;
    fn isxdigit(c: i32) -> i32;
    fn tolower(c: i32) -> i32;
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use mlua::{Lua, Value};

    use super::{CpuWatch, PATTERN_CAP, SUBJECT_CAP, install};

    struct Open {
        aborted: AtomicBool,
    }

    impl CpuWatch for Open {
        fn deadline_ms(&self) -> u64 {
            0
        }
        fn elapsed_ms(&self) -> u64 {
            0
        }
        fn aborted(&self) -> bool {
            self.aborted.load(Ordering::Relaxed)
        }
        fn abort(&self, _: i64) {
            self.aborted.store(true, Ordering::Relaxed);
        }
    }

    struct Trip {
        aborted: AtomicBool,
    }

    impl CpuWatch for Trip {
        fn deadline_ms(&self) -> u64 {
            1
        }
        fn elapsed_ms(&self) -> u64 {
            1
        }
        fn aborted(&self) -> bool {
            self.aborted.load(Ordering::Relaxed)
        }
        fn abort(&self, _: i64) {
            self.aborted.store(true, Ordering::Relaxed);
        }
    }

    fn run(script: &str, ours: bool) -> Result<String, String> {
        let lua = Lua::new();
        if ours {
            let watch = std::sync::Arc::new(Open {
                aborted: AtomicBool::new(false),
            });
            install(&lua, watch).map_err(|err| err.to_string())?;
        }
        lua.load(script)
            .eval::<String>()
            .map_err(|err| err.to_string())
    }

    const SHOW: &str = r##"
local function show(...)
  local n = select("#", ...)
  local o = {}
  for i = 1, n do
    local v = select(i, ...)
    local tv = type(v)
    if tv == "string" then
      o[i] = "s:" .. v
    elseif tv == "number" then
      o[i] = "n:" .. string.format("%.17g", v)
    else
      o[i] = tv
    end
  end
  return table.concat(o, "|")
end
"##;

    #[test]
    fn matches_lua_5_4() {
        let cases = [
            r#"return show(string.find("hello", "ell"))"#,
            r#"return show(string.find("hello", "l+"))"#,
            r#"return show(string.find("hello", "(l+)"))"#,
            r#"return show(string.find("hello", "x"))"#,
            r#"return show(string.find("abc", ""))"#,
            r#"return show(string.find("abc", "", 3))"#,
            r#"return show(string.find("abc", "b", 2))"#,
            r#"return show(string.find("abc", "b", -1))"#,
            r#"return show(string.find("abc", "b", -10))"#,
            r#"return show(string.find("abc", "c", 5))"#,
            r#"return show(string.find("abc", "b", "2"))"#,
            r#"return show(string.find("abc", "^b"))"#,
            r#"return show(string.find("abc", "^a"))"#,
            r#"return show(string.find("abc", "c$"))"#,
            r#"return show(string.find("abc", "a$"))"#,
            r#"return show(string.find("ba", "a$"))"#,
            r#"return show(string.find("a.b", ".", 1, true))"#,
            r#"return show(string.find("a.b", ".", 1, false))"#,
            r#"return show(string.find("a.b", ".", 1, 0))"#,
            r#"return show(string.find("abc", "a?"))"#,
            r#"return show(string.find("aaa", "a*"))"#,
            r#"return show(string.find("aaa", "a+"))"#,
            r#"return show(string.find("aaa", "a-"))"#,
            r#"return show(string.find("abc", "[b-c]+"))"#,
            r#"return show(string.find("abc", "[^a]+"))"#,
            r#"return show(string.find("]", "[]x]"))"#,
            r#"return show(string.find("a1b", "%d+"))"#,
            r#"return show(string.find("a b", "%s"))"#,
            r#"return show(string.find("Ab", "%l"))"#,
            r#"return show(string.find("ABC", "%u+"))"#,
            r#"return show(string.find("ab", "%w+"))"#,
            r#"return show(string.find("!", "%p"))"#,
            r#"return show(string.find("%d", "%%d"))"#,
            r#"return show(string.find("(ab)", "%b()"))"#,
            r#"return show(string.find("xaby", "%f[%w]"))"#,
            r#"return show(string.find("a+", "%f[%w]+"))"#,
            r#"return show(string.find("abc", "()b"))"#,
            r#"return show(string.find("abba", "(.)%1"))"#,
            r#"return show(string.find("hello", "()", 3))"#,
            r#"return show(string.find("a" .. string.char(0) .. "b", "a%zb"))"#,
            r#"return show(string.find("a" .. string.char(0) .. "b", "a" .. string.char(0), 1, true))"#,
            r#"return show(string.find(123, 2))"#,
            r#"return show(("hello"):find("ell"))"#,
            r#"return show(string.find(string.rep("a", 300), string.rep("a", 300)))"#,
            r#"return show(string.find(string.rep("b", 64), "x"))"#,
            r#"return show(string.match("hello", "l+"))"#,
            r#"return show(string.match("hello", "(l)(l)"))"#,
            r#"return show(string.match("hello", "()l"))"#,
            r#"return show(string.match("abc", "^b"))"#,
            r#"return show(string.match("abc", "b$"))"#,
            r#"return show(string.match("abc", ""))"#,
            r#"return (function()
                local t = {}
                for a, b in string.gmatch("a1bb2", "(%a+)(%d+)") do
                  t[#t+1] = a .. ":" .. b
                end
                return table.concat(t, ",")
              end)()"#,
            r#"return (function()
                local n = 0
                for _ in string.gmatch("ab", "") do n = n + 1 end
                return tostring(n)
              end)()"#,
            r#"return (function()
                local t = {}
                for s in string.gmatch("^a^a", "^") do t[#t+1] = s end
                return table.concat(t, ",")
              end)()"#,
            r#"return show(string.gsub("ab", "", "X"))"#,
            r#"return show(string.gsub("abc", "b", "B"))"#,
            r#"return show(string.gsub("abc", "b", "%0%0"))"#,
            r#"return show(string.gsub("ab", "(a)", "%1x"))"#,
            r#"return show(string.gsub("abc", "b", "%%"))"#,
            r#"return show(string.gsub("abc", "x", "y"))"#,
            r#"return show(string.gsub("abc", "b", "%1"))"#,
            r#"return show(string.gsub(123, "2", "9"))"#,
            r#"return show(string.gsub(123, "x", "y"))"#,
            r#"return show(string.gsub("ab", "b", function(s) return string.upper(s) end))"#,
            r#"return show(string.gsub("ab", "b", function() return nil end))"#,
            r#"return show(string.gsub("ab", "b", function() return false end))"#,
            r#"return show(string.gsub("ab", "b", function() return 12 end))"#,
            r#"return show(string.gsub("ab", "b", { b = "B" }))"#,
            r#"return show(string.gsub("ab", "b", { b = false }))"#,
            r#"return show(string.gsub("ab", "b", {}))"#,
            r#"return show(string.gsub("abc", "b", 1.5))"#,
            r#"return show(string.gsub("abc", "b", 10))"#,
            r#"return show(string.gsub("abc", "b", 1.0))"#,
            r#"return show(string.gsub("a b", "()", "%1"))"#,
            r#"return show(string.gsub("aaa", "a", "b", 2))"#,
            r#"return show(string.gsub("abc", "^a", "X"))"#,
            r#"return show(string.gsub("abc", "[b]", "X"))"#,
            r#"return (function()
                local t = {}
                for i = 0, 255 do
                  local s = string.char(i)
                  for _, pat in ipairs({"%a","%A","%c","%C","%d","%D","%g","%G","%l","%L","%p","%P","%s","%S","%u","%U","%w","%W","%x","%X","%z","%Z"}) do
                    t[#t+1] = string.find(s, pat) and "1" or "0"
                  end
                end
                return table.concat(t)
              end)()"#,
        ];
        for case in cases {
            let script = format!("{SHOW}\n{case}");
            let builtin = run(&script, false);
            let ported = run(&script, true);
            assert_eq!(ported, builtin, "{case}\nbuiltin={builtin:?}");
        }
    }

    #[test]
    fn malformed_patterns_stay_catchable() {
        let cases = [
            r#"return string.find("a", "[")"#,
            r#"return string.find("a", "%")"#,
            r#"return string.find("a", "%b")"#,
            r#"return string.find("a", "%bx")"#,
            r#"return string.find("a", "%f")"#,
            r#"return string.find("a", "%fa")"#,
            r#"return string.find("a", "%1")"#,
            r#"return string.find("a", "())")"#,
            r#"return string.gsub("a", "a", "%")"#,
            r#"return string.gsub("a", "(a", "%1")"#,
            r#"return string.gsub("a", "a", true)"#,
            r#"return string.find(nil, "a")"#,
            r#"return string.match(string.rep("a", 250), string.rep("a?", 250))"#,
            r#"return string.find("a", string.rep("(", 40))"#,
        ];
        for case in cases {
            let builtin = run(case, false);
            let ported = run(case, true);
            assert!(builtin.is_err(), "{case}");
            assert!(ported.is_err(), "{case} ported={ported:?}");
            assert!(
                !ported.as_ref().unwrap_err().contains("脚本超时"),
                "{case} {ported:?}"
            );
        }
    }

    #[test]
    fn step_clock_aborts_without_waiting_a_second() {
        let lua = Lua::new();
        let watch = std::sync::Arc::new(Trip {
            aborted: AtomicBool::new(false),
        });
        install(&lua, std::sync::Arc::clone(&watch)).unwrap();
        let err = lua
            .load(r#"return string.find(string.rep("b", 8000), ".*x")"#)
            .eval::<Value>()
            .unwrap_err();
        assert!(err.to_string().contains("脚本超时"), "{err}");
        assert!(watch.aborted());
        let err = lua
            .load(r#"return string.find("a", "a")"#)
            .eval::<Value>()
            .unwrap_err();
        assert!(err.to_string().contains("脚本超时"), "{err}");
    }

    #[test]
    fn plain_search_also_counts_steps() {
        let lua = Lua::new();
        let watch = std::sync::Arc::new(Trip {
            aborted: AtomicBool::new(false),
        });
        install(&lua, watch).unwrap();
        let err = lua
            .load(r#"return string.find(string.rep("a", 50000), string.rep("a", 80) .. "b", 1, true)"#)
            .eval::<Value>()
            .unwrap_err();
        assert!(err.to_string().contains("脚本超时"), "{err}");
    }

    #[test]
    fn length_caps_reject_without_aborting() {
        let lua = Lua::new();
        let watch = std::sync::Arc::new(Open {
            aborted: AtomicBool::new(false),
        });
        install(&lua, std::sync::Arc::clone(&watch)).unwrap();
        let err = lua
            .load(format!(
                "return string.find('a', string.rep('a?', {}))",
                (PATTERN_CAP / 2) + 2
            ))
            .eval::<Value>()
            .unwrap_err();
        assert!(err.to_string().contains("模式过于复杂"), "{err}");
        assert!(!watch.aborted());
        let err = lua
            .load(format!(
                "local a = string.rep('a', {half}) return string.find(a .. a, 'b')",
                half = (SUBJECT_CAP / 2) + 8
            ))
            .eval::<Value>()
            .unwrap_err();
        assert!(err.to_string().contains("文本过长"), "{err}");
        assert!(!watch.aborted());
        let text: String = lua
            .load(r#"return string.find(string.rep("b", 65536), "x") == nil and "nil" or "hit""#)
            .eval()
            .unwrap();
        assert_eq!(text, "nil");
    }

    #[test]
    fn gsub_result_stays_inside_the_memory_budget() {
        let lua = Lua::new();
        let watch = std::sync::Arc::new(Open {
            aborted: AtomicBool::new(false),
        });
        install(&lua, std::sync::Arc::clone(&watch)).unwrap();
        let started = std::time::Instant::now();
        let err = lua
            .load(r#"return string.gsub(string.rep("a", 700), "", string.rep("x", 512 * 1024))"#)
            .eval::<Value>()
            .unwrap_err();
        assert!(
            started.elapsed().as_millis() < 2_000,
            "结果预算应在扩容前拦住，实际 {} ms",
            started.elapsed().as_millis()
        );
        let message = err.to_string();
        assert!(message.contains("not enough memory"), "{message}");
        assert!(!message.contains("脚本超时"), "{message}");
        assert!(!watch.aborted());
        let text: String = lua
            .load(r#"return (string.gsub("ab", "", "X"))"#)
            .eval()
            .unwrap();
        assert_eq!(text, "XaXbX");
    }

    #[test]
    fn gsub_copy_checks_the_deadline_every_64kib() {
        let lua = Lua::new();
        let watch = std::sync::Arc::new(Trip {
            aborted: AtomicBool::new(false),
        });
        install(&lua, std::sync::Arc::clone(&watch)).unwrap();
        let below: i64 = lua
            .load(r#"return #(string.gsub("a", "a", string.rep("x", 65535)))"#)
            .eval()
            .unwrap();
        assert_eq!(below, 65_535);
        assert!(!watch.aborted());
        let started = std::time::Instant::now();
        let err = lua
            .load(r#"return string.gsub("a", "a", string.rep("x", 65536))"#)
            .eval::<Value>()
            .unwrap_err();
        assert!(
            started.elapsed().as_millis() < 1_000,
            "64 KiB 复制应立刻对照截止时间，实际 {} ms",
            started.elapsed().as_millis()
        );
        assert!(err.to_string().contains("脚本超时"), "{err}");
        assert!(watch.aborted());
    }

    #[test]
    fn nested_gsub_buffers_share_one_budget() {
        let lua = Lua::new();
        let watch = std::sync::Arc::new(Open {
            aborted: AtomicBool::new(false),
        });
        install(&lua, std::sync::Arc::clone(&watch)).unwrap();
        let started = std::time::Instant::now();
        let err = lua
            .load(
                r#"
                local piece = string.rep("x", 4 * 1024 * 1024)
                local level = 0
                local function repl(c)
                  if c == "x" then return piece end
                  level = level + 1
                  if level >= 3 then return "z" end
                  return (string.gsub("xy", ".", repl))
                end
                return string.gsub("xy", ".", repl)
                "#,
            )
            .eval::<Value>()
            .unwrap_err();
        assert!(
            started.elapsed().as_millis() < 2_000,
            "嵌套结果应在预算内停下，实际 {} ms",
            started.elapsed().as_millis()
        );
        let message = err.to_string();
        assert!(message.contains("not enough memory"), "{message}");
        assert!(!message.contains("脚本超时"), "{message}");
        assert!(!watch.aborted());
        let text: String = lua
            .load(r#"return (string.gsub("ab", "", "X"))"#)
            .eval()
            .unwrap();
        assert_eq!(text, "XaXbX");
    }

    #[test]
    fn recursive_gsub_stops_before_the_rust_stack_overflows() {
        for limit in [48, 100] {
            let lua = Lua::new();
            let watch = std::sync::Arc::new(Open {
                aborted: AtomicBool::new(false),
            });
            install(&lua, std::sync::Arc::clone(&watch)).unwrap();
            let started = std::time::Instant::now();
            let text: String = lua
                .load(format!(
                    r#"
                    local n = 0
                    local function repl()
                      n = n + 1
                      if n >= {limit} then return "z" end
                      return (string.gsub("a", "a", repl))
                    end
                    local ok, err = pcall(function() string.gsub("a", "a", repl) end)
                    return (ok and "true" or "false") .. ":" .. n .. ":" .. tostring(err)
                    "#
                ))
                .eval()
                .unwrap();
            assert!(
                started.elapsed().as_millis() < 2_000,
                "深度 {limit} 不应撑爆栈，实际 {} ms",
                started.elapsed().as_millis()
            );
            assert!(text.starts_with("false:8:"), "{limit} {text}");
            assert!(text.contains("模式调用过深"), "{limit} {text}");
            assert!(!text.contains("脚本超时"), "{limit} {text}");
            assert!(!watch.aborted());
        }
    }
}
