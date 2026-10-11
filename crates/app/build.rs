fn main() {
    slint_build::compile("ui/shell.slint").expect("compile ui/shell.slint");
    let manifest =
        std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let icon = manifest.join("assets/lanwork.ico");
    println!("cargo:rerun-if-changed={}", icon.display());
    // build.rs 在宿主上编译。这里看的是目标系统，交叉编译到 Windows 时也要嵌入。
    if std::env::var("CARGO_CFG_TARGET_OS").ok().as_deref() == Some("windows") {
        embed_icon(&icon);
    }
}

/// 只把图标资源链进 exe。版本信息块不在这里写。非 Windows 目标不会调用。
fn embed_icon(icon: &std::path::Path) {
    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let rc_path = out_dir.join("lanwork-icon.rc");
    let icon_rc = icon.to_string_lossy().replace('\\', "/");
    std::fs::write(&rc_path, format!("1 ICON \"{icon_rc}\"\n")).expect("write lanwork icon rc");
    let mut res = winresource::WindowsResource::new();
    let rc = rc_path.to_str().expect("icon rc path is utf-8");
    res.set_resource_file(rc);
    res.compile().expect("embed lanwork.ico");
}
