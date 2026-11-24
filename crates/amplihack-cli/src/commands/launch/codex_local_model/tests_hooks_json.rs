//! The generated `hooks.json` (D16, SF3, SEC-L5, SEC-L7).
//!
//! `hooks_json.rs` is pure apart from `verify_generated`, which checks the
//! hooks binary on disk. The contract held here:
//!
//! - `pub(super) fn user_pre_tool_use(text: &str, path: &Path, plugin_root:
//!   &Path) -> anyhow::Result<Vec<serde_json::Value>>`: the user's
//!   PreToolUse groups with amplihack's own pre-tool-use handlers removed;
//! - `pub(super) fn runs_amplihack_pre_tool_use(command: &str, plugin_root:
//!   &Path) -> bool`;
//! - `pub(super) fn render(user_groups: &[Value], hooks_bin: &Path) ->
//!   String`;
//! - `pub(super) fn verify_generated(text: &str) ->
//!   anyhow::Result<Vec<Value>>`: the user groups after amplihack's, when
//!   amplihack's entry is exactly right;
//! - `pub(super) fn handler_count(groups: &[Value]) -> usize`.
//!
//! `plugin_root` is `<home>/.amplihack/codex`.

use super::hooks_json::{
    handler_count, render, runs_amplihack_pre_tool_use, user_pre_tool_use, verify_generated,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const ROOT: &str = "/home/u/.amplihack/codex";
const USER_HOOKS: &str = "/home/u/.codex/hooks.json";

fn root() -> &'static Path {
    Path::new(ROOT)
}

fn wrapper() -> String {
    format!("'{ROOT}/market/plugin/bin/hook'")
}

fn guard(script: &str) -> Value {
    json!({"hooks": [{"type": "command",
        "command": format!("/usr/bin/python3 -I /home/u/.codex/hooks/my-guard.py {script}")}]})
}

fn user_file(pre_tool_use: Value) -> String {
    json!({"hooks": {
        "SessionStart": [{"hooks": [{"type": "command", "command": format!("{} session-start", wrapper())}]}],
        "PreToolUse": pre_tool_use,
        "Stop": [{"hooks": [{"type": "command", "command": format!("{} stop", wrapper())}]}],
    }})
    .to_string()
}

fn filter(pre_tool_use: Value) -> Vec<Value> {
    user_pre_tool_use(&user_file(pre_tool_use), Path::new(USER_HOOKS), root())
        .expect("a well-formed hooks.json")
}

// ---- which handlers are amplihack's ----

#[test]
fn amplihack_pre_tool_use_commands_are_recognised() {
    for command in [
        "amplihack-hooks pre-tool-use".to_string(),
        "/usr/local/bin/amplihack-hooks pre-tool-use".to_string(),
        "'/opt/tools/amplihack-hooks' pre-tool-use".to_string(),
        "env AMPLIHACK_HOOK_PROTOCOL=codex '/x/amplihack-hooks' pre-tool-use".to_string(),
        "env A=1 B=2 amplihack-hooks pre-tool-use".to_string(),
        format!("{} pre-tool-use", wrapper()),
        format!("{ROOT}/market/plugin/bin/hook pre-tool-use"),
        format!("env XPIA_ENABLED=false {} pre-tool-use", wrapper()),
        format!("env XPIA_ENABLED=true {} pre-tool-use", wrapper()),
        format!("'{ROOT}/other-market/plugin-2/bin/hook' pre-tool-use"),
    ] {
        assert!(runs_amplihack_pre_tool_use(&command, root()), "{command}");
    }
}

#[test]
fn other_commands_are_not_amplihack_pre_tool_use() {
    for command in [
        "".to_string(),
        "env".to_string(),
        "amplihack-hooks".to_string(),
        "amplihack-hooks post-tool-use".to_string(),
        "amplihack-hooks pre-tool-use --extra".to_string(),
        "amplihack-hooks pre-tool-use && rm -rf /tmp/x".to_string(),
        "amplihack-hooks pre-tool-use; echo hi".to_string(),
        "amplihack-hooks pre-tool-use | tee /tmp/log".to_string(),
        "sh -c 'amplihack-hooks pre-tool-use'".to_string(),
        "env env amplihack-hooks pre-tool-use".to_string(),
        "amplihack-hooks-v2 pre-tool-use".to_string(),
        "/opt/amplihack-hooks.d/run pre-tool-use".to_string(),
        "'amplihack-hooks pre-tool-use".to_string(),
        "relative/bin/hook pre-tool-use".to_string(),
        format!("'{ROOT}/market/plugin/bin/hook-wrapper' pre-tool-use"),
        format!("'{ROOT}/market/plugin/bin/hook' session-start"),
        // A generated home is never the plugin root, though it shares a
        // prefix with it as text.
        "'/home/u/.amplihack/codex-local/inkling/bin/hook' pre-tool-use".to_string(),
        "'/home/u/.amplihack/codexbin/hook' pre-tool-use".to_string(),
        "/usr/bin/python3 -I /home/u/.codex/hooks/my-guard.py shell-check".to_string(),
    ] {
        assert!(
            !runs_amplihack_pre_tool_use(&command, root()),
            "{command:?}"
        );
    }
}

/// S4: shell-words splits on whitespace only, so a word may hold an
/// operator, an expansion or a glob that the shell acts on. Any such word
/// makes the handler the user's, and it is kept.
#[test]
fn a_command_with_a_shell_special_word_is_not_amplihack_pre_tool_use() {
    for command in [
        "env A=1;/usr/local/bin/my-guard amplihack-hooks pre-tool-use".to_string(),
        "/x;y/amplihack-hooks pre-tool-use".to_string(),
        "/x&&y/amplihack-hooks pre-tool-use".to_string(),
        "true&&/x/amplihack-hooks pre-tool-use".to_string(),
        "/x|tee/amplihack-hooks pre-tool-use".to_string(),
        "env A=1>/tmp/out amplihack-hooks pre-tool-use".to_string(),
        "env A=$(my-guard) amplihack-hooks pre-tool-use".to_string(),
        "env A=`my-guard` amplihack-hooks pre-tool-use".to_string(),
        "/opt/*/amplihack-hooks pre-tool-use".to_string(),
        "~/bin/amplihack-hooks pre-tool-use".to_string(),
        // Quoted, so one word to shell-words, holding a newline or a space.
        "'/x\n/usr/local/bin/my-guard\n/amplihack-hooks' pre-tool-use".to_string(),
        "'/opt/my tools/amplihack-hooks' pre-tool-use".to_string(),
        "env A='1;my-guard' amplihack-hooks pre-tool-use".to_string(),
    ] {
        assert!(
            !runs_amplihack_pre_tool_use(&command, root()),
            "{command:?}"
        );
    }
}

/// Regression for S4: the wrapper as `amplihack install` writes it
/// (`codex_plugin/hooks.rs`, `declarations`: the single-quoted
/// `<package>/bin/hook` and the subcommand), and amplihack's own entry as a
/// generated home holds it, are still recognised and removed, for home paths
/// with the usual punctuation.
#[test]
fn the_wrapper_as_install_writes_it_is_still_removed() {
    for home in ["/home/u", "/home/first.last-2_x", "/Users/a+b@corp"] {
        let plugin_root = format!("{home}/.amplihack/codex");
        let installed = format!("'{plugin_root}/amplihack-local/amplihack/bin/hook' pre-tool-use");
        let generated = format!(
            "env AMPLIHACK_HOOK_PROTOCOL=codex '{home}/.local/bin/amplihack-hooks' pre-tool-use"
        );
        for command in [&installed, &generated] {
            assert!(
                runs_amplihack_pre_tool_use(command, Path::new(&plugin_root)),
                "{command:?}"
            );
        }
        let text = json!({"hooks": {"PreToolUse": [
            {"hooks": [{"type": "command", "command": installed}]},
            {"hooks": [{"type": "command", "command": generated}]},
            guard("shell-check"),
        ]}})
        .to_string();
        let groups =
            user_pre_tool_use(&text, Path::new(USER_HOOKS), Path::new(&plugin_root)).unwrap();
        assert_eq!(groups, vec![guard("shell-check")], "{home}");
    }
}

// ---- the user's groups ----

/// candide's file: the wrapper twice (one with an XPIA prefix) and two
/// guards. The wrappers go; the guards stay, unchanged and in order.
#[test]
fn both_wrapper_forms_are_removed_and_the_guards_kept_in_order() {
    let mut tmux = guard("tmux-check");
    tmux["matcher"] = json!("Bash");
    tmux["hooks"][0]["timeout"] = json!(10);
    let worker = guard("shell-check");
    let groups = filter(json!([
        {"hooks": [{"type": "command", "command": format!("env XPIA_ENABLED=false {} pre-tool-use", wrapper())}]},
        {"hooks": [{"type": "command", "command": format!("{} pre-tool-use", wrapper())}]},
        tmux,
        worker,
    ]));
    assert_eq!(groups, vec![tmux, worker]);
}

#[test]
fn a_group_keeps_its_other_handlers() {
    let groups = filter(json!([
        {"matcher": "Bash", "hooks": [
            {"type": "command", "command": "amplihack-hooks pre-tool-use"},
            {"type": "command", "command": "echo mine"},
        ]},
    ]));
    assert_eq!(
        groups,
        vec![json!({"matcher": "Bash", "hooks": [{"type": "command", "command": "echo mine"}]})]
    );
}

#[test]
fn compound_and_non_command_handlers_are_kept_in_order() {
    let kept = json!([
        {"hooks": [{"type": "command", "command": "amplihack-hooks pre-tool-use && echo after"}]},
        {"hooks": ["just a string", 7, null]},
        {"hooks": [{"type": "prompt", "prompt": "amplihack-hooks pre-tool-use"}]},
        {"hooks": [{"command": "amplihack-hooks pre-tool-use"}]},
        {"hooks": [{"type": "command", "command": "'unbalanced pre-tool-use"}]},
    ]);
    let groups = filter(kept.clone());
    // A handler without `type: command` is not a command, whatever its text.
    assert_eq!(Value::Array(groups), kept);
}

#[test]
fn a_group_left_empty_is_dropped() {
    let groups = filter(json!([
        {"hooks": [{"type": "command", "command": "amplihack-hooks pre-tool-use"}]},
        guard("shell-check"),
        {"matcher": "Bash", "hooks": [{"type": "command", "command": format!("{} pre-tool-use", wrapper())}]},
    ]));
    assert_eq!(groups, vec![guard("shell-check")]);
}

#[test]
fn no_pre_tool_use_means_no_user_groups() {
    for text in [
        r#"{"hooks": {}}"#.to_string(),
        json!({"hooks": {"Stop": [guard("x.py")]}}).to_string(),
    ] {
        let groups = user_pre_tool_use(&text, Path::new(USER_HOOKS), root()).unwrap();
        assert!(groups.is_empty(), "{text}");
    }
}

#[test]
fn malformed_files_are_errors_naming_the_file_and_quoting_nothing() {
    let secret = "sk-SENTINEL-hooks-json-tests-42";
    for text in [
        format!("{{\"hooks\": {{\"PreToolUse\": [\"{secret}\""),
        format!("[\"{secret}\"]"),
        format!("{{\"hooks\": [\"{secret}\"]}}"),
        format!("{{\"hooks\": {{\"PreToolUse\": \"{secret}\"}}}}"),
        format!("{{\"hooks\": {{\"PreToolUse\": {{\"x\": \"{secret}\"}}}}}}"),
        "not json at all".to_string(),
    ] {
        let err = user_pre_tool_use(&text, Path::new(USER_HOOKS), root()).expect_err(&text);
        let message = format!("{err:#}");
        assert!(message.contains(USER_HOOKS), "{message}");
        assert!(!message.contains(secret), "content in the error: {message}");
        assert!(!format!("{err:?}").contains(secret));
    }
}

#[test]
fn the_handler_count_counts_handlers_not_groups() {
    assert_eq!(handler_count(&[]), 0);
    let groups = vec![
        guard("a.py"),
        json!({"hooks": [{"type": "command", "command": "a"}, {"type": "command", "command": "b"}]}),
    ];
    assert_eq!(handler_count(&groups), 3);
}

// ---- the rendered file ----

fn executable(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    crate::test_support::write_executable_with_mode(&path, "#!/bin/sh\necho '{}'\n", 0o700);
    path
}

#[test]
fn render_puts_amplihack_first_and_holds_pre_tool_use_only() {
    let bin = Path::new("/home/u/.local/bin/amplihack-hooks");
    let user = vec![guard("a.py"), guard("b.py")];
    let text = render(&user, bin);
    assert!(text.ends_with('\n'), "a trailing newline");
    let value: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value.as_object().unwrap().len(), 1, "{value:#}");
    assert_eq!(value["hooks"].as_object().unwrap().len(), 1, "{value:#}");
    let groups = value["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(groups.len(), 3);
    assert_eq!(&groups[1..], user.as_slice());
    assert_eq!(
        groups[0],
        json!({"hooks": [{"type": "command",
            "command": "env AMPLIHACK_HOOK_PROTOCOL=codex '/home/u/.local/bin/amplihack-hooks' pre-tool-use"}]})
    );
}

/// The binary is quoted as `codex_plugin/hooks.rs` quotes the plugin
/// wrapper: single quotes, with `'` written `'\''`.
#[test]
fn render_quotes_the_binary_like_the_plugin() {
    let text = render(&[], Path::new("/opt/it's/amplihack-hooks"));
    let value: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        value["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
        "env AMPLIHACK_HOOK_PROTOCOL=codex '/opt/it'\\''s/amplihack-hooks' pre-tool-use"
    );
}

/// SEC-L7: whatever the path holds, the shell runs exactly that file with
/// exactly `pre-tool-use` and the Codex protocol, and nothing else.
#[test]
fn the_generated_command_survives_the_shell() {
    let dir = crate::test_support::tempdir_outside_framework_roots();
    let witness = dir.path().join("witness");
    for name in [
        "plain",
        "with space",
        "it's",
        "dq\"x",
        "$(id)",
        "back`tick`",
        "semi;colon",
        "amp&er",
        "star*",
        "back\\slash",
        "nl\nline",
    ] {
        let sub = dir.path().join(name);
        std::fs::create_dir(&sub).unwrap();
        let bin = sub.join("amplihack-hooks");
        crate::test_support::write_executable_with_mode(
            &bin,
            &format!(
                "#!/bin/sh\nprintf '%s|%s|%s\\n' \"$AMPLIHACK_HOOK_PROTOCOL\" \"$#\" \"$1\" > {}\n",
                crate::test_support::sh_quote(witness.to_str().unwrap())
            ),
            0o700,
        );
        let text = render(&[], &bin);
        let value: Value = serde_json::from_str(&text).unwrap();
        let command = value["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        let _ = std::fs::remove_file(&witness);
        let status = std::process::Command::new("/bin/sh")
            .args(["-c", command])
            .env_remove("AMPLIHACK_HOOK_PROTOCOL")
            .env("PATH", "/usr/bin:/bin")
            .current_dir(dir.path())
            .status()
            .unwrap();
        assert!(status.success(), "{name:?}: {command}");
        assert_eq!(
            std::fs::read_to_string(&witness).unwrap(),
            "codex|1|pre-tool-use\n",
            "{name:?}: {command}"
        );
    }
}

// ---- verification of a generated file (SF3) ----

#[test]
fn verify_accepts_what_render_writes() {
    let dir = crate::test_support::tempdir_outside_framework_roots();
    let bin = executable(dir.path(), "amplihack-hooks");
    let user = vec![guard("a.py"), guard("b.py")];
    let groups = verify_generated(&render(&user, &bin)).expect("verified");
    assert_eq!(groups, user);
    assert_eq!(handler_count(&groups), 2);
}

#[test]
fn verify_refuses_a_weakened_entry() {
    let dir = crate::test_support::tempdir_outside_framework_roots();
    let bin = executable(dir.path(), "amplihack-hooks");
    let not_executable = dir.path().join("plain-file");
    std::fs::write(&not_executable, "#!/bin/sh\n").unwrap();
    let good: Value = serde_json::from_str(&render(&[guard("a.py")], &bin)).unwrap();
    let command = |c: &str| json!({"hooks": [{"type": "command", "command": c}]});
    let quoted = format!("'{}'", bin.display());

    let mut cases: Vec<(&str, Value)> = Vec::new();
    let mut v = good.clone();
    v["hooks"]["PreToolUse"].as_array_mut().unwrap().remove(0);
    cases.push(("entry removed", v));
    let mut v = good.clone();
    v["hooks"]["PreToolUse"][0]["matcher"] = json!("NoSuchTool");
    cases.push(("a matcher narrows the guard", v));
    let mut v = good.clone();
    v["hooks"]["PreToolUse"][0]["hooks"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type": "command", "command": "true"}));
    cases.push(("a second handler", v));
    let mut v = good.clone();
    v["hooks"]["PreToolUse"][0] = command(&format!(
        "env AMPLIHACK_HOOK_PROTOCOL=claude {quoted} pre-tool-use"
    ));
    cases.push(("another protocol", v));
    let mut v = good.clone();
    v["hooks"]["PreToolUse"][0] = command(&format!("{quoted} pre-tool-use"));
    cases.push(("no protocol", v));
    let mut v = good.clone();
    v["hooks"]["PreToolUse"][0] = command(&format!(
        "env AMPLIHACK_HOOK_PROTOCOL=codex {quoted} post-tool-use"
    ));
    cases.push(("another subcommand", v));
    let mut v = good.clone();
    v["hooks"]["PreToolUse"][0] = command(&format!(
        "env AMPLIHACK_HOOK_PROTOCOL=codex {quoted} pre-tool-use || true"
    ));
    cases.push(("extra words", v));
    let mut v = good.clone();
    v["hooks"]["PreToolUse"][0] =
        command("env AMPLIHACK_HOOK_PROTOCOL=codex amplihack-hooks pre-tool-use");
    cases.push(("a relative binary", v));
    let mut v = good.clone();
    v["hooks"]["PreToolUse"][0] = command(&format!(
        "env AMPLIHACK_HOOK_PROTOCOL=codex '{}' pre-tool-use",
        not_executable.display()
    ));
    cases.push(("a binary that is not executable", v));
    let mut v = good.clone();
    v["hooks"]["PreToolUse"][0] = command(&format!(
        "env AMPLIHACK_HOOK_PROTOCOL=codex '{}' pre-tool-use",
        dir.path().join("gone").display()
    ));
    cases.push(("a missing binary", v));
    let mut v = good.clone();
    v["hooks"]["PreToolUse"][0] =
        command("env AMPLIHACK_HOOK_PROTOCOL=codex /bin/true pre-tool-use");
    cases.push(("an executable that is not amplihack-hooks (S3)", v));
    let mut v = good.clone();
    v["hooks"]["PreToolUse"][0]["hooks"][0]["type"] = json!("prompt");
    cases.push(("not a command handler", v));
    let mut v = good.clone();
    v["hooks"]["PreToolUse"] = json!([]);
    cases.push(("no groups", v));

    for (what, value) in cases {
        assert!(
            verify_generated(&value.to_string()).is_err(),
            "{what}: {value:#}"
        );
    }
    for text in ["", "{", "[]", "{\"hooks\": {}}"] {
        assert!(verify_generated(text).is_err(), "{text:?}");
    }
}
