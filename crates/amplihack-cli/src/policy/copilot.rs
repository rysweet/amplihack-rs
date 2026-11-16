//! Issue #2589: the Copilot CLI rendering of the policy list.
//!
//! Copilot CLI has no user-level file for command deny rules. Its
//! `permissions-config.json` holds saved per-project approvals and its own
//! documentation says it supports no deny rules; `--deny-tool` applies to the
//! session it is passed to. Deny takes precedence over allow there, including
//! over `--allow-all`, which amplihack already injects, so the rendering is
//! an argument injection at launch rather than a file writer.
//!
//! The rendered values become process arguments visible to every user on the
//! machine, so they carry short command names and never task text.

/// The `--deny-tool` arguments for `instance`, skipping any prefix the caller
/// has already denied in `user_args`, in either the separate or the joined
/// spelling.
pub(crate) fn deny_args(instance: &[String], user_args: &[String]) -> Vec<String> {
    let mut args = Vec::new();
    for prefix in super::effective_prefixes(instance) {
        let value = format!("shell({})", prefix.as_str());
        if already_denied(user_args, &value) {
            continue;
        }
        args.push("--deny-tool".to_string());
        args.push(value);
    }
    args
}

/// Whether `user_args` already denies `value`.
fn already_denied(user_args: &[String], value: &str) -> bool {
    let joined = format!("--deny-tool={value}");
    user_args
        .windows(2)
        .any(|pair| pair[0] == "--deny-tool" && pair[1] == value)
        || user_args.contains(&joined)
}
