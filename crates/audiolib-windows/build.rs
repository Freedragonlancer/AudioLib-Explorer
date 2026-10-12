// crates/audiolib-windows/build.rs
fn main() {
    println!("cargo:rerun-if-env-changed=CPAL_ASIO_DIR");
    println!("cargo:rerun-if-changed=src/asio_shim.cpp");
    if std::env::var_os("CARGO_FEATURE_ASIO").is_some()
        && std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
    {
        let sdk = std::env::var_os("CPAL_ASIO_DIR")
            .expect("Set CPAL_ASIO_DIR to the approved ASIO SDK path");
        cc::Build::new()
            .cpp(true)
            .file("src/asio_shim.cpp")
            .include(std::path::PathBuf::from(sdk).join("common"))
            .compile("audiolib_asio_shim");
    }
}
