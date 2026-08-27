//! Native Rust port of `amplifier-bundle/tools/orch_helper.py` (#270).
//!
//! Helpers used by `smart-orchestrator.yaml` to parse LLM output:
//!
//! - [`extract_json`] — pull the first complete JSON object out of mixed
//!   markdown/prose/code-block output.
//! - [`normalise_type`] — collapse a free-text task-type label into one of
//!   `Q&A` / `Operations` / `Investigation` / `Development`.
//! - [`count_workstreams`] — count the workstreams in a decomposition blob,
//!   defaulting to 1 if absent.
//! - [`build_workstreams_config_to_tempfile`] — build the workstreams-config
//!   tempfile from a decomposition blob, returning the path.
//!
//! Exposed via the `amplihack orch helper` CLI subcommand so recipes no longer
//! need to shell into `python3`. See issue #270.

use anyhow::{Context, Result};
use clap::Subcommand;
use std::io::Read;
use std::path::PathBuf;

use crate::commands::multitask;

mod workflow_log_inventory;

#[derive(Subcommand, Debug)]
pub enum OrchCommands {
    /// Helper utilities used by smart-orchestrator (formerly orch_helper.py).
    Helper {
        #[command(subcommand)]
        command: OrchHelperCommands,
    },
    /// Run parallel workstreams from a JSON config file (WS_FILE).
    ///
    /// Native workstream orchestration entrypoint for `smart-orchestrator.yaml`.
    /// Delegates to the existing
    /// `multitask run` implementation with default flags (recipe mode,
    /// `default-workflow` recipe, no runtime override, no dry-run).
    Run {
        /// Path to the workstreams JSON config file.
        #[arg(value_name = "WS_FILE")]
        ws_file: PathBuf,
    },
}

#[derive(Subcommand, Debug)]
pub enum OrchHelperCommands {
    /// Read stdin, print the first complete JSON object found in it.
    ///
    /// Mirrors `orch_helper.extract_json`. Tries, in order: ```json blocks,
    /// untagged ``` blocks, then a balanced-brace scan over raw prose.
    /// Prints `{}` if nothing parseable is found (matches the Python).
    ///
    /// With `--require-field NAME`, selection changes deliberately: every JSON
    /// object in the text is collected in DOCUMENT order and the LAST one
    /// carrying `NAME` wins (issue #1337). First-object-wins is fail-open for
    /// a verdict — a model that quotes its input back, or that reconsiders in
    /// prose after an early draft object, gets the wrong object read.
    ExtractJson {
        /// Select the LAST JSON object that carries this field, in document
        /// order, instead of the first parseable object of any shape.
        #[arg(long, value_name = "NAME")]
        require_field: Option<String>,
    },

    /// Read stdin, print the normalised task-type label.
    ///
    /// Mirrors `orch_helper.normalise_type`. Output is one of:
    /// `Q&A`, `Operations`, `Investigation`, `Development` (the default).
    NormaliseType,

    /// Read one already-extracted verdict token from stdin, print the
    /// canonical verdict token.
    ///
    /// Output is one of `WORK_VERIFIED`, `HOLLOW_SUCCESS`, or
    /// `INSUFFICIENT_EVIDENCE` (the default for unknown/empty input).
    /// Unlike `normalise-type`, matching is **exact-token equality**
    /// (case-insensitive), not substring — so negation-adjacent labels
    /// (`UNVERIFIED`, `NOT_APPROVED`, `NOT_ACHIEVED`) never collide with a
    /// pass token and fall through to `INSUFFICIENT_EVIDENCE`. Used by the
    /// verdict-gate recipes (issue #1062, finding A).
    NormaliseVerdict,

    /// Read one already-extracted loop-health verdict token from stdin, print
    /// the canonical loop verdict token.
    ///
    /// Output is one of `CONTINUE`, `DONE`, or `STUCK` (the default for
    /// unknown, malformed, or empty input). Like [`NormaliseVerdict`] the
    /// matching is **case-insensitive exact-token equality**, never substring
    /// — `DISCONTINUE` and `CANNOT_CONTINUE` must not collide with
    /// `CONTINUE`, and `NOT_DONE` must not collide with `DONE`.
    ///
    /// The fail-safe direction is deliberately different from
    /// `normalise-verdict`: an unreadable loop verdict resolves to `STUCK`
    /// (stop and escalate), never `CONTINUE` (spend another round). Failing
    /// safe here means stopping, not looping (issue #1337).
    NormaliseLoopVerdict,

    /// Read decomposition JSON from stdin, print the workstream count.
    ///
    /// Equivalent to `max(1, len(extract_json(stdin)["workstreams"]))`.
    /// With `--force-single`, always prints `1`.
    CountWorkstreams {
        /// If true, ignore the JSON and print 1 (overrides the count).
        #[arg(long, default_value_t = false)]
        force_single: bool,
    },

    /// Read decomposition JSON from stdin, write a workstream-config
    /// tempfile, print the tempfile path on stdout.
    ///
    /// Equivalent to the `create-workstreams-config` python heredoc in
    /// `smart-orchestrator.yaml`. Each workstream becomes one config entry
    /// with: `issue: "TBD"`, branch slug, description, task, recipe.
    BuildWorkstreamsConfig,

    /// Read JSON from stdin, print the value at `--field` as a string.
    ///
    /// If the field is missing or the input is not a JSON object, prints
    /// the value of `--default` (defaults to empty string). Strings are
    /// printed without quoting; objects/arrays are printed as compact JSON.
    /// Used by recipes to avoid pulling in `jq`.
    ExtractField {
        /// Name of the top-level field to extract (no nested paths yet).
        #[arg(long)]
        field: String,
        /// Value to print if the field is absent.
        #[arg(long, default_value = "")]
        default: String,
    },

    /// Reclassify a task type using deterministic dev-signal heuristics (#269).
    ///
    /// Reads the task description from stdin and the current task type from
    /// `--current`. Promotes `Operations` → `Development` when the task
    /// contains strong development signals (file paths, PR mention, unit
    /// test mention, "Add"/"Extend"/"Implement"/"Create" keywords, multiple
    /// numbered requirements). Never demotes any other classification.
    /// Prints the (possibly updated) task type.
    ReclassifyTaskType {
        /// The current task type (e.g. "Operations").
        #[arg(long)]
        current: String,
    },

    /// List deterministic workflow log artifacts by metadata only.
    WorkflowLogInventory {
        /// Repository root to scan.
        #[arg(long, value_name = "PATH")]
        root: PathBuf,
        /// Output format: text or json.
        #[arg(long, default_value = "text", value_parser = ["text", "json"])]
        format: String,
    },
}

/// Extract and parse the FIRST complete JSON object from LLM output.
///
/// Priority (matches Python `extract_json` in `orch_helper.py`):
///   1. ```json fenced blocks (most explicit signal)
///   2. ``` untagged fenced blocks
///   3. Raw JSON in prose, scanning left-to-right with `serde_json`'s
///      streaming deserializer (handles `}` inside string values correctly).
///
/// Returns `None` if no parseable JSON object is found anywhere.
pub fn extract_json(text: &str) -> Option<serde_json::Value> {
    if let Some(v) = scan_fenced_blocks(text, true) {
        return Some(v);
    }
    if let Some(v) = scan_fenced_blocks(text, false) {
        return Some(v);
    }
    scan_raw_braces(text)
}

/// Find ```json (or ```) fenced blocks and return the first one whose body
/// parses as a JSON object. `tagged_only` selects between ```json and ```.
fn scan_fenced_blocks(text: &str, tagged_only: bool) -> Option<serde_json::Value> {
    let opener_needle = if tagged_only { "```json" } else { "```" };
    let mut search_from = 0usize;

    while let Some(open_rel) = text[search_from..].find(opener_needle) {
        let open_abs = search_from + open_rel;
        let body_start_search = open_abs + opener_needle.len();

        if !tagged_only {
            // For untagged blocks, skip any block that is actually ```json —
            // those were already considered (and failed) in the tagged pass.
            let after = &text[body_start_search..];
            let lang_len = after
                .char_indices()
                .find(|(_, c)| !c.is_alphanumeric())
                .map(|(i, _)| i)
                .unwrap_or(after.len());
            if after[..lang_len].eq_ignore_ascii_case("json") {
                search_from = body_start_search;
                continue;
            }
        }

        // Find the body's first `{` and the matching closing ``` after it.
        let Some(brace_rel) = text[body_start_search..].find('{') else {
            break;
        };
        let brace_abs = body_start_search + brace_rel;
        let Some(close_rel) = text[brace_abs..].find("```") else {
            break;
        };
        let close_abs = brace_abs + close_rel;
        let candidate = text[brace_abs..close_abs].trim();
        if let Ok(v @ serde_json::Value::Object(_)) = serde_json::from_str(candidate) {
            return Some(v);
        }
        search_from = close_abs + 3;
    }

    None
}

/// Walk left-to-right; at each `{`, ask `serde_json` if the slice starting
/// here parses as a valid JSON value via `StreamDeserializer`. The
/// streaming deserializer correctly handles braces inside string values,
/// unlike a manual depth counter — same property the Python relies on
/// via `json.JSONDecoder.raw_decode`.
fn scan_raw_braces(text: &str) -> Option<serde_json::Value> {
    let bytes = text.as_bytes();
    let mut pos = 0usize;
    while let Some(rel) = bytes[pos..].iter().position(|&b| b == b'{') {
        let start = pos + rel;
        let mut stream =
            serde_json::Deserializer::from_str(&text[start..]).into_iter::<serde_json::Value>();
        if let Some(Ok(v @ serde_json::Value::Object(_))) = stream.next() {
            return Some(v);
        }
        pos = start + 1;
    }
    None
}

/// Every JSON object in `text`, in DOCUMENT order.
///
/// Unlike [`scan_raw_braces`] this does not stop at the first hit, and on a
/// hit it advances past the object it just consumed rather than by one byte,
/// so a nested object is never also reported as a top-level candidate.
/// Fenced blocks need no special case: a ```json body is raw text too, so the
/// same left-to-right walk sees the objects inside it at their real position.
fn collect_json_objects(text: &str) -> Vec<serde_json::Value> {
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    let mut pos = 0usize;
    while let Some(rel) = bytes[pos..].iter().position(|&b| b == b'{') {
        let start = pos + rel;
        let mut stream =
            serde_json::Deserializer::from_str(&text[start..]).into_iter::<serde_json::Value>();
        match stream.next() {
            Some(Ok(v @ serde_json::Value::Object(_))) => {
                let consumed = stream.byte_offset().max(1);
                found.push(v);
                pos = start + consumed;
            }
            _ => pos = start + 1,
        }
    }
    found
}

/// Select the LAST JSON object in `text` that carries `field`.
///
/// Issue #1337, finding B3. `extract_json` is first-parseable-object-wins,
/// which is fail-OPEN for a verdict in both directions:
///
///   * a draft object followed by a reconsidered one reads the draft
///     (`{"loop_verdict":"CONTINUE"}` … "on reflection nothing moved" …
///     `{"loop_verdict":"STUCK"}` resolved to CONTINUE — the sentence that
///     should stop the loop authorised it);
///   * an evaluator that quotes its own evidence back inside a ```json fence
///     reads the EVIDENCE object, whose missing `loop_verdict` then fails
///     safe to STUCK and kills a converging loop.
///
/// Requiring the field removes the first failure mode; taking the LAST match
/// removes the second and agrees with the prompt's "as the very last thing
/// you emit". Returns `None` when no object carries the field, so the
/// caller's `--default` (STUCK) applies rather than some unrelated object.
pub fn extract_json_with_field(text: &str, field: &str) -> Option<serde_json::Value> {
    collect_json_objects(text)
        .into_iter()
        .rfind(|v| v.get(field).is_some())
}

/// Normalise an LLM task-type label to one of `Q&A`, `Operations`,
/// `Investigation`, `Development` (the default for unknowns).
///
/// Order matters — first matching keyword wins, mirroring Python's
/// short-circuit `any()` chain.
pub fn normalise_type(raw: &str) -> &'static str {
    let t = raw.to_ascii_lowercase();
    if ["q&a", "qa", "question", "answer"]
        .iter()
        .any(|k| t.contains(k))
    {
        return "Q&A";
    }
    if ["ops", "operation", "admin", "command"]
        .iter()
        .any(|k| t.contains(k))
    {
        return "Operations";
    }
    if ["invest", "research", "explor", "analys", "understand"]
        .iter()
        .any(|k| t.contains(k))
    {
        return "Investigation";
    }
    "Development"
}

/// Normalise an already-extracted verdict token to one canonical token:
/// `WORK_VERIFIED`, `HOLLOW_SUCCESS`, or `INSUFFICIENT_EVIDENCE` (the default
/// for any unrecognised or empty input).
///
/// Matching is **case-insensitive exact-token equality**, deliberately NOT the
/// substring/`contains` strategy used by [`normalise_type`]. Verdict labels
/// have negation-adjacent tokens (`UNVERIFIED` vs `VERIFIED`,
/// `NOT_APPROVED` vs `APPROVED`, `NOT_ACHIEVED` vs `ACHIEVED`); a `contains`
/// implementation would fail **open** by matching the pass token inside its
/// own negation. Equality guarantees `PASS ⊄ PASSED` and
/// `ACHIEVED ⊄ NOT_ACHIEVED` (issue #1062 R2, preserving #615 work-verifier
/// behaviour).
///
/// The input is expected to be one already-extracted token (the output of
/// `extract-field --field verdict`), not raw agent prose.
pub fn normalise_verdict(raw: &str) -> &'static str {
    let t = raw.trim().to_ascii_uppercase();
    const PASS: &[&str] = &[
        "VERIFIED",
        "WORK_VERIFIED",
        "SUCCESS",
        "APPROVED",
        "PASS",
        "PASSED",
    ];
    const HOLLOW: &[&str] = &[
        "HOLLOW",
        "HOLLOW_SUCCESS",
        "FAILED",
        "FAIL",
        "NO_WORK",
        "NO_ARTIFACTS",
        "EMPTY",
    ];
    if PASS.contains(&t.as_str()) {
        return "WORK_VERIFIED";
    }
    if HOLLOW.contains(&t.as_str()) {
        return "HOLLOW_SUCCESS";
    }
    // `INSUFFICIENT`, `INCONCLUSIVE`, `PARTIAL`, `UNKNOWN`, `UNCLEAR`, `NEEDS`,
    // and everything else (including empty and negation-adjacent labels).
    "INSUFFICIENT_EVIDENCE"
}

/// Normalise an already-extracted **loop-health** verdict token to one of
/// three canonical tokens: `CONTINUE`, `DONE`, or `STUCK`.
///
/// This is the control token of the agentic loop-health evaluation contract
/// (issue #1337): an evaluator step looks at the accumulated evidence of an
/// iterative loop and decides whether another round is worth spending
/// (`CONTINUE`), whether the loop has converged (`DONE`), or whether it is no
/// longer making progress and must stop and escalate (`STUCK`).
///
/// Two properties are security- and cost-critical:
///
/// 1. **The default is `STUCK`, not `CONTINUE`.** A missing, malformed, or
///    unrecognised verdict must stop the loop, never authorise another round.
///    Failing safe here means stopping — a fail-open default would let exactly
///    the runaway this contract exists to catch keep burning budget. This is
///    the opposite direction from [`normalise_verdict`], whose
///    `INSUFFICIENT_EVIDENCE` default is deliberately non-fatal.
/// 2. **Matching is case-insensitive exact-token equality, never substring.**
///    `DISCONTINUE`, `CANNOT_CONTINUE` and `DO_NOT_CONTINUE` all contain
///    `CONTINUE`, and `NOT_DONE` contains `DONE`; a `str::contains`
///    implementation would fail **open** on every one of them. Under equality
///    they fall through to the `STUCK` default.
///
/// The input is expected to be one already-extracted token (the output of
/// `extract-field --field loop_verdict --default STUCK`), not raw agent prose.
pub fn normalise_loop_verdict(raw: &str) -> &'static str {
    let t = raw.trim().to_ascii_uppercase();
    // Kept deliberately tight: only unambiguous "spend another round" tokens.
    // Anything doubtful belongs in the STUCK default, not here.
    const CONTINUE: &[&str] = &[
        "CONTINUE",
        "CONTINUING",
        "PROCEED",
        "KEEP_GOING",
        "ANOTHER_ROUND",
        "ITERATE",
    ];
    const DONE: &[&str] = &[
        "DONE",
        "COMPLETE",
        "COMPLETED",
        "FINISHED",
        "CONVERGED",
        "ADVANCE",
    ];
    if CONTINUE.contains(&t.as_str()) {
        return "CONTINUE";
    }
    if DONE.contains(&t.as_str()) {
        return "DONE";
    }
    // `STUCK`, `STOP`, `BLOCKED`, `NO_PROGRESS`, `ESCALATE`, `LOOPING`,
    // `NOT_CONVERGING`, every negation-adjacent label, and everything else
    // (including empty input) collapse to the fail-safe stop token.
    "STUCK"
}

/// Read all of stdin into a `String`. Errors if reading fails or the input is
/// not valid UTF-8 (recipe shell pipes always produce UTF-8 in practice).
fn read_stdin() -> Result<String> {
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .context("failed to read stdin")?;
    Ok(buf)
}

/// CLI entry point for `amplihack orch helper <subcommand>`.
pub fn run(command: OrchHelperCommands) -> Result<()> {
    match command {
        OrchHelperCommands::ExtractJson { require_field } => {
            let input = read_stdin()?;
            let found = match require_field.as_deref() {
                Some(field) => extract_json_with_field(&input, field),
                None => extract_json(&input),
            };
            let value = found.unwrap_or(serde_json::json!({}));
            println!("{}", serde_json::to_string(&value)?);
            Ok(())
        }
        OrchHelperCommands::NormaliseType => {
            let input = read_stdin()?;
            println!("{}", normalise_type(input.trim()));
            Ok(())
        }
        OrchHelperCommands::NormaliseVerdict => {
            let input = read_stdin()?;
            println!("{}", normalise_verdict(&input));
            Ok(())
        }
        OrchHelperCommands::NormaliseLoopVerdict => {
            let input = read_stdin()?;
            println!("{}", normalise_loop_verdict(&input));
            Ok(())
        }
        OrchHelperCommands::CountWorkstreams { force_single } => {
            let input = read_stdin()?;
            let count = if force_single {
                1
            } else {
                count_workstreams(&input)
            };
            println!("{count}");
            Ok(())
        }
        OrchHelperCommands::BuildWorkstreamsConfig => {
            let input = read_stdin()?;
            let path = build_workstreams_config_to_tempfile(&input)?;
            println!("{path}");
            Ok(())
        }
        OrchHelperCommands::ExtractField { field, default } => {
            let input = read_stdin()?;
            let out = extract_field(&input, &field).unwrap_or(default);
            println!("{out}");
            Ok(())
        }
        OrchHelperCommands::ReclassifyTaskType { current } => {
            let task = read_stdin()?;
            let out = reclassify_task_type(&current, &task);
            println!("{out}");
            Ok(())
        }
        OrchHelperCommands::WorkflowLogInventory { root, format } => {
            workflow_log_inventory::run(root, &format)
        }
    }
}

/// Promote a task type to `Development` when the task description contains
/// strong development signals that the LLM classifier may have missed (#269).
///
/// Rules:
/// - Only promotes `Operations` → `Development`. All other types pass through
///   unchanged (including `Q&A`, `Investigation`, and already-`Development`).
/// - Counts dev signals and promotes if any of:
///   * Task explicitly mentions opening a PR
///   * Task mentions writing tests / unit tests
///   * Task contains a source file path (`src/`, `crates/`, `bins/`, `.rs`,
///     `.py`, `.ts`, etc.) AND any one Add/Extend/Implement/Create keyword
///   * Three or more enumerated requirements (e.g. `(1) ... (2) ... (3) ...`)
///     AND any one Add/Extend/Implement/Create keyword
pub fn reclassify_task_type(current: &str, task: &str) -> String {
    let canonical = normalise_type(current);
    if canonical != "Operations" {
        return canonical.to_string();
    }
    if has_strong_dev_signals(task) {
        return "Development".to_string();
    }
    canonical.to_string()
}

/// True if `haystack` contains `needle` as a whole word (separated by
/// non-alphanumeric chars or string boundaries). All inputs assumed lowercase.
fn contains_word(haystack: &str, needle: &str) -> bool {
    let nlen = needle.len();
    if nlen == 0 || nlen > haystack.len() {
        return false;
    }
    let bytes = haystack.as_bytes();
    let mut start = 0;
    while let Some(idx) = haystack[start..].find(needle) {
        let abs = start + idx;
        let before_ok = abs == 0 || !bytes[abs - 1].is_ascii_alphanumeric();
        let after = abs + nlen;
        let after_ok = after == bytes.len() || !bytes[after].is_ascii_alphanumeric();
        if before_ok && after_ok {
            return true;
        }
        start = abs + 1;
    }
    false
}

fn has_strong_dev_signals(task: &str) -> bool {
    let lower = task.to_lowercase();

    // 1. Explicit PR mention is an unambiguous dev signal.
    let mentions_pr = lower.contains("open a pr")
        || lower.contains("open pr")
        || lower.contains("pull request")
        || lower.contains("opens a pr");
    if mentions_pr {
        return true;
    }

    // 2. Test-writing language — Operations doesn't write tests.
    let mentions_tests = lower.contains("unit test")
        || lower.contains("unit tests")
        || lower.contains("write tests")
        || lower.contains("add tests")
        || lower.contains("add a test")
        || lower.contains("test coverage");
    if mentions_tests {
        return true;
    }

    // 3. Verb signals (word-boundary aware to avoid e.g. "report" → "port").
    let dev_verbs = [
        "add",
        "extend",
        "implement",
        "create",
        "build",
        "refactor",
        "port",
    ];
    let has_dev_verb = dev_verbs.iter().any(|v| contains_word(&lower, v));

    // 4. Source-file/path signals.
    let path_markers = [
        "src/",
        "crates/",
        "bins/",
        "lib/",
        "tests/",
        "amplifier-bundle/",
        ".rs ",
        ".rs.",
        ".rs:",
        ".rs,",
        ".py ",
        ".py.",
        ".py:",
        ".py,",
        ".ts ",
        ".tsx",
        ".js ",
        ".go ",
        ".java ",
        ".cpp ",
        ".c ",
        ".h ",
    ];
    let has_path = path_markers.iter().any(|p| lower.contains(p))
        || lower.contains(".rs)")
        || lower.contains(".py)")
        || lower.ends_with(".rs")
        || lower.ends_with(".py");

    if has_dev_verb && has_path {
        return true;
    }

    // 5. Multi-requirement structure: 3+ enumerated items like "(1)", "(2)".
    let mut req_count = 0;
    for n in 1..=9 {
        if lower.contains(&format!("({n})")) {
            req_count += 1;
        }
    }
    if req_count >= 3 && has_dev_verb {
        return true;
    }

    false
}

/// Extract `field` from a top-level JSON object. Returns the string form of
/// scalars (without quotes) and the compact JSON encoding of objects/arrays.
/// Returns `None` if the input is not a JSON object, the field is missing, or
/// the field is an explicit JSON `null`.
///
/// Issue #1337: an explicit `null` used to return `Some("")`, which silently
/// bypassed the caller's `--default`. `{"terminal_refusal": null}` therefore
/// read as "the guard did not fire" instead of taking the `--default true`
/// fail-safe. A null carries no value, so it is treated exactly like an
/// absent field and the default applies.
pub fn extract_field(json: &str, field: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(json.trim()).ok()?;
    let obj = v.as_object()?;
    let val = obj.get(field)?;
    if val.is_null() {
        return None;
    }
    Some(match val {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        other => serde_json::to_string(other).unwrap_or_default(),
    })
}

/// Count workstreams in a decomposition JSON blob. Returns at least 1, even
/// when no workstreams are present (matches the Python `max(1, len(...))`).
pub fn count_workstreams(decomp: &str) -> usize {
    let obj = match extract_json(decomp) {
        Some(serde_json::Value::Object(m)) => m,
        _ => return 1,
    };
    let raw = obj
        .get("workstreams")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    raw.max(1)
}

fn workstream_classification<'a>(
    ws: &'a serde_json::Value,
    top_level_task_type: Option<&'a str>,
) -> Option<&'a str> {
    ["classification", "task_type", "type"]
        .iter()
        .find_map(|field| ws.get(*field).and_then(|v| v.as_str()))
        .or(top_level_task_type)
}

fn normalise_workstream_classification(raw: &str) -> &'static str {
    let lower = raw.to_ascii_lowercase();
    if lower.contains("consensus") {
        return "Consensus";
    }
    normalise_type(raw)
}

fn normalise_workstream_recipe(
    ws: &serde_json::Value,
    top_level_task_type: Option<&str>,
) -> String {
    let raw_recipe = ws
        .get("recipe")
        .and_then(|v| v.as_str())
        .unwrap_or("default-workflow");

    let is_development = workstream_classification(ws, top_level_task_type)
        .map(normalise_workstream_classification)
        == Some("Development");

    if is_development {
        "default-workflow".to_string()
    } else {
        raw_recipe.to_string()
    }
}

/// Build the workstreams-config tempfile from a decomposition JSON blob and
/// return the path. Mirrors the `create-workstreams-config` Python heredoc:
/// each entry has `issue: "TBD"`, `branch: feat/orch-{i}-{slug}`,
/// `description`, `task`, `recipe` (default `default-workflow`).
pub fn build_workstreams_config_to_tempfile(decomp: &str) -> Result<String> {
    let obj = extract_json(decomp).unwrap_or(serde_json::json!({}));
    let top_level_task_type = obj.get("task_type").and_then(|v| v.as_str());
    let workstreams = obj
        .get("workstreams")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut entries: Vec<serde_json::Value> = Vec::with_capacity(workstreams.len());
    for (i, ws) in workstreams.iter().enumerate() {
        let idx = i + 1;
        let name = ws
            .get("name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("workstream-{idx}"));
        let slug = slugify(&name, idx);
        let task = ws
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or(&name)
            .to_string();
        let recipe = normalise_workstream_recipe(ws, top_level_task_type);
        entries.push(serde_json::json!({
            "issue": "TBD",
            "branch": format!("feat/orch-{idx}-{slug}"),
            "description": name,
            "task": task,
            "recipe": recipe,
        }));
    }

    let dir = std::env::temp_dir();
    let mut tmp = tempfile::Builder::new()
        .prefix("smart-orch-ws-")
        .suffix(".json")
        .rand_bytes(8)
        .tempfile_in(&dir)
        .context("failed to create workstreams-config tempfile")?;

    use std::io::Write;
    let body = serde_json::to_string_pretty(&entries)?;
    tmp.write_all(body.as_bytes())?;

    // chmod 600 to match Python `os.chmod(p, 0o600)`.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(tmp.path())?.permissions();
        perms.set_mode(0o600);
        std::fs::set_permissions(tmp.path(), perms)?;
    }

    let (_, path) = tmp
        .keep()
        .context("failed to persist workstreams tempfile")?;
    Ok(path.to_string_lossy().into_owned())
}

/// Slugify a workstream name to `[a-z0-9-]{1,30}` with no leading/trailing
/// `-`. Mirrors the Python regex `[^a-z0-9-]` → `-` then trim.
fn slugify(name: &str, idx: usize) -> String {
    let lower: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let truncated: String = lower.chars().take(30).collect();
    let trimmed = truncated.trim_matches('-').to_string();
    if trimmed.is_empty() {
        format!("ws-{idx}")
    } else {
        trimmed
    }
}

/// Dispatch helper used by the top-level CLI: matches the `Orch` variant
/// down to a leaf subcommand.
pub fn dispatch(command: OrchCommands) -> Result<()> {
    match command {
        OrchCommands::Helper { command } => run(command),
        OrchCommands::Run { ws_file } => run_orch_run(ws_file),
    }
}

/// Delegates to [`multitask::run_multitask`] with the same defaults the YAML
/// recipe expects (recipe mode,
/// `default-workflow` recipe, no runtime override, no timeout policy
/// override, not a dry-run).
pub fn run_orch_run(ws_file: PathBuf) -> Result<()> {
    let path = ws_file
        .to_str()
        .with_context(|| format!("workstreams file path is not valid UTF-8: {ws_file:?}"))?;
    multitask::run_multitask(path, "recipe", "default-workflow", None, None, false)
}

/// Public guard so callers can give a friendly error for invalid types.
pub fn is_known_type(label: &str) -> bool {
    matches!(
        label,
        "Q&A" | "Operations" | "Investigation" | "Development"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // --- extract_json --------------------------------------------------------

    #[test]
    fn extract_json_from_tagged_code_block() {
        let input = "blah\n```json\n{\"task_type\": \"dev\", \"x\": 1}\n```\nmore";
        let v = extract_json(input).expect("should parse");
        assert_eq!(v, json!({"task_type": "dev", "x": 1}));
    }

    #[test]
    fn extract_json_from_untagged_code_block() {
        let input = "preamble\n```\n{\"a\": [1, 2, 3]}\n```\n";
        let v = extract_json(input).expect("should parse");
        assert_eq!(v, json!({"a": [1, 2, 3]}));
    }

    #[test]
    fn extract_json_prefers_tagged_over_untagged() {
        let input = concat!(
            "```\n{\"wrong\": true}\n```\n",
            "```json\n{\"right\": true}\n```\n",
        );
        let v = extract_json(input).expect("should parse");
        assert_eq!(v, json!({"right": true}));
    }

    #[test]
    fn extract_json_skips_malformed_tagged_block_then_finds_next() {
        let input = concat!(
            "```json\n{not valid json at all\n```\n",
            "```json\n{\"ok\": 1}\n```\n",
        );
        let v = extract_json(input).expect("should parse");
        assert_eq!(v, json!({"ok": 1}));
    }

    #[test]
    fn extract_json_from_raw_prose_skipping_non_json_braces() {
        // The {nope} prefix is not valid JSON; the scanner must move past it.
        let input = "Some prose {nope, not json} then {\"real\": \"json\"} after";
        let v = extract_json(input).expect("should parse");
        assert_eq!(v, json!({"real": "json"}));
    }

    #[test]
    fn extract_json_handles_braces_inside_string_values() {
        // Critical correctness property the Python relies on via
        // `JSONDecoder.raw_decode`. A naive depth counter would terminate
        // the object early at the first '}' in the string and fail.
        let input = "intro {\"msg\": \"this } looks {tricky\", \"n\": 7}";
        let v = extract_json(input).expect("should parse");
        assert_eq!(v, json!({"msg": "this } looks {tricky", "n": 7}));
    }

    #[test]
    fn extract_json_returns_none_when_no_object_present() {
        assert!(extract_json("just words, no JSON here at all").is_none());
        assert!(extract_json("").is_none());
        assert!(
            extract_json("[1, 2, 3]").is_none(),
            "arrays alone are not objects"
        );
    }

    #[test]
    fn extract_json_handles_multiple_tagged_blocks_first_wins() {
        let input = concat!(
            "```json\n{\"first\": true}\n```\n",
            "```json\n{\"second\": true}\n```\n",
        );
        let v = extract_json(input).expect("should parse");
        assert_eq!(v, json!({"first": true}));
    }

    #[test]
    fn extract_json_nested_object_in_block() {
        let input = "```json\n{\"workstreams\": [{\"name\": \"a\", \"meta\": {\"k\": 1}}]}\n```";
        let v = extract_json(input).expect("should parse");
        assert_eq!(v["workstreams"][0]["meta"]["k"], json!(1));
    }

    // --- extract_json_with_field (issue #1337, finding B3) --------------------
    //
    // The two measured cases the first-JSON-wins extractor got backwards. Both
    // are recorded here as a table so a future change to the selection rule
    // has to answer for them.

    #[test]
    fn require_field_takes_the_reconsidered_verdict_not_the_draft() {
        // Measured against the shipped binary: first-object-wins resolved this
        // to CONTINUE — the sentence that should stop the loop authorised it.
        let input = concat!(
            "{\"plan\":\"check\",\"loop_verdict\":\"CONTINUE\"}\n",
            "On reflection nothing moved.\n",
            "{\"loop_verdict\":\"STUCK\",\"not_converging\":[\"zero commits\"]}\n",
        );
        assert_eq!(
            extract_json(input).expect("first-object-wins picks the draft")["loop_verdict"],
            json!("CONTINUE"),
            "guard: this documents the OLD behaviour the fix replaces"
        );
        let v = extract_json_with_field(input, "loop_verdict").expect("must find a verdict");
        assert_eq!(v["loop_verdict"], json!("STUCK"));
    }

    #[test]
    fn require_field_ignores_evidence_the_evaluator_quotes_back_in_a_fence() {
        // The mirror case, which the prompt itself invites by showing the
        // evidence inside a ```json fence: first-object-wins picked the
        // EVIDENCE object, whose missing `loop_verdict` failed safe to STUCK
        // and killed a converging loop.
        let input = concat!(
            "Here is the evidence I was given:\n",
            "```json\n{\"commits_since_baseline\": 3, \"diff_lines\": 120}\n```\n",
            "It moved. Verdict:\n",
            "{\"loop_verdict\": \"CONTINUE\", \"moved\": [\"3 commits\"]}\n",
        );
        assert!(
            extract_json(input).expect("first-object-wins picks the evidence")["loop_verdict"]
                .is_null(),
            "guard: this documents the OLD behaviour the fix replaces"
        );
        let v = extract_json_with_field(input, "loop_verdict").expect("must find a verdict");
        assert_eq!(v["loop_verdict"], json!("CONTINUE"));
        assert_eq!(v["moved"], json!(["3 commits"]));
    }

    #[test]
    fn require_field_returns_none_when_no_object_carries_it() {
        // No object carries the field -> None, so the caller's `--default`
        // (STUCK) applies. Never fall back to an unrelated object.
        let input = "```json\n{\"commits_since_baseline\": 3}\n```\nI could not decide.";
        assert!(extract_json_with_field(input, "loop_verdict").is_none());
        assert!(extract_json_with_field("no json at all", "loop_verdict").is_none());
        assert!(extract_json_with_field("", "loop_verdict").is_none());
    }

    #[test]
    fn require_field_does_not_mistake_a_nested_object_for_the_verdict() {
        // The verdict object embeds a nested object that also has the key.
        // Document-order collection must not report the nested one separately,
        // or "last wins" would read the inner value.
        let input = "{\"loop_verdict\":\"STUCK\",\"echo\":{\"loop_verdict\":\"CONTINUE\"}}";
        let v = extract_json_with_field(input, "loop_verdict").expect("must find a verdict");
        assert_eq!(v["loop_verdict"], json!("STUCK"));
    }

    #[test]
    fn require_field_survives_braces_inside_string_values() {
        let input = concat!(
            "{\"loop_verdict\":\"CONTINUE\",\"note\":\"a } brace and a { brace\"}\n",
            "{\"loop_verdict\":\"DONE\",\"note\":\"trailing } brace\"}\n",
        );
        let v = extract_json_with_field(input, "loop_verdict").expect("must find a verdict");
        assert_eq!(v["loop_verdict"], json!("DONE"));
    }

    // --- normalise_type ------------------------------------------------------

    #[test]
    fn normalise_type_qa_variants() {
        for s in ["Q&A", "qa", "QA", "this is a Question?", "answer me"] {
            assert_eq!(normalise_type(s), "Q&A", "{s:?}");
        }
    }

    #[test]
    fn normalise_type_ops_variants() {
        for s in ["ops", "OPERATIONS", "admin task", "shell command"] {
            assert_eq!(normalise_type(s), "Operations", "{s:?}");
        }
    }

    #[test]
    fn normalise_type_investigation_variants() {
        for s in [
            "investigate",
            "research-mode",
            "do an Analysis",
            "explore the codebase",
            "help me UNDERSTAND",
        ] {
            assert_eq!(normalise_type(s), "Investigation", "{s:?}");
        }
    }

    #[test]
    fn normalise_type_default_to_development() {
        for s in ["dev", "build", "implement", "feature", "", "blah"] {
            assert_eq!(normalise_type(s), "Development", "{s:?}");
        }
    }

    #[test]
    fn normalise_type_priority_qa_beats_ops_when_both_keywords_present() {
        // "qa" appears before "command" in keyword order — first match wins,
        // matching the Python's short-circuit `any()` evaluation.
        assert_eq!(normalise_type("qa command"), "Q&A");
    }

    // --- normalise_verdict ---------------------------------------------------

    #[test]
    fn normalise_verdict_pass_synonyms() {
        for s in [
            "VERIFIED",
            "WORK_VERIFIED",
            "SUCCESS",
            "APPROVED",
            "PASS",
            "PASSED",
            "approved",
            "  pass  ",
        ] {
            assert_eq!(normalise_verdict(s), "WORK_VERIFIED", "{s:?}");
        }
    }

    #[test]
    fn normalise_verdict_hollow_synonyms() {
        for s in [
            "HOLLOW",
            "HOLLOW_SUCCESS",
            "FAILED",
            "FAIL",
            "NO_WORK",
            "NO_ARTIFACTS",
            "EMPTY",
            "failed",
        ] {
            assert_eq!(normalise_verdict(s), "HOLLOW_SUCCESS", "{s:?}");
        }
    }

    #[test]
    fn normalise_verdict_canonical_tokens_pass_through() {
        // The three canonical outputs must be idempotent so a value that has
        // already been normalised once survives a second pass unchanged.
        for s in ["WORK_VERIFIED", "HOLLOW_SUCCESS", "INSUFFICIENT_EVIDENCE"] {
            assert_eq!(normalise_verdict(s), s, "{s:?} must pass through unchanged");
        }
    }

    #[test]
    fn normalise_verdict_insufficient_and_unknown_default() {
        for s in [
            "INSUFFICIENT",
            "INCONCLUSIVE",
            "PARTIAL",
            "UNKNOWN",
            "UNCLEAR",
            "NEEDS",
            "",
            "   ",
            "banana",
        ] {
            assert_eq!(normalise_verdict(s), "INSUFFICIENT_EVIDENCE", "{s:?}");
        }
    }

    #[test]
    fn normalise_verdict_negation_adjacent_never_collides_with_pass() {
        // R2 regression (issue #1062): exact-token equality, not `contains`.
        // A substring implementation would match VERIFIED inside UNVERIFIED
        // (or APPROVED inside NOT_APPROVED) and fail OPEN. These must all
        // resolve to the fail-safe default, never WORK_VERIFIED.
        for s in [
            "UNVERIFIED",
            "NOT_APPROVED",
            "NOT_ACHIEVED",
            "NOT_VERIFIED",
            "UNSUCCESSFUL",
        ] {
            assert_eq!(
                normalise_verdict(s),
                "INSUFFICIENT_EVIDENCE",
                "{s:?} must NOT collide with a pass token"
            );
        }
    }

    // --- normalise_loop_verdict (issue #1337) --------------------------------

    #[test]
    fn normalise_loop_verdict_continue_synonyms() {
        for s in [
            "CONTINUE",
            "CONTINUING",
            "PROCEED",
            "KEEP_GOING",
            "ANOTHER_ROUND",
            "ITERATE",
            "continue",
            "  Proceed  ",
        ] {
            assert_eq!(normalise_loop_verdict(s), "CONTINUE", "{s:?}");
        }
    }

    #[test]
    fn normalise_loop_verdict_done_synonyms() {
        for s in [
            "DONE",
            "COMPLETE",
            "COMPLETED",
            "FINISHED",
            "CONVERGED",
            "ADVANCE",
            "done",
        ] {
            assert_eq!(normalise_loop_verdict(s), "DONE", "{s:?}");
        }
    }

    #[test]
    fn normalise_loop_verdict_stuck_synonyms() {
        for s in [
            "STUCK",
            "STOP",
            "BLOCKED",
            "NO_PROGRESS",
            "ESCALATE",
            "LOOPING",
            "NOT_CONVERGING",
            "stuck",
        ] {
            assert_eq!(normalise_loop_verdict(s), "STUCK", "{s:?}");
        }
    }

    #[test]
    fn normalise_loop_verdict_canonical_tokens_pass_through() {
        for s in ["CONTINUE", "DONE", "STUCK"] {
            assert_eq!(
                normalise_loop_verdict(s),
                s,
                "{s:?} must pass through unchanged"
            );
        }
    }

    #[test]
    fn normalise_loop_verdict_malformed_and_empty_default_to_stuck() {
        // Issue #1337 core guarantee: a missing or unparseable verdict is
        // treated as STUCK, NEVER as CONTINUE. Failing safe means stopping,
        // not spending another round of an already-unproductive loop.
        for s in [
            "",
            "   ",
            "\n",
            "banana",
            "The review workflow is still running; I'm waiting for its structured findings.",
            "{}",
            "null",
            "MAYBE",
            "UNKNOWN",
            "INSUFFICIENT_EVIDENCE",
        ] {
            assert_eq!(
                normalise_loop_verdict(s),
                "STUCK",
                "{s:?} must fail safe to STUCK"
            );
        }
    }

    #[test]
    fn normalise_loop_verdict_negation_adjacent_never_collides_with_continue_or_done() {
        // Equality, not containment. Every token here CONTAINS a permissive
        // token as a substring; a `str::contains` implementation would fail
        // OPEN and authorise another round of a dead loop.
        for s in [
            "DISCONTINUE",
            "CANNOT_CONTINUE",
            "DO_NOT_CONTINUE",
            "SHOULD_NOT_CONTINUE",
            "NOT_DONE",
            "NOT_COMPLETE",
            "NOT_COMPLETED",
            "UNFINISHED",
            "NOT_CONVERGED",
        ] {
            assert_eq!(
                normalise_loop_verdict(s),
                "STUCK",
                "{s:?} must NOT collide with CONTINUE/DONE"
            );
        }
    }

    #[test]
    fn normalise_loop_verdict_default_is_stuck_not_the_verdict_default() {
        // The two normalisers fail in opposite directions on purpose: an
        // unreadable work verdict is non-fatal (INSUFFICIENT_EVIDENCE), an
        // unreadable loop verdict stops the loop (STUCK). Guard against a
        // future refactor collapsing them into one shared default.
        assert_eq!(normalise_verdict("garbage"), "INSUFFICIENT_EVIDENCE");
        assert_eq!(normalise_loop_verdict("garbage"), "STUCK");
        assert_ne!(normalise_loop_verdict("garbage"), "CONTINUE");
    }

    #[test]
    fn is_known_type_recognises_canonical_forms() {
        for v in ["Q&A", "Operations", "Investigation", "Development"] {
            assert!(is_known_type(v));
        }
        for v in ["q&a", "ops", "Dev", "", "Other"] {
            assert!(!is_known_type(v));
        }
    }

    // --- count_workstreams ---------------------------------------------------

    #[test]
    fn count_workstreams_returns_array_length() {
        let decomp = r#"```json
{"task_type": "dev", "workstreams": [
  {"name": "a"},
  {"name": "b"},
  {"name": "c"}
]}
```"#;
        assert_eq!(count_workstreams(decomp), 3);
    }

    #[test]
    fn count_workstreams_returns_one_when_empty() {
        // Matches Python `max(1, len(...))`.
        assert_eq!(count_workstreams("{\"workstreams\": []}"), 1);
        assert_eq!(count_workstreams("{}"), 1);
        assert_eq!(count_workstreams(""), 1);
        assert_eq!(count_workstreams("not even json"), 1);
    }

    #[test]
    fn count_workstreams_handles_raw_json_in_prose() {
        let decomp = "Here is the plan: {\"workstreams\": [{\"n\":1},{\"n\":2}]} EOM";
        assert_eq!(count_workstreams(decomp), 2);
    }

    // --- build_workstreams_config_to_tempfile --------------------------------

    fn workstreams_config_from(decomp: &str) -> serde_json::Value {
        let path = build_workstreams_config_to_tempfile(decomp).unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(&path).ok();
        serde_json::from_str(&body).unwrap()
    }

    #[test]
    fn build_workstreams_config_writes_tempfile_with_entries() {
        let decomp = r#"{
            "task_type": "Development",
            "workstreams": [
              {"name": "API service",         "description": "Implement the REST API"},
              {"name": "Web UI Front-end!!!", "description": "Build the React UI"}
            ]
        }"#;
        let path = build_workstreams_config_to_tempfile(decomp).unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        let arr = parsed.as_array().unwrap();
        assert_eq!(arr.len(), 2);

        // Each entry has the required shape.
        assert_eq!(arr[0]["issue"], "TBD");
        assert_eq!(arr[0]["description"], "API service");
        assert_eq!(arr[0]["task"], "Implement the REST API");
        assert_eq!(arr[0]["recipe"], "default-workflow");
        assert_eq!(arr[0]["branch"], "feat/orch-1-api-service");

        // Slug strips special chars and lowercases.
        assert_eq!(arr[1]["branch"], "feat/orch-2-web-ui-front-end");

        // Tempfile is restrictive on Unix.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::metadata(&path).unwrap().permissions();
            assert_eq!(perms.mode() & 0o777, 0o600, "tempfile must be 0600");
        }

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn development_workstream_missing_recipe_routes_to_default_workflow() {
        let parsed = workstreams_config_from(
            r#"{
                "task_type": "Development",
                "workstreams": [
                    {"name": "dev task", "classification": "Development", "description": "Add tests"}
                ]
            }"#,
        );

        assert_eq!(parsed[0]["recipe"], "default-workflow");
    }

    #[test]
    fn development_workstream_empty_recipe_routes_to_default_workflow() {
        let parsed = workstreams_config_from(
            r#"{
                "task_type": "Development",
                "workstreams": [
                    {"name": "dev task", "classification": "Development", "recipe": "   "}
                ]
            }"#,
        );

        assert_eq!(parsed[0]["recipe"], "default-workflow");
    }

    #[test]
    fn development_workstream_wrong_recipe_routes_to_default_workflow() {
        let parsed = workstreams_config_from(
            r#"{
                "task_type": "Development",
                "workstreams": [
                    {
                        "name": "dev task",
                        "classification": "Development",
                        "description": "Refactor src/orch.rs",
                        "recipe": "investigation-workflow"
                    }
                ]
            }"#,
        );

        assert_eq!(parsed[0]["recipe"], "default-workflow");
    }

    #[test]
    fn unclassified_workstream_under_top_level_development_routes_to_default_workflow() {
        let parsed = workstreams_config_from(
            r#"{
                "task_type": "Development",
                "workstreams": [
                    {"name": "dev task", "recipe": "investigation-workflow"}
                ]
            }"#,
        );

        assert_eq!(parsed[0]["recipe"], "default-workflow");
    }

    #[test]
    fn non_development_workstream_recipes_are_preserved() {
        let parsed = workstreams_config_from(
            r#"{
                "task_type": "Development",
                "workstreams": [
                    {"name": "research", "classification": "Investigation", "recipe": "investigation-workflow"},
                    {"name": "answer", "classification": "Q&A", "recipe": "qa-workflow"},
                    {"name": "ops", "classification": "Operations", "recipe": "ops-sentinel-workflow"},
                    {"name": "consensus", "classification": "Consensus", "recipe": "consensus-workflow"}
                ]
            }"#,
        );

        assert_eq!(parsed[0]["recipe"], "investigation-workflow");
        assert_eq!(parsed[1]["recipe"], "qa-workflow");
        assert_eq!(parsed[2]["recipe"], "ops-sentinel-workflow");
        assert_eq!(parsed[3]["recipe"], "consensus-workflow");
    }

    #[test]
    fn build_workstreams_config_handles_missing_fields() {
        // No `description` → falls back to name. No `recipe` → default-workflow.
        // No `name` → "workstream-{idx}".
        let decomp = r#"{"workstreams": [{}]}"#;
        let path = build_workstreams_config_to_tempfile(decomp).unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed[0]["description"], "workstream-1");
        assert_eq!(parsed[0]["task"], "workstream-1");
        assert_eq!(parsed[0]["recipe"], "default-workflow");
        assert_eq!(parsed[0]["branch"], "feat/orch-1-workstream-1");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn build_workstreams_config_truncates_slug_to_30_chars() {
        let long = "A".repeat(80);
        let decomp = format!(r#"{{"workstreams": [{{"name": "{long}"}}]}}"#);
        let path = build_workstreams_config_to_tempfile(&decomp).unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        let branch = parsed[0]["branch"].as_str().unwrap();
        let slug = branch.strip_prefix("feat/orch-1-").unwrap();
        assert_eq!(slug.len(), 30);
        assert!(slug.chars().all(|c| c == 'a' || c == '-'));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn build_workstreams_config_empty_input_writes_empty_array() {
        let path = build_workstreams_config_to_tempfile("{}").unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed.as_array().unwrap().len(), 0);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn slug_falls_back_when_input_has_no_alphanumeric() {
        let decomp = r#"{"workstreams": [{"name": "!!!"}]}"#;
        let path = build_workstreams_config_to_tempfile(decomp).unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        // All non-alphanum become '-', then trim → empty → fallback "ws-{idx}".
        assert_eq!(parsed[0]["branch"], "feat/orch-1-ws-1");
        std::fs::remove_file(&path).ok();
    }

    // --- extract_field --------------------------------------------------------

    #[test]
    fn extract_field_returns_string_without_quotes() {
        assert_eq!(
            extract_field(r#"{"task_type": "Investigation"}"#, "task_type"),
            Some("Investigation".to_string())
        );
    }

    #[test]
    fn extract_field_returns_none_for_missing_key() {
        assert_eq!(extract_field(r#"{"a": 1}"#, "task_type"), None);
    }

    #[test]
    fn extract_field_returns_none_for_invalid_json() {
        assert_eq!(extract_field("not json", "task_type"), None);
        assert_eq!(extract_field("[1,2,3]", "task_type"), None); // array, not object
    }

    #[test]
    fn extract_field_handles_scalars_and_nested_values() {
        assert_eq!(extract_field(r#"{"n": 42}"#, "n"), Some("42".to_string()));
        assert_eq!(
            extract_field(r#"{"b": true}"#, "b"),
            Some("true".to_string())
        );
        assert_eq!(
            extract_field(r#"{"o": {"x": 1}}"#, "o"),
            Some(r#"{"x":1}"#.to_string())
        );
    }

    #[test]
    fn extract_field_treats_explicit_null_as_absent_so_the_default_applies() {
        // Issue #1337: `--default true` is the fail-safe the loop-health gate
        // leans on hardest. A null that returned Some("") walked straight past
        // it, so `{"terminal_refusal": null}` read as "the guard did not fire".
        assert_eq!(extract_field(r#"{"x": null}"#, "x"), None);
        assert_eq!(
            extract_field(r#"{"terminal_refusal": null}"#, "terminal_refusal"),
            None
        );
        // An empty STRING is still a value the caller asked for, not an absence.
        assert_eq!(
            extract_field(r#"{"x": ""}"#, "x"),
            Some(String::new()),
            "an explicit empty string must NOT be swallowed by the default"
        );
    }

    // --- reclassify_task_type (#269) -----------------------------------------

    #[test]
    fn reclassify_promotes_issue_269_repro_task() {
        let task = "Add an agentic disk-cleanup loop to the Simard OODA daemon. \
            Requirements: (1) Extend src/cmd_cleanup.rs with a new function \
            agentic_wip_safe_cleanup() that runs as part of the existing per-cycle \
            handle_cleanup() pipeline. (2) Trigger only when free disk on /home or \
            /tmp is below 25 GB. (3) WIP-aware: scan all git worktrees... \
            (5) Add unit tests for free_disk_gb()... \
            (6) Open a PR titled fix(cleanup): agentic WIP-safe target dir reclamation";
        assert_eq!(reclassify_task_type("Operations", task), "Development");
    }

    #[test]
    fn reclassify_promotes_on_pr_mention_alone() {
        assert_eq!(
            reclassify_task_type("Operations", "Open a PR that does X"),
            "Development"
        );
        assert_eq!(
            reclassify_task_type("Operations", "Please open a pull request for the fix"),
            "Development"
        );
    }

    #[test]
    fn reclassify_promotes_on_test_mention_alone() {
        assert_eq!(
            reclassify_task_type("Operations", "Write unit tests for the parser"),
            "Development"
        );
        assert_eq!(
            reclassify_task_type("Operations", "Add tests covering the new helper"),
            "Development"
        );
    }

    #[test]
    fn reclassify_promotes_on_verb_plus_file_path() {
        assert_eq!(
            reclassify_task_type("Operations", "Extend src/foo.rs with a new function"),
            "Development"
        );
        assert_eq!(
            reclassify_task_type("Operations", "Add a flag to crates/cli/src/main.rs"),
            "Development"
        );
        assert_eq!(
            reclassify_task_type("Operations", "Implement amplifier-bundle/recipes/foo.yaml"),
            "Development"
        );
    }

    #[test]
    fn reclassify_promotes_on_three_plus_numbered_reqs_with_dev_verb() {
        let task = "Add a feature: (1) parse input, (2) validate config, \
                    (3) emit metrics, (4) record audit log";
        assert_eq!(reclassify_task_type("Operations", task), "Development");
    }

    #[test]
    fn reclassify_does_not_promote_pure_ops_tasks() {
        for task in [
            "Run cargo test and report failures",
            "Show me git status",
            "Clean up tmp files",
            "Delete the build directory",
            "List the files in target/",
        ] {
            assert_eq!(
                reclassify_task_type("Operations", task),
                "Operations",
                "task should stay Operations: {task:?}",
            );
        }
    }

    #[test]
    fn reclassify_passes_through_non_operations_types() {
        let dev_signal_task = "Add unit tests and open a PR for src/foo.rs";
        assert_eq!(reclassify_task_type("Q&A", dev_signal_task), "Q&A");
        assert_eq!(
            reclassify_task_type("Investigation", dev_signal_task),
            "Investigation"
        );
        assert_eq!(
            reclassify_task_type("Development", dev_signal_task),
            "Development"
        );
    }

    #[test]
    fn reclassify_normalises_unknown_current_to_development() {
        assert_eq!(
            reclassify_task_type("garbage", "do anything"),
            "Development"
        );
    }

    #[test]
    fn reclassify_does_not_promote_three_reqs_without_dev_verb() {
        let task = "Please do the following: (1) check disk space, \
                    (2) report free GB, (3) print the result";
        assert_eq!(reclassify_task_type("Operations", task), "Operations");
    }
}
