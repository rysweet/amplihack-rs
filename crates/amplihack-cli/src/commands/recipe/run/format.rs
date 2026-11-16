use super::super::*;

use crate::commands::launch::model_tiers::record::{Fallback, Record};
use crate::commands::launch::model_tiers::table::{Effort, Tier};

const MAX_OUTPUT_LENGTH: usize = 200;
const MAX_RECENT_SNIPPET_LINES: usize = 5;
const MAX_RECENT_SNIPPET_CHARS: usize = 300;

pub(super) fn format_recipe_run_result(
    result: &RecipeRunResult,
    format: OutputFormat,
    show_context: bool,
) -> Result<String> {
    match format {
        OutputFormat::Json => {
            let mut data = serde_json::to_value(result)?
                .as_object()
                .cloned()
                .unwrap_or_default();
            if !show_context || result.context.is_empty() {
                data.remove("context");
            }
            Ok(serde_json::to_string_pretty(&JsonValue::Object(data))?)
        }
        OutputFormat::Yaml => {
            let mut data = serde_json::to_value(result)?
                .as_object()
                .cloned()
                .unwrap_or_default();
            if !show_context || result.context.is_empty() {
                data.remove("context");
            }
            Ok(serde_yaml::to_string(&JsonValue::Object(data))?)
        }
        OutputFormat::Table => Ok(format_recipe_run_table(result, show_context)),
    }
}

/// Renders the human-readable table.
///
/// The step status line (`  {symbol} {label}: {status}{detail_suffix}`) and
/// the `    Output: ` line are a machine-read contract, not just cosmetics:
/// `amplifier-bundle/tools/autodrive_loop.sh` parses them with awk to read the
/// `LOOP_HEALTH:` verdict of `step-04-enforce-loop-verdict` (issue #1512). If
/// either line's wording, indentation or details bracket changes, update that
/// awk in the same change; `tests_autodrive_health_log.rs` runs the script's
/// awk over this function's output and fails until they agree again.
fn format_recipe_run_table(result: &RecipeRunResult, show_context: bool) -> String {
    let mut lines = vec![format!("Recipe: {}", result.recipe_name)];
    if let Some(run_id) = &result.run_id {
        lines.push(format!("Run ID: {run_id}"));
    }
    lines.push(format!(
        "Status: {}",
        if result.success {
            "✓ Success"
        } else {
            "✗ Failed"
        }
    ));
    if let Some(log_pointer) = &result.log_pointer {
        lines.push(format!(
            "Log pointer: {}",
            format_log_pointer_summary(log_pointer)
        ));
    }
    lines.push(String::new());

    if result.step_results.is_empty() {
        lines.push("No steps executed (0 steps)".to_string());
        return lines.join("\n");
    }

    lines.push("Steps:".to_string());
    for step in &result.step_results {
        let status_symbol = match step.status.as_str() {
            "completed" => "✓",
            "failed" => "✗",
            "skipped" => "⊘",
            _ => "?",
        };
        let step_label = match &step.step_name {
            Some(name) if !name.is_empty() => format!("{} ({name})", step.step_id),
            _ => step.step_id.clone(),
        };
        let mut details = Vec::new();
        if let Some(phase) = &step.phase
            && !phase.is_empty()
        {
            details.push(format!("phase: {phase}"));
        }
        if let Some(elapsed_ms) = step.elapsed_ms {
            details.push(format!("elapsed: {}", format_elapsed(elapsed_ms)));
        }
        if let Some(child) = &step.child
            && let Some(label) = format_child(child)
        {
            details.push(format!("child: {label}"));
        }
        let detail_suffix = if details.is_empty() {
            String::new()
        } else {
            format!(" [{}]", details.join(", "))
        };
        lines.push(format!(
            "  {status_symbol} {step_label}: {}{detail_suffix}",
            step.status
        ));

        if !step.output.is_empty() {
            let output = if step.output.chars().count() > MAX_OUTPUT_LENGTH {
                tracing::warn!(
                    step_id = %step.step_id,
                    original_len = step.output.chars().count(),
                    max_len = MAX_OUTPUT_LENGTH,
                    "Step output truncated"
                );
                format!(
                    "{}... (truncated)",
                    step.output
                        .chars()
                        .take(MAX_OUTPUT_LENGTH)
                        .collect::<String>()
                )
            } else {
                step.output.clone()
            };
            lines.push(format!("    Output: {output}"));
        }

        if !step.error.is_empty() {
            lines.push(format!("    Error: {}", step.error));
        }
        append_recent_snippet_lines(&mut lines, "Recent stdout", &step.recent_stdout);
        append_recent_snippet_lines(&mut lines, "Recent stderr", &step.recent_stderr);
    }

    // Issue #1619: additive, after the steps and before the context, so the
    // step status and `    Output: ` lines keep their exact shape.
    let tiers = model_tier_lines(&run_tier_records(), &result.step_results);
    if !tiers.is_empty() {
        lines.push(String::new());
        lines.extend(tiers);
    }

    if show_context && !result.context.is_empty() {
        lines.push(String::new());
        lines.push("Context:".to_string());
        for (key, value) in &result.context {
            lines.push(format!("  {key}: {}", json_scalar_to_string(value)));
        }
    }

    lines.join("\n")
}

fn format_elapsed(elapsed_ms: u64) -> String {
    if elapsed_ms >= 1_000 && elapsed_ms.is_multiple_of(1_000) {
        format!("{}s", elapsed_ms / 1_000)
    } else if elapsed_ms >= 1_000 {
        format!("{:.1}s", elapsed_ms as f64 / 1_000.0)
    } else {
        format!("{elapsed_ms}ms")
    }
}

pub(super) fn format_log_pointer_summary(log_pointer: &RecipeLogPointerSummary) -> String {
    let mut parts = vec![format!("status={}", log_pointer.status)];
    if let Some(branch) = &log_pointer.branch
        && !branch.is_empty()
    {
        parts.push(format!("branch={branch}"));
    }
    if let Some(worktree) = &log_pointer.worktree
        && !worktree.is_empty()
    {
        parts.push(format!("worktree={worktree}"));
    }
    if let Some(child_pid) = log_pointer.child_pid {
        parts.push(format!("child_pid={child_pid}"));
    }
    if let Some(exit_code) = log_pointer.exit_code {
        parts.push(format!("exit_code={exit_code}"));
    }
    if let Some(runner_path) = &log_pointer.runner_path
        && !runner_path.is_empty()
    {
        parts.push(format!("runner_path={runner_path}"));
    }
    for (name, path) in &log_pointer.log_paths {
        if let Some(path) = path.as_str()
            && !path.is_empty()
        {
            parts.push(format!("log_paths.{name}={path}"));
        }
    }
    parts.join(" ")
}

fn format_child(child: &JsonValue) -> Option<String> {
    match child {
        JsonValue::String(value) if !value.is_empty() => Some(value.clone()),
        JsonValue::Object(map) => {
            let kind = map.get("kind").and_then(JsonValue::as_str).unwrap_or("");
            let name = map.get("name").and_then(JsonValue::as_str).unwrap_or("");
            match (kind.is_empty(), name.is_empty()) {
                (false, false) => Some(format!("{kind} {name}")),
                (false, true) => Some(kind.to_string()),
                (true, false) => Some(name.to_string()),
                (true, true) => None,
            }
        }
        _ => None,
    }
}

fn append_recent_snippet_lines(lines: &mut Vec<String>, label: &str, snippets: &[String]) {
    if snippets.is_empty() {
        return;
    }

    lines.push(format!("    {label}:"));
    let start = snippets.len().saturating_sub(MAX_RECENT_SNIPPET_LINES);
    for snippet in &snippets[start..] {
        lines.push(format!("      {}", truncate_snippet(snippet)));
    }
}

fn truncate_snippet(value: &str) -> String {
    if value.chars().count() <= MAX_RECENT_SNIPPET_CHARS {
        return value.to_string();
    }
    format!(
        "{}... (truncated)",
        value
            .chars()
            .take(MAX_RECENT_SNIPPET_CHARS)
            .collect::<String>()
    )
}

fn json_scalar_to_string(value: &JsonValue) -> String {
    match value {
        JsonValue::Null => "null".to_string(),
        JsonValue::Bool(v) => v.to_string(),
        JsonValue::Number(v) => v.to_string(),
        JsonValue::String(v) => v.clone(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

// =========================================================================
// Issue #1619: the run summary names the model and effort each step ran on.
//
// The section is additive. The step status line and its `    Output: ` line
// are a machine-read contract — `amplifier-bundle/tools/autodrive_loop.sh`
// parses them with awk to read the LOOP_HEALTH verdict (issue #1512) — and
// nothing here reshapes them.
//
// Attribution is a correlation, not a lookup: no step id ever reaches a child
// process, because the recipe runner sets no step-id environment variable. So
// a step id is printed only when the counts agree, and otherwise the section
// prints by launch sequence and says why. A retry, a parallel group and a
// resumed run all make the counts disagree.
// =========================================================================

/// The `Model tiers:` section for one run, or no lines at all when no launch
/// recorded a tier: an empty heading is noise in every summary that has
/// nothing to report.
pub(crate) fn model_tier_lines(
    records: &[Record],
    step_results: &[RecipeRunStepResult],
) -> Vec<String> {
    if records.is_empty() {
        return Vec::new();
    }
    let ran: Vec<&RecipeRunStepResult> = step_results
        .iter()
        .filter(|step| step.status != "skipped")
        .collect();
    let paired = ran.len() == records.len();
    let mut lines = vec!["Model tiers:".to_string()];
    let shown = records.len().min(MAX_TIER_LINES);
    if !paired {
        // Deliberately names no id, not even to say which one is missing:
        // half an attribution reads as a whole one.
        lines.push(format!(
            "  {} tiered launches against {} recipe entries that ran: amplihack cannot \
             tell which launch belongs to which, so none is named here. A retry, a \
             parallel group or a resumed run all do this.",
            records.len(),
            ran.len()
        ));
    }
    for (index, record) in records.iter().take(shown).enumerate() {
        let label = if paired {
            ran[index].step_id.clone()
        } else {
            format!("launch {}", record.seq)
        };
        lines.push(format!("  {label}: {}", describe(record)));
    }
    if records.len() > shown {
        // A summary is read, not parsed. A run of a thousand launches would
        // otherwise print a thousand lines here and bury the step results
        // above them.
        lines.push(format!(
            "  … and {} more launches; the full record is model-tiers.jsonl in the run \
             directory.",
            records.len() - shown
        ));
    }
    lines
}

/// How many launches the section names one by one.
const MAX_TIER_LINES: usize = 50;

/// What one launch ran on, in one line: the tier it asked for, the model and
/// effort it got, and where they differ from what was asked.
fn describe(record: &Record) -> String {
    // Every field here was read back off disk by `record::read_all`, and a
    // run directory is not a trust boundary amplihack owns alone. The values
    // are validated or allowlisted on the way in, so this is defence in
    // depth; it is also the only place in this feature where text from a file
    // reaches a terminal, and the repo already has one answer for that.
    let safe = crate::user_config::safe::display_safe;
    let tier = record.tier.map(Tier::as_str).unwrap_or("none");
    let model = match record.effective_model.as_deref() {
        Some(model) => safe(model),
        None => format!("{} default model", safe(&record.harness)),
    };
    let mut line = format!("{tier} → {model}");
    if let Some(effort) = record.effective_effort.map(Effort::as_str) {
        line.push_str(&format!(", effort {effort}"));
    } else if let Some(effort) = record.requested_effort.map(Effort::as_str) {
        line.push_str(&format!(
            ", effort {effort} asked for but not a launch control here"
        ));
    }
    // What was asked for, when it is not what ran: a substitution is visible
    // rather than silent.
    let mut asides = Vec::new();
    if record.requested_model != record.effective_model
        && let Some(requested) = record.requested_model.as_deref()
    {
        asides.push(format!("requested: {}", safe(requested)));
    }
    if let Some(reason) = record.fallback {
        asides.push(reason_text(reason, &safe(&record.column)));
    }
    if !asides.is_empty() {
        line.push_str(&format!(" ({})", asides.join("; ")));
    }
    line
}

/// Why a tier did not become a model, in the words the stderr line used.
fn reason_text(reason: Fallback, column: &str) -> String {
    match reason {
        Fallback::NoColumn => format!("no `{column}` column"),
        Fallback::NoMapping => format!("no entry in the `{column}` column"),
        Fallback::NoModel => format!("the `{column}` entry names no model"),
        Fallback::Gateway => "the LiteLLM gateway routes on the model name".to_string(),
    }
}

/// The run directory of the run being summarised.
///
/// `execute` records it rather than the summary reading `AMPLIHACK_RUN_DIR`
/// back: a top-level `recipe run` hands that variable to its children without
/// setting it on itself, so reading the environment would make the section
/// appear only for a nested run — and there it would report the parent's
/// launches as this run's steps. A run with no durable directory of its own
/// gets no section, which is the honest answer.
static RUN_DIR: std::sync::Mutex<Option<std::path::PathBuf>> = std::sync::Mutex::new(None);

/// Name the run directory whose tier records this process's summary reads.
pub(super) fn remember_run_dir(dir: &std::path::Path) {
    if let Ok(mut slot) = RUN_DIR.lock() {
        *slot = Some(dir.to_path_buf());
    }
}

/// Every tier record of the run being summarised, in launch order. Empty when
/// the run had no durable directory, which is also what makes the section
/// absent from a summary with nothing to report.
pub(super) fn run_tier_records() -> Vec<Record> {
    let Ok(slot) = RUN_DIR.lock() else {
        return Vec::new();
    };
    match slot.as_deref() {
        Some(dir) => crate::commands::launch::model_tiers::record::read_all(dir),
        None => Vec::new(),
    }
}
