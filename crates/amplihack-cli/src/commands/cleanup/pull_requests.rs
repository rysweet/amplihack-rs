//! Complete, invocation-local pull request evidence for manual cleanup.

use super::eligibility::{PrInfo, Verdict, pr_verdict};
use crate::commands::hygiene::cleanup::{hardened_git, sanitized_stderr_short};
use amplihack_workflows::remote_repository::github_repository_from_remote_url;
use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

const GH_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_OUTPUT: usize = 1024 * 1024;
const PAGE_SIZE: usize = 100;
const MAX_PAGES: usize = 40;

#[derive(PartialEq, Eq, Hash)]
pub(super) struct Repository {
    name: String,
    use_default_host: bool,
}

/// Resolve the checkout's GitHub repository without contacting its remotes.
/// Honor GH_REPO, then the selection saved by gh repo set-default. Otherwise
/// the GitHub fetch remotes must agree. GH_HOST applies unless GH_REPO names
/// a host explicitly. Diagnostics omit selection values.
pub(super) fn repository(checkout: &Path, git_timeout: Duration) -> Result<Repository> {
    if let Some(value) = std::env::var_os("GH_REPO").filter(|value| !value.is_empty()) {
        let value = value.to_str().context("GH_REPO is not UTF-8")?;
        let (name, use_default_host) = match value.split('/').count() {
            2 => {
                check_gh_host()?;
                (value, true)
            }
            3 => {
                let (host, name) = value.split_once('/').context("invalid GH_REPO")?;
                ensure!(
                    host.eq_ignore_ascii_case("github.com"),
                    "unsupported GH_REPO host"
                );
                (name, false)
            }
            _ => bail!("unsupported GH_REPO format"),
        };
        return Ok(Repository {
            name: public_repository_name(name).context("invalid GH_REPO repository")?,
            use_default_host,
        });
    }
    check_gh_host()?;
    let mut command = hardened_git();
    command.arg("-C").arg(checkout).args(["remote", "-v"]);
    let output = crate::util::run_output_with_timeout_limited(command, git_timeout, MAX_OUTPUT)
        .context("cannot read git remotes")?;
    ensure!(output.status.success(), "cannot read git remotes");
    ensure!(
        output.stdout.len() < MAX_OUTPUT,
        "git remote output is too large"
    );
    let remotes = std::str::from_utf8(&output.stdout).context("git remotes are not UTF-8")?;
    let mut remotes_by_name = HashSet::new();
    for line in remotes.lines() {
        let (remote, url) = line.split_once('\t').context("invalid git remote output")?;
        let Some(url) = url.strip_suffix(" (fetch)") else {
            ensure!(url.ends_with(" (push)"), "invalid git remote output");
            continue;
        };
        if let Some(name) = public_github_repository(url) {
            remotes_by_name.insert((remote, name));
        }
    }
    if let Some(name) = configured_repository(checkout, git_timeout, &remotes_by_name)? {
        return Ok(Repository {
            name,
            use_default_host: false,
        });
    }
    let mut names = remotes_by_name.into_iter().map(|(_, name)| name);
    let name = names
        .next()
        .context("checkout has no identifiable GitHub repository")?;
    ensure!(
        names.all(|other| other == name),
        "checkout has several GitHub repositories"
    );
    Ok(Repository {
        name,
        use_default_host: false,
    })
}

fn check_gh_host() -> Result<()> {
    // gh filters checkout remotes by an explicit GH_HOST. A public listing
    // cannot establish PR evidence for a different selected host.
    ensure!(
        std::env::var_os("GH_HOST")
            .is_none_or(|host| host.is_empty() || host.eq_ignore_ascii_case("github.com")),
        "unsupported GH_HOST for PR listing"
    );
    Ok(())
}

fn configured_repository(
    checkout: &Path,
    git_timeout: Duration,
    remotes: &HashSet<(&str, String)>,
) -> Result<Option<String>> {
    let mut command = hardened_git();
    command.arg("-C").arg(checkout).args([
        "config",
        "--null",
        "--get-regexp",
        r"^remote\..*\.gh-resolved$",
    ]);
    let output = crate::util::run_output_with_timeout_limited(command, git_timeout, MAX_OUTPUT)
        .context("cannot read GitHub repository selection")?;
    if output.status.code() == Some(1) && output.stdout.is_empty() {
        return Ok(None);
    }
    ensure!(
        output.status.success(),
        "cannot read GitHub repository selection"
    );
    ensure!(
        output.stdout.len() < MAX_OUTPUT,
        "GitHub repository selection is too large"
    );
    let records = std::str::from_utf8(&output.stdout)
        .context("GitHub repository selection is not UTF-8")?
        .strip_suffix('\0')
        .context("invalid GitHub repository selection")?;
    let mut selected = None;
    for record in records.split('\0') {
        let (remote, value) = record
            .split_once('\n')
            .and_then(|(key, value)| {
                Some((
                    key.strip_prefix("remote.")?.strip_suffix(".gh-resolved")?,
                    value,
                ))
            })
            .context("invalid GitHub repository selection")?;
        let mut names = remotes.iter().filter(|(key, _)| *key == remote);
        let (_, remote_repo) = names
            .next()
            .context("selected remote has no public GitHub fetch URL")?;
        let name = if value == "base" {
            ensure!(
                names.next().is_none(),
                "selected remote has several GitHub repositories"
            );
            remote_repo.clone()
        } else {
            // gh stores owner/repo when the selection has no remote of its own.
            public_repository_name(value).context("invalid configured GitHub repository")?
        };
        ensure!(
            selected.as_ref().is_none_or(|known| *known == name),
            "checkout has conflicting GitHub repository selections"
        );
        selected = Some(name);
    }
    Ok(selected)
}

fn public_repository_name(value: &str) -> Option<String> {
    public_github_repository(&format!("https://github.com/{value}"))
        .filter(|name| name.eq_ignore_ascii_case(value))
}

fn public_github_repository(url: &str) -> Option<String> {
    // The shared identity parser also accepts provider subdomains, arbitrary
    // ports and PR web URLs. This listing targets the public GitHub API.
    let (authority, path, default_port) = if let Some((scheme, rest)) = url.split_once("://") {
        let default_port = match scheme {
            "https" => "443",
            "http" => "80",
            "ssh" => "22",
            "git" => "9418",
            _ => return None,
        };
        let (authority, path) = rest.split_once('/')?;
        (authority, path, Some(default_port))
    } else {
        let (authority, path) = url.split_once(':')?;
        (authority, path, None)
    };
    let host_port = authority.rsplit('@').next()?;
    let (host, port) = host_port
        .split_once(':')
        .map_or((host_port, None), |(host, port)| (host, Some(port)));
    let host = host.strip_suffix('.').unwrap_or(host);
    let public =
        host.eq_ignore_ascii_case("github.com") && (port.is_none() || port == default_port);
    let alternate_ssh = host.eq_ignore_ascii_case("ssh.github.com")
        && url.starts_with("ssh://")
        && port == Some("443");
    let path = path.strip_suffix('/').unwrap_or(path);
    if !(public || alternate_ssh) || path.contains(['?', '#']) || path.split('/').count() != 2 {
        return None;
    }
    github_repository_from_remote_url(url).map(|name| name.to_ascii_lowercase())
}

/// Complete PR evidence from one repository. Ascending creation order keeps
/// PRs in the same position when other PRs are closed, reopened, or updated.
pub(super) struct Listing {
    branches: HashMap<String, PrInfo>,
}

impl Listing {
    pub(super) fn read(repository: &Repository) -> Result<Self, String> {
        let endpoint = format!("repos/{}/pulls", repository.name);
        let mut listing = Self {
            branches: HashMap::new(),
        };
        let mut seen = HashSet::new();
        let mut advertised_last = 1;
        for page in 1..=MAX_PAGES {
            let next_page = read_page(repository, &endpoint, page, &mut advertised_last)
                .and_then(|(rows, has_next)| {
                    for row in rows {
                        let (branch, pr) = row.into_pr()?;
                        // Repeated rows can indicate that the listing shifted.
                        ensure!(seen.insert(pr.number), "PR listing repeated a pull request");
                        listing.record(branch, pr);
                    }
                    Ok(has_next)
                })
                .map_err(|error| format!("PR listing page {page}: {error:#}"))?;
            if !next_page {
                return Ok(listing);
            }
        }
        Err(format!("PR listing exceeds the {MAX_PAGES}-request limit"))
    }

    fn record(&mut self, branch: String, pr: PrInfo) {
        use std::collections::hash_map::Entry;

        match self.branches.entry(branch) {
            Entry::Occupied(mut entry) => {
                let known = entry.get();
                if known.state != "OPEN"
                    && (pr.state == "OPEN"
                        || pr.closed_at.or(pr.merged_at) >= known.closed_at.or(known.merged_at))
                {
                    entry.insert(pr);
                }
            }
            Entry::Vacant(entry) => {
                entry.insert(pr);
            }
        }
    }

    pub(super) fn verdict(
        &self,
        branch: &str,
        now: DateTime<Utc>,
        keep: Duration,
        no_pr: Verdict,
    ) -> Verdict {
        self.branches
            .get(branch)
            .map_or(no_pr, |pr| pr_verdict(pr, now, keep))
    }
}

fn read_page(
    repository: &Repository,
    endpoint: &str,
    page: usize,
    advertised_last: &mut usize,
) -> Result<(Vec<RawPr>, bool)> {
    let body = request_page(repository, endpoint, page)?;
    let (payload, has_next, last_page) = response_page(&body, endpoint, page)?;
    *advertised_last = (*advertised_last).max(last_page.unwrap_or(1));
    ensure!(
        has_next || page >= *advertised_last,
        "PR listing ended before advertised page {advertised_last}"
    );
    ensure!(
        *advertised_last <= MAX_PAGES,
        "PR listing exceeds the {MAX_PAGES}-request limit"
    );
    let rows: Vec<RawPr> = serde_json::from_str(payload).context("cannot parse PR listing")?;
    ensure!(rows.len() <= PAGE_SIZE, "PR listing exceeds the page size");
    Ok((rows, has_next))
}

fn request_page(repository: &Repository, endpoint: &str, page: usize) -> Result<Vec<u8>> {
    let mut command = Command::new("gh");
    amplihack_git::scrub(&mut command);
    command.arg("api");
    // Bare GH_REPO names use gh's configured default, including GH_HOST.
    if !repository.use_default_host {
        command.args(["--hostname", "github.com"]);
    }
    command
        .args(["--include", "--method", "GET"])
        .arg(endpoint)
        .args([
            "-f",
            "state=all",
            "-f",
            "sort=created",
            "-f",
            "direction=asc",
            "-f",
        ])
        .arg(format!("per_page={PAGE_SIZE}"))
        .arg("-f")
        .arg(format!("page={page}"))
        .arg("--jq")
        .arg(concat!(
            "if type == \"array\" then ",
            "map({number, state, closed_at, merged_at, head: {ref: .head.ref}}) ",
            "else error(\"expected an array of pull requests\") end",
        ))
        .stdin(Stdio::null())
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1");
    let output = crate::util::run_output_with_timeout_limited(command, GH_TIMEOUT, MAX_OUTPUT)?;
    ensure!(
        output.status.success(),
        "{}: {}",
        output.status,
        sanitized_stderr_short(&output.stderr)
    );
    ensure!(output.stdout.len() < MAX_OUTPUT, "gh output is too large");
    Ok(output.stdout)
}

fn response_page<'a>(
    output: &'a [u8],
    endpoint: &str,
    page: usize,
) -> Result<(&'a str, bool, Option<usize>)> {
    let text = std::str::from_utf8(output).context("gh output is not UTF-8")?;
    let (headers, payload) = text
        .split_once("\n\n")
        .or_else(|| text.split_once("\r\n\r\n"))
        .context("gh output is missing HTTP headers")?;
    let mut headers = headers.lines();
    let mut status = headers.next().unwrap_or_default().split_whitespace();
    ensure!(
        status
            .next()
            .is_some_and(|version| version.starts_with("HTTP/"))
            && status.next() == Some("200"),
        "PR listing did not return HTTP 200"
    );
    let mut relations = HashSet::new();
    let mut last_page = None;
    for header in headers {
        let (name, value) = header.split_once(':').context("invalid HTTP header")?;
        if name.trim().eq_ignore_ascii_case("link") {
            for link in value.split(',') {
                let (url, relation) = link
                    .trim()
                    .strip_prefix('<')
                    .and_then(|link| link.split_once(">;"))
                    .context("invalid PR pagination link")?;
                let relation = relation
                    .trim()
                    .strip_prefix("rel=\"")
                    .and_then(|value| value.strip_suffix('"'))
                    .filter(|value| matches!(*value, "next" | "prev" | "first" | "last"))
                    .context("invalid PR pagination relation")?;
                ensure!(
                    relations.insert(relation),
                    "repeated PR pagination relation"
                );
                match relation {
                    "next" => ensure!(
                        linked_page(url, endpoint)? == page + 1,
                        "PR continuation does not advance to the next page"
                    ),
                    "last" => last_page = Some(linked_page(url, endpoint)?),
                    _ => {}
                }
            }
        }
    }
    let has_next = relations.contains("next");
    ensure!(
        last_page.is_none_or(|last| last >= page && (last > page) == has_next),
        "inconsistent PR pagination links"
    );
    Ok((payload, has_next, last_page))
}

fn linked_page(url: &str, endpoint: &str) -> Result<usize> {
    let (path, query) = url
        .strip_prefix("https://api.github.com/")
        .and_then(|url| url.split_once('?'))
        .context("invalid PR continuation URL")?;
    // GitHub can use a numeric repository path in its Link header. Page
    // requests always use the original endpoint.
    let numeric_repository = path
        .strip_prefix("repositories/")
        .and_then(|path| path.strip_suffix("/pulls"))
        .is_some_and(|id| !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()));
    ensure!(
        path.eq_ignore_ascii_case(endpoint) || numeric_repository,
        "invalid PR continuation path"
    );
    let mut pages = query
        .split('&')
        .filter_map(|part| part.strip_prefix("page="));
    let page = pages
        .next()
        .filter(|value| value.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|page| *page > 0)
        .context("PR continuation has no valid page number")?;
    ensure!(
        pages.next().is_none() && !query.contains('#'),
        "PR continuation has an invalid query"
    );
    Ok(page)
}

#[derive(Deserialize)]
struct RawPr {
    number: u64,
    state: String,
    closed_at: Option<DateTime<Utc>>,
    merged_at: Option<DateTime<Utc>>,
    head: PrHead,
}

#[derive(Deserialize)]
struct PrHead {
    r#ref: String,
}

impl RawPr {
    fn into_pr(self) -> Result<(String, PrInfo)> {
        ensure!(
            self.number > 0
                && !self.head.r#ref.is_empty()
                && !self.head.r#ref.contains(char::is_control),
            "PR listing has an invalid number or head ref"
        );
        let state = match self.state.as_str() {
            "open" if self.closed_at.is_none() && self.merged_at.is_none() => "OPEN",
            "closed" if self.merged_at.is_some() => "MERGED",
            "closed" if self.closed_at.is_some() => "CLOSED",
            _ => bail!("PR listing has an invalid state or closure time"),
        };
        Ok((
            self.head.r#ref,
            PrInfo {
                number: self.number,
                state: state.into(),
                closed_at: self.closed_at,
                merged_at: self.merged_at,
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENDPOINT: &str = "repos/test-owner/test-repo/pulls";

    #[test]
    fn response_separates_http_headers_from_json_and_reads_continuation() {
        let first = b"HTTP/2.0 200 OK\r\nLink: <https://api.github.com/repositories/12/pulls?state=all&page=2>; rel=\"next\", <https://api.github.com/repositories/12/pulls?state=all&page=40>; rel=\"last\"\r\n\r\n[]";
        assert_eq!(
            response_page(first, ENDPOINT, 1).unwrap(),
            ("[]", true, Some(40))
        );
        let last = b"HTTP/2.0 200 OK\nLink: <https://api.github.com/repositories/12/pulls?page=39>; rel=\"prev\"\n\n[]";
        assert_eq!(
            response_page(last, ENDPOINT, MAX_PAGES).unwrap(),
            ("[]", false, None)
        );
    }

    #[test]
    fn response_rejects_invalid_or_inconsistent_pagination() {
        for header in [
            "Link: broken",
            "Link: <https://api.github.com/repositories/12/pulls?page=1>; rel=\"next\"",
            "Link: <https://api.github.com/repositories/12/pulls?page=2&page=2>; rel=\"next\"",
            "Link: <https://api.github.com/repositories/12/pulls?page=2>; rel=\"last\"",
            "Link: <https://api.github.com/repositories/12/pulls?page=2>; rel=\"next\", <https://api.github.com/repositories/12/pulls?page=2>; rel=\"next\"",
            "Link: <https://api.github.com/repos/test-owner/other/pulls?page=2>; rel=\"next\"",
        ] {
            let response = format!("HTTP/2.0 200 OK\n{header}\n\n[]");
            assert!(response_page(response.as_bytes(), ENDPOINT, 1).is_err());
        }
        assert!(response_page(b"[]", ENDPOINT, 1).is_err());
        assert!(response_page(b"HTTP/2.0 206 Partial Content\n\n[]", ENDPOINT, 1).is_err());
    }

    #[test]
    fn listing_rows_validate_pr_state_and_dates() {
        let merged = r#"{"number":3,"state":"closed","closed_at":null,"merged_at":"2026-10-31T08:02:11Z","head":{"ref":"feat/a"}}"#;
        let (branch, pr) = serde_json::from_str::<RawPr>(merged)
            .unwrap()
            .into_pr()
            .unwrap();
        assert_eq!(branch, "feat/a");
        assert_eq!(pr.state, "MERGED");
        assert_eq!(pr.number, 3);
        assert!(pr.merged_at.is_some());
        assert!(
            serde_json::from_str::<RawPr>(&merged.replace("2026-10-31T08:02:11Z", "yesterday"))
                .is_err()
        );
        assert!(
            serde_json::from_str::<RawPr>(&merged.replace("\"2026-10-31T08:02:11Z\"", "null"))
                .unwrap()
                .into_pr()
                .is_err()
        );
    }
}
