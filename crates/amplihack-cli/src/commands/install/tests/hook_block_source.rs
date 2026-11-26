//! Issue #2279 (C1): `install/hook_block.rs` is the only definition of
//! amplihack's hooks block. `block`, `merge` and `drift` agree with one
//! another for both `limit_resume` values, `merge` is idempotent, and it
//! touches nothing outside `settings["hooks"]` and nothing that is not
//! amplihack's.
//!
//! Pure functions only: no file, no HOME, no Claude.

use super::*;
use hook_block::{HookTarget, Merge};
use serde_json::{Value, json};
use std::path::PathBuf;

const BIN: &str = "/opt/amplihack-test/bin/amplihack-hooks";

fn target(limit_resume: bool) -> HookTarget {
    HookTarget {
        bin: PathBuf::from(BIN),
        limit_resume,
    }
}

fn subcmd(spec: &HookSpec) -> &'static str {
    let HookCommandKind::BinarySubcmd { subcmd } = &spec.cmd;
    subcmd
}

fn commands(settings: &Value, event: &str) -> Vec<String> {
    settings["hooks"][event]
        .as_array()
        .map(|wrappers| {
            wrappers
                .iter()
                .filter_map(|w| w["hooks"][0]["command"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// A stripped home file shaped like the 1010-byte file in #2279: permissions,
/// a status line, `env` with `XPIA_ENABLED: "false"` and a dummy secret, and
/// one foreign PreToolUse hook. No amplihack hook at all.
fn stripped_home() -> Value {
    json!({
        "permissions": {"allow": ["Bash(git status)"], "defaultMode": "default"},
        "statusLine": {"type": "command", "command": "amplihack profile statusline"},
        "env": {"XPIA_ENABLED": "false", "SENTINEL_SECRET": "sk-test-do-not-print"},
        "hooks": {
            "PreToolUse": [{
                "matcher": "Bash",
                "hooks": [{"type": "command", "command": "python3 /opt/foreign/block-tmux-kill.py"}]
            }]
        },
        "language": "en"
    })
}

// ── specs ─────────────────────────────────────────────────────────────────

#[test]
fn specs_with_limit_resume_are_the_full_table() {
    let specs = hook_block::specs(true);
    assert_eq!(specs.len(), AMPLIHACK_HOOK_SPECS.len());
    assert_eq!(specs.len(), 9, "nine amplihack hook entries");
    let events: std::collections::BTreeSet<&str> = specs.iter().map(|s| s.event).collect();
    assert_eq!(events.len(), 8, "eight distinct events");
}

#[test]
fn specs_without_limit_resume_leave_out_stop_failure_only() {
    let specs = hook_block::specs(false);
    assert_eq!(specs.len(), AMPLIHACK_HOOK_SPECS.len() - 1);
    assert!(specs.iter().all(|s| s.event != LIMIT_RESUME_EVENT));
}

// ── block ─────────────────────────────────────────────────────────────────

#[test]
fn block_holds_one_wrapper_per_spec_in_spec_order() {
    let block = hook_block::block(&target(true));
    let hooks = block.as_object().expect("block is the hooks object");
    for spec in hook_block::specs(true) {
        let wrappers = hooks[spec.event].as_array().unwrap();
        let expected = format!("\"{BIN}\" {}", subcmd(&spec));
        assert!(
            wrappers
                .iter()
                .any(|w| w["hooks"][0]["command"].as_str() == Some(expected.as_str())),
            "{} lacks {expected}",
            spec.event
        );
    }
    // UserPromptSubmit: the classification reminder comes first.
    let ups = commands(&json!({"hooks": block}), "UserPromptSubmit");
    assert_eq!(ups.len(), 2);
    assert!(ups[0].ends_with("workflow-classification-reminder"));
    assert!(ups[1].ends_with("user-prompt-submit"));
}

#[test]
fn block_without_limit_resume_has_no_stop_failure_key() {
    let block = hook_block::block(&target(false));
    assert!(block.get(LIMIT_RESUME_EVENT).is_none());
}

#[test]
fn block_never_contains_an_xpia_command() {
    let text = hook_block::block(&target(true)).to_string();
    assert!(!text.contains("tools/xpia/hooks/"));
    assert!(!text.to_lowercase().contains("xpia"));
}

#[test]
fn block_is_byte_identical_across_calls() {
    // D5: the --settings file and the home file must carry byte-identical
    // commands, which is the condition for Claude Code's de-duplication.
    let a = serde_json::to_vec(&hook_block::block(&target(true))).unwrap();
    let b = serde_json::to_vec(&hook_block::block(&target(true))).unwrap();
    assert_eq!(a, b);
}

// ── merge / drift round trip ──────────────────────────────────────────────

#[test]
fn merge_into_empty_object_then_drift_is_clean_for_both_limit_resume_values() {
    for limit_resume in [true, false] {
        let t = target(limit_resume);
        let mut settings = json!({});
        assert_eq!(hook_block::merge(&mut settings, &t), Merge::Changed);
        let drift = hook_block::drift(&settings, &t);
        assert!(drift.is_clean(), "limit_resume={limit_resume}: {drift:?}");
        assert!(drift.lines().is_empty());
        assert_eq!(settings["hooks"], hook_block::block(&t));
    }
}

#[test]
fn merge_is_idempotent() {
    let t = target(true);
    let mut settings = stripped_home();
    assert_eq!(hook_block::merge(&mut settings, &t), Merge::Changed);
    let once = settings.clone();
    assert_eq!(hook_block::merge(&mut settings, &t), Merge::Unchanged);
    assert_eq!(settings, once);
}

#[test]
fn merge_heals_the_stripped_1010_byte_shape_and_keeps_everything_else() {
    let t = target(true);
    let original = stripped_home();
    let mut settings = original.clone();
    hook_block::merge(&mut settings, &t);

    assert!(hook_block::drift(&settings, &t).is_clean());
    for key in ["permissions", "statusLine", "env", "language"] {
        assert_eq!(settings[key], original[key], "{key} must be byte-equal");
    }
    assert_eq!(settings["env"]["XPIA_ENABLED"], json!("false"));
    assert_eq!(
        settings["env"]["SENTINEL_SECRET"],
        json!("sk-test-do-not-print")
    );
    // The foreign wrapper is kept, after amplihack's own.
    let pre = settings["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(pre.len(), 2);
    assert_eq!(pre[1], original["hooks"]["PreToolUse"][0]);
}

#[test]
fn merge_without_limit_resume_removes_a_stale_stop_failure() {
    let mut settings = json!({});
    hook_block::merge(&mut settings, &target(true));
    assert!(settings["hooks"].get(LIMIT_RESUME_EVENT).is_some());
    assert_eq!(
        hook_block::merge(&mut settings, &target(false)),
        Merge::Changed
    );
    assert!(
        settings["hooks"].get(LIMIT_RESUME_EVENT).is_none(),
        "an emptied StopFailure event is removed"
    );
    assert!(hook_block::drift(&settings, &target(false)).is_clean());
}

#[test]
fn merge_keeps_a_foreign_stop_failure_wrapper_when_removing_ours() {
    let foreign = json!({"matcher": "rate_limit", "hooks": [{"type": "command", "command": "/opt/foreign/notify"}]});
    let mut settings = json!({});
    hook_block::merge(&mut settings, &target(true));
    settings["hooks"][LIMIT_RESUME_EVENT]
        .as_array_mut()
        .unwrap()
        .push(foreign.clone());
    hook_block::merge(&mut settings, &target(false));
    assert_eq!(settings["hooks"][LIMIT_RESUME_EVENT], json!([foreign]));
}

#[test]
fn merge_rewrites_an_older_bin_path() {
    let mut settings = json!({});
    hook_block::merge(
        &mut settings,
        &HookTarget {
            bin: PathBuf::from("/opt/amplihack-old/bin/amplihack-hooks"),
            limit_resume: true,
        },
    );
    assert_eq!(
        hook_block::merge(&mut settings, &target(true)),
        Merge::Changed
    );
    assert!(!settings.to_string().contains("amplihack-old"));
    assert!(hook_block::drift(&settings, &target(true)).is_clean());
}

#[test]
fn merge_keeps_foreign_wrappers_byte_equal_and_in_order() {
    let a = json!({"hooks": [{"type": "command", "command": "/opt/foreign/a"}]});
    let b = json!({"hooks": [{"type": "command", "command": "/opt/foreign/b"}], "x": 1});
    let mut settings = json!({"hooks": {"Stop": [a.clone(), b.clone()]}});
    hook_block::merge(&mut settings, &target(true));
    let stop = settings["hooks"]["Stop"].as_array().unwrap();
    let foreign: Vec<&Value> = stop.iter().filter(|w| *w == &a || *w == &b).collect();
    assert_eq!(foreign, vec![&a, &b]);
}

#[test]
fn a_foreign_wrapper_with_amplihack_in_a_non_command_field_is_kept() {
    let foreign = json!({
        "hooks": [{"type": "command", "command": "/opt/foreign/stop", "description": "runs after amplihack-hooks stop"}]
    });
    let mut settings = json!({"hooks": {"Stop": [foreign.clone()]}});
    hook_block::merge(&mut settings, &target(true));
    assert!(
        settings["hooks"]["Stop"]
            .as_array()
            .unwrap()
            .contains(&foreign)
    );
    assert!(hook_block::drift(&settings, &target(true)).extra.is_empty());
}

#[test]
fn merge_touches_only_the_hooks_key() {
    let original = json!({
        "permissions": {"allow": []},
        "env": {"A": "1"},
        "model": "x",
        "hooks": {}
    });
    let mut settings = original.clone();
    hook_block::merge(&mut settings, &target(true));
    let keys: Vec<&String> = settings.as_object().unwrap().keys().collect();
    let original_keys: Vec<&String> = original.as_object().unwrap().keys().collect();
    assert_eq!(keys, original_keys);
    for key in ["permissions", "env", "model"] {
        assert_eq!(settings[key], original[key]);
    }
}

#[test]
fn merge_removes_amplihack_wrappers_left_empty() {
    // A wrapper that held only amplihack's entry is emptied, then dropped.
    let stale = json!({"hooks": [{"type": "command", "command": "/opt/amplihack-old/bin/amplihack-hooks stop"}]});
    let mut settings = json!({"hooks": {"Stop": [stale]}});
    hook_block::merge(&mut settings, &target(true));
    assert!(hook_block::drift(&settings, &target(true)).is_clean());
    assert_eq!(settings["hooks"]["Stop"].as_array().unwrap().len(), 1);
}

#[test]
fn merge_keeps_a_foreign_wrapper_whose_hooks_list_was_already_empty() {
    // SR-9: an empty `hooks` list that amplihack did not empty is foreign.
    let t = target(true);
    let a = json!({"hooks": [{"type": "command", "command": "/opt/foreign/a"}]});
    let empty = json!({"matcher": "X", "hooks": []});
    let b = json!({"hooks": [{"type": "command", "command": "/opt/foreign/b"}]});
    let mut settings = json!({});
    hook_block::merge(&mut settings, &t);
    settings["hooks"]["PreToolUse"]
        .as_array_mut()
        .unwrap()
        .extend([a.clone(), empty.clone(), b.clone()]);
    assert!(hook_block::drift(&settings, &t).is_clean());
    assert_eq!(hook_block::merge(&mut settings, &t), Merge::Unchanged);
    let pre = settings["hooks"]["PreToolUse"].as_array().unwrap();
    let foreign: Vec<&Value> = pre
        .iter()
        .filter(|w| *w == &a || *w == &empty || *w == &b)
        .collect();
    assert_eq!(foreign, vec![&a, &empty, &b]);
}

#[test]
fn validate_contract_is_drift_lines() {
    let t = target(true);
    let mut settings = json!({});
    hook_block::merge(&mut settings, &t);
    assert!(settings::validate_amplihack_native_hook_contract(&settings, true, &t.bin).is_empty());
    settings["hooks"]
        .as_object_mut()
        .unwrap()
        .remove("PreCompact");
    let lines = settings::validate_amplihack_native_hook_contract(&settings, true, &t.bin);
    assert!(!lines.is_empty());
    assert!(lines.iter().any(|l| l.contains("PreCompact")));
}

#[test]
fn missing_hook_registrations_reports_every_event_for_a_fully_stripped_file() {
    // D8: no early return for a file with no amplihack hook at all.
    let missing = settings::missing_hook_registrations(&stripped_home(), true);
    for spec in hook_block::specs(true) {
        assert!(
            missing.iter().any(|m| m.contains(spec.event)),
            "{} not reported: {missing:?}",
            spec.event
        );
    }
}

// ── S1: only the exact form amplihack writes is amplihack's ────────────────

fn with_stop_command(command: &str) -> Value {
    json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": command}]}]}})
}

#[test]
fn merge_keeps_a_user_wrapper_whose_name_mentions_amplihack_hooks() {
    for foreign in [
        "~/bin/my-amplihack-hooks-wrapper stop",
        "sh -c '/opt/x/amplihack-hooks stop'",
        "/opt/x/amplihack-hooks --verbose stop",
        "/opt/x/not-amplihack-hooks stop",
    ] {
        let mut s = with_stop_command(foreign);
        hook_block::merge(&mut s, &target(true));
        assert!(
            commands(&s, "Stop").iter().any(|c| c == foreign),
            "{foreign} was removed: {s}"
        );
    }
}

#[test]
fn merge_still_replaces_the_forms_amplihack_wrote() {
    for own in [
        "\"/opt/amplihack-old/bin/amplihack-hooks\" stop",
        "/opt/amplihack-old/bin/amplihack-hooks stop",
        "amplihack-hooks stop",
        "\"C:\\\\amplihack\\\\amplihack-hooks.exe\" stop",
        "/home/u/.claude/tools/amplihack/hooks/stop.py",
    ] {
        let mut s = with_stop_command(own);
        hook_block::merge(&mut s, &target(true));
        let stop = commands(&s, "Stop");
        assert!(!stop.iter().any(|c| c == own), "{own} was kept: {s}");
        assert!(stop.iter().any(|c| c.starts_with(&format!("\"{BIN}\""))));
    }
}

#[test]
fn native_subcommand_accepts_only_the_written_form() {
    assert_eq!(
        hooks::native_subcommand("\"/a b/amplihack-hooks\" pre-tool-use"),
        Some("pre-tool-use")
    );
    assert_eq!(
        hooks::native_subcommand("/a/amplihack-hooks stop"),
        Some("stop")
    );
    assert_eq!(hooks::native_subcommand("/a/amplihack-hooks"), None);
    assert_eq!(hooks::native_subcommand("\"/a/amplihack-hooks\"stop"), None);
    assert_eq!(
        hooks::native_subcommand("/a/amplihack-hooks stop now"),
        None
    );
    assert_eq!(hooks::native_subcommand("/a/my-amplihack-hooks stop"), None);
}
