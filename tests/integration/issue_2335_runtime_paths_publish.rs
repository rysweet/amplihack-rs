//! TDD-red contracts for #2335: amplihack's own runtime files never make a
//! run's worktree dirty, and the step-16 dirty-worktree failure names the
//! product files it found so an agent can act on them.
//!
//! Fixtures are temporary git repositories with a linked worktree, a `gh`
//! stand-in on PATH that records its calls and exits 77, and a temporary 0700
//! HOME. No real login, remote or tmux session is touched.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

fn workspace_helper_path(name: &str) -> PathBuf {
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    root.pop();
    root.pop();
    root.join("amplifier-bundle").join("tools").join(name)
}

fn git(dir: &Path, args: &[&str]) {
    // amplihack_git::command_in, never a raw git subprocess (issue #1278): a raw
    // one inherits GIT_DIR/GIT_WORK_TREE from the hook that spawned the test and
    // those override .current_dir().
    let out = amplihack_git::command_in(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn write(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    fs::write(path, content).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

struct Fixture {
    _tmp: TempDir,
    wt: PathBuf,
    gh_log: PathBuf,
    bin: PathBuf,
    home: PathBuf,
}

/// A main checkout on `main`, plus a linked worktree on `feat/x` that is one
/// committed file ahead of `origin/main`.
fn fixture() -> Fixture {
    let tmp = TempDir::new().expect("tempdir");
    let main = tmp.path().join("main");
    let wt = tmp.path().join("wt");
    let home = tmp.path().join("home");
    fs::create_dir_all(&main).unwrap();
    fs::create_dir_all(&home).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).unwrap();
    }
    git(&main, &["init", "-b", "main"]);
    git(&main, &["config", "user.email", "test@example.com"]);
    git(&main, &["config", "user.name", "Workflow Test"]);
    write(&main.join("README.md"), "base\n");
    git(&main, &["add", "README.md"]);
    git(&main, &["commit", "-m", "base"]);
    git(
        &main,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/owner/repo.git",
        ],
    );
    git(&main, &["update-ref", "refs/remotes/origin/main", "main"]);
    git(
        &main,
        &["worktree", "add", "-b", "feat/x", wt.to_str().unwrap()],
    );
    write(&wt.join("feature.txt"), "work\n");
    git(&wt, &["add", "feature.txt"]);
    git(&wt, &["commit", "-m", "feature"]);

    let bin = tmp.path().join("bin");
    let gh_log = tmp.path().join("gh.log");
    let gh = bin.join("gh");
    write(
        &gh,
        &format!(
            "#!/usr/bin/env bash\necho \"$*\" >> '{}'\nexit 77\n",
            gh_log.display()
        ),
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
    }
    Fixture {
        _tmp: tmp,
        wt,
        gh_log,
        bin,
        home,
    }
}

fn run_step16(f: &Fixture) -> Output {
    let old_path = std::env::var("PATH").unwrap_or_default();
    let mut cmd = Command::new("bash");
    cmd.arg(workspace_helper_path("workflow_publish_pr.sh"))
        .current_dir(&f.wt)
        .env_clear()
        .env("PATH", format!("{}:{old_path}", f.bin.display()))
        .env("HOME", &f.home)
        .env("TMPDIR", &f.home)
        .env("REMOTE_HOST_TYPE", "github")
        .env("ISSUE_NUMBER", "7")
        .env(
            "WORKFLOW_RUNTIME_ARTIFACT_HELPER",
            workspace_helper_path("workflow_runtime_artifacts.sh"),
        );
    crate::isolated_amplihack::no_git_prompt(&mut cmd);
    cmd.output().expect("run workflow_publish_pr.sh")
}

fn make_runtime_files(wt: &Path) {
    write(&wt.join(".amplihack/graph_db"), "");
    write(&wt.join(".amplihack/indexing.pid"), "1\n");
    write(&wt.join(".amplihack/index.scip"), "");
    write(&wt.join(".claude/runtime/logs/x"), "");
}

fn message(out: &Output) -> String {
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout
        .lines()
        .rev()
        .find(|l| l.trim_start().starts_with('{'))
        .unwrap_or_else(|| panic!("no JSON result in stdout:\n{stdout}"));
    let v: serde_json::Value =
        serde_json::from_str(line).unwrap_or_else(|e| panic!("stdout JSON invalid ({e}): {line}"));
    v["message"].as_str().unwrap_or_default().to_string()
}

fn state(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

const FIRST_SENTENCE: &str = "dirty worktree blocks no-diff or obsolete terminal success; commit or discard changes explicitly";

#[test]
fn runtime_only_untracked_files_reach_the_pr_stage() {
    // The #851 reproduction.
    let f = fixture();
    make_runtime_files(&f.wt);
    let out = run_step16(&f);
    assert!(
        !state(&out).contains("FAILED_DIRTY_WORKTREE"),
        "runtime-only files must not fail as dirty\nstdout:\n{}\nstderr:\n{}",
        state(&out),
        String::from_utf8_lossy(&out.stderr)
    );
    let log = fs::read_to_string(&f.gh_log).unwrap_or_default();
    assert!(!log.trim().is_empty(), "step 16 must reach a gh call");
}

#[test]
fn untracked_and_modified_product_files_are_named() {
    let f = fixture();
    make_runtime_files(&f.wt);
    write(&f.wt.join("new.rs"), "fn a() {}\n");
    write(&f.wt.join("feature.txt"), "changed\n");
    let out = run_step16(&f);
    assert_eq!(out.status.code(), Some(1));
    assert!(state(&out).contains(r#""state":"FAILED_DIRTY_WORKTREE""#));
    let msg = message(&out);
    assert!(
        msg.starts_with(FIRST_SENTENCE),
        "first sentence kept: {msg}"
    );
    assert!(msg.contains("?? new.rs"), "names untracked file: {msg}");
    assert!(msg.contains(" M feature.txt"), "names modified file: {msg}");
    assert!(!msg.contains(".amplihack"), "runtime paths absent: {msg}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("new.rs") && stderr.contains("feature.txt"));
    for banned in ["git clean", "reset --hard", "checkout ."] {
        assert!(!msg.contains(banned), "advice must not name `{banned}`");
    }
    assert!(
        msg.contains("add its path to .git/info/exclude"),
        "guidance names the exclude route for tool output: {msg}"
    );
    assert!(
        msg.contains("continue from step 16"),
        "guidance says how to continue: {msg}"
    );
    assert!(msg.contains("Commit each file that belongs to the change"));
}

#[test]
fn step16_excludes_every_runtime_path_through_info_exclude() {
    // Case (c): git itself attributes each runtime path to info/exclude.
    let f = fixture();
    make_runtime_files(&f.wt);
    let _ = run_step16(&f);
    for p in [
        ".amplihack/graph_db",
        ".amplihack/indexing.pid",
        ".amplihack/index.scip",
        ".amplihack/step-x",
        ".claude/runtime/x",
        "index.scip",
    ] {
        let out = amplihack_git::command_in(&f.wt)
            .args(["check-ignore", "-v", "--", p])
            .output()
            .expect("git check-ignore");
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        assert!(out.status.success(), "{p} must be ignored: {text}");
        assert!(
            text.contains("info/exclude"),
            "{p} ignored via info/exclude: {text}"
        );
    }
}

#[test]
fn more_than_twenty_entries_are_capped() {
    let f = fixture();
    for i in 0..25 {
        write(&f.wt.join(format!("f{i:02}.txt")), "x\n");
    }
    let msg = message(&run_step16(&f));
    assert_eq!(msg.matches("?? f").count(), 20, "{msg}");
    assert!(msg.contains("(+5 more)"), "{msg}");
}

#[test]
fn hostile_filenames_are_sanitised_and_json_stays_valid() {
    let f = fixture();
    write(&f.wt.join("q\"uote\\back\u{202e}rtl.txt"), "x\n");
    let out = run_step16(&f);
    let msg = message(&out); // panics when the JSON is invalid
    assert!(msg.contains("q\"uote\\back"), "{msg}");
    assert!(!msg.contains('\u{202e}'), "bidi override must be stripped");
}

#[test]
fn long_filenames_are_cut_with_a_marker() {
    let f = fixture();
    write(&f.wt.join(format!("{}.txt", "a".repeat(250))), "x\n");
    let msg = message(&run_step16(&f));
    assert!(msg.contains("..."), "{msg}");
    assert!(!msg.contains(&"a".repeat(250)), "entry must be cut");
}

#[test]
fn token_shaped_filenames_are_redacted() {
    let f = fixture();
    let fake = "ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    write(&f.wt.join(format!("{fake}.txt")), "x\n");
    let out = run_step16(&f);
    let all = format!("{}{}", state(&out), String::from_utf8_lossy(&out.stderr));
    assert!(!all.contains(fake), "token-shaped name must be redacted");
}

#[test]
fn a_filename_with_a_newline_is_one_entry() {
    let f = fixture();
    write(&f.wt.join("two\nlines.txt"), "x\n");
    let msg = message(&run_step16(&f));
    assert!(
        !msg.contains('\n'),
        "no raw newline in the message: {msg:?}"
    );
    assert_eq!(msg.matches("?? ").count(), 1, "{msg}");
}
