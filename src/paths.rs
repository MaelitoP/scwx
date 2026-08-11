use std::env;
use std::path::PathBuf;

use anyhow::{Context, Result};

pub fn home_dir() -> Result<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")
}

fn xdg_dir(var: &str, fallback: &str) -> Result<PathBuf> {
    match env::var_os(var).filter(|v| !v.is_empty()) {
        Some(dir) => Ok(PathBuf::from(dir)),
        None => Ok(home_dir()?.join(fallback)),
    }
}

pub fn config_file() -> Result<PathBuf> {
    Ok(xdg_dir("XDG_CONFIG_HOME", ".config")?.join("scwx/config.toml"))
}

pub fn scw_config_file() -> Result<PathBuf> {
    Ok(xdg_dir("XDG_CONFIG_HOME", ".config")?.join("scw/config.yaml"))
}

pub fn cache_file() -> Result<PathBuf> {
    Ok(xdg_dir("XDG_CACHE_HOME", ".cache")?.join("scwx/inventory.json"))
}

#[allow(dead_code)] // consumed when pf lands
pub fn sockets_dir() -> Result<PathBuf> {
    Ok(xdg_dir("XDG_STATE_HOME", ".local/state")?.join("scwx/sockets"))
}

pub fn expand_tilde(path: &str) -> Result<PathBuf> {
    match path.strip_prefix("~/") {
        Some(rest) => Ok(home_dir()?.join(rest)),
        None => Ok(PathBuf::from(path)),
    }
}
