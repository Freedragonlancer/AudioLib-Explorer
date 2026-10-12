// app/audiolib-desktop/build.rs
fn main() {
    slint_build::compile_with_config(
        "ui/app-window.slint",
        slint_build::CompilerConfiguration::new()
            .with_style("fluent".into())
            .with_debug_info(std::env::var("PROFILE").as_deref() != Ok("release")),
    )
    .expect("compile AudioLib desktop UI");
    println!("cargo:rerun-if-changed=ui/app-window.slint");
    println!("cargo:rerun-if-changed=ui/audition-panel.slint");
    println!("cargo:rerun-if-changed=ui/icons");
    println!("cargo:rerun-if-changed=ui/theme.slint");
    println!("cargo:rerun-if-changed=app.manifest");
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        winresource::WindowsResource::new()
            .set_manifest_file("app.manifest")
            .set("ProductName", "AudioLib Explorer")
            .set("FileDescription", "AudioLib Explorer sound library")
            .set("OriginalFilename", "AudioLibExplorer.exe")
            .compile()
            .expect("compile Windows application manifest");
    }
}
