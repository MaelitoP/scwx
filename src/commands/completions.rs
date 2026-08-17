use std::io::{self, Write};

use anyhow::{Context, Result};
use clap::CommandFactory;
use clap_complete::Shell;

use crate::cli::Cli;

pub(crate) fn run(shell: Shell) -> Result<()> {
    let mut command = Cli::command();
    let rewrite: fn(&str) -> String = match shell {
        Shell::Zsh => with_dynamic_names,
        Shell::Fish => with_fish_dynamic_names,
        _ => {
            clap_complete::generate(shell, &mut command, "scwx", &mut io::stdout());
            return Ok(());
        }
    };

    let mut generated = Vec::new();
    clap_complete::generate(shell, &mut command, "scwx", &mut generated);
    let script = String::from_utf8(generated).context("generated completions are not utf-8")?;
    io::stdout()
        .write_all(rewrite(&script).as_bytes())
        .context("writing completions")
}

/// Appends value completions reading the scwx cache; fish evaluates the
/// command substitutions at completion time.
fn with_fish_dynamic_names(script: &str) -> String {
    let dynamic = r#"
complete -c scwx -n "__fish_scwx_using_subcommand connect" -f -a "(scwx ls --names --cached 2>/dev/null)"
complete -c scwx -n "__fish_scwx_using_subcommand db" -f -a "(scwx ls --db-names --cached 2>/dev/null)"
complete -c scwx -n "__fish_scwx_using_subcommand pf; and not __fish_seen_subcommand_from ls stop help" -f -a "(scwx ls --pf-names --cached 2>/dev/null)"
complete -c scwx -n "__fish_scwx_using_subcommand pf; and __fish_seen_subcommand_from stop" -f -a "(scwx pf ls 2>/dev/null | cut -d' ' -f1)"
"#;
    format!("{script}{dynamic}")
}

/// Swaps the static `_default` completers of name/query positionals for
/// helpers that read the scwx cache, so values tab-complete.
fn with_dynamic_names(script: &str) -> String {
    let script = script
        .replace(
            &format!("{}:_default", crate::cli::CONNECT_QUERY_HELP),
            &format!("{}:_scwx_server_names", crate::cli::CONNECT_QUERY_HELP),
        )
        .replace(
            &format!("{}:_default", crate::cli::DB_NAME_HELP),
            &format!("{}:_scwx_db_names", crate::cli::DB_NAME_HELP),
        )
        .replace(
            &format!("{}:_default", crate::cli::PF_QUERY_HELP),
            &format!("{}:_scwx_pf_names", crate::cli::PF_QUERY_HELP),
        )
        .replace("'::name:_default'", "'::name:_scwx_tunnel_names'")
        // The db name positional is optional, so _arguments also offers the
        // mysql_args spec for the first word; _default there mixes files
        // into the db names. A blank action completes nothing.
        .replace(
            &format!("{}:_default", crate::cli::MYSQL_ARGS_HELP),
            &format!("{}: ", crate::cli::MYSQL_ARGS_HELP),
        );

    // pf declares an optional query positional before its subcommands, so
    // zsh assigns the typed word to the query and never descends into
    // `stop`/`ls`. Collapse both into position one (matching how clap
    // parses: a subcommand name wins over the positional) and route the
    // state machine off line[1].
    let script = script
        .replace(
            &format!(
                "'::query -- {}:_scwx_pf_names' \\\n\":: :_scwx__subcmd__pf_commands\" \\\n",
                crate::cli::PF_QUERY_HELP
            ),
            "\":: :_scwx_pf_targets_or_commands\" \\\n",
        )
        .replace(
            "        words=($line[2] \"${words[@]}\")\n        (( CURRENT += 1 ))\n        curcontext=\"${curcontext%:*:*}:scwx-pf-command-$line[2]:\"\n        case $line[2] in",
            "        words=($line[1] \"${words[@]}\")\n        (( CURRENT += 1 ))\n        curcontext=\"${curcontext%:*:*}:scwx-pf-command-$line[1]:\"\n        case $line[1] in",
        );

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
_scwx_pf_targets_or_commands() {
    _scwx_pf_names
    _scwx__subcmd__pf_commands
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
        assert!(script.contains("'::name:_scwx_tunnel_names'"));
        assert!(!script.contains("'::name:_default'"));
        assert!(script.contains("_scwx_server_names() {"));
        assert!(script.contains(&format!("{}: ", crate::cli::MYSQL_ARGS_HELP)));
        assert!(!script.contains(&format!("{}:_default", crate::cli::MYSQL_ARGS_HELP)));

        // pf: the query positional is merged into the subcommand position
        // and the state machine reads line[1], so `pf stop <TAB>` descends.
        assert!(script.contains(":: :_scwx_pf_targets_or_commands"));
        assert!(!script.contains(&format!("::query -- {}", crate::cli::PF_QUERY_HELP)));
        assert!(script.contains("scwx-pf-command-$line[1]"));
        assert!(!script.contains("scwx-pf-command-$line[2]"));
        assert!(script.contains("_scwx_pf_targets_or_commands() {"));

        let helpers = script.find("_scwx_server_names() {").unwrap();
        let dispatch = script.find("if [ \"$funcstack[1]\" = \"_scwx\" ]").unwrap();
        assert!(helpers < dispatch);
    }

    #[test]
    fn fish_script_appends_dynamic_value_completions() {
        let mut command = Cli::command();
        let mut generated = Vec::new();
        clap_complete::generate(Shell::Fish, &mut command, "scwx", &mut generated);
        let script = with_fish_dynamic_names(&String::from_utf8(generated).unwrap());

        // The conditions must reference the helper clap actually generated.
        assert!(script.contains("function __fish_scwx_using_subcommand"));
        assert!(script.contains(r#"subcommand connect" -f -a "(scwx ls --names --cached"#));
        assert!(script.contains("(scwx ls --db-names --cached"));
        assert!(script.contains("(scwx ls --pf-names --cached"));
        assert!(script.contains("(scwx pf ls 2>/dev/null | cut -d' ' -f1)"));
    }
}
