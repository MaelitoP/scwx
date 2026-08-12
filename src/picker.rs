use std::io::Write;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

use crate::config::{Config, NamingSection};
use crate::inventory::Resource;
use crate::table;

pub fn render_resources(resources: &[&Resource], config: &Config) -> Vec<String> {
    let rows: Vec<[String; 4]> = resources
        .iter()
        .map(|resource| {
            [
                resource.display_name(&config.naming).to_owned(),
                resource.kind.to_string(),
                resource
                    .env(&config.tags)
                    .map(|env| env.to_string())
                    .unwrap_or_default(),
                resource.zone.clone(),
            ]
        })
        .collect();
    table::columns(&rows)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickOutcome {
    Window,
    Split,
    VSplit,
    Inline,
}

impl PickOutcome {
    /// Where the session should land in tmux; None means the current
    /// terminal.
    pub fn placement(self) -> Option<crate::tmux::Placement> {
        match self {
            Self::Window => Some(crate::tmux::Placement::Window),
            Self::Split => Some(crate::tmux::Placement::Below),
            Self::VSplit => Some(crate::tmux::Placement::Beside),
            Self::Inline => None,
        }
    }
}

#[derive(Debug)]
pub struct Pick {
    pub index: usize,
    pub outcome: PickOutcome,
}

const KEY_LEGEND: &str = "enter=window  ctrl-s=split  ctrl-v=vsplit  ctrl-o=here";

#[derive(Debug)]
pub enum Selection<'a> {
    /// The query identified exactly one resource without a picker.
    Direct(&'a Resource),
    Picked(&'a Resource, PickOutcome),
    /// The query matched nothing; the caller owns the error message.
    NoMatch,
    Cancelled,
}

/// The shared resolve-a-query-or-pick flow: an exact name match wins, a
/// unique substring match connects directly, anything else opens the picker
/// preseeded with the query. `lines` must align with `candidates`.
pub fn select<'a>(
    candidates: &[&'a Resource],
    lines: &[String],
    header: &str,
    query: Option<&str>,
    naming: &NamingSection,
    with_placement_keys: bool,
) -> Result<Selection<'a>> {
    if let Some(query) = query {
        let exact: Vec<&&Resource> = candidates
            .iter()
            .filter(|resource| resource.name == query)
            .collect();
        if let [resource] = exact.as_slice() {
            return Ok(Selection::Direct(resource));
        }

        let matched: Vec<&&Resource> = candidates
            .iter()
            .filter(|resource| resource.matches(query, naming))
            .collect();
        match matched.as_slice() {
            [] => return Ok(Selection::NoMatch),
            [resource] => return Ok(Selection::Direct(resource)),
            _ => {}
        }
    }

    let pick = if with_placement_keys {
        pick(lines, header, query)?
    } else {
        pick_without_placement(lines, header, query)?.map(|index| Pick {
            index,
            outcome: PickOutcome::Inline,
        })
    };
    let Some(pick) = pick else {
        return Ok(Selection::Cancelled);
    };
    let resource = candidates
        .get(pick.index)
        .context("fzf returned an out-of-range selection")?;
    Ok(Selection::Picked(resource, pick.outcome))
}

/// Runs fzf over pre-rendered lines; returns the picked line index and the
/// key-selected outcome, or None when the picker is cancelled.
pub fn pick(lines: &[String], header: &str, initial_query: Option<&str>) -> Result<Option<Pick>> {
    let Some(stdout) = run_fzf(lines, header, initial_query, true)? else {
        return Ok(None);
    };
    parse_output(&stdout).map(Some)
}

/// Picker without placement keys; returns the picked line index.
pub fn pick_without_placement(
    lines: &[String],
    header: &str,
    initial_query: Option<&str>,
) -> Result<Option<usize>> {
    let Some(stdout) = run_fzf(lines, header, initial_query, false)? else {
        return Ok(None);
    };
    let index = stdout
        .lines()
        .next()
        .context("fzf returned no selection")?
        .split('\t')
        .next()
        .context("fzf selection has no index")?
        .parse()
        .context("fzf selection index is not a number")?;
    Ok(Some(index))
}

fn run_fzf(
    lines: &[String],
    header: &str,
    initial_query: Option<&str>,
    with_placement_keys: bool,
) -> Result<Option<String>> {
    let mut command = Command::new("fzf");
    command
        .arg("--delimiter=\t")
        .arg("--with-nth=2..")
        .arg("--no-multi")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    if with_placement_keys {
        command
            .arg("--expect=ctrl-s,ctrl-v,ctrl-o")
            .arg(format!("--header={header}\n{KEY_LEGEND}"));
    } else {
        command.arg(format!("--header={header}"));
    }
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
    Ok(Some(stdout))
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
    use crate::inventory::ResourceKind;

    fn resource(name: &str) -> Resource {
        Resource {
            kind: ResourceKind::Instance,
            id: "id".to_owned(),
            name: name.to_owned(),
            zone: "fr-par-1".to_owned(),
            tags: vec![],
            endpoint_ip: None,
            endpoint_port: None,
        }
    }

    #[test]
    fn select_resolves_a_unique_substring_match_directly() {
        let naming = NamingSection::default();
        let a = resource("platform-perco-1");
        let b = resource("platform-api-1");
        let candidates = vec![&a, &b];

        let selection = select(&candidates, &[], "h", Some("perco"), &naming, true).unwrap();
        assert!(matches!(selection, Selection::Direct(r) if r.name == "platform-perco-1"));
    }

    #[test]
    fn select_prefers_an_exact_name_over_substring_ambiguity() {
        let naming = NamingSection::default();
        let a = resource("api");
        let b = resource("api-1");
        let candidates = vec![&a, &b];

        let selection = select(&candidates, &[], "h", Some("api"), &naming, true).unwrap();
        assert!(matches!(selection, Selection::Direct(r) if r.name == "api"));
    }

    #[test]
    fn select_reports_no_match_for_an_unknown_query() {
        let naming = NamingSection::default();
        let a = resource("api-1");
        let candidates = vec![&a];

        let selection = select(&candidates, &[], "h", Some("redis"), &naming, true).unwrap();
        assert!(matches!(selection, Selection::NoMatch));
    }

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
