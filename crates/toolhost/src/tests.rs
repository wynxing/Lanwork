#![allow(clippy::field_reassign_with_default)]

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::host::{Options, Session};
use crate::http::{HttpResponse, HttpTransport, Outgoing, RustlsTransport};
use crate::manifest;
use crate::secret::{MemorySecretStore, SecretStore};

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "lanwork-toolhost-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("时钟")
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).expect("临时目录");
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl std::ops::Deref for Scratch {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

fn write_tool(dir: &Path, manifest: &str, script: &str) {
    std::fs::write(dir.join("manifest.toml"), manifest).expect("清单");
    std::fs::write(dir.join("main.lua"), script).expect("脚本");
}

fn text_widgets(ids: &[&str]) -> String {
    ids.iter()
        .map(|id| format!("[[component]]\nid = \"{id}\"\ncomponent = \"text\"\n"))
        .collect()
}

fn tool(id: &str, services: &str, extra: &str, components: &str) -> String {
    format!(
        "id = \"{id}\"\nname = \"示例\"\ndescription = \"说明\"\nentry = \"main.lua\"\nservices = [{services}]\n{extra}\n{components}\n"
    )
}

fn open_at(dir: &Path, options: Options) -> Result<Session, String> {
    Session::open(dir, options)
}

fn caption(session: &Session, id: &str) -> String {
    session
        .widget(id)
        .unwrap_or_else(|| panic!("没有组件 {id}"))
        .caption
}

struct Recorded {
    log: Mutex<Vec<Outgoing>>,
    status: u16,
    body: String,
}

impl HttpTransport for Recorded {
    fn send(&self, request: &Outgoing) -> Result<HttpResponse, crate::http::TransportError> {
        self.log.lock().expect("记录").push(request.clone());
        Ok(HttpResponse {
            status: self.status,
            body: self.body.clone(),
            headers: Vec::new(),
        })
    }
}

fn recorded(body: &str) -> Arc<Recorded> {
    Arc::new(Recorded {
        log: Mutex::new(Vec::new()),
        status: 200,
        body: body.to_string(),
    })
}

#[test]
fn guide_manifest_and_rejections() {
    let (text, script) = crate::guide::quota().expect("额度示例");
    assert!(script.contains("function on_load"));
    let (parsed, entry) = manifest::parse(&text).expect("清单");
    assert_eq!(parsed.id, "example.api-quota");
    assert_eq!(parsed.name, "API 额度");
    assert_eq!(entry, "main.lua");
    assert!(parsed.services.contains("http"));
    assert!(parsed.services.contains("secret"));
    assert!(parsed.services.contains("timer"));
    let ids: Vec<_> = parsed
        .widgets
        .iter()
        .map(|widget| widget.id.as_str())
        .collect();
    assert_eq!(ids, ["summary", "used"]);
    for id in ["lanwork.api-quota", "Lanwork.tool", "LANWORK.x"] {
        let reserved = text.replace("id = \"example.api-quota\"", &format!("id = \"{id}\""));
        let reserved = manifest::parse(&reserved).expect_err("保留前缀");
        assert_eq!(reserved, "id 不能以 lanwork. 开头");
    }
    let dotted = text.replace("id = \"example.api-quota\"", "id = \".quota\"");
    let dotted = manifest::parse(&dotted).expect_err("点开头");
    assert!(dotted.contains("不能以 . 开头"));

    let versioned = text.replace(
        "entry = \"main.lua\"\n",
        "entry = \"main.lua\"\nversion = 1\n",
    );
    let unknown = manifest::parse(&versioned).expect_err("版本字段");
    assert!(unknown.contains("不认识的字段"));

    let ordered = tool(
        "example.order",
        "",
        "",
        "[[component]]\nid = \"b\"\ncomponent = \"text\"\n\n[[component]]\nid = \"a\"\ncomponent = \"text\"\n",
    );
    let (ordered, _) = manifest::parse(&ordered).expect("顺序");
    assert_eq!(ordered.widgets[0].id, "b");
    assert_eq!(ordered.widgets[1].id, "a");

    let bad_component = tool(
        "example.bad",
        "",
        "",
        "[[component]]\nid = \"x\"\ncomponent = \"web\"\n",
    );
    assert!(
        manifest::parse(&bad_component)
            .expect_err("组件")
            .contains("不认识的组件")
    );

    let bad_service = tool("example.svc", "\"clipboard\"", "", &text_widgets(&["out"]));
    assert!(
        manifest::parse(&bad_service)
            .expect_err("服务")
            .contains("不认识的服务")
    );

    let ip = tool(
        "example.ip",
        "\"http\"",
        "[http]\ndomains = [\"127.0.0.1\"]\n",
        &text_widgets(&["out"]),
    );
    assert!(manifest::parse(&ip).expect_err("ip").contains("主机名"));

    let no_domains = tool("example.http", "\"http\"", "", &text_widgets(&["out"]));
    assert!(
        manifest::parse(&no_domains)
            .expect_err("域名")
            .contains("对不齐")
    );

    let empty_domains = tool(
        "example.http",
        "\"http\"",
        "[http]\ndomains = []\n",
        &text_widgets(&["out"]),
    );
    assert!(
        manifest::parse(&empty_domains)
            .expect_err("空域名")
            .contains("没有主机名")
    );

    let secret_mismatch = tool("example.key", "\"secret\"", "", &text_widgets(&["out"]));
    assert!(
        manifest::parse(&secret_mismatch)
            .expect_err("密钥")
            .contains("对不齐")
    );
}

#[test]
fn sandbox_blocks_escape_and_reports_timeout_line() {
    let dir = Scratch::new("sandbox");
    write_tool(
        &dir,
        &tool(
            "example.sandbox",
            "",
            "",
            &text_widgets(&["left", "ver", "ok", "loaded"]),
        ),
        r#"
function on_load()
  local forbidden = {"load","loadfile","dofile","require","collectgarbage","print","warn","rawlen","os","io","package","debug","coroutine","module"}
  local left = {}
  for _, name in ipairs(forbidden) do
    if _G[name] ~= nil then
      table.insert(left, name)
    end
  end
  ui.text("left", table.concat(left, ","))
  ui.text("ver", tostring(_VERSION))
  local dumped = string.dump(function() return 1 end)
  local ran = pcall(function() load(dumped) end)
  ui.text("loaded", tostring(ran))
  local ready = type(assert) == "function" and type(pcall) == "function" and type(math) == "table" and type(utf8) == "table" and type(string) == "table"
  ui.text("ok", tostring(ready))
end
"#,
    );
    let session = open_at(&dir, Options::default()).expect("打开");
    session.on_load();
    assert_eq!(caption(&session, "left"), "");
    assert!(caption(&session, "ver").contains("5.4"));
    assert_eq!(caption(&session, "loaded"), "false");
    assert_eq!(caption(&session, "ok"), "true");
    assert!(session.notices().is_empty());

    let dir = Scratch::new("timeout");
    write_tool(
        &dir,
        &tool("example.timeout", "", "", &text_widgets(&["out"])),
        "function on_load()\n  while true do\n  end\nend\n",
    );
    let session = open_at(&dir, Options::default()).expect("打开");
    let started = Instant::now();
    session.on_load();
    let elapsed = started.elapsed();
    assert!(
        elapsed.as_millis() >= 500 && elapsed.as_millis() < 10_000,
        "超时应落在 1 秒附近，实际 {} ms",
        elapsed.as_millis()
    );
    let notice = session.notices().last().cloned().expect("超时提示");
    assert!(notice.text.contains("脚本超时"), "{}", notice.text);
    assert!(notice.line.is_some(), "{}", notice.text);

    let dir = Scratch::new("pcall-timeout");
    write_tool(
        &dir,
        &tool("example.pcall", "", "", &text_widgets(&["out"])),
        "function on_load()\n  ui.text(\"out\", \"began\")\n  while true do\n    pcall(function()\n      while true do end\n    end)\n  end\nend\n",
    );
    let session = open_at(&dir, Options::default()).expect("打开");
    let started = Instant::now();
    session.on_load();
    let elapsed = started.elapsed();
    assert!(
        elapsed.as_millis() < 8_000,
        "pcall 不能把超时留住，实际 {} ms",
        elapsed.as_millis()
    );
    assert_eq!(caption(&session, "out"), "began");
    let notice = session.notices().last().cloned().expect("超时提示");
    assert!(notice.text.contains("脚本超时"), "{}", notice.text);

    let dir = Scratch::new("xpcall-timeout");
    write_tool(
        &dir,
        &tool("example.xpcall", "", "", &text_widgets(&["out"])),
        "function on_load()\n  ui.text(\"out\", \"began\")\n  xpcall(function() while true do end end, function() while true do end end)\n  ui.text(\"out\", \"after\")\nend\n",
    );
    let session = open_at(&dir, Options::default()).expect("打开");
    let started = Instant::now();
    session.on_load();
    let elapsed = started.elapsed();
    assert!(
        elapsed.as_millis() >= 500 && elapsed.as_millis() < 8_000,
        "xpcall 的两个死循环都应被 1 秒拦住，实际 {} ms",
        elapsed.as_millis()
    );
    assert_eq!(caption(&session, "out"), "began");
    let notice = session.notices().last().cloned().expect("超时提示");
    assert!(notice.text.contains("脚本超时"), "{}", notice.text);

    let dir = Scratch::new("xpcall-handler");
    write_tool(
        &dir,
        &tool("example.xpcallh", "", "", &text_widgets(&["out"])),
        "function on_load()\n  ui.text(\"out\", \"began\")\n  xpcall(function() error(\"x\") end, function() while true do end end)\n  ui.text(\"out\", \"after\")\nend\n",
    );
    let session = open_at(&dir, Options::default()).expect("打开");
    let started = Instant::now();
    session.on_load();
    let elapsed = started.elapsed();
    assert!(
        elapsed.as_millis() >= 500 && elapsed.as_millis() < 8_000,
        "处理函数里的死循环也应被拦住，实际 {} ms",
        elapsed.as_millis()
    );
    assert_eq!(caption(&session, "out"), "began");
    assert!(
        session
            .notices()
            .last()
            .is_some_and(|notice| notice.text.contains("脚本超时"))
    );

    let dir = Scratch::new("xpcall-ok");
    write_tool(
        &dir,
        &tool("example.xpcallok", "", "", &text_widgets(&["out"])),
        r#"
function on_load()
  local ok, saw = xpcall(function() error("boom") end, function() return "saw" end)
  local ok2, sum = xpcall(function(a, b) return a + b end, function() return "no" end, 2, 3)
  ui.text("out", tostring(ok) .. ":" .. tostring(saw) .. ":" .. tostring(ok2) .. ":" .. tostring(sum))
end
"#,
    );
    let session = open_at(&dir, Options::default()).expect("打开");
    session.on_load();
    assert_eq!(caption(&session, "out"), "false:saw:true:5");
    assert!(session.notices().is_empty());
}

#[test]
fn string_probes_return_without_running_the_matcher() {
    let dir = Scratch::new("string-probe");
    write_tool(
        &dir,
        &tool("example.strings", "", "", &text_widgets(&["out", "plain"])),
        r#"
function on_load()
  local probes = {
    function() string.find(string.rep("a", 30), string.rep("a?", 30)) end,
    function() string.match(string.rep("a", 30), string.rep("a?", 30)) end,
    function()
      for _ in string.gmatch(string.rep("a", 30), string.rep("a?", 30)) do end
    end,
    function() string.gsub(string.rep("a", 30), string.rep("a?", 30), "b") end,
    function() string.rep("a", 1e8) end,
    function() string.rep("a", "2000000") end,
    function() string.rep("a", "2000000", "-") end,
    function() string.find("a", "[") end,
    function() string.find("a", "%") end,
    function() string.gsub("a", "a", "%") end,
    function() string.find("a", string.rep("a?", 5000)) end,
    function()
      local a = string.rep("a", 600000)
      string.find(a .. a, "b")
    end,
  }
  local flags = {}
  for _, probe in ipairs(probes) do
    table.insert(flags, tostring(pcall(probe)))
  end
  ui.text("out", table.concat(flags, ",") .. ":done")
  local at = string.find("hello", "ell")
  local rep = string.rep("ab", 3)
  local reps = string.rep("ab", "3")
  local seps = string.rep("ab", "3", "-")
  local method = ("hello"):find("ell")
  local swapped = string.gsub("a", "a", "b")
  local tail = string.find("ba", "a$")
  ui.text("plain", tostring(at) .. ":" .. rep .. ":" .. reps .. ":" .. seps .. ":" .. tostring(method) .. ":" .. swapped .. ":" .. tostring(tail))
end
"#,
    );
    let session = open_at(&dir, Options::default()).expect("打开");
    let started = Instant::now();
    session.on_load();
    let elapsed = started.elapsed();
    assert!(
        elapsed.as_millis() < 2_000,
        "能马上结束的探测不应拖过 2 秒，实际 {} ms",
        elapsed.as_millis()
    );
    assert_eq!(
        caption(&session, "out"),
        "true,true,true,true,false,false,false,false,false,false,false,false:done"
    );
    assert_eq!(caption(&session, "plain"), "2:ababab:ababab:ab-ab-ab:2:b:2");
    assert!(session.notices().is_empty());
}

fn pattern_deadline(name: &str, body: &str) -> Session {
    let dir = Scratch::new(name);
    write_tool(&dir, &tool(name, "", "", &text_widgets(&["out"])), body);
    let session = open_at(&dir, Options::default()).expect("打开");
    let started = Instant::now();
    session.on_load();
    let elapsed = started.elapsed();
    assert!(
        elapsed.as_millis() >= 500 && elapsed.as_millis() < 10_000,
        "{name} 应在 1 秒附近停下，实际 {} ms",
        elapsed.as_millis()
    );
    let notice = session.notices().last().cloned().expect("超时提示");
    assert!(notice.text.contains("脚本超时"), "{}", notice.text);
    session
}

#[test]
fn unanchored_dot_star_on_64kib_times_out() {
    let session = pattern_deadline(
        "example.dotstar",
        r#"
function on_load()
  ui.text("out", "began")
  local anchored = string.find(string.rep("b", 65536), "^.*x")
  ui.text("out", "anchored:" .. tostring(anchored))
  pcall(function()
    string.find(string.rep("b", 65536), ".*x")
  end)
  ui.text("out", "after")
end
"#,
    );
    assert_eq!(caption(&session, "out"), "anchored:nil");
}

#[test]
fn unanchored_match_gmatch_and_gsub_time_out() {
    let session = pattern_deadline(
        "example.dotmatch",
        r#"
function on_load()
  ui.text("out", "began")
  pcall(function()
    string.match(string.rep("b", 65536), ".*x")
  end)
  ui.text("out", "after")
end
"#,
    );
    assert_eq!(caption(&session, "out"), "began");

    let session = pattern_deadline(
        "example.dotgmatch",
        r#"
function on_load()
  ui.text("out", "began")
  pcall(function()
    for _ in string.gmatch(string.rep("b", 65536), ".*x") do end
  end)
  ui.text("out", "after")
end
"#,
    );
    assert_eq!(caption(&session, "out"), "began");

    let session = pattern_deadline(
        "example.dotgsub",
        r#"
function on_load()
  ui.text("out", "began")
  pcall(function()
    string.gsub(string.rep("b", 65536), ".*x", "z")
  end)
  ui.text("out", "after")
end
"#,
    );
    assert_eq!(caption(&session, "out"), "began");
}

#[test]
fn xpcall_cannot_keep_a_pattern_timeout() {
    let session = pattern_deadline(
        "example.dotxpcall",
        r#"
function on_load()
  ui.text("out", "began")
  xpcall(function()
    string.find(string.rep("b", 65536), ".*x")
  end, function()
    ui.text("out", "handler")
  end)
  ui.text("out", "after")
end
"#,
    );
    assert_eq!(caption(&session, "out"), "began");
}

#[test]
fn optional_dollar_probe_does_not_run_away() {
    let dir = Scratch::new("dollar-probe");
    write_tool(
        &dir,
        &tool("example.dollar", "", "", &text_widgets(&["out"])),
        r#"
function on_load()
  ui.text("out", "began")
  pcall(function()
    string.find(string.rep("$", 26), string.rep("$?", 26) .. string.rep("$", 26))
  end)
  ui.text("out", "after")
end
"#,
    );
    let session = open_at(&dir, Options::default()).expect("打开");
    let started = Instant::now();
    session.on_load();
    let elapsed = started.elapsed();
    assert!(
        elapsed.as_millis() < 10_000,
        "$? 探测不应超过截止时间很久，实际 {} ms",
        elapsed.as_millis()
    );
    let timed_out = session
        .notices()
        .last()
        .is_some_and(|notice| notice.text.contains("脚本超时"));
    if timed_out {
        assert!(
            elapsed.as_millis() >= 500,
            "超时不应是长度预检立刻拒绝，实际 {} ms",
            elapsed.as_millis()
        );
        assert_eq!(caption(&session, "out"), "began");
    } else {
        assert_eq!(caption(&session, "out"), "after");
        assert!(session.notices().is_empty());
    }
}

#[test]
fn gsub_result_over_the_budget_stays_catchable() {
    let dir = Scratch::new("gsub-budget");
    write_tool(
        &dir,
        &tool("example.gsubbudget", "", "", &text_widgets(&["out"])),
        r#"
function on_load()
  ui.text("out", "began")
  local ok, err = pcall(function()
    string.gsub(string.rep("a", 700), "", string.rep("x", 512 * 1024))
  end)
  ui.text("out", (ok and "true" or "false") .. ":" .. tostring(err) .. ":done")
end
"#,
    );
    let session = open_at(&dir, Options::default()).expect("打开");
    let started = Instant::now();
    session.on_load();
    let elapsed = started.elapsed();
    assert!(
        elapsed.as_millis() < 2_000,
        "结果超过临时内存预算时应马上返回，实际 {} ms",
        elapsed.as_millis()
    );
    let caption = caption(&session, "out");
    assert!(caption.contains("false"), "{caption}");
    assert!(caption.contains("not enough memory"), "{caption}");
    assert!(caption.contains(":done"), "{caption}");
    assert!(!caption.contains("脚本超时"), "{caption}");
    assert!(session.notices().is_empty());
}

#[test]
fn recursive_gsub_xpcall_and_sort_stay_bounded() {
    let dir = Scratch::new("reentry");
    write_tool(
        &dir,
        &tool("example.reentry", "", "", &text_widgets(&["out"])),
        r#"
function on_load()
  local function gsub_depth(limit)
    local n = 0
    local function repl()
      n = n + 1
      if n >= limit then return "z" end
      return (string.gsub("a", "a", repl))
    end
    local ok, err = pcall(function() string.gsub("a", "a", repl) end)
    return (ok and "true" or "false") .. ":" .. n .. ":" .. tostring(err)
  end
  local function xp_depth()
    local n = 0
    local saved = ""
    local function f()
      n = n + 1
      return xpcall(f, function(err)
        saved = tostring(err)
        return saved
      end)
    end
    pcall(f)
    local hit = string.find(saved, "调用过深", 1, true) and "hit" or "miss"
    return n .. ":" .. hit
  end
  local function xp_bomb()
    local n = 0
    local function f()
      n = n + 1
      return xpcall(f, f)
    end
    pcall(f)
    return tostring(n)
  end
  local function sort_depth()
    local n = 0
    local function cmp()
      n = n + 1
      if n > 100 then return false end
      table.sort({2, 1}, cmp)
      return false
    end
    local ok, err = pcall(function() table.sort({2, 1}, cmp) end)
    local saw = tostring(err)
    local hit = string.find(saw, "调用过深", 1, true) and "hit" or "miss"
    return (ok and "true" or "false") .. ":" .. n .. ":" .. hit
  end
  ui.text("out", "g48:" .. gsub_depth(48)
    .. "|g100:" .. gsub_depth(100)
    .. "|xp:" .. xp_depth()
    .. "|bomb:" .. xp_bomb()
    .. "|sort:" .. sort_depth()
    .. ":done")
end
"#,
    );
    let session = open_at(&dir, Options::default()).expect("打开");
    let started = Instant::now();
    session.on_load();
    let elapsed = started.elapsed();
    assert!(
        elapsed.as_millis() < 2_000,
        "递归 gsub、xpcall、sort 应在重入上限停下，实际 {} ms",
        elapsed.as_millis()
    );
    let caption = caption(&session, "out");
    assert!(caption.contains("g48:false:8:"), "{caption}");
    assert!(caption.contains("g100:false:8:"), "{caption}");
    assert!(caption.contains("模式调用过深"), "{caption}");
    assert!(caption.contains("xp:17:hit"), "{caption}");
    assert!(caption.contains("|sort:false:16:hit"), "{caption}");
    assert!(caption.contains(":done"), "{caption}");
    assert!(!caption.contains("脚本超时"), "{caption}");
    assert!(session.notices().is_empty());
    let bomb = caption
        .split("|bomb:")
        .nth(1)
        .and_then(|rest| rest.split("|sort:").next())
        .and_then(|text| text.parse::<i32>().ok())
        .expect("bomb 次数");
    assert!(
        (1..80).contains(&bomb),
        "xpcall 自递归应在展开时停住，实际 {bomb} 次"
    );
}

#[test]
fn sort_without_comparator_rejects_a_huge_table() {
    let dir = Scratch::new("sort-cap");
    write_tool(
        &dir,
        &tool("example.sort", "", "", &text_widgets(&["out"])),
        r#"
function on_load()
  local t = {3, 1, 2}
  table.sort(t)
  local desc = {3, 1, 2}
  table.sort(desc, function(a, b) return a > b end)
  local big = {}
  for i = 1, 100001 do
    big[i] = 1
  end
  local ok = pcall(table.sort, big)
  ui.text("out", table.concat(t, "") .. ":" .. table.concat(desc, "") .. ":" .. tostring(ok) .. ":done")
end
"#,
    );
    let session = open_at(&dir, Options::default()).expect("打开");
    let started = Instant::now();
    session.on_load();
    assert!(started.elapsed().as_millis() < 2_000);
    assert_eq!(caption(&session, "out"), "123:321:false:done");
    assert!(session.notices().is_empty());
}

#[test]
fn timer_limits_and_unauthorized_http() {
    let dir = Scratch::new("timer");
    write_tool(
        &dir,
        &tool("example.timer", "\"timer\"", "", &text_widgets(&["out"])),
        r#"
local n = 0
local after = 0
function on_load()
  timer.every(1, function()
    n = n + 1
  end)
  timer.every(30, function()
    n = n + 1
    ui.text("out", tostring(n) .. ":" .. tostring(after))
  end)
  timer.after(0, function() end)
  timer.after(1, function()
    after = 1
    ui.text("out", tostring(n) .. ":" .. tostring(after))
  end)
end
"#,
    );
    let session = open_at(&dir, Options::default()).expect("打开");
    session.on_load();
    let notes: Vec<_> = session
        .notices()
        .iter()
        .map(|note| note.text.clone())
        .collect();
    assert!(notes.iter().any(|text| text == "间隔过短"), "{notes:?}");
    assert!(notes.iter().any(|text| text == "参数无效"), "{notes:?}");
    assert_eq!(caption(&session, "out"), "");
    session.advance(1.0);
    assert_eq!(caption(&session, "out"), "0:1");
    session.advance(28.0);
    assert_eq!(caption(&session, "out"), "0:1");
    session.advance(1.0);
    assert_eq!(caption(&session, "out"), "1:1");

    let dir = Scratch::new("deny");
    let transport = recorded("{}");
    write_tool(
        &dir,
        &tool("example.deny", "", "", &text_widgets(&["out"])),
        r#"
function on_load()
  http.request({ method = "GET", url = "https://api.example.com/v1" }, function()
    ui.text("out", "called")
  end)
end
"#,
    );
    let mut options = Options::default();
    options.transport = transport.clone();
    let session = open_at(&dir, options).expect("打开");
    session.on_load();
    assert_eq!(session.http_sends(), 0);
    assert!(transport.log.lock().unwrap().is_empty());
    assert_eq!(caption(&session, "out"), "");
    assert!(
        session
            .notices()
            .iter()
            .any(|notice| notice.text == "未授权")
    );
}

#[test]
fn quota_example_hides_secret_and_limits_hosts() {
    let secret = "lw-raw-secret-9f3a";
    let dir = Scratch::new("quota");
    crate::guide::write_example(&dir, "quota").unwrap();
    let transport = recorded(r#"{"used":20,"limit":100}"#);
    let store = Rc::new(RefCell::new(MemorySecretStore::default()));
    store
        .borrow_mut()
        .put("example.api-quota", "api_key", secret)
        .unwrap();
    let mut options = Options::default();
    options.store = store;
    options.transport = transport.clone();
    let session = open_at(&dir, options).unwrap();
    session.on_load();
    assert_eq!(caption(&session, "summary"), "已用 20 / 100");
    assert!((session.widget("used").unwrap().progress - 0.2).abs() < 1e-9);
    assert_eq!(session.http_sends(), 1);
    let sent = transport.log.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    let expected = format!("Bearer {secret}");
    assert!(
        sent[0]
            .headers
            .iter()
            .any(|(name, value)| name == "Authorization" && value == &expected)
    );
    assert!(!caption(&session, "summary").contains(secret));
    assert!(
        session
            .notices()
            .iter()
            .all(|note| !note.text.contains(secret))
    );
    session.advance(299.0);
    assert_eq!(session.http_sends(), 1);
    session.advance(1.0);
    assert_eq!(session.http_sends(), 2);

    let dir = Scratch::new("missing");
    crate::guide::write_example(&dir, "quota").unwrap();
    let transport = recorded("{}");
    let mut options = Options::default();
    options.transport = transport.clone();
    let session = open_at(&dir, options).unwrap();
    session.on_load();
    assert_eq!(caption(&session, "summary"), "未填写密钥");
    assert_eq!(session.http_sends(), 0);
    assert!(transport.log.lock().unwrap().is_empty());

    let dir = Scratch::new("gate");
    let transport = recorded("{}");
    write_tool(
        &dir,
        &tool(
            "example.api-quota",
            "\"http\", \"secret\"",
            "[http]\ndomains = [\"api.example.com\"]\n\n[[secret]]\nid = \"api_key\"\nlabel = \"API 密钥\"\nheader = \"Authorization\"\nprefix = \"Bearer \"\n",
            &text_widgets(&["out", "kind"]),
        ),
        r#"
function on_load()
  local lines = {}
  local function ask(options)
    http.request(options, function(result)
      local blob = result.error or "ok"
      blob = blob .. tostring(result.header) .. tostring(result.secret)
      table.insert(lines, blob)
      ui.text("out", table.concat(lines, "|"))
    end)
  end
  ask({ method = "POST", url = "https://api.example.com/v1", secret = { id = "api_key", header = "Authorization", prefix = "Bearer " } })
  ask({ method = "GET", url = "http://api.example.com/v1" })
  ask({ method = "GET", url = "https://evil.api.example.com/v1" })
  ask({ method = "GET", url = "https://user:pw@api.example.com/v1" })
  ask({ method = "GET", url = "http://api.example.com/v1", secret = { id = "api_key", header = "Authorization", prefix = "Bearer " } })
  ui.text("kind", type(secret) .. ":" .. type(secret.read))
end
"#,
    );
    let store = Rc::new(RefCell::new(MemorySecretStore::default()));
    store
        .borrow_mut()
        .put("example.api-quota", "api_key", secret)
        .unwrap();
    let mut options = Options::default();
    options.store = store;
    options.transport = transport.clone();
    let session = open_at(&dir, options).unwrap();
    session.on_load();
    let text = caption(&session, "out");
    assert!(text.contains("不在范围内"), "{text}");
    assert!(text.contains("不允许明文 http"), "{text}");
    assert!(text.contains("域名未允许"), "{text}");
    assert!(text.contains("地址无效"), "{text}");
    assert!(!text.contains(secret), "{text}");
    assert_eq!(caption(&session, "kind"), "table:nil");
    assert_eq!(session.http_sends(), 0);
}

#[test]
fn countdown_example_counts_down_after_the_button() {
    let dir = Scratch::new("countdown");
    crate::guide::write_example(&dir, "countdown").unwrap();
    let session = open_at(&dir, Options::default()).unwrap();
    session.on_load();
    assert_eq!(caption(&session, "status"), "准备");
    let clock = session.widget("clock").unwrap();
    assert_eq!(clock.seconds, 0);
    assert!(!clock.running);
    session.click("start");
    let clock = session.widget("clock").unwrap();
    assert!(clock.running);
    assert_eq!(clock.seconds, 60);
    assert_eq!(caption(&session, "status"), "倒数");
    session.advance(59.0);
    assert_eq!(session.widget("clock").unwrap().seconds, 1);
    assert_eq!(caption(&session, "status"), "倒数");
    session.advance(1.0);
    assert_eq!(caption(&session, "status"), "到了");
    let clock = session.widget("clock").unwrap();
    assert_eq!(clock.seconds, 0);
    assert!(!clock.running);
}

#[test]
fn response_echo_is_redacted_before_lua() {
    let secret = "a b/c";
    let dir = Scratch::new("echo");
    write_tool(
        &dir,
        &tool(
            "example.echo",
            "\"http\", \"secret\"",
            "[http]\ndomains = [\"api.example.com\"]\n\n[[secret]]\nid = \"api_key\"\nlabel = \"API 密钥\"\nheader = \"Authorization\"\nprefix = \"Bearer \"\n",
            &text_widgets(&["out"]),
        ),
        r#"
function on_load()
  http.request({
    method = "GET",
    url = "https://api.example.com/v1",
    secret = { id = "api_key", header = "Authorization", prefix = "Bearer " },
  }, function(result)
    local blob = table.concat({
      tostring(result.ok),
      result.body or "",
      result.error or "",
      tostring(result.status),
    }, "|")
    for k, v in pairs(result) do
      blob = blob .. "\n" .. tostring(k) .. "=" .. tostring(v)
    end
    ui.text("out", blob)
  end)
end
"#,
    );
    let echoed = "a b/c YSBiL2M= YSBiL2M a%20b%2Fc a%20b%2fc a+b%2Fc Bearer a b/c";
    let transport = Arc::new(EchoTransport {
        body: echoed.to_string(),
        header: format!("Bearer {secret}"),
    });
    let store = Rc::new(RefCell::new(MemorySecretStore::default()));
    store
        .borrow_mut()
        .put("example.echo", "api_key", secret)
        .unwrap();
    let mut options = Options::default();
    options.store = store;
    options.transport = transport;
    let session = open_at(&dir, options).unwrap();
    session.on_load();
    let text = caption(&session, "out");
    assert!(text.starts_with("false||请求失败|200"), "{text}");
    for pattern in [
        secret,
        "YSBiL2M=",
        "YSBiL2M",
        "a%20b%2Fc",
        "a%20b%2fc",
        "a+b%2Fc",
    ] {
        assert!(!text.contains(pattern), "{pattern} still in {text}");
    }
    assert!(
        session
            .notices()
            .iter()
            .any(|notice| notice.text == "请求失败")
    );
}

#[test]
fn percent_encoded_secret_blanks_the_body() {
    let secret = "lw-raw-secret-9f3a";
    let full: String = secret.bytes().map(|byte| format!("%{byte:02x}")).collect();
    let partial = format!("%{:02X}{}", secret.as_bytes()[0], &secret[1..]);
    let mixed: String = secret
        .bytes()
        .enumerate()
        .map(|(index, byte)| {
            if index % 2 == 0 {
                format!("%{byte:02X}")
            } else {
                format!("%{byte:02x}")
            }
        })
        .collect();
    for (name, body, header) in [
        ("full", full, "x-clean".to_string()),
        ("partial", partial, "x-clean".to_string()),
        ("mixed", mixed, "x-clean".to_string()),
        (
            "header",
            r#"{"used":20,"limit":100}"#.to_string(),
            format!("Bearer {secret}"),
        ),
    ] {
        let text = echo_caption(&format!("pct-{name}"), &body, &header, secret);
        assert_eq!(text, "false||请求失败|200", "{name}: {text}");
        assert!(!text.contains(secret), "{name}");
    }
    let clean = echo_caption("pct-clean", r#"{"used":20,"limit":100}"#, "ok", secret);
    assert_eq!(clean, r#"true|{"used":20,"limit":100}||200"#);
}

#[test]
fn short_secret_base64_blanks_the_body() {
    let text = echo_caption("short-b64", "eHl6", "x-clean", "xyz");
    assert_eq!(text, "false||请求失败|200", "{text}");
    assert!(!text.contains("xyz"));
    assert!(!text.contains("eHl6"));
}

fn echo_caption(name: &str, body: &str, header: &str, secret: &str) -> String {
    let dir = Scratch::new(name);
    write_tool(
        &dir,
        &tool(
            "example.echo",
            "\"http\", \"secret\"",
            "[http]\ndomains = [\"api.example.com\"]\n\n[[secret]]\nid = \"api_key\"\nlabel = \"API 密钥\"\nheader = \"Authorization\"\nprefix = \"Bearer \"\n",
            &text_widgets(&["out"]),
        ),
        r#"
function on_load()
  http.request({
    method = "GET",
    url = "https://api.example.com/v1",
    secret = { id = "api_key", header = "Authorization", prefix = "Bearer " },
  }, function(result)
    ui.text("out", table.concat({
      tostring(result.ok),
      result.body or "",
      result.error or "",
      tostring(result.status),
    }, "|"))
  end)
end
"#,
    );
    let transport = Arc::new(EchoTransport {
        body: body.to_string(),
        header: header.to_string(),
    });
    let store = Rc::new(RefCell::new(MemorySecretStore::default()));
    store
        .borrow_mut()
        .put("example.echo", "api_key", secret)
        .unwrap();
    let mut options = Options::default();
    options.store = store;
    options.transport = transport;
    let session = open_at(&dir, options).unwrap();
    session.on_load();
    assert!(
        session
            .notices()
            .iter()
            .all(|notice| !notice.text.contains(secret))
    );
    if body.contains(secret) || header.contains(secret) || header != "ok" {
        assert!(
            session
                .notices()
                .iter()
                .any(|notice| notice.text == "请求失败"),
            "{name}"
        );
    }
    caption(&session, "out")
}

struct EchoTransport {
    body: String,
    header: String,
}

impl HttpTransport for EchoTransport {
    fn send(&self, _request: &Outgoing) -> Result<HttpResponse, crate::http::TransportError> {
        Ok(HttpResponse {
            status: 200,
            body: self.body.clone(),
            headers: vec![("X-Echo".to_string(), self.header.clone())],
        })
    }
}

#[test]
fn http_callback_does_not_block_on_transport() {
    let dir = Scratch::new("slow-http");
    write_tool(
        &dir,
        &tool(
            "example.slow",
            "\"http\"",
            "[http]\ndomains = [\"api.example.com\"]\n",
            &text_widgets(&["phase", "out"]),
        ),
        r#"
function on_load()
  ui.text("phase", "before")
  http.request({ method = "GET", url = "https://api.example.com/v1" }, function(result)
    ui.text("out", (result.body or "") .. (result.error or ""))
  end)
  ui.text("phase", "after")
end
"#,
    );
    let pair = Arc::new((Mutex::new(false), Condvar::new()));
    let transport = Arc::new(SlowTransport {
        pair: Arc::clone(&pair),
    });
    let mut options = Options::default();
    options.transport = transport;
    options.http_budget = Duration::from_millis(200);
    let session = open_at(&dir, options).unwrap();
    let started = Instant::now();
    session.on_load();
    let elapsed = started.elapsed();
    {
        let (lock, cv) = &*pair;
        *lock.lock().expect("锁") = true;
        cv.notify_one();
    }
    assert!(
        elapsed < Duration::from_millis(800),
        "脚本回调不应等网络，实际 {} ms",
        elapsed.as_millis()
    );
    assert_eq!(caption(&session, "phase"), "after");
    assert_eq!(caption(&session, "out"), "请求失败");
    assert!(!caption(&session, "out").contains("late"));
}

struct SlowTransport {
    pair: Arc<(Mutex<bool>, Condvar)>,
}

impl HttpTransport for SlowTransport {
    fn send(&self, _request: &Outgoing) -> Result<HttpResponse, crate::http::TransportError> {
        let (lock, cv) = &*self.pair;
        let guard = lock.lock().expect("锁");
        let (guard, _wait) = cv
            .wait_timeout_while(guard, Duration::from_secs(2), |ready| !*ready)
            .expect("等待");
        drop(guard);
        Ok(HttpResponse {
            status: 200,
            body: "late".to_string(),
            headers: Vec::new(),
        })
    }
}

#[test]
fn failing_secret_read_does_not_panic() {
    let dir = Scratch::new("bad-store");
    write_tool(
        &dir,
        &tool(
            "example.bad-store",
            "\"http\", \"secret\"",
            "[http]\ndomains = [\"api.example.com\"]\n\n[[secret]]\nid = \"api_key\"\nlabel = \"API 密钥\"\nheader = \"Authorization\"\nprefix = \"Bearer \"\n",
            &text_widgets(&["out"]),
        ),
        r#"
function on_load()
  http.request({
    method = "GET",
    url = "https://api.example.com/v1",
    secret = { id = "api_key", header = "Authorization", prefix = "Bearer " },
  }, function(result)
    ui.text("out", result.error or "ok")
  end)
end
"#,
    );
    let mut options = Options::default();
    options.store = Rc::new(RefCell::new(FailingStore));
    let session = open_at(&dir, options).unwrap();
    session.on_load();
    assert_eq!(caption(&session, "out"), "请求失败");
    assert_eq!(session.http_sends(), 0);
    assert!(session.notices().iter().any(|note| note.text == "请求失败"));
}

struct FailingStore;

impl SecretStore for FailingStore {
    fn put(&mut self, _tool_id: &str, _secret_id: &str, _value: &str) -> Result<(), String> {
        Err("不写".to_string())
    }

    fn read(&self, _tool_id: &str, _secret_id: &str) -> Result<Option<String>, String> {
        Err("凭据读取失败".to_string())
    }

    fn remove(&mut self, _tool_id: &str, _secret_id: &str) -> Result<(), String> {
        Err("不删".to_string())
    }
}

#[test]
fn caught_allocation_still_aborts() {
    let dir = Scratch::new("memory");
    write_tool(
        &dir,
        &tool("example.memory", "", "", &text_widgets(&["out"])),
        r#"
function on_load()
  local t = {}
  while true do
    pcall(function()
      t[#t + 1] = string.rep("x", 200000)
    end)
  end
end
"#,
    );
    let session = open_at(&dir, Options::default()).expect("打开");
    let started = Instant::now();
    session.on_load();
    assert!(
        started.elapsed().as_millis() < 8_000,
        "分配失败被 pcall 接住后不应一直跑，实际 {} ms",
        started.elapsed().as_millis()
    );
    let notice = session.notices().last().cloned().expect("中止提示");
    assert!(notice.text.contains("脚本超时"), "{}", notice.text);
}

#[test]
fn localhost_tls_attaches_secret_header() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use std::time::Duration;

    use rustls::ServerConfig;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};

    let secret = "lw-tls-secret-44c1";
    crate::http::install_ring();
    let certified =
        rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).expect("证书");
    let cert_der = CertificateDer::from(certified.cert.der().to_vec());
    let key_der = PrivateKeyDer::Pkcs8(certified.key_pair.serialize_der().into());
    let server_config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert_der.clone()], key_der)
        .expect("服务端");
    let server_config = Arc::new(server_config);
    let mut roots = rustls::RootCertStore::empty();
    roots.add(cert_der).expect("信任");
    let client = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let listener = TcpListener::bind("127.0.0.1:0").expect("端口");
    let port = listener.local_addr().expect("地址").port();
    let seen = Arc::new(Mutex::new(String::new()));
    let seen_thread = Arc::clone(&seen);
    let server = thread::spawn(move || {
        let (tcp, _) = listener.accept().expect("接入");
        tcp.set_read_timeout(Some(Duration::from_secs(5))).ok();
        tcp.set_write_timeout(Some(Duration::from_secs(5))).ok();
        let connection = rustls::ServerConnection::new(server_config).expect("握手");
        let mut tls = rustls::StreamOwned::new(connection, tcp);
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            match tls.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    if buf.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let body = r#"{"used":20,"limit":100}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = tls.write_all(response.as_bytes());
        *seen_thread.lock().expect("记录") = String::from_utf8_lossy(&buf).into_owned();
    });

    let dir = Scratch::new("tls");
    let (manifest, script) = crate::guide::quota().unwrap();
    let manifest = manifest.replace("api.example.com", "localhost");
    let script = script.replace(
        "https://api.example.com/v1/quota",
        &format!("https://localhost:{port}/v1/quota"),
    );
    write_tool(&dir, &manifest, &script);
    let store = Rc::new(RefCell::new(MemorySecretStore::default()));
    store
        .borrow_mut()
        .put("example.api-quota", "api_key", secret)
        .unwrap();
    let mut options = Options::default();
    options.store = store;
    options.transport = Arc::new(RustlsTransport::with_config(client));
    let session = open_at(&dir, options).unwrap();
    session.on_load();
    server.join().expect("服务线程");
    let request = seen.lock().unwrap().clone();
    assert!(
        request.contains(&format!("Authorization: Bearer {secret}")),
        "{request}"
    );
    assert_eq!(caption(&session, "summary"), "已用 20 / 100");
    assert!(!caption(&session, "summary").contains(secret));
    assert!(
        session
            .notices()
            .iter()
            .all(|note| !note.text.contains(secret))
    );
    assert_eq!(session.http_sends(), 1);
}

#[cfg(windows)]
#[test]
fn windows_credential_roundtrip_and_attach() {
    use crate::secret::WindowsCredentialStore;

    let secret = "lw-wincred-secret-18ab";
    let tool_id = format!("spike.{}", std::process::id());
    let mut native = WindowsCredentialStore;
    native.put(&tool_id, "api_key", secret).expect("写入凭据");
    assert_eq!(
        native.read(&tool_id, "api_key").expect("读取").as_deref(),
        Some(secret)
    );
    let dir = Scratch::new("wincred");
    write_tool(
        &dir,
        &tool(
            &tool_id,
            "\"http\", \"secret\"",
            "[http]\ndomains = [\"api.example.com\"]\n\n[[secret]]\nid = \"api_key\"\nlabel = \"API 密钥\"\nheader = \"Authorization\"\nprefix = \"Bearer \"\n",
            &text_widgets(&["out"]),
        ),
        r#"
function on_load()
  http.request({
    method = "GET",
    url = "https://api.example.com/v1/quota",
    secret = { id = "api_key", header = "Authorization", prefix = "Bearer " },
  }, function(result)
    local blob = ""
    for k, v in pairs(result) do
      blob = blob .. tostring(k) .. "=" .. tostring(v) .. ";"
    end
    ui.text("out", blob)
  end)
end
"#,
    );
    let transport = recorded(r#"{"used":1,"limit":2}"#);
    let store = Rc::new(RefCell::new(native));
    let mut options = Options::default();
    options.store = store.clone();
    options.transport = transport.clone();
    let session = open_at(&dir, options).unwrap();
    session.on_load();
    let sent = transport.log.lock().unwrap().clone();
    let expected = format!("Bearer {secret}");
    assert!(
        sent[0]
            .headers
            .iter()
            .any(|(name, value)| name == "Authorization" && value == &expected)
    );
    assert!(!caption(&session, "out").contains(secret));
    store
        .borrow_mut()
        .remove(&tool_id, "api_key")
        .expect("删除凭据");
    assert!(
        store
            .borrow()
            .read(&tool_id, "api_key")
            .expect("再读")
            .is_none()
    );
}

#[test]
fn several_secrets_fixed_headers_and_post_stay_out_of_the_script() {
    let token = "tok-secret-a";
    let account = "acct-secret-b";
    let dir = Scratch::new("multi");
    write_tool(
        &dir,
        &tool(
            "example.multi",
            "\"http\", \"secret\"",
            r#"
[http]
domains = ["api.example.com"]
post = true
headers = [
  { name = "anthropic-beta", value = "oauth-2025-04-20" },
  { name = "User-Agent", value = "my-tool" },
]

[[secret]]
id = "token"
label = "访问令牌"
header = "Authorization"
prefix = "Bearer "

[[secret]]
id = "account"
label = "账号"
header = "ChatGPT-Account-Id"
"#,
            &text_widgets(&["out"]),
        ),
        r#"
function on_load()
  local lines = {}
  local function ask(options)
    http.request(options, function(result)
      local blob = result.error or "ok"
      blob = blob .. tostring(result.header) .. tostring(result.secret)
      table.insert(lines, blob)
      ui.text("out", table.concat(lines, "|"))
    end)
  end
  ask({
    method = "GET",
    url = "https://api.example.com/v1",
    secrets = {
      { id = "token" },
      { id = "account", header = "chatgpt-account-id", prefix = "" },
    },
  })
  ask({
    method = "POST",
    url = "https://api.example.com/v1/usage",
    body = json.encode({ n = 1 }),
    secret = { id = "token" },
  })
  ask({
    method = "POST",
    url = "https://api.example.com/v1/usage",
    body = "nope",
    secret = { id = "token" },
  })
  ask({
    method = "GET",
    url = "https://api.example.com/v1",
    secrets = { { id = "token", header = "X-Other" } },
  })
  ask({
    method = "GET",
    url = "https://api.example.com/v1",
    secret = { id = "token" },
    secrets = { { id = "account" } },
  })
end
"#,
    );
    let transport = recorded(r#"{"ok":true}"#);
    let store = Rc::new(RefCell::new(MemorySecretStore::default()));
    store
        .borrow_mut()
        .put("example.multi", "token", token)
        .unwrap();
    store
        .borrow_mut()
        .put("example.multi", "account", account)
        .unwrap();
    let mut options = Options::default();
    options.store = store;
    options.transport = transport.clone();
    let session = open_at(&dir, options).unwrap();
    session.on_load();
    let text = caption(&session, "out");
    // 拒绝在 http.request 里当场回调；发出去的两笔等脚本返回后再完成。
    assert_eq!(
        text,
        "数据无效nilnil|地址无效nilnil|地址无效nilnil|oknilnil|oknilnil"
    );
    assert!(!text.contains(token));
    assert!(!text.contains(account));
    assert!(
        session
            .notices()
            .iter()
            .all(|note| { !note.text.contains(token) && !note.text.contains(account) })
    );
    let sent = transport.log.lock().unwrap().clone();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0].method, "GET");
    assert!(sent[0].body.is_empty());
    assert!(has_header(&sent[0], "anthropic-beta", "oauth-2025-04-20"));
    assert!(has_header(&sent[0], "User-Agent", "my-tool"));
    assert!(has_header(
        &sent[0],
        "Authorization",
        &format!("Bearer {token}")
    ));
    assert!(has_header(&sent[0], "ChatGPT-Account-Id", account));
    assert!(!sent[0].url.contains(token));
    assert!(!sent[0].url.contains(account));
    let wire = String::from_utf8(crate::http::encode_http(&sent[0]).unwrap()).unwrap();
    assert_eq!(wire.matches("User-Agent:").count(), 1);
    assert!(wire.contains("User-Agent: my-tool\r\n"));
    assert_eq!(sent[1].method, "POST");
    assert_eq!(sent[1].body, br#"{"n":1}"#);
    assert!(has_header(
        &sent[1],
        "Authorization",
        &format!("Bearer {token}")
    ));
    let post = String::from_utf8(crate::http::encode_http(&sent[1]).unwrap()).unwrap();
    assert!(post.starts_with("POST /v1/usage HTTP/1.1\r\n"));
    assert_eq!(post.matches("Content-Type:").count(), 1);
    assert!(post.contains("Content-Type: application/json\r\n"));
    assert!(!sent[1].url.contains(token));
}

fn has_header(outgoing: &Outgoing, name: &str, value: &str) -> bool {
    outgoing
        .headers
        .iter()
        .any(|(key, got)| key == name && got == value)
}

#[test]
fn a_second_secret_echoed_by_the_host_is_blanked() {
    let account = "acct-echo-secret";
    let dir = Scratch::new("echo-two");
    write_tool(
        &dir,
        &tool(
            "example.echo-two",
            "\"http\", \"secret\"",
            "[http]\ndomains = [\"api.example.com\"]\n\n[[secret]]\nid = \"token\"\nlabel = \"令牌\"\nheader = \"Authorization\"\nprefix = \"Bearer \"\n\n[[secret]]\nid = \"account\"\nlabel = \"账号\"\nheader = \"ChatGPT-Account-Id\"\n",
            &text_widgets(&["out"]),
        ),
        r#"
function on_load()
  http.request({
    method = "GET",
    url = "https://api.example.com/v1",
    secrets = {
      { id = "token", header = "Authorization", prefix = "Bearer " },
      { id = "account", header = "ChatGPT-Account-Id", prefix = "" },
    },
  }, function(result)
    ui.text("out", table.concat({
      tostring(result.ok),
      result.body or "",
      result.error or "",
    }, "|"))
  end)
end
"#,
    );
    let transport = Arc::new(EchoTransport {
        body: account.to_string(),
        header: "clean".to_string(),
    });
    let store = Rc::new(RefCell::new(MemorySecretStore::default()));
    store
        .borrow_mut()
        .put("example.echo-two", "token", "tok-other")
        .unwrap();
    store
        .borrow_mut()
        .put("example.echo-two", "account", account)
        .unwrap();
    let mut options = Options::default();
    options.store = store;
    options.transport = transport;
    let session = open_at(&dir, options).unwrap();
    session.on_load();
    assert_eq!(caption(&session, "out"), "false||请求失败");
    assert!(!caption(&session, "out").contains(account));
    assert!(
        session
            .notices()
            .iter()
            .all(|note| !note.text.contains(account))
    );
}

fn b64url(bytes: &[u8]) -> String {
    const ALPHA: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    let mut index = 0;
    while index + 3 <= bytes.len() {
        let n = (u32::from(bytes[index]) << 16)
            | (u32::from(bytes[index + 1]) << 8)
            | u32::from(bytes[index + 2]);
        for shift in [18, 12, 6, 0] {
            out.push(ALPHA[((n >> shift) & 63) as usize] as char);
        }
        index += 3;
    }
    let rest = bytes.len() - index;
    if rest > 0 {
        let mut n = u32::from(bytes[index]) << 16;
        if rest == 2 {
            n |= u32::from(bytes[index + 1]) << 8;
        }
        out.push(ALPHA[((n >> 18) & 63) as usize] as char);
        out.push(ALPHA[((n >> 12) & 63) as usize] as char);
        if rest == 2 {
            out.push(ALPHA[((n >> 6) & 63) as usize] as char);
        }
    }
    out
}

fn future_access_token() -> String {
    let header = b64url(br#"{"alg":"none"}"#);
    let payload = b64url(br#"{"exp":4102444800}"#);
    format!("{header}.{payload}.sig")
}

fn usage_body() -> String {
    r#"{"plan_type":"plus","rate_limit":{"limit_reached":false,"primary_window":{"used_percent":12.5,"limit_window_seconds":18000,"reset_at":1577836800},"secondary_window":{"used_percent":40,"limit_window_seconds":604800,"reset_at":1578441600}}}"#.to_string()
}

fn open_chatgpt(dir: &Path, transport: Arc<Recorded>, roots: crate::usage::UsageRoots) -> Session {
    crate::guide::write_example(dir, "chatgpt").unwrap();
    let mut options = Options::default();
    options.transport = transport;
    options.usage_roots = Some(roots);
    let session = open_at(dir, options).unwrap();
    session.on_load();
    session
}

#[test]
fn chatgpt_usage_example_hides_the_login_and_caches() {
    crate::usage::reset_usage_cache_for_test();
    let token = future_access_token();
    let account = "acct-leak-9f3a";
    let dir = Scratch::new("chatgpt-usage");
    let codex = dir.join("codex");
    std::fs::create_dir_all(&codex).unwrap();
    std::fs::write(
        codex.join("auth.json"),
        format!(r#"{{"tokens":{{"access_token":"{token}","account_id":"{account}"}}}}"#),
    )
    .unwrap();
    let transport = Arc::new(Recorded {
        log: Mutex::new(Vec::new()),
        status: 200,
        body: usage_body(),
    });
    let roots = crate::usage::UsageRoots {
        chatgpt: Some(codex.clone()),
        claude: Some(dir.join("claude")),
        grok: Some(dir.join("grok")),
    };
    let session = open_chatgpt(&dir, transport.clone(), roots);
    assert_eq!(caption(&session, "five"), "5小时 12.5% · 01-01 00:00");
    assert_eq!(caption(&session, "week"), "每周 40% · 01-08 00:00");
    assert!((session.widget("five_bar").unwrap().progress - 0.125).abs() < 1e-9);
    assert!((session.widget("week_bar").unwrap().progress - 0.4).abs() < 1e-9);
    let sent = transport.log.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].url, crate::usage::CHATGPT_USAGE_URL);
    assert!(
        sent[0].headers.iter().any(|(name, value)| {
            name == "Authorization" && value == &format!("Bearer {token}")
        })
    );
    assert!(
        sent[0]
            .headers
            .iter()
            .any(|(name, value)| name == "ChatGPT-Account-Id" && value == account)
    );
    let debug = format!("{:?}", sent[0]);
    assert!(debug.contains("<redacted>"), "{debug}");
    assert!(!debug.contains(&token), "{debug}");
    assert!(!debug.contains(account), "{debug}");
    for text in [caption(&session, "five"), caption(&session, "week")] {
        assert!(!text.contains(&token), "{text}");
        assert!(!text.contains(account), "{text}");
        assert!(!text.contains("chatgpt.com"), "{text}");
    }
    assert!(
        session
            .notices()
            .iter()
            .all(|note| { !note.text.contains(&token) && !note.text.contains(account) })
    );
    session.advance(300.0);
    assert_eq!(session.http_sends(), 1);
    assert_eq!(transport.log.lock().unwrap().len(), 1);
    crate::usage::age_usage_cache_for_test(Duration::from_secs(61));
    session.advance(300.0);
    assert_eq!(session.http_sends(), 2);
    assert_eq!(transport.log.lock().unwrap().len(), 2);

    crate::usage::reset_usage_cache_for_test();
    let denied = Scratch::new("chatgpt-401");
    std::fs::create_dir_all(denied.join("codex")).unwrap();
    std::fs::write(
        denied.join("codex").join("auth.json"),
        format!(r#"{{"tokens":{{"access_token":"{token}","account_id":"{account}"}}}}"#),
    )
    .unwrap();
    let transport = Arc::new(Recorded {
        log: Mutex::new(Vec::new()),
        status: 401,
        body: token.clone(),
    });
    let session = open_chatgpt(
        &denied,
        transport,
        crate::usage::UsageRoots {
            chatgpt: Some(denied.join("codex")),
            claude: Some(denied.join("claude")),
            grok: Some(denied.join("grok")),
        },
    );
    assert_eq!(caption(&session, "five"), "登录已过期");
    assert_eq!(caption(&session, "week"), "");
    assert!(!caption(&session, "five").contains(&token));
    assert!(
        session
            .notices()
            .iter()
            .all(|note| { note.text == "登录已过期" && !note.text.contains(&token) })
    );

    crate::usage::reset_usage_cache_for_test();
    let missing = Scratch::new("chatgpt-missing");
    std::fs::create_dir_all(missing.join("codex")).unwrap();
    let transport = Arc::new(Recorded {
        log: Mutex::new(Vec::new()),
        status: 200,
        body: token.clone(),
    });
    let session = open_chatgpt(
        &missing,
        transport.clone(),
        crate::usage::UsageRoots {
            chatgpt: Some(missing.join("codex")),
            claude: Some(missing.join("claude")),
            grok: Some(missing.join("grok")),
        },
    );
    assert_eq!(caption(&session, "five"), "登录已过期");
    assert!(transport.log.lock().unwrap().is_empty());
    assert_eq!(session.http_sends(), 0);
    assert!(!caption(&session, "five").contains(&token));

    crate::usage::reset_usage_cache_for_test();
    let broken = Scratch::new("chatgpt-bad-json");
    std::fs::create_dir_all(broken.join("codex")).unwrap();
    std::fs::write(
        broken.join("codex").join("auth.json"),
        format!(r#"{{"tokens":{{"access_token":"{token}","account_id":"{account}"}}}}"#),
    )
    .unwrap();
    let transport = Arc::new(Recorded {
        log: Mutex::new(Vec::new()),
        status: 200,
        body: format!("not-json {token}"),
    });
    let session = open_chatgpt(
        &broken,
        transport,
        crate::usage::UsageRoots {
            chatgpt: Some(broken.join("codex")),
            claude: Some(broken.join("claude")),
            grok: Some(broken.join("grok")),
        },
    );
    assert_eq!(caption(&session, "five"), "数据无效");
    assert!(!caption(&session, "five").contains(&token));
    assert!(!caption(&session, "five").contains(account));
    assert!(session.notices().iter().all(|note| {
        !note.text.contains(&token) && !note.text.contains(account) && note.text == "数据无效"
    }));
}

#[test]
fn relative_usage_root_is_expired_and_sends_nothing() {
    crate::usage::reset_usage_cache_for_test();
    let dir = Scratch::new("usage-relative");
    let transport = recorded(r#"{"plan_type":"plus"}"#);
    let mut options = Options::default();
    options.transport = transport.clone();
    options.usage_roots = Some(crate::usage::UsageRoots {
        chatgpt: Some(PathBuf::from("codex")),
        claude: Some(PathBuf::from(".")),
        grok: None,
    });
    crate::guide::write_example(&dir, "chatgpt").unwrap();
    let session = open_at(&dir, options).unwrap();
    session.on_load();
    assert_eq!(caption(&session, "five"), "登录已过期");
    assert_eq!(session.http_sends(), 0);
    assert!(transport.log.lock().unwrap().is_empty());
}

#[test]
fn undeclared_source_cannot_read_another_tools_cached_usage() {
    crate::usage::reset_usage_cache_for_test();
    let token = future_access_token();
    let account = "acct-leak-9f3a";
    let dir = Scratch::new("usage-owner");
    let codex = dir.join("codex");
    std::fs::create_dir_all(&codex).unwrap();
    std::fs::write(
        codex.join("auth.json"),
        format!(r#"{{"tokens":{{"access_token":"{token}","account_id":"{account}"}}}}"#),
    )
    .unwrap();
    let transport = Arc::new(Recorded {
        log: Mutex::new(Vec::new()),
        status: 200,
        body: usage_body(),
    });
    let owner = open_chatgpt(
        &dir,
        transport,
        crate::usage::UsageRoots {
            chatgpt: Some(codex),
            claude: Some(dir.join("claude")),
            grok: Some(dir.join("grok")),
        },
    );
    assert!(caption(&owner, "five").contains("12.5%"));

    let other = Scratch::new("usage-other");
    write_tool(
        &other,
        &tool(
            "example.other",
            "",
            "[[usage]]\nsource = \"claude\"\n",
            &text_widgets(&["out"]),
        ),
        r#"
function on_load()
  usage.fetch("chatgpt", function(result)
    local text = result.error or ""
    if result.plan then
      text = text .. result.plan
    end
    if type(result.windows) == "table" then
      for _, window in ipairs(result.windows) do
        text = text .. tostring(window.name) .. tostring(window.used_percent)
      end
    end
    ui.text("out", text)
  end)
end
"#,
    );
    let transport = recorded("{}");
    let mut options = Options::default();
    options.transport = transport.clone();
    let session = open_at(&other, options).unwrap();
    session.on_load();
    let text = caption(&session, "out");
    assert_eq!(text, "未授权");
    assert!(!text.contains("12.5"), "{text}");
    assert!(!text.contains("plus"), "{text}");
    assert!(!text.contains("5小时"), "{text}");
    assert!(!text.contains(&token), "{text}");
    assert_eq!(session.http_sends(), 0);
    assert!(transport.log.lock().unwrap().is_empty());
    assert!(session.notices().iter().any(|note| note.text == "未授权"));
    assert!(session.notices().iter().all(|note| {
        note.text == "未授权" && !note.text.contains("12.5") && !note.text.contains(&token)
    }));
}
