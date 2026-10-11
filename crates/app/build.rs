fn main() {
    slint_build::compile("ui/shell.slint").expect("compile ui/shell.slint");
    let manifest =
        std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let icon = manifest.join("assets/lanwork.ico");
    println!("cargo:rerun-if-changed={}", icon.display());
    // build.rs 在宿主上编译。这里看的是目标系统，交叉编译到 Windows 时也要嵌入。
    if std::env::var("CARGO_CFG_TARGET_OS").ok().as_deref() == Some("windows") {
        embed_icon_and_version(&icon);
    }
}

/// 图标是资源 ID 1。版本字符串用 Cargo 的包版本；数值四段由 winresource 从同一版本填。
fn embed_icon_and_version(icon: &std::path::Path) {
    let mut res = winresource::WindowsResource::new();
    let icon = icon.to_string_lossy().replace('\\', "/");
    res.set_icon(&icon);
    res.set("ProductName", "Lanwork");
    res.set("FileDescription", "Lanwork");
    res.set("InternalName", "lanwork");
    res.set("OriginalFilename", "lanwork.exe");
    res.set("LegalCopyright", "Copyright (c) 2026 Wynn");
    res.compile().expect("embed lanwork.ico and version");
}
