//! 软件渲染外壳。安全测试在 `lanwork-toolhost`。

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use lanwork_toolhost::{HttpTransport, MemorySecretStore, Options, Outgoing, SecretStore, Session};

use crate::shell;

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

fn tool(id: &str, components: &str) -> String {
    format!(
        "id = \"{id}\"\nname = \"示例\"\ndescription = \"说明\"\nentry = \"main.lua\"\nservices = []\n{components}\n"
    )
}

struct Recorded {
    body: String,
}

impl HttpTransport for Recorded {
    fn get(
        &self,
        _request: &Outgoing,
    ) -> Result<lanwork_toolhost::HttpResponse, lanwork_toolhost::TransportError> {
        Ok(lanwork_toolhost::HttpResponse {
            status: 200,
            body: self.body.clone(),
            headers: Vec::new(),
        })
    }
}

#[test]
fn shell_draws_text_button_timer_and_progress() {
    let _guard = shell::platform_lock();
    let dir = Scratch::new("shell-timer");
    write_tool(
        &dir,
        &tool(
            "example.timer",
            "[[component]]\nid = \"title\"\ncomponent = \"text\"\ntext = \"示例\"\n\n[[component]]\nid = \"clock\"\ncomponent = \"timer\"\n\n[[component]]\nid = \"go\"\ncomponent = \"button\"\nlabel = \"开始\"\n",
        ),
        "function on_load()\n  ui.text(\"title\", \"示例\")\n  ui.timer(\"clock\", 30)\nend\nfunction on_click(id)\n  if id == \"go\" then\n    ui.button(\"go\", \"已点\")\n  end\nend\n",
    );
    let session = Rc::new(Session::open(&dir, Options::default()).unwrap());
    session.on_load();
    let ui = shell::attach_for_test(Rc::clone(&session));
    assert_eq!(ui.get_row_count(), 3);
    for (index, kind) in ["text", "timer", "button"].into_iter().enumerate() {
        assert_eq!(ui.invoke_row_kind(index as i32), kind);
    }
    assert_eq!(ui.invoke_row_caption(0).as_str(), "示例");
    assert_eq!(ui.invoke_row_caption(1).as_str(), "30");
    assert_eq!(ui.invoke_row_label(2).as_str(), "开始");
    let y = ui.get_drag_height() + 2.0 * ui.get_row_stride() + 22.0;
    shell::click(&ui, 60.0, y);
    assert_eq!(ui.invoke_row_label(2).as_str(), "已点");
    drop(ui);

    let dir = Scratch::new("shell-quota");
    crate::write_example(&dir, "quota").unwrap();
    let transport = Arc::new(Recorded {
        body: r#"{"used":20,"limit":100}"#.to_string(),
    });
    let store = Rc::new(RefCell::new(MemorySecretStore::default()));
    store
        .borrow_mut()
        .put("example.api-quota", "api_key", "not-shown")
        .unwrap();
    let options = Options {
        store,
        transport,
        ..Options::default()
    };
    let session = Rc::new(Session::open(&dir, options).unwrap());
    session.on_load();
    let ui = shell::attach_for_test(Rc::clone(&session));
    assert_eq!(ui.invoke_row_kind(0).as_str(), "text");
    assert_eq!(ui.invoke_row_kind(1).as_str(), "progress");
    assert!((ui.invoke_row_progress(1) - 0.2).abs() < 0.001);
    let partial = shell::green_pixels(&ui);
    drop(ui);

    let dir = Scratch::new("shell-full");
    write_tool(
        &dir,
        &tool(
            "example.full",
            "[[component]]\nid = \"used\"\ncomponent = \"progress\"\n",
        ),
        "function on_load()\n  ui.progress(\"used\", 1)\nend\n",
    );
    let session = Rc::new(Session::open(&dir, Options::default()).unwrap());
    session.on_load();
    let ui = shell::attach_for_test(session);
    let full = shell::green_pixels(&ui);
    assert!(partial > 50, "进度条应画出绿色，实际 {partial}");
    assert!(full > partial * 3, "满格应明显更长：{partial} {full}");
}
