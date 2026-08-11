use clap::{Parser, Subcommand};

use crate::inventory::Environment;

#[derive(Debug, Parser)]
#[command(
    name = "scwx",
    version,
    about = "Fast SSH, database and port-forward access to Scaleway infrastructure"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    /// Bypass the cache and fetch a fresh inventory
    #[arg(long, global = true)]
    pub refresh: bool,

    /// Filter resources by environment
    #[arg(long, global = true, value_enum)]
    pub env: Option<Environment>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// List the resource inventory
    Ls {
        /// Output as JSON
        #[arg(long)]
        json: bool,
        /// Output connectable server names only, one per line
        #[arg(long)]
        names: bool,
        /// Output database names only, one per line
        #[arg(long, hide = true, conflicts_with = "names")]
        db_names: bool,
        /// Output port-forwardable resource names only, one per line
        #[arg(long, hide = true, conflicts_with_all = ["names", "db_names"])]
        pf_names: bool,
        /// Read only the cache; output nothing when it is missing
        #[arg(long, hide = true)]
        cached: bool,
    },
    /// Pick a server and open an SSH session through the bastion
    Connect {
        /// Fuzzy query; connects directly when it matches a single server
        query: Option<String>,
    },
    /// Pick a database and open a mysql session through a tunnel
    Db {
        /// Database name; picks interactively when omitted or ambiguous
        name: Option<String>,
    },
    /// Manage port-forward tunnels
    Pf {
        #[command(subcommand)]
        command: Option<PfCommand>,
        /// Fuzzy query; forwards directly when it matches a single resource
        query: Option<String>,
        /// Local port to bind (defaults to the remote port)
        #[arg(long)]
        local_port: Option<u16>,
        /// Remote port to forward (defaults to the resource's PortForward tag)
        #[arg(long)]
        remote_port: Option<u16>,
    },
    /// Write SSH host entries for all servers to ~/.ssh/config.d/scaleway
    SyncSsh,
    /// Generate shell completions
    Completions {
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
    /// Update scwx to the latest release
    Update,
}

#[derive(Debug, Subcommand)]
pub enum PfCommand {
    /// List active tunnels
    Ls,
    /// Stop a tunnel (all tunnels when no name is given)
    Stop { name: Option<String> },
}
