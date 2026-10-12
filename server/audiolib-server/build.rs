// server/audiolib-server/build.rs
//! Embeds the short git commit hash as `AUDIOLIB_GIT_COMMIT` for `/health`'s `commit`
//! (falls back to "unknown" when git or the repository is unavailable, e.g. a source tarball).
use std::process::Command;

fn main() {
    let commit = Command::new("git")
        .args(["rev-parse", "--short=7", "HEAD"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty() && text.chars().all(|c| c.is_ascii_hexdigit()))
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=AUDIOLIB_GIT_COMMIT={commit}");
    // Rebuild when the checked-out commit changes (workspace root is two levels up).
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs/heads");
    println!("cargo:rerun-if-changed=../../.git/packed-refs");
}
