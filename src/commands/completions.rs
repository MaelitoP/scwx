use std::io;

use anyhow::Result;
use clap::CommandFactory;
use clap_complete::Shell;

use crate::cli::Cli;

pub fn run(shell: Shell) -> Result<()> {
    let mut command = Cli::command();
    clap_complete::generate(shell, &mut command, "scwx", &mut io::stdout());
    Ok(())
}
