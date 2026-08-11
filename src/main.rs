mod cli;
mod commands;

use anyhow::Result;
use clap::Parser;

use crate::cli::{Cli, Command};

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Ls { json, names } => commands::ls::run(&cli, json, names),
        Command::Connect { ref query } => commands::connect::run(&cli, query.as_deref()),
        Command::Db { ref name } => commands::db::run(&cli, name.as_deref()),
        Command::Pf {
            ref command,
            ref query,
            local_port,
            remote_port,
        } => commands::pf::run(
            &cli,
            command.as_ref(),
            query.as_deref(),
            local_port,
            remote_port,
        ),
        Command::SyncSsh => commands::sync_ssh::run(&cli),
        Command::Completions { shell } => commands::completions::run(shell),
        Command::Update => commands::update::run(),
    }
}
