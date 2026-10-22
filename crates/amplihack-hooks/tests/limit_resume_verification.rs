//! Issue #1658 (E): `amplihack doctor`-style verification knows about the
//! `limit-resume` registration.

use amplihack_hooks::hook_verification::verify_native_hook_registrations_at;
use serde_json::json;

fn settings_with(subcmds: &[(&str, &str)]) -> tempfile::NamedTempFile {
    let mut hooks = serde_json::Map::new();
    for (event, sub) in subcmds {
        hooks
            .entry(event.to_string())
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .unwrap()
            .push(json!({"hooks": [{"type": "command", "command": format!("amplihack-hooks {sub}")}]}));
    }
    let f = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(f.path(), json!({"hooks": hooks}).to_string()).unwrap();
    f
}

const EXISTING: &[(&str, &str)] = &[
    ("SessionStart", "session-start"),
    ("Stop", "stop"),
    ("PreToolUse", "pre-tool-use"),
    ("PostToolUse", "post-tool-use"),
    ("UserPromptSubmit", "workflow-classification-reminder"),
    ("UserPromptSubmit", "user-prompt-submit"),
    ("PreCompact", "pre-compact"),
];

#[test]
fn missing_limit_resume_is_reported() {
    let f = settings_with(EXISTING);
    let missing = verify_native_hook_registrations_at(f.path());
    assert_eq!(missing, vec!["limit-resume".to_string()]);
}

#[test]
fn registered_limit_resume_is_not_reported() {
    let mut all = EXISTING.to_vec();
    all.push(("StopFailure", "limit-resume"));
    let f = settings_with(&all);
    assert!(verify_native_hook_registrations_at(f.path()).is_empty());
}
