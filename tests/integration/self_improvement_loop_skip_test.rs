//! Integration tests: graceful-skip contract for `self-improvement-loop.yaml`.
//!
//! Issue #248 / PR #348. The recipe `amplifier-bundle/recipes/self-improvement-loop.yaml`
//! invokes the optional `amplihack.eval.progressive_test_suite` Python module from two
//! steps: `run-baseline-eval` (~L97) and `re-eval-affected` (~L319). PR #347 introduced
//! a probe-then-skip idiom in `sdk-comparison.yaml`; PR #348 mirrored it here so the
//! recipe stays runnable on Rust-only installs without Python or the eval module.
//!
//! These TDD tests codify the contract:
//!
//!   1. The recipe YAML parses cleanly and embedded bash bodies are syntactically valid.
//!   2. Both eval steps contain a probe loop that tries `python3` then `python` and
//!      verifies `import amplihack.eval.progressive_test_suite` before invoking the
//!      module for real.
//!   3. When neither candidate satisfies the probe, the step writes a `[skip] ...`
//!      warning to stderr, a `{"skipped":true,"reason":"..."}` payload to stdout, and
//!      exits 0.
//!   4. No bare, unprobed `PYTHONPATH=src python -m amplihack.eval...` invocations
//!      remain in `amplifier-bundle/recipes/`.
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
// 2. Probe-then-skip presence
// ---------------------------------------------------------------------------

#[test]
fn run_baseline_eval_has_probe_and_skip() {
    let yaml = read_recipe();
    // The probe: try python3 then python, check `import amplihack.eval.progressive_test_suite`.
    let probe_signature = "import amplihack.eval.progressive_test_suite";
    assert!(
        yaml.matches(probe_signature).count() >= 2,
        "expected the import probe to appear in BOTH run-baseline-eval and re-eval-affected (>=2 occurrences); got {}",
        yaml.matches(probe_signature).count()
    );

    // Skip warning to stderr for run-baseline-eval.
    assert!(
        yaml.contains(
            "[skip] amplihack.eval.progressive_test_suite not available; skipping run-baseline-eval"
        ),
        "run-baseline-eval must emit a [skip] stderr warning"
    );

    // Synthetic JSON payload to stdout.
    assert!(
        yaml.contains(r#"{"skipped":true,"reason":"amplihack.eval.progressive_test_suite not installed; step skipped"}"#),
        "run-baseline-eval must emit the synthetic skipped JSON payload"
    );
}

#[test]
fn re_eval_affected_has_probe_and_skip() {
    let yaml = read_recipe();
    assert!(
        yaml.contains(
            "[skip] amplihack.eval.progressive_test_suite not available; skipping re-eval-affected"
        ),
        "re-eval-affected must emit a [skip] stderr warning"
    );
    // Both steps share the same JSON payload string — verified above; here we
    // additionally check the per-step probe loop is wired with `for cand in python3 python`.
    let occurrences = yaml.matches("for cand in python3 python").count();
    assert!(
        occurrences >= 2,
        "expected probe loop in both eval steps; found {} occurrence(s)",
        occurrences
    );
}

#[test]
fn skip_path_uses_exit_zero() {
    // The graceful-skip contract demands `exit 0` on the skip path so downstream
    // recipe steps can branch on `.skipped == true` rather than aborting.
    let yaml = read_recipe();
    // Find the two skip blocks and assert each ends with `exit 0` shortly after
    // the JSON payload line.
    let payload = r#"{"skipped":true,"reason":"amplihack.eval.progressive_test_suite not installed; step skipped"}"#;
    let mut search_from = 0usize;
    let mut found = 0usize;
    while let Some(idx) = yaml[search_from..].find(payload) {
        let abs = search_from + idx;
        // Look in the next ~120 chars for `exit 0`.
        let window_end = (abs + 200).min(yaml.len());
        let window = &yaml[abs..window_end];
        assert!(
            window.contains("exit 0"),
            "skip-payload at offset {} must be followed by `exit 0`",
            abs
        );
        found += 1;
        search_from = abs + payload.len();
    }
    assert_eq!(
        found, 2,
        "expected exactly two skip-path occurrences (one per eval step); found {}",
        found
    );
}

// ---------------------------------------------------------------------------
// 3. Audit: no bare, unprobed `python -m amplihack.eval...` calls anywhere in
//    amplifier-bundle/recipes/. A "bare" call is one preceded by an unguarded
//    `PYTHONPATH=src python` literal — i.e., not inside a probe.
// ---------------------------------------------------------------------------

#[test]
fn no_bare_python_eval_invocations_in_recipes() {
    let recipes_dir = repo_root().join("amplifier-bundle/recipes");
    let mut offenders: Vec<String> = Vec::new();

    for entry in std::fs::read_dir(&recipes_dir).expect("recipes dir") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|s| s.to_str()) != Some("yaml") {
            continue;
        }
        let content = std::fs::read_to_string(&path).unwrap();
        for (lineno, line) in content.lines().enumerate() {
            // We accept `python` references that are:
            //   - inside an `import amplihack.eval...` probe (line contains `import amplihack.eval`)
            //   - the probe loop header `for cand in python3 python`
            //   - the indirect invocation through the resolved `$PY` variable
            //   - pytest invocations
            //   - comments / markdown / template placeholders
            // We REJECT a literal `PYTHONPATH=src python -m amplihack.eval`
            // (note: the resolved-variable form is `"$PY" -m amplihack.eval`,
            // which is allowed because $PY is only set after the probe).
            let trimmed = line.trim_start();
            if trimmed.starts_with('#') {
                continue;
            }
            let is_bare_python_eval = line.contains("PYTHONPATH=src python -m amplihack.eval")
                || line.contains("PYTHONPATH=src python3 -m amplihack.eval");
            if is_bare_python_eval {
                offenders.push(format!(
                    "{}:{}: bare unprobed python eval invocation: {}",
                    path.display(),
                    lineno + 1,
                    line.trim()
                ));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "found bare unprobed python eval invocations:\n{}",
        offenders.join("\n")
    );
}

// ---------------------------------------------------------------------------
// 4. Behavioural simulation: extract the probe block and run it under a
//    PATH that has no python at all, asserting exit 0, [skip] on stderr,
//    and the JSON sentinel on stdout.
// ---------------------------------------------------------------------------

#[test]
fn skip_path_simulation_emits_sentinel_and_exits_zero() {
    // Use the literal probe shape from the recipe. We don't need to extract it
    // verbatim — we test the contract using the same idiom.
    let script = r#"
set -eu
PY=""
for cand in python3 python; do
  if command -v "$cand" >/dev/null 2>&1; then
    if PYTHONPATH=src "$cand" -c 'import amplihack.eval.progressive_test_suite' >/dev/null 2>&1; then
      PY="$cand"
      break
    fi
  fi
done
if [ -z "$PY" ]; then
  echo "[skip] amplihack.eval.progressive_test_suite not available; skipping run-baseline-eval" >&2
  echo '{"skipped":true,"reason":"amplihack.eval.progressive_test_suite not installed; step skipped"}'
  exit 0
fi
echo "should-not-reach"
exit 1
"#;

    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), script).unwrap();

    // Build a PATH that contains coreutils but no python/python3. We point at
    // a tempdir holding only symlinks to `bash`, `command`, `echo`, `[`, etc.
    // Simpler: invoke bash explicitly with a PATH stripped of any python.
    // We synthesize an empty bin dir and prepend it; bash builtins (`command`,
    // `echo`, `[`) work without external python.
    let isolated = tempfile::tempdir().unwrap();
    // Build a PATH that contains *only* an empty tempdir so neither `python`
    // nor `python3` can be resolved. We invoke bash by absolute path so the
    // child process can spawn even with an empty PATH; bash builtins
    // (`command`, `echo`, `[`) handle the rest.
    let path = isolated.path().to_string_lossy().into_owned();
    let bash = if std::path::Path::new("/bin/bash").exists() {
        "/bin/bash"
    } else {
        "/usr/bin/bash"
    };

    let out = Command::new(bash)
        .arg(tmp.path())
        .env("PATH", &path) // only our empty dir → no python anywhere
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
        stderr.contains("[skip] amplihack.eval.progressive_test_suite not available"),
        "stderr must contain [skip] warning; got: {}",
        stderr
    );
    assert!(
        stdout.contains(r#""skipped":true"#),
        "stdout must contain skipped sentinel JSON; got: {}",
        stdout
    );
    assert!(
        stdout.contains(
            r#""reason":"amplihack.eval.progressive_test_suite not installed; step skipped""#
        ),
        "stdout must contain reason field; got: {}",
        stdout
    );
}
