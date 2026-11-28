//! Issue #3423: layout-tolerant comparison of the Codex config with a
//! recorded state, and a refusal detail that says what differs.
//!
//! Native Codex rewrites `config.toml` itself, and its layout (blank lines,
//! spacing, table order) need not match the `toml_edit` prediction. A config
//! matches a recorded state by content when both parse as TOML, the parsed
//! values are equal, and the lines holding `#` are the same. Any change to a
//! value, key, table or comment still differs. Values are never printed,
//! because a Codex config can hold credentials.
use super::*;

const LISTED: usize = 10;

fn bytes(value: &Value) -> Option<Vec<u8>> {
    value
        .as_array()?
        .iter()
        .map(|b| b.as_u64().and_then(|n| u8::try_from(n).ok()))
        .collect()
}

fn text(value: &Value) -> Option<String> {
    String::from_utf8(bytes(value)?).ok()
}

fn parsed(text: &str) -> Option<toml::Table> {
    text.parse::<toml::Table>().ok()
}

/// Trimmed, non-blank lines that contain `#`: comments, inline comments and
/// strings holding `#`. Sorted, so table order does not matter.
fn comment_lines(text: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && line.contains('#'))
        .collect();
    lines.sort_unstable();
    lines
}

/// True when `current` holds the same TOML content as `candidate`. An absent
/// file (`null`) never matches here; absence is compared by bytes only.
pub(super) fn same_content(candidate: &Value, current: &Value) -> bool {
    let (Some(recorded), Some(current)) = (text(candidate), text(current)) else {
        return false;
    };
    match (parsed(&recorded), parsed(&current)) {
        (Some(a), Some(b)) => a == b && comment_lines(&recorded) == comment_lines(&current),
        _ => false,
    }
}

fn key(name: &str) -> String {
    if !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        name.to_owned()
    } else {
        format!("{name:?}")
    }
}

/// Dotted table and key paths that differ, as "added", "removed" or "changed".
fn key_changes(recorded: &toml::Table, current: &toml::Table, prefix: &str, out: &mut Vec<String>) {
    let names: std::collections::BTreeSet<&String> =
        recorded.keys().chain(current.keys()).collect();
    for name in names {
        let path = format!("{prefix}{}", key(name));
        match (recorded.get(name), current.get(name)) {
            (Some(_), None) => out.push(format!("removed {path}")),
            (None, Some(_)) => out.push(format!("added {path}")),
            (Some(toml::Value::Table(a)), Some(toml::Value::Table(b))) => {
                key_changes(a, b, &format!("{path}."), out)
            }
            (Some(a), Some(b)) if a != b => out.push(format!("changed {path}")),
            _ => {}
        }
    }
}

/// What a line holds, without its value: a table header, a key name, a
/// comment or a blank line.
fn line_kind(line: Option<&str>) -> String {
    let Some(line) = line.map(str::trim) else {
        return "end of file".into();
    };
    if line.is_empty() {
        "a blank line".into()
    } else if line.starts_with('#') {
        "a comment".into()
    } else if line.starts_with('[') {
        let end = line.find(']').map_or(line.len(), |i| {
            if line[i + 1..].starts_with(']') {
                i + 2
            } else {
                i + 1
            }
        });
        format!("table {}", &line[..end])
    } else if let Some((name, _)) = line.split_once('=') {
        format!("key {}", name.trim())
    } else {
        "a continued value".into()
    }
}

fn common_prefix(a: &str, b: &str) -> usize {
    a.lines().zip(b.lines()).take_while(|(x, y)| x == y).count()
}

fn first_line(recorded: &str, current: &str) -> String {
    let n = common_prefix(recorded, current);
    format!(
        "first difference at line {}: the config has {}, the recorded state has {}",
        n + 1,
        line_kind(current.lines().nth(n)),
        line_kind(recorded.lines().nth(n))
    )
}

/// Names the difference between `current` and the closest recorded state.
pub(super) fn describe(pending: &Value, current: &Value) -> String {
    let candidates: Vec<Option<String>> = config::candidates(pending).map(text).collect();
    let Some(current) = text(current) else {
        return if current.is_null() {
            "config.toml is absent".into()
        } else {
            "config.toml is not UTF-8 text".into()
        };
    };
    let recorded: Vec<&str> = candidates.iter().flatten().map(String::as_str).collect();
    if recorded.is_empty() {
        return "config.toml is present, and the recorded config was absent".into();
    }
    if let Some(now) = parsed(&current) {
        let closest = recorded
            .iter()
            .filter_map(|text| {
                let mut changes = Vec::new();
                key_changes(&parsed(text)?, &now, "", &mut changes);
                Some((changes, *text))
            })
            .min_by_key(|(changes, _)| changes.len());
        if let Some((changes, text)) = closest {
            if changes.is_empty() {
                return format!("comments differ; {}", first_line(text, &current));
            }
            let more = changes.len().saturating_sub(LISTED);
            let mut listed = changes
                .into_iter()
                .take(LISTED)
                .collect::<Vec<_>>()
                .join(", ");
            if more > 0 {
                listed.push_str(&format!(", and {more} more"));
            }
            return format!("tables or keys differ: {listed}");
        }
    }
    let text = recorded
        .iter()
        .copied()
        .rev()
        .max_by_key(|text| common_prefix(text, &current))
        .unwrap_or_default();
    let reason = if parsed(&current).is_some() {
        ""
    } else {
        "config.toml does not parse as TOML; "
    };
    format!("{reason}{}", first_line(text, &current))
}
