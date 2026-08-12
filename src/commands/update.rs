use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use sha2::{Digest, Sha256};
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
        other => bail!("unsupported os: {other}"),
    };
    Ok(format!("{}-{os}", env::consts::ARCH))
}

fn parse_version(version: &str) -> Option<[u64; 3]> {
    let mut parts = version.trim_start_matches('v').splitn(3, '.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    Some([major, minor, patch])
}

fn expected_sha256(checksum_file: &str, asset_name: &str) -> Result<String> {
    checksum_file
        .lines()
        .find_map(|line| {
            let mut parts = line.split_whitespace();
            let hash = parts.next()?;
            let name = parts.next()?.trim_start_matches('*');
            (name == asset_name).then(|| hash.to_ascii_lowercase())
        })
        .with_context(|| format!("checksum file has no entry for {asset_name}"))
}

fn verify_binary(binary: &[u8], expected_sha256: &str) -> Result<()> {
    let is_executable = binary.starts_with(b"\x7fELF")
        || binary.starts_with(&[0xcf, 0xfa, 0xed, 0xfe])
        || binary.starts_with(&[0xca, 0xfe, 0xba, 0xbe]);
    ensure!(
        is_executable,
        "downloaded file is not an executable (served an error page?)"
    );

    let digest = Sha256::digest(binary);
    let actual: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    ensure!(
        actual == expected_sha256,
        "checksum mismatch: expected {expected_sha256}, got {actual}"
    );
    Ok(())
}

fn download(agent: &Agent, asset: &Asset) -> Result<Vec<u8>> {
    agent
        .get(&asset.browser_download_url)
        .header("User-Agent", "scwx")
        .call()
        .with_context(|| format!("downloading {}", asset.name))?
        .body_mut()
        .with_config()
        .limit(256 * 1024 * 1024)
        .read_to_vec()
        .with_context(|| format!("reading {}", asset.name))
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
    let current_version =
        parse_version(current).with_context(|| format!("invalid current version {current}"))?;
    let latest_version = parse_version(latest)
        .with_context(|| format!("invalid release version {}", release.tag_name))?;
    if latest_version == current_version {
        println!("scwx {current} is up to date");
        return Ok(());
    }
    ensure!(
        latest_version > current_version,
        "latest release {latest} is older than the installed {current}; refusing to downgrade"
    );

    let asset_name = format!("scwx-{}", target()?);
    let find_asset = |name: &str| {
        release
            .assets
            .iter()
            .find(|asset| asset.name == name)
            .with_context(|| format!("release {} has no asset named {name}", release.tag_name))
    };
    let binary_asset = find_asset(&asset_name)?;
    let checksum_asset = find_asset(&format!("{asset_name}.sha256"))?;

    eprintln!("downloading scwx {latest} ({asset_name})...");
    let checksums = String::from_utf8(download(&agent, checksum_asset)?)
        .context("checksum file is not utf-8")?;
    let binary = download(&agent, binary_asset)?;
    verify_binary(&binary, &expected_sha256(&checksums, &asset_name)?)?;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse_and_order() {
        assert_eq!(parse_version("v0.1.0"), Some([0, 1, 0]));
        assert_eq!(parse_version("1.12.3"), Some([1, 12, 3]));
        assert_eq!(parse_version("nightly"), None);
        assert!(parse_version("0.2.0") > parse_version("0.1.9"));
        assert!(parse_version("0.10.0") > parse_version("0.9.9"));
    }

    #[test]
    fn checksum_entry_is_matched_by_asset_name() {
        let file = "abc123  scwx-aarch64-apple-darwin\ndef456 *scwx-x86_64-apple-darwin\n";
        assert_eq!(
            expected_sha256(file, "scwx-aarch64-apple-darwin").unwrap(),
            "abc123"
        );
        assert_eq!(
            expected_sha256(file, "scwx-x86_64-apple-darwin").unwrap(),
            "def456"
        );
        assert!(expected_sha256(file, "scwx-other").is_err());
    }

    #[test]
    fn binary_verification_rejects_bad_magic_and_bad_hash() {
        let elf = b"\x7fELFrest-of-binary".to_vec();
        let digest = Sha256::digest(&elf);
        let hash: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();

        verify_binary(&elf, &hash).unwrap();
        assert!(verify_binary(&elf, "0000").is_err());
        assert!(verify_binary(b"<html>error</html>", &hash).is_err());
    }
}
