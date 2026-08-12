use std::env;
use std::ffi::OsString;
use std::fs;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};

use crate::cli::Scope;
use crate::config::{Config, Credentials};
use crate::database::{db_key, is_database};
use crate::inventory::{Bastion, Environment, Resource, ResourceKind};
use crate::picker::{self, PickOutcome, Selection};
use crate::{cache, paths, scw, secrets, ssh, table, tmux};

#[derive(Debug, Clone, Copy)]
pub struct MysqlOptions<'a> {
    pub execute: Option<&'a str>,
    pub extra_args: &'a [String],
}

pub fn run(
    scope: &Scope,
    config: &Config,
    name: Option<&str>,
    mysql: MysqlOptions<'_>,
) -> Result<()> {
    let env = scope.env.unwrap_or(config.db.default_env);
    let inventory = cache::load_or_fetch(scope.freshness, config)?;
    let bastion = inventory.require_bastion()?.clone();

    let databases: Vec<&Resource> = inventory
        .filtered(Some(env), &config.tags)
        .filter(|resource| is_database(resource, config))
        .collect();
    ensure!(!databases.is_empty(), "no databases tagged for env {env}");

    let lines = render(&databases, config, env);
    let (target, outcome) = match picker::select(
        &databases,
        &lines,
        &format!("Connect to database ({env})"),
        name,
        &config.naming,
        true,
    )? {
        Selection::Direct(target) => (target, PickOutcome::Inline),
        Selection::Picked(target, outcome) => (target, outcome),
        Selection::NoMatch => bail!("no {env} database matches '{}'", name.unwrap_or_default()),
        Selection::Cancelled => return Ok(()),
    };

    if let Some(placement) = outcome.placement()
        && tmux::inside_tmux()
    {
        let argv: Vec<OsString> = [env::current_exe()
            .context("resolving scwx path")?
            .into_os_string()]
        .into_iter()
        .chain(
            ["db", &target.name, "--env", env.as_str()]
                .into_iter()
                .map(OsString::from),
        )
        .chain(
            mysql
                .execute
                .iter()
                .flat_map(|query| ["--execute", query])
                .map(OsString::from),
        )
        .chain(std::iter::once(OsString::from("--")))
        .chain(mysql.extra_args.iter().map(OsString::from))
        .collect();
        let title = format!("db:{}", db_key(target, config, env));
        return tmux::open(placement, &title, &argv);
    }

    connect(target, env, config, &bastion, mysql)
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
                resource.display_name(&config.naming).to_owned(),
                resource.zone.clone(),
            ]
        })
        .collect();
    table::columns(&rows)
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
    mysql: MysqlOptions<'_>,
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

    let local_port = free_local_port(preferred_local_port(remote_port))?;
    let tunnel = ssh::Tunnel {
        local_port,
        target_host: host,
        remote_port,
    };
    let argv = ssh::bastion_tunnel(&tunnel, config, bastion)?.into_argv();
    // The tunnel must not read stdin: piped queries belong to mysql.
    let mut tunnel_child = Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(Stdio::null())
        .spawn()
        .context("starting ssh tunnel")?;

    let result = wait_for_port(&mut tunnel_child, local_port).and_then(|()| {
        eprintln!("connected to {key} ({env}) via 127.0.0.1:{local_port}");
        let defaults_file = write_mysql_defaults(password.expose())?;
        let status = Command::new("mysql")
            .args(mysql_argv(&defaults_file, local_port, &user, &key, mysql))
            .status()
            .context("running mysql (is it installed?)");
        let _ = fs::remove_file(&defaults_file);
        status
    });

    let _ = tunnel_child.kill();
    let _ = tunnel_child.wait();

    let status = result?;
    ensure!(status.success(), "mysql exited with {status}");
    Ok(())
}

/// The password travels in a 0600 option file: MYSQL_PWD is readable in
/// /proc and inherited by pagers and shell escapes.
fn write_mysql_defaults(password: &str) -> Result<PathBuf> {
    let path = env::temp_dir().join(format!("scwx-mysql-{}.cnf", std::process::id()));
    let _ = fs::remove_file(&path);
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    let mut file = options
        .open(&path)
        .with_context(|| format!("creating {}", path.display()))?;
    use std::io::Write as _;
    file.write_all(mysql_defaults_content(password).as_bytes())
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

fn mysql_defaults_content(password: &str) -> String {
    let escaped = password.replace('\\', "\\\\").replace('"', "\\\"");
    format!("[client]\npassword=\"{escaped}\"\n")
}

fn mysql_argv(
    defaults_file: &Path,
    local_port: u16,
    user: &str,
    schema: &str,
    mysql: MysqlOptions<'_>,
) -> Vec<String> {
    let mut argv = vec![
        // mysql requires any defaults option to come first.
        format!("--defaults-extra-file={}", defaults_file.display()),
        "-h".to_owned(),
        "127.0.0.1".to_owned(),
        "-P".to_owned(),
        local_port.to_string(),
        "-u".to_owned(),
        user.to_owned(),
    ];
    if let Some(query) = mysql.execute {
        argv.push("--execute".to_owned());
        argv.push(query.to_owned());
    }
    argv.extend(mysql.extra_args.iter().cloned());
    argv.push(schema.to_owned());
    argv
}

fn preferred_local_port(remote_port: u16) -> u16 {
    u16::try_from(10_000 + u32::from(remote_port))
        .unwrap_or(remote_port)
        .max(1024)
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

    #[test]
    fn mysql_argv_places_execute_and_extra_args_before_the_schema() {
        let options = MysqlOptions {
            execute: Some("SELECT 1"),
            extra_args: &["--table".to_owned()],
        };
        let argv = mysql_argv(
            Path::new("/tmp/d.cnf"),
            13306,
            "mael_lepetit",
            "search",
            options,
        );
        assert_eq!(
            argv,
            [
                "--defaults-extra-file=/tmp/d.cnf",
                "-h",
                "127.0.0.1",
                "-P",
                "13306",
                "-u",
                "mael_lepetit",
                "--execute",
                "SELECT 1",
                "--table",
                "search",
            ]
        );

        let plain = mysql_argv(
            Path::new("/tmp/d.cnf"),
            13306,
            "u",
            "s",
            MysqlOptions {
                execute: None,
                extra_args: &[],
            },
        );
        assert_eq!(plain.last().unwrap(), "s");
        assert!(!plain.contains(&"--execute".to_owned()));
    }

    #[test]
    fn mysql_defaults_escape_quotes_and_backslashes() {
        assert_eq!(
            mysql_defaults_content(r#"pa"ss\word"#),
            "[client]\npassword=\"pa\\\"ss\\\\word\"\n"
        );
    }

    #[test]
    fn preferred_local_port_never_overflows() {
        assert_eq!(preferred_local_port(3306), 13306);
        assert_eq!(preferred_local_port(61000), 61000);
        assert_eq!(preferred_local_port(65535), 65535);
        assert_eq!(preferred_local_port(0), 10000);
    }

    #[test]
    fn free_local_port_skips_a_taken_port() {
        let taken = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let taken_port = taken.local_addr().unwrap().port();

        let port = free_local_port(taken_port).unwrap();
        assert!(port > taken_port);
    }
}
