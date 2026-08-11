use std::env;
use std::ffi::OsString;
use std::process::Command;

use anyhow::{Context, Result, ensure};

use crate::exec::shell_join;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    Window,
    Split,
    VSplit,
}

pub fn inside_tmux() -> bool {
    env::var_os("TMUX").is_some_and(|value| !value.is_empty())
}

pub fn open(placement: Placement, title: &str, argv: &[OsString]) -> Result<()> {
    let command = shell_join(
        &argv
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
    );
    // Hold the pane open on failure; tmux destroys it as soon as the
    // command exits, which would hide the error.
    let command = format!(
        "{command} || {{ status=$?; \
         printf '\\n[scwx] exited with status %d - press enter to close\\n' \"$status\"; \
         read -r _; }}"
    );

    let mut tmux = Command::new("tmux");
    match placement {
        Placement::Window => {
            tmux.args(["new-window", "-n", title, &command]);
        }
        Placement::Split => {
            tmux.args(["split-window", "-v", &command]);
        }
        Placement::VSplit => {
            tmux.args(["split-window", "-h", &command]);
        }
    }
    let status = tmux.status().context("running tmux")?;
    ensure!(status.success(), "tmux exited with {status}");

    if placement != Placement::Window {
        // The new pane is active right after split-window.
        let _ = Command::new("tmux")
            .args(["select-pane", "-T", title])
            .status();
    }
    Ok(())
}
