//! Handing the terminal over to child processes.

use std::ffi::OsString;
use std::os::unix::process::CommandExt;
use std::process::Command;

use anyhow::{Context, Result};

pub(crate) fn command(argv: &[OsString]) -> Result<Command> {
    let (program, args) = argv.split_first().context("empty command")?;
    let mut command = Command::new(program);
    command.args(args);
    Ok(command)
}

/// Replaces the current process; only returns on failure.
pub(crate) fn replace(argv: &[OsString]) -> Result<()> {
    let error = command(argv)?.exec();
    Err(error).with_context(|| format!("executing {}", argv[0].to_string_lossy()))
}
