use anyhow::Result;
use serde::Serialize;

use crate::cli::Scope;
use crate::config::Config;
use crate::inventory::{Environment, Resource};
use crate::{cache, output, table};

#[derive(Debug, Serialize)]
struct ResourceView<'a> {
    kind: &'static str,
    id: &'a str,
    name: &'a str,
    display_name: String,
    zone: &'a str,
    env: Option<Environment>,
    tags: &'a [String],
    endpoint_ip: Option<&'a str>,
    endpoint_port: Option<u16>,
}

impl<'a> ResourceView<'a> {
    fn new(resource: &'a Resource, config: &Config) -> Self {
        Self {
            kind: resource.kind.label(),
            id: &resource.id,
            name: &resource.name,
            display_name: resource.display_name(&config.naming).to_owned(),
            zone: &resource.zone,
            env: resource.env(&config.tags),
            tags: &resource.tags,
            endpoint_ip: resource.endpoint_ip.as_deref(),
            endpoint_port: resource.endpoint_port,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameOutput {
    Servers,
    Databases,
    Forwardable,
}

pub fn run(
    scope: &Scope,
    config: &Config,
    json: bool,
    names: Option<NameOutput>,
    cached: bool,
) -> Result<()> {
    let inventory = if cached {
        match cache::load_ignoring_ttl()? {
            Some(inventory) => inventory,
            None => return Ok(()),
        }
    } else {
        cache::load_or_fetch(scope.freshness, config)?
    };
    let resources: Vec<&Resource> = inventory.filtered(scope.env, &config.tags).collect();

    if let Some(names) = names {
        return print_names(names, &resources, config);
    }

    let views: Vec<ResourceView<'_>> = resources
        .iter()
        .map(|resource| ResourceView::new(resource, config))
        .collect();

    if json {
        output::emit(&serde_json::to_string_pretty(&views)?)?;
        return Ok(());
    }

    print_table(&views)
}

fn print_names(names: NameOutput, resources: &[&Resource], config: &Config) -> Result<()> {
    let selected: Vec<&str> = match names {
        NameOutput::Servers => resources
            .iter()
            .filter(|r| r.kind.is_server())
            .map(|r| r.display_name(&config.naming))
            .collect(),
        NameOutput::Databases => {
            for name in crate::database::database_keys(resources, config) {
                if !output::emit(&name)? {
                    break;
                }
            }
            return Ok(());
        }
        NameOutput::Forwardable => resources
            .iter()
            .filter(|r| r.port_forward_enabled(&config.tags))
            .map(|r| r.display_name(&config.naming))
            .collect(),
    };
    for name in selected {
        if !output::emit(name)? {
            break;
        }
    }
    Ok(())
}

fn print_table(views: &[ResourceView<'_>]) -> Result<()> {
    let mut rows: Vec<[String; 5]> = vec![["NAME", "KIND", "ENV", "ZONE", "IP"].map(str::to_owned)];
    rows.extend(views.iter().map(|view| {
        [
            view.display_name.clone(),
            view.kind.to_owned(),
            view.env.map(|env| env.to_string()).unwrap_or_default(),
            view.zone.to_owned(),
            view.endpoint_ip.unwrap_or_default().to_owned(),
        ]
    }));

    for line in table::columns(&rows) {
        if !output::emit(&line)? {
            return Ok(());
        }
    }
    Ok(())
}
