//! R2 — host-aware `additionalContext` injection.
//!
//! Different agent hosts read hook output differently:
//!
//! * **Copilot CLI** expects a **top-level** `additionalContext` **string** on
//!   the `userPromptSubmitted` hook result. If context is nested under Claude's
//!   `hookSpecificOutput`, Copilot silently ignores it — which is exactly why
//!   inbound Signal operator messages were being dropped.
//! * **Claude Code** (and every other host we do not special-case) expects the
//!   nested `hookSpecificOutput { hookEventName, additionalContext }` shape.
//!
//! [`merge_additional_context`] is a **pure** output shaper: it mutates a
//! `serde_json` object map additively (never clobbering pre-existing keys such
//! as `warnings`/`metadata`), so it is deterministic and parallel-safe.
//! [`inject_host`] resolves the active host from the working directory via the
//! shared agent-binary resolver, falling back to Copilot.

use std::path::Path;

use serde_json::{Map, Value};

/// Merge Signal/operator `additionalContext` into `out` using the shape the
/// given `host` actually understands.
///
/// * `host == "copilot"` → insert a **top-level** `additionalContext` string.
/// * any other host → insert the nested Claude-compatible
///   `hookSpecificOutput { hookEventName: event, additionalContext: ctx }`.
///
/// Additive: existing keys in `out` are preserved.
pub fn merge_additional_context(out: &mut Map<String, Value>, host: &str, event: &str, ctx: &str) {
    if host == "copilot" {
        out.insert(
            "additionalContext".to_string(),
            Value::String(ctx.to_string()),
        );
    } else {
        out.insert(
            "hookSpecificOutput".to_string(),
            serde_json::json!({
                "hookEventName": event,
                "additionalContext": ctx,
            }),
        );
    }
}

/// Resolve the active agent host for `cwd` (an allowlisted binary name:
/// `copilot`/`claude`/`codex`/`amplifier`). Falls back to `copilot` if
/// resolution fails, so callers always get a safe, known identifier.
#[must_use]
pub fn inject_host(cwd: &Path) -> String {
    amplihack_utils::agent_binary::resolve(cwd).unwrap_or_else(|_| "copilot".to_string())
}
