//! Filesystem locations, XDG-aware.

use std::env;
use std::path::PathBuf;

use anyhow::{Context, Result};

pub(crate) fn home_dir() -> Result<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")
}

fn xdg_dir(var: &str, fallback: &str) -> Result<PathBuf> {
    Ok(match env::var_os(var).filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => home_dir()?.join(fallback),
    })
}

pub(crate) fn config_file() -> Result<PathBuf> {
    Ok(xdg_dir("XDG_CONFIG_HOME", ".config")?.join("scwx/config.toml"))
}

pub(crate) fn scw_config_file() -> Result<PathBuf> {
    Ok(xdg_dir("XDG_CONFIG_HOME", ".config")?.join("scw/config.yaml"))
}

pub(crate) fn cache_file() -> Result<PathBuf> {
    Ok(xdg_dir("XDG_CACHE_HOME", ".cache")?.join("scwx/inventory.json"))
}

pub(crate) fn sockets_dir() -> Result<PathBuf> {
    Ok(xdg_dir("XDG_STATE_HOME", ".local/state")?.join("scwx/sockets"))
}

pub(crate) fn expand_tilde(path: &str) -> Result<PathBuf> {
    Ok(match path.strip_prefix("~/") {
        Some(rest) => home_dir()?.join(rest),
        None => PathBuf::from(path),
    })
}
