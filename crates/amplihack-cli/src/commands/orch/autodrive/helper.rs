use super::{
    authorization::InvocationAuthorization, outcome::reduce_outcome, records::*,
    state::EvidenceStore, trusted, validation::*,
};
use anyhow::{Context, Result, ensure};
use clap::Subcommand;
use std::path::PathBuf;

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
            let store = EvidenceStore::create(&directory)?;
            let candidate: CandidateRecord = serde_json::from_slice(&stdin_bytes()?)
                .context("invalid refreshed readiness candidate")?;
            local_binding(&repo, &candidate.binding)?;
            super::review::verify(&repo, &candidate)?;
            let accepted = validate_candidate(&auth, &candidate.binding, &candidate)?;
            if candidate.readiness == Readiness::MergeReady {
                super::completion::verify(&auth, &directory, &candidate)?;
            }
            let followups =
                super::followups::read(&store, &directory, candidate.binding.pr_number)?;
            let artifact_snapshots = store.snapshot_references(accepted.record())?;
            let path = store.snapshot(&serde_json::to_vec(accepted.record())?)?;
            println!(
                "{}",
                serde_json::json!({"accepted":path,"readiness":candidate.readiness,"binding":candidate.binding,"artifact_snapshots":artifact_snapshots,"followups":followups})
            );
        }
        Operations::Round {
            invocation,
            directory,
            repo,
            record,
        } => {
            let auth = InvocationAuthorization::read_private(&invocation)?;
            let store = EvidenceStore::create(&directory)?;
            ensure!(
                record
                    .parent()
                    .context("round record has no parent")?
                    .canonicalize()?
                    == directory.canonicalize()?,
                "round record is outside state"
            );
            let candidate: CandidateRecord =
                serde_json::from_slice(&stdin_bytes()?).context("invalid agent round evidence")?;
            local_binding(&repo, &candidate.binding)?;
            super::review::verify(&repo, &candidate)?;
            let accepted = validate_candidate(&auth, &candidate.binding, &candidate)?;
            store.snapshot_references(accepted.record())?;
            let evidence = store.snapshot(&serde_json::to_vec(accepted.record())?)?;
            let ready = accepted.record().readiness == Readiness::MergeReady;
            let result = serde_json::json!({"merge_ready_verdict":if ready {"MERGE_READY"} else {"NOT_MERGE_READY"}, "blocker_count":candidate.blockers.len(), "head_sha":candidate.binding.head_sha, "round_label":candidate.binding.round_id, "accepted_evidence":evidence, "test_signal":if ready {"PASS"} else {"BLOCKED"}, "ci_signal":if ready {"GREEN"} else {"UNVERIFIED"}});
            let mut findings = String::new();
            for c in &candidate.criteria {
                if !c.blockers.is_empty() {
                    findings.push_str(&c.id);
                    findings.push('\n');
                }
            }
            write_new_private(&record.with_extension("json.findings"), findings.as_bytes())?;
            write_new_private(&record, &serde_json::to_vec(&result)?)?;
            println!("{result}");
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

            local_binding(&repo, &envelope.candidate.binding)?;
            super::review::verify(&repo, &envelope.candidate)?;
            let accepted =
                validate_candidate(&auth, &envelope.candidate.binding, &envelope.candidate)?;
            store.snapshot_references(accepted.record())?;
            if envelope.candidate.readiness == Readiness::MergeReady {
                super::completion::verify(&auth, &directory, &envelope.candidate)?;
            }
            let result = reduce_outcome(
                &auth,
                &accepted,
                &envelope.finalization,
                &envelope.completion,
            )?;
            let snapshot = store.snapshot(&serde_json::to_vec(&envelope.candidate)?)?;
            let observation_snapshot =
                store.snapshot(&serde_json::to_vec(&envelope.finalization)?)?;
            let completion_snapshot = store.snapshot(&serde_json::to_vec(&envelope.completion)?)?;
            let followups =
                super::followups::read(&store, &directory, envelope.candidate.binding.pr_number)?;
            let outcome_snapshot = store.snapshot(&serde_json::to_vec(&result)?)?;
            println!(
                "{}",
                serde_json::json!({"outcome":result,"evidence_snapshot":snapshot,"observation_snapshot":observation_snapshot,"completion_snapshot":completion_snapshot,"outcome_snapshot":outcome_snapshot,"followups":followups})
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
struct Envelope {
    candidate: CandidateRecord,
    finalization: FinalizationObservation,
    completion: LoopCompletion,
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
fn local_binding(repo: &std::path::Path, b: &Binding) -> Result<()> {
    validate_binding(b)?;
    ensure!(
        trusted::git(repo, &["rev-parse", "--verify", "HEAD"])? == b.head_sha,
        "evidence head differs from current worktree; refresh required"
    );
    trusted::git(repo, &["check-ref-format", "--branch", &b.base_branch])?;
    trusted::git(repo, &["check-ref-format", "--branch", &b.head_branch])?;
    ensure!(
        trusted::git(repo, &["symbolic-ref", "--short", "HEAD"])? == b.head_branch,
        "PR head branch differs from worktree"
    );
    ensure!(
        trusted::git(
            repo,
            &[
                "rev-parse",
                "--verify",
                &format!("refs/remotes/origin/{}^{{commit}}", b.base_branch)
            ]
        )? == b.base_sha,
        "base changed or not fetched; refresh required"
    );
    ensure!(
        trusted::git(repo, &["merge-base", &b.head_sha, &b.base_sha])? == b.merge_base_sha,
        "merge-base differs from evidence"
    );
    let common = trusted::git(
        repo,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    ensure!(
        !std::path::Path::new(&common)
            .join("info/grafts")
            .try_exists()?,
        "Git grafts cannot establish evidence"
    );
    Ok(())
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
