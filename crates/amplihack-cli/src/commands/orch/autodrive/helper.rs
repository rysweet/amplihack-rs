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
    /// Prints `{"allows_merge":bool,"no_merge":bool,"dry_run":bool,
    /// "run_id":"..."}`, plus `"pr_selected":bool` when `--pr` is given: false
    /// when the operator selected another PR by number or URL at launch.
    /// `no_merge` and `dry_run` are the captured flags behind `allows_merge`
    /// (#3129). Any error exits 2, prints nothing on standard output and names
    /// the record path or both state folders on standard error; the loop's
    /// merge action then reports `authorization-unreadable` and does not
    /// merge (#2959).
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
    let auth = InvocationAuthorization::read_private(invocation).with_context(|| {
        format!(
            "cannot read the captured invocation record {}",
            invocation.display()
        )
    })?;
    let bound = auth.state_dir().with_context(|| {
        format!(
            "the invocation record {} has no runner-bound state directory",
            invocation.display()
        )
    })?;
    ensure!(
        bound == directory,
        "--directory {} is not the invocation's runner-bound state directory {}",
        directory.display(),
        bound.display()
    );
    EvidenceStore::create(directory)?;
    // #3129: `no_merge` and `dry_run` are the captured flags behind
    // `allows_merge`, printed so the loop's reading can say why merging is
    // off. They grant nothing; `allows_merge` stays the merge action's input.
    let mut out = serde_json::json!({
        "allows_merge": auth.allows_merge(),
        "no_merge": auth.no_merge(),
        "dry_run": auth.dry_run(),
        "run_id": auth.run_id(),
    });
    if let Some(pr) = pr {
        out["pr_selected"] = auth.selects_pr(pr).into();
    }
    Ok(out)
}

#[cfg(all(test, unix))]
mod tests_2959 {
    use super::authorization;
    use crate::commands::orch::autodrive::authorization::InvocationAuthorization;
    use std::collections::BTreeMap;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    /// #2959: a folder mismatch is still refused (the check stays strict), and
    /// the error names both folders so the finalize step's evidence says which
    /// state folder the runner bound and which one finalisation was given.
    #[test]
    fn a_state_folder_mismatch_is_refused_and_names_both_folders() {
        let temp = crate::test_support::tempdir_outside_framework_roots();
        let root = temp.path();
        fs::set_permissions(root, fs::Permissions::from_mode(0o700)).unwrap();
        let bound = root.join("bound-state");
        let other = root.join("other-state");
        let mut auth =
            InvocationAuthorization::capture("auto-drive-to-merge", &BTreeMap::new()).unwrap();
        auth.bind_policy(format!("trusted-launch-bundle:sha256:{}", "a".repeat(64)));
        auth.bind_state_dir(bound.clone());
        let invocation = root.join("invocation.json");
        fs::write(&invocation, serde_json::to_string(&auth).unwrap()).unwrap();
        fs::set_permissions(&invocation, fs::Permissions::from_mode(0o600)).unwrap();

        let error = authorization(&invocation, &other, None).expect_err("mismatch is refused");
        let text = format!("{error:#}");
        assert!(text.contains(&bound.display().to_string()), "{text}");
        assert!(text.contains(&other.display().to_string()), "{text}");
        assert!(!other.exists(), "a refused mismatch creates nothing");
    }

    /// #3129: the printed object carries the captured `no_merge` and
    /// `dry_run` flags beside `allows_merge`, `run_id` and `pr_selected`,
    /// and `allows_merge` is still true only when both are false.
    #[test]
    fn the_answer_carries_the_captured_no_merge_and_dry_run_flags() {
        for (no_merge, dry_run) in [(false, false), (true, false), (false, true), (true, true)] {
            let temp = crate::test_support::tempdir_outside_framework_roots();
            let root = temp.path();
            fs::set_permissions(root, fs::Permissions::from_mode(0o700)).unwrap();
            let bound = root.join("bound-state");
            let mut auth = InvocationAuthorization::capture(
                "auto-drive-to-merge",
                &BTreeMap::from([
                    ("pr_number".into(), "7".into()),
                    ("no_merge".into(), no_merge.to_string()),
                    ("autodrive_dry_run".into(), dry_run.to_string()),
                ]),
            )
            .unwrap();
            auth.bind_policy(format!("trusted-launch-bundle:sha256:{}", "a".repeat(64)));
            auth.bind_state_dir(bound.clone());
            let invocation = root.join("invocation.json");
            fs::write(&invocation, serde_json::to_string(&auth).unwrap()).unwrap();
            fs::set_permissions(&invocation, fs::Permissions::from_mode(0o600)).unwrap();

            let answer = authorization(&invocation, &bound, None).unwrap();
            assert_eq!(
                answer,
                serde_json::json!({
                    "allows_merge": !no_merge && !dry_run,
                    "no_merge": no_merge,
                    "dry_run": dry_run,
                    "run_id": auth.run_id(),
                }),
                "no_merge={no_merge} dry_run={dry_run}"
            );
            for (pr, selected) in [(7, true), (8, false)] {
                let answer = authorization(&invocation, &bound, Some(pr)).unwrap();
                assert_eq!(answer["pr_selected"], selected, "{answer}");
                assert_eq!(answer["no_merge"], no_merge, "{answer}");
                assert_eq!(answer["dry_run"], dry_run, "{answer}");
                assert_eq!(answer["allows_merge"], !no_merge && !dry_run, "{answer}");
            }
        }
    }

    /// #2959: a record that is not there is an error that names the path.
    #[test]
    fn a_missing_record_error_names_the_invocation_path() {
        let temp = crate::test_support::tempdir_outside_framework_roots();
        let missing = temp
            .path()
            .join("autodrive-invocation-gone/invocation.json");
        let error = authorization(&missing, temp.path(), None).expect_err("missing is refused");
        let text = format!("{error:#}");
        assert!(text.contains(&missing.display().to_string()), "{text}");
    }
}
