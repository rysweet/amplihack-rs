//! Issue #1764: no git that a test, or `amplihack` itself, starts may wait on
//! a credential prompt.
//!
//! When a remote asks for a credential and nothing supplies one, git opens
//! `/dev/tty` and asks. Plain `timeout` moves the command into a background
//! process group, so that read stops git on `SIGTTIN`. Nothing fails: the run
//! waits until something kills it, and with uutils `timeout`, which is stopped
//! too, nothing does. CI runners have no terminal, so there git failed with
//! `No such device or address` instead and the trap went unseen.
//!
//! What is pinned here (see `docs/howto/keep-tests-from-prompting-for-git.md`):
//!
//! - (a) `.cargo/config.toml` holds only an `[env]` table that puts
//!   `GIT_TERMINAL_PROMPT=0`, `GIT_ASKPASS=true` and `SSH_ASKPASS=true` into
//!   the test process, and `isolated(..)` sets them again after its
//!   `env_clear()`;
//! - (b) a git started through `amplihack_git::command()`, with no askpass
//!   program and no terminal, fails at once with `terminal prompts disabled`;
//! - (c) under `script` and plain `timeout`, in a compound command so that the
//!   trap is armed, git with none of the variables stops (the control), while
//!   git with `GIT_TERMINAL_PROMPT=0` alone (c1) or with all three (c2) exits
//!   at once;
//! - (d) the three recipe prompts that run long commands tell the agent how to
//!   run them so that a prompt cannot stop them, and step 12's evidence schema
//!   is unchanged;
//! - the how-to is linked and offers no remedy that weakens TLS, host-key or
//!   credential-storage settings.
//!
//! Every git here talks only to [`AlwaysUnauthorised`], a server on
//! `127.0.0.1` that answers `401`, from a temporary `HOME` with
//! `GIT_CONFIG_GLOBAL=/dev/null` and `GIT_CONFIG_NOSYSTEM=1`, so no stored
//! credential can reach it. Each URL names its server's port and this
//! process's id (`http://127.0.0.1:<port>/issue-1764-<pid>.git`), so a process
//! left behind can be found, and this file looks for one after each run.

use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_yaml::Value;

use crate::auto_drive_to_merge::BRICK_LINE_BUDGET;
use crate::isolated_amplihack::{NO_GIT_PROMPT_ENV, isolated};

/// The inner `timeout` in part (c), in seconds. (c1) and (c2) must finish
/// well inside it.
const INNER_SECS: u64 = 8;

/// How long part (c) lets each `script` run before killing it. Longer than
/// [`INNER_SECS`], so GNU `timeout` reports 124 first; uutils `timeout` is
/// stopped with git and never does.
const PTY_DEADLINE: Duration = Duration::from_secs(20);

/// How long part (b) lets git run before killing it.
const BUILDER_DEADLINE: Duration = Duration::from_secs(30);

/// Proxy settings that would send git's request somewhere other than the
/// loopback server.
const PROXY_VARS: [&str; 6] = [
    "http_proxy",
    "HTTP_PROXY",
    "https_proxy",
    "HTTPS_PROXY",
    "all_proxy",
    "ALL_PROXY",
];

/// Settings that must never be offered as a way round a credential prompt
/// (SR10): they weaken TLS, host-key checking or credential storage.
const WEAKENING: [&str; 6] = [
    "GIT_SSL_NO_VERIFY",
    "sslVerify",
    "StrictHostKeyChecking",
    "UserKnownHostsFile",
    "helper store",
    "helper=store",
];

// ── Helpers ──────────────────────────────────────────────────────────────────

/// A git remote on `127.0.0.1` that refuses every request with `401` and
/// `WWW-Authenticate: Basic`, so git asks for a credential. It reads at most
/// 8 KiB of each request, waits at most 5 s for it, and never prints it, so
/// nothing git sends reaches the test output.
struct AlwaysUnauthorised {
    port: u16,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

const UNAUTHORISED: &[u8] = b"HTTP/1.1 401 Unauthorized\r\n\
WWW-Authenticate: Basic realm=\"issue-1764\"\r\n\
Content-Length: 0\r\n\
Connection: close\r\n\
\r\n";

impl AlwaysUnauthorised {
    fn start() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind 127.0.0.1:0");
        let port = listener.local_addr().expect("listener address").port();
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(mut stream) = stream else { continue };
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                    // Read the whole request head before answering: closing a
                    // socket with unread data resets the connection, and git
                    // would then fail without ever asking for a credential.
                    let mut head = Vec::new();
                    let mut chunk = [0u8; 1024];
                    while head.len() < 8192 && !head.windows(4).any(|w| w == b"\r\n\r\n") {
                        match stream.read(&mut chunk) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => head.extend_from_slice(&chunk[..n]),
                        }
                    }
                    let _ = stream.write_all(UNAUTHORISED);
                }
            })
        };
        Self {
            port,
            stop,
            thread: Some(thread),
        }
    }

    fn url(&self) -> String {
        format!(
            "http://127.0.0.1:{}/issue-1764-{}.git",
            self.port,
            std::process::id()
        )
    }
}

impl Drop for AlwaysUnauthorised {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake the blocked accept so the thread sees the flag.
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// How a child started by [`run_until`] ended.
#[derive(Debug)]
enum Ran {
    /// The child exited by itself; `code` is `None` if a signal ended it.
    Exited {
        code: Option<i32>,
        elapsed: Duration,
        stdout: String,
        stderr: String,
    },
    /// The deadline passed and the test killed the child. The output is read
    /// only through `Debug`, in failure messages.
    #[allow(dead_code)]
    Killed { stdout: String, stderr: String },
}

/// Kills and reaps its child when dropped, so a panicking assertion cannot
/// leave a stopped git behind. It signals only this one child: no process
/// group, no `pkill`.
struct Reaped(Child);

impl Drop for Reaped {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

/// Run `command` with stdin null and its output in files, and kill it if it
/// has not exited within `limit`. Files rather than pipes, so neither a full
/// pipe nor a grandchild holding one open can stall the test.
fn run_until(command: &mut Command, limit: Duration) -> Ran {
    let out = tempfile::tempdir().expect("tempdir for the child's output");
    let stdout_path = out.path().join("stdout");
    let stderr_path = out.path().join("stderr");
    let started = Instant::now();
    // Never the test's own stdin: `script` would put a developer's terminal
    // into raw mode.
    let mut child = Reaped(
        command
            .stdin(Stdio::null())
            .stdout(File::create(&stdout_path).expect("stdout file"))
            .stderr(File::create(&stderr_path).expect("stderr file"))
            .spawn()
            .expect("spawn the child"),
    );
    let code = loop {
        if let Some(status) = child.0.try_wait().expect("poll the child") {
            break Some(status.code());
        }
        if started.elapsed() >= limit {
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let elapsed = started.elapsed();
    drop(child);
    let read =
        |path: &Path| String::from_utf8_lossy(&fs::read(path).unwrap_or_default()).into_owned();
    let (stdout, stderr) = (read(&stdout_path), read(&stderr_path));
    match code {
        Some(code) => Ran::Exited {
            code,
            elapsed,
            stdout,
            stderr,
        },
        None => Ran::Killed { stdout, stderr },
    }
}

/// Point a git (or a `script` that runs one) at `home` and away from every
/// stored credential: no global or system config, so no credential helper; a
/// `HOME` with no `.netrc`; no repository discovery above `home`. Git's
/// messages are kept in English so the assertions can read them.
fn sealed_from_credentials<'a>(command: &'a mut Command, home: &Path) -> &'a mut Command {
    command
        .current_dir(home)
        .env("HOME", home)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CEILING_DIRECTORIES",
            home.parent().expect("the temp HOME has a parent"),
        )
        .env("LC_ALL", "C")
}

/// Fail if a process whose command line contains `marker` is still running
/// 5 s after the child that started it has gone.
fn assert_nothing_left_behind(marker: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let left = processes_mentioning(marker);
        if left.is_empty() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "processes for {marker} outlived the test's child: {left:#?}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Every process (by `/proc`) whose command line contains `marker`, as
/// `"<pid> <command line>"`. Empty where there is no `/proc`.
fn processes_mentioning(marker: &str) -> Vec<String> {
    let Ok(entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let pid = entry.file_name().into_string().ok()?;
            if !pid.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let cmdline = fs::read(entry.path().join("cmdline")).ok()?;
            let cmdline = String::from_utf8_lossy(&cmdline).replace('\0', " ");
            cmdline
                .contains(marker)
                .then(|| format!("{pid} {}", cmdline.trim_end()))
        })
        .collect()
}

fn on_path(tool: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(tool).is_file()))
}

/// The value `command` sets for `key`, if it sets one.
fn env_value<'a>(command: &'a Command, key: &str) -> Option<&'a OsStr> {
    command
        .get_envs()
        .find(|(name, _)| *name == OsStr::new(key))
        .and_then(|(_, value)| value)
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|path| path.join("amplifier-bundle/recipes").is_dir())
        .map(Path::to_path_buf)
        .expect("workspace must contain amplifier-bundle/recipes")
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn recipe_text(name: &str) -> String {
    read(&workspace_root().join(format!("amplifier-bundle/recipes/{name}.yaml")))
}

/// The `prompt:` of step `id` in recipe `name`.
fn step_prompt(name: &str, id: &str) -> String {
    let recipe: Value = serde_yaml::from_str(&recipe_text(name))
        .unwrap_or_else(|e| panic!("parse {name}.yaml: {e}"));
    recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .unwrap_or_else(|| panic!("{name}.yaml declares no steps"))
        .iter()
        .find(|step| step.get("id").and_then(Value::as_str) == Some(id))
        .unwrap_or_else(|| panic!("{name}.yaml has no step `{id}`"))
        .get("prompt")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{name}.yaml step `{id}` has no prompt"))
        .to_string()
}

// ── (a) Environment ──────────────────────────────────────────────────────────

/// A test process gets the three variables from cargo's `[env]` table, and an
/// `isolated(..)` child gets them back after `env_clear()` has dropped them.
#[test]
fn part_a_test_processes_and_isolated_children_get_the_suppressors() {
    for (name, value) in NO_GIT_PROMPT_ENV {
        let got = std::env::var(name).ok();
        assert_eq!(
            got.as_deref(),
            Some(value),
            "{name} is {got:?} in the test process, not {value:?}. The [env] table in \
             .cargo/config.toml sets it with force = true, so find out why it did not \
             reach this test. Do not add it to .github/workflows/ci.yml: this check \
             exists to show that [env] reaches the tests."
        );
    }

    let child = isolated("x");
    for (name, value) in NO_GIT_PROMPT_ENV {
        assert_eq!(
            env_value(&child, name),
            Some(OsStr::new(value)),
            "isolated(..) must set {name}={value} after its env_clear(), which drops \
             the value from [env]"
        );
    }
}

/// A cargo config in the repository applies to everyone who builds it (SR1),
/// so it holds the three `[env]` entries and nothing else.
#[test]
fn part_a_cargo_config_holds_only_the_env_table() {
    let config = read(&workspace_root().join(".cargo/config.toml"));
    let lines: Vec<&str> = config
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    let expected: Vec<String> = std::iter::once("[env]".to_string())
        .chain(
            NO_GIT_PROMPT_ENV
                .iter()
                .map(|(name, value)| format!("{name} = {{ value = \"{value}\", force = true }}")),
        )
        .collect();
    assert_eq!(
        lines, expected,
        ".cargo/config.toml must hold exactly the [env] table for the three git prompt \
         variables; review any other setting line by line before changing this test"
    );
}

// ── (b) Git builder ──────────────────────────────────────────────────────────

/// A git started through the builder, with no askpass program and a null
/// stdin, fails at once with `terminal prompts disabled` instead of opening
/// the terminal. On CI, where there is no terminal, the message is what tells
/// this apart from the `No such device or address` failure that hid the trap.
#[test]
fn part_b_builder_git_fails_at_once_without_askpass() {
    let server = AlwaysUnauthorised::start();
    let url = server.url();
    let home = tempfile::tempdir().expect("temp HOME");

    let mut git = amplihack_git::command();
    // This process already has the variable from `[env]`, so git's behaviour
    // below cannot show whether the builder set it.
    assert_eq!(
        env_value(&git, "GIT_TERMINAL_PROMPT"),
        Some(OsStr::new("0")),
        "amplihack_git::command() must set GIT_TERMINAL_PROMPT=0 itself"
    );
    git.args(["ls-remote", &url]);
    // After the builder: it scrubs GIT_CEILING_DIRECTORIES.
    sealed_from_credentials(&mut git, home.path());
    for var in [
        "GIT_ASKPASS",
        "SSH_ASKPASS",
        "GIT_CONFIG_PARAMETERS",
        "GIT_CONFIG_COUNT",
    ]
    .into_iter()
    .chain(PROXY_VARS)
    {
        git.env_remove(var);
    }

    let ran = run_until(&mut git, BUILDER_DEADLINE);
    match &ran {
        Ran::Exited {
            code: Some(code),
            stderr,
            ..
        } if *code != 0 => assert!(
            stderr.contains("terminal prompts disabled"),
            "git failed, but not because terminal prompts are off: {ran:#?}"
        ),
        _ => panic!(
            "git should have exited non-zero by itself within {BUILDER_DEADLINE:?}, \
             with `terminal prompts disabled`: {ran:#?}"
        ),
    }
    assert_nothing_left_behind(&url);
}

// ── (c) Pseudo-terminal ──────────────────────────────────────────────────────

/// Run `timeout 8 git ls-remote <url>` under `script`, in a pseudo-terminal,
/// with `remove` taken out of the environment that `isolated(..)` builds.
///
/// The shell command must be compound. Given one simple command, the shell
/// execs it, `timeout` becomes the session leader and stays in the foreground
/// group, so git can read the terminal and the trap is never armed. With a
/// compound command the shell stays in the foreground, `timeout` puts git in
/// a background group, and a read of the terminal stops it on `SIGTTIN`.
/// Only the port and the process id are put into the shell string.
fn git_under_script(server: &AlwaysUnauthorised, remove: &[&str]) -> Ran {
    let home = tempfile::tempdir().expect("temp HOME");
    let inner = format!(
        "timeout {INNER_SECS} git ls-remote http://127.0.0.1:{port}/issue-1764-{pid}.git \
         </dev/null; rc=$?; echo \"rc=$rc\"; exit $rc",
        port = server.port,
        pid = std::process::id(),
    );
    let mut script = isolated("script");
    script
        .args(["-qec", &inner, "/dev/null"])
        .env("SHELL", "/bin/sh");
    sealed_from_credentials(&mut script, home.path());
    for var in remove {
        script.env_remove(var);
    }
    run_until(&mut script, PTY_DEADLINE)
}

/// The status in the `rc=<n>` line the inner shell prints. Git's prompt ends
/// without a newline, so in the control the status follows it on the same
/// line: `Username for '…': rc=124`.
fn rc_line(stdout: &str) -> Option<i32> {
    stdout
        .lines()
        .find_map(|line| line.trim().rsplit_once("rc=")?.1.parse().ok())
}

/// `ran` must have finished by itself, well inside the inner `timeout`, with
/// a status other than 124. Returns its output.
fn assert_fails_at_once<'a>(label: &str, ran: &'a Ran) -> &'a str {
    match ran {
        Ran::Exited {
            elapsed, stdout, ..
        } if *elapsed < Duration::from_secs(INNER_SECS)
            && rc_line(stdout).is_some_and(|rc| rc != 124) =>
        {
            stdout.as_str()
        }
        _ => panic!(
            "{label}: git should have failed at once, printing an rc= line other than \
             124 in under {INNER_SECS} s: {ran:#?}"
        ),
    }
}

/// The trap, and the variables that keep git out of it, in a real
/// pseudo-terminal. The control has none of the three variables and must not
/// finish normally: GNU `timeout` kills the stopped git and reports 124, and
/// uutils `timeout`, stopped as well, is killed at the test's own deadline,
/// which closes the pseudo-terminal and hangs up the stopped group.
#[test]
fn part_c_plain_timeout_trap_in_a_pseudo_terminal() {
    if !cfg!(target_os = "linux") {
        eprintln!("issue #1764 part (c) skipped: it needs util-linux `script` on Linux");
        return;
    }
    if let Some(missing) = ["script", "timeout"]
        .into_iter()
        .find(|tool| !on_path(tool))
    {
        eprintln!("issue #1764 part (c) skipped: `{missing}` is not on PATH");
        return;
    }
    let server = AlwaysUnauthorised::start();
    let url = server.url();
    let all: Vec<&str> = NO_GIT_PROMPT_ENV.iter().map(|(name, _)| *name).collect();

    let control = git_under_script(&server, &all);
    let armed = match &control {
        Ran::Killed { .. } => true,
        Ran::Exited { stdout, .. } => rc_line(stdout) == Some(124),
    };
    assert!(
        armed,
        "trap was not armed: with none of the three variables, git under plain \
         `timeout` in a pseudo-terminal should have stopped on SIGTTIN (rc=124, or \
         killed at the test's {PTY_DEADLINE:?} deadline). Without the trap, (c1) and \
         (c2) prove nothing: {control:#?}"
    );
    assert_nothing_left_behind(&url);

    let c1 = git_under_script(&server, &["GIT_ASKPASS", "SSH_ASKPASS"]);
    let c1_out = assert_fails_at_once("(c1) GIT_TERMINAL_PROMPT=0 alone", &c1);
    assert!(
        c1_out.contains("terminal prompts disabled"),
        "(c1) must fail because terminal prompts are off: {c1:#?}"
    );
    assert_nothing_left_behind(&url);

    let c2 = git_under_script(&server, &[]);
    assert_fails_at_once("(c2) all three variables", &c2);
    assert_nothing_left_behind(&url);
}

// ── (d) Recipe text ──────────────────────────────────────────────────────────

/// The agent steps that run long commands.
const LONG_COMMAND_STEPS: [(&str, &str); 3] = [
    ("workflow-precommit-test", "step-12-run-precommit"),
    ("workflow-precommit-test", "step-13-local-testing"),
    ("autodrive-merge-round", "step-04-address-blockers"),
];

/// Step 12's evidence schema, which the verification step's `jq` gate reads.
/// The guidance for long commands must not change it.
const STEP_12_SCHEMA: [&str; 2] = [
    "workspace_command (\"cargo test --workspace --locked\"),",
    "{\"status\":\"PASS\",\"exit_code\":0,\"precommit_exit_code\":0,\"workspace_exit_code\":0,\
     \"workspace_command\":\"cargo test --workspace --locked\",\"validation_scope\":\"FULL_WORKSPACE\",\
     \"test_threads\":\"DEFAULT\",\"evidence\":[{\"log\":\"<actual retained raw log>\",\
     \"exit_receipt\":\"<actual retained numeric exit receipt>\"}]}",
];

/// Each step tells the agent to export `GIT_TERMINAL_PROMPT=0`, to run a long
/// command as `setsid -w timeout --kill-after=..` or, failing that,
/// `timeout --foreground`, with stdin from `/dev/null`, and why: plain
/// `timeout` leaves a prompting git stopped on `SIGTTIN`.
#[test]
fn part_d_recipe_prompts_say_how_to_run_long_commands() {
    for (recipe, id) in LONG_COMMAND_STEPS {
        let prompt = step_prompt(recipe, id);
        for needle in [
            "GIT_TERMINAL_PROMPT=0",
            "setsid -w timeout --kill-after=",
            "timeout --foreground",
            "</dev/null",
            "SIGTTIN",
        ] {
            assert!(
                prompt.contains(needle),
                "{recipe}.yaml step `{id}` must tell the agent about `{needle}`"
            );
        }
        for weakening in WEAKENING {
            assert!(
                !prompt.contains(weakening),
                "{recipe}.yaml step `{id}` must not offer `{weakening}`"
            );
        }
    }

    let step_12 = step_prompt("workflow-precommit-test", "step-12-run-precommit");
    for pinned in STEP_12_SCHEMA {
        assert!(
            step_12.contains(pinned),
            "step 12's evidence schema must stay byte-for-byte: missing `{pinned}`"
        );
    }
    assert!(
        recipe_text("workflow-precommit-test")
            .contains(".workspace_command == \"cargo test --workspace --locked\""),
        "the verification step's jq gate on workspace_command must stay"
    );

    let lines = recipe_text("autodrive-merge-round").lines().count();
    assert!(
        lines <= BRICK_LINE_BUDGET,
        "autodrive-merge-round.yaml is {lines} lines; the brick budget is \
         {BRICK_LINE_BUDGET}, and the guidance must fit inside it"
    );
}

// ── Shell tests ──────────────────────────────────────────────────────────────

/// The shell tests that run the system git against a remote that is not
/// local. Run by hand with `bash`, outside cargo, they get no `[env]` table.
const SHELL_TESTS: [&str; 12] = [
    "tests/git_identity_test.sh",
    "tests/issue_1268_terminal_state_adjudication.sh",
    "tests/issue_1290_identity_preflight.sh",
    "tests/issue_1361_duplicate_pr_claim_check.sh",
    "tests/issue_1448_review_is_not_a_duplicate.sh",
    "tests/issue_1596_final_status_branch_pr_lookup.sh",
    "tests/issue_1676_build_resolves_agent_pr.sh",
    "tests/issue_1680_fresh_run_state_key.sh",
    "tests/outside-in/scripts/scenario8-checks.sh",
    "amplifier-bundle/recipes/tests/test-issue-983-azdo-work-item-assignment.sh",
    "amplifier-bundle/recipes/tests/test-default-workflow-reliability.sh",
    "amplifier-bundle/recipes/tests/test-issue-1060-existing-pr-is-idempotent.sh",
];

/// Straight after its `set` line, each script exports the three variables and
/// then takes its stdin from `/dev/null`, before any git can run.
#[test]
fn shell_tests_turn_git_prompts_off_at_the_top() {
    let root = workspace_root();
    let export = format!(
        "export {}",
        NO_GIT_PROMPT_ENV
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join(" ")
    );
    for script in SHELL_TESTS {
        let text = read(&root.join(script));
        let code: Vec<&str> = text
            .lines()
            .map(str::trim)
            .skip_while(|line| !line.starts_with("set "))
            .skip(1)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .take(2)
            .collect();
        assert_eq!(
            code,
            [export.as_str(), "exec </dev/null"],
            "{script}: the first commands after its `set` line must be `{export}` and \
             `exec </dev/null`"
        );
    }
}

// ── How-to ───────────────────────────────────────────────────────────────────

#[test]
fn howto_is_linked_and_offers_no_weakening() {
    let root = workspace_root();
    let howto = read(&root.join("docs/howto/keep-tests-from-prompting-for-git.md"));
    for (file, link) in [
        ("mkdocs.yml", "howto/keep-tests-from-prompting-for-git.md"),
        (
            "docs/index.md",
            "howto/keep-tests-from-prompting-for-git.md",
        ),
        (
            "docs/howto/develop-amplihack.md",
            "(./keep-tests-from-prompting-for-git.md)",
        ),
    ] {
        assert!(
            read(&root.join(file)).contains(link),
            "{file} must link the #1764 how-to"
        );
    }
    for weakening in WEAKENING {
        assert!(
            !howto.contains(weakening),
            "the #1764 how-to must not offer `{weakening}`"
        );
    }
}
