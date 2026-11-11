//! Issue #1954: an install that runs while a recipe run is alive must leave the
//! run alone.
//!
//! The test drives the real `amplihack` binary in a temp HOME. After a first
//! install, two processes start:
//!
//! - **A**, the "run": a bash script under `set -euo pipefail` whose working
//!   directory is the installed bundle. Each pass re-reads bundle files by
//!   relative and absolute path, sources a bundle file both ways and execs
//!   `amplihack --version` and `amplihack-hooks`. It keeps fd 3 open on a
//!   bundle file for its whole life. It does three passes, says it is
//!   ready, blocks until the test says go, then does three more passes.
//! - **S**, a sentinel that only records `TERM`, `HUP` and `INT` and blocks
//!   until the test says go.
//!
//! While they are parked: two `install --force-refresh` (the second one
//! concurrent with a self-heal trigger, so the install lock is exercised)
//! re-stage the same version. `update` takes the same stage-and-switch path
//! for its post-update install, and cannot download offline, so this is the
//! offline stand-in for it (D12).
//!
//! No timers: every wait is a FIFO read or a `Child::wait` (see
//! `support/events.rs`). The published links are read before install 1,
//! between the installs and after install 2; that they are never missing in
//! between follows from `rename(2)` and is tested at the syscall level in
//! `src/commands/install/tests/issue_1954_release_layout.rs`.
//!
//! Acceptance checks (issue #1954):
//! - AC1: A finishes normally.
//! - AC2: no signal reaches S, and S finishes normally.
//! - AC3: each published path is, at every read, a symlink into
//!   `~/.amplihack/releases/<version>…/` with `.complete`.
//! - AC4: A's working directory and open file were never deleted under it.
//! - AC5: once A exits, a later install removes the release it used.
//! - AC6: concurrent install and self-heal both succeed, the stamp is current
//!   and no staging leftovers remain.
//!
//! Every check is collected and reported together, so the failure on the
//! pre-fix code (6b32c7e69's parent) names every acceptance criterion that
//! does not hold.

use crate::support::amplihack_bin::amplihack_bin;
use crate::support::events::{SCRIPT_PREAMBLE, await_line, mkfifo, open_ready, release};
use crate::support::exec_stub::write_executable;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output, Stdio};

pub(crate) const BUNDLE_REL: &str = ".amplihack/amplifier-bundle";
pub(crate) const PROBE_REL: &str = "context/issue-1954-probe.sh";
const RECIPE_REL: &str = "recipes/smart-orchestrator.yaml";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|dir| dir.join("amplifier-bundle").is_dir() && dir.join("Cargo.toml").is_file())
        .expect("the workspace root")
        .to_path_buf()
}

pub(crate) fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let kind = entry.file_type().unwrap();
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &target);
        } else if kind.is_file() {
            fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// A source tree with the real bundle declaring `version`, plus a probe file
/// the run sources on every pass.
fn source_tree(root: &Path, version: &str) {
    copy_tree(
        &workspace_root().join("amplifier-bundle"),
        &root.join("amplifier-bundle"),
    );
    fs::write(
        root.join("amplifier-bundle").join(PROBE_REL),
        "ISSUE_1954_PROBE=ok\n",
    )
    .unwrap();
    fs::write(root.join("CLAUDE.md"), "root\n").unwrap();
    fs::write(
        root.join("Cargo.toml"),
        format!(
            "[workspace]\nmembers = [\"crates/amplihack-cli\"]\n\n\
             [workspace.package]\nversion = \"{version}\"\n"
        ),
    )
    .unwrap();
}

/// A child this test started. Dropped without [`Parked::finish`] (a panic),
/// it is killed through its own `Child` handle and reaped: the only signal a
/// test may send, and only to its own child.
pub(crate) struct Parked {
    child: Option<Child>,
    pub(crate) go: PathBuf,
    /// The ready line: `ready <pid> <cwd>`.
    pub(crate) ready: String,
}

impl Parked {
    pub(crate) fn pid(&self) -> String {
        self.ready
            .split_whitespace()
            .nth(1)
            .expect("ready <pid> <cwd>")
            .to_string()
    }

    pub(crate) fn cwd(&self) -> String {
        self.ready
            .splitn(3, ' ')
            .nth(2)
            .expect("ready <pid> <cwd>")
            .to_string()
    }

    /// Says go, then waits for the child to exit.
    pub(crate) fn finish(mut self) -> ExitStatus {
        release(&self.go, "go");
        self.child.take().unwrap().wait().unwrap()
    }
}

impl Drop for Parked {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub(crate) struct Host {
    pub(crate) temp: tempfile::TempDir,
    pub(crate) home: PathBuf,
    pub(crate) stubs: PathBuf,
    pub(crate) tree: PathBuf,
    pub(crate) work: PathBuf,
}

impl Host {
    pub(crate) fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        fs::create_dir_all(&home).unwrap();
        let stubs = temp.path().join("stubs");
        fs::create_dir_all(&stubs).unwrap();
        let work = temp.path().join("work");
        fs::create_dir_all(&work).unwrap();
        let padding = "x".repeat(1100);
        write_executable(
            &stubs.join("amplihack-hooks"),
            format!("#!/bin/sh\nexit 0\n# {padding}\n"),
            0o755,
        );
        write_executable(
            &stubs.join("recipe-runner-rs"),
            format!(
                "#!/bin/sh\nprintf '%s\\n' '{{\"schema_version\":1,\"version\":\"fixture\",\
                 \"capabilities\":[\"codex_exec\",\"durable_recovery_v1\"]}}'\nexit 0\n# {padding}\n"
            ),
            0o755,
        );
        // Every `git` fails, so nothing reaches the network.
        write_executable(&stubs.join("git"), "#!/bin/sh\nexit 1\n", 0o755);
        let tree = temp.path().join("tag-worktree");
        source_tree(&tree, amplihack_cli::VERSION);
        let host = Self {
            temp,
            home,
            stubs,
            tree,
            work,
        };
        host.assert_contained();
        host
    }

    /// S-D1: every path a child may write is inside this test's temp dir,
    /// so nothing reaches the real `~/.amplihack` or a live run on the host.
    pub(crate) fn assert_contained(&self) {
        let root = self.temp.path();
        for path in [&self.home, &self.stubs, &self.tree, &self.work] {
            assert!(
                path.starts_with(root),
                "{} is outside the test's temp dir {}",
                path.display(),
                root.display()
            );
        }
        if let Some(real) = std::env::var_os("HOME") {
            assert_ne!(Path::new(&real), self.home.as_path());
        }
    }

    pub(crate) fn bundle(&self) -> PathBuf {
        self.home.join(BUNDLE_REL)
    }

    pub(crate) fn releases(&self) -> PathBuf {
        self.home.join(".amplihack/releases")
    }

    /// The environment every `amplihack` the test starts gets, directly or
    /// through a script, besides `HOME`.
    pub(crate) fn install_env(&self) -> Vec<(&'static str, std::ffi::OsString)> {
        vec![
            ("AMPLIHACK_HOME", self.tree.clone().into_os_string()),
            (
                "PATH",
                format!("{}:/usr/local/bin:/usr/bin:/bin", self.stubs.display()).into(),
            ),
            (
                "AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH",
                self.stubs.join("amplihack-hooks").into_os_string(),
            ),
            (
                "RECIPE_RUNNER_RS_PATH",
                self.stubs.join("recipe-runner-rs").into_os_string(),
            ),
            ("AMPLIHACK_SKIP_RECIPE_RUNNER_INSTALL", "1".into()),
            ("AMPLIHACK_SKIP_MMDC", "1".into()),
            ("AMPLIHACK_NO_UPDATE_CHECK", "1".into()),
            ("AMPLIHACK_NONINTERACTIVE", "1".into()),
            ("AMPLIHACK_AGENT_BINARY", "claude".into()),
        ]
    }

    pub(crate) fn amplihack(&self, args: &[&str]) -> Command {
        let mut command = crate::isolated_amplihack::isolated(amplihack_bin());
        command
            .args(args)
            .current_dir(self.temp.path())
            .env("HOME", &self.home)
            .envs(self.install_env())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    pub(crate) fn install(&self, args: &[&str]) -> Output {
        let mut all = vec!["install"];
        all.extend_from_slice(args);
        self.amplihack(&all).output().unwrap()
    }

    /// Starts `body` (after [`SCRIPT_PREAMBLE`]) under bash from `work/`
    /// with this host's HOME and a minimal environment, and blocks until it
    /// writes its ready line. Its stdout and stderr go to
    /// `work/<name>.out|err`; `extra_env` is added to the environment.
    pub(crate) fn park_script(
        &self,
        name: &str,
        body: &str,
        extra_env: &[(&str, &std::ffi::OsStr)],
    ) -> Parked {
        self.assert_contained();
        let script = self.work.join(format!("{name}.sh"));
        fs::write(&script, format!("{SCRIPT_PREAMBLE}{body}")).unwrap();
        let ready = self.work.join(format!("{name}.ready"));
        let go = self.work.join(format!("{name}.go"));
        mkfifo(&ready);
        mkfifo(&go);
        // Open the read end before the child exists, so an early exit
        // cannot lose its `exit:` line (see `support::events`).
        let ready_handle = open_ready(&ready);
        let out = fs::File::create(self.work.join(format!("{name}.out"))).unwrap();
        let err_path = self.work.join(format!("{name}.err"));
        let err = fs::File::create(&err_path).unwrap();
        let mut command = Command::new("/bin/bash");
        command
            .arg(&script)
            .arg(&self.work)
            .arg(&ready)
            .arg(&go)
            .env_clear()
            .env("HOME", &self.home)
            .env("TMPDIR", self.temp.path())
            .env("PATH", format!("{}:/usr/bin:/bin", self.stubs.display()))
            .env("LC_ALL", "C");
        for (key, value) in extra_env {
            command.env(key, value);
        }
        let child = command
            .current_dir(&self.work)
            .stdin(Stdio::null())
            .stdout(out)
            .stderr(err)
            .spawn()
            .unwrap();
        let mut parked = Parked {
            child: Some(child),
            go,
            ready: String::new(),
        };
        // A panic here drops `parked`, which kills and reaps the child.
        parked.ready = await_line(ready_handle, &ready, &err_path);
        parked
    }

    pub(crate) fn work_file(&self, name: &str) -> String {
        fs::read_to_string(self.work.join(name)).unwrap_or_default()
    }
}

pub(crate) fn text(output: &Output) -> String {
    format!(
        "status: {:?}\n--- stdout\n{}\n--- stderr\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// The run (A). See the module docs. Follows [`SCRIPT_PREAMBLE`].
pub(crate) const RUN_SCRIPT: &str = r#"set -euo pipefail
trap 'echo "run failed at line $LINENO (pass ${i:-0}): $BASH_COMMAND; cwd $(readlink /proc/$$/cwd)" >&2' ERR
bundle="$HOME/.amplihack/amplifier-bundle"
cd "$bundle"
exec 3< ./recipes/smart-orchestrator.yaml
pass() {
  test -d .
  test -f ./recipes/smart-orchestrator.yaml
  cat ./recipes/smart-orchestrator.yaml > /dev/null
  ls ./recipes > /dev/null
  ISSUE_1954_PROBE=
  . ./context/issue-1954-probe.sh
  test "$ISSUE_1954_PROBE" = ok
  ISSUE_1954_PROBE=
  . "$bundle/context/issue-1954-probe.sh"
  test "$ISSUE_1954_PROBE" = ok
  cat "$bundle/recipes/smart-orchestrator.yaml" > /dev/null
  "$HOME/.local/bin/amplihack" --version > /dev/null
  "$HOME/.local/bin/amplihack-hooks" > /dev/null
}
for i in 1 2 3; do pass; done
echo "ready $$ $(pwd -P)" >&9
read -r _ <&8
for i in 4 5 6; do pass; done
cat <&3 > /dev/null
echo "DONE $i" > "$work/a.done"
"#;

/// The signal sentinel (S). Follows [`SCRIPT_PREAMBLE`]. A trapped signal
/// interrupts `read`, so it reads again until the test says go.
const SENTINEL_SCRIPT: &str = r#"trap 'echo "SIGNAL TERM" >> "$work/sentinel.log"' TERM
trap 'echo "SIGNAL HUP" >> "$work/sentinel.log"' HUP
trap 'echo "SIGNAL INT" >> "$work/sentinel.log"' INT
cd "$HOME/.amplihack/amplifier-bundle"
echo "ready $$ $(pwd -P)" >&9
until read -r _ <&8; do :; done
echo OK > "$work/s.done"
"#;

/// Whether `target` names `<releases>/<version…>/<rel>`, by component.
pub(crate) fn names_release(releases: &Path, target: &Path, rel: &str) -> Result<PathBuf, String> {
    let Ok(suffix) = target.strip_prefix(releases) else {
        return Err(format!(
            "{} is not under {}",
            target.display(),
            releases.display()
        ));
    };
    let mut parts = suffix.components();
    let Some(release) = parts.next() else {
        return Err(format!("{} names releases/ itself", target.display()));
    };
    let release_name = release.as_os_str().to_string_lossy().into_owned();
    if !release_name.starts_with(amplihack_cli::VERSION) {
        return Err(format!(
            "release {release_name} does not start with v{}",
            amplihack_cli::VERSION
        ));
    }
    if parts.as_path() != Path::new(rel) {
        return Err(format!("{} is not <release>/{rel}", target.display()));
    }
    Ok(releases.join(release_name))
}

/// The release a published link points to, checked for AC3.
pub(crate) fn published_release(host: &Host, link: &Path, rel: &str) -> Result<PathBuf, String> {
    let meta = fs::symlink_metadata(link).map_err(|e| format!("{}: {e}", link.display()))?;
    if !meta.file_type().is_symlink() {
        return Err(format!(
            "{} is not a symlink (file type {:?})",
            link.display(),
            meta.file_type()
        ));
    }
    let target = fs::read_link(link).unwrap();
    let target = if target.is_absolute() {
        target
    } else {
        link.parent().unwrap().join(target)
    };
    let release = names_release(&host.releases(), &target, rel)?;
    if !release.join(".complete").is_file() {
        return Err(format!("{} has no .complete", release.display()));
    }
    Ok(release)
}

#[cfg(target_os = "linux")]
pub(crate) fn proc_link(pid: &str, entry: &str) -> String {
    fs::read_link(format!("/proc/{pid}/{entry}"))
        .map(|p| p.display().to_string())
        .unwrap_or_else(|e| format!("<unreadable: {e}>"))
}

/// Same-uid processes whose `/proc` entries cannot be read (for example a
/// non-dumpable `ssh-agent`). The design keeps every candidate when the scan
/// is unknown, so AC5 can only be checked on a host without them.
#[cfg(target_os = "linux")]
fn unreadable_same_uid_process() -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    let uid = fs::metadata("/proc/self").ok()?.uid();
    for entry in fs::read_dir("/proc").ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if meta.uid() != uid {
            continue;
        }
        if let Err(e) = fs::read_link(entry.path().join("cwd"))
            && e.kind() == std::io::ErrorKind::PermissionDenied
        {
            return Some(name);
        }
    }
    None
}

/// One read of the three published paths, for AC3: each must be a symlink
/// into a complete release. `label` names the moment of the read.
fn check_links(host: &Host, label: &str, failures: &mut Vec<String>) -> String {
    let local_bin = host.home.join(".local/bin");
    let mut seen = Vec::new();
    for (link, rel) in [
        (host.bundle(), "amplifier-bundle"),
        (local_bin.join("amplihack"), "bin/amplihack"),
        (local_bin.join("amplihack-hooks"), "bin/amplihack-hooks"),
    ] {
        match published_release(host, &link, rel) {
            Ok(release) => seen.push(release.display().to_string()),
            Err(e) => {
                seen.push(format!("<{e}>"));
                failures.push(format!("AC3 ({label}): {e}"));
            }
        }
    }
    format!("{label}: {}", seen.join(" | "))
}

#[cfg(target_os = "linux")]
#[test]
fn install_and_self_heal_leave_a_live_run_alone() {
    let host = Host::new();
    let mut failures: Vec<String> = Vec::new();
    let mut targets: Vec<String> = Vec::new();

    let first = host.install(&[]);
    assert!(first.status.success(), "first install: {}", text(&first));
    assert!(host.bundle().join(PROBE_REL).is_file(), "{}", text(&first));

    // Each blocks until its child has written `ready`: no timer.
    let run = host.park_script("a", RUN_SCRIPT, &[]);
    let sentinel = host.park_script("s", SENTINEL_SCRIPT, &[]);
    let a_pid = run.pid();
    let a_cwd = run.cwd();

    targets.push(check_links(&host, "before install 1", &mut failures));

    // Install 1: a same-version re-stage while the run is alive.
    let second = host.install(&["--force-refresh"]);
    if !second.status.success() {
        failures.push(format!(
            "install --force-refresh #1 failed: {}",
            text(&second)
        ));
    }
    targets.push(check_links(&host, "between the installs", &mut failures));

    // Install 2 concurrent with a self-heal: the stamp is made stale, so the
    // next ordinary command re-stages under the install lock.
    fs::write(host.home.join(".amplihack/.installed-version"), "0.0.1\n").unwrap();
    let install_two = host
        .amplihack(&["install", "--force-refresh"])
        .spawn()
        .unwrap();
    let self_heal = host.amplihack(&["recipe", "list"]).spawn().unwrap();
    let install_two = install_two.wait_with_output().unwrap();
    let self_heal = self_heal.wait_with_output().unwrap();
    if !install_two.status.success() {
        failures.push(format!(
            "install --force-refresh #2 failed: {}",
            text(&install_two)
        ));
    }
    if self_heal.status.code().is_none() {
        failures.push(format!(
            "AC6: the self-heal process was ended by a signal: {}",
            text(&self_heal)
        ));
    }
    targets.push(check_links(&host, "after install 2", &mut failures));

    // AC4 while the run is still parked: nothing it holds was deleted.
    let cwd_now = proc_link(&a_pid, "cwd");
    let fd_now = proc_link(&a_pid, "fd/3");
    if cwd_now.ends_with(" (deleted)") {
        failures.push(format!(
            "AC4: the run's working directory was deleted: {cwd_now}"
        ));
    }
    if fd_now.ends_with(" (deleted)") {
        failures.push(format!(
            "AC4: the run's open bundle file was deleted: {fd_now}"
        ));
    }
    if let Err(e) = names_release(&host.releases(), Path::new(&a_cwd), "amplifier-bundle") {
        failures.push(format!(
            "AC4: the run's working directory is not inside a release: {e}"
        ));
    } else if !Path::new(&a_cwd).join(RECIPE_REL).is_file() {
        failures.push(format!(
            "AC4: the release the run uses was removed while it was alive: {a_cwd}"
        ));
    }

    // Release both, then join them.
    let run_status = run.finish();
    let sentinel_status = sentinel.finish();

    // AC1: the run finished normally.
    let a_done = host.work_file("a.done");
    if !(run_status.success() && a_done.starts_with("DONE ")) {
        failures.push(format!(
            "AC1: the run did not finish normally (status {run_status:?}, done {:?})\n\
             a.err:\n{}",
            a_done.trim(),
            host.work_file("a.err")
        ));
    }

    // AC2: no signal reached the sentinel.
    let signals = host.work_file("sentinel.log");
    if !signals.is_empty() {
        failures.push(format!("AC2: the sentinel received signals: {signals}"));
    }
    if !(sentinel_status.success() && host.work_file("s.done").contains("OK")) {
        failures.push(format!(
            "AC2: the sentinel did not finish normally (status {sentinel_status:?})"
        ));
    }

    // AC6: the stamp is current and no staging leftovers remain.
    let stamp =
        fs::read_to_string(host.home.join(".amplihack/.installed-version")).unwrap_or_default();
    if stamp.lines().next() != Some(amplihack_cli::VERSION) {
        failures.push(format!(
            "AC6: the stamp is {:?} after install + self-heal, not v{}\nself-heal: {}",
            stamp.lines().next(),
            amplihack_cli::VERSION,
            text(&self_heal)
        ));
    }
    for leftover in [
        ".amplihack/amplifier-bundle.staging",
        ".amplihack/amplifier-bundle.old",
    ] {
        if host.home.join(leftover).exists() {
            failures.push(format!("AC6: leftover {leftover} after the installs"));
        }
    }

    // AC5: once the run has exited, a later install removes the release it
    // used. Only meaningful on a host where the in-use scan can be known.
    match unreadable_same_uid_process() {
        Some(pid) => eprintln!(
            "issue #1954 AC5 not checked: same-uid pid {pid} has unreadable /proc entries, \
             so the in-use scan is unknown and keeps every release by design"
        ),
        None => {
            let last = host.install(&["--force-refresh"]);
            if !last.status.success() {
                failures.push(format!("AC5: the final install failed: {}", text(&last)));
            } else if names_release(&host.releases(), Path::new(&a_cwd), "amplifier-bundle").is_ok()
                && Path::new(&a_cwd).exists()
            {
                // Three installs ran after the one the run used, so its
                // release is neither the current nor the previous target.
                failures.push(format!(
                    "AC5: the release the finished run used is still on disk: {a_cwd}"
                ));
            }
            let retired = host.releases().join(".retired");
            let left: Vec<String> = fs::read_dir(&retired)
                .map(|it| {
                    it.flatten()
                        .map(|e| e.file_name().to_string_lossy().into_owned())
                        .collect()
                })
                .unwrap_or_default();
            if !left.is_empty() {
                failures.push(format!(
                    "AC5: unused retired trees remain after the final install: {left:?}"
                ));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "issue #1954: {} acceptance failure(s):\n\n{}\n\nlinks read:\n{}",
        failures.len(),
        failures.join("\n\n"),
        targets.join("\n")
    );
}

/// AC4 on its own, for a fast signal: after a re-stage, the directory the
/// previous install published still holds every file, unchanged.
#[cfg(target_os = "linux")]
#[test]
fn a_restage_never_changes_or_removes_the_previous_release() {
    let host = Host::new();
    let first = host.install(&[]);
    assert!(first.status.success(), "{}", text(&first));
    let before = fs::canonicalize(host.bundle()).unwrap();
    let recipe_before = fs::read(before.join(RECIPE_REL)).unwrap();

    let again = host.install(&["--force-refresh"]);
    assert!(again.status.success(), "{}", text(&again));
    let after = fs::canonicalize(host.bundle()).unwrap();

    assert_ne!(
        before,
        after,
        "a re-stage must publish a new release directory, not rewrite {}",
        before.display()
    );
    assert_eq!(
        fs::read(before.join(RECIPE_REL)).ok(),
        Some(recipe_before),
        "the previous release must stay intact: it is the previous target, which cleanup keeps"
    );
    assert!(before.join(PROBE_REL).is_file());
}

/// Writes the pre-#1954 layout: a real bundle directory copied from the tag
/// tree and regular-file binaries in `~/.local/bin`.
pub(crate) fn plant_legacy_layout(host: &Host) {
    copy_tree(&host.tree.join("amplifier-bundle"), &host.bundle());
    let local_bin = host.home.join(".local/bin");
    fs::create_dir_all(&local_bin).unwrap();
    write_executable(
        &local_bin.join("amplihack"),
        "#!/bin/sh\necho legacy\n",
        0o755,
    );
    write_executable(
        &local_bin.join("amplihack-hooks"),
        "#!/bin/sh\nexit 0\n",
        0o755,
    );
}

/// A legacy layout (real bundle directory, regular-file binaries) migrates to
/// links in one install, and a run whose cwd is in the legacy tree survives.
#[cfg(target_os = "linux")]
#[test]
fn migration_from_a_real_bundle_dir_keeps_a_live_run_alive() {
    let host = Host::new();
    plant_legacy_layout(&host);
    let local_bin = host.home.join(".local/bin");

    let run = host.park_script("a", RUN_SCRIPT, &[]);
    let a_pid = run.pid();

    let install = host.install(&[]);
    let cwd_now = proc_link(&a_pid, "cwd");
    let status = run.finish();

    let mut failures = Vec::new();
    if !install.status.success() {
        failures.push(format!(
            "install over a legacy layout failed: {}",
            text(&install)
        ));
    }
    if cwd_now.ends_with(" (deleted)") {
        failures.push(format!(
            "the legacy tree under the run was deleted: {cwd_now}"
        ));
    }
    if !(status.success() && host.work_file("a.done").starts_with("DONE ")) {
        failures.push(format!(
            "the run did not finish normally ({status:?}):\n{}",
            host.work_file("a.err")
        ));
    }
    for (link, rel) in [
        (host.bundle(), "amplifier-bundle"),
        (local_bin.join("amplihack"), "bin/amplihack"),
        (local_bin.join("amplihack-hooks"), "bin/amplihack-hooks"),
    ] {
        if let Err(e) = published_release(&host, &link, rel) {
            failures.push(format!("migration: {e}"));
        }
    }
    // The legacy tree goes to `releases/.retired/`; it is never named
    // `legacy-*`. That it was not deleted under the run is the cwd check above.
    let legacy_named = fs::read_dir(host.releases())
        .map(|it| {
            it.flatten()
                .any(|e| e.file_name().to_string_lossy().starts_with("legacy-"))
        })
        .unwrap_or(false);
    if legacy_named {
        failures
            .push("the legacy tree must go to releases/.retired/, not releases/legacy-*".into());
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
