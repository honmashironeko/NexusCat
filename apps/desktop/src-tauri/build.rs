fn main() {
    if std::env::var("PROFILE").as_deref() == Ok("release")
        && std::env::var_os("CARGO_FEATURE_CUSTOM_PROTOCOL").is_none()
    {
        panic!(
            "release 构建必须启用 custom-protocol：cargo build --release -p nc-desktop --features custom-protocol"
        );
    }
    println!("cargo:rerun-if-changed=app.manifest");
    let windows = if std::env::var("PROFILE").as_deref() == Ok("release") {
        tauri_build::WindowsAttributes::new().app_manifest(include_str!("app.manifest"))
    } else {
        tauri_build::WindowsAttributes::new()
    };
    let attributes = tauri_build::Attributes::new().windows_attributes(windows);
    if let Err(error) = tauri_build::try_build(attributes) {
        panic!("tauri-build 失败：{error:#}");
    }
}
