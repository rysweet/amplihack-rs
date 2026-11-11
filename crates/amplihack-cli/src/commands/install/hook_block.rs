//! Issue #2279: the only definition of amplihack's hooks block.
//!
//! Every writer of a `settings.json` (install, update, the launch repair,
//! profile refresh, profile seeding, the rotator heal and the launch
//! `--settings` file) builds the block with [`block`] or applies it with
//! [`merge`]; doctor and the drift warnings classify a file with [`drift`].
//!
//! [`merge`] touches only `settings["hooks"]`. It removes every amplihack
//! entry (native, legacy Python and XPIA), then puts this build's wrappers
//! first on each event in spec order. Foreign wrappers keep their bytes and
//! their order. No XPIA wrapper is ever written and no `env` key is read or
//! changed.
//!
//! The file functions read with a 1 MiB no-follow read, refuse symlinks,
//! non-regular files, hard-linked files and files owned by another user,
//! never create a file and write only when the content changes. Reasons hold
//! paths, fixed phrases, error kinds or a JSON line and column, never file
//! content (SEC-WS2-04).

use super::binary::validate_binary_path;
use super::hooks::{build_hook_wrapper, ensure_array, ensure_object, is_xpia_command};
use super::hooks::{legacy_hook_script_name, native_subcommand, subcmd_of};
use super::types::{AMPLIHACK_HOOK_SPECS, HookSpec, LIMIT_RESUME_EVENT};
use serde_json::{Map, Value};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

mod file;
pub(super) use file::create_file;
#[cfg(test)]
pub(crate) use file::owner_refusal;
pub(crate) use file::update_file_with_backup;
pub(crate) use file::{FileDrift, FileMerge, drift_file, merge_file, reason, update_file};

/// The hooks binary and whether the usage-limit resume hook is registered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HookTarget {
    pub bin: PathBuf,
    pub limit_resume: bool,
}

impl HookTarget {
    /// `None` when `bin` is not UTF-8 or holds a shell-unsafe character, so
    /// the `expect()` in `build_hook_wrapper` is never reached at runtime.
    pub(crate) fn new(bin: PathBuf, limit_resume: bool) -> Option<Self> {
        validate_binary_path(bin.to_str()?).ok()?;
        Some(Self { bin, limit_resume })
    }
}

/// This process's target: the deployed hooks binary, with the resume hook
/// unless the `claude` on PATH is too old for it. `None` when no hooks
/// binary is found or its path is unsafe; both are logged.
pub(crate) fn hook_target() -> Option<HookTarget> {
    let bin = match super::binary::stable_hooks_binary() {
        Ok(bin) => bin,
        Err(error) => {
            tracing::info!("no hooks binary, hooks block not written: {error:#}");
            return None;
        }
    };
    let limit_resume = super::settings::claude_too_old_for_limit_resume().is_none();
    let target = HookTarget::new(bin.clone(), limit_resume);
    if target.is_none() {
        tracing::warn!(
            "hooks binary path {} holds a shell-unsafe character; hooks block not written",
            bin.display()
        );
    }
    target
}

/// What [`merge`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Merge {
    Changed,
    Unchanged,
}

/// Event names whose amplihack entries are absent, not this build's, or not
/// amplihack's to keep. Each list is sorted and holds each name once.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Drift {
    pub missing: Vec<String>,
    pub differing: Vec<String>,
    pub extra: Vec<String>,
}

impl Drift {
    pub(crate) fn is_clean(&self) -> bool {
        self.missing.is_empty() && self.differing.is_empty() && self.extra.is_empty()
    }

    /// One line per non-empty list, naming events only.
    pub(crate) fn lines(&self) -> Vec<String> {
        [
            ("missing amplihack hooks for", &self.missing),
            (
                "amplihack hooks differ from this build for",
                &self.differing,
            ),
            (
                "hooks amplihack no longer registers remain for",
                &self.extra,
            ),
        ]
        .into_iter()
        .filter(|(_, events)| !events.is_empty())
        .map(|(label, events)| format!("{label}: {}", events.join(", ")))
        .collect()
    }

    /// `missing=[..] differing=[..] extra=[..]`, for doctor.
    pub(crate) fn summary(&self) -> String {
        format!(
            "missing=[{}] differing=[{}] extra=[{}]",
            self.missing.join(", "),
            self.differing.join(", "),
            self.extra.join(", ")
        )
    }
}

/// The amplihack hook specs: the whole table, less `StopFailure` without
/// `limit_resume` (#1659).
pub(super) fn specs(limit_resume: bool) -> Vec<HookSpec> {
    AMPLIHACK_HOOK_SPECS
        .iter()
        .filter(|spec| limit_resume || spec.event != LIMIT_RESUME_EVENT)
        .cloned()
        .collect()
}

/// The `hooks` object for `t`: one wrapper per spec, in spec order.
pub(crate) fn block(t: &HookTarget) -> Value {
    let mut hooks = Map::new();
    for spec in specs(t.limit_resume) {
        let slot = hooks
            .entry(spec.event)
            .or_insert_with(|| Value::Array(Vec::new()));
        ensure_array(slot).push(build_hook_wrapper(&spec, &t.bin));
    }
    Value::Object(hooks)
}

/// Gives `settings` exactly `block(t)` as its amplihack entries.
pub(crate) fn merge(settings: &mut Value, t: &HookTarget) -> Merge {
    let before = settings.clone();
    let Value::Object(wanted) = block(t) else {
        unreachable!("block always returns an object")
    };
    let hooks = ensure_object(
        ensure_object(settings)
            .entry("hooks")
            .or_insert_with(|| Value::Object(Map::new())),
    );
    let events: Vec<String> = hooks.keys().cloned().collect();
    for event in events {
        let Some(Value::Array(wrappers)) = hooks.get_mut(&event) else {
            continue;
        };
        let stripped = strip_own(wrappers);
        if stripped && wrappers.is_empty() && !wanted.contains_key(&event) {
            hooks.remove(&event);
        }
    }
    for (event, ours) in wanted {
        let slot = hooks
            .entry(event)
            .or_insert_with(|| Value::Array(Vec::new()));
        let wrappers = ensure_array(slot);
        let foreign = std::mem::take(wrappers);
        if let Value::Array(ours) = ours {
            wrappers.extend(ours);
        }
        wrappers.extend(foreign);
    }
    if *settings == before {
        Merge::Unchanged
    } else {
        Merge::Changed
    }
}

/// Removes amplihack's hook entries from `wrappers`, and every wrapper that
/// this call left with an empty `hooks` list. A wrapper whose `hooks` list
/// was already empty is foreign and is kept as it is (SR-9). Whether
/// anything was removed.
fn strip_own(wrappers: &mut Vec<Value>) -> bool {
    let before = wrappers.len();
    let mut removed = false;
    wrappers.retain_mut(|wrapper| {
        let Some(entries) = wrapper.get_mut("hooks").and_then(Value::as_array_mut) else {
            return true;
        };
        let count = entries.len();
        entries.retain(|hook| command_of(hook).and_then(classify).is_none());
        removed |= entries.len() != count;
        entries.len() == count || !entries.is_empty()
    });
    removed || wrappers.len() != before
}

/// Drift between `settings` and `block(t)`. Pure.
pub(crate) fn drift(settings: &Value, t: &HookTarget) -> Drift {
    drift_inner(settings, &specs(t.limit_resume), Some(&t.bin))
}

/// [`drift`] that does not compare the hooks binary path, for callers that
/// do not know which binary the file should name.
pub(super) fn drift_ignoring_path(settings: &Value, limit_resume: bool) -> Drift {
    drift_inner(settings, &specs(limit_resume), None)
}

/// Whose a hook command is: amplihack's native or legacy entry for a known
/// subcommand, or an amplihack entry with no spec (XPIA, unknown legacy).
enum Owned {
    Sub(&'static str),
    Unspecified,
}

fn command_of(hook: &Value) -> Option<&str> {
    hook.get("command").and_then(Value::as_str)
}

fn classify(command: &str) -> Option<Owned> {
    if is_xpia_command(command) {
        return Some(Owned::Unspecified);
    }
    let native = native_subcommand(command);
    for spec in AMPLIHACK_HOOK_SPECS {
        let sub = subcmd_of(spec);
        if native == Some(sub) {
            return Some(Owned::Sub(sub));
        }
        if command.contains("tools/amplihack/hooks/")
            && legacy_hook_script_name(sub).is_some_and(|name| command.contains(name))
        {
            return Some(Owned::Sub(sub));
        }
    }
    command
        .contains("tools/amplihack/hooks/")
        .then_some(Owned::Unspecified)
}

fn drift_inner(settings: &Value, specs: &[HookSpec], bin: Option<&Path>) -> Drift {
    let (mut missing, mut differing, mut extra) =
        (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    let mut seen = BTreeSet::new();
    let hooks = settings.get("hooks").and_then(Value::as_object);
    for (event, value) in hooks.into_iter().flatten() {
        let Some(wrappers) = value.as_array() else {
            continue;
        };
        seen.insert(event.as_str());
        let event_specs: Vec<&HookSpec> = specs.iter().filter(|s| s.event == event).collect();
        let mut found: Vec<(usize, &Value)> = Vec::new();
        for wrapper in wrappers {
            let entries = wrapper.get("hooks").and_then(Value::as_array);
            for hook in entries.into_iter().flatten() {
                match command_of(hook).and_then(classify) {
                    None => {}
                    Some(Owned::Sub(sub)) => {
                        match event_specs.iter().position(|s| subcmd_of(s) == sub) {
                            Some(index) => found.push((index, wrapper)),
                            None => {
                                extra.insert(event.clone());
                            }
                        }
                    }
                    Some(Owned::Unspecified) => {
                        extra.insert(event.clone());
                    }
                }
            }
        }
        for (index, spec) in event_specs.iter().enumerate() {
            let hits: Vec<&Value> = found
                .iter()
                .filter(|(i, _)| *i == index)
                .map(|(_, w)| *w)
                .collect();
            match hits.as_slice() {
                [] => {
                    missing.insert(event.clone());
                }
                [one] if wrapper_is(one, spec, bin) => {}
                _ => {
                    differing.insert(event.clone());
                }
            }
        }
        let in_order = found.windows(2).all(|pair| pair[0].0 <= pair[1].0);
        if !in_order && !missing.contains(event) {
            differing.insert(event.clone());
        }
    }
    for spec in specs {
        if !seen.contains(spec.event) {
            missing.insert(spec.event.to_string());
        }
    }
    Drift {
        missing: missing.into_iter().collect(),
        differing: differing.into_iter().collect(),
        extra: extra.into_iter().collect(),
    }
}

/// Whether `wrapper` is exactly this build's wrapper for `spec`. With no
/// `bin`, the command strings are not compared.
fn wrapper_is(wrapper: &Value, spec: &HookSpec, bin: Option<&Path>) -> bool {
    match bin {
        Some(bin) => *wrapper == build_hook_wrapper(spec, bin),
        None => {
            let shape = |w: &Value| {
                let mut w = w.clone();
                if let Some(hook) = w.pointer_mut("/hooks/0").and_then(Value::as_object_mut) {
                    hook.remove("command");
                }
                w
            };
            shape(wrapper) == shape(&build_hook_wrapper(spec, Path::new("/amplihack-hooks")))
        }
    }
}
