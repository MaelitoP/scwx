use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::inventory::Inventory;
use crate::{paths, scw};

const CACHE_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
struct CacheFile {
    version: u32,
    fetched_at_unix: u64,
    inventory: Inventory,
}

pub fn load_fresh(path: &Path, ttl: Duration, now: SystemTime) -> Option<Inventory> {
    let raw = fs::read_to_string(path).ok()?;
    let file: CacheFile = serde_json::from_str(&raw).ok()?;
    if file.version != CACHE_VERSION {
        return None;
    }
    let now_unix = now.duration_since(UNIX_EPOCH).ok()?.as_secs();
    let age = now_unix.checked_sub(file.fetched_at_unix)?;
    (age <= ttl.as_secs()).then_some(file.inventory)
}

pub fn store(path: &Path, inventory: &Inventory, now: SystemTime) -> Result<()> {
    let parent = path.parent().context("cache path has no parent")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("creating cache directory {}", parent.display()))?;

    let file = CacheFile {
        version: CACHE_VERSION,
        fetched_at_unix: now
            .duration_since(UNIX_EPOCH)
            .context("system clock before unix epoch")?
            .as_secs(),
        inventory: inventory.clone(),
    };
    let raw = serde_json::to_string(&file).context("serializing inventory cache")?;

    let temp = path.with_extension("json.tmp");
    fs::write(&temp, raw).with_context(|| format!("writing cache {}", temp.display()))?;
    fs::rename(&temp, path).with_context(|| format!("replacing cache {}", path.display()))
}

/// Cache-only read for completion helpers: any age is fine, never fetch.
pub fn load_ignoring_ttl() -> Result<Option<Inventory>> {
    let path = paths::cache_file()?;
    Ok(load_fresh(&path, Duration::MAX, SystemTime::now()))
}

pub fn load_or_fetch(refresh: bool, config: &Config) -> Result<Inventory> {
    let path = paths::cache_file()?;
    let ttl = Duration::from_secs(config.cache.ttl_seconds);

    if !refresh && let Some(inventory) = load_fresh(&path, ttl, SystemTime::now()) {
        return Ok(inventory);
    }

    eprintln!("refreshing inventory...");
    let credentials = crate::config::Credentials::load(&paths::scw_config_file()?)?;
    let inventory = scw::fetch_inventory(&credentials, config)?;
    store(&path, &inventory, SystemTime::now())?;
    Ok(inventory)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::{Bastion, Inventory};

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("scwx-cache-{name}-{}.json", std::process::id()))
    }

    fn inventory() -> Inventory {
        Inventory {
            resources: vec![],
            bastion: Some(Bastion {
                ip: "5.6.7.8".to_owned(),
                port: 61000,
                zone: "fr-par-1".to_owned(),
            }),
        }
    }

    #[test]
    fn roundtrip_within_ttl_returns_the_inventory() {
        let path = temp_path("roundtrip");
        let now = SystemTime::now();
        store(&path, &inventory(), now).unwrap();

        let loaded = load_fresh(&path, Duration::from_secs(300), now).unwrap();
        fs::remove_file(&path).unwrap();

        assert_eq!(loaded.bastion.unwrap().ip, "5.6.7.8");
    }

    #[test]
    fn expired_cache_is_ignored() {
        let path = temp_path("expired");
        let stored_at = SystemTime::now();
        store(&path, &inventory(), stored_at).unwrap();

        let later = stored_at + Duration::from_secs(301);
        let loaded = load_fresh(&path, Duration::from_secs(300), later);
        fs::remove_file(&path).unwrap();

        assert!(loaded.is_none());
    }

    #[test]
    fn cache_written_in_the_future_is_ignored() {
        let path = temp_path("future");
        let stored_at = SystemTime::now() + Duration::from_secs(3600);
        store(&path, &inventory(), stored_at).unwrap();

        let loaded = load_fresh(&path, Duration::from_secs(300), SystemTime::now());
        fs::remove_file(&path).unwrap();

        assert!(loaded.is_none());
    }

    #[test]
    fn version_mismatch_is_ignored() {
        let path = temp_path("version");
        fs::write(
            &path,
            r#"{"version": 0, "fetched_at_unix": 99999999999, "inventory": {"resources": [], "bastion": null}}"#,
        )
        .unwrap();

        let loaded = load_fresh(&path, Duration::from_secs(300), SystemTime::now());
        fs::remove_file(&path).unwrap();

        assert!(loaded.is_none());
    }

    #[test]
    fn corrupt_cache_is_ignored() {
        let path = temp_path("corrupt");
        fs::write(&path, "not json").unwrap();

        let loaded = load_fresh(&path, Duration::from_secs(300), SystemTime::now());
        fs::remove_file(&path).unwrap();

        assert!(loaded.is_none());
    }

    #[test]
    fn missing_cache_is_ignored() {
        let loaded = load_fresh(
            Path::new("/nonexistent/inventory.json"),
            Duration::from_secs(300),
            SystemTime::now(),
        );
        assert!(loaded.is_none());
    }
}
