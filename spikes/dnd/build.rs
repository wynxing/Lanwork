fn main() {
    slint_build::compile("ui/main.slint").expect("compile ui/main.slint");
    println!("cargo:rerun-if-changed=ui/main.slint");
    println!("cargo:rerun-if-changed=app.manifest");
    if std::env::var("CARGO_CFG_TARGET_OS").ok().as_deref() == Some("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_manifest_file("app.manifest");
        res.compile().expect("embed app.manifest");
    }
}
