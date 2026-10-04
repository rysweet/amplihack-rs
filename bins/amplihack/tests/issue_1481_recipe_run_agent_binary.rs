//! Issue #1481: `amplihack recipe run` launched from a Claude Code session ran
//! its agent steps under Copilot.
//!
//! The dev-orchestrator skill tells callers to `env -u CLAUDECODE`, and
//! recipe-runner-rs spawns every step under `env_clear()` plus a curated
//! environment. A nested `amplihack` resolving the agent binary on its own
//! could no longer see any Claude marker, fell through to the vendor default,
//! npm-installed Copilot, and persisted a launcher context that pinned later
//! runs in the checkout to it.
//!
//! These tests run the real binary under a cleared environment with a stub in
//! place of recipe-runner-rs, and assert what the runner is handed -- which is
//! all any agent step can know about the session that started it.

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SOURCE_ENV: &str = "AMPLIHACK_AGENT_BINARY_SOURCE";

struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("create tempdir");
        let runner = dir.path().join("recipe-runner-rs");
        // Records what it was handed. The unset/empty distinction matters for
        // the tag, so it is written as an explicit marker rather than "".
        fs::write(
            &runner,
            format!(
                "#!/bin/sh\n\
                 out=\"$(dirname \"$0\")/probe.json\"\n\
                 printf '{{\"agent_binary\":\"%s\",\"source\":\"%s\"}}' \
                   \"${{AMPLIHACK_AGENT_BINARY-<unset>}}\" \"${{{SOURCE_ENV}-<unset>}}\" > \"$out\"\n\
                 printf '%s' '{{\"recipe_name\":\"probe\",\"success\":true,\"step_results\":[],\"context\":{{}}}}'\n"
            ),
        )
        .expect("write runner stub");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&runner, fs::Permissions::from_mode(0o755))
                .expect("chmod runner stub");
        }
        fs::write(
            dir.path().join("probe.yaml"),
            "name: probe\nsteps:\n  - id: noop\n    type: bash\n    command: \"true\"\n",
        )
        .expect("write recipe");
        fs::create_dir_all(dir.path().join("home")).expect("create home");
        // A `.git` boundary stops the launcher-context walk-up at the work
        // dir, so a context file above the temp dir (for example with TMPDIR
        // under $HOME) cannot answer for these tests.
        fs::create_dir_all(dir.path().join("work").join(".git")).expect("create work dir");
        Self { dir }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn work(&self) -> PathBuf {
        self.path().join("work")
    }

    /// Run `amplihack recipe run` under a cleared environment, adding only
    /// what a caller would really have plus `extra`.
    fn run(&self, extra: &[(&str, &str)]) -> (Output, Value) {
        self.run_with_args(&[], extra)
    }

    /// [`Fixture::run`] with extra `recipe run` arguments.
    fn run_with_args(&self, args: &[&std::ffi::OsStr], extra: &[(&str, &str)]) -> (Output, Value) {
        let mut command = self.harness(Command::new(env!("CARGO_BIN_EXE_amplihack")));
        command
            .arg("recipe")
            .arg("run")
            .arg(self.path().join("probe.yaml"))
            .args(args);
        for (key, value) in extra {
            command.env(key, value);
        }
        let output = command.output().expect("run amplihack");
        let probe = self.take_probe(&output);
        (output, probe)
    }

    /// `command` under a cleared environment holding only what the test
    /// harness needs, run from the work dir.
    fn harness(&self, mut command: Command) -> Command {
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", self.path().join("home"))
            .env("TMPDIR", std::env::temp_dir())
            .env(
                "RECIPE_RUNNER_RS_PATH",
                self.path().join("recipe-runner-rs"),
            )
            .env("AMPLIHACK_HOME", self.path())
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            // The probe recipe is bash-only, so #1482's root pre-flight never
            // applies; set anyway so no root host can make these tests depend
            // on its sandbox detection.
            .env("IS_SANDBOX", "1")
            .current_dir(self.work());
        command
    }

    /// What the recipe-runner stub recorded, consumed so the next run starts
    /// clean.
    fn take_probe(&self, output: &Output) -> Value {
        let probe_path = self.path().join("probe.json");
        let probe = fs::read_to_string(&probe_path).unwrap_or_else(|_| {
            panic!(
                "recipe-runner stub never ran.\nstdout: {}\nstderr: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        });
        fs::remove_file(&probe_path).ok();
        serde_json::from_str(&probe).expect("probe is JSON")
    }
}

fn handed(probe: &Value) -> (&str, &str) {
    (
        probe["agent_binary"].as_str().unwrap(),
        probe["source"].as_str().unwrap(),
    )
}

/// The field failure, exactly: the caller followed the skill and removed
/// CLAUDECODE, but the rest of the Claude Code session's markers are present.
#[test]
fn a_claude_session_without_claudecode_hands_claude_to_the_runner() {
    let fx = Fixture::new();
    let (output, probe) = fx.run(&[
        ("CLAUDE_CODE_SESSION_ID", "session_0123"),
        ("CLAUDE_CODE_REMOTE", "true"),
    ]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        handed(&probe),
        ("claude", "<unset>"),
        "every agent step inherits this value; resolving it anywhere below \
         recipe run is resolving it without the session's markers"
    );
}

/// Claude Code exports CLAUDE_CODE_ENTRYPOINT in every mode. On a host where
/// CLAUDECODE was the only other marker, `env -u CLAUDECODE` left nothing.
#[test]
fn claude_code_entrypoint_alone_identifies_a_claude_session() {
    let fx = Fixture::new();
    let (output, probe) = fx.run(&[("CLAUDE_CODE_ENTRYPOINT", "cli")]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(handed(&probe), ("claude", "<unset>"));
}

/// With nothing to go on, the vendor default is still what runs -- but it is
/// handed down marked as a guess, and the caller is told.
#[test]
fn a_default_layer_answer_is_tagged_and_reported() {
    let fx = Fixture::new();
    let (output, probe) = fx.run(&[]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(handed(&probe), ("copilot", "default:copilot"));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("'copilot'") && stderr.contains("AMPLIHACK_AGENT_BINARY"),
        "an inferred agent binary must be visible without RUST_LOG:\n{stderr}"
    );
}

/// A guess inherited from a parent must not outrank a marker this process can
/// see -- otherwise the tag would be decoration and one level's fallback would
/// still decide for every level beneath it.
#[test]
fn an_inherited_guess_yields_to_a_visible_session_marker() {
    let fx = Fixture::new();
    let (output, probe) = fx.run(&[
        ("AMPLIHACK_AGENT_BINARY", "copilot"),
        (SOURCE_ENV, "default:copilot"),
        ("CLAUDE_CODE_SESSION_ID", "session_0123"),
    ]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(handed(&probe), ("claude", "<unset>"));
}

/// ...and with nothing better visible, it stays a guess on the way down.
#[test]
fn an_inherited_guess_stays_tagged_when_nothing_better_is_visible() {
    let fx = Fixture::new();
    let (output, probe) = fx.run(&[
        ("AMPLIHACK_AGENT_BINARY", "copilot"),
        (SOURCE_ENV, "default:copilot"),
    ]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(handed(&probe), ("copilot", "default:copilot"));
    // The variable was present, only tagged: the notice must not claim none
    // was found, and must say how to turn it into a choice.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("no AMPLIHACK_AGENT_BINARY")
            && stderr.contains("unset AMPLIHACK_AGENT_BINARY_SOURCE"),
        "{stderr}"
    );
}

/// Every step of a default-guess run inherits the tag. A step that then sets a
/// different binary on purpose has chosen it; the stale tag describes an
/// earlier value and must not veto the new one.
#[test]
fn a_stale_tag_does_not_veto_a_binary_set_after_it() {
    let fx = Fixture::new();
    let (output, probe) = fx.run(&[
        ("AMPLIHACK_AGENT_BINARY", "codex"),
        (SOURCE_ENV, "default:copilot"),
    ]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(handed(&probe), ("codex", "<unset>"));
}

/// A value outside the allowlist is rejected, and the run falls back to the
/// default. The notice must say the value was rejected, not that none was
/// found, and must not echo it.
#[test]
fn a_rejected_value_is_reported_as_rejected() {
    let fx = Fixture::new();
    let (output, probe) = fx.run(&[("AMPLIHACK_AGENT_BINARY", "claude-code")]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(handed(&probe), ("copilot", "default:copilot"));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("is set but is not one of amplifier, claude, codex or copilot")
            && !stderr.contains("no AMPLIHACK_AGENT_BINARY")
            && !stderr.contains("claude-code"),
        "{stderr}"
    );
}

/// An explicit choice still beats everything, and is not tagged.
#[test]
fn an_explicit_binary_wins_and_is_not_tagged() {
    let fx = Fixture::new();
    let (output, probe) = fx.run(&[
        ("AMPLIHACK_AGENT_BINARY", "codex"),
        ("CLAUDE_CODE_SESSION_ID", "session_0123"),
    ]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(handed(&probe), ("codex", "<unset>"));
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("agent steps will run under"),
        "an explicit choice is not an inference and needs no notice"
    );
}

/// `recipe run` itself never stamps the checkout; only a launcher does, and
/// only for a launcher a session chose.
#[test]
fn recipe_run_writes_no_launcher_context() {
    let fx = Fixture::new();
    let (output, _) = fx.run(&[]);
    assert!(output.status.success(), "{output:?}");
    assert!(
        !fx.work()
            .join(".claude/runtime/launcher_context.json")
            .exists(),
        "a default-layer run must not leave a launcher context behind"
    );
}

/// The steps run in `--working-dir`, so that is where a nested `amplihack`
/// looks for a launcher context. Resolving the run's binary from the caller's
/// cwd instead would tag `default:copilot` while the level below read a
/// context naming another CLI, and one run would mix CLIs.
#[test]
fn the_launcher_context_is_read_from_the_working_dir() {
    let fx = Fixture::new();
    let project = fx.path().join("project");
    fs::create_dir_all(project.join(".git")).expect("create project");
    amplihack_cli::launcher_context::write_launcher_context(
        &project,
        amplihack_cli::launcher_context::LauncherKind::Codex,
        "amplihack codex",
        Default::default(),
    )
    .expect("write launcher context");
    let (output, probe) = fx.run_with_args(&["--working-dir".as_ref(), project.as_os_str()], &[]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(handed(&probe), ("codex", "<unset>"));
}

/// Every `launcher_context.json` under `root`.
fn launcher_contexts_under(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && !path.is_symlink() {
                pending.push(path);
            } else if path
                .file_name()
                .is_some_and(|n| n == "launcher_context.json")
            {
                found.push(path);
            }
        }
    }
    found
}

/// Crusty round 4: `amplihack <tool> --auto` built its child with
/// `with_agent_binary`, which clears the tag. The inner launcher then saw an
/// untagged value, handed it on untagged and persisted it. A launcher started
/// on an inherited guess naming itself must hand the guess on as a guess on
/// every path, and persist it nowhere.
#[cfg(unix)]
#[test]
fn auto_mode_hands_an_inherited_guess_on_tagged_and_persists_nothing() {
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant};

    let fx = Fixture::new();
    let bin = fx.path().join("bin");
    let tmp = fx.path().join("tmp");
    fs::create_dir_all(&bin).expect("create bin");
    fs::create_dir_all(&tmp).expect("create tmp");
    let probe = fx.path().join("claude-probe.log");
    // A stand-in `claude` that records what each invocation was handed.
    fs::write(
        bin.join("claude"),
        format!(
            "#!/bin/sh\n\
             if [ \"$1\" = \"--version\" ]; then echo '2.0.0 (Claude Code)'; exit 0; fi\n\
             printf '%s|%s\\n' \"${{AMPLIHACK_AGENT_BINARY-<unset>}}\" \"${{{SOURCE_ENV}-<unset>}}\" >> '{}'\n\
             exit 0\n",
            probe.display()
        ),
    )
    .expect("write claude stub");
    fs::set_permissions(bin.join("claude"), fs::Permissions::from_mode(0o755))
        .expect("chmod claude stub");

    let path = std::env::join_paths(std::iter::once(bin.clone()).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .expect("join PATH");
    let mut child = Command::new(env!("CARGO_BIN_EXE_amplihack"))
        .env_clear()
        .env("PATH", path)
        .env("HOME", fx.path().join("home"))
        .env("TMPDIR", &tmp)
        .env("AMPLIHACK_HOME", fx.path())
        .env("AMPLIHACK_NONINTERACTIVE", "1")
        .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
        // Hermetic under root: without it, #1482's guard refuses to launch
        // `claude --dangerously-skip-permissions` before the stub ever runs,
        // and this test would fail whatever the tag handling did.
        .env("IS_SANDBOX", "1")
        .env("AMPLIHACK_AGENT_BINARY", "claude")
        .env(SOURCE_ENV, "default:claude")
        .current_dir(fx.work())
        .args(["claude", "--auto", "--max-turns", "1", "--", "-p", "hi"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("run amplihack claude --auto");
    let started = Instant::now();
    while child.try_wait().expect("try_wait").is_none() {
        if started.elapsed() > Duration::from_secs(120) {
            let _ = child.kill();
            panic!("amplihack claude --auto did not exit within 120s");
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    let calls = fs::read_to_string(&probe).unwrap_or_default();
    assert!(
        !calls.is_empty(),
        "the claude stub was never run as an agent session"
    );
    for call in calls.lines() {
        assert_eq!(
            call, "claude|default:claude",
            "every level must still see the guess as a guess:\n{calls}"
        );
    }
    let persisted = launcher_contexts_under(fx.path());
    assert!(
        persisted.is_empty(),
        "a default-guess launch must persist nothing, found {persisted:?}"
    );
}

// ---------------------------------------------------------------------------
// Issue #1525: an empty or malformed launcher_context.json used to be dropped
// without a word, and the walk-up carried on past it. The run then took the
// default (or a parent directory's file), and the only notice said that no
// variable or marker was found -- naming no file at all.
// ---------------------------------------------------------------------------

/// Write `body` as the launcher context in `dir` and return its path as the
/// resolver will report it.
fn write_raw_launcher_context(dir: &Path, body: &str) -> PathBuf {
    let runtime = dir.join(".claude").join("runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(runtime.join("launcher_context.json"), body).expect("write launcher context");
    dir.canonicalize()
        .expect("canonicalize")
        .join(".claude/runtime/launcher_context.json")
}

#[test]
fn an_empty_launcher_context_is_named_with_its_reason() {
    let fx = Fixture::new();
    let path = write_raw_launcher_context(&fx.work(), "");
    let (output, probe) = fx.run(&[]);
    assert!(output.status.success(), "{output:?}");
    // Still the default, still tagged -- but no longer silent about why.
    assert_eq!(handed(&probe), ("copilot", "default:copilot"));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&format!(
            "amplihack: ignored {}: it is empty.",
            path.display()
        )),
        "{stderr}"
    );
}

#[test]
fn a_malformed_launcher_context_is_named_with_its_reason() {
    let fx = Fixture::new();
    let path = write_raw_launcher_context(&fx.work(), r#"{"launcher": "claude""#);
    let (output, probe) = fx.run(&[]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(handed(&probe), ("copilot", "default:copilot"));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&format!(
            "amplihack: ignored {}: it is not valid JSON (line 1,",
            path.display()
        )),
        "{stderr}"
    );
}

/// The fall-through is kept, so a bad file in a subdirectory still lets the
/// project's file answer. Both are named: the one that was read (not a fixed
/// relative path) and the one that was skipped.
#[test]
fn a_bad_file_below_a_good_one_is_named_alongside_the_file_that_answered() {
    let fx = Fixture::new();
    let project = fx.path().join("project");
    fs::create_dir_all(project.join(".git")).expect("create project");
    amplihack_cli::launcher_context::write_launcher_context(
        &project,
        amplihack_cli::launcher_context::LauncherKind::Codex,
        "amplihack codex",
        Default::default(),
    )
    .expect("write launcher context");
    let sub = project.join("sub");
    let skipped = write_raw_launcher_context(&sub, "");
    let (output, probe) = fx.run_with_args(&["--working-dir".as_ref(), sub.as_os_str()], &[]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(handed(&probe), ("codex", "<unset>"));
    let stderr = String::from_utf8_lossy(&output.stderr);
    let read = project
        .canonicalize()
        .expect("canonicalize")
        .join(".claude/runtime/launcher_context.json");
    assert!(
        stderr.contains(&format!("read from {}", read.display())),
        "{stderr}"
    );
    assert!(
        stderr.contains(&format!(
            "amplihack: ignored {}: it is empty.",
            skipped.display()
        )),
        "{stderr}"
    );
}

// ---------------------------------------------------------------------------
// Issue #1525: a detached launch loses the agent binary. Once a tmux server is
// running, `tmux new-session` gives the command the server's environment, not
// the caller's, so the session markers never reach `recipe run` (#1335). The
// documented hand-off (dev-orchestrator reference.md) is
//
//   tmux new-session -d -s NAME \
//     "cd REPO && $(amplihack agent-binary --shell -w REPO) amplihack recipe run ..."
//
// `$(...)` expands in the caller's shell, where the markers still are; the
// command string then runs under whatever environment the far side has.
// ---------------------------------------------------------------------------

use amplihack_utils::agent_binary::SESSION_MARKERS;

/// Every marker a Claude Code session can export.
fn claude_session() -> Vec<(&'static str, &'static str)> {
    SESSION_MARKERS
        .iter()
        .filter(|(_, binary)| *binary == "claude")
        .map(|(key, _)| (*key, "1"))
        .collect()
}

/// `env` arguments removing every session marker, from the canonical list.
fn without_any_marker() -> String {
    SESSION_MARKERS
        .iter()
        .map(|(key, _)| format!("-u {key}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The documented hand-off's command string. The caller's shell expands it.
const HAND_OFF: &str =
    r#""cd '$WORK' && $(amplihack agent-binary --shell -w "$WORK") amplihack recipe run '$PROBE'""#;

impl Fixture {
    /// Run `script` in the caller's shell: the harness environment plus
    /// `caller_env`, with this build of amplihack on PATH as `amplihack`.
    fn caller_shell(&self, script: &str, caller_env: &[(&str, &str)]) -> Output {
        let bin_dir = Path::new(env!("CARGO_BIN_EXE_amplihack"))
            .parent()
            .expect("amplihack has a directory")
            .to_path_buf();
        let path = std::env::join_paths(std::iter::once(bin_dir).chain(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        )))
        .expect("join PATH");
        let mut command = self.harness(Command::new("sh"));
        command
            .env("PATH", path)
            .env("WORK", self.work())
            .env("PROBE", self.path().join("probe.yaml"))
            .arg("-c")
            .arg(script);
        for (key, value) in caller_env {
            command.env(key, value);
        }
        command.output().expect("run the caller's shell")
    }

    /// The hand-off into a far side whose environment has no session marker
    /// at all -- a tmux server that never saw this session -- plus
    /// `far_side_env`, which the caller's shell exports to it.
    fn hand_off_to_a_markerless_shell(
        &self,
        caller_env: &[(&str, &str)],
        far_side_env: &[(&str, &str)],
    ) -> (Output, Value) {
        let exports: String = far_side_env
            .iter()
            .map(|(key, value)| format!("{key}={value} "))
            .collect();
        let script = format!("env {} {exports}sh -c {HAND_OFF}", without_any_marker());
        let output = self.caller_shell(&script, caller_env);
        let probe = self.take_probe(&output);
        (output, probe)
    }
}

/// The issue: from a Claude Code session, every marker stripped on the far
/// side, the documented hand-off alone must deliver `claude`, untagged.
#[test]
fn the_hand_off_carries_the_callers_binary_across_a_markerless_launch() {
    let fx = Fixture::new();
    let (output, probe) = fx.hand_off_to_a_markerless_shell(&claude_session(), &[]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(handed(&probe), ("claude", "<unset>"));
}

/// Without the hand-off the same far side falls back to the default. This is
/// what makes the case above evidence rather than coincidence.
#[test]
fn without_the_hand_off_the_markerless_launch_takes_the_default() {
    let fx = Fixture::new();
    let script = format!(
        "env {} sh -c \"cd '$WORK' && amplihack recipe run '$PROBE'\"",
        without_any_marker()
    );
    let output = fx.caller_shell(&script, &claude_session());
    let probe = fx.take_probe(&output);
    assert_eq!(handed(&probe), ("copilot", "default:copilot"));
}

/// An inferred value must stay a guess across the hand-off. Handing over only
/// AMPLIHACK_AGENT_BINARY would make it an untagged instruction, and the
/// nested launcher would persist it (#1481).
#[test]
fn an_inferred_value_stays_tagged_across_the_hand_off() {
    let fx = Fixture::new();
    let (output, probe) = fx.hand_off_to_a_markerless_shell(&[], &[]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(handed(&probe), ("copilot", "default:copilot"));
    // The caller is told, on its own terminal, that it handed on a guess.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("amplihack: resolved the agent binary to 'copilot' (no AMPLIHACK_AGENT_BINARY or agent session marker was found)"),
        "{stderr}"
    );
}

/// The far side may already hold a stale tag naming the same binary. The
/// hand-off clears it, or the caller's observation would arrive as a guess
/// and lose to the default again.
#[test]
fn the_hand_off_clears_a_stale_tag_on_the_far_side() {
    let fx = Fixture::new();
    let (output, probe) = fx.hand_off_to_a_markerless_shell(
        &claude_session(),
        &[("AMPLIHACK_AGENT_BINARY_SOURCE", "default:claude")],
    );
    assert!(output.status.success(), "{output:?}");
    assert_eq!(handed(&probe), ("claude", "<unset>"));
}

/// The same hand-off through a real tmux server started without any marker,
/// when tmux is installed. CI images need not have it; the cases above model
/// the same mechanics with `env`.
#[cfg(unix)]
#[test]
fn the_hand_off_works_through_a_real_tmux_server() {
    use std::time::{Duration, Instant};
    if Command::new("tmux").arg("-V").output().is_err() {
        eprintln!("skipping: tmux is not installed");
        return;
    }
    let fx = Fixture::new();
    // The server socket lives under /tmp, not under the fixture. A Unix socket
    // path is limited to about 108 bytes, and a deep TMPDIR (a nested workflow
    // runner's is ~100 bytes on its own) pushes `<fixture>/tmux/tmux-<uid>/<name>`
    // past it: tmux then fails with "File name too long" before the hand-off
    // is ever exercised.
    let sockets = tempfile::Builder::new()
        .prefix("hoff")
        .tempdir_in("/tmp")
        .expect("create a short tmux socket dir under /tmp");
    let socket = sockets.path().join("s");
    let socket = socket.to_str().expect("utf-8 path").to_string();
    let script = format!(
        "env {} tmux -S \"$TMUX_SOCKET\" -f /dev/null new-session -d -s handoff {HAND_OFF}",
        without_any_marker()
    );
    let mut caller_env = claude_session();
    caller_env.push(("TMUX_SOCKET", &socket));
    let output = fx.caller_shell(&script, &caller_env);
    assert!(
        output.status.success(),
        "tmux new-session failed: {output:?}"
    );

    let probe_path = fx.path().join("probe.json");
    let started = Instant::now();
    while !probe_path.exists() && started.elapsed() < Duration::from_secs(60) {
        std::thread::sleep(Duration::from_millis(100));
    }
    // Give the stub a moment to finish writing, then stop the server.
    std::thread::sleep(Duration::from_millis(200));
    let _ = Command::new("tmux")
        .args(["-S", &socket, "kill-server"])
        .output();
    let probe = fx.take_probe(&output);
    assert_eq!(handed(&probe), ("claude", "<unset>"));
}

// ---------------------------------------------------------------------------
// `$(amplihack agent-binary --shell)` is spliced into a command line, so its
// stdout must be the assignment line and nothing else, whatever the log
// filter. The resolver logs every answer at DEBUG and every inferred answer at
// WARN, and `tracing_subscriber::fmt()` writes to stdout unless told otherwise.
// With `RUST_LOG` set in the caller's shell, those lines would land in front of
// the assignments, and the far side would try to run a timestamp as a command.
// ---------------------------------------------------------------------------

/// `amplihack agent-binary --shell` run directly, stdout and stderr apart.
fn agent_binary_shell(fx: &Fixture, env: &[(&str, &str)]) -> Output {
    let mut command = fx.harness(Command::new(env!("CARGO_BIN_EXE_amplihack")));
    command.args(["agent-binary", "--shell"]);
    for (key, value) in env {
        command.env(key, value);
    }
    command
        .output()
        .expect("run amplihack agent-binary --shell")
}

/// With the most verbose filter, stdout is still exactly the assignment line.
/// The WARN for the default guess must still be emitted, on stderr; that is
/// what makes this a test of where the log goes rather than of whether one
/// was written.
#[test]
fn a_log_filter_does_not_leak_into_the_shell_output() {
    let fx = Fixture::new();
    let output = agent_binary_shell(&fx, &[("RUST_LOG", "trace")]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "AMPLIHACK_AGENT_BINARY=copilot AMPLIHACK_AGENT_BINARY_SOURCE=default:copilot\n",
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("assuming the built-in default"),
        "the resolver's WARN should be on stderr under RUST_LOG=trace:\n{stderr}"
    );
}

/// The documented hand-off, end to end, with `RUST_LOG` exported in the
/// caller's shell: an observed answer (DEBUG) still arrives as `claude`.
#[test]
fn the_hand_off_survives_a_verbose_log_filter() {
    let fx = Fixture::new();
    let mut caller_env = claude_session();
    caller_env.push(("RUST_LOG", "debug"));
    let (output, probe) = fx.hand_off_to_a_markerless_shell(&caller_env, &[]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(handed(&probe), ("claude", "<unset>"));
}

/// ...and an inferred one (WARN) still arrives as a tagged guess.
#[test]
fn an_inferred_value_survives_the_hand_off_under_a_warn_filter() {
    let fx = Fixture::new();
    let (output, probe) = fx.hand_off_to_a_markerless_shell(&[("RUST_LOG", "warn")], &[]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(handed(&probe), ("copilot", "default:copilot"));
}
