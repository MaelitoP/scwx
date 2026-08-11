use std::io::Write;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

use crate::config::Config;
use crate::inventory::Resource;

pub fn render_resources(resources: &[&Resource], config: &Config) -> Vec<String> {
    let rows: Vec<[String; 4]> = resources
        .iter()
        .map(|resource| {
            [
                resource.display_name(&config.naming),
                resource.kind.label().to_owned(),
                resource
                    .env(&config.tags)
                    .map(|env| env.to_string())
                    .unwrap_or_default(),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickOutcome {
    Window,
    Split,
    VSplit,
    Inline,
}

#[derive(Debug)]
pub struct Pick {
    pub index: usize,
    pub outcome: PickOutcome,
}

const KEY_LEGEND: &str = "enter=window  ctrl-s=split  ctrl-v=vsplit  ctrl-o=here";

/// Runs fzf over pre-rendered lines; returns the picked line index and the
/// key-selected outcome, or None when the picker is cancelled.
pub fn pick(lines: &[String], header: &str, initial_query: Option<&str>) -> Result<Option<Pick>> {
    let mut command = Command::new("fzf");
    command
        .arg("--delimiter=\t")
        .arg("--with-nth=2..")
        .arg("--expect=ctrl-s,ctrl-v,ctrl-o")
        .arg(format!("--header={header}\n{KEY_LEGEND}"))
        .arg("--no-multi")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    if let Some(query) = initial_query {
        command.arg(format!("--query={query}"));
    }

    let mut child = command.spawn().context("running fzf (is it installed?)")?;

    let mut stdin = child.stdin.take().context("opening fzf stdin")?;
    let input: String = lines
        .iter()
        .enumerate()
        .map(|(index, line)| format!("{index}\t{line}\n"))
        .collect();
    // fzf may exit before consuming all input; a broken pipe is not an error.
    let _ = stdin.write_all(input.as_bytes());
    drop(stdin);

    let output = child.wait_with_output().context("waiting for fzf")?;
    match output.status.code() {
        Some(0) => {}
        Some(1) | Some(130) => return Ok(None),
        _ => bail!("fzf failed with {}", output.status),
    }

    let stdout = String::from_utf8(output.stdout).context("fzf output is not utf-8")?;
    parse_output(&stdout).map(Some)
}

fn parse_output(stdout: &str) -> Result<Pick> {
    let mut lines = stdout.lines();
    let key = lines.next().context("fzf returned no output")?;
    let selection = lines.next().context("fzf returned no selection")?;

    let outcome = match key {
        "" => PickOutcome::Window,
        "ctrl-s" => PickOutcome::Split,
        "ctrl-v" => PickOutcome::VSplit,
        "ctrl-o" => PickOutcome::Inline,
        other => bail!("unexpected fzf key: {other}"),
    };
    let index = selection
        .split('\t')
        .next()
        .context("fzf selection has no index")?
        .parse()
        .context("fzf selection index is not a number")?;

    Ok(Pick { index, outcome })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enter_maps_to_window_and_keys_to_splits() {
        let pick = parse_output("\n3\tprod-perco-3\n").unwrap();
        assert_eq!(pick.index, 3);
        assert_eq!(pick.outcome, PickOutcome::Window);

        let pick = parse_output("ctrl-s\n0\ta\n").unwrap();
        assert_eq!(pick.outcome, PickOutcome::Split);

        let pick = parse_output("ctrl-v\n12\ta\n").unwrap();
        assert_eq!(pick.outcome, PickOutcome::VSplit);

        let pick = parse_output("ctrl-o\n7\ta\n").unwrap();
        assert_eq!(pick.outcome, PickOutcome::Inline);
        assert_eq!(pick.index, 7);
    }

    #[test]
    fn empty_output_is_an_error() {
        assert!(parse_output("").is_err());
        assert!(parse_output("\n").is_err());
    }
}
