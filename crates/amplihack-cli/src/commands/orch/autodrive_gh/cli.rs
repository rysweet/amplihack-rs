//! `amplihack orch helper autodrive-gh-read` and `autodrive-gh-update-branch`.

use super::{
    EXIT_UNAVAILABLE, EXIT_USAGE, Gh, Read, Target, checks, head, pr, pr_for_branch, reviews,
    update_branch, valid_branch, valid_pr, valid_repo, valid_sha,
};
use clap::{Args, Subcommand};
use serde_json::{Value, json};

#[derive(Args, Debug)]
pub struct ReadArgs {
    #[command(subcommand)]
    read: ReadCommand,
    /// `owner/repo`; default: the working directory's `origin`.
    #[arg(long, global = true)]
    repo: Option<String>,
}

#[derive(Subcommand, Debug)]
enum ReadCommand {
    /// State, draft, head, base, merge state, mergeable, merged time.
    Pr { n: String },
    /// The PR head SHA.
    Head { n: String },
    /// Check buckets at a PR's head, or at a 40-hex commit SHA.
    Checks { target: String },
    /// Review decision, approvals and unresolved threads.
    Reviews { n: String },
    /// The open PR for a branch.
    PrForBranch { branch: String },
}

#[derive(Args, Debug)]
pub struct UpdateBranchArgs {
    n: String,
    #[arg(long)]
    expected_head_sha: String,
    /// The previous round's `.wait` sidecar; empty for round 1. The write is
    /// sent only when it recorded `UNKNOWN` at `--expected-head-sha`.
    #[arg(long)]
    prior_wait: String,
    #[arg(long)]
    repo: Option<String>,
}

fn usage(msg: &str) -> ! {
    eprintln!("ERROR: {msg}; nothing was run");
    std::process::exit(EXIT_USAGE)
}

fn checked_repo(repo: &Option<String>) -> Option<&str> {
    match repo.as_deref() {
        Some(r) if !valid_repo(r) => usage("--repo must be owner/name"),
        r => r,
    }
}

fn pr_arg(n: &str) -> u64 {
    valid_pr(n).unwrap_or_else(|| usage("the PR number must be 1-9999999999"))
}

/// `amplihack orch helper autodrive-gh-read …`: one JSON line; exit 0, 3
/// (unavailable) or 2 (usage).
pub fn read_cli(args: ReadArgs) -> anyhow::Result<()> {
    let repo = checked_repo(&args.repo);
    let dir = std::env::current_dir()?;
    let gh = Gh::new(&dir, repo);
    let (mut head, read) = match &args.read {
        ReadCommand::Pr { n } => {
            let n = pr_arg(n);
            (json!({"read": "pr", "pr": n}), pr(&gh, n))
        }
        ReadCommand::Head { n } => {
            let n = pr_arg(n);
            (json!({"read": "head", "pr": n}), head(&gh, n))
        }
        ReadCommand::Checks { target } => {
            let t = if valid_sha(target) {
                Target::Sha(target.clone())
            } else if let Some(n) = valid_pr(target) {
                Target::Pr(n)
            } else {
                usage("checks takes a PR number or a 40-hex commit SHA")
            };
            let pr = match &t {
                Target::Pr(n) => json!(n),
                Target::Sha(_) => Value::Null,
            };
            (json!({"read": "checks", "pr": pr}), checks(&gh, &t))
        }
        ReadCommand::Reviews { n } => {
            let n = pr_arg(n);
            (json!({"read": "reviews", "pr": n}), reviews(&gh, n))
        }
        ReadCommand::PrForBranch { branch } => {
            if !valid_branch(branch) {
                usage("the branch name fails the branch rules");
            }
            (
                json!({"read": "pr-for-branch", "branch": branch}),
                pr_for_branch(&gh, branch),
            )
        }
    };
    let code = match read {
        Read::Answer { source, fields } => {
            head["source"] = json!(source);
            head.as_object_mut().expect("object").extend(fields);
            0
        }
        Read::Unavailable(u) => {
            head["source"] = Value::Null;
            head["unavailable"] = serde_json::to_value(&u)?;
            eprintln!(
                "WARNING: the read is unavailable on every path: {}",
                u.reason
            );
            EXIT_UNAVAILABLE
        }
    };
    println!("{}", serde_json::to_string(&head)?);
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}

/// `amplihack orch helper autodrive-gh-update-branch <n> --expected-head-sha
/// <sha> --prior-wait <path>`. Without a qualifying previous-round sidecar it
/// answers `first-read`, makes no `gh` call and exits 0.
pub fn update_branch_cli(args: UpdateBranchArgs) -> anyhow::Result<()> {
    let repo = checked_repo(&args.repo);
    let n = pr_arg(&args.n);
    if !valid_sha(&args.expected_head_sha) {
        usage("--expected-head-sha must be 40 lowercase hex characters");
    }
    if let Err(reason) = crate::commands::orch::autodrive_waits::prior_wait_gate(
        &args.prior_wait,
        &args.expected_head_sha,
    ) {
        let v = json!({"pr": n, "action": "none", "result": "first-read",
                       "expected_head_sha": args.expected_head_sha, "reason": reason});
        eprintln!("INFO: update-branch not sent: {reason}; this round is a first read");
        println!("{}", serde_json::to_string(&v)?);
        return Ok(());
    }
    let dir = std::env::current_dir()?;
    let v = update_branch(&Gh::new(&dir, repo), n, &args.expected_head_sha);
    println!("{}", serde_json::to_string(&v)?);
    Ok(())
}
