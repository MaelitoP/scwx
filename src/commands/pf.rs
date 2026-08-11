use anyhow::{Result, bail};

use crate::cli::{Cli, PfCommand};

pub fn run(
    _cli: &Cli,
    _command: Option<&PfCommand>,
    _query: Option<&str>,
    _local_port: Option<u16>,
    _remote_port: Option<u16>,
) -> Result<()> {
    bail!("not implemented yet")
}
