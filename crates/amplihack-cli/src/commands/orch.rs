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

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use std::io::Read;

#[derive(Subcommand, Debug)]
pub enum OrchCommands {
    /// Helper utilities used by smart-orchestrator (formerly orch_helper.py).
    Helper {
        #[command(subcommand)]
        command: OrchHelperCommands,
    },
}

#[derive(Subcommand, Debug)]
pub enum OrchHelperCommands {
    /// Read stdin, print the first complete JSON object found in it.
    ///
    /// Mirrors `orch_helper.extract_json`. Tries, in order: ```json blocks,
    /// untagged ``` blocks, then a balanced-brace scan over raw prose.
    /// Prints `{}` if nothing parseable is found (matches the Python).
    ExtractJson,

    /// Read stdin, print the normalised task-type label.
    ///
    /// Mirrors `orch_helper.normalise_type`. Output is one of:
    /// `Q&A`, `Operations`, `Investigation`, `Development` (the default).
    NormaliseType,

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
            let lang = after
                .chars()
                .take_while(|c| c.is_alphanumeric())
                .collect::<String>();
            if lang.eq_ignore_ascii_case("json") {
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
        let mut stream = serde_json::Deserializer::from_str(&text[start..])
            .into_iter::<serde_json::Value>();
        if let Some(Ok(v @ serde_json::Value::Object(_))) = stream.next() {
            return Some(v);
        }
        pos = start + 1;
    }
    None
}

/// Normalise an LLM task-type label to one of `Q&A`, `Operations`,
/// `Investigation`, `Development` (the default for unknowns).
///
/// Order matters — first matching keyword wins, mirroring Python's
/// short-circuit `any()` chain.
pub fn normalise_type(raw: &str) -> &'static str {
    let t = raw.to_ascii_lowercase();
    if ["q&a", "qa", "question", "answer"].iter().any(|k| t.contains(k)) {
        return "Q&A";
    }
    if ["ops", "operation", "admin", "command"].iter().any(|k| t.contains(k)) {
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

/// Read all of stdin into a `String`. Errors out cleanly if stdin is
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
        OrchHelperCommands::ExtractJson => {
            let input = read_stdin()?;
            let value = extract_json(&input).unwrap_or(serde_json::json!({}));
            println!("{}", serde_json::to_string(&value)?);
            Ok(())
        }
        OrchHelperCommands::NormaliseType => {
            let input = read_stdin()?;
            println!("{}", normalise_type(input.trim()));
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
    }
}

/// Extract `field` from a top-level JSON object. Returns the string form of
/// scalars (without quotes) and the compact JSON encoding of objects/arrays.
/// Returns `None` if the input is not a JSON object or the field is missing.
pub fn extract_field(json: &str, field: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(json.trim()).ok()?;
    let obj = v.as_object()?;
    let val = obj.get(field)?;
    Some(match val {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
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

/// Build the workstreams-config tempfile from a decomposition JSON blob and
/// return the path. Mirrors the `create-workstreams-config` Python heredoc:
/// each entry has `issue: "TBD"`, `branch: feat/orch-{i}-{slug}`,
/// `description`, `task`, `recipe` (default `default-workflow`).
pub fn build_workstreams_config_to_tempfile(decomp: &str) -> Result<String> {
    let obj = extract_json(decomp).unwrap_or(serde_json::json!({}));
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
        let recipe = ws
            .get("recipe")
            .and_then(|v| v.as_str())
            .unwrap_or("default-workflow")
            .to_string();
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

    let (_, path) = tmp.keep().context("failed to persist workstreams tempfile")?;
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
    }
}

/// Public guard so callers can give a friendly error for invalid types.
pub fn is_known_type(label: &str) -> bool {
    matches!(label, "Q&A" | "Operations" | "Investigation" | "Development")
}

// Compile-time guarantee that `bail!` import isn't dead; used in tests below
// when input categories are validated.
#[allow(dead_code)]
fn _bail_used_in_tests() -> Result<()> {
    bail!("placeholder");
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
        assert!(extract_json("[1, 2, 3]").is_none(), "arrays alone are not objects");
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
        assert_eq!(
            extract_field(r#"{"n": 42}"#, "n"),
            Some("42".to_string())
        );
        assert_eq!(
            extract_field(r#"{"b": true}"#, "b"),
            Some("true".to_string())
        );
        assert_eq!(
            extract_field(r#"{"o": {"x": 1}}"#, "o"),
            Some(r#"{"x":1}"#.to_string())
        );
        assert_eq!(extract_field(r#"{"x": null}"#, "x"), Some(String::new()));
    }
}
