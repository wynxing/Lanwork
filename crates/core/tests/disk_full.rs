//! 用小容量固定 VHD 把磁盘写满，确认失败时原文件不变。
#![cfg(windows)]

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use lanwork_core::storage::{DocumentId, ExampleDocument, Store, StorePaths};

#[test]
fn disk_full_keeps_the_original_file() {
    let temp = std::env::temp_dir().join(format!("lanwork-vhd-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp);
    std::fs::create_dir_all(&temp).unwrap();
    let vhd_path = temp.join("full.vhd");
    let mount = temp.join("mnt");
    std::fs::create_dir_all(&mount).unwrap();
    let _guard = VhdGuard {
        vhd: vhd_path.clone(),
        mount: mount.clone(),
        temp: temp.clone(),
    };

    let script = format!(
        "create vdisk file=\"{}\" maximum=64 type=fixed\r\n\
         select vdisk file=\"{}\"\r\n\
         attach vdisk\r\n\
         create partition primary\r\n\
         select partition 1\r\n\
         format fs=ntfs quick\r\n\
         assign mount=\"{}\"\r\n",
        vhd_path.display(),
        vhd_path.display(),
        mount.display()
    );
    let output = run_diskpart(&script);
    let probe = mount.join("probe.txt");
    if let Err(err) = std::fs::write(&probe, b"ok") {
        panic!(
            "volume is not writable: {err}\n{}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
    std::fs::remove_file(&probe).unwrap();

    let store = Store::open(StorePaths {
        data_dir: mount.join("Lanwork"),
        cache_dir: temp.join("cache"),
        user_profile: temp.join("profile"),
        local_app_data: temp.join("local"),
    })
    .unwrap();
    let doc = DocumentId::Note("kept".into());
    let original = ExampleDocument {
        schema_version: 1,
        id: "kept".into(),
        title: "original-v1".into(),
        pinned: false,
    };
    store.write_json(&doc, &original).unwrap();
    let path = store.document_path(&doc).unwrap();
    let before = std::fs::read(&path).unwrap();

    fill_volume(&mount.join("filler.bin"));
    let huge = ExampleDocument {
        schema_version: 1,
        id: "kept".into(),
        title: "x".repeat(2 * 1024 * 1024),
        pinned: false,
    };
    let err = store.write_json(&doc, &huge).unwrap_err();
    assert!(err.to_string().contains("写入失败"), "{err}");
    assert!(
        err.to_string().contains(&path.display().to_string()),
        "{err}"
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(
        store
            .read_json::<ExampleDocument>(&doc)
            .unwrap()
            .unwrap()
            .title,
        "original-v1"
    );
    assert!(!path.with_file_name("kept.json.tmp").exists());
}

fn fill_volume(path: &Path) {
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
        .unwrap();
    let chunk = vec![0u8; 1024 * 1024];
    loop {
        match file.write(&chunk) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => break,
        }
    }
    let one = [0u8; 1];
    loop {
        match file.write(&one) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
    }
    let _ = file.flush();
}

fn run_diskpart(script: &str) -> std::process::Output {
    let script_path =
        std::env::temp_dir().join(format!("lanwork-diskpart-{}.txt", std::process::id()));
    std::fs::write(&script_path, script).unwrap();
    let child = Command::new("diskpart")
        .arg("/s")
        .arg(&script_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("diskpart");
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let _ = sender.send(child.wait_with_output());
    });
    let output = receiver
        .recv_timeout(Duration::from_secs(90))
        .expect("diskpart timed out")
        .expect("diskpart output");
    let _ = std::fs::remove_file(&script_path);
    output
}

struct VhdGuard {
    vhd: PathBuf,
    mount: PathBuf,
    temp: PathBuf,
}

impl Drop for VhdGuard {
    fn drop(&mut self) {
        let script = format!(
            "select vdisk file=\"{}\"\r\ndetach vdisk\r\n",
            self.vhd.display()
        );
        let _ = run_diskpart(&script);
        let _ = std::fs::remove_dir_all(&self.mount);
        let _ = std::fs::remove_file(&self.vhd);
        let _ = std::fs::remove_dir_all(&self.temp);
    }
}
