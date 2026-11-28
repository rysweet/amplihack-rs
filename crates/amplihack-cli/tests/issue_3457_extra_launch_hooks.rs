//! Exercise extra launch hooks through the pinned runner and real launcher.

#![cfg(unix)]

use crate::support::amplihack_bin::amplihack_bin;
use crate::support::exec_stub::write_executable;
use crate::support::recipe_guide::{GuideFixture, private_file, show, stderr};
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SETTING: &str = "AMPLIHACK_EXTRA_HOOKS_FILE";
const START: &str = "printf '%s\\n' start >> \"$HOOK_RECORD\"";
const POST: &str = "printf '%s\\n' post >> \"$HOOK_RECORD\"";

const PARENT: &str = r#"name: "extra-hooks-parent"
description: "Run a direct and a nested agent step"
version: "1.0.0"
steps:
  - id: direct
    type: agent
    prompt: "direct-step-3457"
  - id: nested
    type: recipe
    recipe: "extra-hooks-child"
"#;

const CHILD: &str = r#"name: "extra-hooks-child"
description: "Run a nested agent step"
version: "1.0.0"
steps:
  - id: child
    type: agent
    prompt: "nested-step-3457"
"#;

// Capture what Claude receives and execute the fixture's command hooks.
// Amplihack's own hooks are inspected separately against the unset launch.
const CLAUDE: &str = r#"#!/bin/sh
set -eu
if [ "$1" = "--version" ]; then
  printf '%s\n' '2.1.319 (Claude Code)'
  exit 0
fi
rec="$AMPLIHACK_3457_RECORD_DIR/$$"
mkdir "$rec"
cat > "$rec/prompt"
printf '%s\n' "$@" >> "$rec/prompt"
prev=
for arg do
  if [ "$prev" = "--settings" ]; then cp "$arg" "$rec/settings.json"; fi
  prev=$arg
done
export HOOK_RECORD="$rec/events"
jq -r '.hooks | (.SessionStart[], .PostToolUse[]) | .hooks[] |
  .command | select(startswith("printf "))' "$rec/settings.json" > "$rec/commands"
while IFS= read -r hook; do
  sh -c "$hook"
done < "$rec/commands"
printf '%s\n' done
"#;

const CODEX: &str = r#"#!/bin/sh
set -eu
case "$1" in
  --version) printf 'codex-cli %s\n' "$FIXTURE_CODEX_VERSION"; exit 0 ;;
  plugin) printf '%s\n' '{"installed":[]}'; exit 0 ;;
esac
if [ "$FIXTURE_CODEX_VERSION" = "0.98.0" ]; then
  for arg do
    if [ "$arg" = "--dangerously-bypass-hook-trust" ]; then exit 2; fi
  done
fi
rec="$AMPLIHACK_3457_RECORD_DIR/$$"
mkdir "$rec"
cat > "$rec/prompt"
printf '%s\n' "$AMPLIHACK_GUIDANCE_STEP_ID" > "$rec/step-id"
printf '%s\0' "$@" > "$rec/argv"
for arg do
  case "$arg" in
    hooks.*=*)
      # The fixture's marker commands have no TOML escapes. Rust separately
      # parses and checks the complete override that was sent to this binary.
      marker=$(printf '%s\n' "$arg" | sed -n 's/.*command = "\(touch codex-[a-z]*\)".*/\1/p')
      if [ -n "$marker" ]; then sh -c "$marker"; fi ;;
  esac
done
printf '%s\n' done
"#;

struct Fixture {
    guide: GuideFixture,
    source: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let guide = GuideFixture::new();
        write_executable(&guide.tools.join("claude"), CLAUDE, 0o700);
        let source = guide.root.join("extra-settings.json");
        Self { guide, source }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = self.guide.amplihack(args);
        command
            .env("AMPLIHACK_3457_RECORD_DIR", &self.guide.records)
            .env(
                "AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH",
                amplihack_bin().with_file_name("amplihack-hooks"),
            );
        command
    }

    fn launch(&self, source: &Path) -> Output {
        let mut command = self.command(&["claude", "-p", "direct"]);
        command.env(SETTING, source);
        self.guide.run_with_stdin(command, None)
    }

    fn captures(&self) -> Vec<PathBuf> {
        fs::read_dir(&self.guide.records)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.is_dir())
            .collect()
    }

    fn write(&self, settings: &Value) {
        private_file(&self.source, settings.to_string());
    }
}

fn groups(settings: &Value, event: &str) -> Vec<Value> {
    settings["hooks"][event]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn handler(command: &str) -> Value {
    json!({"type": "command", "command": command})
}

#[test]
fn direct_and_nested_recipe_steps_run_extra_hooks_after_amplihack_hooks() {
    let fixture = Fixture::new();
    let parent = fixture.guide.root.join("extra-hooks-parent.yaml");
    private_file(&parent, PARENT);
    private_file(
        &fixture
            .guide
            .home
            .join(".amplihack/.claude/recipes/extra-hooks-child.yaml"),
        CHILD,
    );
    let project = fixture.guide.root.join("project");
    crate::support::git_fixture::init_with_commit(&project);
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let runner = crate::pinned_runner::pinned_runner(manifest.parent().unwrap().parent().unwrap());

    let run = |source: Option<&Path>| {
        let mut command = fixture.command(&["recipe", "run"]);
        command
            .arg(&parent)
            .args(["--no-detach", "-w"])
            .arg(&project)
            .current_dir(&project)
            .env("RECIPE_RUNNER_RS_PATH", &runner)
            .env("AMPLIHACK_AGENT_BINARY", "claude")
            .env("AMPLIHACK_LAUNCHER_BINARY", amplihack_bin());
        if let Some(source) = source {
            command.env(SETTING, source);
        }
        let output = fixture.guide.run_with_stdin(command, None);
        assert!(output.status.success(), "{}", show(&output));
        let captures = fixture.captures();
        assert_eq!(captures.len(), 2, "{}", show(&output));
        captures
    };

    let baseline = run(None);
    let original = fs::read(baseline[0].join("settings.json")).unwrap();
    assert_eq!(
        fs::read(baseline[1].join("settings.json")).unwrap(),
        original
    );
    let own: Value = serde_json::from_slice(&original).unwrap();
    for capture in baseline {
        assert!(!capture.join("events").exists());
        fs::remove_dir_all(capture).unwrap();
    }

    let own_start = &own["hooks"]["SessionStart"][0]["hooks"][0];
    assert!(own_start["command"].is_string());
    assert!(!groups(&own, "PostToolUse").is_empty());
    let start = json!({"hooks": [handler(START)]});
    let post = json!({"matcher": "Bash", "hooks": [handler(POST)]});
    fixture.write(&json!({
        "hooks": {
            "SessionStart": [{"hooks": [own_start, handler(START), handler(START)]}],
            "PostToolUse": [post]
        },
        "env": {"UNUSED_FIXTURE_KEY": "ignored"}
    }));
    let mut expected = own;
    expected["hooks"]["SessionStart"]
        .as_array_mut()
        .unwrap()
        .push(start);
    expected["hooks"]["PostToolUse"]
        .as_array_mut()
        .unwrap()
        .push(post);
    let mut prompts = Vec::new();
    for capture in run(Some(&fixture.source)) {
        let actual: Value =
            serde_json::from_slice(&fs::read(capture.join("settings.json")).unwrap()).unwrap();
        assert_eq!(actual, expected);
        assert_eq!(
            fs::read_to_string(capture.join("events")).unwrap(),
            "start\npost\n"
        );
        prompts.push(fs::read_to_string(capture.join("prompt")).unwrap());
    }
    for prompt in ["direct-step-3457", "nested-step-3457"] {
        assert_eq!(
            prompts
                .iter()
                .filter(|input| input.contains(prompt))
                .count(),
            1
        );
    }
}

#[test]
fn invalid_files_stop_before_claude_starts() {
    let fixture = Fixture::new();
    let attempt = |source: &Path, categories: &[&str]| {
        let output = fixture.launch(source);
        assert!(!output.status.success(), "{}", show(&output));
        let diagnostic = stderr(&output);
        assert!(
            diagnostic.contains(source.to_str().unwrap()),
            "{}",
            show(&output)
        );
        assert!(
            categories
                .iter()
                .any(|word| diagnostic.to_ascii_lowercase().contains(word)),
            "{}",
            show(&output)
        );
        assert!(fixture.captures().is_empty());
    };

    attempt(&fixture.source, &["no such file", "not found"]);
    fixture.write(&json!({}));
    for (mode, categories) in [
        (0o000, &["readable", "permission"][..]),
        (0o622, &["writable"]),
    ] {
        fs::set_permissions(&fixture.source, fs::Permissions::from_mode(mode)).unwrap();
        attempt(&fixture.source, categories);
    }
    fs::remove_file(&fixture.source).unwrap();
    private_file(&fixture.source, b"{");
    attempt(&fixture.source, &["json"]);
    fixture.write(&json!({"hooks": {"SessionStart": [{"hooks": [handler("")]}]}}));
    attempt(&fixture.source, &["schema"]);
    for hooks in [
        json!({"UnknownEvent": [{"hooks": [handler(START)]}]}),
        json!({"SessionStart": [{"matcher": 5, "hooks": [handler(START)]}]}),
        json!({"SessionStart": [{"hooks": [{"type": "command", "command": START, "async": "yes"}]}]}),
    ] {
        fixture.write(&json!({"hooks": hooks}));
        attempt(&fixture.source, &["schema"]);
    }
    let link = fixture.guide.root.join("linked-settings.json");
    std::os::unix::fs::symlink(&fixture.source, &link).unwrap();
    attempt(&link, &["symlink"]);
}

#[test]
fn other_settings_keys_are_reported_together_once() {
    let fixture = Fixture::new();
    fixture.write(&json!({
        "hooks": {},
        "ignoredAlpha": "fixture",
        "ignoredBeta": {"fixture": "value"}
    }));
    let output = fixture.launch(&fixture.source);
    assert!(output.status.success(), "{}", show(&output));
    let diagnostic = stderr(&output);
    assert_eq!(diagnostic.matches("ignoredAlpha").count(), 1);
    assert_eq!(diagnostic.matches("ignoredBeta").count(), 1);
    assert!(
        diagnostic
            .lines()
            .any(|line| line.contains("ignoredAlpha") && line.contains("ignoredBeta"))
    );
    let captures = fixture.captures();
    assert_eq!(captures.len(), 1);
    let settings: Value =
        serde_json::from_slice(&fs::read(captures[0].join("settings.json")).unwrap()).unwrap();
    assert_eq!(settings.as_object().unwrap().len(), 1);
    assert!(settings["hooks"].is_object());
}

#[test]
fn extra_hooks_keep_explicit_claude_settings() {
    let fixture = Fixture::new();
    fixture.write(&json!({"hooks": {
        "SessionStart": [{"hooks": [handler(START)]}],
        "PostToolUse": [{"hooks": [handler(POST)]}]
    }}));
    let caller = json!({
        "env": {"CALLER_SETTING": "kept"},
        "hooks": {"SessionStart": [{"hooks": [handler(START)]}]}
    });
    private_file(&fixture.guide.root.join("caller.json"), caller.to_string());
    let inline = format!("--settings={caller}");
    for options in [&["--settings", "caller.json"][..], &[inline.as_str()][..]] {
        let mut command = fixture.command(&["claude", "-p", "direct"]);
        command
            .args(options)
            .current_dir(&fixture.guide.root)
            .env(SETTING, &fixture.source);
        let output = fixture.guide.run_with_stdin(command, None);
        assert!(output.status.success(), "{}", show(&output));
        let captures = fixture.captures();
        assert_eq!(captures.len(), 1);
        let capture = &captures[0];
        let contents: Value =
            serde_json::from_slice(&fs::read(capture.join("settings.json")).unwrap()).unwrap();
        assert_eq!(contents["env"], caller["env"]);
        for (event, command) in [("SessionStart", START), ("PostToolUse", POST)] {
            let groups = groups(&contents, event);
            assert!(
                groups[0]["hooks"][0]["command"]
                    .as_str()
                    .unwrap()
                    .contains("amplihack-hooks")
            );
            assert_eq!(groups.len(), 2);
            assert_eq!(groups[1]["hooks"][0]["command"], command);
        }
        assert_eq!(
            fs::read_to_string(capture.join("events")).unwrap(),
            "start\npost\n"
        );
        fs::remove_dir_all(capture).unwrap();
    }
}

#[test]
fn codex_dispatches_supported_hooks_without_changing_the_guidance_check() {
    let fixture = Fixture::new();
    write_executable(&fixture.guide.tools.join("codex"), CODEX, 0o700);
    let own_post = handler("bin/hook post-tool-use");
    let own_prompt = handler("bin/hook user-prompt");
    let own = json!({"hooks": {
        "PostToolUse": [{"hooks": [own_post]}],
        "UserPromptSubmit": [{"hooks": [own_prompt]}]
    }});
    let own_path = fixture.guide.codex_home.join("hooks.json");
    let recipe_log = fixture.guide.root.join("recipe.log");
    let launch = |run_id: &str, version: &str, options: &[&str]| {
        let guidance = fixture.guide.make_run(run_id, run_id).join("guidance");
        let mut args = vec!["codex", "--", "exec"];
        args.extend_from_slice(options);
        args.push("-");
        let mut command = fixture.command(&args);
        command
            .env("AMPLIHACK_AGENT_BINARY", "codex")
            .env("AMPLIHACK_RECIPE_RUN_ID", run_id)
            .env("AMPLIHACK_RUN_GUIDANCE_DIRS", &guidance)
            .env("AMPLIHACK_RECIPE_LOG", &recipe_log)
            .env("FIXTURE_CODEX_VERSION", version)
            .env(SETTING, &fixture.source);
        let output = fixture
            .guide
            .run_with_stdin(command, Some(b"test prompt\n"));
        assert!(output.status.success(), "{}", show(&output));
        let captures = fixture.captures();
        assert_eq!(captures.len(), 1, "{}", show(&output));
        let step = fs::read_to_string(captures[0].join("step-id"))
            .unwrap()
            .trim_end()
            .to_owned();
        let argv = fs::read_to_string(captures[0].join("argv")).unwrap();
        fs::remove_dir_all(&captures[0]).unwrap();
        let mut overrides = toml::Table::new();
        for word in argv.split('\0') {
            let arg = word
                .strip_prefix("--config=")
                .or_else(|| word.strip_prefix("-c"))
                .unwrap_or(word);
            if arg.starts_with("hooks=") {
                overrides = toml::from_str::<toml::Table>(arg).unwrap();
            } else if arg.starts_with("hooks.") {
                let mut incoming = toml::from_str::<toml::Table>(arg).unwrap();
                if let Some(table) = overrides.get_mut("hooks") {
                    table.as_table_mut().unwrap().extend(
                        incoming
                            .remove("hooks")
                            .unwrap()
                            .as_table()
                            .unwrap()
                            .clone(),
                    );
                } else {
                    overrides = incoming;
                }
            }
        }
        let records = fixture.guide.log(&guidance);
        (step, overrides, records)
    };

    let start_command = "touch codex-start";
    let post_command = "touch codex-post";
    let quoted = "printf '%s\\n' \"a line\"\ntrue";
    fixture.write(&json!({"hooks": {
        "SessionStart": [{"hooks": [handler(start_command), handler(start_command)]}],
        "PostToolUse": [{"matcher": "Bash", "hooks": [handler(post_command), own_post]}],
        "UserPromptSubmit": [{"hooks": [handler(quoted)]}],
        "Notification": [{"hooks": [handler("touch codex-notification")]}]
    }}));
    private_file(&own_path, own.to_string());
    let (step, overrides, records) = launch("codex-supported", "0.166.0", &[]);
    let hooks = &overrides["hooks"];
    assert_eq!(
        hooks["SessionStart"][0]["hooks"].as_array().unwrap().len(),
        1
    );
    assert_eq!(
        hooks["SessionStart"][0]["hooks"][0]["command"].as_str(),
        Some(start_command)
    );
    assert_eq!(hooks["PostToolUse"][0]["matcher"].as_str(), Some("Bash"));
    assert_eq!(
        hooks["PostToolUse"][0]["hooks"][0]["command"].as_str(),
        Some(post_command)
    );
    assert_eq!(
        hooks["PostToolUse"][0]["hooks"].as_array().unwrap().len(),
        1
    );
    assert_eq!(
        hooks["UserPromptSubmit"][0]["hooks"][0]["command"].as_str(),
        Some(quoted)
    );
    assert!(hooks.get("Notification").is_none());
    assert!(fixture.guide.root.join("codex-start").is_file());
    assert!(fixture.guide.root.join("codex-post").is_file());
    assert!(!fixture.guide.root.join("codex-notification").exists());
    assert_eq!(fs::read_to_string(&own_path).unwrap(), own.to_string());
    assert!(
        records
            .iter()
            .any(|r| r["event"] == "guidance_step_hooked" && r["step_id"] == step)
    );
    let log = fs::read_to_string(&recipe_log).unwrap();
    let notices: Vec<_> = log.lines().filter(|line| line.contains(&step)).collect();
    assert_eq!(notices.len(), 1);
    assert!(notices[0].contains("Notification"));
    assert!(!notices[0].contains("SessionStart"));

    let (_, overrides, _) = launch(
        "codex-caller-settings",
        "0.166.0",
        &[
            r#"--config=hooks={SessionStart=[{hooks=[{type="command",command="echo caller"},{type="command",command="touch codex-start"}]}]}"#,
            r#"-chooks.PostToolUse=[{hooks=[{type="command",command="echo post"}]}]"#,
        ],
    );
    let hooks = &overrides["hooks"];
    let start = hooks["SessionStart"].as_array().unwrap();
    assert_eq!(start.len(), 1);
    let caller = start[0]["hooks"].as_array().unwrap();
    assert_eq!(caller.len(), 2);
    assert_eq!(caller[0]["command"].as_str(), Some("echo caller"));
    assert_eq!(caller[1]["command"].as_str(), Some(start_command));
    let post = hooks["PostToolUse"].as_array().unwrap();
    assert_eq!(post.len(), 2);
    assert_eq!(post[0]["hooks"][0]["command"].as_str(), Some("echo post"));
    assert_eq!(post[1]["hooks"][0]["command"].as_str(), Some(post_command));

    // Extra hooks with the names recognized by the guidance check are not
    // evidence that amplihack's own hooks are present.
    fs::remove_file(&own_path).unwrap();
    fixture.write(&own);
    let (step, overrides, records) = launch("codex-extra-only", "0.147.0", &[]);
    assert_eq!(overrides["hooks"].as_table().unwrap().len(), 2);
    assert!(
        records
            .iter()
            .any(|r| r["event"] == "guidance_step_unhooked" && r["step_id"] == step)
    );
    assert!(!fs::read_to_string(&recipe_log).unwrap().contains(&step));

    let (step, overrides, _) = launch("codex-disabled", "0.166.0", &["--disable", "hooks"]);
    assert!(overrides.is_empty());
    let log = fs::read_to_string(&recipe_log).unwrap();
    let notices: Vec<_> = log.lines().filter(|line| line.contains(&step)).collect();
    assert_eq!(notices.len(), 1);
    assert!(notices[0].contains("PostToolUse") && notices[0].contains("UserPromptSubmit"));

    let (step, overrides, _) = launch("codex-legacy", "0.98.0", &[]);
    assert!(overrides.is_empty());
    let log = fs::read_to_string(&recipe_log).unwrap();
    let notices: Vec<_> = log.lines().filter(|line| line.contains(&step)).collect();
    assert_eq!(notices.len(), 1);
    assert!(notices[0].contains("UserPromptSubmit"));

    private_file(
        &fixture.guide.codex_home.join("config.toml"),
        "[features]\nhooks = false\n",
    );
    let (step, overrides, _) = launch("codex-enabled", "0.166.0", &["--enable", "hooks"]);
    assert_eq!(overrides["hooks"].as_table().unwrap().len(), 2);
    assert!(!fs::read_to_string(&recipe_log).unwrap().contains(&step));

    fixture.write(&json!({"hooks": {"SessionStart": "invalid"}}));
    let mut invalid = fixture.command(&["codex", "--", "exec", "--disable", "hooks", "-"]);
    invalid
        .env("AMPLIHACK_AGENT_BINARY", "codex")
        .env("FIXTURE_CODEX_VERSION", "0.166.0")
        .env(SETTING, &fixture.source);
    let output = fixture.guide.run_with_stdin(invalid, None);
    assert!(!output.status.success(), "{}", show(&output));
    assert!(stderr(&output).contains(&fixture.source.display().to_string()));
    assert!(fixture.captures().is_empty());
}
