//! Issue #2582 (Crusty B2): the loop-health evaluator call in
//! `amplifier-bundle/tools/autodrive_loop.sh` passes agent output and log text
//! by file, never on the nested child's command line.
//!
//! The evaluator branch for a non-crusty loop used to pass the round history,
//! the round log tail, the findings and the test and CI signals as
//! `-c key=<text>`, so every byte of that text was in the argv of the nested
//! `amplihack recipe run`. Each of these eight keys now goes through
//! `--context-from-file key=<file>`, where the file is 0600 inside a 0700
//! `mktemp -d` folder under the state directory, removed by literal path once
//! the evaluator has returned:
//!
//! `loop_history`, `loop_last_round_output`, `loop_findings_current`,
//! `loop_findings_previous`, `loop_test_signal`, `loop_test_signal_previous`,
//! `loop_ci_signal`, `loop_ci_signal_previous`.
//!
//! The loop is driven end to end with a stub `amplihack` first on `PATH`
//! (the harness of `issue_1570_autodrive.rs`, copied, not shared). Sentinels
//! are planted in every source:
//! - the findings, in the round record's `.findings` file the stub round writes;
//! - the round log, in the stub round's stdout;
//! - the history, through the before-round step's `before_round_result`;
//! - the test and CI signals, through the record's `test_signal` and
//!   `ci_signal` fields.
//!
//! When the stub is called as the evaluator it copies `/proc/$$/cmdline` (its
//! own process: the nested child the loop exec'd) and every file named by
//! `--context-from-file` into the test's temporary directory, with the file
//! and folder modes, then answers. The test synchronises on the loop process
//! exiting (`Command::output`); there is no timer and no signal.
//!
//! The pinned recipe runner does not report `context_file_v1`, so a real
//! nested `amplihack recipe run` still hands these values to the runner as
//! `--set`. That exposure ends when rysweet/amplihack-recipe-runner#211 is
//! released and the pin is moved; it is out of scope here. This test covers
//! the hop from the loop to the nested `amplihack`.

#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::Command;

/// The keys that must reach the evaluator by file only.
const MOVED_KEYS: &[&str] = &[
    "loop_history",
    "loop_last_round_output",
    "loop_findings_current",
    "loop_findings_previous",
    "loop_test_signal",
    "loop_test_signal_previous",
    "loop_ci_signal",
    "loop_ci_signal_previous",
];

const FINDINGS_WORD: &str = "ZQXV2582FINDWORD";
const LOG_WORD: &str = "ZQXV2582LOGWORD";
const HISTORY_WORD: &str = "ZQXV2582HISTWORD";
const TEST_WORD: &str = "ZQXV2582TESTWORD";
const CI_WORD: &str = "ZQXV2582CIWORD";
const SENTINELS: &[&str] = &[FINDINGS_WORD, LOG_WORD, HISTORY_WORD, TEST_WORD, CI_WORD];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root is two levels above the crate manifest")
        .to_path_buf()
}

const STUB: &str = r#"#!/usr/bin/env bash
set -u
# The loop pipes one `orch helper` call into another, so two stubs can run at
# once; take the call number under a lock so no two calls share a log file.
exec 9>"$STUB_STATE/calls.lock"
flock 9
n=$(cat "$STUB_STATE/calls" 2>/dev/null || echo 0)
n=$((n + 1))
printf '%s' "$n" > "$STUB_STATE/calls"
flock -u 9
exec 9>&-
[ "${1:-}" = recipe ] && [ $# -ge 3 ] && set -- "$1" "$2" "$(basename -- "$3" .yaml)" "${@:4}"
printf '%s\0' "$@" > "$STUB_LOG/call-$(printf '%05d' "$n")"
case "${1:-} ${2:-}" in
  "recipe show") exit 0 ;;
  "orch helper")
    cat >/dev/null
    if [ "${3:-}" = "extract-field" ]; then
      field=
      prev=
      for a in "$@"; do
        [ "$prev" = "--field" ] && field=$a
        prev=$a
      done
      case "$field" in
        test_signal) printf '%s' "ZQXV2582TESTWORD tests: 3 failed"; exit 0 ;;
        ci_signal) printf '%s' "ZQXV2582CIWORD ci: red"; exit 0 ;;
        before_round_result) printf '%s' "ZQXV2582HISTWORD"; exit 0 ;;
      esac
      while [ $# -gt 0 ]; do
        if [ "$1" = "--default" ]; then printf '%s' "${2:-}"; exit 0; fi
        shift
      done
    fi
    exit 0 ;;
  "recipe run")
    if [ "${3:-}" = "loop-health-evaluator" ]; then
      h=$(cat "$STUB_STATE/health" 2>/dev/null || echo 0)
      h=$((h + 1))
      printf '%s' "$h" > "$STUB_STATE/health"
      d="$STUB_STATE/eval-$h"
      mkdir -p -- "$d/ctx"
      # $$ is this process, the nested child the loop exec'd; /proc/self
      # would be the reader's own process.
      cat -- "/proc/$$/cmdline" > "$d/cmdline"
      prev=
      for a in "$@"; do
        spec=
        if [ "$prev" = "--context-from-file" ]; then spec=$a; fi
        case "$a" in --context-from-file=*) spec=${a#--context-from-file=} ;; esac
        if [ -n "$spec" ]; then
          k=${spec%%=*}
          f=${spec#*=}
          cp -- "$f" "$d/ctx/$k"
          stat -c %a -- "$f" > "$d/ctx/$k.mode"
          dir=$(dirname -- "$f")
          stat -c %a -- "$dir" > "$d/ctx/$k.dirmode"
          printf '%s' "$dir" > "$d/ctx/$k.dir"
        fi
        prev=$a
      done
      # The loop trusts only the runner's step-04 status block (#1512).
      if [ "$h" -lt 2 ]; then
        printf '  \342\234\223 step-04-enforce-loop-verdict: completed\n    Output: LOOP_HEALTH: CONTINUE\n'
        exit 0
      fi
      echo "LOOP_HEALTH: STUCK"; exit 1
    fi
    prev=
    for a in "$@"; do
      if [ "$prev" = "--working-dir" ]; then
        t="$(git -C "$a" write-tree)"
        c="$(git -C "$a" -c user.name=t -c user.email=t@t commit-tree "$t" -p HEAD -m "stub round $n")"
        git -C "$a" update-ref HEAD "$c"
      fi
      if [ "$prev" = "-c" ]; then
        case "$a" in
          autodrive_round_record=*)
            r=${a#autodrive_round_record=}
            printf '%s\n' 'ZQXV2582FINDWORD finding one $(touch pwned)' 'second finding `touch pwned2`' > "$r.findings" ;;
        esac
      fi
      prev=$a
    done
    echo "ZQXV2582LOGWORD round output"; exit 0 ;;
esac
exit 0
"#;

/// #1743: a stop decided in code posts a PR comment; this one goes nowhere.
const GH_STUB: &str =
    "#!/usr/bin/env bash\nprintf '%s\\0' \"$@\" >> \"$STUB_STATE/gh-calls\"\nexit 0\n";

/// The before-round step: succeeds; its result field is the stub's sentinel.
const BEFORE_ROUND: &str = "#!/usr/bin/env bash\nprintf '{}\\n'\nexit 0\n";

struct Run {
    state_dir: PathBuf,
    stub_state: PathBuf,
    status: std::process::ExitStatus,
    stderr: String,
    cwd: PathBuf,
    _tmp: tempfile::TempDir,
}

/// `git init` `main` with one commit (plumbing, so no hook runs), then add
/// `worktree` as a detached linked worktree of it.
fn main_checkout_with_worktree(home: &Path, main: &Path, worktree: &Path) {
    let git = |args: &[&str]| {
        let out = amplihack_git::command_in(main)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .env("HOME", home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("run git");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    git(&["init", "-q"]);
    let tree = git(&["write-tree"]);
    let commit = git(&["commit-tree", &tree, "-m", "init"]);
    git(&["update-ref", "HEAD", &commit]);
    git(&[
        "worktree",
        "add",
        "-q",
        "--detach",
        &worktree.display().to_string(),
    ]);
}

/// Runs the merge-ready loop for two rounds: the stub evaluator says CONTINUE
/// after round 1 and STUCK after round 2.
fn drive_two_rounds() -> Run {
    let tmp = tempfile::tempdir().unwrap();
    let bin = tmp.path().join("bin");
    let log = tmp.path().join("argv");
    let stub_state = tmp.path().join("stub-state");
    let state_dir = tmp.path().join("state");
    let repo = tmp.path().join("repo");
    let cwd = tmp.path().join("cwd");
    let task_file = tmp.path().join("task-in");
    let before = tmp.path().join("before-round.sh");
    std::fs::write(&task_file, "fix the widget\n").unwrap();
    std::fs::write(&before, BEFORE_ROUND).unwrap();
    for d in [&bin, &log, &stub_state, &cwd] {
        std::fs::create_dir_all(d).unwrap();
    }
    main_checkout_with_worktree(tmp.path(), &cwd, &repo);
    crate::support::exec_stub::write_executable(&bin.join("amplihack"), STUB, 0o755);
    crate::support::exec_stub::write_executable(&bin.join("gh"), GH_STUB, 0o755);

    let script = repo_root().join("amplifier-bundle/tools/autodrive_loop.sh");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into())
    );
    let out = Command::new("bash")
        .arg(&script)
        .args(["--loop-name", "merge-ready"])
        .args(["--round-recipe", "autodrive-merge-round"])
        .args(["--clean-token", "MERGE_READY"])
        .args(["--verdict-field", "merge_ready_verdict"])
        .arg("--repo")
        .arg(&repo)
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("--task-file")
        .arg(&task_file)
        .arg("--before-round")
        .arg(&before)
        .args(["--context", "repo_path=."])
        .args(["--context", "pr_number=2582"])
        .env_clear()
        .env("PATH", path)
        .env("HOME", tmp.path())
        .env("STUB_LOG", &log)
        .env("STUB_STATE", &stub_state)
        .current_dir(&cwd)
        .output()
        .expect("spawn autodrive_loop.sh");

    Run {
        state_dir,
        stub_state,
        status: out.status,
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        cwd,
        _tmp: tmp,
    }
}

fn eval_dir(run: &Run, call: usize) -> PathBuf {
    let dir = run.stub_state.join(format!("eval-{call}"));
    assert!(
        dir.is_dir(),
        "evaluator call {call} never ran; exit {:?}; stderr:\n{}",
        run.status.code(),
        run.stderr
    );
    dir
}

/// The nested evaluator child's argv, read from its `/proc/<pid>/cmdline`.
fn evaluator_cmdline(run: &Run, call: usize) -> Vec<String> {
    let raw = std::fs::read(eval_dir(run, call).join("cmdline")).unwrap();
    raw.split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect()
}

fn context_value(run: &Run, call: usize, key: &str) -> String {
    let path = eval_dir(run, call).join("ctx").join(key);
    std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "evaluator call {call}: {key} was not passed with --context-from-file ({e}); stderr:\n{}",
            run.stderr
        )
    })
}

fn context_meta(run: &Run, call: usize, key: &str, suffix: &str) -> String {
    let path = eval_dir(run, call)
        .join("ctx")
        .join(format!("{key}.{suffix}"));
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("evaluator call {call}: no {suffix} for {key} ({e})"))
        .trim()
        .to_string()
}

#[test]
fn evaluator_child_argv_holds_no_agent_text_or_moved_key() {
    let run = drive_two_rounds();
    assert_eq!(
        run.status.code(),
        Some(1),
        "the stub evaluator says STUCK after round 2, so the loop exits 1; stderr:\n{}",
        run.stderr
    );
    for call in 1..=2 {
        let argv = evaluator_cmdline(&run, call);
        assert!(
            argv.iter().any(|a| a == "recipe") && argv.iter().any(|a| a == "run"),
            "evaluator call {call}: /proc cmdline is not the nested recipe run: {argv:?}"
        );
        for word in SENTINELS {
            assert!(
                !argv.iter().any(|a| a.contains(word)),
                "evaluator call {call}: {word} is on the nested child's argv: {argv:?}"
            );
        }
        for key in MOVED_KEYS {
            // `--context-from-file key=<path>` is the expected form; only a
            // `-c key=<value>` pair would put the value itself on argv.
            let prefix = format!("{key}=");
            assert!(
                !argv
                    .windows(2)
                    .any(|w| w[0] == "-c" && w[1].starts_with(&prefix)),
                "evaluator call {call}: {key} is passed as -c on the nested child's argv: {argv:?}"
            );
            assert!(
                argv.windows(2)
                    .any(|w| w[0] == "--context-from-file" && w[1].starts_with(&prefix)),
                "evaluator call {call}: {key} is not passed with --context-from-file: {argv:?}"
            );
        }
        // The fixed identifiers stay on argv; only free text moved.
        assert!(
            argv.iter()
                .any(|a| a == &format!("loop_round_label=round-{call}")),
            "evaluator call {call}: the round label must still be passed as -c: {argv:?}"
        );
    }
}

#[test]
fn evaluator_context_files_hold_the_values() {
    let run = drive_two_rounds();
    for call in 1..=2 {
        for key in MOVED_KEYS {
            context_value(&run, call, key);
        }
        assert!(
            context_value(&run, call, "loop_history").contains(HISTORY_WORD),
            "evaluator call {call}: loop_history must hold the round history"
        );
        assert!(
            context_value(&run, call, "loop_last_round_output").contains(LOG_WORD),
            "evaluator call {call}: loop_last_round_output must hold the round log tail"
        );
        let findings = context_value(&run, call, "loop_findings_current");
        assert!(
            findings.contains(FINDINGS_WORD) && findings.contains("$(touch pwned)"),
            "evaluator call {call}: loop_findings_current must hold the findings verbatim: {findings:?}"
        );
        assert!(context_value(&run, call, "loop_test_signal").contains(TEST_WORD));
        assert!(context_value(&run, call, "loop_ci_signal").contains(CI_WORD));
    }
    // Round 2's previous values are round 1's.
    assert!(context_value(&run, 2, "loop_findings_previous").contains(FINDINGS_WORD));
    assert!(context_value(&run, 2, "loop_test_signal_previous").contains(TEST_WORD));
    assert!(context_value(&run, 2, "loop_ci_signal_previous").contains(CI_WORD));
    // Round 1 has no previous round; its previous values are empty files.
    assert_eq!(context_value(&run, 1, "loop_findings_previous"), "");
    assert!(
        !run.cwd.join("pwned").exists() && !run.cwd.join("pwned2").exists(),
        "agent text must never be evaluated"
    );
}

#[test]
fn evaluator_context_is_private_and_removed_after_the_evaluator_returns() {
    let run = drive_two_rounds();
    let state = run.state_dir.canonicalize().unwrap();
    let mut dirs = Vec::new();
    for call in 1..=2 {
        let mut call_dirs = std::collections::BTreeSet::new();
        for key in MOVED_KEYS {
            assert_eq!(
                context_meta(&run, call, key, "mode"),
                "600",
                "evaluator call {call}: {key} file must be 0600"
            );
            assert_eq!(
                context_meta(&run, call, key, "dirmode"),
                "700",
                "evaluator call {call}: {key} folder must be 0700"
            );
            call_dirs.insert(PathBuf::from(context_meta(&run, call, key, "dir")));
        }
        assert_eq!(
            call_dirs.len(),
            1,
            "evaluator call {call}: all eight files must share one private folder: {call_dirs:?}"
        );
        let dir = call_dirs.into_iter().next().unwrap();
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            name.starts_with(&format!("merge-ready-round-{call}-ctx.")),
            "evaluator call {call}: the folder must be a mktemp -d under the state dir named <loop>-<round>-ctx.XXXXXX: {}",
            dir.display()
        );
        assert_eq!(
            dir.parent().and_then(|p| p.canonicalize().ok()).as_deref(),
            Some(state.as_path()),
            "evaluator call {call}: the folder must be under the state dir"
        );
        dirs.push(dir);
    }
    assert_ne!(dirs[0], dirs[1], "each round gets its own folder");
    for dir in &dirs {
        assert!(
            !dir.exists(),
            "{} must be removed once the evaluator has returned",
            dir.display()
        );
    }
    let left: Vec<String> = std::fs::read_dir(&run.state_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains("-ctx."))
        .collect();
    assert!(left.is_empty(), "context folders left behind: {left:?}");
}
