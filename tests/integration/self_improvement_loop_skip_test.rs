//! Integration tests: graceful-skip contract for `self-improvement-loop.yaml`.
//!
//! The recipe `amplifier-bundle/recipes/self-improvement-loop.yaml` keeps the
//! eval steps runnable on installs where the native progressive eval runner is
//! unavailable by emitting explicit skipped JSON.
//!
//! These TDD tests codify the contract:
//!
//!   1. The recipe YAML parses cleanly and embedded bash bodies are syntactically valid.
//!   2. Both eval steps write a `[skip] ...` warning to stderr and a
//!      `{"skipped":true,"reason":"..."}` payload to stdout.
//!   3. No interpreter-backed eval invocations remain in `amplifier-bundle/recipes/`.
//!
//! The tests are deliberately pure-Rust string analysis + a small bash-driven
//! skip-path simulation; they do not require a real Python interpreter or the
//! recipe runner.

use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|p| p.join("amplifier-bundle").is_dir())
        .expect("repo root with amplifier-bundle/")
        .to_path_buf()
}

fn recipe_path() -> PathBuf {
    repo_root().join("amplifier-bundle/recipes/self-improvement-loop.yaml")
}

fn read_recipe() -> String {
    std::fs::read_to_string(recipe_path()).expect("recipe is readable")
}

// ---------------------------------------------------------------------------
// 1. Structural / syntactic checks
// ---------------------------------------------------------------------------

#[test]
fn recipe_yaml_parses() {
    let yaml = read_recipe();
    serde_yaml::from_str::<serde_yaml::Value>(&yaml)
        .expect("self-improvement-loop.yaml must parse as YAML");
}

#[test]
fn recipe_passes_bash_n() {
    // Concatenate every `run:` block's bash body and feed to `bash -n` so we
    // catch syntax errors in the embedded shell — same gate PR #348 used.
    let yaml = read_recipe();
    let doc: serde_yaml::Value = serde_yaml::from_str(&yaml).expect("yaml");
    let mut bodies = String::new();
    collect_run_bodies(&doc, &mut bodies);
    assert!(
        !bodies.is_empty(),
        "expected to extract at least one bash body from recipe"
    );

    let tmp = tempfile::NamedTempFile::new().expect("tempfile");
    std::fs::write(tmp.path(), &bodies).unwrap();
    let out = Command::new("bash")
        .arg("-n")
        .arg(tmp.path())
        .output()
        .expect("bash -n");
    assert!(
        out.status.success(),
        "bash -n failed on embedded recipe bodies:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn collect_run_bodies(v: &serde_yaml::Value, out: &mut String) {
    match v {
        serde_yaml::Value::Mapping(m) => {
            for (k, val) in m {
                if let Some(key) = k.as_str()
                    && (key == "run" || key == "command")
                    && let Some(s) = val.as_str()
                {
                    let cleaned = strip_mustache(s);
                    out.push_str(&cleaned);
                    out.push_str("\n# --- next run block ---\n");
                    continue;
                }
                collect_run_bodies(val, out);
            }
        }
        serde_yaml::Value::Sequence(s) => {
            for item in s {
                collect_run_bodies(item, out);
            }
        }
        _ => {}
    }
}

fn strip_mustache(s: &str) -> String {
    // Replace `{{ ... }}` with a benign placeholder identifier so bash -n
    // sees a valid token. Recipe placeholders only appear in argument
    // positions, so a literal works.
    let re = regex::Regex::new(r"\{\{[^}]*\}\}").unwrap();
    re.replace_all(s, "PLACEHOLDER").to_string()
}

// ---------------------------------------------------------------------------
// 2. Native skip presence
// ---------------------------------------------------------------------------

#[test]
fn run_baseline_eval_has_native_skip() {
    let yaml = read_recipe();
    assert!(
        yaml.contains(
            "[skip] native progressive eval runner not available; skipping run-baseline-eval"
        ),
        "run-baseline-eval must emit a [skip] stderr warning"
    );

    // Synthetic JSON payload to stdout.
    assert!(
        yaml.contains(r#"{"skipped":true,"reason":"native progressive eval runner not available; step skipped"}"#),
        "run-baseline-eval must emit the synthetic skipped JSON payload"
    );
}

#[test]
fn re_eval_affected_has_probe_and_skip() {
    let yaml = read_recipe();
    assert!(
        yaml.contains(
            "[skip] native progressive eval runner not available; skipping re-eval-affected"
        ),
        "re-eval-affected must emit a [skip] stderr warning"
    );
}

#[test]
fn skip_path_uses_exit_zero() {
    // The graceful-skip contract demands `exit 0` on the skip path so downstream
    // recipe steps can branch on `.skipped == true` rather than aborting.
    let yaml = read_recipe();
    // Find the two skip blocks and assert each ends with `exit 0` shortly after
    // the JSON payload line.
    let payload =
        r#"{"skipped":true,"reason":"native progressive eval runner not available; step skipped"}"#;
    let mut search_from = 0usize;
    let mut found = 0usize;
    while let Some(idx) = yaml[search_from..].find(payload) {
        let abs = search_from + idx;
        // Look in the next ~120 chars for `exit 0`.
        let window_end = (abs + 200).min(yaml.len());
        let window = &yaml[abs..window_end];
        assert!(
            window.contains("exit 0"),
            "skip-payload at offset {abs} must be followed by `exit 0`"
        );
        found += 1;
        search_from = abs + payload.len();
    }
    assert_eq!(
        found, 2,
        "expected exactly two skip-path occurrences (one per eval step); found {found}"
    );
}

// ---------------------------------------------------------------------------
// 3. Audit: no interpreter-backed eval calls anywhere in amplifier-bundle/recipes/.
// ---------------------------------------------------------------------------

#[test]
fn no_interpreter_eval_invocations_in_recipes() {
    let recipes_dir = repo_root().join("amplifier-bundle/recipes");
    let mut offenders: Vec<String> = Vec::new();

    for entry in std::fs::read_dir(&recipes_dir).expect("recipes dir") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|s| s.to_str()) != Some("yaml") {
            continue;
        }
        let content = std::fs::read_to_string(&path).unwrap();
        for (lineno, line) in content.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with('#') {
                continue;
            }
            let is_interpreter_eval = line.contains("python -m amplihack.eval")
                || line.contains("python3 -m amplihack.eval")
                || line.contains("\"$PY\" -m amplihack.eval")
                || line.contains("import amplihack.eval");
            if is_interpreter_eval {
                offenders.push(format!(
                    "{}:{}: interpreter eval invocation: {}",
                    path.display(),
                    lineno + 1,
                    line.trim()
                ));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "found interpreter eval invocations:\n{}",
        offenders.join("\n")
    );
}

// ---------------------------------------------------------------------------
// 4. Behavioural simulation: run the native skip contract, asserting exit 0,
//    [skip] on stderr, and the JSON sentinel on stdout.
// ---------------------------------------------------------------------------

#[test]
fn skip_path_simulation_emits_sentinel_and_exits_zero() {
    let script = r#"
set -eu
echo "[skip] native progressive eval runner not available; skipping run-baseline-eval" >&2
echo '{"skipped":true,"reason":"native progressive eval runner not available; step skipped"}'
"#;

    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), script).unwrap();

    let isolated = tempfile::tempdir().unwrap();
    let path = isolated.path().to_string_lossy().into_owned();
    let bash = if std::path::Path::new("/bin/bash").exists() {
        "/bin/bash"
    } else {
        "/usr/bin/bash"
    };

    let out = Command::new(bash)
        .arg(tmp.path())
        .env("PATH", &path)
        .output()
        .expect("bash run");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        out.status.success(),
        "skip path must exit 0; got {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        stdout,
        stderr
    );
    assert!(
        stderr.contains("[skip] native progressive eval runner not available"),
        "stderr must contain [skip] warning; got: {stderr}"
    );
    assert!(
        stdout.contains(r#""skipped":true"#),
        "stdout must contain skipped sentinel JSON; got: {stdout}"
    );
    assert!(
        stdout.contains(r#""reason":"native progressive eval runner not available; step skipped""#),
        "stdout must contain reason field; got: {stdout}"
    );
}
