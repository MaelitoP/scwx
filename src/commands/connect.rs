use anyhow::{Result, ensure};

use crate::cli::Cli;
use crate::config::Config;
use crate::inventory::Resource;
use crate::picker::{self, PickOutcome, Selection};
use crate::{cache, exec, ssh, tmux};

pub fn run(cli: &Cli, config: &Config, query: Option<&str>) -> Result<()> {
    let inventory = cache::load_or_fetch(cli.refresh, config)?;
    let bastion = inventory.require_bastion()?.clone();
    let servers: Vec<&Resource> = inventory
        .filtered(cli.env, &config.tags)
        .filter(|resource| resource.kind.is_server())
        .collect();
    ensure!(!servers.is_empty(), "no running servers in the inventory");

    let lines = picker::render_resources(&servers, config);
    match picker::select(
        &servers,
        &lines,
        "Connect to server",
        query,
        &config.naming,
        true,
    )? {
        Selection::Direct(server) => open(server, PickOutcome::Inline, config, &bastion),
        Selection::Picked(server, outcome) => open(server, outcome, config, &bastion),
        Selection::Cancelled => Ok(()),
        Selection::NoMatch => Err(no_server_error(
            &inventory,
            cli,
            config,
            query.unwrap_or_default(),
        )),
    }
}

fn no_server_error(
    inventory: &crate::inventory::Inventory,
    cli: &Cli,
    config: &Config,
    query: &str,
) -> anyhow::Error {
    let unreachable_match = inventory
        .filtered(cli.env, &config.tags)
        .find(|resource| !resource.kind.is_server() && resource.matches(query, &config.naming));
    match unreachable_match {
        Some(resource) => {
            let name = resource.display_name(&config.naming);
            let hint = match resource.kind {
                crate::inventory::ResourceKind::Rdb => format!("scwx db {name}"),
                _ => format!("scwx pf {name}"),
            };
            anyhow::anyhow!(
                "{name} is a {} and has no ssh; try `{hint}`",
                resource.kind.label()
            )
        }
        None => anyhow::anyhow!("no server matches '{query}'"),
    }
}

fn open(
    resource: &Resource,
    outcome: PickOutcome,
    config: &Config,
    bastion: &crate::inventory::Bastion,
) -> Result<()> {
    let argv = ssh::session(&resource.name, config, bastion)?.into_argv();
    let title = resource.display_name(&config.naming).to_owned();
    match outcome.placement() {
        Some(placement) if tmux::inside_tmux() => tmux::open(placement, &title, &argv),
        _ => exec::replace(&argv),
    }
}
