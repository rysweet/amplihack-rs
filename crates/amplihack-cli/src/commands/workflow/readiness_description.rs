//! Compose a readiness report without reconstructing the published description.
use anyhow::{Result, anyhow, bail};
use pulldown_cmark::{BrokenLink, Event, LinkType, Options, Parser, Tag, TagEnd};
use regex::Regex;
use serde::Deserialize;
use std::io::{self, Write};

const MARKER: &str = "<!-- merge-ready-report: v1 -->";
const TEMPLATE_MARKER: &str = "<!-- merge-ready-report: everything from this line down is the pre-merge readiness report and is left out of the squash commit -->";

/// Find one established report boundary, permitting its following legacy heading.
fn report_boundary(text: &str) -> Result<Option<usize>> {
    let mut boundary = None;
    let mut heading_seen = false;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let line_text = line.trim_end_matches(['\r', '\n']);
        if line_text == MARKER || line_text == TEMPLATE_MARKER {
            if boundary.is_some() {
                bail!("conflicting readiness report boundaries");
            }
            boundary = Some(offset);
        } else if line_text.eq_ignore_ascii_case("## Merge readiness") {
            if heading_seen {
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
    let prefix = &description[..boundary.unwrap_or(description.len())];
    reject_closing_references(report, prefix)?;
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

/// Validate the source first, then Markdown views without changing output bytes.
/// The parser resolves inline, full, collapsed and shortcut reference links.
fn reject_closing_references(report: &str, prefix: &str) -> Result<()> {
    let closing = Regex::new(
        r"(?i)\b(?:close[sd]?|fix(?:es|ed)?|resolve[sd]?)\b\s*:?\s*(?:[a-z0-9.-]+/[a-z0-9.-]+)?#[0-9]+|\b(?:close[sd]?|fix(?:es|ed)?|resolve[sd]?)\b\s*:?\s*https?://[^\s]+/issues/[0-9]+",
    )?;
    // Check source before removing any syntax. Stripping a benign link's
    // delimiters could expose a keyword from its hidden non-issue destination.
    if closing.is_match(report) {
        bail!("readiness report must not introduce a closing issue reference");
    }

    let issue_url = Regex::new(r"(?i)^https?://[^\s]+/issues/[0-9]+")?;
    let mut labels = String::new();
    let mut destinations = String::new();
    let mut combined = String::new();
    let mut issue_link = None;
    // Published definitions can resolve new report links, but published text
    // itself stays outside the rejection rule. Earlier definitions take priority.
    let published = Parser::new(prefix);
    let definitions = published.reference_definitions();
    let resolve = |link: BrokenLink<'_>| {
        definitions.get(&link.reference).map(|definition| {
            (
                definition.dest.clone(),
                definition.title.clone().unwrap_or_else(|| "".into()),
            )
        })
    };
    for event in
        Parser::new_with_broken_link_callback(report, Options::ENABLE_STRIKETHROUGH, Some(resolve))
    {
        match event {
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                id,
                ..
            }) => {
                let destination = if matches!(
                    link_type,
                    LinkType::Inline | LinkType::Autolink | LinkType::Email
                ) {
                    dest_url
                } else {
                    definitions
                        .get(&id)
                        .map_or(dest_url, |definition| definition.dest.clone())
                };
                if issue_url.is_match(&destination) {
                    issue_link = Some((destinations.len(), destination));
                }
            }
            Event::End(TagEnd::Link) => {
                if let Some((start, destination)) = issue_link.take() {
                    // Labels alone catch linked keywords next to references;
                    // destinations alone catch keywords next to linked issues;
                    // both together catch a keyword linked directly to an issue.
                    destinations.truncate(start);
                    destinations.push_str(&destination);
                    combined.push(' ');
                    combined.push_str(&destination);
                    combined.push(' ');
                }
            }
            Event::Text(text) | Event::Code(text) | Event::Html(text) | Event::InlineHtml(text) => {
                labels.push_str(&text);
                destinations.push_str(&text);
                combined.push_str(&text);
            }
            Event::SoftBreak | Event::HardBreak | Event::Rule => {
                labels.push('\n');
                destinations.push('\n');
                combined.push('\n');
            }
            Event::End(tag)
                if !matches!(
                    tag,
                    TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Image
                ) =>
            {
                labels.push('\n');
                destinations.push('\n');
                combined.push('\n');
            }
            _ => {}
        }
    }
    // Retain conservative handling of formatting and literal escapes in text,
    // after the parser has separated labels from hidden link metadata.
    let unformat = |text: &str| -> String {
        text.chars()
            .filter(|c| {
                !matches!(
                    c,
                    '*' | '_' | '`' | '~' | '\\' | '[' | ']' | '(' | ')' | '<' | '>'
                )
            })
            .collect()
    };
    if [&labels, &destinations, &combined]
        .iter()
        .any(|text| closing.is_match(&unformat(text)))
    {
        bail!("readiness report must not introduce a closing issue reference");
    }
    Ok(())
}

#[derive(Deserialize)]
struct Request {
    description: String,
    report: String,
}

pub fn run() -> Result<()> {
    let request: Request = serde_json::from_reader(io::stdin().lock())
        .map_err(|error| {
            anyhow!(
                "expected JSON string fields description and report on stdin: {:?} error at line {}, column {}",
                error.classify(), error.line(), error.column()
            )
        })?;
    let body = compose_readiness_description(&request.description, &request.report)?;
    io::stdout().lock().write_all(body.as_bytes())?;
    Ok(())
}
