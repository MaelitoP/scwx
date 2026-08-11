use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

use crate::cli::{Cli, PfCommand};
use crate::config::Config;
use crate::inventory::Resource;
use crate::{cache, paths, picker, ssh};

pub fn run(
    cli: &Cli,
    config: &Config,
    command: Option<&PfCommand>,
    query: Option<&str>,
    local_port: Option<u16>,
    remote_port: Option<u16>,
) -> Result<()> {
    match command {
        Some(PfCommand::Ls) => list(),
        Some(PfCommand::Stop { name }) => stop(name.as_deref()),
        None => start(cli, config, query, local_port, remote_port),
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct TunnelRecord {
    name: String,
    local_port: u16,
    remote_port: u16,
    target: String,
    /// Destination argument required by ssh -O check/exit.
    destination: String,
}

fn start(
    cli: &Cli,
    config: &Config,
    query: Option<&str>,
    local_port: Option<u16>,
    remote_port: Option<u16>,
) -> Result<()> {
    let inventory = cache::load_or_fetch(cli.refresh, config)?;
    let bastion = inventory.bastion()?.clone();

    let candidates: Vec<&Resource> = inventory
        .filtered(cli.env, &config.tags)
        .into_iter()
        .filter(|resource| {
            resource.port_forward_enabled(&config.tags)
                || (remote_port.is_some() && resource.matches(query.unwrap_or(""), &config.naming))
        })
        .collect();
    ensure!(
        !candidates.is_empty(),
        "no resources tagged {} (tag one, or pass --remote-port with a query)",
        config.tags.port_forward_enabled
    );

    let target = match query {
        Some(query) => {
            let matches: Vec<&Resource> = candidates
                .iter()
                .copied()
                .filter(|resource| resource.matches(query, &config.naming))
                .collect();
            match matches.len() {
                0 => bail!("no forwardable resource matches '{query}'"),
                1 => matches[0],
                _ => match pick(&matches, config, query)? {
                    Some(resource) => resource,
                    None => return Ok(()),
                },
            }
        }
        None => match pick(&candidates, config, "")? {
            Some(resource) => resource,
            None => return Ok(()),
        },
    };

    let remote_port = remote_port
        .or_else(|| target.port_forward_port(&config.tags))
        .with_context(|| {
            format!(
                "no remote port known for {}: tag it with {}<port> or pass --remote-port",
                target.name, config.tags.port_forward_prefix
            )
        })?;
    let local_port = local_port.unwrap_or(remote_port);

    let name = format!("{}-{local_port}", target.display_name(&config.naming));
    let sockets = paths::sockets_dir()?;
    fs::create_dir_all(&sockets).with_context(|| format!("creating {}", sockets.display()))?;
    let socket = socket_path(&sockets, &name);
    ensure!(
        !socket.exists(),
        "tunnel {name} already exists (scwx pf stop {name})"
    );

    let (argv, target_label, destination) = if target.kind.is_server() {
        let tunnel = ssh::Tunnel {
            local_port,
            target_host: &target.name,
            remote_port,
        };
        (
            ssh::server_tunnel_argv(&tunnel, config, &bastion)?,
            format!("{}:{remote_port}", target.name),
            format!("{}@{}", config.ssh.user, target.name),
        )
    } else {
        let host = target
            .endpoint_ip
            .as_deref()
            .with_context(|| format!("no endpoint ip known for {}", target.name))?;
        let tunnel = ssh::Tunnel {
            local_port,
            target_host: host,
            remote_port,
        };
        (
            ssh::tunnel_argv(&tunnel, config, &bastion)?,
            format!("{host}:{remote_port}"),
            bastion.destination(&config.bastion.user),
        )
    };
    let argv = ssh::with_control_socket(argv, &socket);

    let status = Command::new(&argv[0])
        .args(&argv[1..])
        .status()
        .context("starting ssh tunnel")?;
    ensure!(status.success(), "ssh tunnel failed with {status}");

    let record = TunnelRecord {
        name: name.clone(),
        local_port,
        remote_port,
        target: target_label.clone(),
        destination,
    };
    fs::write(
        record_path(&sockets, &name),
        serde_json::to_string(&record)?,
    )
    .context("writing tunnel record")?;

    println!("{name}: 127.0.0.1:{local_port} -> {target_label}");
    Ok(())
}

fn pick<'a>(
    resources: &[&'a Resource],
    config: &Config,
    query: &str,
) -> Result<Option<&'a Resource>> {
    let lines = picker::render_resources(resources, config);
    let query = (!query.is_empty()).then_some(query);
    Ok(picker::pick_plain(&lines, "Forward a port to", query)?.map(|index| resources[index]))
}

fn socket_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.sock"))
}

fn record_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.json"))
}

fn records() -> Result<Vec<TunnelRecord>> {
    let dir = paths::sockets_dir()?;
    let Ok(entries) = fs::read_dir(&dir) else {
        return Ok(vec![]);
    };
    let mut records: Vec<TunnelRecord> = entries
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            if path.extension()? != "json" {
                return None;
            }
            serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
        })
        .collect();
    records.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(records)
}

fn is_alive(dir: &Path, record: &TunnelRecord) -> bool {
    let socket = socket_path(dir, &record.name);
    Command::new("ssh")
        .args(["-S", &socket.to_string_lossy(), "-O", "check"])
        .arg(&record.destination)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn remove(dir: &Path, name: &str) {
    let _ = fs::remove_file(record_path(dir, name));
    let _ = fs::remove_file(socket_path(dir, name));
}

fn list() -> Result<()> {
    let dir = paths::sockets_dir()?;
    let mut printed = false;
    for record in records()? {
        if !is_alive(&dir, &record) {
            remove(&dir, &record.name);
            continue;
        }
        println!(
            "{}  127.0.0.1:{} -> {}",
            record.name, record.local_port, record.target
        );
        printed = true;
    }
    if !printed {
        eprintln!("no active tunnels");
    }
    Ok(())
}

fn stop(name: Option<&str>) -> Result<()> {
    let dir = paths::sockets_dir()?;
    let records = records()?;
    ensure!(!records.is_empty(), "no active tunnels");

    let selected: Vec<&TunnelRecord> = match name {
        Some(name) => {
            let matched: Vec<&TunnelRecord> = records
                .iter()
                .filter(|record| record.name.contains(name))
                .collect();
            ensure!(!matched.is_empty(), "no tunnel matches '{name}'");
            matched
        }
        None => records.iter().collect(),
    };

    for record in selected {
        let socket = socket_path(&dir, &record.name);
        let _ = Command::new("ssh")
            .args(["-S", &socket.to_string_lossy(), "-O", "exit"])
            .arg(&record.destination)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        remove(&dir, &record.name);
        println!("stopped {}", record.name);
    }
    Ok(())
}
