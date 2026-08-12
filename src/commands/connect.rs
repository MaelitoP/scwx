use anyhow::{Result, ensure};

use crate::cli::Scope;
use crate::config::Config;
use crate::inventory::Resource;
use crate::inventory::{Inventory, ResourceKind};
use crate::picker::{self, PickOutcome, Selection};
use crate::{cache, exec, ssh, tmux};

pub fn run(scope: &Scope, config: &Config, query: Option<&str>) -> Result<()> {
    let inventory = cache::load_or_fetch(scope.freshness, config)?;
    let bastion = inventory.require_bastion()?.clone();
    let servers: Vec<&Resource> = inventory
        .filtered(scope.env, &config.tags)
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
        Selection::Direct(server) => open_session(server, PickOutcome::Inline, config, &bastion),
        Selection::Picked(server, outcome) => open_session(server, outcome, config, &bastion),
        Selection::Cancelled => Ok(()),
        Selection::NoMatch => Err(no_server_error(
            &inventory,
            scope,
            config,
            query.unwrap_or_default(),
        )),
    }
}

fn no_server_error(
    inventory: &Inventory,
    scope: &Scope,
    config: &Config,
    query: &str,
) -> anyhow::Error {
    let unreachable_match = inventory
        .filtered(scope.env, &config.tags)
        .find(|resource| !resource.kind.is_server() && resource.matches(query, &config.naming));
    match unreachable_match {
        Some(resource) => {
            let name = resource.display_name(&config.naming);
            let hint = match resource.kind {
                ResourceKind::Rdb => format!("scwx db {name}"),
                ResourceKind::Redis | ResourceKind::Lb => format!("scwx pf {name}"),
                ResourceKind::Instance | ResourceKind::Baremetal => {
                    format!("scwx connect {name}")
                }
            };
            anyhow::anyhow!(
                "{name} is a {} and has no ssh; try `{hint}`",
                resource.kind.as_str()
            )
        }
        None => anyhow::anyhow!("no server matches '{query}'"),
    }
}

fn open_session(
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
