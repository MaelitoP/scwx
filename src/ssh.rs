//! ssh invocations: sessions and tunnels through the bastion.

use std::ffi::OsString;
use std::path::Path;

use anyhow::Result;

use crate::config::Config;
use crate::inventory::Bastion;
use crate::paths;
use crate::shell::shell_join;

/// Host keys churn as servers are rebuilt and resolve behind the bastion,
/// so pinning them only produces noise and spurious failures.
pub(crate) const HARDENING_OPTIONS: [(&str, &str); 3] = [
    ("StrictHostKeyChecking", "no"),
    ("UserKnownHostsFile", "/dev/null"),
    ("LogLevel", "ERROR"),
];

const TUNNEL_OPTIONS: [(&str, &str); 3] = [
    ("ServerAliveInterval", "60"),
    ("ServerAliveCountMax", "3"),
    ("ExitOnForwardFailure", "yes"),
];

/// Renders options as `-o key=value` argument pairs.
fn option_args(options: &[(&str, &str)]) -> Vec<String> {
    options
        .iter()
        .flat_map(|(key, value)| ["-o".to_owned(), format!("{key}={value}")])
        .collect()
}

/// Renders options as indented ssh_config lines, for sync-ssh.
pub(crate) fn option_config_lines(options: &[(&str, &str)]) -> String {
    options
        .iter()
        .map(|(key, value)| format!("    {key} {value}\n"))
        .collect()
}

/// An ssh invocation whose destination stays explicit until the end, so
/// options can always be appended safely.
#[derive(Debug)]
pub(crate) struct SshCommand {
    options: Vec<OsString>,
    destination: String,
}

impl SshCommand {
    fn new(destination: String) -> Self {
        Self {
            options: Vec::new(),
            destination,
        }
    }

    fn push_args<I: IntoIterator<Item = String>>(&mut self, args: I) {
        self.options.extend(args.into_iter().map(OsString::from));
    }

    /// Backgrounds the connection as a control master so it can be checked
    /// and stopped later via ssh -S <socket> -O check/exit.
    pub(crate) fn with_control_socket(mut self, socket: &Path) -> Self {
        self.options.extend(["-f", "-M", "-S"].map(OsString::from));
        self.options.push(socket.into());
        self
    }

    pub(crate) fn destination(&self) -> &str {
        &self.destination
    }

    pub(crate) fn into_argv(self) -> Vec<OsString> {
        let mut argv = vec![OsString::from("ssh")];
        argv.extend(self.options);
        argv.push(OsString::from(self.destination));
        argv
    }
}

fn identity_key(config: &Config) -> Result<Option<String>> {
    config
        .ssh
        .key
        .as_deref()
        .map(|key| Ok(paths::expand_tilde(key)?.to_string_lossy().into_owned()))
        .transpose()
}

fn base_options(key: Option<&str>) -> Vec<String> {
    let mut options = option_args(&HARDENING_OPTIONS);
    if let Some(key) = key {
        options.push("-o".to_owned());
        options.push("IdentitiesOnly=yes".to_owned());
        options.push("-i".to_owned());
        options.push(key.to_owned());
    }
    options
}

/// ProxyCommand carrying its own options: ssh does not propagate
/// command-line options to the jump connection.
fn proxy_command(bastion: &Bastion, config: &Config, key: Option<&str>) -> String {
    let mut argv = vec![
        "ssh".to_owned(),
        "-W".to_owned(),
        "%h:%p".to_owned(),
        "-p".to_owned(),
        bastion.port.to_string(),
    ];
    argv.extend(base_options(key));
    argv.push(bastion.destination(&config.bastion.user));
    shell_join(&argv)
}

fn through_bastion(config: &Config, bastion: &Bastion, destination: String) -> Result<SshCommand> {
    let key = identity_key(config)?;
    let mut command = SshCommand::new(destination);
    command.push_args(base_options(key.as_deref()));
    command.push_args([
        "-o".to_owned(),
        format!(
            "ProxyCommand={}",
            proxy_command(bastion, config, key.as_deref())
        ),
    ]);
    Ok(command)
}

/// Interactive session to a server, reached by name through the bastion.
pub(crate) fn session(host: &str, config: &Config, bastion: &Bastion) -> Result<SshCommand> {
    through_bastion(config, bastion, format!("{}@{host}", config.ssh.user))
}

pub(crate) struct Tunnel<'a> {
    pub(crate) local_port: u16,
    pub(crate) target_host: &'a str,
    pub(crate) remote_port: u16,
}

/// Forward through the bastion itself: -L local:target:remote on the
/// bastion connection, for targets the bastion can reach directly.
pub(crate) fn bastion_tunnel(
    tunnel: &Tunnel<'_>,
    config: &Config,
    bastion: &Bastion,
) -> Result<SshCommand> {
    let key = identity_key(config)?;
    let mut command = SshCommand::new(bastion.destination(&config.bastion.user));
    command.push_args(base_options(key.as_deref()));
    command.push_args(option_args(&TUNNEL_OPTIONS));
    command.push_args([
        "-L".to_owned(),
        format!(
            "{}:{}:{}",
            tunnel.local_port, tunnel.target_host, tunnel.remote_port
        ),
        "-N".to_owned(),
        "-p".to_owned(),
        bastion.port.to_string(),
    ]);
    Ok(command)
}

/// Forward to a server's own loopback, jumping through the bastion.
pub(crate) fn server_tunnel(
    tunnel: &Tunnel<'_>,
    config: &Config,
    bastion: &Bastion,
) -> Result<SshCommand> {
    let mut command = through_bastion(
        config,
        bastion,
        format!("{}@{}", config.ssh.user, tunnel.target_host),
    )?;
    command.push_args(option_args(&TUNNEL_OPTIONS));
    command.push_args([
        "-L".to_owned(),
        format!("{}:localhost:{}", tunnel.local_port, tunnel.remote_port),
        "-N".to_owned(),
    ]);
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bastion() -> Bastion {
        Bastion {
            ip: "5.6.7.8".to_owned(),
            port: 61000,
            zone: "fr-par-1".to_owned(),
        }
    }

    fn config_with_key() -> Config {
        let mut config = Config::default();
        config.ssh.key = Some("/keys/id_ed25519".to_owned());
        config
    }

    fn as_strings(argv: Vec<OsString>) -> Vec<String> {
        argv.into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn session_jumps_through_the_bastion_with_the_key_on_both_hops() {
        let argv = as_strings(
            session("web-1", &config_with_key(), &bastion())
                .unwrap()
                .into_argv(),
        );

        assert_eq!(argv[0], "ssh");
        assert_eq!(argv.last().unwrap(), "root@web-1");
        assert!(argv.contains(&"/keys/id_ed25519".to_owned()));
        let proxy = argv
            .iter()
            .find(|arg| arg.starts_with("ProxyCommand="))
            .unwrap();
        assert_eq!(
            proxy,
            "ProxyCommand=ssh -W %h:%p -p 61000 -o StrictHostKeyChecking=no \
             -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR \
             -o IdentitiesOnly=yes -i /keys/id_ed25519 bastion@5.6.7.8"
        );
    }

    #[test]
    fn session_without_key_leaves_identity_to_ssh() {
        let argv = as_strings(
            session("web-1", &Config::default(), &bastion())
                .unwrap()
                .into_argv(),
        );
        assert!(!argv.contains(&"-i".to_owned()));
        assert!(!argv.contains(&"IdentitiesOnly=yes".to_owned()));
    }

    #[test]
    fn bastion_tunnel_binds_the_target_through_the_bastion_connection() {
        let tunnel = Tunnel {
            local_port: 13306,
            target_host: "172.16.8.11",
            remote_port: 3306,
        };
        let command = bastion_tunnel(&tunnel, &Config::default(), &bastion()).unwrap();
        assert_eq!(command.destination(), "bastion@5.6.7.8");

        let argv = as_strings(command.into_argv());
        assert!(argv.contains(&"13306:172.16.8.11:3306".to_owned()));
        assert!(argv.contains(&"-N".to_owned()));
        assert!(argv.contains(&"ExitOnForwardFailure=yes".to_owned()));
        assert_eq!(argv.last().unwrap(), "bastion@5.6.7.8");
        assert!(!argv.iter().any(|arg| arg.starts_with("ProxyCommand=")));
    }

    #[test]
    fn server_tunnel_targets_the_server_loopback_through_the_jump() {
        let tunnel = Tunnel {
            local_port: 18090,
            target_host: "worker-1",
            remote_port: 8090,
        };
        let argv = as_strings(
            server_tunnel(&tunnel, &Config::default(), &bastion())
                .unwrap()
                .into_argv(),
        );

        assert!(argv.contains(&"18090:localhost:8090".to_owned()));
        assert_eq!(argv.last().unwrap(), "root@worker-1");
        assert!(argv.iter().any(|arg| arg.starts_with("ProxyCommand=")));
    }

    #[test]
    fn control_socket_options_stay_before_the_destination() {
        let tunnel = Tunnel {
            local_port: 13306,
            target_host: "172.16.8.11",
            remote_port: 3306,
        };
        let command = bastion_tunnel(&tunnel, &Config::default(), &bastion())
            .unwrap()
            .with_control_socket(Path::new("/tmp/t.sock"));

        let argv = as_strings(command.into_argv());
        assert_eq!(argv.last().unwrap(), "bastion@5.6.7.8");
        let socket_flag = argv.iter().position(|arg| arg == "-S").unwrap();
        assert_eq!(argv[socket_flag + 1], "/tmp/t.sock");
        assert!(argv.contains(&"-f".to_owned()));
        assert!(argv.contains(&"-M".to_owned()));
    }

    #[test]
    fn config_lines_render_the_same_hardening_options() {
        assert_eq!(
            option_config_lines(&HARDENING_OPTIONS),
            "    StrictHostKeyChecking no\n    UserKnownHostsFile /dev/null\n    LogLevel ERROR\n"
        );
    }
}
