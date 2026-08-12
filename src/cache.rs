//! On-disk inventory cache with a short TTL.

use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
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

pub(crate) fn load_fresh(path: &Path, ttl: Duration, now: SystemTime) -> Option<Inventory> {
    let raw = fs::read_to_string(path).ok()?;
    let file: CacheFile = serde_json::from_str(&raw).ok()?;
    if file.version != CACHE_VERSION {
        return None;
    }
    let now_unix = now.duration_since(UNIX_EPOCH).ok()?.as_secs();
    let age = now_unix.checked_sub(file.fetched_at_unix)?;
    (age <= ttl.as_secs()).then_some(file.inventory)
}

pub(crate) fn store(path: &Path, inventory: &Inventory, now: SystemTime) -> Result<()> {
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

    // Per-process temp name so concurrent scwx invocations don't truncate
    // each other's writes; 0600 because the content maps private network
    // topology.
    let temp = path.with_extension(format!("tmp.{}", std::process::id()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true).mode(0o600);
    let mut temp_file = options
        .open(&temp)
        .with_context(|| format!("creating cache {}", temp.display()))?;
    temp_file
        .write_all(raw.as_bytes())
        .with_context(|| format!("writing cache {}", temp.display()))?;
    drop(temp_file);
    fs::rename(&temp, path).with_context(|| format!("replacing cache {}", path.display()))
}

/// Cache-only read for completion helpers: any age is fine, never fetch.
pub(crate) fn load_ignoring_ttl() -> Result<Option<Inventory>> {
    let path = paths::cache_file()?;
    Ok(load_fresh(&path, Duration::MAX, SystemTime::now()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Freshness {
    CacheOk,
    Refresh,
}

pub(crate) fn load_or_fetch(freshness: Freshness, config: &Config) -> Result<Inventory> {
    let path = paths::cache_file()?;
    let ttl = Duration::from_secs(config.cache.ttl_seconds);

    if freshness == Freshness::CacheOk
        && let Some(inventory) = load_fresh(&path, ttl, SystemTime::now())
    {
        return Ok(inventory);
    }

    eprintln!("refreshing inventory...");
    let credentials = crate::config::Credentials::load(&paths::scw_config_file()?)?;
    let fetched = scw::fetch_inventory(&credentials, config)?;
    if fetched.complete {
        store(&path, &fetched.inventory, SystemTime::now())?;
    } else {
        eprintln!("warning: inventory is incomplete; not caching it");
    }
    Ok(fetched.inventory)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::{Bastion, Inventory};

    fn inventory() -> Inventory {
        Inventory::new(
            vec![],
            Some(Bastion {
                ip: "5.6.7.8".to_owned(),
                port: 61000,
                zone: "fr-par-1".to_owned(),
            }),
        )
    }

    fn now_unix() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    #[test]
    fn roundtrip_within_ttl_returns_the_inventory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("inventory.json");
        let now = SystemTime::now();
        store(&path, &inventory(), now).unwrap();

        let loaded = load_fresh(&path, Duration::from_secs(300), now).unwrap();
        assert_eq!(loaded.require_bastion().unwrap().ip, "5.6.7.8");
    }

    #[test]
    fn cache_is_written_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("inventory.json");
        store(&path, &inventory(), SystemTime::now()).unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn stale_or_future_timestamps_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("inventory.json");
        let stored_at = SystemTime::now();
        store(&path, &inventory(), stored_at).unwrap();

        let expired = stored_at + Duration::from_secs(301);
        assert!(load_fresh(&path, Duration::from_secs(300), expired).is_none());

        let before_write = stored_at - Duration::from_secs(3600);
        assert!(load_fresh(&path, Duration::from_secs(300), before_write).is_none());

        assert!(load_fresh(&path, Duration::from_secs(300), stored_at).is_some());
    }

    #[test]
    fn unusable_cache_content_is_ignored() {
        let empty_inventory = r#"{"resources": [], "bastion": null}"#;
        let cases = [
            (
                "wrong version, fresh timestamp",
                format!(
                    r#"{{"version": 0, "fetched_at_unix": {}, "inventory": {empty_inventory}}}"#,
                    now_unix()
                ),
            ),
            ("corrupt json", "not json".to_owned()),
            (
                "missing fields",
                format!(r#"{{"version": {CACHE_VERSION}}}"#),
            ),
        ];

        for (label, content) in cases {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("inventory.json");
            fs::write(&path, content).unwrap();

            let loaded = load_fresh(&path, Duration::from_secs(300), SystemTime::now());
            assert!(loaded.is_none(), "case '{label}' should be rejected");
        }
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
