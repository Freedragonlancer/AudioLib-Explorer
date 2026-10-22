// crates/audiolib-update/examples/check-release.rs
//! GitHub discovery/download probe; never loads a library or deploys an application.
use anyhow::{Result, bail};
use audiolib_update::{Channel, Product, UpdateClient};

fn main() -> Result<()> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    let product = match arguments.first().map(String::as_str) {
        Some("desktop") => Product::Desktop,
        Some("server") => Product::Server,
        _ => bail!(
            "Usage: check-release desktop|server stable|beta [current-version] [--verify-download]"
        ),
    };
    let channel = match arguments.get(1).map(String::as_str) {
        Some("stable") => Channel::Stable,
        Some("beta") => Channel::Beta,
        _ => bail!(
            "Usage: check-release desktop|server stable|beta [current-version] [--verify-download]"
        ),
    };
    let current = arguments
        .get(2)
        .filter(|argument| !argument.starts_with("--"))
        .map_or(env!("CARGO_PKG_VERSION"), String::as_str);
    match UpdateClient::new(product, channel, current)?.check()? {
        Some(release) => {
            println!("{} {}", release.version, release.release_url);
            if arguments
                .iter()
                .any(|argument| argument == "--verify-download")
            {
                let directory = tempfile::tempdir()?;
                let client = UpdateClient::new(product, channel, current)?;
                let staged = client.prepare(&release, directory.path())?;
                println!(
                    "Verified archive, SHA256 and public payload for {} (no deployment).",
                    release.tag
                );
                drop(staged);
            }
        }
        None => println!("No newer supported release for {current}."),
    }
    Ok(())
}
