use std::io::{self, Write};

use anyhow::{Context, Result};
use clap::CommandFactory;
use clap_complete::Shell;

use crate::cli::Cli;

pub fn run(shell: Shell) -> Result<()> {
    let mut command = Cli::command();
    if shell != Shell::Zsh {
        clap_complete::generate(shell, &mut command, "scwx", &mut io::stdout());
        return Ok(());
    }

    let mut generated = Vec::new();
    clap_complete::generate(shell, &mut command, "scwx", &mut generated);
    let script = String::from_utf8(generated).context("generated completions are not utf-8")?;
    io::stdout()
        .write_all(with_dynamic_names(&script).as_bytes())
        .context("writing completions")
}

/// Swaps the static `_default` completers of name/query positionals for
/// helpers that read the scwx cache, so values tab-complete.
fn with_dynamic_names(script: &str) -> String {
    let script = script
        .replace(
            "matches a single server:_default",
            "matches a single server:_scwx_server_names",
        )
        .replace(
            "picks interactively when omitted or ambiguous:_default",
            "picks interactively when omitted or ambiguous:_scwx_db_names",
        )
        .replace(
            "matches a single resource:_default",
            "matches a single resource:_scwx_pf_names",
        )
        .replace("'::name:_default'", "'::name:_scwx_tunnel_names'");

    let helpers = r#"
_scwx_server_names() {
    compadd -- ${(f)"$(scwx ls --names --cached 2>/dev/null)"}
}
_scwx_db_names() {
    compadd -- ${(f)"$(scwx ls --db-names --cached 2>/dev/null)"}
}
_scwx_pf_names() {
    compadd -- ${(f)"$(scwx ls --pf-names --cached 2>/dev/null)"}
}
_scwx_tunnel_names() {
    compadd -- ${(f)"$(scwx pf ls 2>/dev/null | cut -d' ' -f1)"}
}

"#;

    match script.find("if [ \"$funcstack[1]\" = \"_scwx\" ]") {
        Some(position) => format!("{}{helpers}{}", &script[..position], &script[position..]),
        None => script,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zsh_script_wires_dynamic_helpers() {
        let mut command = Cli::command();
        let mut generated = Vec::new();
        clap_complete::generate(Shell::Zsh, &mut command, "scwx", &mut generated);
        let script = with_dynamic_names(&String::from_utf8(generated).unwrap());

        assert!(script.contains("matches a single server:_scwx_server_names"));
        assert!(script.contains(":_scwx_db_names"));
        assert!(script.contains(":_scwx_pf_names"));
        assert!(script.contains("'::name:_scwx_tunnel_names'"));
        assert!(!script.contains("'::name:_default'"));
        assert!(script.contains("_scwx_server_names() {"));

        let helpers = script.find("_scwx_server_names() {").unwrap();
        let dispatch = script.find("if [ \"$funcstack[1]\" = \"_scwx\" ]").unwrap();
        assert!(helpers < dispatch);
    }
}
