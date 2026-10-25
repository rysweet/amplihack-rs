//! Issue #1658 (E): install wiring for `amplihack-hooks limit-resume`.
//!
//! `StopFailure`, matcher `rate_limit`, timeout `LIMIT_RESUME_TIMEOUT_SECS`
//! and `asyncRewake: true`. `asyncRewake` appears on that wrapper only; every
//! other wrapper is written exactly as before.

use super::helpers::create_exe_stub;
use super::*;
use amplihack_utils::rotator_status::{LIMIT_RESUME_TIMEOUT_SECS, MAX_WAIT, SLOW_POLL_INTERVAL};
use serde_json::{Value, json};

const PLUGIN_HOOKS: &str = include_str!("../../../../../../claude-plugin/hooks.json");

fn subcmd(spec: &HookSpec) -> &'static str {
    let HookCommandKind::BinarySubcmd { subcmd } = &spec.cmd;
    subcmd
}

fn stop_failure_spec() -> &'static HookSpec {
    let found: Vec<&HookSpec> = AMPLIHACK_HOOK_SPECS
        .iter()
        .filter(|s| s.event == "StopFailure")
        .collect();
    assert_eq!(found.len(), 1, "exactly one StopFailure spec");
    found[0]
}

/// The wrapper shape every existing spec had before #1658, built without
/// `build_hook_wrapper`.
fn legacy_wrapper(spec: &HookSpec, bin: &std::path::Path) -> Value {
    let mut hook = json!({
        "type": "command",
        "command": format!("\"{}\" {}", bin.display(), subcmd(spec)),
    });
    if let Some(t) = spec.timeout {
        hook["timeout"] = json!(t);
    }
    let mut w = json!({});
    if let Some(m) = spec.matcher {
        w["matcher"] = json!(m);
    }
    w["hooks"] = json!([hook]);
    w
}

// ── Spec and contract ───────────────────────────────────────────────────────

#[test]
fn stop_failure_spec_runs_limit_resume_on_rate_limit_with_async_rewake() {
    let spec = stop_failure_spec();
    assert_eq!(subcmd(spec), "limit-resume");
    assert_eq!(spec.matcher, Some("rate_limit"));
    assert_eq!(spec.timeout, Some(LIMIT_RESUME_TIMEOUT_SECS));
    assert!(spec.async_rewake);
}

#[test]
fn only_the_stop_failure_spec_has_async_rewake() {
    for spec in AMPLIHACK_HOOK_SPECS.iter().chain(XPIA_HOOK_SPECS) {
        assert_eq!(
            spec.async_rewake,
            spec.event == "StopFailure",
            "{} {}",
            spec.event,
            subcmd(spec)
        );
    }
}

#[test]
fn canonical_contract_has_the_matching_stop_failure_entry() {
    let entry = CANONICAL_AMPLIHACK_HOOK_CONTRACT
        .iter()
        .find(|e| e.event == "StopFailure")
        .expect("canonical StopFailure entry");
    assert_eq!(entry.native_subcmd, Some("limit-resume"));
    assert_eq!(entry.hook_file, None);
    assert_eq!(entry.matcher, Some("rate_limit"));
    assert_eq!(entry.timeout, Some(LIMIT_RESUME_TIMEOUT_SECS));
    assert!(entry.async_rewake);
    assert!(
        CANONICAL_AMPLIHACK_HOOK_CONTRACT
            .iter()
            .filter(|e| e.event != "StopFailure")
            .all(|e| !e.async_rewake)
    );
}

#[test]
fn spec_list_and_contract_agree_entry_for_entry() {
    assert_eq!(
        AMPLIHACK_HOOK_SPECS.len(),
        CANONICAL_AMPLIHACK_HOOK_CONTRACT.len()
    );
    for (spec, entry) in AMPLIHACK_HOOK_SPECS
        .iter()
        .zip(CANONICAL_AMPLIHACK_HOOK_CONTRACT)
    {
        assert_eq!(spec.event, entry.event);
        assert_eq!(Some(subcmd(spec)), entry.native_subcmd);
        assert_eq!(spec.timeout, entry.timeout);
        assert_eq!(spec.matcher, entry.matcher);
        assert_eq!(spec.async_rewake, entry.async_rewake);
    }
}

#[test]
fn wait_plus_one_poll_is_shorter_than_the_registered_timeout() {
    assert!((MAX_WAIT + SLOW_POLL_INTERVAL).as_secs() < LIMIT_RESUME_TIMEOUT_SECS);
}

// ── Wrapper JSON ────────────────────────────────────────────────────────────

#[test]
fn stop_failure_wrapper_json() {
    let temp = tempfile::tempdir().unwrap();
    let bin = create_exe_stub(temp.path(), "amplihack-hooks");
    let w = hooks::build_hook_wrapper(stop_failure_spec(), &bin);
    assert_eq!(
        w,
        json!({
            "matcher": "rate_limit",
            "hooks": [{
                "type": "command",
                "command": format!("\"{}\" limit-resume", bin.display()),
                "timeout": LIMIT_RESUME_TIMEOUT_SECS,
                "asyncRewake": true
            }]
        })
    );
}

#[test]
fn every_other_wrapper_is_byte_identical_to_before() {
    let temp = tempfile::tempdir().unwrap();
    let bin = create_exe_stub(temp.path(), "amplihack-hooks");
    for spec in AMPLIHACK_HOOK_SPECS
        .iter()
        .chain(XPIA_HOOK_SPECS)
        .filter(|s| s.event != "StopFailure")
    {
        let built = hooks::build_hook_wrapper(spec, &bin);
        let want = legacy_wrapper(spec, &bin);
        assert_eq!(
            serde_json::to_string(&built).unwrap(),
            serde_json::to_string(&want).unwrap(),
            "{} {}",
            spec.event,
            subcmd(spec)
        );
        assert!(
            !serde_json::to_string(&built)
                .unwrap()
                .contains("asyncRewake")
        );
    }
}

// ── wrapper_matches ─────────────────────────────────────────────────────────

#[test]
fn wrapper_without_async_rewake_does_not_match_the_stop_failure_spec() {
    let temp = tempfile::tempdir().unwrap();
    let bin = create_exe_stub(temp.path(), "amplihack-hooks");
    let spec = stop_failure_spec();
    let good = hooks::build_hook_wrapper(spec, &bin);
    assert!(hooks::wrapper_matches(&good, spec, "amplihack"));

    let mut stale = good.clone();
    stale["hooks"][0]
        .as_object_mut()
        .unwrap()
        .remove("asyncRewake");
    assert!(!hooks::wrapper_matches(&stale, spec, "amplihack"));

    let mut off = good.clone();
    off["hooks"][0]["asyncRewake"] = json!(false);
    assert!(!hooks::wrapper_matches(&off, spec, "amplihack"));
}

#[test]
fn wrapper_with_async_rewake_does_not_match_a_plain_spec() {
    let temp = tempfile::tempdir().unwrap();
    let bin = create_exe_stub(temp.path(), "amplihack-hooks");
    let stop = AMPLIHACK_HOOK_SPECS
        .iter()
        .find(|s| s.event == "Stop")
        .unwrap();
    let mut w = hooks::build_hook_wrapper(stop, &bin);
    assert!(hooks::wrapper_matches(&w, stop, "amplihack"));
    w["hooks"][0]["asyncRewake"] = json!(true);
    assert!(!hooks::wrapper_matches(&w, stop, "amplihack"));
}

// ── update_hook_paths ───────────────────────────────────────────────────────

fn amplihack_stop_failure_wrappers(settings: &Value) -> Vec<&Value> {
    settings["hooks"]["StopFailure"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|w| {
                    w["hooks"][0]["command"]
                        .as_str()
                        .is_some_and(|c| c.ends_with("limit-resume"))
                })
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn install_registers_stop_failure_once_and_is_idempotent() {
    let temp = tempfile::tempdir().unwrap();
    let bin = create_exe_stub(temp.path(), "amplihack-hooks");
    let mut settings = json!({});
    hooks::update_hook_paths(&mut settings, "amplihack", AMPLIHACK_HOOK_SPECS, &bin);
    hooks::update_hook_paths(&mut settings, "amplihack", AMPLIHACK_HOOK_SPECS, &bin);
    let found = amplihack_stop_failure_wrappers(&settings);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0]["hooks"][0]["asyncRewake"], json!(true));
}

#[test]
fn install_replaces_a_stale_stop_failure_wrapper_without_async_rewake() {
    let temp = tempfile::tempdir().unwrap();
    let bin = create_exe_stub(temp.path(), "amplihack-hooks");
    let mut settings = json!({
        "hooks": {"StopFailure": [{
            "matcher": "rate_limit",
            "hooks": [{
                "type": "command",
                "command": format!("\"{}\" limit-resume", bin.display()),
                "timeout": 60
            }]
        }]}
    });
    hooks::update_hook_paths(&mut settings, "amplihack", AMPLIHACK_HOOK_SPECS, &bin);
    let found = amplihack_stop_failure_wrappers(&settings);
    assert_eq!(found.len(), 1, "{settings:#}");
    assert_eq!(found[0]["hooks"][0]["asyncRewake"], json!(true));
    assert_eq!(
        found[0]["hooks"][0]["timeout"],
        json!(LIMIT_RESUME_TIMEOUT_SECS)
    );
}

#[test]
fn install_keeps_unrelated_user_stop_failure_hooks() {
    let temp = tempfile::tempdir().unwrap();
    let bin = create_exe_stub(temp.path(), "amplihack-hooks");
    let user = json!({
        "matcher": "overloaded",
        "hooks": [{"type": "command", "command": "/usr/local/bin/notify-me"}]
    });
    let mut settings = json!({"hooks": {"StopFailure": [user.clone()]}});
    hooks::update_hook_paths(&mut settings, "amplihack", AMPLIHACK_HOOK_SPECS, &bin);
    let all = settings["hooks"]["StopFailure"].as_array().unwrap();
    assert!(all.contains(&user), "user hook dropped: {settings:#}");
    assert_eq!(amplihack_stop_failure_wrappers(&settings).len(), 1);
}

#[test]
fn xpia_install_does_not_register_stop_failure() {
    let temp = tempfile::tempdir().unwrap();
    let bin = create_exe_stub(temp.path(), "amplihack-hooks");
    let mut settings = json!({});
    hooks::update_hook_paths(&mut settings, "xpia", XPIA_HOOK_SPECS, &bin);
    assert!(settings["hooks"].get("StopFailure").is_none());
}

// ── Claude Code plugin manifest ─────────────────────────────────────────────

#[test]
fn plugin_manifest_registers_the_same_stop_failure_entry() {
    let manifest: Value = serde_json::from_str(PLUGIN_HOOKS).unwrap();
    let wrappers = manifest["hooks"]["StopFailure"]
        .as_array()
        .expect("claude-plugin/hooks.json has StopFailure");
    assert_eq!(wrappers.len(), 1);
    let w = &wrappers[0];
    assert_eq!(w["matcher"], json!("rate_limit"));
    let hooks = w["hooks"].as_array().unwrap();
    assert_eq!(hooks.len(), 1);
    let h = &hooks[0];
    assert_eq!(h["type"], json!("command"));
    assert_eq!(
        h["command"],
        json!("\"${CLAUDE_PLUGIN_ROOT}/claude-plugin/bin/amplihack-hook\" limit-resume")
    );
    assert_eq!(h["timeout"], json!(LIMIT_RESUME_TIMEOUT_SECS));
    assert_eq!(h["asyncRewake"], json!(true));
}

#[test]
fn plugin_manifest_has_async_rewake_nowhere_else() {
    let manifest: Value = serde_json::from_str(PLUGIN_HOOKS).unwrap();
    for (event, wrappers) in manifest["hooks"].as_object().unwrap() {
        if event == "StopFailure" {
            continue;
        }
        assert!(
            !wrappers.to_string().contains("asyncRewake"),
            "{event} must not carry asyncRewake"
        );
    }
}
