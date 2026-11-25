//! The generated `hooks.json` of a local-model Codex home.
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
//! - `pub(super) fn verify_generated(text: &str, expected: &Path) ->
//!   anyhow::Result<(Vec<Value>, String)>`: the user groups after amplihack's
//!   and its actual trust hash, when the entry is right and runs `expected`;
//! - `pub(super) fn handler_count(groups: &[Value]) -> usize`.
//!
//! `plugin_root` is `<home>/.amplihack/codex`.

use super::hooks_json::{
    command_check_hash, handler_count, render, runs_amplihack_pre_tool_use, user_pre_tool_use,
    verify_generated,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const ROOT: &str = "/work/fixture/.amplihack/codex";
const USER_HOOKS: &str = "/work/fixture/.codex/hooks.json";

/// The expected value was checked against Codex 0.166.0's native hooks/list
/// response. This covers its normalisation and trust identity protocol.
#[test]
fn command_check_hash_matches_codex() {
    let binary = Path::new("/work/bin/amplihack-hooks");
    let expected_digest: [u8; 32] = [
        0x3a, 0x32, 0x22, 0x7c, 0x54, 0x00, 0x35, 0xce, 0x50, 0x57, 0xdb, 0x5d, 0xe1, 0x79, 0xfa,
        0xbc, 0xb2, 0x50, 0x89, 0x61, 0xc3, 0x12, 0x7c, 0x5b, 0xad, 0x82, 0x57, 0x21, 0xe1, 0x8d,
        0xed, 0x8b,
    ];
    let expected_hex: String = expected_digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(command_check_hash(binary), format!("sha256:{expected_hex}"));
    assert_ne!(
        command_check_hash(binary),
        command_check_hash(Path::new("/work/other/amplihack-hooks"))
    );
}

fn root() -> &'static Path {
    Path::new(ROOT)
}

fn wrapper() -> String {
    format!("'{ROOT}/market/plugin/bin/hook'")
}

fn guard(script: &str) -> Value {
    json!({"hooks": [{"type": "command",
        "command": format!("/bin/sh /work/fixture/.codex/hooks/my-check.sh {script}")}]})
}

fn user_file(pre_tool_use: Value) -> String {
    json!({"hooks": {
        "SessionStart": [{"hooks": [{"type": "command", "command": format!("{} session-start", wrapper())}]}],
        "PreToolUse": pre_tool_use,
        "Stop": [{"hooks": [{"type": "command", "command": format!("{} stop", wrapper())}]}],
    }})
    .to_string()
}

/// A hooks binary that does not exist: no program named `amplihack-hooks`
/// resolves to it, so the filter keeps every such handler.
const ABSENT_HOOKS_BIN: &str = "/work/absent/amplihack-hooks";

fn filter(pre_tool_use: Value) -> Vec<Value> {
    filter_with(pre_tool_use, Path::new(ABSENT_HOOKS_BIN))
}

fn filter_with(pre_tool_use: Value, hooks_bin: &Path) -> Vec<Value> {
    user_pre_tool_use(
        &user_file(pre_tool_use),
        Path::new(USER_HOOKS),
        root(),
        hooks_bin,
    )
    .expect("a well-formed hooks.json")
}

/// Two real executables named `amplihack-hooks` in different directories:
/// `ours`, the binary amplihack's entry runs, and `other`.
struct Bins {
    _dir: tempfile::TempDir,
    ours: PathBuf,
    other: PathBuf,
}

fn bins() -> Bins {
    let dir = crate::test_support::tempdir_outside_framework_roots();
    let mut paths = Vec::new();
    for sub in ["ours", "other"] {
        std::fs::create_dir(dir.path().join(sub)).unwrap();
        let path = dir.path().join(sub).join("amplihack-hooks");
        crate::test_support::write_executable_with_mode(&path, "#!/bin/sh\nexit 0\n", 0o700);
        paths.push(path);
    }
    let other = paths.pop().unwrap();
    let ours = paths.pop().unwrap();
    Bins {
        _dir: dir,
        ours,
        other,
    }
}

// ---- which handlers are amplihack's ----

#[test]
fn amplihack_pre_tool_use_commands_are_recognised() {
    for command in [
        "amplihack-hooks pre-tool-use".to_string(),
        "/usr/local/bin/amplihack-hooks pre-tool-use".to_string(),
        "'/opt/tools/amplihack-hooks' pre-tool-use".to_string(),
        "env AMPLIHACK_HOOK_PROTOCOL=codex '/x/amplihack-hooks' pre-tool-use".to_string(),
        "env amplihack-hooks pre-tool-use".to_string(),
        format!("{} pre-tool-use", wrapper()),
        format!("{ROOT}/market/plugin/bin/hook pre-tool-use"),
        format!(
            "env AMPLIHACK_HOOK_PROTOCOL=codex {} pre-tool-use",
            wrapper()
        ),
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
        "amplihack-hooks pre-tool-use && rm -rf /work/x".to_string(),
        "amplihack-hooks pre-tool-use; echo hi".to_string(),
        "amplihack-hooks pre-tool-use | tee /work/log".to_string(),
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
        "'/work/fixture/.amplihack/codex-local/inkling/bin/hook' pre-tool-use".to_string(),
        "'/work/fixture/.amplihack/codexbin/hook' pre-tool-use".to_string(),
        "/bin/sh /work/fixture/.codex/hooks/my-check.sh shell-check".to_string(),
    ] {
        assert!(
            !runs_amplihack_pre_tool_use(&command, root()),
            "{command:?}"
        );
    }
}

/// A handler that sets any variable but amplihack's protocol may check more
/// than amplihack's own entry (`XPIA_ENABLED=true` turns the XPIA check on),
/// so it is the user's and is kept, even when it runs amplihack's hook.
#[test]
fn a_handler_that_sets_another_variable_is_kept() {
    for command in [
        format!("env XPIA_ENABLED=true {} pre-tool-use", wrapper()),
        format!("env XPIA_ENABLED=false {} pre-tool-use", wrapper()),
        "env XPIA_ENABLED=true amplihack-hooks pre-tool-use".to_string(),
        "env A=1 B=2 amplihack-hooks pre-tool-use".to_string(),
        "env AMPLIHACK_HOOK_PROTOCOL=codex XPIA_ENABLED=true amplihack-hooks pre-tool-use"
            .to_string(),
        "env XPIA_ENABLED=true AMPLIHACK_HOOK_PROTOCOL=codex amplihack-hooks pre-tool-use"
            .to_string(),
        "env AMPLIHACK_HOOK_PROTOCOL=claude amplihack-hooks pre-tool-use".to_string(),
    ] {
        assert!(
            !runs_amplihack_pre_tool_use(&command, root()),
            "{command:?}"
        );
    }
    let xpia = json!({"hooks": [{"type": "command",
        "command": format!("env XPIA_ENABLED=true {} pre-tool-use", wrapper())}]});
    assert_eq!(filter(json!([xpia.clone()])), vec![xpia]);
}

/// Shell operators, expansions and globs are not evaluated by shell-words.
/// Any such character keeps the handler, even after quoting is removed.
#[test]
fn a_command_with_a_shell_special_word_is_not_amplihack_pre_tool_use() {
    for command in [
        "env A=1;/usr/local/bin/my-guard amplihack-hooks pre-tool-use".to_string(),
        "/x;y/amplihack-hooks pre-tool-use".to_string(),
        "/x&&y/amplihack-hooks pre-tool-use".to_string(),
        "true&&/x/amplihack-hooks pre-tool-use".to_string(),
        "/x|tee/amplihack-hooks pre-tool-use".to_string(),
        "env A=1>/work/out amplihack-hooks pre-tool-use".to_string(),
        "env A=$(my-guard) amplihack-hooks pre-tool-use".to_string(),
        "env A=`my-guard` amplihack-hooks pre-tool-use".to_string(),
        "/opt/*/amplihack-hooks pre-tool-use".to_string(),
        "~/bin/amplihack-hooks pre-tool-use".to_string(),
        "amplihack-hooks\npre-tool-use".to_string(),
        "env\nAMPLIHACK_HOOK_PROTOCOL=codex amplihack-hooks pre-tool-use".to_string(),
        "amplihack-hooks pre-tool-use\n".to_string(),
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

/// The wrapper as `amplihack install` writes it
/// (`codex_plugin/hooks.rs`, `declarations`: the single-quoted
/// `<package>/bin/hook` and the subcommand), and amplihack's own entry as a
/// generated home writes it (running the same hooks binary file), are still
/// recognised and removed, for home paths with the usual punctuation.
#[test]
fn the_wrapper_as_install_writes_it_is_still_removed() {
    let dir = crate::test_support::tempdir_outside_framework_roots();
    for name in ["fixture", "first.last-2_x", "a+b"] {
        let home = dir.path().join(name).display().to_string();
        let bin = dir.path().join(name).join(".local/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let hooks_bin = bin.join("amplihack-hooks");
        crate::test_support::write_executable_with_mode(&hooks_bin, "#!/bin/sh\nexit 0\n", 0o700);
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
        let groups = user_pre_tool_use(
            &text,
            Path::new(USER_HOOKS),
            Path::new(&plugin_root),
            &hooks_bin,
        )
        .unwrap();
        assert_eq!(groups, vec![guard("shell-check")], "{home}");
    }
}

// ---- the user's groups ----

/// the fixture's file: the wrapper twice (one with an `XPIA_ENABLED=false`
/// prefix) and two guards. The bare wrapper goes; the prefixed wrapper and
/// the guards stay, unchanged and in order.
#[test]
fn the_bare_wrapper_is_removed_and_the_rest_kept_in_order() {
    let mut tmux = guard("tmux-check");
    tmux["matcher"] = json!("Bash");
    tmux["hooks"][0]["timeout"] = json!(10);
    let worker = guard("shell-check");
    let xpia = json!({"hooks": [{"type": "command",
        "command": format!("env XPIA_ENABLED=false {} pre-tool-use", wrapper())}]});
    let groups = filter(json!([
        xpia,
        {"hooks": [{"type": "command", "command": format!("{} pre-tool-use", wrapper())}]},
        tmux,
        worker,
    ]));
    assert_eq!(groups, vec![xpia, tmux, worker]);
}

#[test]
fn a_group_keeps_its_other_handlers() {
    let bins = bins();
    let groups = filter_with(
        json!([
            {"matcher": "Bash", "hooks": [
                {"type": "command", "command": format!("{} pre-tool-use", bins.ours.display())},
                {"type": "command", "command": "echo mine"},
            ]},
        ]),
        &bins.ours,
    );
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

/// A raw newline separates shell commands, even though shell-words splits
/// it like a space. Keep the compound handler and run its user check.
#[test]
fn a_newline_handler_survives_filtering_and_executes_the_user_check() {
    let dir = crate::test_support::tempdir_outside_framework_roots();
    for (name, script) in [
        ("amplihack-hooks", "#!/bin/sh\nprintf 'hook:%s\\n' \"$#\"\n"),
        (
            "pre-tool-use",
            "#!/bin/sh\nprintf 'user-check\\n'\nexit 2\n",
        ),
    ] {
        crate::test_support::write_executable_with_mode(&dir.path().join(name), script, 0o700);
    }
    let original = json!({"matcher": "Bash", "hooks": [{
        "type": "command", "command": "amplihack-hooks\npre-tool-use",
    }]});
    let groups = filter(json!([original.clone()]));
    assert_eq!(groups, vec![original]);
    let generated: Value =
        serde_json::from_str(&render(&groups, &dir.path().join("amplihack-hooks"))).unwrap();
    let command = generated["hooks"]["PreToolUse"][1]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    let output = std::process::Command::new("/bin/sh")
        .args(["-c", command])
        .env("PATH", dir.path())
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "hook:0\nuser-check\n"
    );
    assert_eq!(output.status.code(), Some(2), "the user check must reject");
}

#[test]
fn a_group_left_empty_is_dropped() {
    let bins = bins();
    let groups = filter_with(
        json!([
            {"hooks": [{"type": "command", "command": format!("{} pre-tool-use", bins.ours.display())}]},
            guard("shell-check"),
            {"matcher": "Bash", "hooks": [{"type": "command", "command": format!("{} pre-tool-use", wrapper())}]},
        ]),
        &bins.ours,
    );
    assert_eq!(groups, vec![guard("shell-check")]);
}

/// Item 4 of the review: a handler is dropped only when its program is the
/// same file as the hooks binary amplihack's entry runs, directly or
/// through a link. A different binary that happens to be named
/// `amplihack-hooks`, or one that cannot be resolved, is the user's own
/// check and is kept.
#[test]
fn only_a_handler_running_the_same_hooks_binary_file_is_dropped() {
    let bins = bins();
    let link = bins.ours.parent().unwrap().parent().unwrap().join("link");
    std::fs::create_dir(&link).unwrap();
    std::os::unix::fs::symlink(&bins.ours, link.join("amplihack-hooks")).unwrap();
    let command = |program: &Path| {
        json!({"hooks": [{"type": "command",
            "command": format!("env AMPLIHACK_HOOK_PROTOCOL=codex '{}' pre-tool-use", program.display())}]})
    };
    let other = command(&bins.other);
    let unresolved = command(Path::new("/work/absent-elsewhere/amplihack-hooks"));
    let relative =
        json!({"hooks": [{"type": "command", "command": "bin/amplihack-hooks pre-tool-use"}]});
    let groups = filter_with(
        json!([
            command(&bins.ours),
            other.clone(),
            command(&link.join("amplihack-hooks")),
            unresolved.clone(),
            relative.clone(),
            guard("shell-check"),
        ]),
        &bins.ours,
    );
    assert_eq!(
        groups,
        vec![other, unresolved, relative, guard("shell-check")]
    );
}

/// A bare `amplihack-hooks` is resolved through `PATH`: dropped when that
/// finds the hooks binary file, kept when it finds another file.
#[test]
fn a_bare_hooks_program_is_resolved_through_path() {
    let _lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let bins = bins();
    let bare = json!({"hooks": [{"type": "command", "command": "amplihack-hooks pre-tool-use"}]});
    let mut env = crate::test_support::EnvGuard::capture(&[]);
    env.assign("PATH", bins.ours.parent().unwrap());
    assert_eq!(
        filter_with(json!([bare.clone()]), &bins.ours),
        Vec::<Value>::new()
    );
    env.assign("PATH", bins.other.parent().unwrap());
    assert_eq!(filter_with(json!([bare.clone()]), &bins.ours), vec![bare]);
    drop(env);
}

#[test]
fn no_pre_tool_use_means_no_user_groups() {
    for text in [
        r#"{"hooks": {}}"#.to_string(),
        json!({"hooks": {"Stop": [guard("x.py")]}}).to_string(),
    ] {
        let groups = user_pre_tool_use(
            &text,
            Path::new(USER_HOOKS),
            root(),
            Path::new(ABSENT_HOOKS_BIN),
        )
        .unwrap();
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
        let err = user_pre_tool_use(
            &text,
            Path::new(USER_HOOKS),
            root(),
            Path::new(ABSENT_HOOKS_BIN),
        )
        .expect_err(&text);
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
    let bin = Path::new("/work/fixture/.local/bin/amplihack-hooks");
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
            "command": "env AMPLIHACK_HOOK_PROTOCOL=codex '/work/fixture/.local/bin/amplihack-hooks' pre-tool-use"}]})
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

/// whatever the path holds, the shell runs exactly that file with
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

// ---- verification of a generated file a nested launch reuses ----

#[test]
fn verify_accepts_what_render_writes() {
    let dir = crate::test_support::tempdir_outside_framework_roots();
    let bin = executable(dir.path(), "amplihack-hooks");
    let user = vec![guard("a.py"), guard("b.py")];
    let (groups, hash) = verify_generated(&render(&user, &bin), &bin).expect("verified");
    assert_eq!(groups, user);
    assert_eq!(handler_count(&groups), 2);
    assert_eq!(hash, command_check_hash(&bin));
}

#[test]
fn shell_equivalent_commands_have_different_trust_hashes() {
    let dir = crate::test_support::tempdir_outside_framework_roots();
    let bin = executable(dir.path(), "amplihack-hooks");
    let text = render(&[], &bin);
    for changed in [
        text.replacen("env ", "env  ", 1),
        text.replacen("env ", "'env' ", 1),
    ] {
        let (_, hash) = verify_generated(&changed, &bin).expect("same command semantics");
        assert_ne!(hash, command_check_hash(&bin));
    }
}

/// A session that points amplihack's entry at another executable named
/// `amplihack-hooks` is refused: the nested launch would otherwise use that
/// file as its command check.
#[test]
fn verify_refuses_another_hooks_binary() {
    let dir = crate::test_support::tempdir_outside_framework_roots();
    let bin = executable(dir.path(), "amplihack-hooks");
    let other_dir = dir.path().join("other");
    std::fs::create_dir(&other_dir).unwrap();
    let other = executable(&other_dir, "amplihack-hooks");
    let text = render(&[guard("a.py")], &other);
    assert!(verify_generated(&text, &other).is_ok());
    assert!(verify_generated(&text, &bin).is_err());

    // The same file reached through a symbolic link is the same binary.
    let link_dir = dir.path().join("link");
    std::fs::create_dir(&link_dir).unwrap();
    let link = link_dir.join("amplihack-hooks");
    std::os::unix::fs::symlink(&bin, &link).unwrap();
    assert!(verify_generated(&render(&[], &link), &bin).is_ok());
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
    cases.push(("an executable that is not amplihack-hooks", v));
    let mut v = good.clone();
    v["hooks"]["PreToolUse"][0]["hooks"][0]["type"] = json!("prompt");
    cases.push(("not a command handler", v));
    let mut v = good.clone();
    v["hooks"]["PreToolUse"] = json!([]);
    cases.push(("no groups", v));

    for (what, value) in cases {
        assert!(
            verify_generated(&value.to_string(), &bin).is_err(),
            "{what}: {value:#}"
        );
    }
    for text in ["", "{", "[]", "{\"hooks\": {}}"] {
        assert!(verify_generated(text, &bin).is_err(), "{text:?}");
    }
}
