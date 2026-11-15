//! #2546 (PR #2579): auto-drive repairs its own state written under a loose
//! umask, and still refuses entries another user could change.
//!
//! Moved out of tests.rs when #2575 retired the parsed readiness record: these
//! tests use only the private state store and the completion checker.

use crate::commands::orch::autodrive::state::EvidenceStore;

// ── #2546: umask 002 and repair of our own state ──
//
// The umask is per process and sibling tests create files concurrently, so the
// umask tests run alone in a child of the test binary. The parent test process
// never changes its umask or its environment.
//
// B1 (PR #2579): repair is allowed by location, not by the folder's name. A
// state folder is repairable when it sits directly under the auto-drive state
// root, `${AMPLIHACK_STATE_DIR:-${HOME:-/tmp}/.amplihack/state}/auto-drive`,
// the root autodrive_state.sh uses. Every test that expects a repair therefore
// runs in a child whose HOME, AMPLIHACK_HOME and AMPLIHACK_STATE_DIR point at
// a temporary folder (`isolated_2546`), so no test touches the real
// ~/.amplihack or ~/.claude.

#[cfg(unix)]
const UMASK_2546_CHILD: &str = "AMPLIHACK_ISSUE_2546_UMASK_CHILD";

/// The temporary folder that holds the child's HOME, AMPLIHACK_HOME and
/// AMPLIHACK_STATE_DIR (set by the parent, read by the child).
#[cfg(unix)]
const ISO_2546: &str = "AMPLIHACK_ISSUE_2546_ISO";

/// The log line the evidence check writes for each repaired entry.
#[cfg(unix)]
const REPAIR_2546: &str = "removed group and other access";

/// The refusal for an entry with extra bits outside the state tree (row 7).
const OUTSIDE_2546: &str = "evidence outside the auto-drive state tree has group or other access";

/// The repository URL and PR number the real-layout tests use, and the slug
/// autodrive_state_key makes from them.
#[cfg(unix)]
const ORIGIN_2546: &str = "https://github.com/example/widgets.git";
#[cfg(unix)]
const PR_2546: &str = "2579";
#[cfg(unix)]
const SLUG_2546: &str = "https___github.com_example_widgets_2579";

#[cfg(unix)]
fn tools_2546() -> std::path::PathBuf {
    std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../amplifier-bundle/tools"
    ))
    .canonicalize()
    .unwrap()
}

/// Runs `body` in a child test process under `mask`, and returns its stderr.
///
/// `env` is applied to the child only: `Some` sets a variable, `None` removes
/// it. Every `GIT_*` variable the parent has (pre-commit and pre-push hooks
/// set GIT_DIR and GIT_INDEX_FILE) and AMPLIHACK_BIN are removed, and
/// GIT_CONFIG_NOSYSTEM=1 is set, so git in the child sees only its own repo.
#[cfg(unix)]
fn under_umask_2546(
    test: &str,
    mask: libc::mode_t,
    env: &[(&str, Option<std::ffi::OsString>)],
    body: impl FnOnce(),
) -> Option<String> {
    if std::env::var_os(UMASK_2546_CHILD).is_some() {
        // SAFETY: umask has no failure mode, and this child runs only this test.
        let old = unsafe { libc::umask(mask) };
        body();
        unsafe { libc::umask(old) };
        return None;
    }
    let name = format!("commands::orch::autodrive_issue_2546_state_tests::{test}");
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", &name, "--test-threads=1", "--nocapture"])
        .env(UMASK_2546_CHILD, "1");
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            command.env_remove(&key);
        }
    }
    command
        .env_remove("AMPLIHACK_BIN")
        .env("GIT_CONFIG_NOSYSTEM", "1");
    for (key, value) in env {
        match value {
            Some(value) => command.env(key, value),
            None => command.env_remove(key),
        };
    }
    let output = command.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "{name} failed under umask {mask:o}:\n{stdout}\n{stderr}"
    );
    assert!(
        stdout.contains("test result: ok. 1 passed"),
        "re-running {name} ran no test:\n{stdout}"
    );
    Some(stderr)
}

/// The isolated folders of one child (`isolated_2546`).
#[cfg(unix)]
struct Iso2546 {
    base: std::path::PathBuf,
}

#[cfg(unix)]
impl Iso2546 {
    fn home(&self) -> std::path::PathBuf {
        self.base.join("home")
    }
    fn state(&self) -> std::path::PathBuf {
        self.base.join("state")
    }
    /// The auto-drive state root for this child's environment.
    fn root(&self) -> std::path::PathBuf {
        self.state().join("auto-drive")
    }
    /// Makes the state root at 0700 (if missing) and a folder `name` directly
    /// under it at `mode`, by hand, so that no `created_now` applies.
    fn slug(&self, name: &str, mode: u32) -> std::path::PathBuf {
        let root = self.root();
        if !root.exists() {
            make_2546(&root, None, 0o700);
        }
        let slug = root.join(name);
        make_2546(&slug, None, mode);
        slug
    }
}

/// Runs `body` in a child under `mask` with HOME, AMPLIHACK_HOME and
/// AMPLIHACK_STATE_DIR set to folders (0700) in a new temporary folder.
/// `overrides` then changes or removes any of them for this child. Before
/// `body` runs, the child checks that every absolute value of the three
/// variables lies inside that temporary folder (S21).
#[cfg(unix)]
fn isolated_2546(
    test: &str,
    mask: libc::mode_t,
    overrides: &[(&str, Option<&str>)],
    body: impl FnOnce(&Iso2546),
) -> Option<String> {
    let parent = if std::env::var_os(UMASK_2546_CHILD).is_some() {
        None
    } else {
        Some(tempfile::tempdir().unwrap())
    };
    let mut env: Vec<(&str, Option<std::ffi::OsString>)> = Vec::new();
    if let Some(dir) = &parent {
        // Canonical, so that the shell's paths equal their canonical form.
        let base = dir.path().canonicalize().unwrap();
        for sub in ["home", "amplihack-home", "state"] {
            make_2546(&base.join(sub), None, 0o700);
        }
        env.push((ISO_2546, Some(base.clone().into_os_string())));
        env.push(("HOME", Some(base.join("home").into_os_string())));
        env.push((
            "AMPLIHACK_HOME",
            Some(base.join("amplihack-home").into_os_string()),
        ));
        env.push((
            "AMPLIHACK_STATE_DIR",
            Some(base.join("state").into_os_string()),
        ));
        for (key, value) in overrides {
            env.retain(|(k, _)| k != key);
            env.push((key, value.map(std::ffi::OsString::from)));
        }
    }
    under_umask_2546(test, mask, &env, || {
        let base = std::path::PathBuf::from(
            std::env::var_os(ISO_2546).expect("the parent sets the isolated folder"),
        );
        assert!(base.is_absolute(), "{base:?}");
        for var in ["HOME", "AMPLIHACK_HOME", "AMPLIHACK_STATE_DIR"] {
            if let Some(value) = std::env::var_os(var) {
                let path = std::path::Path::new(&value);
                assert!(
                    !path.is_absolute() || path.starts_with(&base),
                    "{var}={value:?} is outside the isolated folder {base:?}"
                );
            }
        }
        body(&Iso2546 { base });
    })
}

/// Starts `command` from an empty environment, then adds back PATH and the
/// child's HOME, AMPLIHACK_HOME and AMPLIHACK_STATE_DIR (when set) and
/// GIT_CONFIG_NOSYSTEM=1 (S22).
#[cfg(unix)]
fn shell_env_2546(command: &mut std::process::Command) -> &mut std::process::Command {
    command.env_clear().env("GIT_CONFIG_NOSYSTEM", "1");
    for var in ["PATH", "HOME", "AMPLIHACK_HOME", "AMPLIHACK_STATE_DIR"] {
        if let Some(value) = std::env::var_os(var) {
            command.env(var, value);
        }
    }
    command
}

#[cfg(unix)]
fn run_ok_2546(command: &mut std::process::Command, what: &str) -> String {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{what} failed ({:?}):\n{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// A git repository at `repo` whose origin is `ORIGIN_2546`.
#[cfg(unix)]
fn git_repo_2546(repo: &std::path::Path) {
    run_ok_2546(
        shell_env_2546(&mut amplihack_git::command())
            .args(["init", "-q"])
            .arg(repo),
        "git init",
    );
    run_ok_2546(
        shell_env_2546(&mut amplihack_git::command())
            .arg("-C")
            .arg(repo)
            .args(["remote", "add", "origin", ORIGIN_2546]),
        "git remote add",
    );
}

/// Calls `autodrive_state_dir <repo> <pr>` from autodrive_state.sh under
/// `mask`, as a drive does, and returns the folder it chose.
#[cfg(unix)]
fn autodrive_state_dir_2546(
    repo: &std::path::Path,
    pr: &str,
    mask: libc::mode_t,
) -> std::path::PathBuf {
    let stdout = run_ok_2546(
        shell_env_2546(&mut std::process::Command::new("bash"))
            .arg("-c")
            .arg(
                r#"umask "$MASK"
. "$TOOLS/autodrive_state.sh" || exit 3
autodrive_state_dir "$REPO" "$PR"
"#,
            )
            .env("MASK", format!("{mask:03o}"))
            .env("TOOLS", tools_2546())
            .env("REPO", repo)
            .env("PR", pr),
        "autodrive_state_dir",
    );
    std::path::PathBuf::from(stdout.trim_end_matches('\n'))
}

#[cfg(unix)]
fn mode_2546(path: &std::path::Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::symlink_metadata(path)
        .unwrap()
        .permissions()
        .mode()
        & 0o7777
}

/// Every folder at or below `root` is 0700 and every file 0600.
#[cfg(unix)]
fn assert_private_tree_2546(root: &std::path::Path) {
    let meta = std::fs::symlink_metadata(root).unwrap();
    if meta.is_dir() {
        assert_eq!(mode_2546(root), 0o700, "folder {}", root.display());
        for entry in std::fs::read_dir(root).unwrap() {
            assert_private_tree_2546(&entry.unwrap().path());
        }
    } else {
        assert!(meta.is_file(), "{} is not a regular file", root.display());
        assert_eq!(mode_2546(root), 0o600, "file {}", root.display());
    }
}

#[cfg(unix)]
fn repair_lines_2546(stderr: &str) -> Vec<&str> {
    stderr.lines().filter(|l| l.contains(REPAIR_2546)).collect()
}

/// A repair line starts with `INFO: `, or with `WARNING: ` when this user's
/// group may have other members (M3).
#[cfg(unix)]
fn is_repair_prefix_2546(line: &str) -> bool {
    line.starts_with("INFO: ") || line.starts_with("WARNING: ")
}

/// Stand-in for the amplihack binary the completion context script calls. It
/// records the mode of each --context-from-file target and of its folder while
/// they still exist (the script removes them afterwards).
#[cfg(unix)]
const CONTEXT_STUB_2546: &str = r#"#!/usr/bin/env bash
mode() { stat -c %a "$1" 2>/dev/null || stat -f %Lp "$1"; }
while [ $# -gt 0 ]; do
  if [ "$1" = --context-from-file ]; then
    p="${2#*=}"
    printf '%s %s %s\n' "$(mode "$p")" "$(mode "$(dirname "$p")")" "${2%%=*}" >> "$STUB_MODES"
    shift
  fi
  shift
done
printf '{}'
"#;

/// Runs the shell writers under `mask` against `state`: the phase marker from
/// autodrive_state.sh and the completion_context writer. Returns the recorded
/// "<file mode> <folder mode> <name>" lines from the stub.
#[cfg(unix)]
fn run_shell_writers_2546(state: &std::path::Path, mask: libc::mode_t) -> Vec<String> {
    use std::os::unix::fs::PermissionsExt;
    let tools = tools_2546();
    let stub_dir = tempfile::tempdir().unwrap();
    let stub = stub_dir.path().join("amplihack-stub");
    std::fs::write(&stub, CONTEXT_STUB_2546).unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o700)).unwrap();
    let modes = stub_dir.path().join("modes.txt");
    run_ok_2546(
        shell_env_2546(&mut std::process::Command::new("bash"))
            .arg("-c")
            .arg(
                r#"umask "$MASK"
field() { printf 'stub'; }
. "$TOOLS/autodrive_state.sh" || exit 3
autodrive_mark_phase_done "$STATE_DIR" crusty-loop || exit 4
run_context() { . "$TOOLS/autodrive_completion_context.sh"; }
run_context || exit 5
"#,
            )
            .env("MASK", format!("{mask:03o}"))
            .env("TOOLS", &tools)
            .env("STATE_DIR", state)
            .env("AMPLIHACK_BIN", &stub)
            .env("STUB_MODES", &modes)
            .env("HISTORY", "history")
            .env("BASELINE", "baseline")
            .env("PREV_FINDINGS", "findings")
            .env("PREV_TEST", "test")
            .env("PREV_CI", "ci")
            .env("TOKEN_SAFE", "token")
            .env("ROUND_LABEL", "round-1")
            .env("RECORD", state.join("record.json"))
            .env("REPO", state)
            .env("REVIEW_REAL", state)
            .env("LOOP_HOME", &tools),
        "shell writers",
    );
    let lines: Vec<String> = std::fs::read_to_string(&modes)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    assert!(!lines.is_empty(), "the context stub recorded nothing");
    lines
}

/// (a) B1 end to end, with the real layout: the state folder is chosen and
/// created by autodrive_state.sh (`autodrive_state_dir`), exactly as a drive
/// does, under `mask`. One round is written through the normal writers (the
/// evidence store, the phase marker, the completion context writer), and an
/// agent step writes `qa-run.log` and `agent-findings.json` by plain shell
/// redirection. The completion checker (`prepare`) and the next evidence check
/// (`EvidenceStore`) then pass, each file with group or other bits is repaired
/// exactly once and logged, and the slug folder needs no repair.
#[cfg(unix)]
fn real_layout_round_2546(test: &str, mask: libc::mode_t) {
    let stderr = isolated_2546(test, mask, &[], |iso| {
        let repo = iso.base.join("repo");
        git_repo_2546(&repo);

        // The drive's state folder, from the shell, once (R6).
        let state = autodrive_state_dir_2546(&repo, PR_2546, mask);
        assert_eq!(
            state,
            iso.root().join(SLUG_2546),
            "the folder the shell chose"
        );
        assert_eq!(
            mode_2546(&state),
            0o700,
            "autodrive_private_dir makes it 0700"
        );
        assert_eq!(mode_2546(&iso.root()), 0o700, "and the root it created");

        // Round N, Rust writer: the private state store (#2575 retired the
        // parsed readiness record; the store still holds the drive's files).
        let store = EvidenceStore::create(&state).unwrap();
        store.write_new("candidate.json", b"{}").unwrap();
        store.write_new("loop-1.json", b"{}").unwrap();

        // Round N, shell writers.
        let context = run_shell_writers_2546(&state, mask);
        assert_eq!(context.len(), 6, "context files seen: {context:?}");
        for line in &context {
            assert!(
                line.starts_with("600 700 "),
                "completion_context wrote a file that is not private under umask {mask:03o}: {line}"
            );
        }
        assert_eq!(mode_2546(&state.join("phases.tsv")), 0o600);

        // Round N, an agent step writing by shell redirection.
        run_ok_2546(
            shell_env_2546(&mut std::process::Command::new("bash"))
                .arg("-c")
                .arg(
                    r#"umask "$MASK"
printf 'qa: 12 passed\n' > "$STATE_DIR/qa-run.log"
printf '{"findings":[]}\n' > "$STATE_DIR/agent-findings.json"
"#,
                )
                .env("MASK", format!("{mask:03o}"))
                .env("STATE_DIR", &state),
            "agent-style writes",
        );
        #[allow(clippy::unnecessary_cast)] // mode_t is u16 on macOS
        let written = 0o666 & !(mask as u32);
        assert_eq!(mode_2546(&state.join("qa-run.log")), written);
        assert_eq!(mode_2546(&state.join("agent-findings.json")), written);

        // The completion checker reads qa-run.log as evaluator context.
        let s = state.to_str().unwrap();
        let record = state.join("crusty-round-1.json");
        let tools = tools_2546();
        let qa = format!("loop_test_signal={}", state.join("qa-run.log").display());
        prepare_2546(&[
            "--state-dir",
            s,
            "--round-label",
            "round-1",
            "--record",
            record.to_str().unwrap(),
            "--repo",
            repo.to_str().unwrap(),
            "--review-dir",
            repo.to_str().unwrap(),
            "--launch-tools",
            tools.to_str().unwrap(),
            "--context-from-file",
            &qa,
        ])
        .unwrap_or_else(|e| panic!("completion prepare under the state root: {e:#}"));

        // Round N+1: the next evidence check reads the same state.
        let next = EvidenceStore::create(&state).unwrap();
        for name in [
            "candidate.json".to_owned(),
            "loop-1.json".to_owned(),
            "phases.tsv".to_owned(),
            "qa-run.log".to_owned(),
            "agent-findings.json".to_owned(),
        ] {
            next.read_private(&name)
                .unwrap_or_else(|e| panic!("{name}: {e:#}"));
        }
        assert_private_tree_2546(&state);
    });
    if let Some(stderr) = stderr {
        let lines = repair_lines_2546(&stderr);
        assert_eq!(
            lines.len(),
            2,
            "exactly the two agent-written files need repair:\n{stderr}"
        );
        #[allow(clippy::unnecessary_cast)] // mode_t is u16 on macOS
        let written = format!("{:04o}", 0o666 & !(mask as u32));
        for file in ["qa-run.log\"", "agent-findings.json\""] {
            let matching: Vec<_> = lines.iter().filter(|l| l.contains(file)).collect();
            assert_eq!(matching.len(), 1, "one repair line for {file}:\n{stderr}");
            let line = matching[0];
            assert!(
                is_repair_prefix_2546(line)
                    && line.contains(&written)
                    && line.contains("0600")
                    && line.contains(SLUG_2546),
                "{line}"
            );
        }
        let slug_itself = format!("{SLUG_2546}\" had");
        assert!(
            !lines.iter().any(|l| l.contains(&slug_itself)),
            "no repair line for the slug folder itself:\n{stderr}"
        );
    }
}

#[cfg(unix)]
#[test]
fn issue_2546_real_layout_round_under_umask_002() {
    real_layout_round_2546("issue_2546_real_layout_round_under_umask_002", 0o002);
}

#[cfg(unix)]
#[test]
fn issue_2546_real_layout_round_under_umask_022() {
    real_layout_round_2546("issue_2546_real_layout_round_under_umask_022", 0o022);
}

/// (b) A record in a slug folder under the state root, chmod'ed to 0664, is
/// repaired to 0600 and logged. The folder exists before the store opens it,
/// so only the location rule can allow the repair.
#[cfg(unix)]
#[test]
fn issue_2546_hand_chmod_0664_is_repaired_and_logged() {
    use std::os::unix::fs::PermissionsExt;
    let stderr = isolated_2546(
        "issue_2546_hand_chmod_0664_is_repaired_and_logged",
        0o022,
        &[],
        |iso| {
            let root = iso.slug("slug-0664", 0o700);
            let store = EvidenceStore::create(&root).unwrap();
            store.write_new("record.json", b"{}").unwrap();
            let file = root.join("record.json");
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o664)).unwrap();
            assert_eq!(store.read_private("record.json").unwrap(), b"{}");
            assert_eq!(mode_2546(&file), 0o600);
            // A second read finds nothing left to repair.
            assert_eq!(store.read_private("record.json").unwrap(), b"{}");
        },
    );
    if let Some(stderr) = stderr {
        let lines = repair_lines_2546(&stderr);
        assert_eq!(lines.len(), 1, "one repair, logged once:\n{stderr}");
        let line = lines[0];
        assert!(
            is_repair_prefix_2546(line)
                && line.contains("slug-0664/record.json\"")
                && line.contains("0664")
                && line.contains("(gid ")
                && line.contains("0600"),
            "{line}"
        );
    }
}

/// (b) An existing slug folder under the state root, chmod'ed to 0775, is
/// repaired to 0700 when the evidence store opens it.
#[cfg(unix)]
#[test]
fn issue_2546_state_folder_0775_is_repaired_and_logged() {
    let stderr = isolated_2546(
        "issue_2546_state_folder_0775_is_repaired_and_logged",
        0o022,
        &[],
        |iso| {
            let root = iso.slug("slug-0775", 0o775);
            let store = EvidenceStore::create(&root).unwrap();
            assert_eq!(mode_2546(&root), 0o700);
            store.write_new("record.json", b"{}").unwrap();
            assert_eq!(store.read_private("record.json").unwrap(), b"{}");
        },
    );
    if let Some(stderr) = stderr {
        let lines = repair_lines_2546(&stderr);
        assert_eq!(lines.len(), 1, "one repair, logged once:\n{stderr}");
        let line = lines[0];
        assert!(
            is_repair_prefix_2546(line)
                && line.contains("slug-0775\"")
                && line.contains("0775")
                && line.contains("0700"),
            "{line}"
        );
    }
}

/// S1: outside the auto-drive state root, the completion checker repairs no
/// folder and refuses an open one, changing nothing. A folder named
/// `autodrive-state` in a temporary folder is not under the root, so its
/// scope is `Strict` (B1); and even a `StateTree` scope repairs no folder
/// outside its own tree. Only this user's file inside a private folder is
/// repaired (ResearchDrafts #865).
#[cfg(unix)]
#[test]
fn issue_2546_outside_state_tree_stays_strict() {
    use crate::commands::orch::autodrive_completion::secure_fs::{self, Scope};
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("autodrive-state");
    std::fs::create_dir(&state).unwrap();
    assert!(
        matches!(Scope::of(&state), Scope::Strict),
        "the folder name allows no repair"
    );
    for scope in [Scope::Strict, Scope::StateTree(&state)] {
        // An open folder holding an open file.
        let open = tempfile::tempdir_in(dir.path()).unwrap();
        std::fs::set_permissions(open.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        let file = open.path().join("notes.txt");
        std::fs::write(&file, b"notes").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        let error =
            secure_fs::read(scope, &file, 1 << 20).expect_err("read outside the state tree");
        assert!(format!("{error:#}").contains(OUTSIDE_2546), "{error:#}");
        assert_eq!(
            mode_2546(open.path()),
            0o755,
            "folder outside the tree changed"
        );
        assert_eq!(mode_2546(&file), 0o644, "file outside the tree changed");

        // A private folder holding this user's open file (ResearchDrafts
        // #865): no other user can create, replace or change an entry in a
        // 0700 folder owned by this user, so the file is repaired to 0600
        // and read. The folder itself is never repaired by this rule.
        let closed = tempfile::tempdir_in(dir.path()).unwrap();
        std::fs::set_permissions(closed.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let file = closed.path().join("notes.txt");
        std::fs::write(&file, b"notes").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(secure_fs::read(scope, &file, 1 << 20).unwrap(), b"notes");
        assert_eq!(mode_2546(&file), 0o600, "file in a private folder");
        assert_eq!(mode_2546(closed.path()), 0o700);
        // A second read finds nothing left to repair.
        assert_eq!(secure_fs::read(scope, &file, 1 << 20).unwrap(), b"notes");
    }
}

/// (c) The pure rule: foreign owner, foreign group, world-writable, symlink,
/// wrong type and hard links are refused; repair happens only where allowed.
#[test]
fn issue_2546_foreign_owner_symlink_and_hard_link_are_still_refused() {
    use crate::commands::orch::autodrive::private_mode::{Access, Entry, Kind, Repair, access};
    const SYMLINK: &str = "evidence is a symlink";
    const TYPE: &str = "evidence is not a regular file or state directory";
    const OWNER: &str = "untrusted evidence ownership or permissions";
    const LINKS: &str = "evidence has unexpected hard links";
    let (me, group) = (1000, 1000);
    // The table below lists (entry, dir, single_link, repair) as booleans;
    // this maps them onto `Kind` and `Repair`.
    let check = |entry: Entry, dir: bool, single_link: bool, repair: bool| {
        let kind = match (dir, single_link) {
            (true, _) => Kind::Dir,
            (false, true) => Kind::SingleLinkFile,
            (false, false) => Kind::File,
        };
        let repair = if repair {
            Repair::Allowed
        } else {
            Repair::Refuse
        };
        access(entry, kind, me, group, repair)
    };
    let file = Entry {
        symlink: false,
        is_dir: false,
        is_file: true,
        uid: me,
        gid: group,
        mode: 0o664,
        nlink: 1,
    };
    let folder = Entry {
        is_dir: true,
        is_file: false,
        mode: 0o775,
        nlink: 3,
        ..file
    };

    // Repaired: this user's entries with extra bits, in the state tree.
    assert_eq!(check(file, false, true, true), Access::Repair(0o600));
    assert_eq!(check(folder, true, true, true), Access::Repair(0o700));
    assert_eq!(
        check(
            Entry {
                mode: 0o640,
                ..file
            },
            false,
            true,
            true
        ),
        Access::Repair(0o600)
    );
    // The setgid bit goes with the group bits.
    assert_eq!(
        check(
            Entry {
                mode: 0o2775,
                ..folder
            },
            true,
            true,
            true
        ),
        Access::Repair(0o700)
    );
    // Other-read only, with a foreign group: no group bit trusts that group.
    assert_eq!(
        check(
            Entry {
                mode: 0o705,
                gid: group + 1,
                ..folder
            },
            true,
            true,
            true
        ),
        Access::Repair(0o700)
    );

    // Already private.
    assert_eq!(
        check(
            Entry {
                mode: 0o600,
                ..file
            },
            false,
            true,
            true
        ),
        Access::Private
    );
    assert_eq!(
        check(
            Entry {
                mode: 0o700,
                ..folder
            },
            true,
            true,
            true
        ),
        Access::Private
    );
    assert_eq!(
        check(
            Entry {
                mode: 0o600,
                ..file
            },
            false,
            true,
            false
        ),
        Access::Private
    );

    // Refused, each with its reason, whether or not repair is allowed.
    let refused = [
        (
            Entry {
                uid: me + 1,
                ..file
            },
            false,
            true,
            OWNER,
        ),
        (
            Entry {
                uid: me + 1,
                mode: 0o600,
                ..file
            },
            false,
            true,
            OWNER,
        ),
        (
            Entry {
                uid: me + 1,
                ..folder
            },
            true,
            true,
            OWNER,
        ),
        (
            Entry {
                mode: 0o666,
                ..file
            },
            false,
            true,
            OWNER,
        ),
        (
            Entry {
                mode: 0o602,
                ..file
            },
            false,
            true,
            OWNER,
        ),
        (
            Entry {
                mode: 0o777,
                ..folder
            },
            true,
            true,
            OWNER,
        ),
        (
            Entry {
                gid: group + 1,
                ..file
            },
            false,
            true,
            OWNER,
        ),
        (
            Entry {
                gid: group + 1,
                mode: 0o750,
                ..folder
            },
            true,
            true,
            OWNER,
        ),
        (
            Entry {
                symlink: true,
                ..file
            },
            false,
            true,
            SYMLINK,
        ),
        (
            Entry {
                symlink: true,
                ..folder
            },
            true,
            true,
            SYMLINK,
        ),
        (file, true, true, TYPE),
        (folder, false, true, TYPE),
        (
            Entry {
                is_file: false,
                ..file
            },
            false,
            true,
            TYPE,
        ),
        (
            Entry {
                nlink: 2,
                mode: 0o600,
                ..file
            },
            false,
            true,
            LINKS,
        ),
        (Entry { nlink: 2, ..file }, false, true, LINKS),
        (Entry { nlink: 2, ..file }, false, false, LINKS),
    ];
    for repair in [true, false] {
        for (entry, dir, single_link, reason) in refused {
            assert_eq!(
                check(entry, dir, single_link, repair),
                Access::Refuse(reason),
                "{entry:?} (dir {dir}, single_link {single_link}, repair {repair})"
            );
        }
    }

    // The completion checker allows a clean hard-linked file but never
    // repairs one.
    assert_eq!(
        check(
            Entry {
                nlink: 2,
                mode: 0o600,
                ..file
            },
            false,
            false,
            true
        ),
        Access::Private
    );

    // Extra bits where repair is not allowed: the old strict refusal, with
    // its own reason.
    assert_eq!(
        check(file, false, true, false),
        Access::Refuse(OUTSIDE_2546)
    );
    assert_eq!(
        check(folder, true, true, false),
        Access::Refuse(OUTSIDE_2546)
    );
    assert_eq!(
        check(
            Entry {
                mode: 0o755,
                ..folder
            },
            true,
            false,
            false
        ),
        Access::Refuse(OUTSIDE_2546)
    );

    // ResearchDrafts #865: a file opened through a private folder (0700,
    // owned by this user, a real folder). No other user can create, replace
    // or change an entry in it, so this user's file with extra bits is
    // repaired. Rows 1 to 6 still decide first, and no folder is repaired.
    let check_pf = |entry: Entry, kind: Kind| access(entry, kind, me, group, Repair::PrivateFolder);
    let agent_file = Entry {
        mode: 0o644,
        ..file
    };
    for kind in [Kind::File, Kind::SingleLinkFile] {
        for mode in [0o644, 0o664, 0o640, 0o604] {
            assert_eq!(
                check_pf(Entry { mode, ..file }, kind),
                Access::Repair(0o600),
                "mode {mode:o}, {kind:?}"
            );
        }
        // Setuid, setgid and sticky bits are cleared with the extra bits.
        assert_eq!(
            check_pf(
                Entry {
                    mode: 0o2664,
                    ..file
                },
                kind
            ),
            Access::Repair(0o600),
            "{kind:?}"
        );
        assert_eq!(
            check_pf(
                Entry {
                    mode: 0o744,
                    ..file
                },
                kind
            ),
            Access::Repair(0o700),
            "{kind:?}"
        );
        assert_eq!(
            check_pf(
                Entry {
                    mode: 0o600,
                    ..file
                },
                kind
            ),
            Access::Private,
            "{kind:?}"
        );
    }
    let refused_pf = [
        // A file another user owns: they could have written it.
        (
            Entry {
                uid: me + 1,
                ..agent_file
            },
            Kind::SingleLinkFile,
            OWNER,
        ),
        (
            Entry {
                uid: me + 1,
                mode: 0o600,
                ..agent_file
            },
            Kind::File,
            OWNER,
        ),
        (
            Entry {
                symlink: true,
                ..agent_file
            },
            Kind::SingleLinkFile,
            SYMLINK,
        ),
        (
            Entry {
                is_file: false,
                ..agent_file
            },
            Kind::SingleLinkFile,
            TYPE,
        ),
        (folder, Kind::File, TYPE),
        (
            Entry {
                nlink: 2,
                ..agent_file
            },
            Kind::SingleLinkFile,
            LINKS,
        ),
        (
            Entry {
                nlink: 2,
                ..agent_file
            },
            Kind::File,
            LINKS,
        ),
        // Other-write is never repaired.
        (
            Entry {
                mode: 0o666,
                ..file
            },
            Kind::SingleLinkFile,
            OWNER,
        ),
        (
            Entry {
                mode: 0o602,
                ..file
            },
            Kind::File,
            OWNER,
        ),
        // Group bits for a group that is not this user's (or an inherited
        // setgid group).
        (
            Entry {
                mode: 0o664,
                gid: group + 1,
                ..file
            },
            Kind::SingleLinkFile,
            OWNER,
        ),
        // Folders keep the location rule: a private folder repairs no folder.
        (
            Entry {
                mode: 0o755,
                ..folder
            },
            Kind::Dir,
            OUTSIDE_2546,
        ),
        (folder, Kind::Dir, OUTSIDE_2546),
        (
            Entry {
                mode: 0o777,
                ..folder
            },
            Kind::Dir,
            OWNER,
        ),
    ];
    for (entry, kind, reason) in refused_pf {
        assert_eq!(
            check_pf(entry, kind),
            Access::Refuse(reason),
            "{entry:?} ({kind:?}, private folder)"
        );
    }
    assert_eq!(
        check_pf(
            Entry {
                mode: 0o700,
                ..folder
            },
            Kind::Dir
        ),
        Access::Private
    );
    // A clean hard-linked file is still allowed where the completion
    // checker allows one.
    assert_eq!(
        check_pf(
            Entry {
                nlink: 2,
                mode: 0o600,
                ..file
            },
            Kind::File
        ),
        Access::Private
    );
}

// ── #2546: a file in a private state folder (ResearchDrafts #865) ──

/// The #865 drive: a run folder (0700) holding `autodrive-state` (0700),
/// outside the auto-drive state root, into which the drive's QA agent wrote
/// a log with a shell redirect under umask 022. Files at 0644 and 0664 are
/// repaired to 0600 and read, through both evidence read paths, with one
/// repair line per file; a second read writes no line.
#[cfg(unix)]
#[test]
fn issue_2546_private_run_dir_state_repairs_agent_written_files() {
    use crate::commands::orch::autodrive_completion::secure_fs::{self, Scope};
    const STORE_FILES: [(&str, u32); 2] = [
        ("qa-criterion1-attempt2-ffi.log", 0o644),
        ("store-0664.log", 0o664),
    ];
    const FS_FILES: [(&str, u32); 2] = [("fs-0644.log", 0o644), ("fs-0664.log", 0o664)];
    let stderr = isolated_2546(
        "issue_2546_private_run_dir_state_repairs_agent_written_files",
        0o022,
        &[],
        |iso| {
            let run = iso.base.join("run");
            make_2546(&run, None, 0o700);
            let state = run.join("autodrive-state");
            make_2546(&state, None, 0o700);
            assert!(
                matches!(Scope::of(&state), Scope::Strict),
                "the run folder is outside the state root"
            );
            for (name, mode) in STORE_FILES.iter().chain(FS_FILES.iter()) {
                make_2546(&state.join(name), Some(name.as_bytes()), *mode);
                assert_eq!(mode_2546(&state.join(name)), *mode);
            }

            let store = EvidenceStore::create(&state).unwrap();
            for _ in 0..2 {
                for (name, _) in STORE_FILES {
                    assert_eq!(store.read_private(name).unwrap(), name.as_bytes(), "{name}");
                    assert_eq!(mode_2546(&state.join(name)), 0o600, "{name}");
                }
                for (name, _) in FS_FILES {
                    let path = state.join(name);
                    assert_eq!(
                        secure_fs::read(Scope::of(&state), &path, 1 << 20).unwrap(),
                        name.as_bytes(),
                        "{name}"
                    );
                    assert_eq!(mode_2546(&path), 0o600, "{name}");
                }
            }
            assert_eq!(mode_2546(&state), 0o700, "the folder is unchanged");
            assert_eq!(mode_2546(&run), 0o700, "the run folder is unchanged");
        },
    );
    if let Some(stderr) = stderr {
        let lines = repair_lines_2546(&stderr);
        assert_eq!(lines.len(), 4, "one repair line per file:\n{stderr}");
        for (name, mode) in STORE_FILES.iter().chain(FS_FILES.iter()) {
            let quoted = format!("autodrive-state/{name}\"");
            let matching: Vec<_> = lines.iter().filter(|l| l.contains(&quoted)).collect();
            assert_eq!(matching.len(), 1, "{name}:\n{stderr}");
            let line = matching[0];
            assert!(
                is_repair_prefix_2546(line)
                    && line.contains(&format!("{mode:04o}"))
                    && line.contains("0600"),
                "{line}"
            );
        }
    }
}

/// The same agent-written files in an open (0755) folder outside the state
/// root are still refused by both evidence read paths: the folder is neither
/// private nor repairable, so the refusal comes before any file is examined.
/// Nothing changes and nothing is logged.
#[cfg(unix)]
#[test]
fn issue_2546_open_folder_outside_state_root_still_refuses_files() {
    use crate::commands::orch::autodrive_completion::secure_fs::{self, Scope};
    const FILES: [(&str, u32); 2] = [
        ("qa-criterion1-attempt2-ffi.log", 0o644),
        ("agent-0664.log", 0o664),
    ];
    let stderr = isolated_2546(
        "issue_2546_open_folder_outside_state_root_still_refuses_files",
        0o022,
        &[],
        |iso| {
            let state = iso.base.join("autodrive-state");
            make_2546(&state, None, 0o755);
            for (name, mode) in FILES {
                make_2546(&state.join(name), Some(b"log"), mode);
            }

            let error = store_error_2546(&state);
            assert!(error.contains(OUTSIDE_2546), "{error}");
            for (name, _) in FILES {
                let error = secure_fs::read(Scope::of(&state), &state.join(name), 1 << 20)
                    .expect_err("a file in an open folder");
                assert!(format!("{error:#}").contains(OUTSIDE_2546), "{error:#}");
            }

            assert_eq!(mode_2546(&state), 0o755, "the folder is unchanged");
            for (name, mode) in FILES {
                assert_eq!(mode_2546(&state.join(name)), mode, "{name} is unchanged");
            }
        },
    );
    if let Some(stderr) = stderr {
        assert!(
            repair_lines_2546(&stderr).is_empty(),
            "nothing is repaired:\n{stderr}"
        );
    }
}

// ── #2546: repairs stay at or below a repairable state root (S1) ──

/// Creates `path` as a folder (or a file holding `bytes`) with `mode`.
#[cfg(unix)]
fn make_2546(path: &std::path::Path, bytes: Option<&[u8]>, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    match bytes {
        Some(bytes) => std::fs::write(path, bytes).unwrap(),
        None => std::fs::create_dir(path).unwrap(),
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

/// Runs `amplihack orch auto-drive-completion prepare` with `args`.
#[cfg(unix)]
fn prepare_2546(args: &[&str]) -> anyhow::Result<()> {
    use crate::commands::orch::autodrive_completion::{Prepare, prepare};
    use clap::Parser;
    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        prepare: Prepare,
    }
    let cli = Cli::try_parse_from(std::iter::once("prepare").chain(args.iter().copied())).unwrap();
    prepare(cli.prepare)
}

#[cfg(unix)]
fn store_error_2546(root: &std::path::Path) -> String {
    match EvidenceStore::create(root) {
        Ok(_) => panic!("{} was accepted", root.display()),
        Err(error) => format!("{error:#}"),
    }
}

/// (i) A `.completion-*` part does not make its parent folder repairable: a
/// 0755 folder `x` above `x/.completion-y/f` is refused and left at 0755,
/// with no root and with a root that is not `x`. A folder named
/// `autodrive-state` outside the state root has scope `Strict` (B1).
#[cfg(unix)]
#[test]
fn issue_2546_completion_part_outside_state_root_stays_strict() {
    use crate::commands::orch::autodrive_completion::secure_fs::{self, Scope};
    let dir = tempfile::tempdir().unwrap();
    let x = dir.path().join("x");
    make_2546(&x, None, 0o755);
    make_2546(&x.join(".completion-y"), None, 0o700);
    let file = x.join(".completion-y").join("f");
    make_2546(&file, Some(b"f"), 0o600);
    let other = dir.path().join("autodrive-state");
    make_2546(&other, None, 0o700);
    assert!(matches!(Scope::of(&other), Scope::Strict));
    for scope in [Scope::Strict, Scope::StateTree(&other)] {
        let error = secure_fs::read(scope, &file, 1 << 20).expect_err("x is not repairable");
        assert!(format!("{error:#}").contains(OUTSIDE_2546), "{error:#}");
        assert_eq!(mode_2546(&x), 0o755, "{scope:?} changed x");
    }
}

/// (ii) B1: `check_dir` repairs a slug folder directly under the auto-drive
/// state root. A folder anywhere else is checked strictly and left as it was:
/// a plain folder, a folder named `autodrive-state` in a shared folder (M2), a
/// folder below a slug folder, and a slug in a sibling of the root.
#[cfg(unix)]
#[test]
fn issue_2546_check_dir_repairs_slug_folder_under_state_root() {
    use crate::commands::orch::autodrive_completion::secure_fs::{self, Scope};
    let stderr = isolated_2546(
        "issue_2546_check_dir_repairs_slug_folder_under_state_root",
        0o022,
        &[],
        |iso| {
            let slug = iso.slug("slug-check", 0o755);
            assert!(matches!(Scope::of(&slug), Scope::StateTree(_)));
            secure_fs::check_dir(&slug).unwrap();
            assert_eq!(mode_2546(&slug), 0o700);

            let plain = iso.base.join("plain");
            make_2546(&plain, None, 0o755);
            let shared = iso.base.join("shared");
            make_2546(&shared, None, 0o777);
            let named = shared.join("autodrive-state");
            make_2546(&named, None, 0o755);
            let parent = iso.slug("slug-parent", 0o700);
            let deeper = parent.join("deeper");
            make_2546(&deeper, None, 0o755);
            let sibling = iso.state().join("auto-drive-x");
            make_2546(&sibling, None, 0o700);
            let beside = sibling.join("slug");
            make_2546(&beside, None, 0o755);
            for folder in [&plain, &named, &deeper, &beside] {
                assert!(
                    matches!(Scope::of(folder), Scope::Strict),
                    "{}",
                    folder.display()
                );
                let error = secure_fs::check_dir(folder).expect_err("outside the state root");
                let error = format!("{error:#}");
                assert!(
                    error.contains(OUTSIDE_2546) && error.contains("chmod 700"),
                    "{}: {error}",
                    folder.display()
                );
                assert_eq!(mode_2546(folder), 0o755, "{}", folder.display());
            }
        },
    );
    if let Some(stderr) = stderr {
        let lines = repair_lines_2546(&stderr);
        assert_eq!(lines.len(), 1, "only the slug folder:\n{stderr}");
        assert!(
            is_repair_prefix_2546(lines[0]) && lines[0].contains("slug-check\""),
            "{}",
            lines[0]
        );
    }
}

/// (iii) `prepare` validates its arguments before it repairs the state
/// folder: a rejected call leaves a repairable 0775 slug folder at 0775.
#[cfg(unix)]
#[test]
fn issue_2546_rejected_prepare_changes_no_modes() {
    let stderr = isolated_2546(
        "issue_2546_rejected_prepare_changes_no_modes",
        0o022,
        &[],
        |iso| {
            let state = iso.slug("slug-rejected", 0o775);
            let s = state.to_str().unwrap();
            let record = state.join("crusty-round-x.json");
            let error = prepare_2546(&[
                "--state-dir",
                s,
                "--round-label",
                "round-x",
                "--record",
                record.to_str().unwrap(),
                "--repo",
                s,
                "--review-dir",
                s,
                "--launch-tools",
                s,
            ])
            .expect_err("invalid round label");
            assert!(
                format!("{error:#}").contains("invalid round label"),
                "{error:#}"
            );
            assert_eq!(mode_2546(&state), 0o775);
        },
    );
    if let Some(stderr) = stderr {
        assert!(repair_lines_2546(&stderr).is_empty(), "{stderr}");
    }
}

/// (iv) `prepare` refuses context files outside a repairable state folder
/// before it reads them, and leaves them and their folder as they were.
#[cfg(unix)]
#[test]
fn issue_2546_prepare_refuses_context_outside_state_folder() {
    let stderr = isolated_2546(
        "issue_2546_prepare_refuses_context_outside_state_folder",
        0o022,
        &[],
        |iso| {
            let state = iso.slug("slug-context", 0o700);
            let outside = iso.root().join("outside");
            make_2546(&outside, None, 0o755);
            let file = outside.join("f");
            make_2546(&file, Some(b"history"), 0o644);
            let s = state.to_str().unwrap();
            let record = state.join("crusty-round-1.json");
            let record = record.to_str().unwrap();
            let dotted = format!("{s}/../outside/f");
            for extra in [
                vec![
                    "--context-from-file".to_owned(),
                    format!("loop_history={}", file.display()),
                ],
                vec![
                    "--context-from-file".to_owned(),
                    format!("loop_history={dotted}"),
                ],
                vec![
                    "--evaluator-context-file".to_owned(),
                    file.display().to_string(),
                ],
            ] {
                let mut args = vec![
                    "--state-dir",
                    s,
                    "--round-label",
                    "round-1",
                    "--record",
                    record,
                    "--repo",
                    s,
                    "--review-dir",
                    s,
                    "--launch-tools",
                    s,
                ];
                args.extend(extra.iter().map(String::as_str));
                let error = prepare_2546(&args).expect_err("context outside the state folder");
                assert!(
                    format!("{error:#}")
                        .contains("context file must be inside the state directory"),
                    "{extra:?}: {error:#}"
                );
                assert_eq!(mode_2546(&outside), 0o755);
                assert_eq!(mode_2546(&file), 0o644);
            }
        },
    );
    if let Some(stderr) = stderr {
        assert!(repair_lines_2546(&stderr).is_empty(), "{stderr}");
    }
}

/// (v) B1: `EvidenceStore::create` repairs an existing folder only when it is
/// directly under the auto-drive state root. A 0775 folder named
/// `autodrive-state` outside the root, and a 0775 folder below a slug folder,
/// are refused and left as they were; the refusal names the state root.
#[cfg(unix)]
#[test]
fn issue_2546_evidence_store_repairs_only_slug_folder_under_state_root() {
    let stderr = isolated_2546(
        "issue_2546_evidence_store_repairs_only_slug_folder_under_state_root",
        0o022,
        &[],
        |iso| {
            let named = iso.base.join("autodrive-state");
            make_2546(&named, None, 0o775);
            let error = store_error_2546(&named);
            assert!(
                error.contains(OUTSIDE_2546)
                    && error.contains("chmod 700")
                    && error.contains(&format!("{:?}", iso.root())),
                "{error}"
            );
            assert_eq!(mode_2546(&named), 0o775);

            let parent = iso.slug("slug-parent", 0o700);
            let deeper = parent.join("deeper");
            make_2546(&deeper, None, 0o775);
            let error = store_error_2546(&deeper);
            assert!(error.contains(OUTSIDE_2546), "{error}");
            assert_eq!(mode_2546(&deeper), 0o775);

            let slug = iso.slug("slug-store", 0o775);
            EvidenceStore::create(&slug).unwrap();
            assert_eq!(mode_2546(&slug), 0o700);
        },
    );
    if let Some(stderr) = stderr {
        let lines = repair_lines_2546(&stderr);
        assert_eq!(lines.len(), 1, "only the slug folder:\n{stderr}");
        assert!(lines[0].contains("slug-store\""), "{}", lines[0]);
    }
}

/// S6 (security review F1): only the store's own `mkdir` makes
/// `created_now` true. A folder that already exists outside the state root,
/// as one made by another process between the lstat and the `mkdir` would,
/// is reported as existing, so the store refuses it and leaves it at 0775. A
/// missing folder is made at 0700, with its missing parents, and reported as
/// created. Run under umask 002, where the race matters.
#[cfg(unix)]
#[test]
fn issue_2546_existing_folder_is_not_treated_as_created() {
    use crate::commands::orch::autodrive::state::open_or_create_root;
    let stderr = isolated_2546(
        "issue_2546_existing_folder_is_not_treated_as_created",
        0o002,
        &[],
        |iso| {
            let existing = iso.base.join("existing");
            make_2546(&existing, None, 0o775);
            let (created_now, _) = open_or_create_root(&existing).unwrap();
            assert!(!created_now, "an existing folder is not created now");
            let error = store_error_2546(&existing);
            assert!(error.contains(OUTSIDE_2546), "{error}");
            assert_eq!(mode_2546(&existing), 0o775);

            let fresh = iso.base.join("parent").join("fresh");
            let (created_now, _) = open_or_create_root(&fresh).unwrap();
            assert!(created_now, "a missing folder is created now");
            assert_eq!(mode_2546(&fresh), 0o700);
            assert_eq!(mode_2546(&iso.base.join("parent")), 0o700);
            let (created_now, _) = open_or_create_root(&fresh).unwrap();
            assert!(!created_now, "the second call finds it existing");
        },
    );
    if let Some(stderr) = stderr {
        assert!(repair_lines_2546(&stderr).is_empty(), "{stderr}");
    }
}

/// D3/D4/S2/S3/S23: the location rule fails strict. With a private root a
/// slug folder qualifies; the root itself, a folder below a slug and a slug in
/// a sibling of the root never do. A root with group or other write bits, a
/// root that is a symlink (even to a private folder) and a missing root each
/// make the slug folder strict, and nothing is changed.
#[cfg(unix)]
#[test]
fn issue_2546_in_state_root_fails_strict() {
    use crate::commands::orch::autodrive::private_mode::in_state_root;
    use crate::commands::orch::autodrive_completion::secure_fs;
    use std::os::unix::fs::PermissionsExt;
    let stderr = isolated_2546("issue_2546_in_state_root_fails_strict", 0o022, &[], |iso| {
        let root = iso.root();
        let slug = iso.slug("slug-x", 0o775);
        assert!(in_state_root(&slug), "a slug folder under a private root");
        assert!(!in_state_root(&root), "the root itself");
        make_2546(&slug.join("deeper"), None, 0o700);
        assert!(!in_state_root(&slug.join("deeper")), "below a slug folder");
        let sibling = iso.state().join("auto-drive-x");
        make_2546(&sibling, None, 0o700);
        make_2546(&sibling.join("slug"), None, 0o700);
        assert!(
            !in_state_root(&sibling.join("slug")),
            "a sibling of the root"
        );

        let refused = |why: &str| {
            assert!(!in_state_root(&slug), "{why}");
            let error = store_error_2546(&slug);
            assert!(error.contains(OUTSIDE_2546), "{why}: {error}");
            let error = format!("{:#}", secure_fs::check_dir(&slug).expect_err(why));
            assert!(error.contains(OUTSIDE_2546), "{why}: {error}");
            assert_eq!(mode_2546(&slug), 0o775, "{why}: the slug folder changed");
        };
        for mode in [0o770, 0o757, 0o720, 0o702] {
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(mode)).unwrap();
            refused(&format!("root at {mode:04o}"));
            assert_eq!(mode_2546(&root), mode, "the root is never changed");
        }
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();

        // The root path is a symlink to a private folder holding the slug.
        let real = iso.base.join("real-root");
        std::fs::rename(&root, &real).unwrap();
        std::os::unix::fs::symlink(&real, &root).unwrap();
        let real_slug = real.join("slug-x");
        assert!(!in_state_root(&real_slug), "a symlinked root");
        let error = store_error_2546(&root.join("slug-x"));
        assert!(error.contains(OUTSIDE_2546), "symlinked root: {error}");
        assert_eq!(mode_2546(&real_slug), 0o775);

        // No root at all.
        std::fs::remove_file(&root).unwrap();
        assert!(!in_state_root(&real_slug), "a missing root");
        assert!(!in_state_root(&root.join("slug-x")), "a missing root");
    });
    if let Some(stderr) = stderr {
        assert!(repair_lines_2546(&stderr).is_empty(), "{stderr}");
    }
}

/// D3: a relative AMPLIHACK_STATE_DIR leaves every folder strict, even the
/// one it names relative to the current folder.
#[cfg(unix)]
#[test]
fn issue_2546_relative_state_dir_stays_strict() {
    use crate::commands::orch::autodrive::private_mode::{autodrive_state_root, in_state_root};
    let stderr = isolated_2546(
        "issue_2546_relative_state_dir_stays_strict",
        0o022,
        &[("AMPLIHACK_STATE_DIR", Some("rel-state"))],
        |iso| {
            // This child runs only this test, so its current folder is its own.
            std::env::set_current_dir(&iso.base).unwrap();
            assert_eq!(
                autodrive_state_root(),
                std::path::PathBuf::from("rel-state/auto-drive")
            );
            let root = iso.base.join("rel-state");
            make_2546(&root, None, 0o700);
            make_2546(&root.join("auto-drive"), None, 0o700);
            let slug = root.join("auto-drive").join("slug-rel");
            make_2546(&slug, None, 0o775);
            assert!(!in_state_root(&slug));
            let error = store_error_2546(&slug);
            assert!(error.contains(OUTSIDE_2546), "{error}");
            assert_eq!(mode_2546(&slug), 0o775);
        },
    );
    if let Some(stderr) = stderr {
        assert!(repair_lines_2546(&stderr).is_empty(), "{stderr}");
    }
}

/// S7: with HOME and AMPLIHACK_STATE_DIR both unset the root is the shell's
/// `/tmp` fallback, which is shared, so the rule never applies there. The test
/// creates nothing under /tmp.
#[cfg(unix)]
#[test]
fn issue_2546_unset_state_vars_stay_strict() {
    use crate::commands::orch::autodrive::private_mode::{autodrive_state_root, in_state_root};
    isolated_2546(
        "issue_2546_unset_state_vars_stay_strict",
        0o022,
        &[("HOME", None), ("AMPLIHACK_STATE_DIR", None)],
        |_| {
            let root = autodrive_state_root();
            assert_eq!(
                root,
                std::path::PathBuf::from("/tmp/.amplihack/state/auto-drive")
            );
            assert!(!in_state_root(&root.join(SLUG_2546)));
        },
    );
}

/// D2: an empty AMPLIHACK_STATE_DIR counts as unset, as with the shell's
/// `:-`, so the root is under HOME and a slug folder there is repaired.
#[cfg(unix)]
#[test]
fn issue_2546_empty_state_dir_uses_home_root() {
    use crate::commands::orch::autodrive::private_mode::{autodrive_state_root, in_state_root};
    let stderr = isolated_2546(
        "issue_2546_empty_state_dir_uses_home_root",
        0o022,
        &[("AMPLIHACK_STATE_DIR", Some(""))],
        |iso| {
            let root = iso.home().join(".amplihack/state/auto-drive");
            assert_eq!(autodrive_state_root(), root);
            make_2546(&iso.home().join(".amplihack"), None, 0o700);
            make_2546(&iso.home().join(".amplihack/state"), None, 0o700);
            make_2546(&root, None, 0o700);
            let slug = root.join("slug-home");
            make_2546(&slug, None, 0o775);
            assert!(in_state_root(&slug));
            EvidenceStore::create(&slug).unwrap();
            assert_eq!(mode_2546(&slug), 0o700);
        },
    );
    if let Some(stderr) = stderr {
        let lines = repair_lines_2546(&stderr);
        assert_eq!(lines.len(), 1, "{stderr}");
        assert!(lines[0].contains("slug-home\""), "{}", lines[0]);
    }
}

/// The state root cases for `state_root_from` and the shell parity test:
/// (AMPLIHACK_STATE_DIR, HOME, expected root).
const ROOT_CASES_2546: [(Option<&str>, Option<&str>, &str); 7] = [
    (
        Some("/srv/state-2546"),
        Some("/home/u2546"),
        "/srv/state-2546/auto-drive",
    ),
    (
        None,
        Some("/home/u2546"),
        "/home/u2546/.amplihack/state/auto-drive",
    ),
    (
        Some(""),
        Some("/home/u2546"),
        "/home/u2546/.amplihack/state/auto-drive",
    ),
    (None, None, "/tmp/.amplihack/state/auto-drive"),
    (None, Some(""), "/tmp/.amplihack/state/auto-drive"),
    (Some(""), None, "/tmp/.amplihack/state/auto-drive"),
    (
        Some("rel/state"),
        Some("/home/u2546"),
        "rel/state/auto-drive",
    ),
];

/// D2: `state_root_from` is
/// `${AMPLIHACK_STATE_DIR:-${HOME:-/tmp}/.amplihack/state}/auto-drive`, with an
/// empty value counting as unset. AMPLIHACK_HOME plays no part.
#[test]
fn issue_2546_state_root_from_cases() {
    use crate::commands::orch::autodrive::private_mode::state_root_from;
    for (state, home, expected) in ROOT_CASES_2546 {
        assert_eq!(
            state_root_from(state.map(Into::into), home.map(Into::into)),
            std::path::PathBuf::from(expected),
            "AMPLIHACK_STATE_DIR={state:?} HOME={home:?}"
        );
    }
}

/// D2: the Rust root is the root autodrive_state.sh prints, for every case,
/// from an empty environment. Nothing is created.
#[cfg(unix)]
#[test]
fn issue_2546_state_root_matches_shell() {
    use crate::commands::orch::autodrive::private_mode::state_root_from;
    for (state, home, _) in ROOT_CASES_2546 {
        let mut command = std::process::Command::new("bash");
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("AMPLIHACK_HOME", "/elsewhere-2546/amplihack-home")
            .env("TOOLS", tools_2546())
            .arg("-c")
            .arg(r#". "$TOOLS/autodrive_state.sh" || exit 3; autodrive_state_root"#);
        if let Some(state) = state {
            command.env("AMPLIHACK_STATE_DIR", state);
        }
        if let Some(home) = home {
            command.env("HOME", home);
        }
        let shell = run_ok_2546(&mut command, "autodrive_state_root");
        assert_eq!(
            std::path::PathBuf::from(shell.trim_end_matches('\n')),
            state_root_from(state.map(Into::into), home.map(Into::into)),
            "AMPLIHACK_STATE_DIR={state:?} HOME={home:?}"
        );
    }
}

/// M3/D7: a group has other members when any listed member is not this user.
/// Names are compared as raw bytes, so a name that differs only in a byte
/// that is not valid UTF-8 is another user.
#[test]
fn issue_2546_shared_group_cases() {
    use crate::commands::orch::autodrive::private_mode::shared_group;
    let me: &[u8] = b"dev";
    let ops: &[u8] = b"ops";
    assert!(!shared_group(&[], me), "no listed members");
    assert!(!shared_group(&[me], me), "only me");
    assert!(shared_group(&[me, ops], me), "me and another user");
    assert!(shared_group(&[ops], me), "another user only");
    let raw_me: &[u8] = b"ry\xffan";
    let raw_other: &[u8] = b"ry\xfean";
    assert!(!shared_group(&[raw_me], raw_me), "same raw bytes");
    assert!(
        shared_group(&[raw_other], raw_me),
        "differs in one raw byte"
    );
}
