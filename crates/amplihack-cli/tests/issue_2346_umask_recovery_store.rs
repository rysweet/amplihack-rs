//! Issue #2346: a recipe run under umask 0002 in a fresh `HOME`.
//!
//! Since #2334 a run opens its recovery store through the pinned runner's
//! `private_fs::Directory::open`, which stops any work root with a group- or
//! world-writable folder above it that has no sticky bit, and requires the
//! work root itself to be owned and `0700`. Under umask 0002, Ubuntu's
//! default for a user with a private group, amplihack created
//! `~/.amplihack` at `0775` itself, so the first run in a fresh `HOME`
//! ended at once with a bare `Error: unsafe recovery ancestor` that named
//! no folder.
//!
//! Each case starts the built `amplihack` with the pinned `recipe-runner-rs`
//! and umask 0002 set in the child only (`pre_exec`), so the result is the
//! same under CI's umask 022 and this test process keeps its own umask.
//! `HOME` and the work root are temporary folders; nothing reads or writes
//! the real `~/.amplihack` or `~/.claude`. The recipe has one bash step and
//! no agent step, so no harness is ever started.
//!
//! - T1: a `0750` `HOME` (Ubuntu's default home mode) and the default work
//!   root. The run succeeds, `~/.amplihack` is `0700`, and every folder under
//!   the work root is `0700` and every file `0600`.
//! - T2: a `0775` folder above the work root, made by hand. The run stops,
//!   the message names the folder, its mode and `chmod g-w`, and the work
//!   root is not created.
//! - T3: an owned work root at `0755`. The run sets it to `0700`, says so
//!   on stderr, and succeeds.
//! - T4: the runner's own admission check fails on a `0755`
//!   `<work root>/recovery-index`. The message names the work root and the
//!   run folder and ends with the runner's reason.
//!
//! The auto-drive review allocation under umask 0002 is covered by
//! `issue_2096_review_isolation::review_allocate_under_umask_0002_makes_folders_the_trust_scan_keeps`
//! (#2778, pull request #2781) and is not repeated here.

#![cfg(unix)]

use crate::isolated_amplihack::HOME_GUARD_ENV;
use crate::support::amplihack_bin::amplihack_bin;
use crate::support::private_tempdir::{private_dir_all, private_tempdir_in};
use crate::support::scratch_base::scratch_base;
use std::fs::{self, DirBuilder};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// What the step writes to its marker file, so a passing run is known to
/// have reached its step.
const MARKER: &str = "QA_2346_STEP_RAN";

/// The repository root, two levels above this crate's manifest.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the repo root is two levels above the crate manifest")
        .to_path_buf()
}

/// The permission bits, special bits included, without following a link, as
/// octal text, so a failure reads `0775` rather than `509`.
fn mode_of(path: &Path) -> String {
    let meta =
        fs::symlink_metadata(path).unwrap_or_else(|e| panic!("lstat {}: {e}", path.display()));
    octal(meta.mode())
}

fn octal(mode: u32) -> String {
    format!("{:04o}", mode & 0o7777)
}

/// A folder with exactly `mode`, whatever the umask.
fn dir_with_mode(path: &Path, mode: u32) {
    DirBuilder::new()
        .mode(0o700)
        .create(path)
        .unwrap_or_else(|e| panic!("mkdir {}: {e}", path.display()));
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    assert_eq!(
        mode_of(path),
        octal(mode),
        "fixture mode of {}",
        path.display()
    );
}

/// The documented quoting rule for a path in a suggested command.
fn word(path: &Path) -> String {
    let text = path.to_str().expect("UTF-8 fixture path");
    if text
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"/._+,:@%=-".contains(&b))
    {
        text.to_string()
    } else {
        format!("'{}'", text.replace('\'', r"'\''"))
    }
}

/// Fails, naming the folder, when a folder above `root` would make the
/// runner stop every case for a reason outside this test: one that is
/// group- or world-writable without the sticky bit, or a symbolic link.
fn assert_usable_base(root: &Path) {
    for ancestor in root.ancestors() {
        let meta = fs::symlink_metadata(ancestor)
            .unwrap_or_else(|e| panic!("lstat {}: {e}", ancestor.display()));
        let open = meta.mode() & 0o022 != 0 && meta.mode() & 0o1000 == 0;
        assert!(
            !meta.file_type().is_symlink() && !open,
            "{} (mode {:04o}) above the test's temporary folder is one the recovery store \
             stops; run the tests with TMPDIR under folders only you can write",
            ancestor.display(),
            meta.mode() & 0o7777
        );
    }
}

/// Umask 0002 in the child only.
fn umask_0002(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: the closure runs in the forked child before exec. It calls only
    // umask(2), which is async-signal-safe, cannot fail, takes no lock and
    // allocates nothing; it changes the child's umask and nothing else, so
    // this test process keeps its own.
    unsafe {
        command.pre_exec(|| {
            libc::umask(0o002);
            Ok(())
        });
    }
}

fn text(out: &Output) -> String {
    format!(
        "status: {}\n--- stdout\n{}\n--- stderr\n{}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

struct Fixture {
    _tmp: tempfile::TempDir,
    /// The scratch base the root lives in, given to the child as `TMPDIR` so
    /// that `HOME` is under the child's temporary folder, as the home guard
    /// requires.
    base: PathBuf,
    root: PathBuf,
    home: PathBuf,
    project: PathBuf,
    marker: PathBuf,
    recipe: PathBuf,
    runner: PathBuf,
}

impl Fixture {
    /// A `0700` root holding `home` at `home_mode`, a project folder and the
    /// one-step recipe. `context` is the recipe's `context:` block.
    fn new(home_mode: u32, context: &str) -> Self {
        let runner = crate::pinned_runner::pinned_runner(&repo_root());
        let base = scratch_base();
        let tmp = private_tempdir_in(&base);
        let root = tmp.path().canonicalize().unwrap();
        assert_eq!(mode_of(&root), octal(0o700), "the fixture root is private");
        assert_usable_base(&root);
        let home = root.join("home");
        dir_with_mode(&home, home_mode);
        let project = root.join("project");
        let records = root.join("records");
        private_dir_all(&project);
        private_dir_all(&records);
        let marker = records.join("marker");
        let recipe = root.join("issue-2346.yaml");
        fs::write(
            &recipe,
            format!(
                "name: issue-2346-umask\ndescription: one bash step\nversion: 1.0.0\n\
                 context: {context}\nsteps:\n  - id: step\n    type: bash\n    command: |\n      \
                 printf '%s' {MARKER} > {marker}\n",
                marker = word(&marker)
            ),
        )
        .unwrap();
        Self {
            _tmp: tmp,
            base,
            root,
            home,
            project,
            marker,
            recipe,
            runner,
        }
    }

    /// `amplihack recipe run` in the foreground, under umask 0002, with the
    /// default work root unless the caller sets `AMPLIHACK_WORK_ROOT`.
    fn command(&self) -> Command {
        let path = format!(
            "{}:/usr/local/bin:/usr/bin:/bin",
            self.runner
                .parent()
                .expect("the runner has a folder")
                .display()
        );
        let mut c = crate::isolated_amplihack::isolated(amplihack_bin());
        c.args(["recipe", "run"])
            .arg(&self.recipe)
            .arg("-w")
            .arg(&self.project)
            .arg("--no-detach")
            .current_dir(&self.project)
            .env(HOME_GUARD_ENV, "1")
            .env("TMPDIR", &self.base)
            .env("PATH", &path)
            .env("HOME", &self.home)
            .env("AMPLIHACK_HOME", self.home.join(".amplihack"))
            .env("RECIPE_RUNNER_RS_PATH", &self.runner)
            .env("AMPLIHACK_AGENT_BINARY", "claude")
            .env_remove("AMPLIHACK_WORK_ROOT")
            .env_remove("AMPLIHACK_WORK_ROOT_EXPLICIT")
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
            .env("AMPLIHACK_SKIP_REFLECTION", "1")
            .env("AMPLIHACK_CLEANUP_ON_EXIT", "0")
            .env("AMPLIHACK_GOVERNOR", "off")
            .env("AMPLIHACK_USAGE_LIMIT_WAIT", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        umask_0002(&mut c);
        c
    }

    fn run(&self, command: &mut Command) -> Output {
        command.output().expect("run amplihack recipe run")
    }

    fn step_ran(&self) -> bool {
        fs::read_to_string(&self.marker).is_ok_and(|m| m == MARKER)
    }
}

/// Every entry at or below `dir` whose mode is not the private one: folders
/// `0700`, regular files `0600`. Anything else (a link, a socket) is listed
/// too. Paths are relative to `dir`, with the octal mode.
///
/// No exclusions are needed for this run: a one-step bash recipe in a folder
/// that is not a git repository makes no execution checkout, and the files
/// the pinned runner writes under the work root are all `0600`. A file the
/// runner writes with the process umask would be listed here; exclude it by
/// its relative path, with a comment naming the runner as its writer.
fn not_private(dir: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(path) = stack.pop() {
        let meta =
            fs::symlink_metadata(&path).unwrap_or_else(|e| panic!("lstat {}: {e}", path.display()));
        let mode = meta.mode() & 0o7777;
        let shown = path.strip_prefix(dir).unwrap().display().to_string();
        let shown = if shown.is_empty() { ".".into() } else { shown };
        let kind = meta.file_type();
        if kind.is_dir() {
            if mode != 0o700 {
                found.push(format!("{shown}/ {mode:04o}"));
            }
            for entry in fs::read_dir(&path).unwrap() {
                stack.push(entry.unwrap().path());
            }
        } else if kind.is_file() {
            if mode != 0o600 {
                found.push(format!("{shown} {mode:04o}"));
            }
        } else {
            found.push(format!("{shown} is neither a folder nor a regular file"));
        }
    }
    found.sort();
    found
}

// ---------------------------------------------------------------------- T1

#[test]
fn recipe_run_under_umask_0002_with_a_fresh_home_succeeds() {
    let f = Fixture::new(0o750, "{}");
    let out = f.run(&mut f.command());
    assert!(
        out.status.success(),
        "the run must succeed:\n{}",
        text(&out)
    );
    assert!(f.step_ran(), "the step never ran:\n{}", text(&out));

    let amplihack = f.home.join(".amplihack");
    assert_eq!(
        mode_of(&amplihack),
        octal(0o700),
        "~/.amplihack must be created 0700 under any umask:\n{}",
        text(&out)
    );
    assert_eq!(
        mode_of(&f.home),
        octal(0o750),
        "HOME is never changed: amplihack did not create it"
    );
    let work = amplihack.join("work");
    let open = not_private(&work);
    assert!(
        open.is_empty(),
        "under umask 0002 every folder under the work root must be 0700 and every file \
         0600; these are not:\n  {}",
        open.join("\n  ")
    );
}

// ---------------------------------------------------------------------- T2

#[test]
fn a_group_writable_folder_above_the_work_root_is_named() {
    let f = Fixture::new(0o700, "{}");
    let shared = f.root.join("shared");
    dir_with_mode(&shared, 0o775);
    let work = shared.join("work");
    let out = f.run(f.command().env("AMPLIHACK_WORK_ROOT", &work));
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        !out.status.success(),
        "a 0775 folder above the work root must stop the run:\n{}",
        text(&out)
    );
    assert!(!f.step_ran(), "no step may run:\n{}", text(&out));
    for part in [
        shared.display().to_string(),
        "group-writable".to_string(),
        "0775".to_string(),
        format!("chmod g-w {}", word(&shared)),
    ] {
        assert!(
            stderr.contains(&part),
            "stderr must contain {part:?}:\n{}",
            text(&out)
        );
    }
    assert!(
        !stderr
            .lines()
            .any(|line| line.trim() == "Error: unsafe recovery ancestor"),
        "the bare runner message names no folder:\n{}",
        text(&out)
    );
    assert!(
        !work.exists(),
        "nothing is created below the folder at fault"
    );
    assert_eq!(
        mode_of(&shared),
        octal(0o775),
        "a folder above is never changed"
    );
}

// ---------------------------------------------------------------------- T3

#[test]
fn an_owned_0755_work_root_is_set_to_0700() {
    let f = Fixture::new(0o700, "{}");
    let work = f.root.join("work");
    dir_with_mode(&work, 0o755);
    let out = f.run(f.command().env("AMPLIHACK_WORK_ROOT", &work));
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        out.status.success(),
        "the run must succeed:\n{}",
        text(&out)
    );
    assert!(f.step_ran(), "the step never ran:\n{}", text(&out));
    assert_eq!(mode_of(&work), octal(0o700), "the work root ends 0700");
    let notice = format!(
        "amplihack: work root {} had mode 0755; set it to 0700, the mode the recovery store \
         requires",
        work.display()
    );
    assert!(
        stderr.lines().any(|line| line == notice),
        "stderr must hold the notice {notice:?}:\n{}",
        text(&out)
    );
}

// ---------------------------------------------------------------------- T4

#[test]
fn a_failed_admission_names_the_work_root_the_run_folder_and_the_runners_reason() {
    let f = Fixture::new(0o700, "\n  issue_number: \"\"");
    crate::support::git_fixture::init_with_commit(&f.project);
    let work = f.root.join("work");
    dir_with_mode(&work, 0o700);
    let index = work.join("recovery-index");
    dir_with_mode(&index, 0o755);
    let out = f.run(
        f.command()
            .args(["-c", "issue_number=1"])
            .env("AMPLIHACK_WORK_ROOT", &work),
    );
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        !out.status.success(),
        "a 0755 recovery-index must stop admission:\n{}",
        text(&out)
    );
    assert!(!f.step_ran(), "no step may run:\n{}", text(&out));
    let start = format!(
        "Error: cannot durably admit recipe launch (work root {}, run folder {}/runs/",
        work.display(),
        work.display()
    );
    let end = "): recovery directory must be owned and mode 0700";
    assert!(
        stderr
            .lines()
            .any(|line| line.starts_with(&start) && line.ends_with(end)),
        "stderr must hold one line starting {start:?} and ending {end:?}:\n{}",
        text(&out)
    );
    assert_eq!(
        mode_of(&index),
        octal(0o755),
        "the runner's folders are never changed"
    );
}
