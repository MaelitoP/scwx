use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

use crate::cli::{Cli, PfCommand};
use crate::config::Config;
use crate::inventory::Resource;
use crate::picker::{self, Selection};
use crate::{cache, output, paths, ssh};

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
        .filter(|resource| resource.port_forward_enabled(&config.tags) || remote_port.is_some())
        .collect();
    ensure!(
        !candidates.is_empty(),
        "no resources tagged {} (tag one, or pass --remote-port with a query)",
        config.tags.port_forward_enabled
    );

    let lines = picker::render_resources(&candidates, config);
    let target = match picker::select(
        &candidates,
        &lines,
        "Forward a port to",
        query,
        &config.naming,
        false,
    )? {
        Selection::Direct(resource) | Selection::Picked(resource, _) => resource,
        Selection::NoMatch => bail!(
            "no forwardable resource matches '{}'",
            query.unwrap_or_default()
        ),
        Selection::Cancelled => return Ok(()),
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
    if socket.exists() {
        // A socket can outlive its master (kill -9, reboot); probe before
        // refusing, or a stale file blocks this target forever.
        let probe = TunnelRecord {
            name: name.clone(),
            local_port,
            remote_port,
            target: String::new(),
            destination: "stale-probe".to_owned(),
        };
        match master_state(&sockets, &probe) {
            MasterState::Alive => {
                bail!("tunnel {name} is already running (scwx pf stop {name})")
            }
            MasterState::Dead => {
                eprintln!("pruning stale socket for {name}");
                remove(&sockets, &name);
            }
            MasterState::Unknown => {
                bail!("cannot probe the existing socket for {name} (is ssh installed?)")
            }
        }
    }

    let (command, target_label) = if target.kind.is_server() {
        let tunnel = ssh::Tunnel {
            local_port,
            target_host: &target.name,
            remote_port,
        };
        (
            ssh::server_tunnel(&tunnel, config, &bastion)?,
            format!("{}:{remote_port}", target.name),
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
            ssh::bastion_tunnel(&tunnel, config, &bastion)?,
            format!("{host}:{remote_port}"),
        )
    };
    let destination = command.destination().to_owned();
    let argv = command.with_control_socket(&socket).into_argv();

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

    output::emit(&format!("{name}: 127.0.0.1:{local_port} -> {target_label}"))?;
    Ok(())
}

fn socket_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.sock"))
}

fn record_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.json"))
}

fn records(dir: &Path) -> Vec<TunnelRecord> {
    let Ok(entries) = fs::read_dir(dir) else {
        return vec![];
    };
    let mut records: Vec<TunnelRecord> = entries
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            if path.extension()? != "json" {
                return None;
            }
            let raw = fs::read_to_string(&path).ok()?;
            let record = serde_json::from_str(&raw);
            if record.is_err() {
                eprintln!("warning: ignoring unreadable record {}", path.display());
            }
            record.ok()
        })
        .collect();
    records.sort_by(|a, b| a.name.cmp(&b.name));
    records
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MasterState {
    Alive,
    Dead,
    /// ssh itself could not run; says nothing about the master.
    Unknown,
}

fn control_command(dir: &Path, record: &TunnelRecord, operation: &str) -> Command {
    let socket = socket_path(dir, &record.name);
    let mut command = Command::new("ssh");
    command
        .arg("-S")
        .arg(socket)
        .args(["-O", operation])
        .arg(&record.destination)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

fn master_state(dir: &Path, record: &TunnelRecord) -> MasterState {
    match control_command(dir, record, "check").status() {
        Ok(status) if status.success() => MasterState::Alive,
        Ok(_) => MasterState::Dead,
        Err(_) => MasterState::Unknown,
    }
}

fn remove(dir: &Path, name: &str) {
    let _ = fs::remove_file(record_path(dir, name));
    let _ = fs::remove_file(socket_path(dir, name));
}

fn list() -> Result<()> {
    let dir = paths::sockets_dir()?;
    let mut printed = false;
    for record in records(&dir) {
        match master_state(&dir, &record) {
            MasterState::Dead => {
                remove(&dir, &record.name);
                continue;
            }
            MasterState::Unknown => {
                eprintln!("warning: could not check {}; keeping it", record.name);
                continue;
            }
            MasterState::Alive => {}
        }
        if !output::emit(&format!(
            "{}  127.0.0.1:{} -> {}",
            record.name, record.local_port, record.target
        ))? {
            return Ok(());
        }
        printed = true;
    }
    if !printed {
        eprintln!("no active tunnels");
    }
    Ok(())
}

fn matching<'a>(records: &'a [TunnelRecord], name: Option<&str>) -> Vec<&'a TunnelRecord> {
    match name {
        Some(name) => records
            .iter()
            .filter(|record| record.name.contains(name))
            .collect(),
        None => records.iter().collect(),
    }
}

fn stop(name: Option<&str>) -> Result<()> {
    let dir = paths::sockets_dir()?;
    let records = records(&dir);
    ensure!(!records.is_empty(), "no active tunnels");

    let selected = matching(&records, name);
    if let Some(name) = name {
        ensure!(!selected.is_empty(), "no tunnel matches '{name}'");
    }

    for record in selected {
        let exited = control_command(&dir, record, "exit")
            .status()
            .is_ok_and(|status| status.success());
        // The master may already be gone; confirm before deleting its
        // bookkeeping, or a live tunnel becomes unstoppable.
        if exited || master_state(&dir, record) != MasterState::Alive {
            remove(&dir, &record.name);
            output::emit(&format!("stopped {}", record.name))?;
        } else {
            eprintln!(
                "warning: {} did not exit; keeping its socket in place",
                record.name
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(name: &str) -> TunnelRecord {
        TunnelRecord {
            name: name.to_owned(),
            local_port: 13306,
            remote_port: 3306,
            target: "172.16.0.1:3306".to_owned(),
            destination: "bastion@5.6.7.8".to_owned(),
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("scwx-pf-{name}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn records_reads_json_files_and_warns_on_garbage() {
        let dir = temp_dir("records");
        fs::write(
            record_path(&dir, "good"),
            serde_json::to_string(&record("good")).unwrap(),
        )
        .unwrap();
        fs::write(record_path(&dir, "bad"), "not json").unwrap();
        fs::write(dir.join("ignored.sock"), "").unwrap();

        let records = records(&dir);
        fs::remove_dir_all(&dir).unwrap();

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].name, "good");
    }

    #[test]
    fn records_of_a_missing_dir_is_empty() {
        assert!(records(Path::new("/nonexistent/scwx-sockets")).is_empty());
    }

    #[test]
    fn matching_filters_by_substring_and_none_selects_all() {
        let records = vec![record("api-8080"), record("redis-commons-6379")];

        let all = matching(&records, None);
        assert_eq!(all.len(), 2);

        let redis = matching(&records, Some("redis"));
        assert_eq!(redis.len(), 1);
        assert_eq!(redis[0].name, "redis-commons-6379");

        assert!(matching(&records, Some("nope")).is_empty());
    }
}
