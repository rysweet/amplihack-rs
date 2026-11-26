//! `text.routing` and `text.prompt_preferences` at the hook program's process
//! boundary (issue #3247, and #3199 for recipe steps).
//!
//! Every run starts the real `amplihack-hooks` in a fresh [`HookSandbox`]
//! whose environment is cleared, so no setting of the test process reaches a
//! hook. The settings come from a user file under the sandbox's `HOME`, a
//! project file under the sandbox's project, or the variables the test sets.
//!
//! Outputs are compared with the same hook run with nothing set, in the same
//! test, never with stored text. Text is detected by its JSON field, by a
//! constant the code inserts, or by the setting's name in a report; no test
//! pins the wording of any text.
#![cfg(unix)]

#[path = "../../../tests/support/isolated_hook.rs"]
mod isolated_hook;

use amplihack_types::workflow::DEFAULT_WORKFLOW_SESSION_CONTEXT;
use isolated_hook::HookSandbox;
use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::Stdio;

const BIN: &str = env!("CARGO_BIN_EXE_amplihack-hooks");
const ROUTING_VAR: &str = "AMPLIHACK_ROUTING_TEXT";
const PREFERENCES_VAR: &str = "AMPLIHACK_PROMPT_PREFERENCES";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hook {
    SessionStart,
    Prompt,
    Routing,
}

const HOOKS: [Hook; 3] = [Hook::SessionStart, Hook::Prompt, Hook::Routing];

impl Hook {
    fn sub(self) -> &'static str {
        match self {
            Hook::SessionStart => "session-start",
            Hook::Prompt => "user-prompt-submit",
            Hook::Routing => "workflow-classification-reminder",
        }
    }

    fn payload(self) -> String {
        match self {
            Hook::SessionStart => {
                json!({"hook_event_name":"SessionStart","session_id":"s1","source":"startup"})
            }
            Hook::Prompt | Hook::Routing => json!({"hook_event_name":"UserPromptSubmit",
                "session_id":"s1","prompt":"Fix the failing login test"}),
        }
        .to_string()
    }
}

/// What a test puts in the sandbox before the hook runs.
#[derive(Default)]
struct Setup<'a> {
    user_file: Option<&'a str>,
    /// A directory where the user file should be, so it cannot be read.
    user_file_is_dir: bool,
    project_file: Option<&'a str>,
    env: &'a [(&'a str, &'a str)],
    log: Option<&'a str>,
}

struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
    sandbox: HookSandbox,
}

impl Run {
    fn json(&self) -> Value {
        serde_json::from_str(&self.stdout)
            .unwrap_or_else(|e| panic!("stdout is one JSON value ({e}): {:?}", self.stdout))
    }

    fn context(&self) -> String {
        let output = self.json();
        [
            output.get("additionalContext"),
            output.pointer("/hookSpecificOutput/additionalContext"),
        ]
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join("\n")
    }

    fn has_context_field(&self) -> bool {
        let output = self.json();
        output.get("additionalContext").is_some()
            || output
                .pointer("/hookSpecificOutput/additionalContext")
                .is_some()
    }

    fn stderr_lines_with(&self, needle: &str) -> usize {
        self.stderr.lines().filter(|l| l.contains(needle)).count()
    }

    /// The rule the routing hook recorded for its last prompt.
    fn recorded_rule(&self) -> Option<String> {
        find_file(self.sandbox.root(), "classification_state.json")
            .and_then(|path| fs::read_to_string(path).ok())
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|state| state["last_rule"].as_str().map(str::to_string))
    }
}

fn find_file(dir: &Path, name: &str) -> Option<std::path::PathBuf> {
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_file(&path, name) {
                return Some(found);
            }
        } else if path.file_name().is_some_and(|n| n == name) {
            return Some(path);
        }
    }
    None
}

fn write(path: impl AsRef<Path>, contents: &str) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn run(hook: Hook, setup: &Setup<'_>) -> Run {
    let sandbox = HookSandbox::new();
    // A preferences file the prompt-event hook turns into its table.
    write(
        sandbox.project().join("USER_PREFERENCES.md"),
        "| Setting | Value |\n|---|---|\n| Verbosity | balanced |\n",
    );
    let user_file = sandbox.home().join(".amplihack/config.toml");
    if setup.user_file_is_dir {
        fs::create_dir_all(&user_file).unwrap();
    }
    if let Some(text) = setup.user_file {
        write(&user_file, text);
    }
    if let Some(text) = setup.project_file {
        write(
            sandbox.project().join(".git/HEAD"),
            "ref: refs/heads/main\n",
        );
        write(sandbox.project().join(".amplihack/config.toml"), text);
    }
    let mut command = sandbox.command(BIN);
    command
        .arg(hook.sub())
        // No background code-graph indexing from session-start.
        .env("AMPLIHACK_DISABLE_BLARIFY", "1")
        .env("CODEX_HOME", sandbox.home().join(".codex"))
        .env("AMPLIHACK_AGENT_BINARY", "claude")
        .env_remove("RUST_LOG")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, value) in setup.env {
        command.env(name, value);
    }
    if let Some(filter) = setup.log {
        command.env("RUST_LOG", filter);
    }
    let mut child = command.spawn().expect("start amplihack-hooks");
    {
        let mut stdin = child.stdin.take().unwrap();
        let _ = stdin.write_all(hook.payload().as_bytes());
    }
    let output = child.wait_with_output().unwrap();
    let mut stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    for root in [
        sandbox.root().canonicalize().ok(),
        Some(sandbox.root().to_path_buf()),
    ]
    .into_iter()
    .flatten()
    {
        stdout = stdout.replace(&*root.to_string_lossy(), "<SANDBOX>");
    }
    Run {
        code: output.status.code(),
        stdout,
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        sandbox,
    }
}

fn nothing() -> Setup<'static> {
    Setup::default()
}

const ROUTING_OFF: &str = "[text]\nrouting = \"off\"\n";
const PREFERENCES_OFF: &str = "[text]\nprompt_preferences = \"off\"\n";

// ---- Nothing set: today's output ----

#[test]
fn default_output_is_unchanged_in_four_file_states() {
    let other_keys = "# no text keys\n[sessions]\narchive_days = 7\n";
    for hook in HOOKS {
        let baseline = run(hook, &nothing());
        assert_eq!(baseline.code, Some(0), "{hook:?}");
        assert!(
            !baseline.context().is_empty(),
            "fixture check: {hook:?} gives context: {}",
            baseline.stdout
        );
        let states = [
            (
                "files without the keys",
                Setup {
                    user_file: Some(other_keys),
                    project_file: Some(other_keys),
                    log: Some("warn"),
                    ..nothing()
                },
            ),
            (
                "a user file that does not parse",
                Setup {
                    user_file: Some("this is = = not [toml\n"),
                    log: Some("warn"),
                    ..nothing()
                },
            ),
            (
                "a user file that cannot be read",
                Setup {
                    user_file_is_dir: true,
                    log: Some("warn"),
                    ..nothing()
                },
            ),
        ];
        for (label, setup) in states {
            let other = run(hook, &setup);
            assert_eq!(other.code, Some(0), "{hook:?} with {label}");
            assert_eq!(other.stdout, baseline.stdout, "{hook:?} with {label}");
            let expected_log_lines = usize::from(label != "files without the keys");
            assert_eq!(
                other.stderr_lines_with(".amplihack/config.toml"),
                expected_log_lines,
                "{hook:?} with {label}: {}",
                other.stderr
            );
        }
    }
}

// ---- text.routing ----

#[test]
fn routing_off_removes_the_reminder_and_the_session_start_section_only() {
    let off = Setup {
        user_file: Some(ROUTING_OFF),
        ..nothing()
    };

    let reminder = run(Hook::Routing, &off);
    assert_eq!(reminder.code, Some(0));
    assert_eq!(reminder.json(), json!({}));
    assert_eq!(
        reminder.recorded_rule().as_deref(),
        Some("routing_text_off")
    );

    let start_on = run(Hook::SessionStart, &nothing());
    let start_off = run(Hook::SessionStart, &off);
    assert!(
        start_on
            .context()
            .contains(DEFAULT_WORKFLOW_SESSION_CONTEXT)
    );
    assert!(
        !start_off
            .context()
            .contains(DEFAULT_WORKFLOW_SESSION_CONTEXT)
    );
    assert!(
        !start_off.context().is_empty(),
        "the preferences text stays"
    );

    // The prompt-event hook does not insert routing text.
    assert_eq!(
        run(Hook::Prompt, &off).stdout,
        run(Hook::Prompt, &nothing()).stdout
    );
}

#[test]
fn a_nested_recipe_session_keeps_its_text_with_routing_off() {
    let depth = [("AMPLIHACK_SESSION_DEPTH", "1")];
    let nested_on = run(
        Hook::SessionStart,
        &Setup {
            env: &depth,
            ..nothing()
        },
    );
    let nested_off = run(
        Hook::SessionStart,
        &Setup {
            env: &depth,
            user_file: Some(ROUTING_OFF),
            ..nothing()
        },
    );
    assert_eq!(nested_off.stdout, nested_on.stdout);
}

#[test]
fn routing_off_is_logged_with_its_source() {
    let reminder = run(
        Hook::Routing,
        &Setup {
            user_file: Some(ROUTING_OFF),
            log: Some("info"),
            ..nothing()
        },
    );
    let lines: Vec<&str> = reminder
        .stderr
        .lines()
        .filter(|l| l.contains("text.routing"))
        .collect();
    assert_eq!(lines.len(), 1, "{}", reminder.stderr);
    assert!(lines[0].contains("\"user\""), "{}", lines[0]);
}

// ---- text.prompt_preferences ----

#[test]
fn prompt_preferences_off_removes_the_table_on_prompt_events_only() {
    let off = Setup {
        user_file: Some(PREFERENCES_OFF),
        ..nothing()
    };
    let prompt_on = run(Hook::Prompt, &nothing());
    let prompt_off = run(Hook::Prompt, &off);
    assert!(prompt_on.has_context_field(), "{}", prompt_on.stdout);
    assert_eq!(prompt_off.code, Some(0));
    assert!(!prompt_off.has_context_field(), "{}", prompt_off.stdout);

    for hook in [Hook::SessionStart, Hook::Routing] {
        assert_eq!(
            run(hook, &off).stdout,
            run(hook, &nothing()).stdout,
            "{hook:?}"
        );
    }
}

#[test]
fn a_recipe_step_gets_no_table_whatever_the_setting() {
    let without_table = run(
        Hook::Prompt,
        &Setup {
            user_file: Some(PREFERENCES_OFF),
            ..nothing()
        },
    );
    for var in ["AMPLIHACK_RECIPE_RUN_ID", "AMPLIHACK_STEP_ID"] {
        for setting in ["on", "off"] {
            let env = [(var, "run-1"), (PREFERENCES_VAR, setting)];
            let step = run(
                Hook::Prompt,
                &Setup {
                    env: &env,
                    ..nothing()
                },
            );
            assert_eq!(step.stdout, without_table.stdout, "{var} {setting}");
        }
    }
}

// ---- Precedence ----

#[test]
fn the_variable_wins_over_the_files() {
    for (hook, var, file_off) in [
        (Hook::Routing, ROUTING_VAR, ROUTING_OFF),
        (Hook::Prompt, PREFERENCES_VAR, PREFERENCES_OFF),
    ] {
        let on = run(hook, &nothing());
        let off = run(
            hook,
            &Setup {
                user_file: Some(file_off),
                ..nothing()
            },
        );
        assert_ne!(on.stdout, off.stdout, "fixture check: {hook:?}");

        let env_on = [(var, "on")];
        let file_off_env_on = run(
            hook,
            &Setup {
                user_file: Some(file_off),
                project_file: Some(file_off),
                env: &env_on,
                ..nothing()
            },
        );
        assert_eq!(file_off_env_on.stdout, on.stdout, "{hook:?}");

        let env_off = [(var, "off")];
        assert_eq!(
            run(
                hook,
                &Setup {
                    env: &env_off,
                    ..nothing()
                }
            )
            .stdout,
            off.stdout,
            "{hook:?}"
        );
    }
}

#[test]
fn a_project_file_is_honoured_and_wins_over_the_user_file() {
    let off = run(
        Hook::Routing,
        &Setup {
            user_file: Some(ROUTING_OFF),
            ..nothing()
        },
    );
    let project_off = run(
        Hook::Routing,
        &Setup {
            project_file: Some(ROUTING_OFF),
            ..nothing()
        },
    );
    assert_eq!(project_off.stdout, off.stdout);

    let project_on = run(
        Hook::Routing,
        &Setup {
            user_file: Some(ROUTING_OFF),
            project_file: Some("[text]\nrouting = \"on\"\n"),
            ..nothing()
        },
    );
    assert_eq!(project_on.stdout, run(Hook::Routing, &nothing()).stdout);
}

// ---- Unknown values ----

#[test]
fn an_unknown_value_keeps_the_default_and_is_reported_once_on_stderr() {
    for (hook, var) in [
        (Hook::Routing, ROUTING_VAR),
        (Hook::SessionStart, ROUTING_VAR),
        (Hook::Prompt, PREFERENCES_VAR),
    ] {
        let baseline = run(hook, &nothing());
        let env = [(var, "bogus-value")];
        let bogus = run(
            hook,
            &Setup {
                env: &env,
                ..nothing()
            },
        );
        assert_eq!(bogus.code, Some(0), "{hook:?}");
        assert_eq!(bogus.stdout, baseline.stdout, "{hook:?}");
        assert_eq!(
            bogus.stderr_lines_with(var),
            1,
            "{hook:?}: {}",
            bogus.stderr
        );
        assert!(!bogus.stdout.contains("bogus-value"), "{hook:?}");
    }
}

#[test]
fn an_unknown_value_in_a_file_cannot_split_the_report_or_reach_a_terminal_raw() {
    // A TOML string with a line break and an escape sequence in it.
    let file = "[text]\nrouting = \"x\\u001b[31m\\nsecond line\"\n";
    let baseline = run(Hook::Routing, &nothing());
    let bogus = run(
        Hook::Routing,
        &Setup {
            user_file: Some(file),
            ..nothing()
        },
    );
    assert_eq!(bogus.code, Some(0));
    assert_eq!(bogus.stdout, baseline.stdout);
    assert_eq!(
        bogus.stderr_lines_with("text.routing"),
        1,
        "{:?}",
        bogus.stderr
    );
    assert_eq!(
        bogus.stderr_lines_with("second line"),
        1,
        "{:?}",
        bogus.stderr
    );
    assert!(
        !bogus.stderr.lines().any(|l| l.starts_with("second line")),
        "{:?}",
        bogus.stderr
    );
    assert!(!bogus.stderr.contains('\u{1b}'), "{:?}", bogus.stderr);
}

// ---- AMPLIHACK_HOOK_TEXT=none keeps precedence ----

#[test]
fn hook_text_none_removes_context_whatever_the_settings() {
    for routing in ["on", "off"] {
        for preferences in ["on", "off"] {
            let env = [
                ("AMPLIHACK_HOOK_TEXT", "none"),
                (ROUTING_VAR, routing),
                (PREFERENCES_VAR, preferences),
            ];
            for hook in HOOKS {
                let none = run(
                    hook,
                    &Setup {
                        env: &env,
                        ..nothing()
                    },
                );
                assert_eq!(none.code, Some(0), "{hook:?}");
                assert!(
                    !none.has_context_field(),
                    "{hook:?} routing {routing} preferences {preferences}: {}",
                    none.stdout
                );
            }
        }
    }
}
