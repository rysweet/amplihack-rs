//! Issue #1619: per-step model tiers.
//!
//! A recipe agent step declares `model: "fast" | "standard" | "strong"`
//! instead of a model name, and amplihack turns that word into whatever the
//! harness it is about to launch actually has. Codex has no Opus and no
//! Sonnet, so its tiers differ by reasoning effort; Copilot CLI ships with no
//! entries at all and keeps its own default. A tier that has nothing to
//! resolve to is removed from the command line, never substituted, and the
//! step runs on the harness's own default model: it never fails for this
//! reason, and there is no retry, no timer and no probe.
//!
//! The parts:
//!
//! - [`table`] — the closed vocabulary and the per-harness mapping. Pure.
//! - [`resolve`] — the one place a tier becomes a model. Pure.
//! - [`codex_effort`] — Codex's `-c model_reasoning_effort=…`. Pure.
//! - [`record`] — one JSON line per launch, so the saving is measurable.
//! - this module — the only part that reads a file or the environment, and
//!   the only part that decides nothing.
//!
//! The contract is `docs/reference/model-tiers.md`.

pub(crate) mod argv_scan;
pub(crate) mod codex_effort;
pub(crate) mod notice;
pub(crate) mod record;
pub(crate) mod resolve;
pub(crate) mod table;

#[cfg(test)]
mod tests_codex_effort;
#[cfg(test)]
mod tests_record;
#[cfg(test)]
mod tests_resolve;
#[cfg(test)]
mod tests_table;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use notice::Notice;
use resolve::{Request, Resolution};
use table::{Table, Tier};

use crate::user_config::safe::{display_path, read_bounded};
use crate::user_config::{self, Value};

/// The file name an instance's mapping has, wherever it lives.
pub(crate) const TIER_FILE: &str = "model-tiers.toml";

/// The run directory a launch records into, when it has one.
const RUN_DIR_ENV: &str = "AMPLIHACK_RUN_DIR";

/// The run this launch belongs to, when it has one.
const RUN_ID_ENV: &str = "AMPLIHACK_RUN_ID";

/// Resolve every model tier on one launch's passthrough tail, and add the
/// reasoning effort the tier asks for where the harness has a control for it.
///
/// This is the whole feature, applied once, as early in `run_launch` as the
/// tool name is known and before anything else reads the tail. It has to be
/// this early: the LiteLLM routing check refuses a launch whose explicit
/// `--model` is not `AMPLIHACK_LITELLM_MODEL`, so a tier word left in place
/// would stop every gateway launch before it started, and the passthrough
/// rewrite and the Codex agent-step injection must see the same argv the
/// harness will.
///
/// Prints the stderr lines the resolution produced and returns the tail to
/// use in place of `extra_args`. A tail with no tier and no forced model
/// comes back byte-identical.
pub(crate) fn resolve_passthrough(tool: &str, extra_args: &[String]) -> Vec<String> {
    let mut resolved = resolve_for_launch(tool, extra_args);
    let (argv, effort) = reasoning_effort(tool, resolved.argv, resolved.effort);
    for notice in &resolved.notices {
        eprintln!("{}", notice.render());
    }
    // The record is written after the injection, so `effective_effort` says
    // what actually reached the harness rather than what the tier asked for.
    if let Some(record) = resolved.record.as_mut() {
        record.effective_effort = match effort {
            EffortOutcome::Added(effort) => Some(effort),
            EffortOutcome::NotApplicable => record.effective_effort,
            EffortOutcome::CallerChose | EffortOutcome::Rejected => None,
        };
        if let Some(notice) = applied_notice(tool, record, effort) {
            eprintln!("{}", notice.render());
        }
        record::append(run_dir().as_deref(), record);
    }
    argv
}

/// What became of the tier's reasoning effort on this launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EffortOutcome {
    /// The tier named none, or this harness has no launch control for one.
    NotApplicable,
    /// It reached the harness as its own argument.
    Added(table::Effort),
    /// The command line already set one, which amplihack never overrides.
    CallerChose,
    /// amplihack would not write the value; the model still applies.
    Rejected,
}

/// The one line a tiered launch prints, or `None` when the tier gave this
/// launch nothing and a fall-back line has already said so.
fn applied_notice(tool: &str, record: &record::Record, effort: EffortOutcome) -> Option<Notice> {
    let tier = record.tier?;
    if let EffortOutcome::CallerChose = effort {
        return Some(Notice::EffortCallerChose {
            harness: tool.to_string(),
            tier,
        });
    }
    if record.effective_model.is_none() && record.effective_effort.is_none() {
        return None;
    }
    Some(Notice::Applied {
        harness: tool.to_string(),
        tier,
        model: record.effective_model.clone(),
        effort: record.effective_effort,
    })
}

/// The tier's reasoning effort, on the one harness that has a launch control
/// for it.
///
/// Codex takes it as its own `-c model_reasoning_effort=<value>` argument,
/// which is the fix for the propagation defect of the issue's 3 November
/// comment. Claude Code and Copilot CLI have no such control, so their tiers
/// differ by model and a declared effort for them is recorded rather than
/// written onto a command line.
///
/// A value the injection will not write is dropped with one line: the model
/// still applies and the launch proceeds. There is no retry.
fn reasoning_effort(
    tool: &str,
    args: Vec<String>,
    effort: Option<table::Effort>,
) -> (Vec<String>, EffortOutcome) {
    let Some(effort) = effort.filter(|_| tool == "codex") else {
        return (args, EffortOutcome::NotApplicable);
    };
    match codex_effort::inject(&args, effort.as_str()) {
        codex_effort::Injection::Added(injected) => (injected, EffortOutcome::Added(effort)),
        codex_effort::Injection::CallerChose => (args, EffortOutcome::CallerChose),
        codex_effort::Injection::Rejected => {
            eprintln!(
                "amplihack: model tiers: `{}` is not a reasoning effort amplihack will write; \
                 the model still applies and `{tool}` uses its own effort.",
                effort.as_str()
            );
            (args, EffortOutcome::Rejected)
        }
    }
}

/// Resolve every tier on one launch's passthrough tail.
///
/// Reads the instance mapping, `model.force` and the gateway variables, calls
/// [`resolve::resolve`], writes the measurement record and hands the stderr
/// lines back to the caller. It decides nothing itself.
fn resolve_for_launch(binary_name: &str, extra_args: &[String]) -> Resolution {
    let loaded = loaded_table();
    let force = forced_model();
    let through_gateway = super::command::is_claude_compatible_tool(binary_name)
        && amplihack_utils::litellm_proxy::proxy_requested();
    let mut out = resolve::resolve(&Request {
        harness: binary_name,
        argv: extra_args,
        table: &loaded.table,
        force: force.as_deref(),
        through_gateway,
    });
    // The file's own diagnostics come first: they explain why a mapping is
    // not what the operator wrote.
    let mut notices = loaded.notices.clone();
    notices.append(&mut out.notices);
    out.notices = notices;
    if let Some(record) = out.record.as_mut() {
        record.run_id = non_empty(RUN_ID_ENV);
    }
    out
}

/// The per-step tier overrides an instance has configured, for the recipe
/// runner to apply recipe-side. Empty when nothing is configured, which is
/// what makes a run with no overrides byte-identical to one before this
/// feature.
pub(crate) fn step_tier_overrides() -> BTreeMap<String, Tier> {
    let mut out = loaded_table().table.step_tiers().clone();
    if let Some(path) = configured_path("model.step_tiers") {
        let (table, _) = read_tier_file(&path);
        for (step, tier) in table.step_tiers() {
            out.insert(step.clone(), *tier);
        }
    }
    out
}

/// The mapping and the diagnostics gathered while loading it, read once per
/// process.
struct Loaded {
    table: Table,
    notices: Vec<Notice>,
}

fn loaded_table() -> &'static Loaded {
    static LOADED: OnceLock<Loaded> = OnceLock::new();
    LOADED.get_or_init(|| match override_file() {
        Some(path) => {
            let (overrides, notices) = read_tier_file(&path);
            Loaded {
                table: overrides.merged_over(Table::shipped()),
                notices,
            }
        }
        None => Loaded {
            table: Table::shipped().clone(),
            notices: Vec::new(),
        },
    })
}

/// Read one tier file. A file that cannot be read or parsed yields the empty
/// mapping and one diagnostic, so the shipped mapping applies and the launch
/// proceeds: a typo must not stop a run of a hundred agent steps.
fn read_tier_file(path: &Path) -> (Table, Vec<Notice>) {
    let failed = |reason: String| {
        (
            Table::default(),
            vec![Notice::TableLoadFailed {
                path: display_path(path),
                reason,
            }],
        )
    };
    // The same bounded, type-checked, non-blocking read every amplihack
    // config file gets: at most 64 KiB, regular files only, no hang on a
    // FIFO or a device.
    match read_bounded(path) {
        Ok(Some(text)) => match Table::parse_override(&text) {
            Ok((table, notices)) => (table, notices),
            Err(reason) => failed(reason),
        },
        Ok(None) => (Table::default(), Vec::new()),
        Err(reason) => failed(reason),
    }
}

/// The single instance mapping this process reads, highest first:
/// `model.tier_map`, then `$AMPLIHACK_HOME/model-tiers.toml`, then
/// `$HOME/.amplihack/model-tiers.toml`. The first that exists is the one;
/// there is no merging of several files, so one `config get model.tier_map`
/// answers "which file is in force".
fn override_file() -> Option<PathBuf> {
    let candidates = [
        configured_path("model.tier_map"),
        non_empty("AMPLIHACK_HOME").map(|home| PathBuf::from(home).join(TIER_FILE)),
        user_config::home_dir().map(|home| home.join(".amplihack").join(TIER_FILE)),
    ];
    candidates.into_iter().flatten().find(|path| path.exists())
}

/// A path key from the user's own config file.
fn configured_path(name: &str) -> Option<PathBuf> {
    user_only()
        .entry(name)
        .value
        .as_ref()
        .and_then(Value::as_text)
        .map(PathBuf::from)
}

/// `model.force`, from the user's config file or `AMPLIHACK_FORCE_MODEL`.
///
/// A value from the environment is held to the same validation as one in the
/// file, by the same code, so no path can put a token on the command line
/// that the other would refuse.
fn forced_model() -> Option<String> {
    user_only()
        .entry("model.force")
        .value
        .as_ref()
        .and_then(Value::as_text)
        .map(str::to_string)
}

/// The user-only layer of the config, read once per process. The project
/// layer is deliberately not read: a cloned repository must not name the
/// model every step runs on, nor the file that drives amplihack's rewriting
/// of recipe YAML.
fn user_only() -> &'static user_config::Effective {
    static USER: OnceLock<user_config::Effective> = OnceLock::new();
    USER.get_or_init(|| {
        user_config::load(&user_config::LoadCtx {
            home: user_config::home_dir(),
            cwd: PathBuf::from("/"),
            read_project: false,
            env: &|name| std::env::var(name).ok(),
            // Launching is not the place to rewrite a legacy config file.
            migrate: false,
        })
    })
}

/// `AMPLIHACK_RUN_DIR`, when it names something. Empty means unset to every
/// consumer, and marks a launch outside a run scope.
fn run_dir() -> Option<PathBuf> {
    non_empty(RUN_DIR_ENV).map(PathBuf::from)
}

fn non_empty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// The schema keys this feature owns, so a reader of `schema.rs` can find
/// the consumer of each one.
#[allow(dead_code)]
pub(crate) const KEYS: [&str; 3] = ["model.force", "model.tier_map", "model.step_tiers"];

/// Pin that every key this module reads is in the schema (issue #1606: no
/// dead config, and no config read that was never declared).
#[cfg(test)]
#[test]
fn every_key_this_module_reads_is_declared() {
    for name in KEYS {
        assert!(
            crate::user_config::schema::key(name).is_some(),
            "{name} is not a schema key"
        );
    }
}
