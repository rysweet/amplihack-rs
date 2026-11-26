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
//! [`table`], [`resolve`], [`codex_effort`], [`claude_effort`] and [`record`]
//! are pure; this module is the only one that reads a file or the
//! environment, and the only one that decides nothing. The contract is
//! `docs/reference/model-tiers.md`.

pub(crate) mod argv_scan;
pub(crate) mod claude_effort;
pub(crate) mod codex_effort;
pub(crate) mod notice;
pub(crate) mod record;
pub(crate) mod resolve;
pub(crate) mod table;

#[cfg(test)]
mod tests_claude_effort;
#[cfg(test)]
mod tests_codex_effort;
#[cfg(test)]
mod tests_default_model;
#[cfg(test)]
mod tests_effort_ceiling;
#[cfg(test)]
mod tests_load;
#[cfg(test)]
mod tests_notice_effort;
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

use crate::user_config::safe::{display_path, read_checked};
use crate::user_config::{self, Value};

/// The file name an instance's mapping has, wherever it lives.
const TIER_FILE: &str = "model-tiers.toml";

/// The run directory a launch records into, when it has one.
const RUN_DIR_ENV: &str = "AMPLIHACK_RUN_DIR";

/// The run this launch belongs to, when it has one.
const RUN_ID_ENV: &str = "AMPLIHACK_RUN_ID";

/// Resolve every model tier on one launch's passthrough tail, and add the
/// reasoning effort the tier asks for where the harness has a control for it.
///
/// Applied once in `run_launch`, as soon as the tool name is known and before
/// anything else reads the tail. The order matters: the LiteLLM routing check
/// stops a launch whose explicit `--model` is not `AMPLIHACK_LITELLM_MODEL`,
/// so a tier word left in place would stop every gateway launch, and the
/// passthrough rewrite and the Codex injection must see the same argv.
///
/// `gateway_target` is `proxy_target.is_some()` at the call site, passed as
/// data rather than re-derived: Copilot CLI is a gateway target that
/// `is_claude_compatible_tool` excludes, so deriving it twice put a `--model`
/// the gateway rejects on every tiered Copilot step.
///
/// The returned [`TierLaunch`] is finished with [`TierLaunch::finish`] once
/// the binary is resolved: only then can a Claude effort be checked against
/// the Claude Code that will run, and the record and the applied line say
/// what reached it.
///
/// A tail with no tier and no forced model comes back byte-identical.
pub(crate) fn resolve_passthrough(
    tool: &str,
    extra_args: &[String],
    gateway_target: bool,
) -> (Vec<String>, TierLaunch) {
    let resolved = resolve_for_launch(tool, extra_args, gateway_target);
    let (argv, effort) = reasoning_effort(tool, resolved.argv, resolved.effort);
    for notice in &resolved.notices {
        eprintln!("{}", notice.render());
    }
    let pending = TierLaunch(Some(Pending {
        tool: tool.to_string(),
        claude_effort: matches!(effort, EffortOutcome::Added(_))
            && table::column_for(tool) != "codex",
        record: resolved.record,
        effort,
    }));
    (argv, pending)
}

/// A tier decision waiting for the launch's binary.
///
/// A launch that returns before [`TierLaunch::finish`] (the Docker hand-off,
/// or a binary that cannot be resolved) still prints the applied line and
/// writes the record when this is dropped, so those launches are recorded
/// with the effort the tier asked for.
pub(crate) struct TierLaunch(Option<Pending>);

struct Pending {
    tool: String,
    /// Whether amplihack itself put `--effort` on a Claude command line.
    claude_effort: bool,
    record: Option<record::Record>,
    effort: EffortOutcome,
}

impl TierLaunch {
    /// Check a Claude effort against the version of the binary that will
    /// run, then print the applied line and write the record.
    ///
    /// `version` is the binary's `--version` output, read when the binary
    /// was resolved; nothing is spawned here. When amplihack added `--effort`
    /// and that version is older than
    /// [`claude_effort::MIN_EFFORT_VERSION`], or could not be read, the pair
    /// is taken back off `argv` and a line says so: the step runs without
    /// the effort rather than failing on an option Claude Code does not know
    /// (issue #2916).
    pub(crate) fn finish(mut self, argv: Vec<String>, version: Option<&str>) -> Vec<String> {
        let Some(mut pending) = self.0.take() else {
            return argv;
        };
        let argv = pending.check_version(argv, version);
        pending.write();
        argv
    }
}

impl Pending {
    /// The version check behind [`TierLaunch::finish`], apart from the write
    /// so a test can read the outcome it leaves.
    ///
    /// The outcome becomes `Withdrawn`, and the notice prints, only when the
    /// pair really came off `argv`. Should `argv` no longer start with the
    /// pair `inject` prepended, it is sent as it is and the outcome stays
    /// `Added`, so the record and the applied line say what Claude Code was
    /// handed. That breaks `inject`'s contract and cannot happen on today's
    /// path; Claude Code's own refusal of the option would then be visible.
    fn check_version(&mut self, argv: Vec<String>, version: Option<&str>) -> Vec<String> {
        let EffortOutcome::Added(effort) = self.effort else {
            return argv;
        };
        if !self.claude_effort || claude_effort::version_accepts_effort(version) {
            return argv;
        }
        let argv = match claude_effort::withdraw(argv) {
            Ok(argv) => argv,
            Err(argv) => return argv,
        };
        self.effort = EffortOutcome::Withdrawn;
        if let Some(tier) = self.record.as_ref().and_then(|r| r.tier) {
            let notice = Notice::EffortUnsupported {
                harness: self.tool.clone(),
                tier,
                effort,
                found: version.map(str::to_owned),
            };
            eprintln!("{}", notice.render());
        }
        argv
    }
}

impl Drop for TierLaunch {
    fn drop(&mut self) {
        if let Some(pending) = self.0.take() {
            pending.write();
        }
    }
}

impl Pending {
    /// Print the applied line and append the record. Written after the
    /// check, so `effective_effort` says what actually reached the harness
    /// rather than what the tier asked for.
    fn write(mut self) {
        if let Some(record) = self.record.as_mut() {
            record.effective_effort = match self.effort {
                EffortOutcome::Added(effort) => Some(effort),
                EffortOutcome::NotApplicable => record.effective_effort,
                EffortOutcome::CallerChose | EffortOutcome::Withdrawn => None,
            };
            if let Some(notice) = applied_notice(&self.tool, record, self.effort) {
                eprintln!("{}", notice.render());
            }
            record::append(run_dir().as_deref(), record);
        }
    }
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
    /// It was added for Claude Code, and the installed Claude Code's version
    /// is older than the first seen to accept `--effort`, or could not be
    /// read, so it was taken back off (issue #2916).
    Withdrawn,
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
        capped_from: capped_from(record),
        // Under the gateway the model is substituted downstream, so this line
        // says nothing about it and the gateway's own line, which still
        // prints, explains where it came from.
        model_elsewhere: record.fallback == Some(record::Fallback::Gateway),
    })
}

/// The effort a column's declared `max_effort` stepped this launch down from,
/// or `None`.
///
/// Derived from the record's two effort fields, and only when both are known:
/// `Option<Effort>` orders `None` below every `Some`, and a column with no
/// launch control for effort records one it never writes, so a naive
/// comparison would claim a cap on an effort that never reached a harness.
fn capped_from(record: &record::Record) -> Option<table::Effort> {
    match (record.requested_effort, record.effective_effort) {
        (Some(requested), Some(effective)) if requested > effective => Some(requested),
        _ => None,
    }
}

/// The tier's reasoning effort, on the harnesses that have a launch control
/// for it.
///
/// Codex takes it as its own `-c model_reasoning_effort=<value>` argument,
/// which is the fix for the propagation defect of issue #1619's 3 November
/// comment. Claude Code takes `--effort <level>` (issue #2916). Copilot CLI
/// has no such control, so a declared effort there is recorded rather than
/// written onto a command line.
///
/// The dispatch is on the column, not the binary name, so `rusty`,
/// `rustyclawd` and `amplifier` travel with `claude`: they already receive
/// Claude's `--model` with Claude-only model ids, so they are already treated
/// as command-line compatible with it.
///
/// The value is a [`table::Effort`], so a word outside the column's own
/// vocabulary is refused at the table, where the operator wrote it, and never
/// reaches here.
fn reasoning_effort(
    tool: &str,
    args: Vec<String>,
    effort: Option<table::Effort>,
) -> (Vec<String>, EffortOutcome) {
    let column = table::column_for(tool);
    let Some(effort) = effort.filter(|_| table::Effort::has_effort_control(&column)) else {
        return (args, EffortOutcome::NotApplicable);
    };
    let injection = match column.as_str() {
        "codex" => codex_effort::inject(&args, effort),
        // `has_effort_control` admits two columns and Codex is the other, so
        // this is the Claude one. Written as the fall-through rather than a
        // second name test, so the two readers of `has_effort_control` cannot
        // disagree about which columns it admits.
        _ => claude_effort::inject(&args, effort),
    };
    match injection {
        argv_scan::Injection::Added(injected) => (injected, EffortOutcome::Added(effort)),
        argv_scan::Injection::CallerChose => (args, EffortOutcome::CallerChose),
    }
}

/// Resolve every tier on one launch's passthrough tail.
///
/// Reads the instance mapping, `model.force`, `AMPLIHACK_DEFAULT_MODEL`, the
/// gateway variables and whether a local-model Codex launch is in scope,
/// then calls [`resolve::resolve`]. It decides nothing itself.
fn resolve_for_launch(
    binary_name: &str,
    extra_args: &[String],
    gateway_target: bool,
) -> Resolution {
    let loaded = loaded_table();
    let force = forced_model();
    let default_model = operator_default_model();
    // One source of truth for which harnesses the gateway routes: the target
    // the caller already resolved. The environment read stays here, in the
    // module that owns every syscall.
    let through_gateway = gateway_target && amplihack_utils::litellm_proxy::proxy_requested();
    let mut out = resolve::resolve(&Request {
        harness: binary_name,
        argv: extra_args,
        table: &loaded.table,
        force: force.as_deref(),
        default_model: default_model.as_deref(),
        through_gateway,
        local_model: crate::commands::launch::codex_local_model::current(binary_name).is_some(),
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

/// The per-step tier overrides an instance has configured, and the
/// diagnostics gathered while reading the file they came from.
///
/// Empty when nothing is configured. The notices come back with the map: this
/// is the recipe-run process's only entry into the tier file and it has no
/// launch of its own to print from, so an unparsable file or a bad
/// `[step_tiers]` value would otherwise never be mentioned.
pub(crate) fn step_tier_overrides() -> (&'static BTreeMap<String, Tier>, &'static [Notice]) {
    let loaded = loaded_table();
    (loaded.table.step_tiers(), &loaded.notices)
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
                path: tier_file_display(path),
                reason,
            }],
        )
    };
    // The same bounded, type-checked, non-blocking read every amplihack
    // config file gets, plus the ownership and permission check `model.force`
    // and `model.tier_map` get in the config loader: otherwise a
    // group-writable home chooses the model every agent step runs on. The
    // check is made on the open handle, so the file judged and the file
    // parsed are one object.
    match read_checked(path) {
        Ok(Some(text)) => match Table::parse_override(&text) {
            Ok((table, notices)) => (table, notices),
            Err(reason) => failed(reason),
        },
        Ok(None) => (Table::default(), Vec::new()),
        Err(reason) => failed(reason),
    }
}

/// The single instance mapping this process reads, highest first:
/// `model.tier_map`, `$AMPLIHACK_HOME/model-tiers.toml`, then
/// `$HOME/.amplihack/model-tiers.toml`. The first that exists wins outright;
/// files are never merged, so one answer says which file is in force.
fn override_file() -> Option<PathBuf> {
    let candidates = [
        configured_path(),
        non_empty("AMPLIHACK_HOME").map(|home| PathBuf::from(home).join(TIER_FILE)),
        user_config::home_dir().map(|home| home.join(".amplihack").join(TIER_FILE)),
    ];
    candidates.into_iter().flatten().find(|path| path.exists())
}

/// `model.tier_map`, from the user's own config file.
fn configured_path() -> Option<PathBuf> {
    user_only()
        .entry("model.tier_map")
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

/// The operator's own `AMPLIHACK_DEFAULT_MODEL`, when they set one.
///
/// Read through the same function the untiered path uses, so the two cannot
/// normalise a dotted Claude id differently. Only a value the operator set
/// counts: `configured_default_model` answers the built-in default when the
/// variable is unset, and taking that as a decision would flatten every tier
/// onto one model and hide every fall-back this feature reports.
fn operator_default_model() -> Option<String> {
    use crate::commands::launch::command::{ModelSource, configured_default_model};
    configured_default_model()
        .filter(|selection| selection.source == ModelSource::DefaultModelEnv)
        .map(|selection| selection.model)
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

/// The tier file as a diagnostic names it: home-relative where it sits under
/// the operator's home, escaped either way. These lines land in run logs that
/// get pasted into issues, and an absolute path carries the username.
fn tier_file_display(path: &Path) -> String {
    let shown = user_config::home_dir()
        .and_then(|home| path.strip_prefix(&home).ok().map(Path::to_path_buf))
        .map(|rest| Path::new("~").join(rest));
    display_path(shown.as_deref().unwrap_or(path))
}

/// `AMPLIHACK_RUN_DIR`, when it names something. Empty means unset to every
/// consumer, and marks a launch outside a run scope.
fn run_dir() -> Option<PathBuf> {
    non_empty(RUN_DIR_ENV).map(PathBuf::from)
}

fn non_empty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// Pin that every key this module reads is in the schema (issue #1606: no
/// dead config, and no config read that was never declared).
#[cfg(test)]
#[test]
fn every_key_this_module_reads_is_declared() {
    for name in ["model.force", "model.tier_map"] {
        assert!(
            crate::user_config::schema::key(name).is_some(),
            "{name} is not a schema key"
        );
    }
}
