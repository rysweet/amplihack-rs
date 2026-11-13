//! Compose a readiness report without reconstructing the published description.
use anyhow::{Context, Result, bail};
use regex::Regex;
use serde::Deserialize;
use std::io::{self, Write};

const MARKER: &str = "<!-- merge-ready-report: v1 -->";
const TEMPLATE_MARKER: &str = "<!-- merge-ready-report: everything from this line down is the pre-merge readiness report and is left out of the squash commit -->";

/// Find one established report boundary, permitting its following legacy heading.
fn report_boundary(text: &str) -> Result<Option<usize>> {
    let mut boundary = None;
    let mut marker_seen = false;
    let mut heading_seen = false;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let line_text = line.trim_end_matches(['\r', '\n']);
        if line_text == MARKER || line_text == TEMPLATE_MARKER {
            if boundary.is_some() {
                bail!("conflicting readiness report boundaries");
            }
            boundary = Some(offset);
            marker_seen = true;
        } else if line_text.eq_ignore_ascii_case("## Merge readiness") {
            if heading_seen || (boundary.is_some() && !marker_seen) {
                bail!("conflicting readiness report headings");
            }
            boundary.get_or_insert(offset);
            heading_seen = true;
        } else if line_text.starts_with("<!-- merge-ready-report") {
            bail!("unsupported readiness report marker");
        }
        offset += line.len();
    }
    Ok(boundary)
}

/// Preserve the published prefix verbatim and replace or append one safe report.
///
/// Returns an error for ambiguous boundaries or closing references in the report.
/// Existing published references are data and are never reclassified.
pub fn compose_readiness_description(description: &str, report: &str) -> Result<String> {
    let boundary = report_boundary(description)?;
    if report_boundary(report)? != Some(0) {
        bail!("readiness report must start with the established marker or heading");
    }
    // Resolve Markdown labels for validation only. An issue link's destination
    // is the reference even when its label says just "issue"; a linked keyword
    // is still a keyword. Keep the original report bytes for output.
    let links = Regex::new(r"\[([^\]]*)\]\(([^\s)]+)(?:\s+[^)]*)?\)")?;
    let issue_url = Regex::new(r"(?i)^https?://[^\s]+/issues/[0-9]+")?;
    let unlinked = links.replace_all(report, |captures: &regex::Captures<'_>| {
        if issue_url.is_match(&captures[2]) {
            captures[2].to_owned()
        } else {
            captures[1].to_owned()
        }
    });
    let plain: String = unlinked
        .chars()
        .filter(|c| {
            !matches!(
                c,
                '*' | '_' | '`' | '~' | '\\' | '[' | ']' | '(' | ')' | '<' | '>'
            )
        })
        .collect();
    let closing = Regex::new(
        r"(?i)\b(?:close[sd]?|fix(?:es|ed)?|resolve[sd]?)\b\s*:?\s*(?:[a-z0-9.-]+/[a-z0-9.-]+)?#[0-9]+|\b(?:close[sd]?|fix(?:es|ed)?|resolve[sd]?)\b\s*:?\s*https?://[^\s]+/issues/[0-9]+",
    )?;
    if closing.is_match(&plain) {
        bail!("readiness report must not introduce a closing issue reference");
    }
    let prefix = &description[..boundary.unwrap_or(description.len())];
    let mut body = prefix.to_owned();
    if boundary.is_none() && !prefix.is_empty() {
        if !prefix.ends_with('\n') {
            body.push('\n');
        }
        body.push('\n');
    }
    body.push_str(report);
    Ok(body)
}

#[derive(Deserialize)]
struct Request {
    description: String,
    report: String,
}

pub fn run() -> Result<()> {
    let request: Request = serde_json::from_reader(io::stdin().lock())
        .context("expected JSON string fields description and report on stdin")?;
    let body = compose_readiness_description(&request.description, &request.report)?;
    io::stdout().lock().write_all(body.as_bytes())?;
    Ok(())
}
