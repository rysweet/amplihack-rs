//! Integration tests: `sdk-comparison.yaml` graceful skip when
//! `amplihack.eval.sdk_eval_loop` Python module is unavailable.
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
//! 2. The header (above `name:`) contains a trade-off comment block referring
//!    to issues #248 and #285 and explaining the graceful-skip choice.
//! 3. Each of the four eval steps (`eval-mini`, `eval-claude`, `eval-copilot`,
//!    `eval-microsoft`) contains a probe that:
//!      - tries `python3` then falls back to `python`,
//!      - imports `amplihack.eval.sdk_eval_loop`,
//!      - on failure: writes a `[skip]` warning to **stderr**, prints a
//!        single fallback JSON object `{"sdk":"<name>","error":"..."}` to
//!        **stdout**, and exits 0.
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
// TC-02: Trade-off comment block is present in the header.
// ---------------------------------------------------------------------------
#[test]
fn tc02_tradeoff_header_comment_present() {
    let text = read_recipe();
    let header: String = text
        .lines()
        .take_while(|l| !l.trim_start().starts_with("name:"))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        header.contains("#248") && header.contains("#285"),
        "header must reference both issues #248 and #285. Header was:\n{header}"
    );
    let lc = header.to_lowercase();
    assert!(
        lc.contains("graceful") || lc.contains("optional") || lc.contains("skip"),
        "header must explain the graceful-skip / optional trade-off"
    );
    assert!(
        lc.contains("amplihack.eval.sdk_eval_loop") || lc.contains("sdk_eval_loop"),
        "header must name the optional python module"
    );
}

// ---------------------------------------------------------------------------
// TC-03: Each eval step contains a probe and skip block.
// ---------------------------------------------------------------------------
#[test]
fn tc03_each_eval_step_has_probe_and_skip() {
    let text = read_recipe();
    for sdk in SDKS {
        let block = extract_step_command(&text, &format!("eval-{sdk}"))
            .unwrap_or_else(|| panic!("step `eval-{sdk}` must exist with a `command:` body"));

        assert!(
            block.contains("python3") && block.contains("python"),
            "eval-{sdk}: probe must try both python3 and python. Got:\n{block}"
        );
        assert!(
            block.contains("amplihack.eval.sdk_eval_loop"),
            "eval-{sdk}: probe must import amplihack.eval.sdk_eval_loop"
        );
        assert!(
            block.contains("[skip]"),
            "eval-{sdk}: must emit a `[skip]` warning when module is unavailable"
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
            block.contains("exit 0"),
            "eval-{sdk}: skip path must exit 0 (graceful skip, not error)"
        );
        // Use if/then/else for set -e safety, not && / ||
        assert!(
            block.contains("if ") && block.contains("then") && block.contains("fi"),
            "eval-{sdk}: probe must use if/then/else (set -e safe), not && / ||"
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
// TC-05: With no python on PATH, the extracted command emits the skip JSON
// to stdout, an `[skip]` warning to stderr, and exits 0.
// ---------------------------------------------------------------------------
#[test]
fn tc05_skip_behaviour_no_python_on_path() {
    let text = read_recipe();
    for sdk in SDKS {
        let block = extract_step_command(&text, &format!("eval-{sdk}"))
            .unwrap_or_else(|| panic!("step `eval-{sdk}` must exist"));

        let stubbed = block
            .replace("{{levels}}", "L1")
            .replace("{{loops_per_sdk}}", "1")
            .replace("{{output_dir}}", "/tmp/eval_test_out_skip");

        // Build a PATH that excludes any directory containing python/python3.
        let original = std::env::var("PATH").unwrap_or_default();
        let clean: Vec<&str> = original
            .split(':')
            .filter(|d| {
                !std::path::Path::new(d).join("python").exists()
                    && !std::path::Path::new(d).join("python3").exists()
            })
            .collect();
        let clean_path = clean.join(":");

        // Resolve absolute path to bash since env_clear() drops PATH.
        let bash_path = ["/bin/bash", "/usr/bin/bash"]
            .iter()
            .find(|p| std::path::Path::new(p).exists())
            .copied()
            .expect("bash must exist at /bin/bash or /usr/bin/bash");

        let mut child = Command::new(bash_path)
            .env_clear()
            .env("PATH", &clean_path)
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
