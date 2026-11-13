use super::{
    authorization::InvocationAuthorization, outcome::reduce_outcome, records::*,
    state::EvidenceStore, trusted, validation::*,
};
use anyhow::{Context, Result, ensure};
use clap::Subcommand;
use std::path::{Path, PathBuf};

/// Local, portable operations. None invokes hosting APIs or performs a merge.
#[derive(Debug, Subcommand)]
pub enum Operations {
    /// Seal the loop's accepted round plus evaluator DONE for this invocation.
    CompleteLoop {
        #[arg(long)]
        invocation: PathBuf,
        #[arg(long)]
        directory: PathBuf,
        #[arg(long)]
        round: String,
    },
    /// Accept refreshed readiness before the action step runs (stdin CandidateRecord).
    Assess {
        #[arg(long)]
        invocation: PathBuf,
        #[arg(long)]
        directory: PathBuf,
        #[arg(long)]
        repo: PathBuf,
    },
    /// Accept an agent round and write the loop's existing signal record (stdin CandidateRecord).
    Round {
        #[arg(long)]
        invocation: PathBuf,
        #[arg(long)]
        directory: PathBuf,
        #[arg(long)]
        repo: PathBuf,
        #[arg(long)]
        record: PathBuf,
    },
    /// Reduce a final refreshed candidate and observation envelope from stdin.
    Reduce {
        #[arg(long)]
        accepted: PathBuf,
        #[arg(long)]
        invocation: PathBuf,
        #[arg(long)]
        directory: PathBuf,
        #[arg(long)]
        repo: PathBuf,
    },
    /// Create or validate private state; unsafe historical directories are rejected.
    State {
        #[arg(long)]
        directory: PathBuf,
    },
    /// Validate and snapshot one round's evidence against a previously observed identity.
    Accept {
        #[arg(long)]
        invocation: PathBuf,
        #[arg(long)]
        directory: PathBuf,
        #[arg(long)]
        binding: String,
        #[arg(long)]
        candidate: String,
    },
    /// Reduce observations from private state into an outcome, preserving incomplete causes.
    Outcome {
        #[arg(long)]
        invocation: PathBuf,
        #[arg(long)]
        directory: PathBuf,
        #[arg(long)]
        binding: String,
        #[arg(long)]
        candidate: String,
        #[arg(long)]
        finalization: String,
        #[arg(long)]
        completion: String,
    },
    /// Verify the local linked worktree and head using sanitized Git arguments.
    Worktree {
        #[arg(long)]
        repo: PathBuf,
        #[arg(long)]
        head: String,
        #[arg(long)]
        branch: String,
    },
}
pub fn run(op: Operations) -> Result<()> {
    match op {
        Operations::CompleteLoop {
            invocation,
            directory,
            round,
        } => {
            let auth = InvocationAuthorization::read_private(&invocation)?;
            super::completion::record(&auth, &directory, &round)?;
        }
        Operations::Assess {
            invocation,
            directory,
            repo,
        } => {
            let auth = InvocationAuthorization::read_private(&invocation)?;
            // Authorisation and private state come before stdin is read.
            EvidenceStore::create(&directory)?;
            let out = assess(&auth, &directory, &repo, &stdin_bytes()?)?;
            println!("{out}");
        }
        Operations::Round {
            invocation,
            directory,
            repo,
            record,
        } => {
            let auth = InvocationAuthorization::read_private(&invocation)?;
            EvidenceStore::create(&directory)?;
            round_record_path(&directory, &record)?;
            let out = round(&auth, &directory, &repo, &record, &stdin_bytes()?)?;
            println!("{out}");
        }
        Operations::Reduce {
            accepted: accepted_path,
            invocation,
            directory,
            repo,
        } => {
            let auth = InvocationAuthorization::read_private(&invocation)?;
            let store = EvidenceStore::create(&directory)?;
            let envelope: Envelope = serde_json::from_slice(&stdin_bytes()?)
                .context("invalid finalization evidence envelope")?;
            let result =
                validate_reduction(&auth, &store, &directory, &repo, &accepted_path, &envelope)?;
            let snapshot = store.snapshot(&serde_json::to_vec(&envelope.candidate)?)?;
            let observation_snapshot =
                store.snapshot(&serde_json::to_vec(&envelope.finalization)?)?;
            let completion_snapshot = store.snapshot(&serde_json::to_vec(&envelope.completion)?)?;
            let followups =
                super::followups::read(&store, &directory, envelope.candidate.binding.pr_number)?;
            let outcome_snapshot = store.snapshot(&serde_json::to_vec(&result)?)?;
            println!(
                "{}",
                serde_json::json!({"outcome":result,"accepted_snapshot":accepted_path,"repo":repo.canonicalize()?,"evidence_snapshot":snapshot,"observation_snapshot":observation_snapshot,"completion_snapshot":completion_snapshot,"outcome_snapshot":outcome_snapshot,"followups":followups})
            );
        }
        Operations::State { directory } => {
            EvidenceStore::create(&directory)?;
            println!("{}", serde_json::json!({"state_dir":directory}));
        }
        Operations::Accept {
            invocation,
            directory,
            binding,
            candidate,
        } => {
            let auth = InvocationAuthorization::read_private(&invocation)?;
            let store = EvidenceStore::create(&directory)?;
            let binding: Binding = serde_json::from_slice(&store.read_private(&binding)?)?;
            let accepted = store.accept_candidate(&candidate, &auth, &binding)?;
            println!(
                "{}",
                serde_json::json!({"accepted":accepted.path(), "readiness":accepted.record.record().readiness})
            );
        }
        Operations::Outcome {
            invocation,
            directory,
            binding,
            candidate,
            finalization,
            completion,
        } => {
            let auth = InvocationAuthorization::read_private(&invocation)?;
            let store = EvidenceStore::create(&directory)?;
            let binding: Binding = serde_json::from_slice(&store.read_private(&binding)?)?;
            let accepted = store.accept_candidate(&candidate, &auth, &binding)?;
            let f: FinalizationObservation =
                serde_json::from_slice(&store.read_private(&finalization)?)?;
            let done: LoopCompletion = serde_json::from_slice(&store.read_private(&completion)?)?;
            let out = reduce_outcome(&auth, &accepted.record, &f, &done)?;
            let path = store.snapshot(&serde_json::to_vec(&out)?)?;
            println!("{}", serde_json::json!({"outcome":out,"snapshot":path}));
        }
        Operations::Worktree { repo, head, branch } => {
            ensure!(
                [40, 64].contains(&head.len()) && head.bytes().all(|b| b.is_ascii_hexdigit()),
                "invalid expected head"
            );
            trusted::git(&repo, &["check-ref-format", "--branch", &branch])?;
            ensure!(
                trusted::git(&repo, &["rev-parse", "--verify", "HEAD"])? == head,
                "worktree is not at observed PR head"
            );
            ensure!(
                trusted::git(&repo, &["symbolic-ref", "--short", "HEAD"])? == branch,
                "worktree branch differs from PR head branch"
            );
            let common = trusted::git(
                &repo,
                &["rev-parse", "--path-format=absolute", "--git-common-dir"],
            )?;
            ensure!(
                std::path::Path::new(&common)
                    .join("info/grafts")
                    .symlink_metadata()
                    .is_err(),
                "Git grafts cannot establish PR identity"
            );
            let listing = trusted::git(&repo, &["worktree", "list", "--porcelain"])?;
            let canonical = repo.canonicalize().context("canonicalize PR worktree")?;
            ensure!(
                listing.split("\n\n").skip(1).any(|entry| entry
                    .lines()
                    .any(|l| l == format!("worktree {}", canonical.display()))),
                "PR must use a linked worktree; main checkout cannot be used"
            );
            println!(
                "{}",
                serde_json::json!({"worktree": canonical,"head_sha": head,"branch":branch,"verified":true})
            );
        }
    }
    Ok(())
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Envelope {
    pub(crate) candidate: CandidateRecord,
    pub(crate) finalization: FinalizationObservation,
    pub(crate) completion: LoopCompletion,
}

/// The CLI reducer and runner reporting share this complete acceptance boundary.
/// Neither an agent envelope nor a child success flag is accepted readiness.
pub(crate) fn validate_reduction(
    auth: &InvocationAuthorization,
    store: &EvidenceStore,
    directory: &std::path::Path,
    repo: &std::path::Path,
    accepted_path: &std::path::Path,
    envelope: &Envelope,
) -> Result<Outcome> {
    ensure!(
        accepted_path
            .parent()
            .context("accepted candidate has no parent")?
            .canonicalize()?
            == directory.canonicalize()?,
        "accepted candidate lies outside state"
    );
    let accepted_name = accepted_path
        .file_name()
        .and_then(|s| s.to_str())
        .context("invalid accepted candidate name")?;
    let before_action: CandidateRecord =
        serde_json::from_slice(&store.read_private(accepted_name)?)?;
    ensure!(
        before_action == envelope.candidate,
        "finalization changed accepted readiness after the action; refresh and assess again"
    );

    local_binding(repo, &envelope.candidate.binding)?;
    super::review::verify(repo, &envelope.candidate)?;
    let accepted = validate_candidate(auth, &envelope.candidate.binding, &envelope.candidate)?;
    store.snapshot_references(accepted.record())?;
    if envelope.candidate.readiness == Readiness::MergeReady {
        super::completion::verify(auth, directory, &envelope.candidate)?;
    }
    reduce_outcome(
        auth,
        &accepted,
        &envelope.finalization,
        &envelope.completion,
    )
}

fn stdin_bytes() -> Result<Vec<u8>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(MAX_RECORD_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= MAX_RECORD_BYTES,
        "evidence exceeds size limit"
    );
    Ok(bytes)
}
/// Final acceptance keeps the strict messages: stale evidence is an error here.
fn local_binding(repo: &Path, b: &Binding) -> Result<()> {
    let blockers = binding_drift(repo, b)?;
    let stale = |name: &str| blockers.iter().any(|x| x == name);
    ensure!(
        !stale(STALE_HEAD),
        "evidence head differs from current worktree; refresh required"
    );
    ensure!(
        !stale(STALE_BASE),
        "base changed or not fetched; refresh required"
    );
    ensure!(!stale(STALE_MERGE_BASE), "merge-base differs from evidence");
    // Fail closed: an unreadable HEAD, or any blocker added later, rejects.
    ensure!(
        blockers.is_empty(),
        "worktree HEAD unreadable or evidence not current; refresh required"
    );
    Ok(())
}

const STALE_HEAD: &str = "stale-evidence-head";
const STALE_BASE: &str = "stale-evidence-base";
const STALE_MERGE_BASE: &str = "stale-evidence-merge-base";
const HEAD_UNREADABLE: &str = "current-head-unreadable";

/// Placeholder for a worktree HEAD that Git could not read. It can never
/// equal a bound SHA, because `validate_binding` requires 40 or 64 hex digits,
/// and `revision_or_empty` prints it as `""`.
const UNKNOWN_HEAD: &str = "unknown";

/// #2556: read the worktree HEAD. A HEAD that cannot be read (for example a
/// repository with no commits) is a recoverable fault: it adds the fixed
/// blocker `current-head-unreadable` and returns `UNKNOWN_HEAD`. Git's message
/// goes only to debug logging, never to the output. Only this read is
/// tolerant; every other Git failure stays an error.
fn head_or_unknown(repo: &Path, blockers: &mut Vec<String>) -> String {
    match trusted::git(repo, &["rev-parse", "--verify", "HEAD"]) {
        Ok(head) => head,
        Err(err) => {
            tracing::debug!(error = %err, "autodrive: worktree HEAD is unreadable");
            blockers.push(HEAD_UNREADABLE.to_string());
            UNKNOWN_HEAD.to_string()
        }
    }
}

struct Drift {
    blockers: Vec<String>,
    current_head: String,
    current_base: String,
}

/// #2556: name each way the evidence binding has drifted from the local
/// worktree (`stale-evidence-head`, `-base`, `-merge-base`), or that the
/// worktree HEAD cannot be read (`current-head-unreadable`). Drift is a
/// refresh outcome; an invalid binding, a bad ref format, a branch mismatch,
/// grafts and every other Git failure stay hard errors.
pub fn binding_drift(repo: &Path, b: &Binding) -> Result<Vec<String>> {
    Ok(drift(repo, b)?.blockers)
}

fn drift(repo: &Path, b: &Binding) -> Result<Drift> {
    // Validate every record value before any of it reaches a Git argument.
    validate_binding(b)?;
    trusted::git(repo, &["check-ref-format", "--branch", &b.base_branch])?;
    trusted::git(repo, &["check-ref-format", "--branch", &b.head_branch])?;
    ensure!(
        trusted::git(repo, &["symbolic-ref", "--short", "HEAD"])? == b.head_branch,
        "PR head branch differs from worktree"
    );
    let common = trusted::git(
        repo,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    ensure!(
        !Path::new(&common).join("info/grafts").try_exists()?,
        "Git grafts cannot establish evidence"
    );
    let mut blockers = Vec::new();
    let current_head = head_or_unknown(repo, &mut blockers);
    if current_head == UNKNOWN_HEAD {
        // Nothing can be compared with a HEAD that cannot be read.
        return Ok(Drift {
            blockers,
            current_head,
            current_base: String::new(),
        });
    }
    let current_base = trusted::git(
        repo,
        &[
            "rev-parse",
            "--verify",
            &format!("refs/remotes/origin/{}^{{commit}}", b.base_branch),
        ],
    )?;
    if current_head != b.head_sha {
        blockers.push(STALE_HEAD.to_string());
    }
    if current_base != b.base_sha {
        blockers.push(STALE_BASE.to_string());
    }
    // The merge-base is only meaningful once head and base both match.
    if blockers.is_empty()
        && trusted::git(repo, &["merge-base", &b.head_sha, &b.base_sha])? != b.merge_base_sha
    {
        blockers.push(STALE_MERGE_BASE.to_string());
    }
    Ok(Drift {
        blockers,
        current_head,
        current_base,
    })
}

/// Why a candidate could not be assessed: a curable writer fault or stale
/// evidence. Integrity faults are errors and never reach this type.
struct NotAccepted {
    blockers: Vec<String>,
    evidence_head: String,
    round_label: String,
    current_head: String,
    current_base: String,
}

enum Reading {
    Current(Box<CandidateRecord>),
    Refresh(NotAccepted),
}

/// Parse, name unknown values, then compare the binding with the worktree.
/// A binding with unknown fields never reaches Git.
fn read_candidate(repo: &Path, input: &[u8]) -> Result<Reading> {
    ensure!(
        input.len() <= MAX_RECORD_BYTES,
        "evidence exceeds size limit"
    );
    let candidate: CandidateRecord = match serde_json::from_slice(input) {
        Ok(candidate) => candidate,
        Err(err) => {
            // Identity is echoed only from a well-formed JSON value, and only
            // after it passes the SHA and label patterns below.
            let loose: serde_json::Value =
                serde_json::from_slice(input).unwrap_or(serde_json::Value::Null);
            let text = |v: &serde_json::Value| v.as_str().unwrap_or("").to_string();
            let mut blockers = vec![parse_blocker(&err)];
            let current_head = head_or_unknown(repo, &mut blockers);
            return Ok(Reading::Refresh(NotAccepted {
                blockers,
                evidence_head: text(&loose["binding"]["head_sha"]),
                round_label: text(&loose["binding"]["round_id"]),
                current_head,
                current_base: String::new(),
            }));
        }
    };
    let unknown = unknown_fields(&candidate);
    let binding_unknown = unknown
        .iter()
        .any(|b| b.starts_with("unknown-field:binding."));
    let (drift_blockers, current_head, current_base) = if binding_unknown {
        let mut blockers = Vec::new();
        let current_head = head_or_unknown(repo, &mut blockers);
        (blockers, current_head, String::new())
    } else {
        let d = drift(repo, &candidate.binding)?;
        (d.blockers, d.current_head, d.current_base)
    };
    if unknown.is_empty() && drift_blockers.is_empty() {
        return Ok(Reading::Current(Box::new(candidate)));
    }
    Ok(Reading::Refresh(NotAccepted {
        blockers: unknown.into_iter().chain(drift_blockers).collect(),
        evidence_head: candidate.binding.head_sha.clone(),
        round_label: candidate.binding.round_id.clone(),
        current_head,
        current_base,
    }))
}

/// #2556: assess a refreshed readiness candidate. A writer fault or stale
/// evidence gives `status:"refresh_required"` with named blockers; nothing is
/// written and no follow-ups are read. Integrity faults stay errors.
pub fn assess(
    auth: &InvocationAuthorization,
    directory: &Path,
    repo: &Path,
    input: &[u8],
) -> Result<serde_json::Value> {
    let store = EvidenceStore::create(directory)?;
    let candidate = match read_candidate(repo, input)? {
        Reading::Current(candidate) => candidate,
        Reading::Refresh(n) => {
            tracing::info!(
                blockers = ?n.blockers,
                "autodrive assess: evidence not accepted; refresh required"
            );
            return Ok(serde_json::json!({
                "status": "refresh_required",
                "accepted": null,
                "readiness": Readiness::Incomplete,
                "refresh_required": true,
                "evidence_head": revision_or_empty(&n.evidence_head),
                "current_head": revision_or_empty(&n.current_head),
                "current_base": revision_or_empty(&n.current_base),
                "blockers": safe_blockers(&n.blockers),
            }));
        }
    };
    super::review::verify(repo, &candidate)?;
    let accepted = validate_candidate(auth, &candidate.binding, &candidate)?;
    if candidate.readiness == Readiness::MergeReady {
        super::completion::verify(auth, directory, &candidate)?;
    }
    let followups = super::followups::read(&store, directory, candidate.binding.pr_number)?;
    let artifact_snapshots = store.snapshot_references(accepted.record())?;
    let path = store.snapshot(&serde_json::to_vec(accepted.record())?)?;
    Ok(serde_json::json!({
        "status": "accepted",
        "accepted": path,
        "readiness": candidate.readiness,
        "refresh_required": false,
        "binding": candidate.binding,
        "artifact_snapshots": artifact_snapshots,
        "followups": followups,
    }))
}

/// The round record must be a new file directly inside the state directory.
fn round_record_path(directory: &Path, record: &Path) -> Result<()> {
    ensure!(
        record
            .parent()
            .context("round record has no parent")?
            .canonicalize()?
            == directory.canonicalize()?,
        "round record is outside state"
    );
    ensure!(
        record.symlink_metadata().is_err(),
        "round record {} already exists",
        record.display()
    );
    Ok(())
}

/// #2556: accept an agent round and write the loop's signal record. A writer
/// fault or stale evidence writes a NOT_MERGE_READY record whose blockers are
/// also written to `<record>.findings`, and returns it (exit 0).
pub fn round(
    auth: &InvocationAuthorization,
    directory: &Path,
    repo: &Path,
    record: &Path,
    input: &[u8],
) -> Result<serde_json::Value> {
    let store = EvidenceStore::create(directory)?;
    round_record_path(directory, record)?;
    let (result, findings) = match read_candidate(repo, input)? {
        Reading::Refresh(n) => {
            let blockers = safe_blockers(&n.blockers);
            tracing::info!(
                blockers = ?blockers,
                "autodrive round: evidence not accepted; NOT_MERGE_READY"
            );
            let findings = lines(blockers.iter());
            let result = serde_json::json!({
                "merge_ready_verdict": "NOT_MERGE_READY",
                "blocker_count": blockers.len(),
                "blockers": blockers,
                "head_sha": revision_or_empty(&n.evidence_head),
                "round_label": label_or_empty(&n.round_label),
                "accepted_evidence": null,
                "test_signal": "BLOCKED",
                "ci_signal": "UNVERIFIED",
            });
            (result, findings)
        }
        Reading::Current(candidate) => {
            super::review::verify(repo, &candidate)?;
            let accepted = validate_candidate(auth, &candidate.binding, &candidate)?;
            store.snapshot_references(accepted.record())?;
            let evidence = store.snapshot(&serde_json::to_vec(accepted.record())?)?;
            let ready = accepted.record().readiness == Readiness::MergeReady;
            let blockers = safe_blockers(&candidate.blockers);
            let result = serde_json::json!({
                "merge_ready_verdict": if ready {"MERGE_READY"} else {"NOT_MERGE_READY"},
                "blocker_count": blockers.len(),
                "blockers": blockers,
                "head_sha": revision_or_empty(&candidate.binding.head_sha),
                "round_label": label_or_empty(&candidate.binding.round_id),
                "accepted_evidence": evidence,
                "test_signal": if ready {"PASS"} else {"BLOCKED"},
                "ci_signal": if ready {"GREEN"} else {"UNVERIFIED"},
            });
            let blocked = candidate.criteria.iter().filter(|c| !c.blockers.is_empty());
            (result, lines(blocked.map(|c| &c.id)))
        }
    };
    write_new_private(&record.with_extension("json.findings"), findings.as_bytes())?;
    write_new_private(record, &serde_json::to_vec(&result)?)?;
    Ok(result)
}

/// One item per line, each ending in a newline.
fn lines<'a>(items: impl Iterator<Item = &'a String>) -> String {
    items.map(|item| format!("{item}\n")).collect()
}

/// The parse fault is named from serde's category and position only; its
/// message can quote record text, so it is never used.
fn parse_blocker(err: &serde_json::Error) -> String {
    use serde_json::error::Category;
    let category = match err.classify() {
        Category::Io => "io",
        Category::Syntax => "syntax",
        Category::Data => "data",
        Category::Eof => "eof",
    };
    format!(
        "evidence-parse-error:{category} at line {} column {}",
        err.line(),
        err.column()
    )
}

const MAX_BLOCKER_BYTES: usize = 256;

/// Blockers are printed and written to files: no control characters and at
/// most 256 bytes each.
fn safe_blockers(blockers: &[String]) -> Vec<String> {
    blockers
        .iter()
        .map(|b| {
            let mut out = String::new();
            for c in b.chars().filter(|c| !c.is_control()) {
                if out.len() + c.len_utf8() > MAX_BLOCKER_BYTES {
                    break;
                }
                out.push(c);
            }
            out
        })
        .collect()
}

fn revision_or_empty(value: &str) -> &str {
    if [40, 64].contains(&value.len()) && value.bytes().all(|b| b.is_ascii_hexdigit()) {
        value
    } else {
        ""
    }
}

fn label_or_empty(value: &str) -> &str {
    if (1..=128).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    {
        value
    } else {
        ""
    }
}

fn write_new_private(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("create new loop record {}", path.display()))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
