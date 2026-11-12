//! Issue #2096: a run records the index-writing git commands its steps ran,
//! in `<run dir>/git-commands.jsonl`, and never writes trace output to a
//! place the caller chose.
//!
//! Also a synthetic stash/restore diagnostic for candidate 1; this does not
//! replay the real project hooks or establish the historical index writer.
use super::*;
use serde_json::Value;
use std::os::unix::fs::PermissionsExt;

/// The run folders made under the fixture's work root.
fn run_dirs(f: &Fixture) -> Vec<PathBuf> {
    let runs = f.root.join("work/runs");
    let read = match fs::read_dir(&runs) {
        Ok(read) => read,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => panic!("read {}: {error}", runs.display()),
    };
    read.map(|entry| {
        entry
            .unwrap_or_else(|error| panic!("list {}: {error}", runs.display()))
            .path()
    })
    .filter(|path| {
        fs::metadata(path)
            .unwrap_or_else(|error| panic!("inspect {}: {error}", path.display()))
            .is_dir()
    })
    .collect()
}

fn the_run_dir(f: &Fixture) -> PathBuf {
    let dirs = run_dirs(f);
    assert_eq!(dirs.len(), 1, "expected one run folder, found {dirs:?}");
    dirs.into_iter().next().unwrap()
}

fn record_of(run_dir: &Path) -> Vec<Value> {
    let path = run_dir.join("git-commands.jsonl");
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    text.lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("{l}: {e}")))
        .collect()
}

/// The entries whose subcommand, after `-C`/`-c` options, is `sub`.
fn with_subcommand<'a>(entries: &'a [Value], sub: &str) -> Vec<&'a Value> {
    entries
        .iter()
        .filter(|e| {
            let argv: Vec<&str> = e["argv"]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| a.as_str().unwrap())
                .collect();
            let mut i = 1;
            while i < argv.len() && (argv[i] == "-C" || argv[i] == "-c") {
                i += 2;
            }
            argv.get(i) == Some(&sub)
        })
        .collect()
}

#[test]
fn a_run_records_its_steps_git_add_with_directory_and_time() {
    let f = Fixture::new();
    succeeds(
        &f.command("rec")
            .env("ISSUE_2096_STAGE", "1")
            .output()
            .unwrap(),
    );
    let run_dir = the_run_dir(&f);
    let entries = record_of(&run_dir);
    let checkout = String::from_utf8(f.observed("rec", "run_checkout")).unwrap();

    let adds = with_subcommand(&entries, "add");
    assert_eq!(adds.len(), 1, "{entries:#?}");
    let add = adds[0];
    assert_eq!(
        add["argv"],
        serde_json::json!(["git", "add", "--", "run-output"])
    );
    assert_eq!(add["cwd"].as_str(), Some(checkout.as_str()));
    assert!(add["time"].is_string(), "{add}");
    assert!(add["sid"].is_string(), "{add}");
    assert_eq!(with_subcommand(&entries, "commit").len(), 1, "{entries:#?}");
    assert!(
        with_subcommand(&entries, "rev-parse").is_empty(),
        "read-only commands are not recorded: {entries:#?}"
    );

    assert!(
        !run_dir.join("git-trace2").exists(),
        "raw trace files were left"
    );
    let mode = fs::metadata(run_dir.join("git-commands.jsonl"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn a_failed_run_still_writes_its_record() {
    let f = Fixture::new();
    let out = f
        .command("rec-fail")
        .env("ISSUE_2096_STAGE", "1")
        .env("ISSUE_2096_FAIL", "1")
        .output()
        .unwrap();
    assert!(!out.status.success());
    let run_dir = the_run_dir(&f);
    assert_eq!(with_subcommand(&record_of(&run_dir), "add").len(), 1);
    assert!(!run_dir.join("git-trace2").exists());
}

#[test]
fn a_run_with_no_matching_command_writes_an_empty_record() {
    let f = Fixture::new();
    // Outside git: the stub runs no git command that writes an index.
    fs::remove_dir_all(f.repo.join(".git")).unwrap();
    succeeds(&f.run("rec-empty"));
    let run_dir = the_run_dir(&f);
    assert_eq!(fs::read(run_dir.join("git-commands.jsonl")).unwrap(), b"");
}

/// The caller's trace targets: amplihack's own git calls may still write
/// there, but nothing a step ran may.
fn assert_caller_targets_saw_no_step(f: &Fixture, out: &Output) {
    for name in ["caller-event", "caller-normal", "caller-perf"] {
        let path = f.root.join(name);
        let seen = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => panic!("read {}: {error}", path.display()),
        };
        assert!(
            !String::from_utf8_lossy(&seen).contains("run-output"),
            "a step's git command was traced to the caller's {name}\nstderr {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

fn with_caller_trace(f: &Fixture, mut c: Command) -> Command {
    c.env("GIT_TRACE2_EVENT", f.root.join("caller-event"))
        .env("GIT_TRACE2_ENV_VARS", "HOME")
        .env("GIT_TRACE2", f.root.join("caller-normal"))
        .env("GIT_TRACE2_PERF", f.root.join("caller-perf"))
        .env("ISSUE_2096_STAGE", "1");
    c
}

#[test]
fn the_callers_trace_targets_never_receive_a_steps_commands() {
    let f = Fixture::new();
    let out = with_caller_trace(&f, f.command("rec-caller"))
        .output()
        .unwrap();
    succeeds(&out);
    assert_caller_targets_saw_no_step(&f, &out);
    assert_eq!(
        with_subcommand(&record_of(&the_run_dir(&f)), "add").len(),
        1,
        "the run's own record still has the step's add"
    );
}

#[test]
fn a_run_without_durable_record_storage_is_rejected_before_execution() {
    let f = Fixture::new();
    // A relative work root cannot provide required recording storage.
    let mut c = with_caller_trace(&f, f.command("rec-legacy"));
    c.env("AMPLIHACK_WORK_ROOT", "relative/work");
    let out = c.output().unwrap();
    assert!(
        !out.status.success(),
        "recording cannot silently become optional: {out:?}"
    );
    assert!(
        !f.root.join("rec-legacy/cwd").exists(),
        "runner launched without mandatory storage"
    );
    assert!(run_dirs(&f).is_empty());
    assert_caller_targets_saw_no_step(&f, &out);
}

/// Model candidate 1's stash/restore sequence with a synthetic hook during a
/// commit in a linked worktree, while the main checkout has an untracked file.
///
/// Observed with git 2.56 on 10 Nov 2026, the hook's `git diff`,
/// `git checkout -- .` and `git apply` all ran with `GIT_INDEX_FILE` set by
/// git to the linked worktree's own index (`.git/worktrees/<name>/index`),
/// and the main checkout's index was byte-identical before and after. This
/// pins that synthetic observation, without ruling out the real hook chain
/// or identifying the historical writer. A regression names the commands
/// that ran with which index.
#[test]
fn candidate_one_precommit_stash_and_restore_in_a_worktree_leaves_the_main_index_alone() {
    let f = Fixture::new();
    fs::write(f.repo.join("untracked-in-main"), "u").unwrap();
    let wt = f.root.join("wt");
    f.git(&["worktree", "add", "-q", "-b", "feat", wt.to_str().unwrap()]);
    let wt = fs::canonicalize(wt).unwrap();
    let wt_index = PathBuf::from(
        String::from_utf8(f.git_in(
            &wt,
            &["rev-parse", "--path-format=absolute", "--git-path", "index"],
        ))
        .unwrap()
        .trim(),
    );

    // pre-commit's staged_files_only: save unstaged changes as a patch,
    // check out the index, run hooks, apply the patch back.
    let hook = f.repo.join(".git/hooks/pre-commit");
    crate::support::exec_stub::write_executable(
        &hook,
        r#"#!/bin/sh
set -e
patch="$(git rev-parse --git-dir)/stash.patch"
if ! git diff --ignore-submodules --binary --exit-code --no-color --no-ext-diff > "$patch"; then
  git checkout -- .
  : > "$(git rev-parse --git-dir)/hook-stashed"
  git apply "$patch"
fi
rm -f "$patch"
"#,
        0o755,
    );

    fs::write(wt.join("staged"), "s").unwrap();
    f.git_in(&wt, &["add", "staged"]);
    fs::write(wt.join("tracked"), "base\nunstaged\n").unwrap();

    let main_index = f.repo.join(".git/index");
    let before = fs::read(&main_index).unwrap();
    let trace = f.root.join("replay-trace");
    fs::create_dir(&trace).unwrap();
    let mut c = amplihack_git::command();
    f.environment(&mut c);
    let mut target = trace.clone().into_os_string();
    target.push("/");
    let out = c
        .current_dir(&wt)
        .env("GIT_TRACE2_EVENT", target)
        .env(
            "GIT_TRACE2_ENV_VARS",
            "GIT_DIR,GIT_INDEX_FILE,GIT_WORK_TREE,PWD",
        )
        .args(["commit", "-qm", "worktree commit"])
        .output()
        .unwrap();
    succeeds(&out);
    let after = fs::read(&main_index).unwrap();

    // The hook did take the stash path.
    let wt_git_dir = wt_index.parent().unwrap();
    assert!(
        wt_git_dir.join("hook-stashed").exists(),
        "the hook never stashed"
    );
    assert_eq!(
        fs::read_to_string(wt.join("tracked")).unwrap(),
        "base\nunstaged\n",
        "the hook restored the unstaged change"
    );

    // Which index each of the hook's git commands ran against.
    let mut indexes: std::collections::BTreeMap<String, (Vec<String>, Option<String>)> =
        Default::default();
    for file in fs::read_dir(&trace).unwrap() {
        let text = fs::read_to_string(file.unwrap().path()).unwrap();
        for line in text.lines() {
            let Ok(event) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            let sid = event["sid"].as_str().unwrap_or_default().to_string();
            match event["event"].as_str() {
                Some("start") => {
                    indexes.entry(sid).or_default().0 = event["argv"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|s| s.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default();
                }
                Some("def_param") if event["param"] == "GIT_INDEX_FILE" => {
                    indexes.entry(sid).or_default().1 = event["value"].as_str().map(String::from);
                }
                _ => {}
            }
        }
    }
    let hook_commands: Vec<_> = indexes
        .values()
        .filter(|(argv, _)| argv.get(1).is_some_and(|s| s == "checkout" || s == "apply"))
        .collect();
    assert_eq!(hook_commands.len(), 2, "{indexes:#?}");
    for (argv, index) in &hook_commands {
        assert_eq!(
            index.as_deref(),
            wt_index.to_str(),
            "{argv:?} ran against an index other than the worktree's"
        );
    }

    assert!(
        before == after,
        "candidate 1 reproduced: the main index changed; commands and their indexes: {indexes:#?}"
    );
}
