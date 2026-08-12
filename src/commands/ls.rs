use anyhow::Result;
use serde::Serialize;

use crate::cli::Cli;
use crate::config::Config;
use crate::inventory::{Environment, Resource};
use crate::{cache, output};

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
            display_name: resource.display_name(&config.naming),
            zone: &resource.zone,
            env: resource.env(&config.tags),
            tags: &resource.tags,
            endpoint_ip: resource.endpoint_ip.as_deref(),
            endpoint_port: resource.endpoint_port,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct NameFilter {
    pub servers: bool,
    pub databases: bool,
    pub forwardable: bool,
}

pub fn run(cli: &Cli, config: &Config, json: bool, filter: NameFilter, cached: bool) -> Result<()> {
    let inventory = if cached {
        match cache::load_ignoring_ttl()? {
            Some(inventory) => inventory,
            None => return Ok(()),
        }
    } else {
        cache::load_or_fetch(cli.refresh, config)?
    };
    let resources = inventory.filtered(cli.env, &config.tags);

    if filter.servers {
        for resource in resources.iter().filter(|r| r.kind.is_server()) {
            if !output::emit(&resource.display_name(&config.naming))? {
                break;
            }
        }
        return Ok(());
    }
    if filter.databases {
        for name in crate::commands::db::names(&resources, config)? {
            if !output::emit(&name)? {
                break;
            }
        }
        return Ok(());
    }
    if filter.forwardable {
        for resource in resources
            .iter()
            .filter(|r| r.port_forward_enabled(&config.tags))
        {
            if !output::emit(&resource.display_name(&config.naming))? {
                break;
            }
        }
        return Ok(());
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

fn print_table(views: &[ResourceView<'_>]) -> Result<()> {
    let rows: Vec<[String; 5]> = views
        .iter()
        .map(|view| {
            [
                view.display_name.clone(),
                view.kind.to_owned(),
                view.env.map(|env| env.to_string()).unwrap_or_default(),
                view.zone.to_owned(),
                view.endpoint_ip.unwrap_or_default().to_owned(),
            ]
        })
        .collect();

    let header = ["NAME", "KIND", "ENV", "ZONE", "IP"];
    let mut widths = header.map(str::len);
    for row in &rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.len());
        }
    }

    if !print_row(&header.map(str::to_owned), &widths)? {
        return Ok(());
    }
    for row in &rows {
        if !print_row(row, &widths)? {
            return Ok(());
        }
    }
    Ok(())
}

fn print_row(cells: &[String; 5], widths: &[usize; 5]) -> Result<bool> {
    let line = cells
        .iter()
        .zip(widths)
        .map(|(cell, width)| format!("{cell:<width$}"))
        .collect::<Vec<_>>()
        .join("  ");
    output::emit(line.trim_end())
}
