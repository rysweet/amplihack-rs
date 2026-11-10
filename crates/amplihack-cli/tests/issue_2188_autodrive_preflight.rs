//! Regression contract for #2188. Uses real Git, fake gh and an offline wire
//! substitute after destination validation. Denial tests use real Git rewrites.
//! No credentials, real profile roots, or timers.
//! CLI contract: --pr, --repo, --branch, --state-dir; schema_version/outcome
//! are envelope fields. Hooks provide deterministic race boundaries.
#![cfg(unix)]

use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};

struct Fixture {
    tmp: tempfile::TempDir,
    repo: PathBuf,
    remote: PathBuf,
    base: String,
}

impl Fixture {
    fn command(&self, program: &str) -> Command {
        let mut c = Command::new(program);
        c.env_clear();
        c.env(
            "PATH",
            format!(
                "{}:{}",
                self.tmp.path().join("bin").display(),
                std::env::var("PATH").unwrap()
            ),
        );
        c.env("TMPDIR", self.tmp.path());
        for key in [
            "HOME",
            "USERPROFILE",
            "XDG_CONFIG_HOME",
            "XDG_STATE_HOME",
            "XDG_CACHE_HOME",
            "XDG_DATA_HOME",
            "AMPLIHACK_HOME",
            "AMPLIHACK_PROFILE_DIR",
            "AMPLIHACK_CLAUDE_PROFILE_ROOT",
            "AMPLIHACK_STATE_DIR",
            "CLAUDE_CONFIG_DIR",
            "CODEX_HOME",
        ] {
            c.env(key, self.tmp.path().join(key));
        }
        c.env("GIT_CONFIG_NOSYSTEM", "1")
            .env("AMPLIHACK_NO_UPDATE_CHECK", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("GIT_CONFIG_GLOBAL", self.tmp.path().join("gitconfig"))
            .env("GH_CONFIG_DIR", self.tmp.path().join("gh-config"))
            .env("FIXTURE", self.tmp.path())
            .current_dir(&self.repo);
        c
    }

    fn git(&self, args: &[&str]) -> String {
        let o = self.command("git").args(args).output().unwrap();
        assert!(
            o.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        String::from_utf8(o.stdout).unwrap().trim().into()
    }

    fn commit(&self, parent: &str, message: &str) -> String {
        let tree = self.git(&["write-tree"]);
        self.git(&["commit-tree", &tree, "-p", parent, "-m", message])
    }

    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(tmp.path().join("bin")).unwrap();
        // Use the real Git binary directly on macOS. The /usr/bin shim
        // tries to populate an xcrun cache outside the isolated fixture.
        #[cfg(target_os = "macos")]
        for git in [
            "/Applications/Xcode.app/Contents/Developer/usr/bin/git",
            "/Library/Developer/CommandLineTools/usr/bin/git",
        ] {
            if std::path::Path::new(git).is_file() {
                std::os::unix::fs::symlink(git, tmp.path().join("bin/git")).unwrap();
                break;
            }
        }
        let mut f = Self {
            remote: tmp.path().join("source.git"),
            tmp,
            repo,
            base: String::new(),
        };
        f.git(&["init", "-q", "-b", "feature"]);
        f.git(&["config", "user.name", "fixture"]);
        f.git(&["config", "user.email", "fixture@example.invalid"]);
        let tree = f.git(&["write-tree"]);
        f.base = f.git(&["commit-tree", &tree, "-m", "base"]);
        f.git(&["update-ref", "HEAD", &f.base]);
        f.git(&["init", "-q", "--bare", f.remote.to_str().unwrap()]);
        f.git(&[
            "config",
            "--global",
            &format!("url.{}.insteadOf", f.remote.display()),
            "https://github.com/source/project.git",
        ]);
        f.git(&[
            "remote",
            "add",
            "origin",
            "https://github.com/source/project.git",
        ]);
        f.git(&["push", "-q", "origin", "HEAD:refs/heads/feature"]);
        // Keep real Git object/ref/hook behavior while replacing only the HTTPS
        // wire in this offline fixture. Production policy still sees canonical
        // GitHub destinations; only the test executable enables file transport.
        let real_git = f.command("git").args(["--exec-path"]).output().unwrap();
        assert!(real_git.status.success());
        let git_path = std::fs::canonicalize(f.tmp.path().join("bin/git"))
            .ok()
            .unwrap_or_else(|| PathBuf::from("/usr/bin/git"));
        fs::remove_file(f.tmp.path().join("bin/git")).ok();
        f.script(
            "bin/git",
            &format!(
                r#"#!/bin/bash
args=(); isolated=0
for arg in "$@"; do
  case "$arg" in --git-dir=*) isolated=1;; esac
done
for arg in "$@"; do
  if [ "$isolated" = 1 ] && [ "$arg" = https://github.com/source/project.git ]; then
    args+=("$FIXTURE/source.git")
  else args+=("$arg"); fi
done
if [ "$isolated" = 1 ]; then args=(-c protocol.file.allow=always "${{args[@]}}"); fi
exec '{}' "${{args[@]}}"
"#,
                git_path.display()
            ),
        );
        f.git(&[
            "config",
            "--global",
            "--unset-all",
            &format!("url.{}.insteadOf", f.remote.display()),
        ]);
        f.git(&["remote", "set-url", "origin", f.remote.to_str().unwrap()]);
        let metadata = json!({"number":7,"state":"OPEN","merged":false,"isCrossRepository":true,"headRefName":"feature","headRefOid":f.base,"headRepository":{"name":"project","nameWithOwner":"source/project","url":"https://github.com/source/project"},"headRepositoryOwner":{"login":"source"},"baseRepository":{"nameWithOwner":"target/project"}});
        fs::write(f.tmp.path().join("metadata"), metadata.to_string()).unwrap();
        f.script("bin/gh", "#!/bin/sh\ncase \"$1 $2\" in\n 'pr view') cat \"$FIXTURE/metadata\";;\n 'pr comment') printf 'comment\\n' >> \"$FIXTURE/comments\"; test ! -f \"$FIXTURE/comment-fails\";;\n *) exit 2;;\nesac\n");
        f
    }

    fn script(&self, path: &str, body: &str) {
        let p = self.tmp.path().join(path);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, body).unwrap();
        fs::set_permissions(p, fs::Permissions::from_mode(0o700)).unwrap();
    }

    fn run(&self) -> Output {
        self.command(
            super::support::amplihack_bin::amplihack_bin()
                .to_str()
                .unwrap(),
        )
        .args([
            "orch",
            "helper",
            "autodrive-preflight",
            "--pr",
            "7",
            "--repo",
            "target/project",
            "--branch",
            "feature",
            "--state-dir",
        ])
        .arg(self.tmp.path().join("state"))
        .output()
        .unwrap()
    }

    fn envelope(&self, status: i32, outcome: &str) -> Value {
        let o = self.run();
        assert_eq!(
            o.status.code(),
            Some(status),
            "stderr={} stdout={}",
            String::from_utf8_lossy(&o.stderr),
            String::from_utf8_lossy(&o.stdout)
        );
        let v: Value = serde_json::from_slice(&o.stdout).expect("exactly one JSON envelope");
        assert_eq!(v["schema_version"], 1);
        assert_eq!(v["outcome"], outcome, "{v}");
        v
    }

    fn refusal(&self, outcome: &str) {
        self.envelope(1, outcome);
        assert_eq!(
            fs::read_to_string(self.tmp.path().join("comments")).unwrap(),
            "comment\n"
        );
        let state = self.tmp.path().join("state");
        assert_eq!(
            fs::metadata(&state).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let records: Vec<_> = fs::read_dir(state)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.is_file())
            .collect();
        assert!(
            records.iter().any(|p| {
                let bytes = fs::read(p).unwrap();
                let found = serde_json::from_slice::<Value>(&bytes)
                    .ok()
                    .is_some_and(|v| v["outcome"] == outcome);
                if found {
                    assert_eq!(fs::metadata(p).unwrap().permissions().mode() & 0o777, 0o600);
                }
                found
            }),
            "named refusal must be durably sweep-readable"
        );
    }
}

#[test]
fn synchronized_is_ready_without_publication() {
    let f = Fixture::new();
    f.envelope(0, "ready");
    assert!(!f.tmp.path().join("comments").exists());
}

#[test]
fn clean_ahead_pushes_captured_commit_with_hook() {
    let f = Fixture::new();
    let next = f.commit(&f.base, "ahead");
    f.git(&["update-ref", "HEAD", &next]);
    f.script(
        "repo/.git/hooks/pre-push",
        "#!/bin/sh\nprintf 'hook\\n' >> \"$FIXTURE/hook\"\n",
    );
    f.envelope(0, "ready");
    assert_eq!(
        f.git(&[
            "--git-dir",
            f.remote.to_str().unwrap(),
            "rev-parse",
            "refs/heads/feature"
        ]),
        next
    );
    assert_eq!(
        fs::read_to_string(f.tmp.path().join("hook")).unwrap(),
        "hook\n"
    );
}

#[test]
fn behind_and_diverged_are_distinct_durable_refusals() {
    for diverged in [false, true] {
        let f = Fixture::new();
        let remote = f.commit(&f.base, "remote");
        f.git(&[
            "push",
            "-q",
            "origin",
            &format!("{remote}:refs/heads/feature"),
        ]);
        let metadata_path = f.tmp.path().join("metadata");
        let mut metadata: Value =
            serde_json::from_slice(&fs::read(&metadata_path).unwrap()).unwrap();
        metadata["headRefOid"] = json!(remote);
        fs::write(metadata_path, metadata.to_string()).unwrap();
        if diverged {
            let local = f.commit(&f.base, "local");
            f.git(&["update-ref", "HEAD", &local]);
        }
        let before = f.git(&["rev-parse", "HEAD"]);
        f.refusal(if diverged {
            "pr-worktree-diverged"
        } else {
            "pr-worktree-behind"
        });
        assert_eq!(f.git(&["rev-parse", "HEAD"]), before);
    }
}

#[test]
fn dirty_unusual_path_is_preserved_and_refused() {
    let f = Fixture::new();
    let path = f.repo.join("untracked\n\tfile");
    fs::write(&path, "preserve").unwrap();
    f.refusal("pr-worktree-dirty");
    assert_eq!(fs::read_to_string(path).unwrap(), "preserve");
}

#[test]
fn operation_without_unmerged_paths_precedes_dirty_or_history() {
    for marker in ["MERGE_HEAD", "CHERRY_PICK_HEAD", "REVERT_HEAD"] {
        let f = Fixture::new();
        let p = f.repo.join(".git").join(marker);
        fs::write(&p, format!("{}\n", f.base)).unwrap();
        fs::write(f.repo.join("dirty"), "preserve").unwrap();
        f.refusal("pr-worktree-interrupted");
        assert_eq!(fs::read_to_string(p).unwrap(), format!("{}\n", f.base));
        assert!(f.git(&["ls-files", "-u"]).is_empty());
    }
}

#[test]
fn rejected_hook_preserves_local_commit_and_remote_head() {
    let f = Fixture::new();
    let next = f.commit(&f.base, "ahead");
    f.git(&["update-ref", "HEAD", &next]);
    f.script(
        "repo/.git/hooks/pre-push",
        "#!/bin/sh\nprintf 'hook\\n' >> \"$FIXTURE/hook\"\nexit 1\n",
    );
    f.refusal("pr-worktree-push-failed");
    assert_eq!(f.git(&["rev-parse", "HEAD"]), next);
    assert_eq!(
        f.git(&[
            "--git-dir",
            f.remote.to_str().unwrap(),
            "rev-parse",
            "refs/heads/feature"
        ]),
        f.base
    );
    assert_eq!(
        fs::read_to_string(f.tmp.path().join("hook")).unwrap(),
        "hook\n"
    );
}

#[test]
fn unavailable_source_ref_is_comparison_failure() {
    let f = Fixture::new();
    f.git(&[
        "--git-dir",
        f.remote.to_str().unwrap(),
        "update-ref",
        "-d",
        "refs/heads/feature",
    ]);
    f.refusal("pr-worktree-comparison-failed");
}

#[test]
fn publication_failure_keeps_primary_refusal_and_local_record() {
    let f = Fixture::new();
    fs::write(f.tmp.path().join("comment-fails"), "").unwrap();
    fs::write(f.repo.join("dirty"), "").unwrap();
    f.refusal("pr-worktree-dirty");
}

#[test]
fn rebase_detached_worktree_remains_identifiable_without_recovery_mutation() {
    let f = Fixture::new();
    let next = f.commit(&f.base, "local");
    f.git(&["update-ref", "HEAD", &next]);
    f.git(&["checkout", "--detach", "-q", &f.base]);
    let rebase = f.repo.join(".git/rebase-merge");
    fs::create_dir(&rebase).unwrap();
    for (name, contents) in [
        ("head-name", "refs/heads/feature"),
        ("orig-head", next.as_str()),
        ("onto", f.base.as_str()),
    ] {
        fs::write(rebase.join(name), format!("{contents}\n")).unwrap();
    }
    f.refusal("pr-worktree-interrupted");
    assert_eq!(
        fs::read_to_string(rebase.join("head-name")).unwrap(),
        "refs/heads/feature\n"
    );
    assert_eq!(f.git(&["rev-parse", "HEAD"]), f.base);
    assert_eq!(f.git(&["rev-parse", "feature"]), next);
}

#[test]
fn unmerged_index_is_interrupted_even_without_operation_marker() {
    let f = Fixture::new();
    let blob = f.git(&["hash-object", "-w", "--stdin"]);
    let mut child = f
        .command("git")
        .args(["update-index", "--index-info"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    writeln!(child.stdin.take().unwrap(), "100644 {blob} 1\tconflict").unwrap();
    assert!(child.wait().unwrap().success());
    let before = f.git(&["ls-files", "-u"]);
    f.refusal("pr-worktree-interrupted");
    assert_eq!(f.git(&["ls-files", "-u"]), before);
}

#[test]
fn source_branch_mismatch_is_explicit_and_does_not_push() {
    let f = Fixture::new();
    let p = f.tmp.path().join("metadata");
    let mut metadata: Value = serde_json::from_slice(&fs::read(&p).unwrap()).unwrap();
    metadata["headRefName"] = json!("other-branch");
    fs::write(p, metadata.to_string()).unwrap();
    f.refusal("pr-worktree-branch-mismatch");
    assert_eq!(f.git(&["rev-parse", "HEAD"]), f.base);
}

#[test]
fn missing_branch_worktree_is_named_refusal() {
    let f = Fixture::new();
    f.git(&["checkout", "-q", "--detach"]);
    f.refusal("pr-worktree-not-found");
}

#[test]
fn symlinked_state_keeps_primary_refusal_without_writing_target() {
    let f = Fixture::new();
    let target = f.tmp.path().join("protected-target");
    fs::create_dir(&target).unwrap();
    std::os::unix::fs::symlink(&target, f.tmp.path().join("state")).unwrap();
    fs::write(f.repo.join("dirty"), "preserve").unwrap();
    let v = f.envelope(1, "pr-worktree-dirty");
    assert!(
        v.to_string().contains("persist"),
        "persistence failure must be visible: {v}"
    );
    assert_eq!(fs::read_dir(target).unwrap().count(), 0);
    assert_eq!(
        fs::read_to_string(f.tmp.path().join("comments")).unwrap(),
        "comment\n"
    );
}

impl Fixture {
    fn install_loop_tools(&self) {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let tools = self
            .tmp
            .path()
            .join("AMPLIHACK_HOME/amplifier-bundle/tools");
        fs::create_dir_all(&tools).unwrap();
        fs::copy(
            root.join("amplifier-bundle/tools/autodrive_state.sh"),
            tools.join("autodrive_state.sh"),
        )
        .unwrap();
        std::os::unix::fs::symlink(
            super::support::amplihack_bin::amplihack_bin(),
            self.tmp.path().join("bin/amplihack"),
        )
        .unwrap();
        self.script(
            "bin/gh",
            r#"#!/bin/sh
case "$1 $2" in
 'repo view') printf 'target/project\n';;
 'pr view') case "$*" in *'--jq .state'*) echo OPEN;; *) cat "$FIXTURE/metadata";; esac;;
 'pr comment') printf 'comment\n' >> "$FIXTURE/comments"; printf '%s\n' "$*" > "$FIXTURE/comment-body"; test ! -f "$FIXTURE/comment-fails";;
 *) exit 2;;
esac
"#,
        );
    }
    fn loop_preflight(&self, name: &str) -> Output {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let yaml: serde_yaml::Value = serde_yaml::from_str(
            &fs::read_to_string(root.join(format!(
                "amplifier-bundle/recipes/autodrive-{name}-loop.yaml"
            )))
            .unwrap(),
        )
        .unwrap();
        // Model the runner's single decision before invoking the shell consumer.
        let platform = self
            .command("gh")
            .args(["pr", "view", "7", "--json", "state,mergedAt"])
            .output()
            .unwrap();
        let merged = serde_json::from_slice::<Value>(&platform.stdout)
            .ok()
            .is_some_and(|v| v["state"] == "MERGED");
        let completed = name == "crusty"
            && fs::read_to_string(self.tmp.path().join("state/phases.tsv"))
                .is_ok_and(|s| s.starts_with("crusty-loop\t"));
        let ready = if merged || completed {
            Vec::new()
        } else {
            let decision = self.run();
            if !decision.status.success() {
                return decision;
            }
            decision.stdout
        };
        self.command("bash")
            .args(["-c", yaml["steps"][0]["command"].as_str().unwrap()])
            .env(
                "AMPLIHACK_AUTODRIVE_READY",
                String::from_utf8(ready).unwrap(),
            )
            .env("PR_NUMBER", "7")
            .env("REPO_PATH", &self.repo)
            .env("AUTODRIVE_STATE_DIR", self.tmp.path().join("state"))
            .output()
            .unwrap()
    }
}

#[test]
fn both_loop_preflights_push_ahead_and_refuse_interrupted() {
    for name in ["crusty", "merge"] {
        for interrupted in [false, true] {
            let f = Fixture::new();
            f.install_loop_tools();
            let next = f.commit(&f.base, "ahead");
            f.git(&["update-ref", "HEAD", &next]);
            if interrupted {
                fs::write(f.repo.join(".git/MERGE_HEAD"), &f.base).unwrap();
            }
            let o = f.loop_preflight(name);
            assert_eq!(
                o.status.code(),
                Some(if interrupted { 1 } else { 0 }),
                "{name}: {}",
                String::from_utf8_lossy(&o.stderr)
            );
            let v: Value = serde_json::from_slice(&o.stdout).unwrap();
            if interrupted {
                assert_eq!(v["outcome"], "pr-worktree-interrupted");
                assert!(f.tmp.path().join("comments").exists());
            } else {
                assert_eq!(v["should_run"], "true");
                assert_eq!(v["repo"], f.repo.to_str().unwrap());
                assert_eq!(
                    f.git(&["ls-remote", "origin", "refs/heads/feature"])
                        .split_whitespace()
                        .next(),
                    Some(next.as_str())
                );
            }
        }
    }
}

#[test]
fn duplicate_branch_worktrees_are_not_selected() {
    let f = Fixture::new();
    f.git(&[
        "worktree",
        "add",
        "--force",
        f.tmp.path().join("second").to_str().unwrap(),
        "feature",
    ]);
    f.refusal("pr-worktree-ambiguous");
}

#[test]
fn remote_change_during_push_is_not_retried() {
    let f = Fixture::new();
    let next = f.commit(&f.base, "ahead");
    let other = f.commit(&f.base, "concurrent");
    f.git(&["update-ref", "HEAD", &next]);
    fs::write(f.tmp.path().join("other"), other).unwrap();
    f.script(
        "repo/.git/hooks/pre-push",
        r#"#!/bin/sh
printf 'hook\n' >> "$FIXTURE/hook"
git --git-dir "$FIXTURE/source.git" update-ref refs/heads/feature "$(cat "$FIXTURE/other")"
"#,
    );
    f.refusal("pr-worktree-push-failed");
    assert_eq!(
        fs::read_to_string(f.tmp.path().join("hook")).unwrap(),
        "hook\n"
    );
    assert_eq!(f.git(&["rev-parse", "HEAD"]), next);
}

#[test]
fn shallow_history_is_unverifiable() {
    let f = Fixture::new();
    fs::write(f.repo.join(".git/shallow"), format!("{}\n", f.base)).unwrap();
    f.refusal("pr-worktree-comparison-failed");
}

#[path = "issue_2188_autodrive_refusal_propagation.rs"]
mod propagation;

#[test]
fn successful_push_with_changed_remote_verification_refuses() {
    let f = Fixture::new();
    let next = f.commit(&f.base, "ahead");
    f.git(&["update-ref", "HEAD", &next]);
    fs::write(f.tmp.path().join("base"), &f.base).unwrap();
    f.script(
        "source.git/hooks/post-receive",
        r#"#!/bin/sh
git --git-dir "$FIXTURE/source.git" update-ref refs/heads/feature "$(cat "$FIXTURE/base")"
"#,
    );
    let v = f.envelope(1, "pr-worktree-push-failed");
    assert_eq!(v["evidence"]["stage"], "remote-verification");
    assert_eq!(f.git(&["rev-parse", "HEAD"]), next);
}

#[test]
#[cfg(target_os = "linux")]
fn non_utf8_dirty_filename_is_lossless() {
    use std::os::unix::ffi::OsStrExt;
    let f = Fixture::new();
    let name = std::ffi::OsStr::from_bytes(b"untracked-\xff");
    fs::write(f.repo.join(name), "preserve").unwrap();
    let v = f.envelope(1, "pr-worktree-dirty");
    assert_eq!(v["evidence"]["paths"][0]["encoding"], "base64");
    use base64::Engine;
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(v["evidence"]["paths"][0]["value"].as_str().unwrap())
            .unwrap(),
        b"untracked-\xff"
    );
}

#[test]
fn rename_keeps_both_endpoints() {
    let f = Fixture::new();
    fs::write(f.repo.join("before"), "content").unwrap();
    f.git(&["add", "before"]);
    f.git(&["commit", "-qm", "tracked file"]);
    f.git(&["mv", "before", "after"]);
    let v = f.envelope(1, "pr-worktree-dirty");
    let paths: Vec<_> = v["evidence"]["paths"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["value"].as_str().unwrap())
        .collect();
    assert!(paths.contains(&"before") && paths.contains(&"after"));
}

#[test]
fn both_loops_keep_primary_refusal_when_state_is_a_symlink() {
    for name in ["crusty", "merge"] {
        let f = Fixture::new();
        f.install_loop_tools();
        let target = f.tmp.path().join("protected-target");
        fs::create_dir(&target).unwrap();
        std::os::unix::fs::symlink(&target, f.tmp.path().join("state")).unwrap();
        fs::write(f.repo.join("dirty"), "preserve").unwrap();
        let o = f.loop_preflight(name);
        assert_eq!(
            o.status.code(),
            Some(1),
            "stderr={} stdout={}",
            String::from_utf8_lossy(&o.stderr),
            String::from_utf8_lossy(&o.stdout)
        );
        let v: Value = serde_json::from_slice(&o.stdout).unwrap();
        assert_eq!(v["outcome"], "pr-worktree-dirty");
        assert!(v.get("persistence_error").is_some());
        assert_eq!(fs::read_dir(target).unwrap().count(), 0);
        assert!(f.tmp.path().join("comments").exists());
    }
}

#[test]
fn fork_source_is_used_even_when_origin_is_unrelated() {
    let f = Fixture::new();
    let next = f.commit(&f.base, "ahead");
    f.git(&["update-ref", "HEAD", &next]);
    f.git(&[
        "remote",
        "set-url",
        "origin",
        "https://github.com/unrelated/target.git",
    ]);
    f.envelope(0, "ready");
    assert_eq!(
        f.git(&[
            "--git-dir",
            f.remote.to_str().unwrap(),
            "rev-parse",
            "refs/heads/feature"
        ]),
        next
    );
}

#[test]
fn hook_local_mutation_prevents_continuation_after_push() {
    let f = Fixture::new();
    let next = f.commit(&f.base, "ahead");
    f.git(&["update-ref", "HEAD", &next]);
    f.script(
        "repo/.git/hooks/pre-push",
        "#!/bin/sh\nprintf preserve > \"$FIXTURE/repo/dirty\"\n",
    );
    let v = f.envelope(1, "pr-worktree-push-failed");
    assert_eq!(v["evidence"]["stage"], "local-verification");
    assert_eq!(
        fs::read_to_string(f.repo.join("dirty")).unwrap(),
        "preserve"
    );
}

#[test]
fn comment_failure_diagnostic_is_durable_and_does_not_replace_outcome() {
    let f = Fixture::new();
    fs::write(f.tmp.path().join("comment-fails"), "").unwrap();
    fs::write(f.repo.join("dirty"), "preserve").unwrap();
    let v = f.envelope(1, "pr-worktree-dirty");
    assert!(v.get("publication_error").is_some());
    assert_eq!(v["persistence"]["initial_record_written"], true);
    let saved: Value = serde_json::from_slice(
        &fs::read(f.tmp.path().join("state/preflight-result.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(saved, v);
}

#[test]
fn existing_result_symlink_is_never_followed() {
    let f = Fixture::new();
    let state = f.tmp.path().join("state");
    fs::create_dir(&state).unwrap();
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
    let target = f.tmp.path().join("protected-result");
    fs::write(&target, "preserve").unwrap();
    std::os::unix::fs::symlink(&target, state.join("preflight-result.json")).unwrap();
    fs::write(f.repo.join("dirty"), "preserve").unwrap();
    let v = f.envelope(1, "pr-worktree-dirty");
    assert!(v.get("persistence_error").is_some());
    assert_eq!(fs::read_to_string(target).unwrap(), "preserve");
    assert!(
        fs::symlink_metadata(state.join("preflight-result.json"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn invalid_invocation_is_exit_two_without_publication() {
    let f = Fixture::new();
    for pr in ["0", "not-a-number"] {
        let o = f
            .command(
                super::support::amplihack_bin::amplihack_bin()
                    .to_str()
                    .unwrap(),
            )
            .args([
                "orch",
                "helper",
                "autodrive-preflight",
                "--pr",
                pr,
                "--repo",
                "target/project",
                "--state-dir",
                "unused",
            ])
            .output()
            .unwrap();
        assert_eq!(o.status.code(), Some(2));
    }
    assert!(!f.tmp.path().join("comments").exists());
}

#[test]
fn both_loops_propagate_named_refusals_without_phase_completion() {
    for name in ["crusty", "merge"] {
        for outcome in [
            "dirty",
            "behind",
            "diverged",
            "not-found",
            "ambiguous",
            "comparison-failed",
            "push-failed",
        ] {
            let f = Fixture::new();
            f.install_loop_tools();
            match outcome {
                "dirty" => fs::write(f.repo.join("dirty"), "preserve").unwrap(),
                "behind" | "diverged" => {
                    let remote = f.commit(&f.base, "remote");
                    f.git(&["push", "origin", &format!("{remote}:refs/heads/feature")]);
                    let p = f.tmp.path().join("metadata");
                    let mut metadata: Value =
                        serde_json::from_slice(&fs::read(&p).unwrap()).unwrap();
                    metadata["headRefOid"] = json!(remote);
                    fs::write(p, metadata.to_string()).unwrap();
                    if outcome == "diverged" {
                        let local = f.commit(&f.base, "local");
                        f.git(&["update-ref", "HEAD", &local]);
                    }
                }
                "not-found" => {
                    f.git(&["checkout", "--detach", "-q"]);
                }
                "ambiguous" => {
                    f.git(&[
                        "worktree",
                        "add",
                        "--force",
                        f.tmp.path().join("second").to_str().unwrap(),
                        "feature",
                    ]);
                }
                "comparison-failed" => {
                    f.git(&[
                        "--git-dir",
                        f.remote.to_str().unwrap(),
                        "update-ref",
                        "-d",
                        "refs/heads/feature",
                    ]);
                }
                "push-failed" => {
                    let next = f.commit(&f.base, "ahead");
                    f.git(&["update-ref", "HEAD", &next]);
                    f.script("repo/.git/hooks/pre-push", "#!/bin/sh\nexit 1\n");
                }
                _ => unreachable!(),
            }
            let o = f.loop_preflight(name);
            assert_eq!(
                o.status.code(),
                Some(1),
                "{name}/{outcome}: {}",
                String::from_utf8_lossy(&o.stderr)
            );
            let v: Value = serde_json::from_slice(&o.stdout).unwrap();
            assert_eq!(v["outcome"], format!("pr-worktree-{outcome}"));
            assert!(f.tmp.path().join("state/preflight-result.json").exists());
            assert!(!f.tmp.path().join("state/phases.tsv").exists());
            assert_eq!(
                fs::read_to_string(f.tmp.path().join("comments")).unwrap(),
                "comment\n"
            );
        }
    }
}

#[test]
fn dangling_operation_marker_is_interrupted_without_following_it() {
    let f = Fixture::new();
    let marker = f.repo.join(".git/MERGE_HEAD");
    std::os::unix::fs::symlink(f.tmp.path().join("missing-target"), &marker).unwrap();
    let v = f.envelope(1, "pr-worktree-interrupted");
    assert!(
        v["evidence"]["operations"][0]["metadata"]["diagnostic"]
            .as_str()
            .unwrap()
            .contains("not followed")
    );
    assert!(
        fs::symlink_metadata(marker)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn both_loops_preserve_merged_skips_without_publishing_local_commits() {
    for name in ["crusty", "merge"] {
        let f = Fixture::new();
        f.install_loop_tools();
        let local = f.commit(&f.base, "unpublished");
        f.git(&["update-ref", "HEAD", &local]);
        f.script(
            "bin/gh",
            r#"#!/bin/sh
case "$*" in *'--json state,mergedAt'*) printf '%s\n' '{"state":"MERGED","mergedAt":"completed"}';; *) exit 91;; esac
"#,
        );
        let o = f.loop_preflight(name);
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        let v: Value = serde_json::from_slice(&o.stdout).unwrap();
        assert_eq!(v["should_run"], "false");
        assert_eq!(
            f.git(&[
                "--git-dir",
                f.remote.to_str().unwrap(),
                "rev-parse",
                "refs/heads/feature"
            ]),
            f.base
        );
        assert!(!f.tmp.path().join("comments").exists());
    }
}

#[test]
fn crusty_completed_phase_skip_preserves_existing_behavior() {
    let f = Fixture::new();
    f.install_loop_tools();
    let state = f.tmp.path().join("state");
    fs::create_dir(&state).unwrap();
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(state.join("phases.tsv"), "crusty-loop\tcompleted\n").unwrap();
    fs::set_permissions(state.join("phases.tsv"), fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(f.repo.join("dirty"), "preserve").unwrap();
    let o = f.loop_preflight("crusty");
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["should_run"], "false");
    assert!(!f.tmp.path().join("comments").exists());
}

#[test]
fn failed_diagnostic_replacement_preserves_initial_record_and_primary_outcome() {
    let f = Fixture::new();
    let target = f.tmp.path().join("target");
    fs::create_dir(&target).unwrap();
    fs::write(f.repo.join("dirty"), "preserve").unwrap();
    f.script(
        "bin/gh",
        r#"#!/bin/sh
case "$1 $2" in
 'pr view') cat "$FIXTURE/metadata";;
 'pr comment')
   printf 'comment\n' >> "$FIXTURE/comments"
   mv "$FIXTURE/state" "$FIXTURE/initial-state"
   ln -s "$FIXTURE/target" "$FIXTURE/state";;
 *) exit 2;;
esac
"#,
    );
    let v = f.envelope(1, "pr-worktree-dirty");
    assert_eq!(v["persistence"]["initial_record_written"], true);
    assert!(v.get("persistence_error").is_some());
    let initial: Value = serde_json::from_slice(
        &fs::read(f.tmp.path().join("initial-state/preflight-result.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(initial["outcome"], "pr-worktree-dirty");
    assert_eq!(initial["persistence"]["initial_record_written"], true);
    assert_eq!(fs::read_dir(target).unwrap().count(), 0);
}

#[test]
fn synchronized_existing_public_state_directory_is_refused_without_chmod() {
    let f = Fixture::new();
    let state = f.tmp.path().join("state");
    fs::create_dir(&state).unwrap();
    fs::set_permissions(&state, fs::Permissions::from_mode(0o755)).unwrap();
    f.envelope(1, "pr-worktree-comparison-failed");
    assert_eq!(
        fs::metadata(&state).unwrap().permissions().mode() & 0o777,
        0o755
    );
    assert_eq!(fs::read_dir(state).unwrap().count(), 0);
}

#[test]
fn executable_remote_transport_is_rejected_even_if_enabled_in_configuration() {
    let f = Fixture::new();
    f.script(
        "remote-helper",
        "#!/bin/sh\nprintf invoked > \"$FIXTURE/executed-helper\"\nexit 1\n",
    );
    f.git(&["config", "--global", "protocol.ext.allow", "always"]);
    f.git(&[
        "config",
        "--global",
        &format!(
            "url.ext::{}.insteadOf",
            f.tmp.path().join("remote-helper").display()
        ),
        "https://github.com/source/project.git",
    ]);
    f.refusal("pr-worktree-comparison-failed");
    assert!(!f.tmp.path().join("executed-helper").exists());
}

#[test]
fn sequencer_between_cherry_picks_is_still_an_interrupted_operation() {
    let f = Fixture::new();
    let sequencer = f.repo.join(".git/sequencer");
    fs::create_dir(&sequencer).unwrap();
    let todo = format!("pick {} pending commit\n", f.base);
    fs::write(sequencer.join("todo"), &todo).unwrap();
    fs::write(sequencer.join("head"), &f.base).unwrap();
    f.refusal("pr-worktree-interrupted");
    assert_eq!(fs::read_to_string(sequencer.join("todo")).unwrap(), todo);
}

#[test]
fn destination_policy_rejects_custom_helpers_rewrites_and_multiple_push_urls() {
    for (key, destination) in [
        ("insteadOf", "reviewprobe::payload"),
        ("pushInsteadOf", "reviewprobe::payload"),
        ("insteadOf", "https://github.com/unintended/repo.git"),
        ("pushInsteadOf", "https://github.com/unintended/repo.git"),
    ] {
        let f = Fixture::new();
        f.script(
            "bin/git-remote-reviewprobe",
            "#!/bin/sh\ntouch \"$FIXTURE/executed-helper\"\nexit 1\n",
        );
        let before = fs::read(f.remote.join("refs/heads/feature")).unwrap();
        f.git(&[
            "config",
            "--global",
            &format!("url.{destination}.{key}"),
            "https://github.com/source/project.git",
        ]);
        f.git(&["config", "--global", "protocol.reviewprobe.allow", "always"]);
        f.refusal("pr-worktree-comparison-failed");
        assert!(!f.tmp.path().join("executed-helper").exists());
        assert_eq!(
            fs::read(f.remote.join("refs/heads/feature")).unwrap(),
            before
        );
    }
    let f = Fixture::new();
    for url in [
        "https://github.com/source/project.git",
        "https://github.com/unintended/repo.git",
    ] {
        f.git(&["config", "--add", "remote.amplihack-preflight.pushurl", url]);
    }
    f.refusal("pr-worktree-comparison-failed");
}

#[test]
fn sensitive_subprocess_diagnostics_are_sanitized_in_stdout_and_state() {
    let f = Fixture::new();
    f.script("bin/gh", "#!/bin/sh\nprintf '\\033[31mmetadata failed\\033[0m https://user:secret@github.com/a/b\\nAuthorization: Bearer hidden\\n\\033]0;window\\007context remains\\n' >&2\nexit 1\n");
    let o = f.run();
    assert!(!o.status.success());
    let state = fs::read(f.tmp.path().join("state/preflight-result.json")).unwrap();
    for bytes in [&o.stdout, &state] {
        let text = String::from_utf8_lossy(bytes);
        for secret in [
            "user:secret",
            "Bearer",
            "hidden",
            "window",
            "\\u001b",
            "\\u0007",
        ] {
            assert!(!text.contains(secret), "{text}");
        }
        assert!(text.contains("metadata failed"));
        assert!(text.contains("context remains"));
    }
}

#[test]
fn rewritten_destination_after_validation_cannot_reach_network_execution() {
    let f = Fixture::new();
    f.script(
        "bin/git-remote-reviewprobe",
        "#!/bin/sh\ntouch \"$FIXTURE/executed-helper\"\nexit 1\n",
    );
    let wrapper = fs::read_to_string(f.tmp.path().join("bin/git")).unwrap();
    let injection = r#"
if [ "$isolated" = 1 ]; then
  case " $* " in
    *' fetch '*|*' push '*|*' ls-remote '*)
      printf '[url "reviewprobe::payload"]\n insteadOf = https://github.com/source/project.git\n' > "$FIXTURE/gitconfig";;
  esac
fi
exec '"#;
    f.script("bin/git", &wrapper.replace("\nexec '", injection));
    // The command has already captured policy and disabled the global config;
    // a later write cannot redirect even the first network operation.
    f.envelope(1, "pr-worktree-comparison-failed");
    assert!(!f.tmp.path().join("executed-helper").exists());
}

#[test]
fn ancestor_remote_change_before_advertisement_refuses() {
    let f = Fixture::new();
    let middle = f.commit(&f.base, "middle");
    let next = f.commit(&middle, "ahead");
    f.git(&["update-ref", "HEAD", &next]);
    f.git(&["push", "origin", &format!("{middle}:refs/heads/feature")]);
    let metadata_path = f.tmp.path().join("metadata");
    let mut metadata: Value = serde_json::from_slice(&fs::read(&metadata_path).unwrap()).unwrap();
    metadata["headRefOid"] = json!(middle);
    fs::write(metadata_path, metadata.to_string()).unwrap();
    let wrapper = f.tmp.path().join("bin/git");
    let script = fs::read_to_string(&wrapper).unwrap();
    let real = script
        .split("exec '")
        .nth(1)
        .unwrap()
        .split('\'')
        .next()
        .unwrap();
    let injection = format!(
        r#"
if [ "$isolated" = 1 ]; then
  for arg in "$@"; do
    if [ "$arg" = push ]; then
      '{}' --git-dir "$FIXTURE/source.git" update-ref refs/heads/feature '{}'
    fi
  done
fi
"#,
        real, f.base
    );
    fs::write(wrapper, script.replace("exec '", &(injection + "exec '"))).unwrap();
    f.refusal("pr-worktree-push-failed");
    assert_eq!(
        f.git(&[
            "--git-dir",
            f.remote.to_str().unwrap(),
            "rev-parse",
            "refs/heads/feature"
        ]),
        f.base
    );
    assert_eq!(f.git(&["rev-parse", "HEAD"]), next);
}

#[test]
fn authentication_snapshot_is_private_and_absent_from_argv() {
    let f = Fixture::new();
    let secret = "AUTHORIZATION: basic dummy-2188-credential";
    f.git(&["config", "http.https://github.com/.extraheader", secret]);
    let next = f.commit(&f.base, "ahead");
    f.git(&["update-ref", "HEAD", &next]);
    let wrapper = f.tmp.path().join("bin/git");
    let script = fs::read_to_string(&wrapper).unwrap();
    let injection = r#"
if [ "$isolated" = 1 ]; then
  printf '%s\n' "$@" >> "$FIXTURE/network-argv"
  for arg in "$@"; do
    case "$arg" in --git-dir=*)
      cp -p "${arg#--git-dir=}/config" "$FIXTURE/frozen-config";;
    esac
  done
fi
"#;
    fs::write(
        wrapper,
        script.replace("exec '", &(injection.to_owned() + "exec '")),
    )
    .unwrap();
    let o = f.run();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let args = fs::read_to_string(f.tmp.path().join("network-argv")).unwrap();
    assert!(!args.contains(secret));
    assert!(!args.contains("dummy-2188-credential"));
    let config = f.tmp.path().join("frozen-config");
    assert_eq!(
        fs::metadata(&config).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        f.git(&[
            "config",
            "--file",
            config.to_str().unwrap(),
            "--get",
            "http.https://github.com/.extraheader"
        ]),
        secret
    );
}
