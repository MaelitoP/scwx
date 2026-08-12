//! Opening sessions in tmux windows and splits.

use std::env;
use std::ffi::OsString;
use std::process::Command;

use anyhow::{Context, Result, ensure};

use crate::shell::shell_join;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Placement {
    Window,
    Below,
    Beside,
}

pub(crate) fn inside_tmux() -> bool {
    env::var_os("TMUX").is_some_and(|value| !value.is_empty())
}

pub(crate) fn open(placement: Placement, title: &str, argv: &[OsString]) -> Result<()> {
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
        Placement::Below => {
            tmux.args(["split-window", "-v", &command]);
        }
        Placement::Beside => {
            tmux.args(["split-window", "-h", &command]);
        }
    }
    let status = tmux.status().context("running tmux")?;
    ensure!(status.success(), "tmux exited with {status}");

    if placement != Placement::Window {
        // The new pane is active right after split-window. Best-effort:
        // the title is cosmetic and older tmux lacks -T.
        let _ = Command::new("tmux")
            .args(["select-pane", "-T", title])
            .status();
    }
    Ok(())
}
