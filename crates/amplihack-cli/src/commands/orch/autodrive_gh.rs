//! Auto-drive's GitHub reads, made to survive a GraphQL limit (issue #2333).
//!
//! Each read tries GraphQL (`gh pr view`, `gh pr checks`, `gh pr list`), then
//! REST (`gh api repos/{owner}/{repo}/…`), then, for the head only,
//! `git ls-remote origin refs/pull/{n}/head`. A read that fails on every path
//! is **unavailable**: a wait, never a finding such as "head differs".
//!
//! Nothing here sleeps, retries on a timer or polls: each source is tried
//! once. The bash tools call `amplihack orch helper autodrive-gh-read`; the
//! completion dispatch calls [`head`] and [`pr_for_branch`] directly.
//! Contract: docs/reference/autodrive-github-reads.md.

mod cli;
mod ratelimit;
mod reads;
mod rest_map;

pub use cli::{ReadArgs, UpdateBranchArgs, read_cli, update_branch_cli};
pub use reads::{
    Target, checks, current_branch_head, head, pr, pr_for_branch, reviews, update_branch,
};

use regex::Regex;
use serde::Serialize;
use serde_json::{Map, Value};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::LazyLock;

/// Exit code of a read that failed on every path.
pub const EXIT_UNAVAILABLE: i32 = 3;
/// Exit code of a usage error; nothing was run.
pub const EXIT_USAGE: i32 = 2;
/// Largest answer read from one source, across its pages.
const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_PAGES: u32 = 10;
const PER_PAGE: usize = 100;
const EXCERPT_BYTES: usize = 200;

// ── argument rules ──────────────────────────────────────────────────────────

/// A PR number `1`–`9999999999`, digits only, no leading zero.
pub fn valid_pr(s: &str) -> Option<u64> {
    let digits = !s.is_empty() && s.len() <= 10 && s.bytes().all(|b| b.is_ascii_digit());
    if !digits || s.starts_with('0') {
        return None;
    }
    s.parse().ok()
}

/// A full commit SHA: 40 lowercase hex characters.
pub fn valid_sha(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// A branch name under `git check-ref-format --branch` rules, never starting
/// with `-` so it can never read as an option.
pub fn valid_branch(s: &str) -> bool {
    if s.is_empty()
        || s.len() > 255
        || s == "@"
        || s.starts_with(['-', '/'])
        || s.ends_with(['/', '.'])
        || s.contains("..")
        || s.contains("//")
        || s.contains("@{")
    {
        return false;
    }
    if s.bytes()
        .any(|b| b < 0x21 || b == 0x7f || b"~^:?*[\\".contains(&b))
    {
        return false;
    }
    s.split('/')
        .all(|c| !c.is_empty() && !c.starts_with('.') && !c.ends_with(".lock"))
}

/// `owner/name`, each part `[A-Za-z0-9._-]`, not starting with `-` or `.`.
pub fn valid_repo(s: &str) -> bool {
    let part = |p: &str| {
        !p.is_empty()
            && p.len() <= 100
            && !p.starts_with(['-', '.'])
            && p.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-._".contains(&b))
    };
    s.split_once('/').is_some_and(|(o, r)| part(o) && part(r))
}

// ── excerpts ────────────────────────────────────────────────────────────────

static TOKEN_VALUE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(gh[pousr]_[A-Za-z0-9_]+|github_pat_[A-Za-z0-9_]+)").expect("static pattern")
});
static HEADER_VALUE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(authorization|bearer|token)(:?\s+)(?:(?:bearer|token|basic)\s+)?[^\s,;]+")
        .expect("static pattern")
});

/// One line of a failed call's stderr: the first non-blank line, token
/// values redacted, printable ASCII only, at most 200 bytes.
pub fn excerpt(text: &str) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let line = TOKEN_VALUE.replace_all(line, "[REDACTED]");
    let line = HEADER_VALUE.replace_all(&line, "${1}${2}[REDACTED]");
    let mut out: String = line.chars().filter(|c| (' '..='~').contains(c)).collect();
    out.truncate(EXCERPT_BYTES);
    out.trim().to_string()
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ── results ─────────────────────────────────────────────────────────────────

/// Why a read failed on every path.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Unavailable {
    pub reason: String,
    pub rate_limited: bool,
    pub reset_epoch: Option<i64>,
}

/// The outcome of one read.
#[derive(Debug)]
pub enum Read {
    Answer {
        source: &'static str,
        fields: Map<String, Value>,
    },
    Unavailable(Unavailable),
}

/// The failures met along the cascade, kept as one excerpt per path.
#[derive(Default)]
struct Trail {
    parts: Vec<String>,
    limited_texts: Vec<String>,
    graphql_limited: bool,
    rest_limited: bool,
}

impl Trail {
    fn fail(&mut self, path: &'static str, text: &str) {
        if ratelimit::classify(text) == ratelimit::Class::RateLimit {
            match path {
                "graphql" => self.graphql_limited = true,
                _ => self.rest_limited = true,
            }
            self.limited_texts.push(text.to_string());
        }
        self.parts.push(format!("{path}: {}", excerpt(text)));
    }

    fn unavailable(self, gh: &Gh) -> Unavailable {
        let rate_limited = self.graphql_limited || self.rest_limited;
        let now = now();
        let reset_epoch = if rate_limited {
            self.limited_texts
                .iter()
                .find_map(|t| ratelimit::reset_from_text(t, now))
                .or_else(|| gh.reset_lookup(self.graphql_limited, now))
        } else {
            None
        };
        Unavailable {
            reason: self.parts.join("; "),
            rate_limited,
            reset_epoch,
        }
    }
}

// ── the gh runner ───────────────────────────────────────────────────────────

struct Out {
    ok: bool,
    stdout: Vec<u8>,
    err: String,
}

/// Runs `gh` in one directory for one repository (`None`: the directory's
/// `origin`, through gh's `{owner}`/`{repo}` placeholders).
pub struct Gh<'a> {
    dir: &'a Path,
    repo: Option<&'a str>,
}

impl<'a> Gh<'a> {
    pub fn new(dir: &'a Path, repo: Option<&'a str>) -> Self {
        Self { dir, repo }
    }

    fn run(&self, args: &[String]) -> Out {
        let out = Command::new("gh")
            .current_dir(self.dir)
            .args(args)
            .env_remove("GH_REPO")
            .env("GH_PAGER", "cat")
            .env("GH_PROMPT_DISABLED", "1")
            .env("GH_NO_UPDATE_NOTIFIER", "1")
            .stdin(Stdio::null())
            .output();
        match out {
            Err(e) => Out {
                ok: false,
                stdout: Vec::new(),
                err: format!("gh could not run: {e}"),
            },
            Ok(o) if o.stdout.len() > MAX_BYTES => Out {
                ok: false,
                stdout: Vec::new(),
                err: "answer over 8 MiB".into(),
            },
            Ok(o) => {
                let mut err = String::from_utf8_lossy(&o.stderr).into_owned();
                if !o.status.success() && err.trim().is_empty() {
                    err = format!("exit {}", o.status.code().unwrap_or(-1));
                }
                Out {
                    ok: o.status.success(),
                    stdout: o.stdout,
                    err,
                }
            }
        }
    }

    fn json(&self, args: &[String]) -> Result<Value, String> {
        let o = self.run(args);
        if !o.ok {
            return Err(o.err);
        }
        serde_json::from_slice(&o.stdout).map_err(|e| format!("answer is not JSON ({e})"))
    }

    /// `--repo owner/name` for `gh pr …`, or nothing.
    fn repo_flag(&self) -> Vec<String> {
        self.repo
            .map(|r| vec!["--repo".to_string(), r.to_string()])
            .unwrap_or_default()
    }

    fn endpoint(&self, rest: &str) -> String {
        match self.repo {
            Some(r) => format!("repos/{r}/{rest}"),
            None => format!("repos/{{owner}}/{{repo}}/{rest}"),
        }
    }

    fn owner_value(&self) -> (String, String) {
        match self.repo.and_then(|r| r.split_once('/')) {
            Some((o, r)) => (o.to_string(), r.to_string()),
            None => ("{owner}".to_string(), "{repo}".to_string()),
        }
    }

    /// `-f` for a literal value, `-F` for a `{owner}`/`{repo}` placeholder,
    /// which gh fills only in `-F` fields.
    fn field_flag(&self) -> &'static str {
        if self.repo.is_some() { "-f" } else { "-F" }
    }

    fn view(&self, n: u64, fields: &str) -> Result<Value, String> {
        let mut a = vec![
            "pr".into(),
            "view".into(),
            n.to_string(),
            "--json".into(),
            fields.into(),
        ];
        a.extend(self.repo_flag());
        self.json(&a)
    }

    fn rest(&self, path: &str) -> Result<Value, String> {
        self.json(&["api".into(), self.endpoint(path)])
    }

    /// Every item of a paged REST list, at most 10 pages and 8 MiB. A larger
    /// answer is an error, never a truncated list.
    fn rest_pages(&self, path: &str, key: Option<&str>) -> Result<Vec<Value>, String> {
        let mut all = Vec::new();
        let mut bytes = 0usize;
        for page in 1..=MAX_PAGES {
            let o = self.run(&[
                "api".into(),
                self.endpoint(&format!("{path}?per_page={PER_PAGE}&page={page}")),
            ]);
            if !o.ok {
                return Err(o.err);
            }
            bytes += o.stdout.len();
            if bytes > MAX_BYTES {
                return Err("answer over 8 MiB".into());
            }
            let v: Value = serde_json::from_slice(&o.stdout)
                .map_err(|e| format!("answer is not JSON ({e})"))?;
            let list = match key {
                Some(k) => v.get(k).and_then(Value::as_array),
                None => v.as_array(),
            }
            .ok_or("answer holds no list")?;
            let n = list.len();
            all.extend(list.iter().cloned());
            if n < PER_PAGE {
                return Ok(all);
            }
        }
        Err(format!("answer over {MAX_PAGES} pages"))
    }

    /// GitHub's stated reset, from `gh api rate_limit` (which GitHub answers
    /// under a limit): the GraphQL resource when GraphQL was refused, else core.
    fn reset_lookup(&self, graphql: bool, now: i64) -> Option<i64> {
        let v = self.json(&["api".into(), "rate_limit".into()]).ok()?;
        let resource = if graphql { "graphql" } else { "core" };
        let reset = v["resources"][resource]["reset"].as_i64()?;
        ratelimit::bounded(reset, now)
    }
}

fn ls_remote(dir: &Path, n: u64) -> Result<String, String> {
    let refname = format!("refs/pull/{n}/head");
    let o = amplihack_git::command_in(dir)
        .args(["ls-remote", "origin", &refname])
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("git could not run: {e}"))?;
    if !o.status.success() {
        return Err(format!("exit {}", o.status.code().unwrap_or(-1)));
    }
    let text = String::from_utf8_lossy(&o.stdout);
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    match lines.as_slice() {
        [one] => match one.split_once('\t') {
            Some((sha, name)) if name == refname && valid_sha(sha) => Ok(sha.to_string()),
            _ => Err(format!("unexpected answer for {refname}")),
        },
        [] => Err(format!("no {refname} on origin")),
        _ => Err(format!("more than one line for {refname}")),
    }
}

#[cfg(test)]
#[path = "autodrive_gh_tests.rs"]
mod tests;
