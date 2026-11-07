//! Issue #1954: an install that runs while a recipe run is alive must leave the
//! run alone.
//!
//! The test drives the real `amplihack` binary in a temp HOME. After a first
//! install, three processes start:
//!
//! - **A**, the "run": a bash loop under `set -euo pipefail` whose working
//!   directory is the installed bundle. Each pass re-reads bundle files by
//!   relative and absolute path, sources a bundle file both ways and execs
//!   `amplihack --version` and `amplihack-hooks`. It keeps fd 3 open on a
//!   bundle file for its whole life.
//! - **S**, a sentinel that only records `TERM`, `HUP` and `INT`.
//! - **L**, a link sampler that records every moment a published path is
//!   missing and every target the links name.
//!
//! While they run: two `install --force-refresh` (the second one concurrent
//! with a self-heal trigger, so the install lock is exercised) re-stage the
//! same version. `update` takes the same stage-and-switch path for its
//! post-update install, and cannot download offline, so this is the offline
//! stand-in for it (D12).
//!
//! Acceptance checks (issue #1954):
//! - AC1: A finishes normally.
//! - AC2: no signal reaches S, and S finishes normally.
//! - AC3: the published paths never go missing; at the end each is a symlink
//!   into `~/.amplihack/releases/<version>…/` with `.complete`.
//! - AC4: A's working directory and open file were never deleted under it.
//! - AC5: once A exits, a later install removes the release it used.
//! - AC6: concurrent install and self-heal both succeed, the stamp is current
//!   and no staging leftovers remain.
//!
//! Every check is collected and reported together, so the failure on
//! `origin/main` names every acceptance criterion that does not hold.

use crate::support::amplihack_bin::amplihack_bin;
use crate::support::exec_stub::write_executable;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

const BUNDLE_REL: &str = ".amplihack/amplifier-bundle";
const PROBE_REL: &str = "context/issue-1954-probe.sh";
const RECIPE_REL: &str = "recipes/smart-orchestrator.yaml";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|dir| dir.join("amplifier-bundle").is_dir() && dir.join("Cargo.toml").is_file())
        .expect("the workspace root")
        .to_path_buf()
}

fn copy_tree(from: &Path, to: &Path) {
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

struct Host {
    temp: tempfile::TempDir,
    home: PathBuf,
    stubs: PathBuf,
    tree: PathBuf,
    work: PathBuf,
}

impl Host {
    fn new() -> Self {
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
                 \"capabilities\":[\"codex_exec\"]}}'\nexit 0\n# {padding}\n"
            ),
            0o755,
        );
        // Every `git` fails, so nothing reaches the network.
        write_executable(&stubs.join("git"), "#!/bin/sh\nexit 1\n", 0o755);
        let tree = temp.path().join("tag-worktree");
        source_tree(&tree, amplihack_cli::VERSION);
        Self {
            temp,
            home,
            stubs,
            tree,
            work,
        }
    }

    fn bundle(&self) -> PathBuf {
        self.home.join(BUNDLE_REL)
    }

    fn releases(&self) -> PathBuf {
        self.home.join(".amplihack/releases")
    }

    fn amplihack(&self, args: &[&str]) -> Command {
        let mut command = crate::isolated_amplihack::isolated(amplihack_bin());
        command
            .args(args)
            .current_dir(self.temp.path())
            .env("HOME", &self.home)
            .env("AMPLIHACK_HOME", &self.tree)
            .env(
                "PATH",
                format!("{}:/usr/local/bin:/usr/bin:/bin", self.stubs.display()),
            )
            .env(
                "AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH",
                self.stubs.join("amplihack-hooks"),
            )
            .env("RECIPE_RUNNER_RS_PATH", self.stubs.join("recipe-runner-rs"))
            .env("AMPLIHACK_SKIP_RECIPE_RUNNER_INSTALL", "1")
            .env("AMPLIHACK_SKIP_MMDC", "1")
            .env("AMPLIHACK_NO_UPDATE_CHECK", "1")
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_AGENT_BINARY", "claude")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    fn install(&self, args: &[&str]) -> Output {
        let mut all = vec!["install"];
        all.extend_from_slice(args);
        self.amplihack(&all).output().unwrap()
    }

    /// Starts a bash script from `work/` with this host's HOME and a minimal
    /// environment. Its stdout and stderr go to `work/<name>.out|err`.
    fn spawn_script(&self, name: &str, body: &str) -> Child {
        let script = self.work.join(format!("{name}.sh"));
        fs::write(&script, body).unwrap();
        let out = fs::File::create(self.work.join(format!("{name}.out"))).unwrap();
        let err = fs::File::create(self.work.join(format!("{name}.err"))).unwrap();
        Command::new("/bin/bash")
            .arg(&script)
            .arg(&self.work)
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", format!("{}:/usr/bin:/bin", self.stubs.display()))
            .env("LC_ALL", "C")
            .current_dir(&self.work)
            .stdin(Stdio::null())
            .stdout(out)
            .stderr(err)
            .spawn()
            .unwrap()
    }

    fn work_file(&self, name: &str) -> String {
        fs::read_to_string(self.work.join(name)).unwrap_or_default()
    }
}

fn text(output: &Output) -> String {
    format!(
        "status: {:?}\n--- stdout\n{}\n--- stderr\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// Waits for `condition` with a deadline, never a fixed sleep of the length
/// being waited for.
fn wait_until(deadline: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + deadline;
    while Instant::now() < end {
        if condition() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    condition()
}

/// Waits for `child` to exit, up to `deadline`. A child still running at the
/// deadline is reported, then reaped by `Drop` of the temp dir's processes.
fn wait_child(child: &mut Child, deadline: Duration) -> Option<std::process::ExitStatus> {
    let end = Instant::now() + deadline;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return Some(status);
        }
        if Instant::now() >= end {
            return None;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// The run (A). See the module docs.
const RUN_SCRIPT: &str = r#"set -euo pipefail
work="$1"
trap 'echo "run failed at line $LINENO (pass ${i:-0}): $BASH_COMMAND; cwd $(readlink /proc/$$/cwd)" >&2' ERR
bundle="$HOME/.amplihack/amplifier-bundle"
cd "$bundle"
pwd -P > "$work/a.cwd"
exec 3< ./recipes/smart-orchestrator.yaml
echo $$ > "$work/a.pid"
i=0
while :; do
  i=$((i + 1))
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
  echo "$i" > "$work/a.iter"
  if [ -e "$work/stop" ] && [ "$i" -ge 5 ]; then
    break
  fi
  sleep 0.05
done
cat <&3 > /dev/null
echo "DONE $i" > "$work/a.done"
"#;

/// The signal sentinel (S).
const SENTINEL_SCRIPT: &str = r#"work="$1"
trap 'echo TERM >> "$work/sig.log"' TERM
trap 'echo HUP >> "$work/sig.log"' HUP
trap 'echo INT >> "$work/sig.log"' INT
cd "$HOME/.amplihack/amplifier-bundle"
echo $$ > "$work/s.pid"
while [ ! -e "$work/stop" ]; do
  sleep 0.05
done
echo OK > "$work/s.done"
"#;

/// The link sampler (L). Writes a line to `l.bad` whenever a published path
/// is missing and each distinct `readlink` result to `l.targets`.
const SAMPLER_SCRIPT: &str = r#"work="$1"
b="$HOME/.amplihack/amplifier-bundle"
a="$HOME/.local/bin/amplihack"
h="$HOME/.local/bin/amplihack-hooks"
last=
n=0
while [ ! -e "$work/stop" ]; do
  for p in "$b" "$a" "$h"; do
    [ -e "$p" ] || echo "missing $p" >> "$work/l.bad"
  done
  [ -f "$b/recipes/smart-orchestrator.yaml" ] || echo "no recipe under $b" >> "$work/l.bad"
  t="$(readlink "$b" || echo REAL-DIR)|$(readlink "$a" || echo REAL-FILE)|$(readlink "$h" || echo REAL-FILE)"
  if [ "$t" != "$last" ]; then
    echo "$t" >> "$work/l.targets"
    last="$t"
  fi
  n=$((n + 1))
  sleep 0.01
done
echo "$n" > "$work/l.count"
"#;

/// Whether `target` names `<releases>/<version…>/<rel>`, by component.
fn names_release(releases: &Path, target: &Path, rel: &str) -> Result<PathBuf, String> {
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
fn published_release(host: &Host, link: &Path, rel: &str) -> Result<PathBuf, String> {
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
fn proc_link(pid: &str, entry: &str) -> String {
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

#[cfg(target_os = "linux")]
#[test]
fn install_and_self_heal_leave_a_live_run_alone() {
    let host = Host::new();
    let mut failures: Vec<String> = Vec::new();

    let first = host.install(&[]);
    assert!(first.status.success(), "first install: {}", text(&first));
    assert!(host.bundle().join(PROBE_REL).is_file(), "{}", text(&first));

    let mut run = host.spawn_script("a", RUN_SCRIPT);
    let mut sentinel = host.spawn_script("s", SENTINEL_SCRIPT);
    let mut sampler = host.spawn_script("l", SAMPLER_SCRIPT);

    let started = wait_until(Duration::from_secs(60), || {
        host.work_file("a.iter").trim().parse::<u32>().unwrap_or(0) >= 3
            && !host.work_file("s.pid").is_empty()
            && !host.work_file("l.targets").is_empty()
    });
    assert!(
        started,
        "the run, sentinel and sampler must start before any install\n\
         a.err: {}\ns.err: {}\nl.err: {}",
        host.work_file("a.err"),
        host.work_file("s.err"),
        host.work_file("l.err")
    );
    let a_pid = host.work_file("a.pid").trim().to_string();
    let a_cwd = host.work_file("a.cwd").trim().to_string();

    // Install 1: a same-version re-stage while the run is alive.
    let second = host.install(&["--force-refresh"]);
    if !second.status.success() {
        failures.push(format!(
            "install --force-refresh #1 failed: {}",
            text(&second)
        ));
    }

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

    // AC4 while the run is still alive: nothing it holds was deleted.
    let cwd_now = proc_link(&a_pid, "cwd");
    let fd_now = proc_link(&a_pid, "fd/3");
    if !cwd_now.starts_with('<') {
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
    }

    fs::write(host.work.join("stop"), "").unwrap();
    let run_status = wait_child(&mut run, Duration::from_secs(60));
    let sentinel_status = wait_child(&mut sentinel, Duration::from_secs(30));
    let sampler_status = wait_child(&mut sampler, Duration::from_secs(30));
    for (name, child, status) in [
        ("run", &mut run, run_status),
        ("sentinel", &mut sentinel, sentinel_status),
        ("sampler", &mut sampler, sampler_status),
    ] {
        if status.is_none() {
            failures.push(format!("{name} did not exit within its deadline"));
            // Our own child, started by this test: the only signal allowed.
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    // AC1: the run finished normally.
    let a_done = host.work_file("a.done");
    if !(run_status.is_some_and(|s| s.success()) && a_done.starts_with("DONE ")) {
        failures.push(format!(
            "AC1: the run did not finish normally (status {:?}, last pass {}, done {:?})\n\
             a.err:\n{}",
            run_status,
            host.work_file("a.iter").trim(),
            a_done.trim(),
            host.work_file("a.err")
        ));
    }

    // AC2: no signal reached the sentinel.
    let signals = host.work_file("sig.log");
    if !signals.is_empty() {
        failures.push(format!("AC2: the sentinel received signals: {signals}"));
    }
    if !(sentinel_status.is_some_and(|s| s.success()) && host.work_file("s.done").contains("OK")) {
        failures.push(format!(
            "AC2: the sentinel did not finish normally (status {sentinel_status:?})"
        ));
    }

    // AC3: never missing, and each link ends on a complete release.
    let bad = host.work_file("l.bad");
    if !bad.is_empty() {
        let first_lines: Vec<&str> = bad.lines().take(10).collect();
        failures.push(format!(
            "AC3: a published path went missing ({} samples):\n{}",
            bad.lines().count(),
            first_lines.join("\n")
        ));
    }
    let targets = host.work_file("l.targets");
    for line in targets.lines() {
        if line.contains("REAL-") {
            failures.push(format!(
                "AC3: a published path was a real file or directory, not a link: {line}"
            ));
            break;
        }
    }
    let local_bin = host.home.join(".local/bin");
    for (link, rel) in [
        (host.bundle(), "amplifier-bundle".to_string()),
        (local_bin.join("amplihack"), "bin/amplihack".to_string()),
        (
            local_bin.join("amplihack-hooks"),
            "bin/amplihack-hooks".to_string(),
        ),
    ] {
        if let Err(e) = published_release(&host, &link, &rel) {
            failures.push(format!("AC3: {e}"));
        }
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
        "issue #1954: {} acceptance failure(s):\n\n{}\n\nlink targets seen:\n{}",
        failures.len(),
        failures.join("\n\n"),
        targets
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

/// A legacy layout (real bundle directory, regular-file binaries) migrates to
/// links in one install, and a run whose cwd is in the legacy tree survives.
#[cfg(target_os = "linux")]
#[test]
fn migration_from_a_real_bundle_dir_keeps_a_live_run_alive() {
    let host = Host::new();
    // Legacy: a real bundle directory and a regular-file binary.
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

    let mut run = host.spawn_script("a", RUN_SCRIPT);
    assert!(
        wait_until(Duration::from_secs(60), || {
            host.work_file("a.iter").trim().parse::<u32>().unwrap_or(0) >= 2
        }),
        "a.err: {}",
        host.work_file("a.err")
    );
    let a_pid = host.work_file("a.pid").trim().to_string();

    let install = host.install(&[]);
    let cwd_now = proc_link(&a_pid, "cwd");
    fs::write(host.work.join("stop"), "").unwrap();
    let status = wait_child(&mut run, Duration::from_secs(60));
    if status.is_none() {
        let _ = run.kill();
        let _ = run.wait();
    }

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
    if !status.is_some_and(|s| s.success()) {
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
