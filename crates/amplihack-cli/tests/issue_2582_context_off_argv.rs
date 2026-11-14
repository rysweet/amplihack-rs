//! Issue #2582: no recipe context value appears in the argv of any process
//! that `amplihack recipe run` starts, when the runner reports
//! `context_file_v1`.
//!
//! Runs the real `amplihack` binary against a `#!/bin/sh` runner stub
//! (through `RECIPE_RUNNER_RS_PATH`). The stub answers `--capabilities` at
//! once, records its argv, copies the file named by `--context-file`, then
//! blocks reading a FIFO this test created. While it is blocked the whole
//! tree is live, and the test walks it from the `amplihack` child it started
//! (`support::proc_tree`, read-only) and checks every argv element.
//!
//! Synchronisation is by event only, with no sleep, deadline or poll:
//! - the test opens the FIFO for writing, which returns once the stub has
//!   opened it for reading;
//! - a watcher thread waits on the `amplihack` child; if the child exits
//!   first, it opens the read end itself, so the test's open returns and the
//!   test fails with the child's output instead of hanging;
//! - [`Live`]'s `Drop` writes the release byte and joins the watcher, which
//!   has reaped the child, on every way out, panics included.
//!
//! The root gets its context through `recipe run --context-file <file>`, so
//! no value is on the test's own command line and the root is part of the
//! walk. Every value is a synthetic `ZQXV2582*` token.
//!
//! The pinned runner does not report `context_file_v1`; the fallback case
//! checks that today's `--set` path is unchanged, and that the walk does see
//! the values there (the negative control for the walker).

#![cfg(target_os = "linux")]

use crate::isolated_amplihack::HOME_GUARD_ENV;
use crate::support::amplihack_bin::amplihack_bin;
use crate::support::proc_tree;
use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

/// Distinctive context values, each at least 12 characters with a unique
/// token.
const VALUES: &[(&str, &str)] = &[
    ("task_description", "ZQXV2582TASKWORD fix the widget"),
    (
        "pr_body",
        "ZQXV2582BODYWORD {\"json\": [1, 2]} $(touch pwned)",
    ),
    ("note", "ZQXV2582NOTEWORD second line\nthird line"),
];

const TOKENS: &[&str] = &["ZQXV2582TASKWORD", "ZQXV2582BODYWORD", "ZQXV2582NOTEWORD"];

const CAPS_WITH: &str = r#"["codex_exec","durable_recovery_v1","context_file_v1"]"#;
const CAPS_WITHOUT: &str = r#"["codex_exec","durable_recovery_v1"]"#;

const RECIPE: &str = "name: issue-2582\ndescription: context off argv\nversion: 1.0.0\n\
context: {}\nsteps:\n  - id: noop\n    type: bash\n    command: \"true\"\n";

/// The runner stub. `{caps}` is the capability array it reports.
fn runner_stub(caps: &str) -> String {
    format!(
        r#"#!/bin/sh
case "$1" in --capabilities) printf '%s\n' '{{"schema_version":1,"version":"fixture","capabilities":{caps}}}'; exit 0 ;; esac
: > "$RECORD_DIR/argv.bin"
for a in "$@"; do printf '%s\0' "$a" >> "$RECORD_DIR/argv.bin"; done
prev=
for a in "$@"; do
  if [ "$prev" = "--context-file" ]; then
    cp "$a" "$RECORD_DIR/context.copy.json"
    printf '%s' "$a" > "$RECORD_DIR/context.path"
    stat -c %a "$a" > "$RECORD_DIR/file.mode"
    stat -c %a "$(dirname "$a")" > "$RECORD_DIR/dir.mode"
  fi
  prev=$a
done
read _ < "$FIFO"
printf '%s\n' '{{"recipe_name":"issue-2582","success":true,"step_results":[],"context":{{}}}}'
"#
    )
}

/// A deterministic tmux: `new-session ... -- CMD...` runs CMD in the
/// foreground, the pattern of `issue_1741_recipe_recovery`. No tmux server
/// is started.
const TMUX_STUB: &str = r#"#!/bin/sh
case "$1" in
  -V) exit 0 ;;
  has-session) exit 1 ;;
  new-session)
    shift
    while [ "$1" != -- ]; do shift; done
    shift
    "$@"
    exit 0 ;;
  kill-session) exit 0 ;;
esac
exit 1
"#;

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    records: PathBuf,
    tools: PathBuf,
    work_root: PathBuf,
    recipe: PathBuf,
    context_file: PathBuf,
    fifo: PathBuf,
}

impl Fixture {
    fn new(caps: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let home = root.join("home");
        let records = root.join("records");
        let tools = root.join("tools");
        let work_root = root.join("work");
        for d in [&home, &records, &tools, &work_root, &home.join("profiles")] {
            fs::create_dir_all(d).unwrap();
            fs::set_permissions(d, fs::Permissions::from_mode(0o700)).unwrap();
        }
        crate::support::exec_stub::write_executable(
            &tools.join("recipe-runner-rs"),
            runner_stub(caps),
            0o700,
        );
        crate::support::exec_stub::write_executable(&tools.join("tmux"), TMUX_STUB, 0o700);
        let recipe = root.join("recipe.yaml");
        fs::write(&recipe, RECIPE).unwrap();

        let object: serde_json::Map<String, serde_json::Value> = VALUES
            .iter()
            .map(|(k, v)| (k.to_string(), serde_json::Value::String(v.to_string())))
            .collect();
        let context_file = root.join("ctx.json");
        fs::write(&context_file, serde_json::to_vec(&object).unwrap()).unwrap();
        fs::set_permissions(&context_file, fs::Permissions::from_mode(0o600)).unwrap();

        let fifo = records.join("release.fifo");
        let c = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: `c` is a valid NUL-terminated path.
        let rc = unsafe { libc::mkfifo(c.as_ptr(), 0o600) };
        assert_eq!(rc, 0, "mkfifo: {}", std::io::Error::last_os_error());

        Self {
            _tmp: tmp,
            root,
            home,
            records,
            tools,
            work_root,
            recipe,
            context_file,
            fifo,
        }
    }

    /// `amplihack recipe run <recipe> --context-file <ctx> -w <root> <extra>`
    /// in an isolated environment.
    fn command(&self, extra: &[&str]) -> Command {
        let mut c = crate::isolated_amplihack::isolated(amplihack_bin());
        c.args(["recipe", "run"])
            .arg(&self.recipe)
            .arg("--context-file")
            .arg(&self.context_file)
            .arg("-w")
            .arg(&self.root)
            .args(extra)
            .current_dir(&self.root)
            .env(HOME_GUARD_ENV, "1")
            .env(
                "PATH",
                format!("{}:/usr/local/bin:/usr/bin:/bin", self.tools.display()),
            )
            .env("HOME", &self.home)
            .env("AMPLIHACK_HOME", self.home.join(".amplihack"))
            .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", self.home.join("profiles"))
            .env("AMPLIHACK_WORK_ROOT", &self.work_root)
            .env("RECIPE_RUNNER_RS_PATH", self.tools.join("recipe-runner-rs"))
            .env("RECIPE_RUNNER_NO_UPDATE_CHECK", "1")
            .env("RECORD_DIR", &self.records)
            .env("FIFO", &self.fifo)
            .env("AMPLIHACK_AGENT_BINARY", "claude")
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
        c
    }

    /// Start the run and return once the stub is blocked on the FIFO, with
    /// the tree live.
    fn start(&self, extra: &[&str]) -> Live {
        let child = self.command(extra).spawn().expect("start amplihack");
        let pid = child.id();
        let exited = Arc::new(AtomicBool::new(false));
        let watcher = {
            let exited = Arc::clone(&exited);
            let fifo = self.fifo.clone();
            std::thread::spawn(move || {
                let output = child.wait_with_output().expect("wait for amplihack");
                exited.store(true, Ordering::SeqCst);
                // Release a test blocked in its open for writing. Non-blocking,
                // so this never waits for a writer; held until joined.
                let reader = OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NONBLOCK)
                    .open(&fifo)
                    .ok();
                (output, reader)
            })
        };
        let mut live = Live {
            pid,
            writer: None,
            watcher: Some(watcher),
            output: None,
        };
        let writer = OpenOptions::new()
            .write(true)
            .open(&self.fifo)
            .expect("open the release FIFO");
        live.writer = Some(writer);
        if exited.load(Ordering::SeqCst) {
            let output = live.finish();
            panic!(
                "amplihack exited before the runner stub blocked:\n{}",
                describe(&output)
            );
        }
        live
    }

    fn runner_argv(&self) -> Vec<String> {
        let raw = fs::read(self.records.join("argv.bin")).expect("runner stub not started");
        raw.split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect()
    }

    fn recorded(&self, name: &str) -> Option<String> {
        fs::read_to_string(self.records.join(name)).ok()
    }
}

/// A run whose runner stub is blocked on the FIFO. Dropping it releases the
/// stub and reaps the `amplihack` child.
struct Live {
    pid: u32,
    writer: Option<File>,
    watcher: Option<JoinHandle<(Output, Option<File>)>>,
    output: Option<Output>,
}

impl Live {
    /// Release the stub, wait for `amplihack` to exit and return its output.
    fn finish(&mut self) -> Output {
        if let Some(mut writer) = self.writer.take() {
            let _ = writer.write_all(b"\n");
        }
        if let Some(watcher) = self.watcher.take() {
            let (output, _reader) = watcher.join().expect("watcher thread");
            self.output = Some(output);
        }
        self.output.clone().expect("amplihack output")
    }

    fn tree(&self) -> Vec<proc_tree::Process> {
        proc_tree::descendants(self.pid)
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        if self.watcher.is_some() {
            let _ = self.finish();
        }
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

fn show(tree: &[proc_tree::Process]) -> String {
    tree.iter()
        .map(|(pid, argv)| {
            let argv: Vec<String> = argv
                .iter()
                .map(|a| String::from_utf8_lossy(a).into_owned())
                .collect();
            format!("{pid}: {argv:?}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The tree must include `amplihack` and the runner stub; a walk that finds
/// only the root (no `CONFIG_PROC_CHILDREN`) fails here instead of passing.
fn assert_tree_reaches_the_runner(tree: &[proc_tree::Process], runner: &Path) {
    assert!(
        tree.len() >= 2,
        "the walk found too few processes:\n{}",
        show(tree)
    );
    let runner = runner.as_os_str().as_bytes();
    assert!(
        tree.iter()
            .any(|(_, argv)| argv.iter().any(|a| a.as_slice() == runner)),
        "the runner stub is not in the walked tree:\n{}",
        show(tree)
    );
}

fn assert_copy_holds_every_value(f: &Fixture) {
    let copy: serde_json::Map<String, serde_json::Value> = serde_json::from_slice(
        &fs::read(f.records.join("context.copy.json")).expect("no --context-file copy"),
    )
    .unwrap();
    for (key, value) in VALUES {
        assert_eq!(
            copy.get(*key).and_then(serde_json::Value::as_str),
            Some(*value),
            "{key} did not reach the runner byte-exact"
        );
    }
}

#[test]
fn capability_present_no_value_on_any_argv() {
    let f = Fixture::new(CAPS_WITH);
    let mut live = f.start(&["--no-detach"]);

    let tree = live.tree();
    assert_tree_reaches_the_runner(&tree, &f.tools.join("recipe-runner-rs"));
    let hits = proc_tree::matches(&tree, TOKENS);
    assert!(
        hits.is_empty(),
        "context values on argv: {hits:?}\ntree:\n{}",
        show(&tree)
    );

    let argv = f.runner_argv();
    assert!(!argv.iter().any(|a| a == "--set"), "{argv:?}");
    let flag = argv
        .iter()
        .position(|a| a == "--context-file")
        .expect("the runner must get --context-file");
    let path = PathBuf::from(&argv[flag + 1]);
    assert_eq!(f.recorded("context.path").as_deref(), path.to_str());
    assert!(path.ends_with("context/context.json"), "{}", path.display());
    assert!(
        path.is_file(),
        "the context file exists while the runner runs"
    );
    assert_eq!(f.recorded("file.mode").unwrap().trim(), "600");
    assert_eq!(f.recorded("dir.mode").unwrap().trim(), "700");
    assert_copy_holds_every_value(&f);

    let output = live.finish();
    assert!(output.status.success(), "{}", describe(&output));
    assert!(!path.exists(), "context.json must be removed after the run");
    assert!(
        !path.parent().unwrap().exists(),
        "the context folder must be removed after the run"
    );
    for out in [&output.stdout, &output.stderr] {
        let text = String::from_utf8_lossy(out);
        assert!(
            !text.contains("ZQXV2582BODYWORD"),
            "the run printed a context value: {text}"
        );
    }
}

#[test]
fn fallback_keeps_set() {
    let f = Fixture::new(CAPS_WITHOUT);
    let mut live = f.start(&["--no-detach"]);

    let tree = live.tree();
    assert_tree_reaches_the_runner(&tree, &f.tools.join("recipe-runner-rs"));
    // Negative control: on the fallback the values are on the runner's argv,
    // and the walk sees them.
    let hits = proc_tree::matches(&tree, TOKENS);
    assert!(
        hits.iter().any(|(_, a)| a.contains("ZQXV2582TASKWORD")),
        "the walk must see --set values on the fallback:\n{}",
        show(&tree)
    );

    let argv = f.runner_argv();
    assert!(!argv.iter().any(|a| a == "--context-file"), "{argv:?}");
    assert!(f.recorded("context.copy.json").is_none());
    let sets: Vec<&str> = argv
        .windows(2)
        .filter(|w| w[0] == "--set")
        .map(|w| w[1].as_str())
        .collect();
    for (key, value) in VALUES {
        let pair = format!("{key}={value}");
        assert!(
            sets.contains(&pair.as_str()),
            "missing --set {key}: {sets:?}"
        );
    }

    let output = live.finish();
    assert!(output.status.success(), "{}", describe(&output));
}

/// D9: `recipe run --detach` through a foreground tmux stub. The tree is
/// `recipe run --detach` -> tmux stub -> `recipe detached-child` -> inner
/// `recipe run` -> runner stub, so one walk covers the detached child and
/// the nested run.
#[test]
fn detached_child_and_nested_run() {
    let f = Fixture::new(CAPS_WITH);
    let mut live = f.start(&["--detach"]);

    let tree = live.tree();
    assert_tree_reaches_the_runner(&tree, &f.tools.join("recipe-runner-rs"));
    let detached = tree
        .iter()
        .any(|(_, argv)| argv.iter().any(|a| a.as_slice() == b"detached-child"));
    assert!(detached, "no detached-child in the tree:\n{}", show(&tree));
    let runs = tree
        .iter()
        .filter(|(_, argv)| {
            argv.windows(2)
                .any(|w| w[0].as_slice() == b"recipe" && w[1].as_slice() == b"run")
        })
        .count();
    assert!(
        runs >= 2,
        "expected the launcher and the inner run:\n{}",
        show(&tree)
    );
    let hits = proc_tree::matches(&tree, TOKENS);
    assert!(
        hits.is_empty(),
        "context values on argv: {hits:?}\ntree:\n{}",
        show(&tree)
    );

    let argv = f.runner_argv();
    assert!(!argv.iter().any(|a| a == "--set"), "{argv:?}");
    let flag = argv
        .iter()
        .position(|a| a == "--context-file")
        .expect("the inner run's runner must get --context-file");
    let path = PathBuf::from(&argv[flag + 1]);
    assert_copy_holds_every_value(&f);

    let output = live.finish();
    assert!(output.status.success(), "{}", describe(&output));
    assert!(!path.exists(), "context.json must be removed after the run");
    assert!(!path.parent().unwrap().exists());
    let logs = f.home.join(".amplihack").join("recipe-logs");
    let leftovers: Vec<_> = fs::read_dir(&logs)
        .map(|d| {
            d.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.ends_with(".context.json") || n.ends_with(".spec.json"))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        leftovers.is_empty(),
        "the pane's private files must be removed: {leftovers:?}"
    );
}
