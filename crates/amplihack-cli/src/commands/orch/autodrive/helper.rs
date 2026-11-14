use super::{authorization::InvocationAuthorization, state::EvidenceStore, trusted};
use anyhow::{Context, Result, ensure};
use clap::Subcommand;
use std::path::{Path, PathBuf};

/// Local, portable operations. None invokes hosting APIs or performs a merge.
#[derive(Debug, Subcommand)]
pub enum Operations {
    /// Create or validate private state; unsafe historical directories are rejected.
    State {
        #[arg(long)]
        directory: PathBuf,
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
    /// Report whether the captured invocation allows a merge (#2575).
    ///
    /// Prints `{"allows_merge":bool,"run_id":"..."}`, plus `"pr_selected":bool`
    /// when `--pr` is given: false when the operator selected another PR by
    /// number or URL at launch. Any error exits 2 and
    /// prints nothing on standard output; the finalize merge step then reports
    /// `authorization-unreadable`, does not merge and hands the PR off.
    Authorization {
        #[arg(long)]
        invocation: PathBuf,
        #[arg(long)]
        directory: PathBuf,
        /// The PR finalisation would merge.
        #[arg(long)]
        pr: Option<u64>,
    },
}

pub fn run(op: Operations) -> Result<()> {
    match op {
        Operations::State { directory } => {
            EvidenceStore::create(&directory)?;
            println!("{}", serde_json::json!({"state_dir":directory}));
        }
        Operations::Authorization {
            invocation,
            directory,
            pr,
        } => match authorization(&invocation, &directory, pr) {
            Ok(line) => println!("{line}"),
            Err(error) => {
                eprintln!("ERROR: auto-drive authorization unavailable: {error:#}");
                std::process::exit(2);
            }
        },
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

/// Reads the captured private invocation and checks that `directory` is the
/// state directory the runner bound to it. Recipe context cannot grant a
/// merge: only this captured record can.
fn authorization(
    invocation: &Path,
    directory: &Path,
    pr: Option<u64>,
) -> Result<serde_json::Value> {
    let auth = InvocationAuthorization::read_private(invocation)?;
    let bound = auth
        .state_dir()
        .context("the invocation has no runner-bound state directory")?;
    ensure!(
        bound == directory,
        "--directory is not the invocation's runner-bound state directory"
    );
    EvidenceStore::create(directory)?;
    let mut out = serde_json::json!({"allows_merge": auth.allows_merge(), "run_id": auth.run_id()});
    if let Some(pr) = pr {
        out["pr_selected"] = auth.selects_pr(pr).into();
    }
    Ok(out)
}
