//! Command-line surface: clap definitions and the cross-command scope.

use clap::{Parser, Subcommand};

use crate::cache::Freshness;
use crate::inventory::Environment;

// Shared with the zsh completion rewriter, which anchors its dynamic-name
// surgery on these exact strings.
pub(crate) const CONNECT_QUERY_HELP: &str =
    "Fuzzy query; connects directly when it matches a single server";
pub(crate) const DB_NAME_HELP: &str =
    "Database name; picks interactively when omitted or ambiguous";
pub(crate) const PF_QUERY_HELP: &str =
    "Fuzzy query; forwards directly when it matches a single resource";

const DB_AFTER_HELP: &str = "\
Scripting (no tty): an exact resource name skips the picker, so one-shot \
queries work without a terminal: scwx db <exact-name> -e 'SELECT 1'. Exact \
names and roles come from `scwx ls --json` (the master carries the master \
tag, 'Master' by default).";

#[derive(Debug, Parser)]
#[command(
    name = "scwx",
    version,
    about = "Fast SSH, database and port-forward access to Scaleway infrastructure"
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Command,

    /// Bypass the cache and fetch a fresh inventory
    #[arg(long, global = true)]
    pub(crate) refresh: bool,

    /// Filter resources by environment
    #[arg(long, global = true, value_enum)]
    pub(crate) env: Option<Environment>,
}

/// The two cross-command inputs every inventory-backed command needs.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Scope {
    pub(crate) env: Option<Environment>,
    pub(crate) freshness: Freshness,
}

impl Cli {
    pub(crate) fn scope(&self) -> Scope {
        Scope {
            env: self.env,
            freshness: if self.refresh {
                Freshness::Refresh
            } else {
                Freshness::CacheOk
            },
        }
    }
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
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
        #[arg(help = CONNECT_QUERY_HELP)]
        query: Option<String>,
    },
    /// Pick a database and open a mysql session through a tunnel
    #[command(after_help = DB_AFTER_HELP)]
    Db {
        #[arg(help = DB_NAME_HELP)]
        name: Option<String>,
        /// Run a single query and exit (mysql --execute)
        #[arg(short = 'e', long)]
        execute: Option<String>,
        /// Extra arguments passed to mysql, e.g. -- --table
        #[arg(last = true)]
        mysql_args: Vec<String>,
    },
    /// Manage port-forward tunnels
    Pf {
        #[command(subcommand)]
        command: Option<PfCommand>,
        #[arg(help = PF_QUERY_HELP)]
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
pub(crate) enum PfCommand {
    /// List active tunnels
    Ls,
    /// Stop a tunnel (all tunnels when no name is given)
    Stop { name: Option<String> },
}
