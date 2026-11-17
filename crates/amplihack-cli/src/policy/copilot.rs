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

/// The `--deny-tool` arguments for `instance`, one flag and value pair per
/// prefix, unconditionally.
///
/// The rendered value carries the `:*` suffix (`shell(git push --force:*)`),
/// which is what GitHub documents as the shell-rule prefix form: `shell(git:*)`
/// matches `git push` but not `gitea`, while a bare `shell(git status)` is
/// documented as matching that text literally and so would spare every
/// invocation that has arguments -- that is, everything worth denying.
///
/// This reads nothing from the caller's own arguments. An earlier version
/// skipped a prefix the caller appeared to have denied already, by scanning
/// argv for an adjacent `--deny-tool <value>` pair. That scan matched any
/// adjacent pair, so `--model --deny-tool "shell(pkill:*)"` -- where Copilot
/// consumes `--deny-tool` as the value of `--model` -- suppressed amplihack's
/// own injection and left the command undenied. Silently dropping a deny rule
/// is the worst failure direction available here, and the deduplication it
/// bought was never needed: a caller who passes the same `--deny-tool` sees it
/// twice in argv, which denies the same command twice. Whether Copilot accepts
/// a repeated `--deny-tool` is recorded as an open check in
/// `docs/reference/dangerous-command-policy.md`; a flag it rejected would fail
/// loudly at launch, which the suppressed deny did not.
pub(crate) fn deny_args(instance: &[String]) -> Vec<String> {
    let mut args = Vec::new();
    for prefix in super::effective_prefixes(instance) {
        args.push("--deny-tool".to_string());
        args.push(format!("shell({}:*)", prefix.as_str()));
    }
    args
}
