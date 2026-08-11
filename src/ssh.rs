use std::ffi::OsString;

use anyhow::Result;

use crate::config::Config;
use crate::exec::shell_join;
use crate::inventory::Bastion;
use crate::paths;

fn base_options(key: Option<&str>) -> Vec<String> {
    let mut options = vec![
        "-o".to_owned(),
        "StrictHostKeyChecking=no".to_owned(),
        "-o".to_owned(),
        "UserKnownHostsFile=/dev/null".to_owned(),
        "-o".to_owned(),
        "LogLevel=ERROR".to_owned(),
    ];
    if let Some(key) = key {
        options.push("-o".to_owned());
        options.push("IdentitiesOnly=yes".to_owned());
        options.push("-i".to_owned());
        options.push(key.to_owned());
    }
    options
}

fn identity_key(config: &Config) -> Result<Option<String>> {
    config
        .ssh
        .key
        .as_deref()
        .map(|key| Ok(paths::expand_tilde(key)?.to_string_lossy().into_owned()))
        .transpose()
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

fn through_bastion(config: &Config, bastion: &Bastion) -> Result<Vec<String>> {
    let key = identity_key(config)?;
    let mut argv = vec!["ssh".to_owned()];
    argv.extend(base_options(key.as_deref()));
    argv.push("-o".to_owned());
    argv.push(format!(
        "ProxyCommand={}",
        proxy_command(bastion, config, key.as_deref())
    ));
    Ok(argv)
}

/// Interactive session to a server, reached by name through the bastion.
pub fn session_argv(host: &str, config: &Config, bastion: &Bastion) -> Result<Vec<OsString>> {
    let mut argv = through_bastion(config, bastion)?;
    argv.push(format!("{}@{host}", config.ssh.user));
    Ok(argv.into_iter().map(OsString::from).collect())
}

pub struct Tunnel<'a> {
    pub local_port: u16,
    pub target_host: &'a str,
    pub remote_port: u16,
}

/// Forward through the bastion itself: -L local:target:remote on the
/// bastion connection, for targets the bastion can reach directly.
pub fn tunnel_argv(
    tunnel: &Tunnel<'_>,
    config: &Config,
    bastion: &Bastion,
) -> Result<Vec<OsString>> {
    let key = identity_key(config)?;
    let mut argv = vec!["ssh".to_owned()];
    argv.extend(base_options(key.as_deref()));
    argv.extend([
        "-o".to_owned(),
        "ServerAliveInterval=60".to_owned(),
        "-o".to_owned(),
        "ServerAliveCountMax=3".to_owned(),
        "-o".to_owned(),
        "ExitOnForwardFailure=yes".to_owned(),
        "-L".to_owned(),
        format!(
            "{}:{}:{}",
            tunnel.local_port, tunnel.target_host, tunnel.remote_port
        ),
        "-N".to_owned(),
        "-p".to_owned(),
        bastion.port.to_string(),
        bastion.destination(&config.bastion.user),
    ]);
    Ok(argv.into_iter().map(OsString::from).collect())
}

/// Forward to a server's own loopback, jumping through the bastion.
pub fn server_tunnel_argv(
    tunnel: &Tunnel<'_>,
    config: &Config,
    bastion: &Bastion,
) -> Result<Vec<OsString>> {
    let mut argv = through_bastion(config, bastion)?;
    argv.extend([
        "-o".to_owned(),
        "ServerAliveInterval=60".to_owned(),
        "-o".to_owned(),
        "ServerAliveCountMax=3".to_owned(),
        "-o".to_owned(),
        "ExitOnForwardFailure=yes".to_owned(),
        "-L".to_owned(),
        format!("{}:localhost:{}", tunnel.local_port, tunnel.remote_port),
        "-N".to_owned(),
        format!("{}@{}", config.ssh.user, tunnel.target_host),
    ]);
    Ok(argv.into_iter().map(OsString::from).collect())
}

/// Turns a foreground tunnel into a backgrounded control-master so it can
/// be checked and stopped later via ssh -S <socket> -O check/exit.
pub fn with_control_socket(mut argv: Vec<OsString>, socket: &std::path::Path) -> Vec<OsString> {
    let destination = argv.pop().expect("tunnel argv ends with a destination");
    argv.extend([
        OsString::from("-f"),
        OsString::from("-M"),
        OsString::from("-S"),
        socket.into(),
    ]);
    argv.push(destination);
    argv
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
        let argv = as_strings(session_argv("web-1", &config_with_key(), &bastion()).unwrap());

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
        let argv = as_strings(session_argv("web-1", &Config::default(), &bastion()).unwrap());
        assert!(!argv.contains(&"-i".to_owned()));
        assert!(!argv.contains(&"IdentitiesOnly=yes".to_owned()));
    }

    #[test]
    fn tunnel_binds_the_target_through_the_bastion_connection() {
        let tunnel = Tunnel {
            local_port: 13306,
            target_host: "172.16.8.11",
            remote_port: 3306,
        };
        let argv = as_strings(tunnel_argv(&tunnel, &Config::default(), &bastion()).unwrap());

        assert!(argv.contains(&"13306:172.16.8.11:3306".to_owned()));
        assert!(argv.contains(&"-N".to_owned()));
        assert!(argv.contains(&"ExitOnForwardFailure=yes".to_owned()));
        assert_eq!(argv.last().unwrap(), "bastion@5.6.7.8");
        assert!(!argv.iter().any(|arg| arg.starts_with("ProxyCommand=")));
    }

    #[test]
    fn control_socket_options_are_inserted_before_the_destination() {
        let tunnel = Tunnel {
            local_port: 13306,
            target_host: "172.16.8.11",
            remote_port: 3306,
        };
        let argv = tunnel_argv(&tunnel, &Config::default(), &bastion()).unwrap();
        let argv = as_strings(with_control_socket(
            argv,
            std::path::Path::new("/tmp/t.sock"),
        ));

        assert_eq!(argv.last().unwrap(), "bastion@5.6.7.8");
        let socket_flag = argv.iter().position(|arg| arg == "-S").unwrap();
        assert_eq!(argv[socket_flag + 1], "/tmp/t.sock");
        assert!(argv.contains(&"-f".to_owned()));
        assert!(argv.contains(&"-M".to_owned()));
    }

    #[test]
    fn server_tunnel_targets_the_server_loopback_through_the_jump() {
        let tunnel = Tunnel {
            local_port: 18090,
            target_host: "worker-1",
            remote_port: 8090,
        };
        let argv = as_strings(server_tunnel_argv(&tunnel, &Config::default(), &bastion()).unwrap());

        assert!(argv.contains(&"18090:localhost:8090".to_owned()));
        assert_eq!(argv.last().unwrap(), "root@worker-1");
        assert!(argv.iter().any(|arg| arg.starts_with("ProxyCommand=")));
    }
}
