use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use ureq::Agent;

const REPO: &str = "MaelitoP/scwx";

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Debug, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

fn target() -> Result<String> {
    let os = match env::consts::OS {
        "macos" => "apple-darwin",
        "linux" => "unknown-linux-gnu",
        other => anyhow::bail!("unsupported os: {other}"),
    };
    Ok(format!("{}-{os}", env::consts::ARCH))
}

pub fn run() -> Result<()> {
    let agent: Agent = Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(120)))
        .https_only(true)
        .build()
        .into();

    let release: Release = agent
        .get(format!(
            "https://api.github.com/repos/{REPO}/releases/latest"
        ))
        .header("User-Agent", "scwx")
        .header("Accept", "application/vnd.github+json")
        .call()
        .context("fetching the latest release")?
        .body_mut()
        .read_json()
        .context("parsing the latest release")?;

    let current = env!("CARGO_PKG_VERSION");
    let latest = release.tag_name.trim_start_matches('v');
    if latest == current {
        println!("scwx {current} is up to date");
        return Ok(());
    }

    let asset_name = format!("scwx-{}", target()?);
    let asset = release
        .assets
        .iter()
        .find(|asset| asset.name == asset_name)
        .with_context(|| {
            format!(
                "release {} has no asset named {asset_name}",
                release.tag_name
            )
        })?;

    eprintln!("downloading scwx {latest} ({asset_name})...");
    let binary = agent
        .get(&asset.browser_download_url)
        .header("User-Agent", "scwx")
        .call()
        .context("downloading the release binary")?
        .body_mut()
        .with_config()
        .limit(256 * 1024 * 1024)
        .read_to_vec()
        .context("reading the release binary")?;
    ensure!(!binary.is_empty(), "downloaded binary is empty");

    let current_exe = env::current_exe().context("resolving the current scwx binary")?;
    let temp = current_exe.with_extension("update");
    fs::write(&temp, &binary).with_context(|| {
        format!(
            "writing {} (if scwx is managed by nix or a package manager, update it there instead)",
            temp.display()
        )
    })?;
    fs::set_permissions(&temp, fs::Permissions::from_mode(0o755))
        .context("marking the new binary executable")?;
    fs::rename(&temp, &current_exe)
        .with_context(|| format!("replacing {}", current_exe.display()))?;

    println!("updated scwx {current} -> {latest}");
    Ok(())
}
