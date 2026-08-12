//! Quoting for strings that a shell will re-parse (tmux commands,
//! ProxyCommand values).

pub(crate) fn shell_join(argv: &[String]) -> String {
    argv.iter()
        .map(|arg| shell_quote(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_quote(arg: &str) -> String {
    let safe = arg
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "@%+=:,./-_".contains(c));
    if safe && !arg.is_empty() {
        return arg.to_owned();
    }
    format!("'{}'", arg.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_arguments_stay_bare() {
        let args = ["ssh".to_owned(), "-W".to_owned(), "%h:%p".to_owned()];
        assert_eq!(shell_join(&args), "ssh -W %h:%p");
    }

    #[test]
    fn arguments_with_spaces_and_quotes_are_quoted() {
        let args = ["a b".to_owned(), "it's".to_owned(), String::new()];
        assert_eq!(shell_join(&args), r#"'a b' 'it'\''s' ''"#);
    }
}
