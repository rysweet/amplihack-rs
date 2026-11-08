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
use std::sync::OnceLock;

use tempfile::TempDir;

use crate::workflow_checkpoint_support::*;

/// Repositories `A` and `B`, each with one commit, `gc.auto=0` and
/// `core.fsmonitor=false` so no background work changes `.git` between two
/// snapshots. Built once per test process (with the hermetic environment)
/// and copied into each test's sandbox by [`quiet_repo`].
fn template_pair() -> &'static Path {
    static TEMPLATE: OnceLock<TempDir> = OnceLock::new();
    TEMPLATE
        .get_or_init(|| {
            let dir = tempfile::Builder::new()
                .prefix("p1-template-")
                .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
                .unwrap();
            let sb = Sandbox::new();
            for name in ["A", "B"] {
                let repo = dir.path().join(name);
                fs::create_dir_all(&repo).unwrap();
                git(&sb, &repo, &["init", "-q", "-b", "main"]);
                git(&sb, &repo, &["config", "gc.auto", "0"]);
                git(&sb, &repo, &["config", "core.fsmonitor", "false"]);
                fs::write(repo.join("seed"), "seed\n").unwrap();
                git(&sb, &repo, &["add", "seed"]);
                git(&sb, &repo, &["commit", "-q", "-m", "seed"]);
            }
            dir
        })
        .path()
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        let kind = entry.file_type().unwrap();
        if kind.is_dir() {
            copy_tree(&entry.path(), &target);
        } else if kind.is_symlink() {
            std::os::unix::fs::symlink(fs::read_link(entry.path()).unwrap(), &target).unwrap();
        } else {
            fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// A private copy of template repository `B` for `name == "B"`, else of
/// `A`. The index is refreshed after the copy so its stat data matches the
/// new files and no later Git call rewrites it on its own.
fn quiet_repo(sb: &Sandbox, name: &str) -> PathBuf {
    let dir = sb.path().join(name);
    let source = if name == "B" { "B" } else { "A" };
    copy_tree(&template_pair().join(source), &dir);
    git(sb, &dir, &["update-index", "-q", "--refresh"]);
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
    // `git add` under an alternate index still writes the new blob into the
    // repository's object store. The 918c STRICT_PATHS_PY oracle does the
    // same (re-run 2026-11-08 from ~/.amplihack/work/pr2020-oracle/strict.py:
    // the only new file in A/.git is the loose object for new.txt). So A must
    // be byte-identical apart from exactly that one object: no index, ref,
    // config or worktree change.
    let blob = sb
        .command(which("git"))
        .current_dir(&a)
        .args(["hash-object", "--", "new.txt"])
        .output()
        .unwrap();
    let blob = String::from_utf8(blob.stdout).unwrap().trim().to_owned();
    let object = PathBuf::from(format!(".git/objects/{}/{}", &blob[..2], &blob[2..]));
    let mut after = snapshot(&a);
    assert!(
        after.remove(&object).is_some(),
        "the new blob is in A's store"
    );
    let fanout = object.parent().unwrap().to_path_buf();
    if !control.contains_key(&fanout) {
        after.remove(&fanout);
    }
    assert_eq!(control, after, "A changed beyond the new blob object");
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
        probes.push_str(&format!(
            "printf '{n}=%s\\n' \"${{{n}-<unset>}}\" >> '{}'\n",
            log.display()
        ));
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
    // Git's wording differs by version ("global 'glob' and 'literal'
    // pathspec settings are incompatible" in older releases, "global
    // 'literal' pathspec setting is incompatible with all other global
    // pathspec settings" in newer ones); the contract is that Git's own line
    // reaches stderr before the refusal.
    let err = stderr(&out);
    let git_line = err.lines().next().unwrap_or("");
    assert!(
        git_line.starts_with("fatal: global ")
            && git_line.contains("pathspec")
            && git_line.contains("incompatible"),
        "Git's own diagnostic must pass through: {err}"
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
        child.get_envs().any(|(k, v)| k == "GIT_DIR" && v.is_none()),
        "Sandbox::command must remove GIT_DIR explicitly"
    );
    let out = sb
        .checkpoint("stage-strict", [list.as_os_str()])
        .current_dir(&a)
        .output()
        .unwrap();
    assert_eq!(status(&out), 0, "stderr: {}", stderr(&out));
    assert!(staged(&sb, &a, None).contains("\th.txt\n"));
    assert_eq!(
        bogus_before,
        snapshot(&bogus),
        "the bogus repository was written"
    );
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
    let setup = r#"{"branch_name": "feat/x", "worktree_path": "/w"}"#;
    // (action, args, stdin, expected status, expected stdout). Each expected
    // value is the 918c snippet's output for the same input.
    type Row<'a> = (&'a str, Vec<std::ffi::OsString>, &'a [u8], i32, &'a [u8]);
    let rows: Vec<Row> = vec![
        (
            "advertised-ref",
            vec![advert.clone().into(), "refs/heads/x".into(), "0".into()],
            b"",
            0,
            b"",
        ),
        (
            "sha256",
            vec![],
            b"abc",
            0,
            b"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad\n",
        ),
        (
            "admission-refused-result",
            vec![],
            b"",
            0,
            b"{\"pushed\": \"false\", \"sha\": \"\", \"branch\": \"feat/x\", \"reason\": \"scope-admission-refused\"}\n",
        ),
        (
            "cleanup-skip-result",
            vec!["reason".into()],
            b"",
            0,
            b"{\"skipped\": true, \"admitted\": false, \"reason\": \"reason\"}\n",
        ),
        ("worktree-path", vec![], b"", 0, b"/w\n"),
    ];
    for (action, args, stdin, want_status, want_stdout) in rows {
        let _ = fs::remove_file(&marker);
        let mut child = sb
            .checkpoint(action, &args)
            .env("RECIPE_VAR_worktree_setup", setup)
            .env("GIT_DIR", sb.path().join("nowhere"))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        {
            use std::io::Write;
            let mut input = child.stdin.take().unwrap();
            input.write_all(stdin).unwrap();
        }
        let out = child.wait_with_output().unwrap();
        assert!(
            !marker.exists(),
            "{action} started git (status {}, stderr {})",
            status(&out),
            stderr(&out)
        );
        assert_eq!(status(&out), want_status, "{action}: {}", stderr(&out));
        assert_eq!(
            out.stdout,
            want_stdout,
            "{action}: stdout {:?}",
            String::from_utf8_lossy(&out.stdout)
        );
        assert!(out.stderr.is_empty(), "{action}: {}", stderr(&out));
    }
}

#[test]
fn a_parent_bash_env_that_sets_git_dir_has_no_effect_on_the_wrapper_route() {
    // If BASH_ENV reached the `bash -c` wrapper it would export GIT_DIR=bogus
    // before staging, and stage-strict would write the bogus repository.
    let sb = Sandbox::new();
    let bogus = quiet_repo(&sb, "bogus");
    let a = quiet_repo(&sb, "A");
    fs::write(a.join("e.txt"), "e\n").unwrap();
    let list = nul_list(&sb, "list", &["e.txt"]);
    let rc = sb.path().join("bash-env.sh");
    fs::write(
        &rc,
        format!(
            "export GIT_DIR='{}'\nexport GIT_WORK_TREE='{}'\n",
            bogus.join(".git").display(),
            bogus.display()
        ),
    )
    .unwrap();
    // The variables are removed by name, whatever the parent had.
    let probe = sb.command("bash");
    for name in ["BASH_ENV", "ENV"] {
        assert!(
            probe.get_envs().any(|(k, v)| k == name && v.is_none()),
            "checkpoint_cmd must remove {name}"
        );
    }
    // Control: with BASH_ENV handed to bash, the rc file really does retarget.
    let control = sb
        .command("bash")
        .env("BASH_ENV", &rc)
        .args(["-c", "printf %s \"$GIT_DIR\""])
        .output()
        .unwrap();
    assert_eq!(
        control.stdout,
        bogus.join(".git").as_os_str().as_encoded_bytes()
    );
    // The real route: no BASH_ENV reaches the wrapper.
    let bogus_before = snapshot(&bogus);
    let out = wrapper(&sb, &a, &list, &[]);
    assert_eq!(status(&out), 0, "stderr: {}", stderr(&out));
    assert!(staged(&sb, &a, None).contains("\te.txt\n"));
    assert_eq!(
        bogus_before,
        snapshot(&bogus),
        "the BASH_ENV repository was written"
    );
}

// ------------------------------------------- 17.126-stage-strict-79 (R2c)

/// A `git` placed first on PATH for the binary under test only. When the
/// Git subcommand (the first argument after the global options
/// `--literal-pathspecs`, `-C <dir>` and `-c <name=value>`) is `add`, it
/// records a marker and exits `code`; every other call runs the real git.
/// Fixture setup uses `which("git")` from the host PATH, so the fake never
/// affects setup.
fn add_exits_shim(sb: &Sandbox, code: i32) -> PathBuf {
    let marker = sb.path().join("fake-git-add-ran");
    write_exec(
        &sb.shims().join("git"),
        &format!(
            "#!/bin/sh\n\
             sub=\n\
             skip=\n\
             for a in \"$@\"; do\n\
             \x20 if [ -n \"$skip\" ]; then skip=; continue; fi\n\
             \x20 case \"$a\" in\n\
             \x20   -C|-c) skip=1 ;;\n\
             \x20   -*) ;;\n\
             \x20   *) sub=$a; break ;;\n\
             \x20 esac\n\
             done\n\
             if [ \"$sub\" = add ]; then : > '{marker}'; exit {code}; fi\n\
             exec '{real}' \"$@\"\n",
            marker = marker.display(),
            real = which("git").display(),
        ),
    );
    marker
}

/// HEAD and every ref of `repo`, as Git reports them.
fn refs(sb: &Sandbox, repo: &Path) -> String {
    let head = git(sb, repo, &["rev-parse", "HEAD"]);
    let all = git(
        sb,
        repo,
        &["for-each-ref", "--format=%(objectname) %(refname)"],
    );
    format!(
        "{}{}",
        String::from_utf8_lossy(&head.stdout),
        String::from_utf8_lossy(&all.stdout)
    )
}

const REFUSAL: &str = "stage_workflow_changes: strict refusal: ";

/// Pin test for ApprovedRow 17.126-stage-strict-79: a real Git exit 79 from
/// the staging child reaches the caller as 79, with no stdout and no
/// refusal line, and HEAD and the refs are unchanged.
#[test]
fn stage_strict_passes_a_real_git_79_through() {
    let sb = Sandbox::new();
    let a = quiet_repo(&sb, "A");
    fs::write(a.join("a.txt"), "a\n").unwrap();
    let list = nul_list(&sb, "list", &["a.txt"]);
    let refs_before = refs(&sb, &a);
    let marker = add_exits_shim(&sb, 79);

    let out = direct(&sb, &a, &list, &[]);

    assert!(marker.exists(), "the fake git add must have run");
    assert_eq!(status(&out), 79, "stderr: {}", stderr(&out));
    assert!(out.stdout.is_empty(), "no stdout on a non-zero status");
    assert!(!stderr(&out).contains(REFUSAL), "{}", stderr(&out));
    assert_eq!(refs(&sb, &a), refs_before);
}

/// Negative half: only a real 79 passes through. A refused list and every
/// other Git status (here 78, the bounded route's own status, and 128,
/// Git's fatal status) stay what 918c made them: exactly status 2 with the
/// strict refusal line as the last line of stderr.
#[test]
fn stage_strict_refuses_every_other_failure_with_exactly_2() {
    let sb = Sandbox::new();
    let a = quiet_repo(&sb, "A");
    let missing = nul_list(&sb, "missing", &["does-not-exist.txt"]);
    let out = direct(&sb, &a, &missing, &[]);
    assert_eq!(status(&out), 2, "stderr: {}", stderr(&out));
    assert_eq!(
        stderr(&out).lines().last(),
        Some(format!("{REFUSAL}missing untracked path").as_str())
    );

    fs::write(a.join("a.txt"), "a\n").unwrap();
    let list = nul_list(&sb, "list", &["a.txt"]);
    for code in [78, 128] {
        let marker = add_exits_shim(&sb, code);
        let _ = fs::remove_file(&marker);
        let out = direct(&sb, &a, &list, &[]);
        assert!(marker.exists(), "{code}: the fake git add must have run");
        assert_eq!(status(&out), 2, "{code}: stderr: {}", stderr(&out));
        let err = stderr(&out);
        let last = err.lines().last().unwrap_or_default();
        assert!(
            last.starts_with(REFUSAL)
                && last.ends_with(&format!("returned non-zero exit status {code}.")),
            "{code}: {err}"
        );
    }
}

// ------------------------- P1 edge: GIT_OBJECT_DIRECTORY with alternates

/// With GIT_OBJECT_DIRECTORY naming a private store and the repository's
/// own store as an alternate, the new blob goes to the private store only.
/// The caller's object store stays byte-identical; its index gains the path.
#[test]
fn stage_strict_writes_the_new_blob_to_the_inherited_object_directory() {
    let sb = Sandbox::new();
    let a = quiet_repo(&sb, "A");
    fs::write(a.join("a.txt"), "object-directory probe\n").unwrap();
    let list = nul_list(&sb, "list", &["a.txt"]);
    let alt = sb.path().join("alt-objects");
    fs::create_dir_all(&alt).unwrap();
    let own = a.join(".git/objects");
    let own_before = snapshot(&own);

    let out = direct(
        &sb,
        &a,
        &list,
        &[
            ("GIT_OBJECT_DIRECTORY", alt.as_path()),
            ("GIT_ALTERNATE_OBJECT_DIRECTORIES", own.as_path()),
        ],
    );

    assert_eq!(status(&out), 0, "918c exit 0; stderr: {}", stderr(&out));
    assert_eq!(
        own_before,
        snapshot(&own),
        "own object store must not change"
    );
    let alt_files = snapshot(&alt)
        .values()
        .filter(|v| v.as_slice() != b"<dir>")
        .count();
    assert!(
        alt_files >= 1,
        "the blob must be written to GIT_OBJECT_DIRECTORY"
    );
    assert!(staged(&sb, &a, None).contains("\ta.txt\n"));
}

// ------------------------------------------- R30: hostile BASH_ENV/ENV

/// checkpoint_cmd removes BASH_ENV and ENV from every child it builds, so a
/// hostile value inherited from the caller never reaches a shell. The
/// behavioural half is
/// `a_parent_bash_env_that_sets_git_dir_has_no_effect_on_the_wrapper_route`.
#[test]
fn checkpoint_cmd_removes_bash_env_and_env() {
    let sb = Sandbox::new();
    let c = checkpoint_cmd(&sb, "bash");
    let removed: Vec<String> = c
        .get_envs()
        .filter(|(_, v)| v.is_none())
        .map(|(k, _)| k.to_string_lossy().into_owned())
        .collect();
    for name in ["BASH_ENV", "ENV"] {
        assert!(
            removed.iter().any(|r| r == name),
            "checkpoint_cmd keeps {name}"
        );
    }
}
