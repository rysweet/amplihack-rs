//! Issue #2582: a runner whose capability report does not validate ends the
//! run before a transport is chosen, so it never falls back to `--set`.
//!
//! `issue_2582_context_off_argv` covers the two reports that validate: one
//! that lists `context_file_v1`, which takes the file transport, and one that
//! does not, which takes `--set` unchanged. Neither covers a report that
//! fails validation outright — a truncated answer, a non-string capability
//! array, a missing `durable_recovery_v1`, or a probe that exits non-zero.
//!
//! `docs/reference/recipe-context-transport.md` states that such a report
//! ends the run at the probe, so a degraded runner does not reach the
//! fallback. Without this file that sentence rests on reading
//! `runner_validation::probe` and the order of the two call sites in
//! `execute.rs`, and a later change that moved the transport choice above the
//! probe, or that downgraded the probe error to a warning, would put the
//! whole context back on argv in exactly the case where the runner is already
//! known to be misbehaving.
//!
//! Each case runs the real `amplihack` binary against a `#!/bin/sh` runner
//! stub supplied through `RECIPE_RUNNER_RS_PATH`. The stub answers
//! `--capabilities` with the case's text and records its argv on any other
//! invocation. The run is expected to fail, and the argv record is expected
//! never to be written: the stub was asked for its capabilities and nothing
//! else.
//!
//! The last test is the control, and it carries a second assertion of its
//! own. It gives a report that validates but omits `context_file_v1`, and
//! asserts the stub *is* run with `--set` pairs that hold the values —
//! without that, every assertion above could pass because the fixture never
//! reached the runner at all. It then asserts that the run said so on
//! stderr. That notice is the only compensating control for a runner that
//! puts the whole context back on argv, and it is checked here rather than in
//! a unit test because only the real binary shows what an operator actually
//! sees: the `tracing` record of the same event is filtered out entirely
//! unless `RUST_LOG` is set, and `eprintln!` under `cargo test` writes to
//! libtest's capture buffer rather than to fd 2. This run sets no `RUST_LOG`.
//!
//! No sleep, deadline or poll: no case leaves a process running, so each run
//! is started and waited for with `output()`.

#![cfg(target_os = "linux")]

use crate::support::amplihack_bin::amplihack_bin;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};

/// Distinctive context values, each with a unique token, so a match can only
/// come from this test's context.
const VALUES: &[(&str, &str)] = &[
    ("task_description", "ZQXV2582DEGRADEWORD fix the widget"),
    ("pr_body", "ZQXV2582DEGRADEBODY {\"json\": [1, 2]}"),
];

const TOKENS: &[&str] = &["ZQXV2582DEGRADEWORD", "ZQXV2582DEGRADEBODY"];

const RECIPE: &str = "name: issue-2582-degraded\ndescription: degraded runner\n\
version: 1.0.0\ncontext: {}\nsteps:\n  - id: noop\n    type: bash\n    command: \"true\"\n";

/// A report that validates and lists `durable_recovery_v1`, without
/// `context_file_v1`. The control's capabilities.
const VALID_WITHOUT_CAPABILITY: &str = r#"{"schema_version":1,"version":"fixture","capabilities":["codex_exec","durable_recovery_v1"]}"#;

/// The runner stub. `answer` is the shell fragment that answers
/// `--capabilities`; everything else records argv and reports success.
fn runner_stub(answer: &str) -> String {
    format!(
        r#"#!/bin/sh
case "$1" in --capabilities) {answer} ;; esac
: > "$RECORD_DIR/argv.bin"
for a in "$@"; do printf '%s\0' "$a" >> "$RECORD_DIR/argv.bin"; done
printf '%s\n' '{{"recipe_name":"issue-2582-degraded","success":true,"step_results":[],"context":{{}}}}'
"#
    )
}

/// A deterministic tmux: `new-session ... -- CMD...` runs CMD in the
/// foreground. No tmux server is started.
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
}

impl Fixture {
    /// `answer` is the stub's `--capabilities` fragment.
    fn new(answer: &str) -> Self {
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
            runner_stub(answer),
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

        Self {
            _tmp: tmp,
            root,
            home,
            records,
            tools,
            work_root,
            recipe,
            context_file,
        }
    }

    /// Run `amplihack recipe run <recipe> --context-file <ctx> -w <root>` to
    /// completion in an isolated environment. The context reaches the run in
    /// a file, so no value is on this test's own command line either.
    fn run(&self) -> Output {
        crate::isolated_amplihack::isolated(amplihack_bin())
            .args(["recipe", "run"])
            .arg(&self.recipe)
            .arg("--context-file")
            .arg(&self.context_file)
            .arg("-w")
            .arg(&self.root)
            .current_dir(&self.root)
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
            .env("AMPLIHACK_AGENT_BINARY", "claude")
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
            .env("AMPLIHACK_SKIP_REFLECTION", "1")
            .env("AMPLIHACK_CLEANUP_ON_EXIT", "0")
            .env("AMPLIHACK_GOVERNOR", "off")
            .env("AMPLIHACK_USAGE_LIMIT_WAIT", "0")
            // The environment is inherited, so an operator running the suite
            // with `RUST_LOG=info` set would otherwise make the `tracing`
            // record appear and mask a missing stderr notice.
            .env_remove("RUST_LOG")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("run amplihack")
    }

    /// The stub's recorded argv, or `None` when it was never run for the
    /// recipe.
    fn runner_argv(&self) -> Option<Vec<String>> {
        let raw = fs::read(self.records.join("argv.bin")).ok()?;
        Some(
            raw.split(|b| *b == 0)
                .filter(|s| !s.is_empty())
                .map(|s| String::from_utf8_lossy(s).into_owned())
                .collect(),
        )
    }

    /// Every `<runtime>/context` folder left anywhere under the work root.
    fn leftover_context_dirs(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let Ok(entries) = fs::read_dir(dir) else {
            return out;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !entry.file_type().is_ok_and(|k| k.is_dir()) {
                continue;
            }
            if entry.file_name() == "context" {
                out.push(path.clone());
            }
            out.extend(Self::leftover_context_dirs(&path));
        }
        out
    }
}

fn describe(out: &Output) -> String {
    format!(
        "status {}\n--- stdout ---\n{}\n--- stderr ---\n{}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// The whole condition for one degraded report: the run fails, the runner is
/// never launched for the recipe, no value is printed and no context folder
/// is left behind.
fn assert_ends_before_a_transport_is_chosen(answer: &str, case: &str) {
    let f = Fixture::new(answer);
    let out = f.run();

    assert!(
        !out.status.success(),
        "{case}: the run must fail when the capability report does not \
         validate:\n{}",
        describe(&out)
    );

    // The condition itself. The stub records argv on every invocation that is
    // not `--capabilities`, so an empty record means the runner was asked for
    // its capabilities and nothing else: no `--set`, and no `--context-file`.
    if let Some(argv) = f.runner_argv() {
        panic!(
            "{case}: the runner was launched for the recipe after its report \
             failed validation, with argv {argv:?}\n{}",
            describe(&out)
        );
    }

    // The failure is reported without echoing what the run was carrying.
    for (name, stream) in [("stdout", &out.stdout), ("stderr", &out.stderr)] {
        let text = String::from_utf8_lossy(stream).into_owned();
        for token in TOKENS {
            assert!(
                !text.contains(token),
                "{case}: the failure printed the context value {token} on \
                 {name}: {text}"
            );
        }
    }

    // Nothing was written for a transport that was never chosen.
    let leftovers = Fixture::leftover_context_dirs(&f.work_root);
    assert!(
        leftovers.is_empty(),
        "{case}: a context folder was written for a run that never chose a \
         transport: {leftovers:?}"
    );
}

#[test]
fn a_truncated_report_ends_the_run_before_a_transport_is_chosen() {
    assert_ends_before_a_transport_is_chosen(
        r#"printf '%s\n' '{"schema_version":1,"version":"fixture","capabilities":["durable_recovery_v1"'; exit 0"#,
        "truncated JSON",
    );
}

#[test]
fn a_non_string_capability_array_ends_the_run_before_a_transport_is_chosen() {
    assert_ends_before_a_transport_is_chosen(
        r#"printf '%s\n' '{"schema_version":1,"version":"fixture","capabilities":[1,2]}'; exit 0"#,
        "non-string capabilities",
    );
}

#[test]
fn a_report_without_durable_recovery_ends_the_run_before_a_transport_is_chosen() {
    assert_ends_before_a_transport_is_chosen(
        r#"printf '%s\n' '{"schema_version":1,"version":"fixture","capabilities":["codex_exec"]}'; exit 0"#,
        "missing durable_recovery_v1",
    );
}

#[test]
fn a_probe_that_fails_ends_the_run_before_a_transport_is_chosen() {
    assert_ends_before_a_transport_is_chosen(
        r#"printf '%s\n' 'not json at all' >&2; exit 3"#,
        "probe exits non-zero",
    );
}

/// The control. A report that validates and omits `context_file_v1` reaches
/// the runner on the `--set` path with the values on argv, which is what a
/// runner older than the pin supplied through `RECIPE_RUNNER_RS_PATH` still
/// does. Without this, every assertion above could hold because the fixture
/// never reached the runner for an unrelated reason.
#[test]
fn a_valid_report_without_the_capability_still_reaches_the_runner_with_set() {
    let f = Fixture::new(&format!(
        r#"printf '%s\n' '{VALID_WITHOUT_CAPABILITY}'; exit 0"#
    ));
    let out = f.run();
    let argv = f.runner_argv().unwrap_or_else(|| {
        panic!(
            "the runner was never launched, so the four cases above prove \
             nothing:\n{}",
            describe(&out)
        )
    });
    let joined = argv.join(" ");
    assert!(
        argv.iter().any(|a| a == "--set" || a == "-c"),
        "expected `--set` pairs on the fallback path: {argv:?}"
    );
    for token in TOKENS {
        assert!(
            joined.contains(token),
            "the fallback must still carry {token} on argv, or the absence \
             the other cases assert would mean nothing: {argv:?}"
        );
    }

    // The downgrade is announced. `Fixture::run` sets no `RUST_LOG`, so this
    // is what an operator sees with no opt-in of any kind. Matched on the
    // distinctive middle of the line rather than the whole of it, so
    // rewording the sentence does not fail the test while dropping it does.
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        stderr.contains("recipe context is on the runner's argv"),
        "the run took the --set transport and must say so on stderr, or the \
         downgrade is visible only in /proc:\n{}",
        describe(&out)
    );
    assert!(
        stderr.contains("context_file_v1") && stderr.contains("RECIPE_RUNNER_RS_PATH"),
        "the notice must name the missing capability and the override that \
         supplied this runner:\n{}",
        describe(&out)
    );
    // stderr and NOT stdout. stdout is the run's machine-readable channel —
    // the log-pointer JSON a caller parses — so a human-readable notice there
    // would corrupt a consumer. Asserting only "it is on stderr" leaves the
    // choice of stream untested, since a notice printed to BOTH would pass.
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        !stdout.contains("recipe context is on the runner's argv"),
        "the downgrade notice must go to stderr alone, never stdout:\n{}",
        describe(&out)
    );
    // The notice is printed because the context is already exposed; it must
    // not add to the exposure. (The run's other output is covered by
    // `assert_ends_before_a_transport_is_chosen`; here the values are on argv
    // by design, so only the notice's own line is checked.)
    for line in stderr
        .lines()
        .filter(|l| l.contains("recipe context is on the runner's argv"))
    {
        for token in TOKENS {
            assert!(
                !line.contains(token),
                "the downgrade notice printed the context value {token}: {line}"
            );
        }
    }
}
