use std::env;
use std::ffi::OsString;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};

use crate::cli::Cli;
use crate::config::{Config, Credentials};
use crate::inventory::{Bastion, Environment, Resource, ResourceKind};
use crate::picker::{self, PickOutcome};
use crate::{cache, paths, scw, secrets, ssh, tmux};

pub fn run(cli: &Cli, config: &Config, name: Option<&str>) -> Result<()> {
    let env = target_env(cli, config)?;
    let inventory = cache::load_or_fetch(cli.refresh, config)?;
    let bastion = inventory.bastion()?.clone();

    let databases: Vec<&Resource> = inventory
        .resources
        .iter()
        .filter(|resource| is_database(resource, config))
        .filter(|resource| resource.env(&config.tags) == Some(env))
        .collect();
    ensure!(!databases.is_empty(), "no databases tagged for env {env}");

    if let Some(name) = name {
        let exact: Vec<&Resource> = databases
            .iter()
            .copied()
            .filter(|resource| resource.name == name || db_key(resource, config, env) == name)
            .collect();
        if let [target] = exact.as_slice() {
            return connect(target, env, config, &bastion);
        }
    }

    let lines = render(&databases, config, env);
    let Some(pick) = picker::pick(&lines, &format!("Connect to database ({env})"), name)? else {
        return Ok(());
    };
    let target = databases[pick.index];

    if pick.outcome != PickOutcome::Inline && tmux::inside_tmux() {
        let argv: Vec<OsString> = [env::current_exe()
            .context("resolving scwx path")?
            .into_os_string()]
        .into_iter()
        .chain(
            ["db", &target.name, "--env", env.as_str()]
                .into_iter()
                .map(OsString::from),
        )
        .collect();
        let placement = match pick.outcome {
            PickOutcome::Window => tmux::Placement::Window,
            PickOutcome::Split => tmux::Placement::Split,
            PickOutcome::VSplit => tmux::Placement::VSplit,
            PickOutcome::Inline => unreachable!(),
        };
        let title = format!("db:{}", db_key(target, config, env));
        return tmux::open(placement, &title, &argv);
    }

    connect(target, env, config, &bastion)
}

fn target_env(cli: &Cli, config: &Config) -> Result<Environment> {
    match cli.env {
        Some(env) => Ok(env),
        None => config
            .db
            .default_env
            .parse()
            .with_context(|| format!("invalid db.default_env '{}'", config.db.default_env)),
    }
}

fn is_database(resource: &Resource, config: &Config) -> bool {
    match resource.kind {
        ResourceKind::Rdb => true,
        ResourceKind::Baremetal => resource.is_mysql(&config.tags),
        ResourceKind::Instance | ResourceKind::Redis | ResourceKind::Lb => false,
    }
}

/// Short database key: display name without the env segment and shard suffix.
/// `platform-ingestor-prod-search-2` -> `search`.
fn db_key(resource: &Resource, config: &Config, env: Environment) -> String {
    let mut key = resource.display_name(&config.naming);
    if let Some(rest) = key.strip_prefix(&format!("{env}-")) {
        key = rest.to_owned();
    }
    if let Some(rest) = config
        .db
        .strip_prefixes
        .iter()
        .find_map(|prefix| key.strip_prefix(prefix))
    {
        key = rest.to_owned();
    }
    if let Some(position) = key.rfind('-')
        && !key[position + 1..].is_empty()
        && key[position + 1..].chars().all(|c| c.is_ascii_digit())
    {
        key.truncate(position);
    }
    key
}

fn render(databases: &[&Resource], config: &Config, env: Environment) -> Vec<String> {
    let rows: Vec<[String; 4]> = databases
        .iter()
        .map(|resource| {
            let role = match resource.kind {
                ResourceKind::Rdb => "managed".to_owned(),
                _ if resource.is_master(&config.tags) => "master".to_owned(),
                _ => "replica".to_owned(),
            };
            [
                db_key(resource, config, env),
                role,
                resource.display_name(&config.naming),
                resource.zone.clone(),
            ]
        })
        .collect();

    let mut widths = [0usize; 4];
    for row in &rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.len());
        }
    }
    rows.iter()
        .map(|row| {
            row.iter()
                .zip(widths)
                .map(|(cell, width)| format!("{cell:<width$}"))
                .collect::<Vec<_>>()
                .join("  ")
                .trim_end()
                .to_owned()
        })
        .collect()
}

fn database_user(config: &Config) -> Result<String> {
    if let Some(user) = &config.db.user {
        return Ok(user.clone());
    }
    let os_user = env::var("USER").context("USER is not set and db.user is not configured")?;
    Ok(os_user.replace('.', "_"))
}

fn connect(
    resource: &Resource,
    env: Environment,
    config: &Config,
    bastion: &Bastion,
) -> Result<()> {
    let host = resource
        .endpoint_ip
        .as_deref()
        .with_context(|| format!("no endpoint ip known for {}", resource.name))?;
    let remote_port = resource.port_forward_port(&config.tags).unwrap_or(3306);
    let key = db_key(resource, config, env);
    let user = database_user(config)?;

    let credentials = Credentials::load(&paths::scw_config_file()?)?;
    let project_id = config
        .db
        .secret_project_id
        .clone()
        .or_else(|| credentials.default_project_id.clone())
        .context("db.secret_project_id is not configured")?;
    let region = config
        .scaleway
        .regions
        .first()
        .context("no scaleway region configured")?;
    let secret_name = secrets::secret_name(&config.db.secret_name_template, &key, &user, env);
    let client = scw::Client::new(&credentials);
    let password = secrets::access_secret(&client, region, &project_id, &secret_name)?;

    let local_port = free_local_port(10000 + remote_port)?;
    let tunnel = ssh::Tunnel {
        local_port,
        target_host: host,
        remote_port,
    };
    let argv = ssh::tunnel_argv(&tunnel, config, bastion)?;
    let mut tunnel_child = Command::new(&argv[0])
        .args(&argv[1..])
        .spawn()
        .context("starting ssh tunnel")?;

    let result = wait_for_port(&mut tunnel_child, local_port).and_then(|()| {
        eprintln!("connected to {key} ({env}) via 127.0.0.1:{local_port}");
        Command::new("mysql")
            .args([
                "-h",
                "127.0.0.1",
                "-P",
                &local_port.to_string(),
                "-u",
                &user,
                &key,
            ])
            .env("MYSQL_PWD", &password)
            .status()
            .context("running mysql (is it installed?)")
    });

    let _ = tunnel_child.kill();
    let _ = tunnel_child.wait();

    let status = result?;
    ensure!(status.success(), "mysql exited with {status}");
    Ok(())
}

fn free_local_port(preferred: u16) -> Result<u16> {
    let mut port = preferred;
    loop {
        if TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return Ok(port);
        }
        port = port
            .checked_add(1)
            .with_context(|| format!("no free local port above {preferred}"))?;
    }
}

fn wait_for_port(child: &mut Child, port: u16) -> Result<()> {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = child.try_wait().context("checking ssh tunnel")? {
            bail!("ssh tunnel exited with {status}");
        }
        if TcpStream::connect_timeout(&address, Duration::from_millis(200)).is_ok() {
            return Ok(());
        }
        ensure!(
            Instant::now() < deadline,
            "ssh tunnel did not open port {port} within 15s"
        );
        thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resource(name: &str, tags: &[&str]) -> Resource {
        Resource {
            kind: ResourceKind::Baremetal,
            id: "id".to_owned(),
            name: name.to_owned(),
            zone: "fr-par-1".to_owned(),
            tags: tags.iter().map(|t| (*t).to_owned()).collect(),
            endpoint_ip: None,
            endpoint_port: None,
        }
    }

    fn config() -> Config {
        let mut config = Config::default();
        config.naming.strip_prefixes = vec!["platform-ingestor-".to_owned()];
        config
    }

    #[test]
    fn db_key_strips_prefix_env_and_shard() {
        let r = resource("platform-ingestor-prod-search-2", &[]);
        assert_eq!(db_key(&r, &config(), Environment::Prod), "search");
    }

    #[test]
    fn db_key_keeps_inner_digits_and_foreign_env() {
        let r = resource("platform-ingestor-db-matched-article-1", &[]);
        assert_eq!(db_key(&r, &config(), Environment::Prod), "matched-article");

        let r = resource("platform-ingestor-beta-saga", &[]);
        assert_eq!(db_key(&r, &config(), Environment::Prod), "beta-saga");
        assert_eq!(db_key(&r, &config(), Environment::Beta), "saga");
    }

    #[test]
    fn only_rdb_and_mysql_tagged_baremetal_are_databases() {
        let config = config();
        let mut rdb = resource("a", &[]);
        rdb.kind = ResourceKind::Rdb;
        assert!(is_database(&rdb, &config));

        assert!(is_database(&resource("b", &["Mysql"]), &config));
        assert!(!is_database(&resource("c", &[]), &config));

        let mut instance = resource("d", &["Mysql"]);
        instance.kind = ResourceKind::Instance;
        assert!(!is_database(&instance, &config));
    }

    #[test]
    fn free_local_port_skips_a_taken_port() {
        let taken = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let taken_port = taken.local_addr().unwrap().port();

        let port = free_local_port(taken_port).unwrap();
        assert!(port > taken_port);
    }
}
