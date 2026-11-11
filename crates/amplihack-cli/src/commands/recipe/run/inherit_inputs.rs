//! Issue #1883: the effort and model every agent step of one recipe run
//! inherits, worked out once per run (D1, D3) and checked against the run's
//! agent binary (D6). The logic is in `launch::inherit`; this module only
//! gathers its inputs: the recipe context, the parent launch's variables, the
//! nearest `claude` ancestor's argv and the fallback variables.

use crate::commands::launch::inherit::{
    Inherited, PARENT_EFFORT_ENV, PARENT_MODEL_ENV, ParentInputs, Resolved, checked,
    inherit_summary_line, parent_model_binary, resolve_parent, vet_for_binary,
};
use std::collections::BTreeMap;

/// The values the steps of a run with agent `binary` inherit, and the stderr
/// lines to print once for the run, in order: "ignoring" warnings, "cannot be
/// passed" notices, then the summary of the values actually passed on. Never fails: a source that cannot be
/// read counts as unset.
pub(crate) fn inherit_for_run(
    context: &BTreeMap<String, String>,
    binary: &str,
) -> (Inherited, Vec<String>) {
    let env = |name: &str| std::env::var(name).ok();
    // The fallbacks are not amplihack's own variables, so an empty value is
    // read as unset rather than warned about.
    let non_empty = |name: &str| env(name).filter(|v| !v.trim().is_empty());
    let parent_effort = env(PARENT_EFFORT_ENV);
    let parent_model = env(PARENT_MODEL_ENV);
    let effort_env = non_empty("CLAUDE_CODE_EFFORT_LEVEL");
    let model_env = non_empty("ANTHROPIC_MODEL");
    let context_effort = context.get("effort").map(String::as_str);
    let context_model = context.get("model").map(String::as_str);
    // D2: the ancestor is only a fallback. Scanning `/proc` reads every
    // process on the host, so skip it when the context or the parent launch
    // already gives a valid effort and model.
    let settled = |a: Option<&str>, b: Option<&str>| {
        [a, b].into_iter().flatten().any(|v| checked(v).is_some())
    };
    let (ancestor_effort, ancestor_model) = if settled(context_effort, parent_effort.as_deref())
        && settled(context_model, parent_model.as_deref())
    {
        (None, None)
    } else {
        ancestor_opts().unwrap_or_default()
    };
    let resolved = resolve_parent(&ParentInputs {
        context_effort,
        context_model,
        parent_effort: parent_effort.as_deref(),
        parent_model: parent_model.as_deref(),
        ancestor_effort: ancestor_effort.as_deref(),
        ancestor_model: ancestor_model.as_deref(),
        // C2: an older amplihack session may have put its own default model
        // on the ancestor's argv, so that model is trusted only outside one.
        ancestor_model_trusted: std::env::var_os("AMPLIHACK_SESSION_ID").is_none(),
        effort_env: effort_env.as_deref(),
        model_env: model_env.as_deref(),
    });
    let through_gateway = amplihack_utils::litellm_proxy::proxy_requested();
    let (vetted, notices) = vet_for_binary(
        binary,
        &resolved,
        through_gateway,
        parent_model_binary().as_deref(),
    );
    let mut lines = resolved.warnings.clone();
    lines.extend(notices);
    // The summary names only what the steps receive, so it never repeats a
    // value a "cannot be passed" line has just refused.
    let passed = Resolved {
        effort_source: resolved.effort_source.filter(|_| vetted.effort.is_some()),
        model_source: resolved.model_source.filter(|_| vetted.model.is_some()),
        inherited: vetted.clone(),
        warnings: Vec::new(),
    };
    lines.extend(inherit_summary_line(&passed));
    (vetted, lines)
}

/// `--effort` and `--model` on the nearest `claude` ancestor's command line.
type AncestorOpts = (Option<String>, Option<String>);

/// D2: the nearest same-user `claude` ancestor's options, read through
/// `procfs::proc_root()`. `None` when `/proc` is missing or unreadable or
/// there is no such ancestor. Its argv is never logged and its parse
/// warnings are discarded (S8, S9).
#[cfg(unix)]
fn ancestor_opts() -> Option<AncestorOpts> {
    use crate::commands::agent::{cmdline, procfs};
    let root = procfs::proc_root();
    let snapshot = procfs::snapshot(&root, amplihack_utils::secure_files::euid()).ok()?;
    let pid = snapshot
        .ancestors(snapshot.self_pid)
        .into_iter()
        .find(|&pid| snapshot.readable_claude(pid))?;
    let (opts, _warnings) = cmdline::parse_claude(snapshot.argv(pid)?);
    Some((opts.effort, opts.model))
}

#[cfg(not(unix))]
fn ancestor_opts() -> Option<AncestorOpts> {
    None
}
