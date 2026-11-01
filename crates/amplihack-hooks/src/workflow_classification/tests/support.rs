//! Fixtures shared by the coordination marker tests (issue #1742).
//!
//! Messages typed into a session by another session (status questions,
//! result reports, PR hand-overs) must not start a full recipe. A leading
//! `[coord]` marker, read by code, routes them to a short note instead of
//! the routing prompt. See docs/reference/coordination-messages.md.

use super::*;
use crate::test_support::{CwdGuard, EnvVarGuard};
use std::sync::MutexGuard;

pub(super) const EXPECTED_NOTE: &str = "[coordination] This message starts with the [coord] marker \
     and is coordination traffic between sessions. Answer it or act on it directly, \
     without dev-orchestrator, smart-orchestrator or a recipe. \
     If it asks for a code change, use dev-orchestrator as usual.";

/// Strings that would tell the agent to launch a workflow (D1).
pub(super) const LAUNCH_STRINGS: [&str; 4] = [
    "auto-intent-router",
    "launching dev-orchestrator",
    "Invoke Skill(skill=\"dev-orchestrator\")",
    "[auto-routed]",
];

/// The five calibration lines from D9, exactly as the docs page shows them.
pub(super) const CALIBRATION_BLOCK: &str = concat!(
    "  \"report your lane status\" = Q&A. \"what's your status?\" = Q&A.\n",
    "  \"CI passed on PR 12\" = Q&A.\n",
    "  \"handing PR 57 over to you\" = Q&A (acknowledge, no workflow).\n",
    "  \"PR 134 is merged, retry the install\" = OPS.\n",
    "  \"build #1740\" = DEV. A leading [coord] marker does not change the classification.\n",
);

/// Rule 8 keeps the unqualified DEV default from before #1742 and adds only
/// the status and hand-over exception the issue asked for.
pub(super) const RULE_8: &str = "8. Ambiguous → **DEV** (safe default). \
     A one-line status question, result report, acknowledgement or PR hand-over is not \
     ambiguous: classify it as **Q&A**, or **OPS** when it asks for one direct action \
     such as retrying a command.";

/// Unchanged since #91.
pub(super) const CLOSING_LINE: &str = "When in doubt, choose DEV. \
     False positive costs minutes. False negative costs quality.";

/// A temp project root that is also the cwd and HOME, with every
/// provenance variable cleared. Fields drop in declaration order, so the
/// cwd and environment are restored before the temp dir is removed and
/// before the lock is released.
pub(super) struct Isolated {
    _cwd: CwdGuard,
    _home: EnvVarGuard,
    _root: EnvVarGuard,
    _run: EnvVarGuard,
    _depth: EnvVarGuard,
    pub(super) dir: tempfile::TempDir,
    _lock: MutexGuard<'static, ()>,
}

impl Isolated {
    pub(super) fn new() -> Self {
        Self::with_run_id(None)
    }

    pub(super) fn with_run_id(run_id: Option<&str>) -> Self {
        let lock = crate::test_support::env_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let run = match run_id {
            Some(id) => EnvVarGuard::set(RECIPE_RUN_ID_ENV, id),
            None => EnvVarGuard::unset(RECIPE_RUN_ID_ENV),
        };
        Self {
            _depth: EnvVarGuard::unset("AMPLIHACK_SESSION_DEPTH"),
            _run: run,
            _root: EnvVarGuard::unset("AMPLIHACK_ROOT"),
            _home: EnvVarGuard::set("HOME", dir.path()),
            _cwd: CwdGuard::set(dir.path()).unwrap(),
            dir,
            _lock: lock,
        }
    }

    pub(super) fn state_dir(&self) -> std::path::PathBuf {
        self.dir.path().join(".claude/runtime/classification_state")
    }

    pub(super) fn state_path(&self, session: &str) -> std::path::PathBuf {
        self.state_dir().join(format!("{session}.json"))
    }

    pub(super) fn read_state(&self, session: &str) -> Option<Value> {
        let raw = fs::read_to_string(self.state_path(session)).ok()?;
        Some(serde_json::from_str(&raw).expect("state file is JSON"))
    }

    pub(super) fn seed_state(&self, session: &str, body: &str) {
        fs::create_dir_all(self.state_dir()).unwrap();
        fs::write(self.state_path(session), body).unwrap();
    }

    pub(super) fn seed_classified(&self, session: &str, turn: u64) {
        self.seed_state(
            session,
            &json!({ "last_classified_turn": turn, "session_id": session }).to_string(),
        );
    }
}

pub(super) fn run_hook(session: Option<&str>, turn: Option<u64>, prompt: &str) -> Value {
    let extra = match turn {
        Some(t) => json!({ "turnCount": t }),
        None => json!({}),
    };
    WorkflowClassificationReminderHook
        .process(HookInput::UserPromptSubmit {
            user_prompt: Some(prompt.to_string()),
            session_id: session.map(str::to_string),
            extra,
        })
        .expect("the hook never fails on a prompt")
}

pub(super) fn empty() -> Value {
    Value::Object(serde_json::Map::new())
}

/// Assert both output shapes carry `text` and return it.
pub(super) fn assert_context(out: &Value, text: &str) {
    assert_eq!(
        out.get("additionalContext").and_then(Value::as_str),
        Some(text),
        "top-level additionalContext (Copilot shape) in {out}"
    );
    assert_eq!(
        out.pointer("/hookSpecificOutput/additionalContext")
            .and_then(Value::as_str),
        Some(text),
        "hookSpecificOutput.additionalContext (Claude shape) in {out}"
    );
    assert_eq!(
        out.pointer("/hookSpecificOutput/hookEventName")
            .and_then(Value::as_str),
        Some("UserPromptSubmit")
    );
}

pub(super) fn assert_note(out: &Value) {
    assert_context(out, COORDINATION_NOTE);
    let text = out.to_string();
    for banned in LAUNCH_STRINGS {
        assert!(
            !text.contains(banned),
            "the coordination output must not contain {banned:?}: {text}"
        );
    }
}

pub(super) fn assert_router(out: &Value) {
    assert_context(out, DEFAULT_ROUTING_PROMPT);
}

pub(super) fn last_rule(state: &Value) -> Option<&str> {
    state.get("last_rule").and_then(Value::as_str)
}

pub(super) fn classified_turn(state: &Value) -> Option<u64> {
    state.get("last_classified_turn").and_then(Value::as_u64)
}
