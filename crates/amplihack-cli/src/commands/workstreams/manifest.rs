//! The optional workstreams manifest (issue #1710): one tab-separated
//! `label<TAB>issue<TAB>pr` line per workstream, issue and PR as `123`,
//! `#123` or `-`. It labels discovered runs and adds queued items that are
//! not running yet.

use super::discover::Run;
use crate::commands::profile::usage_cmd::sanitize_display;

/// Bad values are quoted in warnings up to this many characters.
const MAX_QUOTED: usize = 40;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManifestRow {
    pub label: String,
    pub issue: Option<u64>,
    pub pr: Option<u64>,
}

/// The rows of `text`, and one `manifest:<line>: …` warning per line skipped.
pub(crate) fn parse(text: &str) -> (Vec<ManifestRow>, Vec<String>) {
    let mut rows = Vec::new();
    let mut warnings = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let n = i + 1;
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() < 3 {
            warnings.push(format!(
                "manifest:{n}: expected 3 tab-separated fields (label, issue, pr); line skipped"
            ));
            continue;
        }
        let label = sanitize_display(fields[0], false).trim().to_string();
        if label.is_empty() {
            warnings.push(format!("manifest:{n}: the label is empty; line skipped"));
            continue;
        }
        let issue = match number(fields[1]) {
            Ok(v) => v,
            Err(()) => {
                warnings.push(format!(
                    "manifest:{n}: issue {} is not 123, #123 or -; line skipped",
                    quote(fields[1])
                ));
                continue;
            }
        };
        let pr = match number(fields[2]) {
            Ok(v) => v,
            Err(()) => {
                warnings.push(format!(
                    "manifest:{n}: pr {} is not 123, #123 or -; line skipped",
                    quote(fields[2])
                ));
                continue;
            }
        };
        rows.push(ManifestRow { label, issue, pr });
    }
    (rows, warnings)
}

/// `123`, `#123` → `Some(123)`; `-` → `None`.
fn number(field: &str) -> Result<Option<u64>, ()> {
    let f = field.trim();
    if f == "-" {
        return Ok(None);
    }
    f.strip_prefix('#')
        .unwrap_or(f)
        .parse::<u64>()
        .map(Some)
        .map_err(|_| ())
}

fn quote(value: &str) -> String {
    let clean = sanitize_display(value, false);
    let mut out: String = clean.chars().take(MAX_QUOTED).collect();
    if clean.chars().count() > MAX_QUOTED {
        out.push('…');
    }
    format!("\"{out}\"")
}

/// One row of the table: a manifest row, a discovered run, or both.
#[derive(Debug, Clone)]
pub(crate) struct Entry {
    pub label: String,
    pub issue: Option<u64>,
    pub pr: Option<u64>,
    pub run: Option<Run>,
}

/// Manifest rows in file order, each with the run it matches (by PR first,
/// across every row, then by issue; each run used once), then the runs no row
/// matched, oldest first, unknown start last.
pub(crate) fn merge(manifest: Vec<ManifestRow>, runs: Vec<Run>) -> Vec<Entry> {
    let mut used = vec![false; runs.len()];
    let mut claimed: Vec<Option<usize>> = vec![None; manifest.len()];
    for (row, slot) in manifest.iter().zip(claimed.iter_mut()) {
        let Some(pr) = row.pr else { continue };
        if let Some(i) = (0..runs.len()).find(|&i| !used[i] && runs[i].pr == Some(pr)) {
            used[i] = true;
            *slot = Some(i);
        }
    }
    for (row, slot) in manifest.iter().zip(claimed.iter_mut()) {
        if slot.is_some() {
            continue;
        }
        let Some(issue) = row.issue else { continue };
        if let Some(i) = (0..runs.len()).find(|&i| !used[i] && runs[i].issue == Some(issue)) {
            used[i] = true;
            *slot = Some(i);
        }
    }
    let mut taken: Vec<Option<Run>> = runs.into_iter().map(Some).collect();
    let mut out: Vec<Entry> = manifest
        .into_iter()
        .zip(claimed)
        .map(|(row, slot)| Entry {
            label: row.label,
            issue: row.issue,
            pr: row.pr,
            run: slot.and_then(|i| taken[i].take()),
        })
        .collect();
    let mut rest: Vec<Run> = taken.into_iter().flatten().collect();
    rest.sort_by(|a, b| match (a.started, b.started) {
        (Some(x), Some(y)) => x.cmp(&y).then_with(|| a.session.cmp(&b.session)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.session.cmp(&b.session),
    });
    out.extend(rest.into_iter().map(|run| Entry {
        label: match (&run.recipe, run.issue) {
            (Some(recipe), Some(issue)) => format!("{recipe} #{issue}"),
            _ => run.session.clone(),
        },
        issue: run.issue,
        pr: run.pr,
        run: Some(run),
    }));
    out
}
