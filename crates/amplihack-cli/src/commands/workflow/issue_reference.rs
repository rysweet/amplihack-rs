//! Issue keyword selection and preservation. Classification and issue discovery
//! remain the callers' responsibility; only the exported recipe policy is read.

use anyhow::{Context, Result, bail, ensure};
use clap::Subcommand;
use regex::Regex;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::Duration;

const BODY_LIMIT: usize = 65_536;
const EVIDENCE_LIMIT: u64 = 1_048_576;
const GITHUB_TIMEOUT: Duration = Duration::from_secs(30);
// Allow the worst-case JSON escaping of a bounded body plus response metadata.
const BODY_RESPONSE_LIMIT: usize = BODY_LIMIT * 6 + 4096;

#[derive(Debug, Subcommand)]
pub enum IssueReferenceCommands {
    /// Validate RECIPE_VAR_pr_issue_link; successful validation writes nothing.
    Validate,
    /// Read classifier --classify output from stdin and print Closes or Refs.
    Keyword,
    /// Replace a GitHub body while preserving its published issue references.
    EditBody {
        #[arg(long)]
        repo: String,
        #[arg(long)]
        pr: String,
        #[arg(long)]
        candidate_file: PathBuf,
    },
}

#[derive(Clone, Copy)]
enum Policy {
    Auto,
    Refs,
    Closes,
}

fn invocation(message: impl std::fmt::Display) -> anyhow::Error {
    // The binary deliberately does not print CliExitError; its caller owns
    // the diagnostic so it can keep stdout reserved for machine output.
    eprintln!("error: {message}");
    crate::command_error::exit_error(2)
}

fn policy() -> Result<Policy> {
    parse_policy(std::env::var_os("RECIPE_VAR_pr_issue_link"))
}

pub(crate) fn validate_value(value: &str) -> Result<()> {
    parse_policy(Some(value.into())).map(|_| ())
}

fn parse_policy(value: Option<std::ffi::OsString>) -> Result<Policy> {
    match value {
        None => Ok(Policy::Auto),
        Some(value) => match value.to_str() {
            Some("auto") => Ok(Policy::Auto),
            Some("refs") => Ok(Policy::Refs),
            Some("closes") => Ok(Policy::Closes),
            _ => Err(invocation(format!(
                "invalid pr_issue_link {value:?}; expected auto, refs or closes"
            ))),
        },
    }
}

pub fn dispatch(command: IssueReferenceCommands) -> Result<()> {
    match command {
        IssueReferenceCommands::Validate => {
            policy()?;
        }
        IssueReferenceCommands::Keyword => {
            // Validate before consuming even empty or unusable evidence.
            let policy = policy()?;
            let mut evidence = Vec::new();
            std::io::stdin()
                .take(EVIDENCE_LIMIT + 1)
                .read_to_end(&mut evidence)
                .context("could not read issue classification from stdin")?;
            let mut substantive = false;
            let usable = evidence.len() <= EVIDENCE_LIMIT as usize
                && std::str::from_utf8(&evidence).is_ok_and(|text| {
                    text.lines().all(|line| {
                        let Some((class, path)) = line.split_once(' ') else {
                            return false;
                        };
                        substantive |= class == "substantive";
                        !path.trim().is_empty()
                            && matches!(class, "substantive" | "planning" | "generated")
                    })
                });
            println!(
                "{}",
                if usable && substantive && !matches!(policy, Policy::Refs) {
                    "Closes"
                } else {
                    "Refs"
                }
            );
        }
        IssueReferenceCommands::EditBody {
            repo,
            pr,
            candidate_file,
        } => {
            ensure_invocation(&repo, &pr)?;
            let file = std::fs::File::open(&candidate_file)
                .context("could not open candidate body file")?;
            let mut candidate = String::new();
            file.take(BODY_LIMIT as u64 + 1)
                .read_to_string(&mut candidate)
                .context("candidate body must be readable UTF-8")?;
            check_body(&candidate)?;
            edit_body(&repo, &pr, &candidate)?;
        }
    }
    Ok(())
}

fn valid_component(part: &str) -> bool {
    !part.is_empty()
        && part != "."
        && part != ".."
        && !part.starts_with('-')
        && part
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}

fn ensure_invocation(repo: &str, pr: &str) -> Result<()> {
    let parts: Vec<_> = repo.split('/').collect();
    if parts.len() != 2 || !parts.iter().all(|p| valid_component(p)) {
        return Err(invocation("--repo must be an explicit owner/repository"));
    }
    if !pr.bytes().all(|b| b.is_ascii_digit())
        || pr.parse::<u64>().ok().filter(|n| *n > 0).is_none()
    {
        return Err(invocation("--pr must be a positive pull request number"));
    }
    Ok(())
}

fn check_body(body: &str) -> Result<()> {
    ensure!(
        body.len() <= BODY_LIMIT,
        "issue-reference body exceeds 65,536 bytes"
    );
    ensure!(
        !body.contains('\0'),
        "issue-reference body contains a NUL character"
    );
    Ok(())
}

#[derive(Clone)]
struct Reference {
    start: usize,
    end: usize,
    identity: String,
    published: String,
}

/// Mask code spans without changing byte offsets. Fences follow Markdown's
/// three-or-more backtick/tilde rule; inline spans use matching delimiter runs.
fn prose(body: &str) -> String {
    let mut bytes = body.as_bytes().to_vec();
    let mut fence: Option<(u8, usize)> = None;
    let mut offset = 0;
    for line in body.split_inclusive('\n') {
        let trimmed = line.trim_start_matches(' ');
        let indent = line.len() - trimmed.len();
        let marker = trimmed.as_bytes().first().copied().unwrap_or(0);
        let count = trimmed.bytes().take_while(|b| *b == marker).count();
        let delimiter = indent <= 3 && matches!(marker, b'`' | b'~') && count >= 3;
        if let Some((open, width)) = fence {
            if delimiter && marker == open && count >= width && trimmed[count..].trim().is_empty() {
                fence = None;
            }
            bytes[offset..offset + line.len()].fill(0);
        } else if delimiter {
            fence = Some((marker, count));
            bytes[offset..offset + line.len()].fill(0);
        }
        offset += line.len();
    }
    // Precompute matching delimiter runs. An unmatched backtick is ordinary
    // prose, and must not hide a published reference later in the body.
    let mut runs = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'`' {
            i += 1;
            continue;
        }
        let end = i + bytes[i..].iter().take_while(|b| **b == b'`').count();
        let escaped = bytes[..i].iter().rev().take_while(|b| **b == b'\\').count() % 2 == 1;
        runs.push((i, end, escaped));
        i = end;
    }
    let mut next = vec![None; runs.len()];
    let mut widths = BTreeMap::new();
    for (index, (start, end, _)) in runs.iter().enumerate().rev() {
        next[index] = widths.insert(end - start, index);
    }
    let mut index = 0;
    while index < runs.len() {
        if !runs[index].2
            && let Some(close) = next[index]
        {
            // A non-whitespace sentinel also prevents a keyword and identity
            // on opposite sides of code from becoming a new reference.
            bytes[runs[index].0..runs[close].1].fill(0);
            index = close + 1;
        } else {
            index += 1;
        }
    }
    // Non-ASCII code bytes were all masked together; prose is untouched.
    String::from_utf8(bytes).expect("masking complete code spans preserves UTF-8")
}

fn identity(text: &str, repo: &str) -> Option<String> {
    let prefix = "https://github.com/";
    let (repository, number) = if text
        .get(..prefix.len())
        .is_some_and(|s| s.eq_ignore_ascii_case(prefix))
    {
        let url = &text[prefix.len()..];
        let parts: Vec<_> = url.split('/').collect();
        if parts.len() != 4 || parts[2] != "issues" {
            return None;
        }
        (format!("{}/{}", parts[0], parts[1]), parts[3])
    } else {
        let (prefix, number) = text.split_once('#')?;
        (
            if prefix.is_empty() {
                repo.to_owned()
            } else {
                prefix.to_owned()
            },
            number,
        )
    };
    let parts: Vec<_> = repository.split('/').collect();
    if parts.len() != 2
        || !parts.iter().all(|p| valid_component(p))
        || !number.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let number = number.parse::<u64>().ok().filter(|n| *n > 0)?;
    Some(format!("{}#{number}", repository.to_ascii_lowercase()))
}

fn references(body: &str, repo: &str) -> Vec<Reference> {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    let pattern = PATTERN.get_or_init(|| Regex::new(
        r"(?i)\b(refs|close|closes|closed|fix|fixes|fixed|resolve|resolves|resolved)[ \t\r\n]+(https://[^\s<>()`]+|[a-z0-9_.-]+/[a-z0-9_.-]+#[0-9]+|#[0-9]+)"
    ).expect("static reference expression"));
    let visible = prose(body);
    pattern
        .captures_iter(&visible)
        .filter_map(|c| {
            let whole = c.get(0)?;
            let raw = c.get(2)?;
            let text = raw.as_str().trim_end_matches(['.', ',', ';', ':']);
            let end = raw.start() + text.len();
            if visible
                .as_bytes()
                .get(end)
                .is_some_and(|b| b.is_ascii_alphanumeric() || b"_/#?=-".contains(b))
            {
                return None;
            }
            let identity = identity(text, repo)?;
            Some(Reference {
                start: whole.start(),
                end,
                identity,
                published: format!("{} {text}", c.get(1)?.as_str()),
            })
        })
        .collect()
}

fn report_boundary(body: &str) -> usize {
    let mut offset = 0;
    for line in body.split_inclusive('\n') {
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("<!-- merge-ready-report") || lower.starts_with("## merge readiness") {
            return offset;
        }
        offset += line.len();
    }
    body.len()
}

/// Preserve the published spelling and identity, independently of today's
/// policy or diff. References are kept above the squash report boundary.
fn preserve(baseline: &str, candidate: &str, repo: &str) -> Result<String> {
    check_body(baseline)?;
    check_body(candidate)?;
    let mut published = BTreeMap::new();
    for reference in references(baseline, repo) {
        if let Some(previous) = published.insert(reference.identity, reference.published.clone()) {
            ensure!(
                previous == reference.published,
                "ambiguous published issue references; body was not edited"
            );
        }
    }
    if published.is_empty() {
        return Ok(candidate.to_owned());
    }
    let mut result = candidate.to_owned();
    // Reverse offsets keep earlier ranges valid. Code examples are untouched.
    for reference in references(candidate, repo).iter().rev() {
        if published.contains_key(&reference.identity) {
            result.replace_range(reference.start..reference.end, "");
        }
    }
    let boundary = report_boundary(&result);
    let mut description = result[..boundary].trim_end().to_owned();
    if !description.is_empty() {
        description.push_str("\n\n");
    }
    for reference in published.values() {
        description.push_str(reference);
        description.push('\n');
    }
    if boundary < result.len() {
        description.push('\n');
        description.push_str(&result[boundary..]);
    }
    check_body(&description)?;
    let operative: BTreeMap<_, _> = references(&description, repo)
        .into_iter()
        .filter(|r| r.end <= report_boundary(&description))
        .map(|r| (r.identity, r.published))
        .collect();
    ensure!(
        published
            .iter()
            .all(|(id, text)| operative.get(id) == Some(text)),
        "candidate Markdown prevents operative reference preservation; body was not edited"
    );
    Ok(description)
}

fn view_body(repo: &str, pr: &str) -> Result<String> {
    let mut command = Command::new("gh");
    command
        .args(["pr", "view", pr, "--repo", repo, "--json", "body"])
        .stdin(Stdio::null());
    let output =
        crate::util::run_output_with_timeout_limited(command, GITHUB_TIMEOUT, BODY_RESPONSE_LIMIT)
            .context("could not read GitHub PR body")?;
    ensure!(
        output.status.success(),
        "GitHub PR body read failed ({})",
        output.status
    );
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).context("GitHub body response is not valid JSON")?;
    let body = match value.get("body") {
        Some(serde_json::Value::String(body)) => body.clone(),
        Some(serde_json::Value::Null) => String::new(),
        _ => bail!("GitHub body response has no readable body"),
    };
    check_body(&body)?;
    Ok(body)
}

fn edit_body(repo: &str, pr: &str, candidate: &str) -> Result<()> {
    let baseline = view_body(repo, pr)?;
    let body = preserve(&baseline, candidate, repo)?;
    let mut command = Command::new("gh");
    command.args(["pr", "edit", pr, "--repo", repo, "--body-file", "-"]);
    // The existing helper bounds both process waiting and stdin delivery.
    // Never retry an edit: a timed-out request may already have been accepted.
    let output = crate::util::run_output_with_timeout_stdin(
        command,
        GITHUB_TIMEOUT,
        Some(body.as_bytes().to_vec()),
    )
    .context("GitHub body edit failed; update may already have occurred")?;
    let status = output.status;
    ensure!(
        status.success(),
        "GitHub body edit failed ({status}); update may already have occurred"
    );
    let readback =
        view_body(repo, pr).context("GitHub readback failed; update may already have occurred")?;
    ensure!(
        readback == body,
        "GitHub readback differs from submitted body; update may already have occurred"
    );
    // Exact readback also verifies reference positions and their operative form.
    Ok(())
}
