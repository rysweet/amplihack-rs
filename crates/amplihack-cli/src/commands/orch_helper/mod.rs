//! Native Rust replacement for `amplifier-bundle/tools/orch_helper.py`.
//!
//! Provides subcommands used by the smart-orchestrator recipe:
//!   - `extract-json`  — extract first JSON object from LLM output on stdin
//!   - `normalise-type` — normalise a task type string to a canonical label
//!   - `create-workstreams-config` — build workstream config from decomposition JSON
//!   - `uuid` — print 8-char hex identifier
//!   - `json-output` — build JSON object from key=value CLI args

use anyhow::{Context, Result};
use regex::Regex;
use serde_json::Value;
use std::io::Read as _;

use crate::OrchestratorHelperCommands;

/// Dispatch an `orch-helper` subcommand.
pub fn dispatch(command: OrchestratorHelperCommands) -> Result<()> {
    match command {
        OrchestratorHelperCommands::ExtractJson => run_extract_json(),
        OrchestratorHelperCommands::NormaliseType => run_normalise_type(),
        OrchestratorHelperCommands::CreateWorkstreamsConfig { input } => {
            run_create_workstreams_config(&input)
        }
        OrchestratorHelperCommands::Uuid => run_uuid(),
        OrchestratorHelperCommands::JsonOutput { pairs } => run_json_output(&pairs),
    }
}

// ---------------------------------------------------------------------------
// extract-json
// ---------------------------------------------------------------------------

/// Read stdin, extract the first valid JSON object, print it to stdout.
/// Prints `{}` on failure.
fn run_extract_json() -> Result<()> {
    let mut text = String::new();
    std::io::stdin()
        .read_to_string(&mut text)
        .context("Failed to read stdin")?;

    let obj = extract_json(&text);
    println!(
        "{}",
        serde_json::to_string(&obj).unwrap_or_else(|_| "{}".to_string())
    );
    Ok(())
}

/// Extract and parse the first complete JSON object from LLM output.
///
/// Priority:
/// 1. ```json-tagged code blocks
/// 2. Untagged ``` code blocks
/// 3. Raw JSON in prose (try parsing from each `{`)
fn extract_json(text: &str) -> Value {
    // 1. ```json-tagged code blocks
    let json_block_re = Regex::new(r"(?s)```json\s*(\{[^`]*\})\s*```").unwrap();
    for cap in json_block_re.captures_iter(text) {
        if let Some(m) = cap.get(1) {
            if let Ok(v) = serde_json::from_str::<Value>(m.as_str()) {
                if v.is_object() {
                    return v;
                }
            }
        }
    }

    // 2. Untagged code blocks
    let untagged_re = Regex::new(r"(?s)```\s*(\{[^`]*\})\s*```").unwrap();
    for cap in untagged_re.captures_iter(text) {
        if let Some(m) = cap.get(1) {
            if let Ok(v) = serde_json::from_str::<Value>(m.as_str()) {
                if v.is_object() {
                    return v;
                }
            }
        }
    }

    // 3. Fallback: scan for first valid JSON object starting at each `{`.
    // Use serde_json::Deserializer::from_str to handle } inside string values.
    let mut pos = 0;
    let bytes = text.as_bytes();
    while pos < bytes.len() {
        if let Some(offset) = text[pos..].find('{') {
            let start = pos + offset;
            let slice = &text[start..];
            let mut stream = serde_json::Deserializer::from_str(slice).into_iter::<Value>();
            if let Some(Ok(v)) = stream.next() {
                if v.is_object() {
                    return v;
                }
            }
            pos = start + 1;
        } else {
            break;
        }
    }

    Value::Object(serde_json::Map::new())
}

// ---------------------------------------------------------------------------
// normalise-type
// ---------------------------------------------------------------------------

fn run_normalise_type() -> Result<()> {
    let mut text = String::new();
    std::io::stdin()
        .read_to_string(&mut text)
        .context("Failed to read stdin")?;
    println!("{}", normalise_type(text.trim()));
    Ok(())
}

/// Normalise LLM task_type to one of: Q&A, Operations, Investigation, Development.
fn normalise_type(raw: &str) -> &'static str {
    let t = raw.to_lowercase();
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

// ---------------------------------------------------------------------------
// create-workstreams-config
// ---------------------------------------------------------------------------

fn run_create_workstreams_config(input_path: &str) -> Result<()> {
    let raw = std::fs::read_to_string(input_path)
        .with_context(|| format!("Failed to read input file: {input_path}"))?;

    let obj = extract_json(&raw);

    let workstreams = obj
        .get("workstreams")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut config: Vec<Value> = Vec::new();
    for (i, ws) in workstreams.iter().enumerate() {
        let name = ws
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or(&format!("workstream-{}", i + 1))
            .to_string();

        let slug = make_slug(&name, i);

        let description = ws
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or(&name)
            .to_string();

        let recipe = ws
            .get("recipe")
            .and_then(|v| v.as_str())
            .unwrap_or("default-workflow")
            .to_string();

        config.push(serde_json::json!({
            "issue": "TBD",
            "branch": format!("feat/orch-{}-{}", i + 1, slug),
            "description": name,
            "task": description,
            "recipe": recipe,
        }));
    }

    // Write to a temp file with the expected prefix/suffix.
    let tmp_dir = std::path::Path::new("/tmp");
    let tmp = tempfile::Builder::new()
        .prefix("smart-orch-ws-")
        .suffix(".json")
        .tempfile_in(tmp_dir)
        .context("Failed to create temp file")?;

    // Set permissions to 0o600.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o600))?;
    }

    let json_str = serde_json::to_string_pretty(&config)?;
    std::fs::write(tmp.path(), &json_str)?;

    // Persist the temp file (prevent deletion on drop).
    let path = tmp.into_temp_path().keep()?;
    println!("{}", path.display());
    Ok(())
}

/// Generate a slug from a workstream name: lowercase, non-[a-z0-9-] replaced
/// with '-', truncated to 30 chars, leading/trailing '-' stripped.
fn make_slug(name: &str, index: usize) -> String {
    let lowered = name.to_lowercase();
    let slug: String = lowered
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();

    let truncated = if slug.len() > 30 { &slug[..30] } else { &slug };

    let trimmed = truncated.trim_matches('-').to_string();
    if trimmed.is_empty() {
        format!("ws-{}", index + 1)
    } else {
        trimmed
    }
}

// ---------------------------------------------------------------------------
// uuid
// ---------------------------------------------------------------------------

fn run_uuid() -> Result<()> {
    // Read 4 random bytes from /dev/urandom and format as 8-char hex.
    let mut buf = [0u8; 4];
    let mut f =
        std::fs::File::open("/dev/urandom").context("Failed to open /dev/urandom for UUID")?;
    std::io::Read::read_exact(&mut f, &mut buf).context("Failed to read from /dev/urandom")?;
    println!("{}", hex_encode(&buf));
    Ok(())
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ---------------------------------------------------------------------------
// json-output
// ---------------------------------------------------------------------------

fn run_json_output(pairs: &[String]) -> Result<()> {
    let mut map = serde_json::Map::new();
    for pair in pairs {
        if let Some((key, value)) = pair.split_once('=') {
            map.insert(key.to_string(), Value::String(value.to_string()));
        } else {
            anyhow::bail!("Invalid key=value pair: {pair}");
        }
    }
    println!("{}", serde_json::to_string(&Value::Object(map))?);
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_json_tagged_block() {
        let input = r#"Here is the result:
```json
{"task_type": "Development", "count": 1}
```
Done."#;
        let v = extract_json(input);
        assert_eq!(v["task_type"], "Development");
        assert_eq!(v["count"], 1);
    }

    #[test]
    fn test_extract_json_untagged_block() {
        let input = r#"Result:
```
{"foo": "bar"}
```"#;
        let v = extract_json(input);
        assert_eq!(v["foo"], "bar");
    }

    #[test]
    fn test_extract_json_raw_prose() {
        let input = r#"The answer is {"key": "value"} and more text."#;
        let v = extract_json(input);
        assert_eq!(v["key"], "value");
    }

    #[test]
    fn test_extract_json_empty_fallback() {
        let input = "No JSON here at all";
        let v = extract_json(input);
        assert!(v.as_object().unwrap().is_empty());
    }

    #[test]
    fn test_extract_json_with_braces_in_strings() {
        let input = r#"{"msg": "use { and } in text", "ok": true}"#;
        let v = extract_json(input);
        assert_eq!(v["msg"], "use { and } in text");
        assert_eq!(v["ok"], true);
    }

    #[test]
    fn test_normalise_type_qa() {
        assert_eq!(normalise_type("Q&A"), "Q&A");
        assert_eq!(normalise_type("qa"), "Q&A");
        assert_eq!(normalise_type("question about X"), "Q&A");
    }

    #[test]
    fn test_normalise_type_operations() {
        assert_eq!(normalise_type("Operations"), "Operations");
        assert_eq!(normalise_type("admin task"), "Operations");
    }

    #[test]
    fn test_normalise_type_investigation() {
        assert_eq!(normalise_type("Investigation"), "Investigation");
        assert_eq!(normalise_type("research topic"), "Investigation");
        assert_eq!(normalise_type("explore the code"), "Investigation");
    }

    #[test]
    fn test_normalise_type_development() {
        assert_eq!(normalise_type("Development"), "Development");
        assert_eq!(normalise_type("implement feature"), "Development");
        assert_eq!(normalise_type("random text"), "Development");
    }

    #[test]
    fn test_make_slug() {
        assert_eq!(make_slug("Add Auth System", 0), "add-auth-system");
        assert_eq!(make_slug("Hello World!!!", 0), "hello-world");
        assert_eq!(make_slug("---", 2), "ws-3");
        // Truncation
        let long = "a".repeat(50);
        assert_eq!(make_slug(&long, 0).len(), 30);
    }

    #[test]
    fn test_hex_encode() {
        assert_eq!(hex_encode(&[0x0a, 0xff, 0x00, 0x42]), "0aff0042");
    }

    #[test]
    fn test_extract_json_tagged_block_priority() {
        // Tagged block should take priority over untagged
        let input = r#"
```
{"wrong": true}
```
```json
{"right": true}
```"#;
        let v = extract_json(input);
        assert_eq!(v["right"], true);
    }
}
