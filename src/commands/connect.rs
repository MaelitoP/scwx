use anyhow::{Result, bail, ensure};

use crate::cli::Cli;
use crate::config::Config;
use crate::inventory::Resource;
use crate::picker::{self, PickOutcome};
use crate::{cache, exec, ssh, tmux};

pub fn run(cli: &Cli, config: &Config, query: Option<&str>) -> Result<()> {
    let inventory = cache::load_or_fetch(cli.refresh, config)?;
    let bastion = inventory.bastion()?.clone();
    let servers: Vec<&Resource> = inventory
        .filtered(cli.env, &config.tags)
        .into_iter()
        .filter(|resource| resource.kind.is_server())
        .collect();
    ensure!(!servers.is_empty(), "no running servers in the inventory");

    if let Some(query) = query {
        let matches: Vec<&Resource> = servers
            .iter()
            .copied()
            .filter(|resource| resource.matches(query, &config.naming))
            .collect();
        match matches.len() {
            0 => {
                let unreachable_match =
                    inventory
                        .filtered(cli.env, &config.tags)
                        .into_iter()
                        .find(|resource| {
                            !resource.kind.is_server() && resource.matches(query, &config.naming)
                        });
                match unreachable_match {
                    Some(resource) => {
                        let name = resource.display_name(&config.naming);
                        let hint = match resource.kind {
                            crate::inventory::ResourceKind::Rdb => {
                                format!("scwx db {name}")
                            }
                            _ => format!("scwx pf {name}"),
                        };
                        bail!(
                            "{name} is a {} and has no ssh; try `{hint}`",
                            resource.kind.label()
                        )
                    }
                    None => bail!("no server matches '{query}'"),
                }
            }
            1 => return open(matches[0], PickOutcome::Inline, config, &bastion),
            _ => {}
        }
    }

    let lines = picker::render_resources(&servers, config);
    let Some(pick) = picker::pick(&lines, "Connect to server", query)? else {
        return Ok(());
    };
    open(servers[pick.index], pick.outcome, config, &bastion)
}

fn open(
    resource: &Resource,
    outcome: PickOutcome,
    config: &Config,
    bastion: &crate::inventory::Bastion,
) -> Result<()> {
    let argv = ssh::session_argv(&resource.name, config, bastion)?;
    let title = resource.display_name(&config.naming);
    let placement = match outcome {
        PickOutcome::Inline => None,
        PickOutcome::Window => Some(tmux::Placement::Window),
        PickOutcome::Split => Some(tmux::Placement::Split),
        PickOutcome::VSplit => Some(tmux::Placement::VSplit),
    };
    match placement {
        Some(placement) if tmux::inside_tmux() => tmux::open(placement, &title, &argv),
        _ => exec::replace(&argv),
    }
}
