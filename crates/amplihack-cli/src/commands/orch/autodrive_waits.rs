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
    "base-unavailable",
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
/// Finding id for an update-branch GitHub refused (a 403, or a 422 because the
/// head moved). Waiting will not clear a forbidden write, and a moved head is
/// a new head next round, so neither is a wait.
const UPDATE_BRANCH_REFUSED: &str = "update-branch-refused";

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

/// The wait id and `read@…` target for a helper read name; an unknown name
/// is not a wait (the caller records it as fixable).
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
            // An unknown read name is a classing error: fixable, never dropped.
            push(&mut fixable, "gh-read-unknown");
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
                "base-unavailable" => t.read("read@pr"),
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
    if input
        .recompute
        .as_ref()
        .is_some_and(|r| r.result == "refused")
    {
        push(&mut fixable, UPDATE_BRANCH_REFUSED);
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

// ── The merge round's own classing (`orch helper autodrive round`) ────────

/// Finding id when a measured input the classifier needs cannot be parsed.
const FACTS_UNPARSED: &str = "wait-facts-unparsed";

/// An upper-case signal token: the text before any `:`, `A`–`Z` and `_` only.
fn token(v: &serde_json::Value) -> String {
    v.as_str()
        .unwrap_or_default()
        .split(':')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase()
        .chars()
        .filter(|c| c.is_ascii_uppercase() || *c == '_')
        .collect()
}

/// The classifier input for one merge round, built from the criteria the
/// assessment left blocked and the round's measured facts: `platform_facts`
/// (merge state, approval, threads, unavailable reads), `ci_evidence` (the CI
/// signal) and `merge_sync` (the recompute). A blocked criterion is dropped
/// only when a measured wait explains it; every other one stays a fixable
/// id, and a measured input that does not parse is a fixable id too.
pub fn round_input(
    head_sha: &str,
    blocked: &[&str],
    facts: &serde_json::Value,
    ci: &serde_json::Value,
    sync: &serde_json::Value,
) -> Input {
    let mut findings = Vec::new();
    let mut measured_downgrade = Vec::new();
    let ci_signal = match token(&ci["ci_signal"]) {
        s if s.is_empty() => token(&ci["ci_status"]),
        s => s,
    };
    let merge_state = token(&facts["merge_state"]);
    let mergeable = token(&facts["mergeable"]);
    let unavailable = match &facts["unavailable"] {
        serde_json::Value::Null => Vec::new(),
        v => serde_json::from_value::<Vec<UnavailableRead>>(v.clone()).unwrap_or_else(|_| {
            push(&mut findings, FACTS_UNPARSED);
            Vec::new()
        }),
    };
    // The helper's own line (with `pr`, `expected_head_sha`, `reason`), as an
    // object or as JSON text: only the fields the classifier reads are kept.
    let recompute = match &sync["recompute"] {
        serde_json::Value::Null => None,
        v => {
            let v = match v {
                serde_json::Value::String(t) => serde_json::from_str(t).unwrap_or_default(),
                v => v.clone(),
            };
            let mut kept = serde_json::Map::new();
            for k in [
                "action",
                "result",
                "pushed_sha",
                "rate_limited",
                "reset_epoch",
            ] {
                if let Some(x) = v.get(k).filter(|x| !x.is_null()) {
                    kept.insert(k.to_string(), x.clone());
                }
            }
            serde_json::from_value::<Recompute>(serde_json::Value::Object(kept))
                .map_err(|_| push(&mut findings, FACTS_UNPARSED))
                .ok()
        }
    };
    // The ids the merge-evidence prompts name for a refused read and an
    // unavailable base; any other agent text is left to the criteria.
    for list in [&ci["blockers"], &sync["blockers"]] {
        for b in list.as_array().into_iter().flatten() {
            let id = b.as_str().or_else(|| b["id"].as_str()).unwrap_or_default();
            if matches!(id, "gh-read-refused" | "base-unavailable") {
                push(&mut findings, id);
            }
        }
    }
    let ci_failed = ci["ci_failing"].as_u64().is_some_and(|n| n > 0)
        || ci["ci_failing"]
            .as_str()
            .and_then(|s| s.parse::<u64>().ok())
            .is_some_and(|n| n > 0)
        || ci_signal == "RED";
    let ci_wait = matches!(ci_signal.as_str(), "PENDING" | "QUEUED") && !ci_failed;
    // A known conflict is never explained by an UNKNOWN beside it.
    let merge_wait = (merge_state == "UNKNOWN" || mergeable == "UNKNOWN")
        && merge_state != "DIRTY"
        && mergeable != "CONFLICTING";
    let read_wait = |r: &str| unavailable.iter().any(|u| u.read == r && !u.refused);
    let approval_source = facts["approval_source"].as_str().unwrap_or_default();
    let approval_status = token(&facts["approval_status"]);
    let threads = facts["unresolved_threads"].as_str().unwrap_or_default();
    for c in blocked {
        let explained = match *c {
            "checks" => {
                // The facts' own checks read counts only when CI was not measured:
                // a MISSING or GREEN signal is a measurement.
                ci_wait
                    || (!ci_failed
                        && (ci_signal == "UNREADABLE"
                            || (ci_signal.is_empty() && read_wait("checks"))))
            }
            "conflicts" => {
                merge_wait
                    || (read_wait("pr") && merge_state != "DIRTY" && mergeable != "CONFLICTING")
            }
            "approvals" => {
                approval_source == "facts-unreadable"
                    || read_wait("reviews")
                    || (approval_status == "PENDING" && (ci_wait || merge_wait))
            }
            "conversations" => matches!(threads, "unavailable" | "unreadable"),
            _ => false,
        };
        if !explained {
            push(&mut findings, c);
        } else if *c == "approvals" && approval_source == "facts-unreadable" {
            push(&mut findings, "approval-facts-unreadable");
        } else if *c == "conversations" {
            // The classifier turns this token into the threads wait.
            measured_downgrade.push("unresolved_threads=unreadable".to_string());
        }
    }
    Input {
        head_sha: head_sha.to_string(),
        ci_signal,
        merge_state,
        mergeable,
        unavailable,
        findings,
        measured_downgrade,
        recompute,
    }
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
