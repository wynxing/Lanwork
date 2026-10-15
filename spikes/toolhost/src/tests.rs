#![allow(clippy::field_reassign_with_default)]

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::host::{Durations, Notification, Options, Session, TodoItem};
use crate::http::{HttpResponse, HttpTransport, Outgoing, RustlsTransport};
use crate::manifest::{self, Origin};
use crate::secret::{MemorySecretStore, SecretStore};

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "lanwork-toolhost-spike-{name}-{}-{}",
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

fn todo(id: &str, title: &str, current: bool) -> TodoItem {
    TodoItem {
        id: id.to_string(),
        list_id: "inbox".to_string(),
        title: title.to_string(),
        completed: false,
        due: None,
        current,
        trashed: false,
        conflict: false,
    }
}

fn open_at(dir: &Path, origin: Origin, options: Options) -> Result<Session, String> {
    Session::open(dir, origin, options)
}

fn caption(session: &Session, id: &str) -> String {
    session
        .widget(id)
        .unwrap_or_else(|| panic!("没有组件 {id}"))
        .caption
}

fn label(session: &Session, id: &str) -> String {
    session
        .widget(id)
        .unwrap_or_else(|| panic!("没有组件 {id}"))
        .label
}

struct Recorded {
    log: Mutex<Vec<Outgoing>>,
    status: u16,
    body: String,
}

impl HttpTransport for Recorded {
    fn get(&self, request: &Outgoing) -> Result<HttpResponse, ()> {
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
    let (text, script) = crate::guide::pomodoro().expect("番茄钟示例");
    assert!(script.contains("function on_timer"));
    let (parsed, entry) = manifest::parse(&text, Origin::Builtin).expect("内置清单");
    assert_eq!(parsed.id, "lanwork.pomodoro");
    assert_eq!(parsed.name, "番茄钟");
    assert_eq!(entry, "main.lua");
    assert!(parsed.services.contains("notify"));
    assert!(parsed.services.contains("todo.read"));
    assert!(!parsed.services.contains("timer"));
    assert!(!parsed.services.contains("storage"));
    let ids: Vec<_> = parsed
        .widgets
        .iter()
        .map(|widget| widget.id.as_str())
        .collect();
    assert_eq!(ids, ["title", "status", "clock", "pause", "skip", "done"]);
    let imported = manifest::parse(&text, Origin::Imported).expect_err("导入前缀");
    assert!(imported.contains("lanwork."));

    let versioned = text.replace(
        "entry = \"main.lua\"\n",
        "entry = \"main.lua\"\nversion = 1\n",
    );
    let unknown = manifest::parse(&versioned, Origin::Builtin).expect_err("版本字段");
    assert!(unknown.contains("不认识的字段"));

    let ordered = tool(
        "example.order",
        "",
        "",
        "[[component]]\nid = \"b\"\ncomponent = \"text\"\n\n[[component]]\nid = \"a\"\ncomponent = \"text\"\n",
    );
    let (ordered, _) = manifest::parse(&ordered, Origin::Imported).expect("顺序");
    assert_eq!(ordered.widgets[0].id, "b");
    assert_eq!(ordered.widgets[1].id, "a");

    let bad_component = tool(
        "example.bad",
        "",
        "",
        "[[component]]\nid = \"x\"\ncomponent = \"web\"\n",
    );
    assert!(
        manifest::parse(&bad_component, Origin::Imported)
            .expect_err("组件")
            .contains("不认识的组件")
    );

    let bad_service = tool("example.svc", "\"clipboard\"", "", &text_widgets(&["out"]));
    assert!(
        manifest::parse(&bad_service, Origin::Imported)
            .expect_err("服务")
            .contains("不认识的服务")
    );

    let ip = tool(
        "example.ip",
        "\"http\"",
        "[http]\ndomains = [\"127.0.0.1\"]\n",
        &text_widgets(&["out"]),
    );
    assert!(
        manifest::parse(&ip, Origin::Imported)
            .expect_err("ip")
            .contains("主机名")
    );

    let no_domains = tool("example.http", "\"http\"", "", &text_widgets(&["out"]));
    assert!(
        manifest::parse(&no_domains, Origin::Imported)
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
        manifest::parse(&empty_domains, Origin::Imported)
            .expect_err("空域名")
            .contains("没有主机名")
    );

    let secret_mismatch = tool("example.key", "\"secret\"", "", &text_widgets(&["out"]));
    assert!(
        manifest::parse(&secret_mismatch, Origin::Imported)
            .expect_err("密钥")
            .contains("对不齐")
    );
}

#[test]
fn durations_out_of_range_are_rejected() {
    let dir = Scratch::new("range");
    let (manifest, script) = crate::guide::pomodoro().unwrap();
    write_tool(&dir, &manifest, &script);
    let mut options = Options::default();
    options.durations.work_min = 0;
    match open_at(&dir, Origin::Builtin, options) {
        Err(err) => assert!(err.contains("1 至 120"), "{err}"),
        Ok(_) => panic!("0 分钟应当拒绝"),
    }

    let mut options = Options::default();
    options.durations.break_min = 61;
    match open_at(&dir, Origin::Builtin, options) {
        Err(err) => assert!(err.contains("1 至 60"), "{err}"),
        Ok(_) => panic!("休息超范围应当拒绝"),
    }

    let mut options = Options::default();
    options.durations.work_min = 120;
    options.durations.break_min = 60;
    options.durations.long_min = 60;
    open_at(&dir, Origin::Builtin, options).expect("边界内");
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
    let session = open_at(&dir, Origin::Imported, Options::default()).expect("打开");
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
    let session = open_at(&dir, Origin::Imported, Options::default()).expect("打开");
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
    let session = open_at(&dir, Origin::Imported, Options::default()).expect("打开");
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
    let session = open_at(&dir, Origin::Imported, Options::default()).expect("打开");
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
    let session = open_at(&dir, Origin::Imported, options).expect("打开");
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
fn pomodoro_example_runs_end_to_end() {
    let dir = Scratch::new("pomo");
    crate::guide::write_example(&dir, "pomodoro").unwrap();
    let short = Durations {
        work_min: 1,
        break_min: 1,
        long_min: 1,
    };

    let session = open_named(&dir, "写报告", short);
    session.on_load();
    assert_eq!(caption(&session, "title"), "写报告");
    assert_eq!(caption(&session, "status"), "工作");
    assert_eq!(label(&session, "pause"), "继续");
    assert_eq!(caption(&session, "clock"), "60");
    assert!(!session.widget("clock").unwrap().running);
    session.click("pause");
    assert_eq!(label(&session, "pause"), "暂停");
    assert!(session.widget("clock").unwrap().running);
    session.advance(10.0);
    assert_eq!(caption(&session, "clock"), "50");
    session.click("pause");
    assert_eq!(label(&session, "pause"), "继续");
    session.advance(10.0);
    assert_eq!(caption(&session, "clock"), "50");
    session.click("pause");
    session.advance(50.0);
    assert_eq!(caption(&session, "status"), "休息");
    assert_eq!(caption(&session, "clock"), "60");
    assert_eq!(
        session.popped(),
        vec![Notification {
            title: "番茄钟".to_string(),
            body: "到点".to_string(),
        }]
    );
    session.on_unload();

    let session = open_named(&dir, "写报告", short);
    session.on_load();
    assert_eq!(caption(&session, "clock"), "60");
    session.click("done");
    assert!(session.popped().is_empty());
    assert_eq!(caption(&session, "status"), "休息");
    session.click("skip");
    assert_eq!(caption(&session, "status"), "工作");
    session.click("done");
    session.click("skip");
    session.click("done");
    session.click("skip");
    assert_eq!(caption(&session, "status"), "工作");
    session.click("skip");
    assert_eq!(caption(&session, "status"), "休息");
    session.click("skip");
    session.click("done");
    assert_eq!(caption(&session, "status"), "长休息");
    assert!(session.popped().is_empty());
    session.click("done");
    assert_eq!(caption(&session, "status"), "长休息");

    let session = open_named(&dir, "写报告", short);
    session.on_load();
    session.click("pause");
    session.advance(15.0);
    session.set_todos(vec![todo("t1", "改标题", true)]);
    session.todos_changed();
    assert_eq!(caption(&session, "title"), "改标题");
    assert_eq!(caption(&session, "clock"), "45");
    session.set_todos(vec![todo("other", "另一条", true)]);
    session.todos_changed();
    assert_eq!(caption(&session, "status"), "");
    assert_eq!(label(&session, "pause"), "暂停");
    assert_eq!(caption(&session, "clock"), "0");
    assert!(!session.widget("clock").unwrap().running);
    assert!(session.popped().is_empty());

    let session = open_named(&dir, "写报告", short);
    session.on_load();
    session.click("pause");
    session.advance(10.0);
    let mut cleared = todo("t1", "写报告", false);
    cleared.current = false;
    session.set_todos(vec![cleared]);
    session.todos_changed();
    assert_eq!(caption(&session, "clock"), "0");
    assert_eq!(caption(&session, "title"), "没有待办");

    let mut options = Options::default();
    options.durations = short;
    let session = open_at(&dir, Origin::Builtin, options).unwrap();
    session.on_load();
    assert_eq!(caption(&session, "title"), "没有待办");
    assert_eq!(caption(&session, "clock"), "0");
    session.click("pause");
    session.click("skip");
    session.click("done");
    assert_eq!(label(&session, "pause"), "继续");
    assert!(session.popped().is_empty());

    let mut options = Options::default();
    options.durations = short;
    options.quiet = true;
    options.todos.push(todo("t1", "写报告", true));
    let session = open_at(&dir, Origin::Builtin, options).unwrap();
    session.on_load();
    session.click("pause");
    session.advance(60.0);
    assert!(session.popped().is_empty());
    assert_eq!(
        session.deferred(),
        vec![Notification {
            title: "番茄钟".to_string(),
            body: "到点".to_string(),
        }]
    );

    let mut options = Options::default();
    options.todos.push(todo("t1", "写报告", true));
    let session = open_at(&dir, Origin::Builtin, options).unwrap();
    session.on_load();
    assert_eq!(caption(&session, "clock"), "1500");
}

fn open_named(dir: &Path, title: &str, durations: Durations) -> Session {
    let mut options = Options::default();
    options.durations = durations;
    options.todos.push(todo("t1", title, true));
    open_at(dir, Origin::Builtin, options).expect("番茄钟")
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
    let session = open_at(&dir, Origin::Imported, options).unwrap();
    session.on_load();
    assert_eq!(caption(&session, "summary"), "已用 20 / 100");
    assert!((session.widget("used").unwrap().progress - 0.2).abs() < 1e-9);
    assert_eq!(session.http_sends(), 1);
    let sent = transport.log.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    let expected = format!("Bearer {secret}");
    assert_eq!(sent[0].header_value.as_deref(), Some(expected.as_str()));
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
    let session = open_at(&dir, Origin::Imported, options).unwrap();
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
            "[http]\ndomains = [\"api.example.com\"]\n\n[[secret]]\nid = \"api_key\"\nlabel = \"API 密钥\"\n",
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
    let session = open_at(&dir, Origin::Imported, options).unwrap();
    session.on_load();
    let text = caption(&session, "out");
    assert!(text.contains("不在范围内"), "{text}");
    assert!(text.contains("明文 http（是否允许仍待定）"), "{text}");
    assert!(text.contains("域名未允许"), "{text}");
    assert!(text.contains("地址无效"), "{text}");
    assert!(!text.contains(secret), "{text}");
    assert_eq!(caption(&session, "kind"), "table:nil");
    assert_eq!(session.http_sends(), 0);
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
            "[http]\ndomains = [\"api.example.com\"]\n\n[[secret]]\nid = \"api_key\"\nlabel = \"API 密钥\"\n",
            &text_widgets(&["out"]),
        ),
        r#"
function on_load()
  http.request({
    method = "GET",
    url = "https://api.example.com/v1",
    secret = { id = "api_key", header = "Authorization", prefix = "Bearer " },
  }, function(result)
    local blob = tostring(result.body)
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
    let session = open_at(&dir, Origin::Imported, options).unwrap();
    session.on_load();
    let text = caption(&session, "out");
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
}

struct EchoTransport {
    body: String,
    header: String,
}

impl HttpTransport for EchoTransport {
    fn get(&self, _request: &Outgoing) -> Result<HttpResponse, ()> {
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
    let session = open_at(&dir, Origin::Imported, options).unwrap();
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
    fn get(&self, _request: &Outgoing) -> Result<HttpResponse, ()> {
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
            "[http]\ndomains = [\"api.example.com\"]\n\n[[secret]]\nid = \"api_key\"\nlabel = \"API 密钥\"\n",
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
    let session = open_at(&dir, Origin::Imported, options).unwrap();
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
    let session = open_at(&dir, Origin::Imported, Options::default()).expect("打开");
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
    let session = open_at(&dir, Origin::Imported, options).unwrap();
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

#[test]
fn shell_draws_text_button_timer_and_progress() {
    let _guard = crate::shell::platform_lock();
    let dir = Scratch::new("shell-pomo");
    crate::guide::write_example(&dir, "pomodoro").unwrap();
    let mut options = Options::default();
    options.todos.push(todo("t1", "写报告", true));
    let session = Rc::new(open_at(&dir, Origin::Builtin, options).unwrap());
    session.on_load();
    let ui = crate::shell::attach_for_test(Rc::clone(&session));
    assert_eq!(ui.get_row_count(), 6);
    for (index, kind) in ["text", "text", "timer", "button", "button", "button"]
        .into_iter()
        .enumerate()
    {
        assert_eq!(ui.invoke_row_kind(index as i32), kind);
    }
    assert_eq!(ui.invoke_row_caption(0).as_str(), "写报告");
    assert_eq!(ui.invoke_row_caption(1).as_str(), "工作");
    assert_eq!(ui.invoke_row_caption(2).as_str(), "1500");
    assert_eq!(ui.invoke_row_label(3).as_str(), "继续");
    let y = ui.get_drag_height() + 3.0 * ui.get_row_stride() + 22.0;
    crate::shell::click(&ui, 60.0, y);
    assert_eq!(ui.invoke_row_label(3).as_str(), "暂停");
    drop(ui);

    let dir = Scratch::new("shell-quota");
    crate::guide::write_example(&dir, "quota").unwrap();
    let transport = recorded(r#"{"used":20,"limit":100}"#);
    let store = Rc::new(RefCell::new(MemorySecretStore::default()));
    store
        .borrow_mut()
        .put("example.api-quota", "api_key", "not-shown")
        .unwrap();
    let mut options = Options::default();
    options.store = store;
    options.transport = transport;
    let session = Rc::new(open_at(&dir, Origin::Imported, options).unwrap());
    session.on_load();
    let ui = crate::shell::attach_for_test(Rc::clone(&session));
    assert_eq!(ui.invoke_row_kind(0).as_str(), "text");
    assert_eq!(ui.invoke_row_kind(1).as_str(), "progress");
    assert!((ui.invoke_row_progress(1) - 0.2).abs() < 0.001);
    let partial = crate::shell::green_pixels(&ui);
    drop(ui);

    let dir = Scratch::new("shell-full");
    write_tool(
        &dir,
        &tool(
            "example.full",
            "",
            "",
            "[[component]]\nid = \"used\"\ncomponent = \"progress\"\n",
        ),
        "function on_load()\n  ui.progress(\"used\", 1)\nend\n",
    );
    let session = Rc::new(open_at(&dir, Origin::Imported, Options::default()).unwrap());
    session.on_load();
    let ui = crate::shell::attach_for_test(session);
    let full = crate::shell::green_pixels(&ui);
    assert!(partial > 50, "进度条应画出绿色，实际 {partial}");
    assert!(full > partial * 3, "满格应明显更长：{partial} {full}");
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
            "[http]\ndomains = [\"api.example.com\"]\n\n[[secret]]\nid = \"api_key\"\nlabel = \"API 密钥\"\n",
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
    let session = open_at(&dir, Origin::Imported, options).unwrap();
    session.on_load();
    let sent = transport.log.lock().unwrap().clone();
    let expected = format!("Bearer {secret}");
    assert_eq!(sent[0].header_value.as_deref(), Some(expected.as_str()));
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
