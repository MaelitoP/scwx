mod cli;
mod commands;
#[allow(dead_code)] // consumed progressively while commands are stubs
mod config;
#[allow(dead_code)] // consumed progressively while commands are stubs
mod paths;

use anyhow::Result;
use clap::Parser;

use crate::cli::{Cli, Command};
use crate::config::Config;

fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = Config::load(&paths::config_file()?)?;
    match cli.command {
        Command::Ls { json, names } => commands::ls::run(&cli, &config, json, names),
        Command::Connect { ref query } => commands::connect::run(&cli, &config, query.as_deref()),
        Command::Db { ref name } => commands::db::run(&cli, &config, name.as_deref()),
        Command::Pf {
            ref command,
            ref query,
            local_port,
            remote_port,
        } => commands::pf::run(
            &cli,
            &config,
            command.as_ref(),
            query.as_deref(),
            local_port,
            remote_port,
        ),
        Command::SyncSsh => commands::sync_ssh::run(&cli, &config),
        Command::Completions { shell } => commands::completions::run(shell),
        Command::Update => commands::update::run(),
    }
}
