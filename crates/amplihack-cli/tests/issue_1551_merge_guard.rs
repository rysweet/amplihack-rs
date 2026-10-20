//! Issue #1551: `amplihack recipe run` puts the `gh` merge guard first on
//! `PATH`, and sets `AMPLIHACK_MERGE_GUARD=1`, for auto-drive recipes and for
//! any run that inherits the marker. It fails closed, before the runner
//! starts, when the shim is missing.
//!
//! Runs the real `amplihack` binary. The recipe runner is a stub (through
//! `RECIPE_RUNNER_RS_PATH`) that records the `PATH` and marker it was given
//! and, with `RUN_GH_MERGE=1`, plays a phase-1 bash step that runs
//! `gh pr merge` with that `PATH`. `AMPLIHACK_HOME` points at a fixture bundle
//! holding only `amplifier-bundle/tools/gh-merge-guard/gh`.

#![cfg(unix)]

use crate::support::amplihack_bin::amplihack_bin;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const GUARD_REL: &str = "amplifier-bundle/tools/gh-merge-guard";
const MARKER_ENV: &str = "AMPLIHACK_MERGE_GUARD";

fn executable(path: &Path, body: &str, mode: u32) {
    crate::support::exec_stub::write_executable(path, body, mode);
}

/// Records `PATH` and the marker, then reports success in the runner's JSON.
/// With `RUN_GH_MERGE=1` it first does what a phase-1 agent's bash step
/// would: `gh pr merge` and then `gh pr view`, through the `PATH` the runner
/// was given, recording each exit code and stderr.
const STUB_RUNNER: &str = r#"#!/bin/sh
printf '%s' "$PATH" > "$RECORD_DIR/path"
if [ -n "${AMPLIHACK_MERGE_GUARD+set}" ]; then
  printf '%s' "$AMPLIHACK_MERGE_GUARD" > "$RECORD_DIR/marker"
fi
if [ "${RUN_AGENT_SHELL:-}" = 1 ]; then
  # What an agent step's shell tool does: a login shell, whose profile runs
  # first, then the command.
  bash -lc 'type -P gh' > "$RECORD_DIR/login.gh" 2>&1
  bash -lc 'type -t gh' > "$RECORD_DIR/login.gh.kind" 2>&1
  bash -lc 'gh pr merge 1 --squash' 2> "$RECORD_DIR/login.merge.err"
  printf '%s' "$?" > "$RECORD_DIR/login.merge.rc"
  bash -c 'type -P gh' > "$RECORD_DIR/plain.gh" 2>&1
fi
if [ "${RUN_GH_MERGE:-}" = 1 ]; then
  gh pr merge 1 --squash --admin 2> "$RECORD_DIR/merge.err"
  printf '%s' "$?" > "$RECORD_DIR/merge.rc"
  gh pr view 1 2> "$RECORD_DIR/view.err"
  printf '%s' "$?" > "$RECORD_DIR/view.rc"
fi
printf '%s\n' '{"recipe_name":"issue-1551","success":true,"step_results":[],"context":{}}'
"#;

const SHIM_BODY: &str = "#!/usr/bin/env bash\n# amplihack-gh-merge-guard\nexit 0\n";

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    records: PathBuf,
    tools: PathBuf,
    amplihack_home: PathBuf,
}

impl Fixture {
    /// A fixture whose bundle has an executable shim.
    fn new() -> Self {
        let f = Self::without_shim();
        let dir = f.guard_dir();
        f.make_guard_dir();
        executable(&dir.join("gh"), SHIM_BODY, 0o755);
        f
    }

    /// The guard dir at 0755 whatever the umask: G11 refuses a guard dir that
    /// group or others can write.
    fn make_guard_dir(&self) {
        use std::os::unix::fs::PermissionsExt;
        let dir = self.guard_dir();
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("bash-env"), "").unwrap();
        fs::set_permissions(dir.join("bash-env"), fs::Permissions::from_mode(0o644)).unwrap();
        // tools/ too: G11 also refuses a writable directory above the guard.
        for d in [dir.parent().unwrap(), dir.as_path()] {
            fs::set_permissions(d, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    /// A fixture whose bundle holds the shipped shim itself.
    fn with_shipped_shim() -> Self {
        let f = Self::without_shim();
        f.make_guard_dir();
        crate::support::exec_stub::copy_executable(&shipped_shim(), &f.guard_dir().join("gh"));
        fs::copy(
            shipped_shim().with_file_name("bash-env"),
            f.guard_dir().join("bash-env"),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            f.guard_dir().join("bash-env"),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        fs::set_permissions(f.guard_dir().join("gh"), fs::Permissions::from_mode(0o755)).unwrap();
        f
    }

    fn without_shim() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let home = root.join("home");
        let records = root.join("records");
        let tools = root.join("tools");
        let amplihack_home = root.join("ah");
        for d in [&home, &records, &tools, &amplihack_home] {
            fs::create_dir_all(d).unwrap();
        }
        executable(&tools.join("recipe-runner-rs"), STUB_RUNNER, 0o755);
        Self {
            _tmp: tmp,
            root,
            home,
            records,
            tools,
            amplihack_home,
        }
    }

    fn guard_dir(&self) -> PathBuf {
        self.amplihack_home.join(GUARD_REL)
    }

    /// Writes `<file_stem>.yaml` with `name: <name>` and returns its path.
    fn recipe(&self, file_stem: &str, name: &str) -> PathBuf {
        let path = self.root.join(format!("{file_stem}.yaml"));
        fs::write(
            &path,
            format!(
                "name: {name}\ndescription: issue 1551 fixture\nversion: 1.0.0\ncontext: {{}}\n\
                 steps:\n  - id: s\n    type: bash\n    command: \"true\"\n"
            ),
        )
        .unwrap();
        path
    }

    /// `amplihack recipe run <recipe>` with no inherited environment except
    /// a minimal `PATH` (plus `extra_path` in front) and `extra_env`.
    fn run(&self, recipe: &Path, extra_path: Option<&str>, extra_env: &[(&str, &str)]) -> Output {
        self.run_with_args(recipe, &[], extra_path, extra_env)
    }

    fn run_with_args(
        &self,
        recipe: &Path,
        args: &[&str],
        extra_path: Option<&str>,
        extra_env: &[(&str, &str)],
    ) -> Output {
        let mut path = format!("{}:/usr/local/bin:/usr/bin:/bin", self.tools.display());
        if let Some(extra) = extra_path {
            path = format!("{extra}:{path}");
        }
        let mut c = Command::new(amplihack_bin());
        c.arg("recipe")
            .arg("run")
            .arg(recipe)
            .args(args)
            .arg("-w")
            .arg(&self.root)
            .current_dir(&self.root)
            .env_clear()
            .env("PATH", path)
            .env("HOME", &self.home)
            .env("RECORD_DIR", &self.records)
            .env("RECIPE_RUNNER_RS_PATH", self.tools.join("recipe-runner-rs"))
            .env("AMPLIHACK_AGENT_BINARY", "claude")
            .env("AMPLIHACK_HOME", &self.amplihack_home)
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
            .env("AMPLIHACK_SKIP_REFLECTION", "1");
        for (k, v) in extra_env {
            c.env(k, v);
        }
        c.output().expect("failed to run amplihack")
    }

    fn runner_started(&self) -> bool {
        self.records.join("path").exists()
    }

    fn runner_path(&self) -> Vec<String> {
        let raw =
            fs::read_to_string(self.records.join("path")).expect("stub runner was not started");
        raw.split(':').map(str::to_string).collect()
    }

    fn runner_marker(&self) -> Option<String> {
        fs::read_to_string(self.records.join("marker")).ok()
    }

    /// Entries of the runner's PATH that are the guard directory.
    fn guard_entries(&self) -> Vec<usize> {
        // No guard dir in the bundle: nothing on PATH can be it.
        let Ok(guard) = self.guard_dir().canonicalize() else {
            return Vec::new();
        };
        self.runner_path()
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                Path::new(e.as_str())
                    .canonicalize()
                    .map(|p| p == guard)
                    .unwrap_or(false)
            })
            .map(|(i, _)| i)
            .collect()
    }
}

fn describe(out: &Output) -> String {
    format!(
        "status: {}\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn assert_guarded(f: &Fixture, out: &Output, what: &str) {
    assert!(out.status.success(), "{what}: {}", describe(out));
    assert_eq!(
        f.guard_entries(),
        vec![0],
        "{what}: the guard dir must be first on PATH exactly once; PATH = {:?}",
        f.runner_path()
    );
    assert_eq!(
        f.runner_marker().as_deref(),
        Some("1"),
        "{what}: {MARKER_ENV} must be 1"
    );
}

fn assert_unguarded(f: &Fixture, out: &Output, what: &str) {
    assert!(out.status.success(), "{what}: {}", describe(out));
    assert!(
        f.guard_entries().is_empty(),
        "{what}: the guard dir must not be on PATH; PATH = {:?}",
        f.runner_path()
    );
    assert_eq!(
        f.runner_marker(),
        None,
        "{what}: {MARKER_ENV} must not be set"
    );
}

#[test]
fn autodrive_recipe_gets_the_guard_first_on_path_and_the_marker() {
    let f = Fixture::new();
    let recipe = f.recipe("autodrive-build", "autodrive-build");
    let out = f.run(&recipe, None, &[]);
    assert_guarded(&f, &out, "autodrive-build");
}

#[test]
fn auto_drive_to_merge_gets_the_guard() {
    let f = Fixture::new();
    let recipe = f.recipe("auto-drive-to-merge", "auto-drive-to-merge");
    let out = f.run(&recipe, None, &[]);
    assert_guarded(&f, &out, "auto-drive-to-merge");
}

#[test]
fn autodrive_file_stem_alone_gets_the_guard() {
    let f = Fixture::new();
    let recipe = f.recipe("autodrive-merge-loop", "renamed-loop");
    let out = f.run(&recipe, None, &[]);
    assert_guarded(&f, &out, "autodrive-* file stem with another name:");
}

#[test]
fn autodrive_name_alone_gets_the_guard() {
    let f = Fixture::new();
    let recipe = f.recipe("plain-file", "autodrive-crusty-round");
    let out = f.run(&recipe, None, &[]);
    assert_guarded(&f, &out, "autodrive-* name in a plain file");
}

#[test]
fn unrelated_recipe_gets_no_guard_and_no_marker() {
    let f = Fixture::new();
    for (stem, name) in [
        ("quality-loop", "quality-loop"),
        ("default-workflow", "default-workflow"),
        ("autodrivex", "autodrivex"),
    ] {
        let recipe = f.recipe(stem, name);
        let out = f.run(&recipe, None, &[]);
        assert_unguarded(&f, &out, stem);
        let _ = fs::remove_file(f.records.join("marker"));
    }
}

/// `-c no_merge=true` is a context value, not a guard trigger.
#[test]
fn no_merge_context_alone_does_not_install_the_guard() {
    let f = Fixture::new();
    let recipe = f.recipe("quality-loop", "quality-loop");
    let out = f.run_with_args(&recipe, &["-c", "no_merge=true"], None, &[]);
    assert_unguarded(&f, &out, "quality-loop with no_merge=true");
}

#[test]
fn inherited_marker_guards_an_unrelated_recipe() {
    for value in ["1", "yes"] {
        let f = Fixture::new();
        let recipe = f.recipe("quality-loop", "quality-loop");
        let out = f.run(&recipe, None, &[(MARKER_ENV, value)]);
        assert_guarded(&f, &out, &format!("inherited {MARKER_ENV}={value}"));
    }
}

#[test]
fn inherited_empty_marker_does_not_guard() {
    let f = Fixture::new();
    let recipe = f.recipe("quality-loop", "quality-loop");
    let out = f.run(&recipe, None, &[(MARKER_ENV, "")]);
    assert!(out.status.success(), "{}", describe(&out));
    assert!(
        f.guard_entries().is_empty(),
        "an empty inherited marker must not install the guard; PATH = {:?}",
        f.runner_path()
    );
}

#[test]
fn guard_already_on_path_is_kept_once_and_first() {
    let f = Fixture::new();
    let recipe = f.recipe("autodrive-build", "autodrive-build");
    let guard = f.guard_dir().display().to_string();
    // Nested run: the parent already put the guard in front, then something
    // else in front of it.
    let extra = format!("/opt/elsewhere:{guard}");
    let out = f.run(&recipe, Some(&extra), &[(MARKER_ENV, "1")]);
    assert_guarded(&f, &out, "PATH already holding the guard dir");
}

#[test]
fn missing_shim_fails_closed_before_the_runner_starts() {
    let f = Fixture::without_shim();
    let recipe = f.recipe("autodrive-build", "autodrive-build");
    let out = f.run(&recipe, None, &[]);
    assert!(!out.status.success(), "{}", describe(&out));
    let stderr = String::from_utf8_lossy(&out.stderr);
    let want = format!(
        "amplihack: merge guard not found at {}; refusing to run auto-drive without it (#1551)",
        f.guard_dir().display()
    );
    assert!(stderr.contains(&want), "want {want:?}\n{}", describe(&out));
    assert!(!f.runner_started(), "the runner must not be started");
}

#[test]
fn non_executable_shim_fails_closed_before_the_runner_starts() {
    let f = Fixture::without_shim();
    let dir = f.guard_dir();
    f.make_guard_dir();
    executable(&dir.join("gh"), SHIM_BODY, 0o644);
    let recipe = f.recipe("auto-drive-to-merge", "auto-drive-to-merge");
    let out = f.run(&recipe, None, &[]);
    assert!(!out.status.success(), "{}", describe(&out));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("refusing to run auto-drive without it (#1551)"),
        "{}",
        describe(&out)
    );
    assert!(!f.runner_started(), "the runner must not be started");
}

#[test]
fn shim_that_is_a_directory_fails_closed() {
    let f = Fixture::without_shim();
    fs::create_dir_all(f.guard_dir().join("gh")).unwrap();
    let recipe = f.recipe("autodrive-build", "autodrive-build");
    let out = f.run(&recipe, None, &[]);
    assert!(!out.status.success(), "{}", describe(&out));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("refusing to run auto-drive without it"),
        "{}",
        describe(&out)
    );
    assert!(!f.runner_started(), "the runner must not be started");
}

#[test]
fn inherited_marker_with_missing_shim_fails_closed() {
    let f = Fixture::without_shim();
    let recipe = f.recipe("quality-loop", "quality-loop");
    let out = f.run(&recipe, None, &[(MARKER_ENV, "1")]);
    assert!(!out.status.success(), "{}", describe(&out));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("refusing to run auto-drive without it"),
        "{}",
        describe(&out)
    );
    assert!(!f.runner_started(), "the runner must not be started");
}

#[test]
fn unrelated_recipe_without_a_shim_still_runs() {
    let f = Fixture::without_shim();
    let recipe = f.recipe("quality-loop", "quality-loop");
    let out = f.run(&recipe, None, &[]);
    assert_unguarded(&f, &out, "quality-loop without a bundle shim");
}

fn shipped_shim() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(GUARD_REL)
        .join("gh")
}

/// The shipped bundle carries the shim, executable, with its marker line.
#[test]
fn shipped_bundle_has_an_executable_shim() {
    use std::os::unix::fs::PermissionsExt;
    let shim = shipped_shim();
    let meta = fs::metadata(&shim).unwrap_or_else(|e| panic!("{}: {e}", shim.display()));
    assert!(meta.is_file(), "{} must be a file", shim.display());
    assert!(
        meta.permissions().mode() & 0o111 != 0,
        "{} must be executable",
        shim.display()
    );
    let body = fs::read_to_string(&shim).unwrap();
    // Privileged mode: bash neither sources BASH_ENV nor imports exported
    // functions, so neither can stand in for the gate identity checks.
    assert!(
        body.starts_with("#!/bin/bash -p\n"),
        "{} must start with #!/bin/bash -p",
        shim.display()
    );
    assert!(body.lines().any(|l| l == "# amplihack-gh-merge-guard"));
}

/// G11: a shim or guard dir that group or others can write fails closed
/// before the runner starts.
#[test]
fn writable_shim_or_guard_dir_fails_closed_before_the_runner_starts() {
    use std::os::unix::fs::PermissionsExt;
    for (shim_mode, dir_mode) in [
        (0o775, 0o755),
        (0o757, 0o755),
        (0o755, 0o775),
        (0o755, 0o777),
    ] {
        let f = Fixture::without_shim();
        f.make_guard_dir();
        executable(&f.guard_dir().join("gh"), SHIM_BODY, shim_mode);
        fs::set_permissions(f.guard_dir(), fs::Permissions::from_mode(dir_mode)).unwrap();
        let recipe = f.recipe("autodrive-build", "autodrive-build");
        let out = f.run(&recipe, None, &[]);
        let what = format!("shim {shim_mode:o}, dir {dir_mode:o}");
        assert!(!out.status.success(), "{what}: {}", describe(&out));
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("(#1551)"), "{what}: {}", describe(&out));
        assert!(
            !f.runner_started(),
            "{what}: the runner must not be started"
        );
        fs::set_permissions(f.guard_dir(), fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// A stand-in for the real gh, behind the guard on PATH: logs every argv and
/// succeeds, so a merge that got through would show in its log.
const REAL_GH_STUB: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$RECORD_DIR/gh.log"
exit 0
"#;

/// Runs a phase-1 recipe whose step calls `gh pr merge` through the PATH the
/// runner builds, and returns (merge rc, merge stderr, view rc, gh.log).
fn phase1_step_runs_gh_merge(f: &Fixture) -> (String, String, String, String) {
    let real_gh_dir = f.root.join("real-gh");
    fs::create_dir_all(&real_gh_dir).unwrap();
    executable(&real_gh_dir.join("gh"), REAL_GH_STUB, 0o755);
    let recipe = f.recipe("autodrive-build", "autodrive-build");
    let out = f.run(
        &recipe,
        Some(&real_gh_dir.display().to_string()),
        &[("RUN_GH_MERGE", "1")],
    );
    assert!(out.status.success(), "{}", describe(&out));
    let read = |name: &str| fs::read_to_string(f.records.join(name)).unwrap_or_default();
    (
        read("merge.rc"),
        read("merge.err"),
        read("view.rc"),
        read("gh.log"),
    )
}

/// G2, end to end: an agent step inside phase 1 that runs `gh pr merge` is
/// refused by the shipped shim the runner put first on PATH. Exit 3, the
/// REFUSED line on stderr, and the real gh never saw a merge. A non-merge
/// call from the same step still reaches the real gh.
#[cfg(target_os = "linux")]
#[test]
fn phase1_step_gh_pr_merge_is_refused_end_to_end() {
    let f = Fixture::with_shipped_shim();
    let (merge_rc, merge_err, view_rc, log) = phase1_step_runs_gh_merge(&f);
    assert_eq!(
        merge_rc, "3",
        "gh pr merge must exit 3; stderr: {merge_err}"
    );
    assert!(
        merge_err.contains("REFUSED (amplihack #1551)"),
        "stderr must carry the refusal: {merge_err:?}"
    );
    assert!(
        !log.lines().any(|l| l.starts_with("pr merge")),
        "the real gh must never see the merge; gh.log: {log:?}"
    );
    assert_eq!(
        view_rc, "0",
        "gh pr view must pass through; gh.log: {log:?}"
    );
    assert!(
        log.lines().any(|l| l == "pr view 1"),
        "gh pr view must reach the real gh unchanged; gh.log: {log:?}"
    );
}

/// The non-Linux companion: without /proc the gate cannot be identified, so
/// the shim must still never let a merge through (fail closed). The exact
/// exit code may differ with the platform's bash, but it is never 0 and the
/// real gh never sees the merge.
#[cfg(not(target_os = "linux"))]
#[test]
fn phase1_step_gh_pr_merge_fails_closed_off_linux() {
    let f = Fixture::with_shipped_shim();
    let (merge_rc, merge_err, _view_rc, log) = phase1_step_runs_gh_merge(&f);
    assert!(
        !merge_rc.is_empty() && merge_rc != "0",
        "gh pr merge must fail; rc {merge_rc:?}, stderr: {merge_err}"
    );
    assert!(
        !log.lines().any(|l| l.starts_with("pr merge")),
        "the real gh must never see the merge; gh.log: {log:?}"
    );
}

/// Crusty round 8, structural: an agent step's login shell whose profile
/// prepends a directory holding another `gh` (as `~/.local/bin` or Homebrew
/// would) still resolves `gh` to the shim, because the runner's `BASH_ENV`
/// puts the guard back first after the profile has run. `gh pr merge` from
/// that shell exits 3 and never reaches the other gh. No command text is read
/// here: this is PATH resolution only.
#[cfg(target_os = "linux")]
#[test]
fn agent_login_shell_resolves_gh_to_the_shim_after_a_profile_reorders_path() {
    let f = Fixture::with_shipped_shim();
    let profile_bin = f.home.join(".local/bin");
    fs::create_dir_all(&profile_bin).unwrap();
    executable(&profile_bin.join("gh"), REAL_GH_STUB, 0o755);
    let prepend = format!("export PATH=\"{}:$PATH\"\n", profile_bin.display());
    for profile in [".bash_profile", ".bashrc", ".profile"] {
        fs::write(f.home.join(profile), &prepend).unwrap();
    }

    let recipe = f.recipe("autodrive-build", "autodrive-build");
    let out = f.run(&recipe, None, &[("RUN_AGENT_SHELL", "1")]);
    assert!(out.status.success(), "{}", describe(&out));
    let read = |name: &str| fs::read_to_string(f.records.join(name)).unwrap_or_default();
    let shim = f.guard_dir().join("gh").display().to_string();

    assert_eq!(
        read("login.gh").trim(),
        shim,
        "bash -lc must find the shim first on PATH despite the profile"
    );
    assert_eq!(
        read("login.gh.kind").trim(),
        "function",
        "bash -lc must run gh through the guard's function"
    );
    assert_eq!(
        read("plain.gh").trim(),
        shim,
        "bash -c must resolve gh to the shim"
    );
    let merge_err = read("login.merge.err");
    assert_eq!(read("login.merge.rc"), "3", "stderr: {merge_err}");
    assert!(
        merge_err.contains("REFUSED (amplihack #1551)"),
        "{merge_err}"
    );
    assert!(
        !read("gh.log").lines().any(|l| l.starts_with("pr merge")),
        "the profile's gh must never see the merge"
    );
}

/// The shipped bash-env keeps a user's own BASH_ENV working: it is sourced
/// first, then the guard is put back in front.
#[cfg(target_os = "linux")]
#[test]
fn shipped_bash_env_sources_the_users_bash_env_then_puts_the_guard_first() {
    let f = Fixture::with_shipped_shim();
    let user_env = f.home.join("user-bash-env");
    fs::write(
        &user_env,
        "export FROM_USER_BASH_ENV=yes\nexport PATH=/elsewhere:$PATH\n",
    )
    .unwrap();
    let out = Command::new("bash")
        .arg("-c")
        .arg("printf '%s|%s' \"$FROM_USER_BASH_ENV\" \"${PATH%%:*}\"")
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", &f.home)
        .env("BASH_ENV", f.guard_dir().join("bash-env"))
        .env("AMPLIHACK_MERGE_GUARD_USER_BASH_ENV", &user_env)
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        format!("yes|{}", f.guard_dir().display()),
        "{}",
        describe(&out)
    );
}
