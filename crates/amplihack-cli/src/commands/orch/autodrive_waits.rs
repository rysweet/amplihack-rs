//! Splits one round's blockers into waits and fixable findings (issue #2472,
//! waits slice). Pure: no `gh`, no `git`, no files.
//!
//! Waits come from measured facts (the CI signal, the merge state, reads that
//! were unavailable) and from a fixed list of wait ids matched on the whole
//! id. Anything else is fixable, so a classing error leans toward `STUCK`,
//! never toward a silent wait. `wait_target` is built only from closed
//! patterns and validated SHAs; no input text reaches it.
//! Contract: docs/reference/autodrive-github-reads.md#waits-and-fixable-findings.

use serde::{Deserialize, Serialize};
use std::io::Read;

/// Finding ids that are waits when an agent or a tool reports them.
const WAIT_IDS: &[&str] = &[
    "ci-pending",
    "approval-facts-unreadable",
    "expected-checks-unreadable",
    "crusty-pr-head-unreadable",
];
const MEAD: &str = "measured-evidence-disagrees";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnavailableRead {
    pub read: String,
    pub rate_limited: bool,
    pub reset_epoch: Option<i64>,
    /// Every path refused the read (bad credentials, no permission, not
    /// found): a fixable finding, because waiting will not clear it.
    #[serde(default)]
    pub refused: bool,
}

/// Finding id for a read that every path refused.
const READ_REFUSED: &str = "gh-read-refused";

/// The outcome of an R7 merge-state recompute, when step-01 ran one.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recompute {
    #[serde(deserialize_with = "recompute_action")]
    pub action: String,
    #[serde(deserialize_with = "recompute_result")]
    pub result: String,
    #[serde(default)]
    pub pushed_sha: Option<String>,
    #[serde(default)]
    pub rate_limited: bool,
    #[serde(default)]
    pub reset_epoch: Option<i64>,
}

/// The recompute actions step-01 can record: the helper's, and `none` when
/// it wrote nothing (`first-read`).
const RECOMPUTE_ACTIONS: &[&str] = &["merge-push", "update-branch", "noop-up-to-date", "none"];
const RECOMPUTE_RESULTS: &[&str] = &["ok", "refused", "unavailable", "first-read"];

fn one_of<'de, D: serde::Deserializer<'de>>(
    d: D,
    allowed: &'static [&'static str],
    what: &str,
) -> Result<String, D::Error> {
    let v = String::deserialize(d)?;
    if allowed.contains(&v.as_str()) {
        Ok(v)
    } else {
        Err(serde::de::Error::custom(format!(
            "recompute {what} is not one of {allowed:?}"
        )))
    }
}

fn recompute_action<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    one_of(d, RECOMPUTE_ACTIONS, "action")
}

fn recompute_result<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    one_of(d, RECOMPUTE_RESULTS, "result")
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub head_sha: String,
    pub ci_signal: String,
    pub merge_state: String,
    pub mergeable: String,
    pub unavailable: Vec<UnavailableRead>,
    pub findings: Vec<String>,
    pub measured_downgrade: Vec<String>,
    /// Optional: present only after an R7 recompute.
    #[serde(default)]
    pub recompute: Option<Recompute>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Output {
    pub waits: Vec<String>,
    pub fixable: Vec<String>,
    pub wait_target: String,
    pub round_waiting: bool,
}

fn valid_sha(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// `<kind>@<sha>` when the SHA is valid, else a head re-read.
fn at_sha(kind: &str, sha: &str) -> String {
    if valid_sha(sha) {
        format!("{kind}@{sha}")
    } else {
        "read@pr-head".to_string()
    }
}

/// The wait id and `read@…` target for a helper read name; unknown names
/// are not waits.
fn read_wait(read: &str) -> Option<(&'static str, &'static str)> {
    Some(match read {
        "pr" => ("pr-unavailable", "read@pr"),
        "head" => ("head-unavailable", "read@pr-head"),
        "pr-for-branch" => ("pr-number-unavailable", "read@pr-number"),
        "checks" => ("checks-unavailable", "read@checks"),
        "reviews" => ("reviews-unavailable", "read@reviews"),
        "threads" => ("threads-unavailable", "read@threads"),
        _ => return None,
    })
}

/// The targets one wait could name, by precedence rank.
#[derive(Default)]
struct Targets {
    reset: Option<Option<i64>>,
    ci: Option<String>,
    mergeability: Option<String>,
    read: Option<String>,
}

impl Targets {
    fn reset(&mut self, epoch: Option<i64>) {
        self.reset = Some(match (self.reset.flatten(), epoch) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        });
    }
    fn read(&mut self, t: &str) {
        self.read.get_or_insert_with(|| t.to_string());
    }
    fn first(&self) -> String {
        match (&self.reset, &self.ci, &self.mergeability, &self.read) {
            (Some(Some(e)), ..) => format!("ratelimit-reset@{e}"),
            (Some(None), ..) => "ratelimit-reset@unknown".into(),
            (None, Some(t), ..) | (None, None, Some(t), _) | (None, None, None, Some(t)) => {
                t.clone()
            }
            _ => String::new(),
        }
    }
}

fn push(list: &mut Vec<String>, id: &str) {
    if !list.iter().any(|x| x == id) {
        list.push(id.to_string());
    }
}

/// Classes one round.
pub fn classify(input: &Input) -> Output {
    let sha = input.head_sha.as_str();
    let mut waits = Vec::new();
    let mut fixable = Vec::new();
    let mut t = Targets::default();
    let mut measured = false; // a measured CI or mergeability wait

    // Measured facts.
    match input.ci_signal.as_str() {
        "PENDING" | "QUEUED" => {
            push(&mut waits, "ci-pending");
            t.ci = Some(at_sha("ci", sha));
            measured = true;
        }
        "UNREADABLE" => {
            push(&mut waits, "expected-checks-unreadable");
            t.read("read@checks");
        }
        _ => {}
    }
    if input.merge_state == "UNKNOWN" || input.mergeable == "UNKNOWN" {
        push(&mut waits, "merge-state-unknown");
        t.mergeability = Some(at_sha("mergeability", sha));
        measured = true;
    }
    for u in &input.unavailable {
        let Some((id, target)) = read_wait(&u.read) else {
            continue;
        };
        if u.refused {
            push(&mut fixable, READ_REFUSED);
            continue;
        }
        push(&mut waits, id);
        if u.rate_limited {
            t.reset(u.reset_epoch);
        } else {
            t.read(target);
        }
    }

    // Reported ids: the fixed wait list on the whole id; everything else fixable.
    let mut mead = false;
    for f in &input.findings {
        if f == MEAD {
            mead = true;
        } else if WAIT_IDS.contains(&f.as_str()) {
            push(&mut waits, f);
            match f.as_str() {
                "ci-pending" => {
                    t.ci.get_or_insert_with(|| at_sha("ci", sha));
                }
                "approval-facts-unreadable" => t.read("read@reviews"),
                "expected-checks-unreadable" => t.read("read@checks"),
                _ => t.read("read@pr-head"),
            }
        } else {
            push(&mut fixable, f);
        }
    }

    // measured-evidence-disagrees: dropped only when every token is a wait.
    let tokens: Vec<&str> = input
        .measured_downgrade
        .iter()
        .map(String::as_str)
        .collect();
    let neutral = tokens.contains(&"approval_status=PENDING");
    let rest: Vec<&str> = tokens
        .into_iter()
        .filter(|t| *t != "approval_status=PENDING")
        .collect();
    let failed_read = input
        .findings
        .iter()
        .any(|f| f == "approval-facts-unreadable");
    let mut all_wait = !rest.is_empty();
    for tok in &rest {
        match *tok {
            "ci_status=PENDING" => {
                push(&mut waits, "ci-pending");
                t.ci.get_or_insert_with(|| at_sha("ci", sha));
            }
            "ci_status=UNREADABLE" => {
                push(&mut waits, "expected-checks-unreadable");
                t.read("read@checks");
            }
            "conflict=unknown" => {
                push(&mut waits, "merge-state-unknown");
                t.mergeability
                    .get_or_insert_with(|| at_sha("mergeability", sha));
            }
            "unresolved_threads=unreadable" => {
                push(&mut waits, "threads-unavailable");
                t.read("read@threads");
            }
            "approval_status=UNREADABLE" if failed_read => t.read("read@reviews"),
            _ => all_wait = false,
        }
    }
    // Only the neutral PENDING approval left: it is explained by a measured
    // CI or mergeability wait, which hides the approval; nothing else is.
    if rest.is_empty() && neutral && measured {
        all_wait = true;
    }
    if mead && !all_wait {
        push(&mut fixable, MEAD);
    }

    let mut wait_target = if waits.is_empty() {
        String::new()
    } else {
        t.first()
    };
    // The recompute replaces only the mergeability target: a rate-limit reset
    // or pending CI ranks above it and stays the target.
    if let Some(r) = &input.recompute
        && t.mergeability.is_some()
        && t.reset.is_none()
        && t.ci.is_none()
    {
        wait_target = recompute_target(
            &r.action,
            &r.result,
            sha,
            r.pushed_sha.as_deref(),
            r.rate_limited,
            r.reset_epoch,
        );
    }
    Output {
        round_waiting: !waits.is_empty() && fixable.is_empty(),
        waits,
        fixable,
        wait_target,
    }
}

/// The wait target after an R7 merge-state recompute.
pub fn recompute_target(
    action: &str,
    result: &str,
    head_sha: &str,
    pushed_sha: Option<&str>,
    rate_limited: bool,
    reset_epoch: Option<i64>,
) -> String {
    match (action, result) {
        ("merge-push", "ok") => at_sha("mergeability", pushed_sha.unwrap_or(head_sha)),
        ("update-branch" | "noop-up-to-date", "ok") | ("none", "first-read") => {
            at_sha("mergeability", head_sha)
        }
        (_, "unavailable") if rate_limited => match reset_epoch {
            Some(e) => format!("ratelimit-reset@{e}"),
            None => "ratelimit-reset@unknown".into(),
        },
        (_, "unavailable") => "read@pr".into(),
        _ => "read@pr-head".into(),
    }
}

// ── IV3: the previous round's `.wait` sidecar ─────────────────────────────

/// Largest sidecar read; a larger file is invalid, never truncated.
const PRIOR_WAIT_MAX: u64 = 4096;

/// The previous round's `.wait` sidecar, as merge-evidence step-01 writes it
/// on an `UNKNOWN` merge state.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PriorWait {
    pub wait: String,
    pub head_sha: String,
    pub merge_state: String,
}

/// Reads the sidecar strictly. `None` unless `path` is a regular file owned
/// by this user (opened without following a symlink and without blocking on
/// a FIFO) of at most 4 KiB holding a JSON object with exactly the string
/// keys `wait`, `head_sha` and `merge_state`, and a `head_sha` of 40
/// lowercase hex characters. The file is untrusted; every check is made on
/// the open handle, so a swap after the checks changes nothing.
pub fn read_prior_wait(path: &std::path::Path) -> Option<PriorWait> {
    let f = open_prior_wait(path)?;
    let meta = f.metadata().ok()?;
    if !meta.file_type().is_file() || meta.len() > PRIOR_WAIT_MAX || !owned_by_me(&meta) {
        return None;
    }
    let mut raw = Vec::new();
    f.take(PRIOR_WAIT_MAX + 1).read_to_end(&mut raw).ok()?;
    if raw.len() as u64 > PRIOR_WAIT_MAX {
        return None;
    }
    let w: PriorWait = serde_json::from_slice(&raw).ok()?;
    valid_sha(&w.head_sha).then_some(w)
}

#[cfg(unix)]
fn open_prior_wait(path: &std::path::Path) -> Option<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    // O_NOFOLLOW refuses a symlink at the path; O_NONBLOCK keeps a FIFO from
    // blocking the open. A regular file reads the same either way.
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .ok()
}

#[cfg(not(unix))]
fn open_prior_wait(path: &std::path::Path) -> Option<std::fs::File> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    meta.file_type()
        .is_file()
        .then(|| std::fs::File::open(path).ok())
        .flatten()
}

#[cfg(unix)]
fn owned_by_me(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    // SAFETY: geteuid has no preconditions and cannot fail.
    meta.uid() == unsafe { libc::geteuid() }
}

#[cfg(not(unix))]
fn owned_by_me(_meta: &std::fs::Metadata) -> bool {
    true
}

/// Whether update-branch may write: only when the previous round's sidecar
/// recorded `UNKNOWN` at `expected_head_sha`. Otherwise one of four fixed
/// reasons, and the caller treats the round as a first read.
pub fn prior_wait_gate(path: &str, expected_head_sha: &str) -> Result<(), &'static str> {
    if path.is_empty() {
        return Err("prior wait: none");
    }
    let p = std::path::Path::new(path);
    if std::fs::symlink_metadata(p).is_err() {
        return Err("prior wait: none");
    }
    let w = read_prior_wait(p).ok_or("prior wait: invalid")?;
    if w.merge_state != "UNKNOWN" {
        return Err("prior wait: not unknown");
    }
    if !valid_sha(expected_head_sha) || w.head_sha != expected_head_sha {
        return Err("prior wait: head differs");
    }
    Ok(())
}

/// `amplihack orch helper autodrive-round-waits`: JSON on stdin, one JSON
/// line out; exit 2 on input that is not the documented shape.
pub fn cli() -> anyhow::Result<()> {
    let mut raw = Vec::new();
    std::io::stdin()
        .take(1024 * 1024 + 1)
        .read_to_end(&mut raw)?;
    let input: Input = match serde_json::from_slice(&raw) {
        Ok(i) if raw.len() <= 1024 * 1024 => i,
        Ok(_) => {
            eprintln!("ERROR: autodrive-round-waits: input over 1 MiB");
            std::process::exit(2)
        }
        Err(e) => {
            eprintln!(
                "ERROR: autodrive-round-waits: input is not the documented JSON object ({e})"
            );
            std::process::exit(2)
        }
    };
    println!("{}", serde_json::to_string(&classify(&input))?);
    Ok(())
}

#[cfg(test)]
#[path = "autodrive_waits_tests.rs"]
mod tests;
