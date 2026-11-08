//! P1 (#1554): the reference pass-through route inherits the caller's Git
//! selectors, as 918c's `STRICT_PATHS_PY` did (`subprocess.run(['git', ...])`
//! with the whole inherited environment); the bounded checkpoint route still
//! refuses them with status 2 before any Git child starts.
//!
//! Oracle: `git show 918c256cf:amplifier-bundle/tools/workflow_stage_changes.sh`
//! lines 69-124, run under `python3 -I` in `~/.amplihack/work` on 2026-11-07
//! (Python 3.14.10, Git 2.56.0). Probe (a): with GIT_INDEX_FILE=alt, status 0,
//! `alt` lists the path and the caller's index is untouched. Probe (b): cwd in
//! A with GIT_DIR/GIT_WORK_TREE naming B, status 0, B has the path staged and A
//! is unchanged.
//!
//! Selectors are set on the child `Command` only, after the hermetic
//! environment, because `Sandbox::command` removes all 17 of them.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use crate::workflow_checkpoint_support::*;

/// A repository with one commit, `gc.auto=0` and `core.fsmonitor=false` so no
/// background work changes `.git` between the two snapshots.
fn quiet_repo(sb: &Sandbox, name: &str) -> PathBuf {
    let dir = sb.path().join(name);
    fs::create_dir_all(&dir).unwrap();
    git(sb, &dir, &["init", "-q", "-b", "main"]);
    git(sb, &dir, &["config", "gc.auto", "0"]);
    git(sb, &dir, &["config", "core.fsmonitor", "false"]);
    fs::write(dir.join("seed"), "seed\n").unwrap();
    git(sb, &dir, &["add", "seed"]);
    git(sb, &dir, &["commit", "-q", "-m", "seed"]);
    dir
}

fn nul_list(sb: &Sandbox, name: &str, paths: &[&str]) -> PathBuf {
    let list = sb.tmp().join(name);
    let mut raw = Vec::new();
    for p in paths {
        raw.extend_from_slice(p.as_bytes());
        raw.push(0);
    }
    fs::write(&list, raw).unwrap();
    list
}

fn staged(sb: &Sandbox, repo: &Path, index: Option<&Path>) -> String {
    let mut c = sb.command(which("git"));
    c.current_dir(repo).args(["ls-files", "--stage"]);
    if let Some(index) = index {
        c.env("GIT_INDEX_FILE", index);
    }
    let out = c.output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    String::from_utf8(out.stdout).unwrap()
}

fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    tree_snapshot(dir)
}

/// The engine's direct route.
fn direct(sb: &Sandbox, cwd: &Path, list: &Path, selectors: &[(&str, &Path)]) -> Output {
    let mut c = sb.checkpoint("stage-strict", [list.as_os_str()]);
    c.current_dir(cwd);
    for (k, v) in selectors {
        c.env(k, v);
    }
    c.output().unwrap()
}

/// `stage_workflow_changes --strict --path-list LIST` through the wrapper.
fn wrapper(sb: &Sandbox, cwd: &Path, list: &Path, selectors: &[(&str, &Path)]) -> Output {
    let script = tools_dir().join("workflow_stage_changes.sh");
    let mut c = sb.command("bash");
    c.arg("-c")
        .arg(r#"source "$1"; stage_workflow_changes --strict --path-list "$2""#)
        .arg("p1")
        .arg(&script)
        .arg(list)
        .current_dir(cwd);
    for (k, v) in selectors {
        c.env(k, v);
    }
    c.output().unwrap()
}

type Route = fn(&Sandbox, &Path, &Path, &[(&str, &Path)]) -> Output;

// ------------------------------------------------------------- probe (a)

fn probe_a(route: Route) {
    let sb = Sandbox::new();
    let a = quiet_repo(&sb, "A");
    fs::write(a.join("new.txt"), "new\n").unwrap();
    let alt = sb.path().join("alt-index");
    let list = nul_list(&sb, "list", &["new.txt"]);

    let control = snapshot(&a);
    assert_eq!(control, snapshot(&a), "no-op control: A must be stable");
    let before_index = staged(&sb, &a, None);

    let out = route(&sb, &a, &list, &[("GIT_INDEX_FILE", alt.as_path())]);
    assert_eq!(status(&out), 0, "918c exit 0; stderr: {}", stderr(&out));
    assert!(alt.is_file(), "the alternate index must be written");
    assert!(
        staged(&sb, &a, Some(&alt)).contains("\tnew.txt\n"),
        "alt must list new.txt"
    );
    assert_eq!(before_index, staged(&sb, &a, None), "A's own index changed");
    assert_eq!(control, snapshot(&a), "A's .git or worktree changed");
}

#[test]
fn p1a_binary_stage_strict_writes_only_the_inherited_alternate_index() {
    probe_a(direct);
}

#[test]
fn p1a_wrapper_stage_strict_writes_only_the_inherited_alternate_index() {
    probe_a(wrapper);
}

// ------------------------------------------------------------- probe (b)

fn probe_b(route: Route) {
    let sb = Sandbox::new();
    let a = quiet_repo(&sb, "A");
    let b = quiet_repo(&sb, "B");
    fs::write(b.join("b.txt"), "b\n").unwrap();
    let list = nul_list(&sb, "list", &["b.txt"]);
    let b_git = b.join(".git");

    let a_before = snapshot(&a);
    let out = route(
        &sb,
        &a,
        &list,
        &[("GIT_DIR", b_git.as_path()), ("GIT_WORK_TREE", b.as_path())],
    );
    assert_eq!(status(&out), 0, "918c exit 0; stderr: {}", stderr(&out));
    assert!(
        staged(&sb, &b, None).contains("\tb.txt\n"),
        "B must have b.txt staged"
    );
    assert_eq!(a_before, snapshot(&a), "A must be byte-identical");
}

#[test]
fn p1b_binary_stage_strict_stages_the_repository_git_dir_selects() {
    probe_b(direct);
}

#[test]
fn p1b_wrapper_stage_strict_stages_the_repository_git_dir_selects() {
    probe_b(wrapper);
}

// ------------------------------------- every inherited selector reaches git

/// A `git` shim that records, for each call, which of the named variables
/// were set, then runs the real git with the unscrubbed environment.
fn env_logging_git(sb: &Sandbox, names: &[&str]) -> PathBuf {
    let log = sb.path().join("git-env.log");
    let mut probes = String::new();
    for n in names {
        probes.push_str(&format!("printf '{n}=%s\\n' \"${{{n}-<unset>}}\" >> '{}'\n", log.display()));
    }
    write_exec(
        &sb.shims().join("git"),
        &format!(
            "#!/bin/bash\nprintf -- '--- %s\\n' \"$*\" >> '{log}'\n{probes}exec '{real}' \"$@\"\n",
            log = log.display(),
            real = which("git").display(),
        ),
    );
    log
}

#[test]
fn stage_strict_passes_non_retargeting_selectors_to_every_git_child() {
    // GIT_COMMON_DIR etc. are inherited verbatim by 918c; their values here
    // are chosen so git still resolves the same repository.
    let sb = Sandbox::new();
    let a = quiet_repo(&sb, "A");
    fs::write(a.join("n.txt"), "n\n").unwrap();
    let list = nul_list(&sb, "list", &["n.txt"]);
    let objects = a.join(".git/objects");
    let ceiling = sb.path().to_path_buf();
    let names = [
        "GIT_COMMON_DIR",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_NAMESPACE",
        "GIT_CEILING_DIRECTORIES",
    ];
    let log = env_logging_git(&sb, &names);
    let common = a.join(".git");
    let empty_alt = sb.path().join("no-alternates");
    fs::create_dir_all(&empty_alt).unwrap();
    let out = sb
        .checkpoint("stage-strict", [list.as_os_str()])
        .current_dir(&a)
        .env("GIT_COMMON_DIR", &common)
        .env("GIT_OBJECT_DIRECTORY", &objects)
        .env("GIT_ALTERNATE_OBJECT_DIRECTORIES", &empty_alt)
        .env("GIT_NAMESPACE", "p1probe")
        .env("GIT_CEILING_DIRECTORIES", &ceiling)
        .output()
        .unwrap();
    assert_eq!(status(&out), 0, "stderr: {}", stderr(&out));
    let calls = fs::read_to_string(&log).expect("git was called through the shim");
    assert!(calls.contains("--- --literal-pathspecs"), "{calls}");
    for n in names {
        assert!(
            !calls.contains(&format!("{n}=<unset>")),
            "{n} was scrubbed before git ran:\n{calls}"
        );
    }
    assert!(calls.contains("GIT_NAMESPACE=p1probe"), "{calls}");
}

#[test]
fn stage_strict_does_not_set_git_terminal_prompt_when_the_caller_did_not() {
    // 918c parity: STRICT_PATHS_PY never set GIT_TERMINAL_PROMPT. The
    // difference, if ever made, belongs in the PR body, not in silence.
    let sb = Sandbox::new();
    let a = quiet_repo(&sb, "A");
    fs::write(a.join("t.txt"), "t\n").unwrap();
    let list = nul_list(&sb, "list", &["t.txt"]);
    let log = env_logging_git(&sb, &["GIT_TERMINAL_PROMPT"]);
    let out = sb
        .checkpoint("stage-strict", [list.as_os_str()])
        .current_dir(&a)
        .env_remove("GIT_TERMINAL_PROMPT")
        .output()
        .unwrap();
    assert_eq!(status(&out), 0, "stderr: {}", stderr(&out));
    let calls = fs::read_to_string(&log).unwrap();
    assert!(
        calls
            .lines()
            .filter(|l| l.starts_with("GIT_TERMINAL_PROMPT="))
            .all(|l| l == "GIT_TERMINAL_PROMPT=<unset>"),
        "stage-strict added GIT_TERMINAL_PROMPT:\n{calls}"
    );
}

#[test]
fn stage_strict_pathspec_mode_conflict_reproduces_git_status_and_stderr() {
    // GIT_GLOB_PATHSPECS with --literal-pathspecs is a Git usage error; 918c
    // passed it through, so Git's own message reaches stderr and the strict
    // refusal (2) follows.
    let sb = Sandbox::new();
    let a = quiet_repo(&sb, "A");
    fs::write(a.join("g.txt"), "g\n").unwrap();
    let list = nul_list(&sb, "list", &["g.txt"]);
    let before = snapshot(&a);
    let out = sb
        .checkpoint("stage-strict", [list.as_os_str()])
        .current_dir(&a)
        .env("GIT_GLOB_PATHSPECS", "1")
        .output()
        .unwrap();
    assert_eq!(status(&out), 2, "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("global 'glob' and 'literal' pathspec settings are incompatible"),
        "Git's own diagnostic must pass through: {}",
        stderr(&out)
    );
    assert!(stderr(&out).contains("stage_workflow_changes: strict refusal: "));
    assert_eq!(before, snapshot(&a));
}

// ------------------------------------------------- bounded route twins

#[test]
fn bounded_route_still_refuses_each_p1_selector_with_status_2_and_writes_nothing() {
    let sb = Sandbox::new();
    let a = quiet_repo(&sb, "A");
    let b = quiet_repo(&sb, "B");
    let alt = sb.path().join("alt-index");
    let marker = sb.path().join("git-was-called");
    write_exec(
        &sb.shims().join("git"),
        &format!(
            "#!/bin/sh\n: > '{}'\nexec '{}' \"$@\"\n",
            marker.display(),
            which("git").display()
        ),
    );
    let b_git = b.join(".git");
    let cases: [(&str, &[(&str, &Path)]); 2] = [
        ("probe-a", &[("GIT_INDEX_FILE", alt.as_path())]),
        (
            "probe-b",
            &[("GIT_DIR", b_git.as_path()), ("GIT_WORK_TREE", b.as_path())],
        ),
    ];
    for (name, selectors) in cases {
        let a_before = snapshot(&a);
        let b_before = snapshot(&b);
        let mut c = sb.checkpoint("selectors", std::iter::empty::<&str>());
        c.current_dir(&a);
        for (k, v) in selectors {
            c.env(k, v);
        }
        let out = c.output().unwrap();
        assert_eq!(status(&out), 2, "{name}: {}", stderr(&out));
        assert!(
            stderr(&out).contains("unsupported inherited Git selector"),
            "{name}: {}",
            stderr(&out)
        );
        assert!(!marker.exists(), "{name}: a git child started");
        assert!(!alt.exists(), "{name}: alternate index written");
        assert_eq!(a_before, snapshot(&a), "{name}: A changed");
        assert_eq!(b_before, snapshot(&b), "{name}: B changed");
    }
}

// ------------------------------------------------------------ hook leak

#[test]
fn a_bogus_git_dir_in_the_parent_environment_never_reaches_a_test_child() {
    // pre-push exports GIT_DIR into cargo test. The harness must drop it, and
    // the stage-strict tests must set selectors only on their own child.
    let sb = Sandbox::new();
    let bogus = quiet_repo(&sb, "bogus");
    let a = quiet_repo(&sb, "A");
    fs::write(a.join("h.txt"), "h\n").unwrap();
    let list = nul_list(&sb, "list", &["h.txt"]);
    let bogus_before = snapshot(&bogus);
    // Simulate the hook: put GIT_DIR on a parent Command, then build the
    // child from the hermetic sandbox exactly as every P1 test does.
    let mut parent = Command::new("env");
    parent.env("GIT_DIR", bogus.join(".git"));
    let inherited = parent
        .get_envs()
        .any(|(k, v)| k == "GIT_DIR" && v.is_some());
    assert!(inherited, "precondition: the parent carries GIT_DIR");
    let child = sb.checkpoint("stage-strict", [list.as_os_str()]);
    assert!(
        child
            .get_envs()
            .any(|(k, v)| k == "GIT_DIR" && v.is_none()),
        "Sandbox::command must remove GIT_DIR explicitly"
    );
    let out = sb
        .checkpoint("stage-strict", [list.as_os_str()])
        .current_dir(&a)
        .output()
        .unwrap();
    assert_eq!(status(&out), 0, "stderr: {}", stderr(&out));
    assert!(staged(&sb, &a, None).contains("\th.txt\n"));
    assert_eq!(bogus_before, snapshot(&bogus), "the bogus repository was written");
}

// ------------------------------------------- Selectors::Ignored row audit

/// Every Selectors::Ignored row other than stage-strict is Git-free at 918c
/// (mod.rs rows for advertised-ref, sha256, admission-refused-result,
/// cleanup-skip-result, worktree-path). A failing `git` that leaves a marker
/// proves no hidden helper starts Git.
#[test]
fn git_free_ignored_rows_never_start_git() {
    let sb = Sandbox::new();
    let marker = sb.path().join("git-was-called");
    write_exec(
        &sb.shims().join("git"),
        &format!("#!/bin/sh\n: > '{}'\nexit 97\n", marker.display()),
    );
    let advert = sb.tmp().join("advert");
    fs::write(&advert, format!("{}\trefs/heads/x\n", oid1('a'))).unwrap();
    let sha_input = sb.tmp().join("sha-input");
    fs::write(&sha_input, b"abc").unwrap();
    let setup = r#"{"branch_name": "feat/x", "worktree_path": "/w"}"#;
    let rows: Vec<(&str, Vec<std::ffi::OsString>)> = vec![
        (
            "advertised-ref",
            vec![advert.clone().into(), "refs/heads/x".into(), "0".into()],
        ),
        ("sha256", vec![sha_input.clone().into()]),
        ("admission-refused-result", vec![]),
        ("cleanup-skip-result", vec!["reason".into()]),
        ("worktree-path", vec![]),
    ];
    for (action, args) in rows {
        let _ = fs::remove_file(&marker);
        let out = sb
            .checkpoint(action, &args)
            .env("RECIPE_VAR_worktree_setup", setup)
            .env("GIT_DIR", sb.path().join("nowhere"))
            .output()
            .unwrap();
        assert!(
            !marker.exists(),
            "{action} started git (status {}, stderr {})",
            status(&out),
            stderr(&out)
        );
    }
}
