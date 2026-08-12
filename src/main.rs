mod cache;
mod cli;
mod commands;
mod config;
mod exec;
mod inventory;
mod output;
mod paths;
mod picker;
mod scw;
mod secrets;
mod sensitive;
mod ssh;
mod table;
mod tmux;

use anyhow::Result;
use clap::Parser;

use crate::cli::{Cli, Command};
use crate::config::Config;

fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = Config::load(&paths::config_file()?)?;
    match cli.command {
        Command::Ls {
            json,
            names,
            db_names,
            pf_names,
            cached,
        } => {
            let filter = commands::ls::NameFilter {
                servers: names,
                databases: db_names,
                forwardable: pf_names,
            };
            commands::ls::run(&cli, &config, json, filter, cached)
        }
        Command::Connect { ref query } => commands::connect::run(&cli, &config, query.as_deref()),
        Command::Db {
            ref name,
            ref execute,
            ref mysql_args,
        } => commands::db::run(
            &cli,
            &config,
            name.as_deref(),
            commands::db::MysqlOptions {
                execute: execute.as_deref(),
                extra_args: mysql_args,
            },
        ),
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
