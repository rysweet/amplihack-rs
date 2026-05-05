//! Integration tests: `sdk-comparison.yaml` graceful skip when the native SDK
//! eval runner is unavailable.
//!
//! Refs: #248, #285
//!
//! These tests follow TDD methodology: they specify the contract the recipe
//! modification must satisfy. They FAIL against the unmodified recipe and
//! PASS once the recipe is updated per the design spec.
//!
//! # Contract
//!
//! 1. `sdk-comparison.yaml` parses as valid YAML.
//! 2. Each of the four eval steps (`eval-mini`, `eval-claude`, `eval-copilot`,
//!    `eval-microsoft`) writes a `[skip]` warning to **stderr** and prints a
//!    single fallback JSON object `{"sdk":"<name>","skipped":true,...}` to
//!    **stdout**.
//! 4. Each modified `command:` block passes `bash -n` syntax check.
//! 5. End-to-end behaviour: with no Python on PATH, executing the extracted
//!    command body produces exactly one JSON object on stdout with the
//!    expected `sdk` field, an `[skip]` line on stderr, and exit code 0.

use std::path::PathBuf;
use std::process::{Command, Stdio};

fn repo_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // tests/  ->  workspace root candidate
    path.pop();
    path
}

fn recipe_path() -> PathBuf {
    repo_root().join("amplifier-bundle/recipes/sdk-comparison.yaml")
}

fn read_recipe() -> String {
    std::fs::read_to_string(recipe_path()).expect("sdk-comparison.yaml must exist")
}

const SDKS: &[&str] = &["mini", "claude", "copilot", "microsoft"];

// ---------------------------------------------------------------------------
// TC-01: Recipe still parses as YAML after edits.
// ---------------------------------------------------------------------------
#[test]
fn tc01_recipe_parses_as_yaml() {
    let text = read_recipe();
    let parsed: serde_yaml::Value =
        serde_yaml::from_str(&text).expect("sdk-comparison.yaml must remain valid YAML");
    assert!(parsed.get("name").is_some(), "recipe must have `name` key");
    assert!(
        parsed.get("steps").is_some(),
        "recipe must have `steps` key"
    );
}

// ---------------------------------------------------------------------------
// TC-02: Native skip comment block is present in the header.
// ---------------------------------------------------------------------------
#[test]
fn tc02_native_skip_header_comment_present() {
    let text = read_recipe();
    let header: String = text
        .lines()
        .take_while(|l| !l.trim_start().starts_with("name:"))
        .collect::<Vec<_>>()
        .join("\n");

    let lc = header.to_lowercase();
    assert!(
        lc.contains("native") && lc.contains("skip"),
        "header must explain the native graceful-skip contract"
    );
}

// ---------------------------------------------------------------------------
// TC-03: Each eval step contains a native skip block.
// ---------------------------------------------------------------------------
#[test]
fn tc03_each_eval_step_has_native_skip() {
    let text = read_recipe();
    for sdk in SDKS {
        let block = extract_step_command(&text, &format!("eval-{sdk}"))
            .unwrap_or_else(|| panic!("step `eval-{sdk}` must exist with a `command:` body"));

        assert!(
            block.contains("[skip]"),
            "eval-{sdk}: must emit a `[skip]` warning when native runner is unavailable"
        );
        assert!(
            block.contains(">&2"),
            "eval-{sdk}: warning must be routed to stderr (>&2)"
        );
        assert!(
            block.contains(&format!("\"sdk\":\"{sdk}\""))
                || block.contains(&format!("\"sdk\": \"{sdk}\"")),
            "eval-{sdk}: skip path must emit fallback JSON with sdk={sdk}"
        );
        assert!(
            block.contains("\"skipped\":true") || block.contains("\"skipped\": true"),
            "eval-{sdk}: fallback JSON must mark skipped=true"
        );
    }
}

// ---------------------------------------------------------------------------
// TC-04: Each modified command block passes `bash -n` syntax check.
// ---------------------------------------------------------------------------
#[test]
fn tc04_each_eval_command_passes_bash_n() {
    let text = read_recipe();
    for sdk in SDKS {
        let block = extract_step_command(&text, &format!("eval-{sdk}"))
            .unwrap_or_else(|| panic!("step `eval-{sdk}` must exist"));

        // Substitute recipe placeholders with safe literals so bash -n is meaningful.
        let stubbed = block
            .replace("{{levels}}", "L1 L2")
            .replace("{{loops_per_sdk}}", "1")
            .replace("{{output_dir}}", "/tmp/eval_test_out");

        let mut child = Command::new("bash")
            .arg("-n")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("bash must be available");
        use std::io::Write;
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(stubbed.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "eval-{sdk}: bash -n failed.\nstderr: {}\n--- script ---\n{}",
            String::from_utf8_lossy(&out.stderr),
            stubbed
        );
    }
}

// ---------------------------------------------------------------------------
// TC-05: The extracted command emits the skip JSON to stdout, an `[skip]`
// warning to stderr, and exits 0.
// ---------------------------------------------------------------------------
#[test]
fn tc05_skip_behaviour_native_runner_unavailable() {
    let text = read_recipe();
    for sdk in SDKS {
        let block = extract_step_command(&text, &format!("eval-{sdk}"))
            .unwrap_or_else(|| panic!("step `eval-{sdk}` must exist"));

        let stubbed = block
            .replace("{{levels}}", "L1")
            .replace("{{loops_per_sdk}}", "1")
            .replace("{{output_dir}}", "/tmp/eval_test_out_skip");

        // Resolve absolute path to bash since env_clear() drops PATH.
        let bash_path = ["/bin/bash", "/usr/bin/bash"]
            .iter()
            .find(|p| std::path::Path::new(p).exists())
            .copied()
            .expect("bash must exist at /bin/bash or /usr/bin/bash");

        let mut child = Command::new(bash_path)
            .env_clear()
            .env("PATH", "")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("bash must be available");
        use std::io::Write;
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(stubbed.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);

        assert!(
            out.status.success(),
            "eval-{sdk}: skip path must exit 0 (got {:?}).\nstderr:\n{}",
            out.status.code(),
            stderr
        );
        assert!(
            stderr.contains("[skip]"),
            "eval-{sdk}: stderr must contain `[skip]`. stderr was:\n{stderr}"
        );

        // Last JSON object on stdout must be the fallback for this sdk.
        let last_brace = stdout.rfind('{').unwrap_or_else(|| {
            panic!("eval-{sdk}: stdout must contain a JSON object. stdout:\n{stdout}")
        });
        let json_tail = &stdout[last_brace..];
        let parsed: serde_json::Value =
            serde_json::from_str(json_tail.trim()).unwrap_or_else(|e| {
                panic!("eval-{sdk}: tail of stdout must parse as JSON ({e}). tail:\n{json_tail}")
            });
        assert_eq!(
            parsed.get("sdk").and_then(|v| v.as_str()),
            Some(*sdk),
            "eval-{sdk}: fallback JSON `sdk` field mismatch. parsed: {parsed:?}"
        );
        assert!(
            parsed.get("error").is_some(),
            "eval-{sdk}: fallback JSON must include `error` field"
        );
        assert_eq!(
            parsed.get("skipped").and_then(|v| v.as_bool()),
            Some(true),
            "eval-{sdk}: fallback JSON must include skipped=true"
        );
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract the body of the `command: |` block for the given step id.
///
/// This is a lightweight YAML walker that avoids structural surgery: it
/// finds the line `- id: "<step_id>"`, then finds the subsequent
/// `command: |` line, then collects the indented block following it.
fn extract_step_command(text: &str, step_id: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let id_needle_dq = format!("id: \"{step_id}\"");
    let id_needle_sq = format!("id: '{step_id}'");
    let id_needle_bare = format!("id: {step_id}");
    let mut i = 0;
    while i < lines.len() {
        let l = lines[i];
        if l.contains(&id_needle_dq) || l.contains(&id_needle_sq) || l.contains(&id_needle_bare) {
            // Find the next `command: |` (or `command: |-`) within this step.
            let mut j = i + 1;
            while j < lines.len() {
                let lj = lines[j].trim_start();
                if lj.starts_with("- id:") {
                    return None;
                }
                if lj.starts_with("command:") {
                    // Determine block indent from the first non-empty body line.
                    let mut k = j + 1;
                    let mut body_indent: Option<usize> = None;
                    let mut body = String::new();
                    while k < lines.len() {
                        let lk = lines[k];
                        if lk.trim().is_empty() {
                            body.push('\n');
                            k += 1;
                            continue;
                        }
                        let indent = lk.len() - lk.trim_start().len();
                        match body_indent {
                            None => {
                                body_indent = Some(indent);
                                body.push_str(&lk[indent.min(lk.len())..]);
                                body.push('\n');
                            }
                            Some(bi) => {
                                if indent < bi {
                                    return Some(body);
                                }
                                body.push_str(&lk[bi.min(lk.len())..]);
                                body.push('\n');
                            }
                        }
                        k += 1;
                    }
                    return Some(body);
                }
                j += 1;
            }
            return None;
        }
        i += 1;
    }
    None
}
