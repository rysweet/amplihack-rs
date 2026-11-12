//! Issue #1570 — `amplifier-bundle/tools/recipe_context_value.sh`.
//!
//! Once a recipe context outgrows the runner's environment budget, the runner
//! writes it to a 0600 JSON file named by `AMPLIHACK_CONTEXT_FILE` and unsets
//! the `RECIPE_VAR_*` variables. A step that reads `$RECIPE_VAR_cycle_history`
//! then aborts under `set -u`, or silently passes on an empty history.
//! `write_context_value KEY OUT` reads whichever channel holds the value.
//!
//! Contract: docs/reference/large-payload-delivery.md, section
//! "Shell helper: recipe_context_value.sh". These tests execute the real
//! script, both sourced and run directly. They skip, with a note, when `jq`
//! is not installed, as the merge-validations tests do.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root is two levels above the crate manifest")
        .to_path_buf()
}

fn script() -> PathBuf {
    let path = repo_root().join("amplifier-bundle/tools/recipe_context_value.sh");
    assert!(
        path.is_file(),
        "issue #1570: {} must exist (reads a context value from RECIPE_VAR_* or AMPLIHACK_CONTEXT_FILE)",
        path.display()
    );
    path
}

fn jq_available() -> bool {
    let ok = Command::new("jq")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !ok {
        eprintln!("skip: `jq` not available on PATH; recipe_context_value.sh needs it");
    }
    ok
}

#[derive(Clone, Copy, Debug)]
enum Mode {
    Sourced,
    Direct,
}

const MODES: [Mode; 2] = [Mode::Sourced, Mode::Direct];

/// Run the helper with a scrubbed environment, so a `RECIPE_VAR_*` or
/// `AMPLIHACK_CONTEXT_FILE` inherited from an outer recipe run cannot leak in.
fn run(mode: Mode, cwd: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    let script = script();
    let mut cmd = Command::new("bash");
    match mode {
        Mode::Sourced => {
            cmd.arg("-c")
                .arg(
                    "set -euo pipefail; umask 022; . \"$1\"; shift; \
                     write_context_value \"$@\"",
                )
                .arg("bash")
                .arg(&script);
        }
        Mode::Direct => {
            cmd.arg("-c")
                .arg("umask 022; exec bash \"$@\"")
                .arg("bash")
                .arg(&script);
        }
    }
    cmd.args(args);
    let tmpdir = cwd.join("private-tmp");
    std::fs::create_dir_all(&tmpdir).unwrap();
    cmd.env_clear()
        .env(
            "PATH",
            std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into()),
        )
        .env("HOME", cwd)
        .env("TMPDIR", &tmpdir)
        .current_dir(cwd);
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().expect("spawn bash")
}

fn write_ctx(dir: &Path, json: &serde_json::Value) -> PathBuf {
    let path = dir.join("context.json");
    std::fs::write(&path, serde_json::to_vec(json).unwrap()).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    path
}

fn assert_ok(out: &Output, ctx: &str) {
    assert!(
        out.status.success(),
        "{ctx}: helper failed with {:?}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Run with OUT as a file and as `-`, and return both results.
fn read_both(mode: Mode, key: &str, env: &[(&str, &str)], dir: &Path) -> (Vec<u8>, Vec<u8>) {
    let out_path = dir.join(format!("out-{mode:?}"));
    let out = run(mode, dir, &[key, out_path.to_str().unwrap()], env);
    assert_ok(&out, &format!("{mode:?} {key} -> file"));
    let file = std::fs::read(&out_path).expect("OUT written");
    let out = run(mode, dir, &[key, "-"], env);
    assert_ok(&out, &format!("{mode:?} {key} -> stdout"));
    (file, out.stdout)
}

#[test]
fn recipe_var_wins_over_context_file() {
    if !jq_available() {
        return;
    }
    for mode in MODES {
        let dir = tempfile::tempdir().unwrap();
        let ctx = write_ctx(
            dir.path(),
            &serde_json::json!({"cycle_history": "from-file"}),
        );
        let env = [
            ("RECIPE_VAR_cycle_history", "from-env"),
            ("AMPLIHACK_CONTEXT_FILE", ctx.to_str().unwrap()),
        ];
        let (file, stdout) = read_both(mode, "cycle_history", &env, dir.path());
        assert_eq!(file, b"from-env");
        assert_eq!(stdout, b"from-env");
    }
}

#[test]
fn empty_recipe_var_still_wins() {
    if !jq_available() {
        return;
    }
    for mode in MODES {
        let dir = tempfile::tempdir().unwrap();
        let ctx = write_ctx(
            dir.path(),
            &serde_json::json!({"cycle_history": "from-file"}),
        );
        let env = [
            ("RECIPE_VAR_cycle_history", ""),
            ("AMPLIHACK_CONTEXT_FILE", ctx.to_str().unwrap()),
        ];
        let (file, stdout) = read_both(mode, "cycle_history", &env, dir.path());
        assert_eq!(file, b"", "{mode:?}: a set-but-empty RECIPE_VAR_ is used");
        assert_eq!(stdout, b"");
    }
}

#[test]
fn recipe_var_value_is_written_literally() {
    if !jq_available() {
        return;
    }
    let value = "$(touch pwned) `touch pwned2` %s -n\n\"q\" 'q' é € 😀\n";
    for mode in MODES {
        let dir = tempfile::tempdir().unwrap();
        let (file, stdout) = read_both(
            mode,
            "task_description",
            &[("RECIPE_VAR_task_description", value)],
            dir.path(),
        );
        assert_eq!(file, value.as_bytes());
        assert_eq!(stdout, value.as_bytes());
        assert!(!dir.path().join("pwned").exists());
        assert!(!dir.path().join("pwned2").exists());
    }
}

#[test]
fn context_file_string_is_written_raw() {
    if !jq_available() {
        return;
    }
    let value = "line one\n\"quoted\" \\backslash\\ $(touch pwned) é € 😀\n\n";
    for mode in MODES {
        let dir = tempfile::tempdir().unwrap();
        let ctx = write_ctx(dir.path(), &serde_json::json!({"cycle_history": value}));
        let env = [("AMPLIHACK_CONTEXT_FILE", ctx.to_str().unwrap())];
        let (file, stdout) = read_both(mode, "cycle_history", &env, dir.path());
        assert_eq!(
            file,
            value.as_bytes(),
            "{mode:?}: raw string, no added newline"
        );
        assert_eq!(stdout, value.as_bytes());
        assert!(!dir.path().join("pwned").exists());
    }
}

#[test]
fn context_file_non_strings_are_compact_json() {
    if !jq_available() {
        return;
    }
    let ctx_json = serde_json::json!({
        "min_cycles": 3,
        "validation_threshold": 0.85,
        "fix_all_per_cycle": true,
        "categories": ["security", "reliability"],
        "worktree_setup": {"branch": "fix/1570", "created": false},
    });
    let cases = [
        ("min_cycles", "3"),
        ("validation_threshold", "0.85"),
        ("fix_all_per_cycle", "true"),
        ("categories", r#"["security","reliability"]"#),
        ("worktree_setup", r#"{"branch":"fix/1570","created":false}"#),
    ];
    for mode in MODES {
        let dir = tempfile::tempdir().unwrap();
        let ctx = write_ctx(dir.path(), &ctx_json);
        let env = [("AMPLIHACK_CONTEXT_FILE", ctx.to_str().unwrap())];
        for (key, want) in cases {
            let (file, stdout) = read_both(mode, key, &env, dir.path());
            assert_eq!(String::from_utf8(file).unwrap(), want, "{mode:?} {key}");
            assert_eq!(
                String::from_utf8(stdout).unwrap(),
                want,
                "{mode:?} {key} stdout"
            );
        }
    }
}

// ── Dotted keys read the base key's JSON (#1561) ────────────────────────────
//
// The runner writes `RECIPE_VAR_<a>__<b>` only for fields the current object
// has, so a nested name can be a parent run's value. `write_context_value a.b`
// reads `RECIPE_VAR_a` and takes field `b` from its JSON; it never builds or
// reads `RECIPE_VAR_a__b`. See docs/howto/pass-large-recipe-context.md.

/// Run `key` to a file, expecting exit 3 and no file.
fn assert_exit_3(mode: Mode, key: &str, env: &[(&str, &str)], dir: &Path, why: &str) {
    let out_path = dir.join(format!("out-{mode:?}-exit3"));
    let out = run(mode, dir, &[key, out_path.to_str().unwrap()], env);
    assert_eq!(
        out.status.code(),
        Some(3),
        "{mode:?} {key}: {why}; stdout: {:?}, stderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !out_path.exists(),
        "{mode:?} {key}: exit 3 writes no file ({why})"
    );
    let out = run(mode, dir, &[key, "-"], env);
    assert_eq!(
        out.status.code(),
        Some(3),
        "{mode:?} {key} -> stdout: {why}"
    );
    assert!(
        out.stdout.is_empty(),
        "{mode:?} {key}: nothing printed on exit 3 ({why}), got {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("stale"),
        "{mode:?} {key}: the message never includes a value: {stderr}"
    );
}

#[test]
fn dotted_key_reads_the_base_key_json_not_the_nested_name() {
    if !jq_available() {
        return;
    }
    for mode in MODES {
        let dir = tempfile::tempdir().unwrap();
        let env = [
            (
                "RECIPE_VAR_final_requirements",
                r#"{"summary":"own-value","count":2}"#,
            ),
            ("RECIPE_VAR_final_requirements__summary", "stale-nested"), // deliberately stale (#1561)
        ];
        let (file, stdout) = read_both(mode, "final_requirements.summary", &env, dir.path());
        assert_eq!(
            String::from_utf8_lossy(&file),
            "own-value",
            "{mode:?}: the run's own base JSON wins over an inherited nested name"
        );
        assert_eq!(String::from_utf8_lossy(&stdout), "own-value");
    }
}

#[test]
fn dotted_key_with_empty_base_exits_3_and_ignores_the_nested_name() {
    if !jq_available() {
        return;
    }
    for mode in MODES {
        let dir = tempfile::tempdir().unwrap();
        let env = [
            ("RECIPE_VAR_final_requirements", ""),
            ("RECIPE_VAR_final_requirements__summary", "stale-nested"), // deliberately stale (#1561)
        ];
        assert_exit_3(
            mode,
            "final_requirements.summary",
            &env,
            dir.path(),
            "a base key set to \"\" has no field, whatever the nested name holds",
        );
    }
}

#[test]
fn dotted_key_with_unusable_base_exits_3() {
    if !jq_available() {
        return;
    }
    let bases = [
        ("not JSON", "not json {"),
        ("a JSON string", r#""summary""#),
        ("a JSON array", r#"[{"summary":"x"}]"#),
        ("a JSON number", "5"),
        ("an object without the field", r#"{"other":"x"}"#),
        (
            "two JSON documents",
            r#"{"summary":"own"} {"summary":"/evil"}"#,
        ),
    ];
    for mode in MODES {
        let dir = tempfile::tempdir().unwrap();
        for (what, base) in bases {
            let env = [
                ("RECIPE_VAR_final_requirements", base),
                ("RECIPE_VAR_final_requirements__summary", "stale-nested"), // deliberately stale (#1561)
            ];
            assert_exit_3(
                mode,
                "final_requirements.summary",
                &env,
                dir.path(),
                &format!("base is {what}"),
            );
        }
    }
}

#[test]
fn dotted_key_with_two_json_documents_exits_3() {
    if !jq_available() {
        return;
    }
    // Each document alone is an object, so a per-document check would join
    // the two fields into `own/evil` (#1561, security review L1).
    for mode in MODES {
        let dir = tempfile::tempdir().unwrap();
        assert_exit_3(
            mode,
            "a.b",
            &[("RECIPE_VAR_a", r#"{"b":"own"} {"b":"/evil"}"#)],
            dir.path(),
            "a base holding two JSON documents is not one object",
        );
        let out = run(
            mode,
            dir.path(),
            &["a.b", "-"],
            &[("RECIPE_VAR_a", r#"{"b":"own"} {"b":"/evil"}"#)],
        );
        assert_eq!(out.status.code(), Some(3), "{mode:?} a.b -: exit 3");
        assert!(out.stdout.is_empty(), "{mode:?} a.b -: prints nothing");
    }
}

#[test]
fn dotted_key_with_set_base_never_falls_back_to_the_context_file() {
    if !jq_available() {
        return;
    }
    for mode in MODES {
        let dir = tempfile::tempdir().unwrap();
        let ctx = write_ctx(
            dir.path(),
            &serde_json::json!({"final_requirements": {"summary": "stale-file"}}),
        );
        // The base key is set, so it is this run's value; the context file is
        // only for an unset base key (S-F2).
        let env = [
            ("RECIPE_VAR_final_requirements", r#"{"other":"x"}"#),
            ("AMPLIHACK_CONTEXT_FILE", ctx.to_str().unwrap()),
        ];
        assert_exit_3(
            mode,
            "final_requirements.summary",
            &env,
            dir.path(),
            "a set base key without the field does not read AMPLIHACK_CONTEXT_FILE",
        );
    }
}

#[test]
fn dotted_key_with_unset_base_uses_the_context_file_not_the_nested_name() {
    if !jq_available() {
        return;
    }
    for mode in MODES {
        let dir = tempfile::tempdir().unwrap();
        let ctx = write_ctx(
            dir.path(),
            &serde_json::json!({"final_requirements": {"summary": "from-file"}}),
        );
        let env = [
            ("RECIPE_VAR_final_requirements__summary", "stale-nested"), // deliberately stale (#1561)
            ("AMPLIHACK_CONTEXT_FILE", ctx.to_str().unwrap()),
        ];
        let (file, _) = read_both(mode, "final_requirements.summary", &env, dir.path());
        assert_eq!(String::from_utf8_lossy(&file), "from-file", "{mode:?}");

        // Unset base, no context file: exit 3, even with a nested name set.
        let env = [("RECIPE_VAR_final_requirements__summary", "stale-nested")]; // deliberately stale (#1561)
        assert_exit_3(
            mode,
            "final_requirements.summary",
            &env,
            dir.path(),
            "an unset base key never reads the nested name",
        );
    }
}

#[test]
fn dotted_key_writes_non_strings_from_the_base_key_as_compact_json() {
    if !jq_available() {
        return;
    }
    let base = r#"{"ok":false,"n":0,"list":["a","b"],"obj":{"x":1},"text":"line\nnext"}"#;
    let cases = [
        ("cfg.ok", "false"),
        ("cfg.n", "0"),
        ("cfg.list", r#"["a","b"]"#),
        ("cfg.obj", r#"{"x":1}"#),
        ("cfg.text", "line\nnext"),
    ];
    for mode in MODES {
        let dir = tempfile::tempdir().unwrap();
        let env = [("RECIPE_VAR_cfg", base)];
        for (key, want) in cases {
            let (file, stdout) = read_both(mode, key, &env, dir.path());
            assert_eq!(
                String::from_utf8_lossy(&file),
                want,
                "{mode:?} {key}: `false` and `0` are values, not missing"
            );
            assert_eq!(String::from_utf8_lossy(&stdout), want, "{mode:?} {key}");
        }
    }
}

#[test]
fn dotted_key_reads_a_deeper_path_from_the_base_key() {
    if !jq_available() {
        return;
    }
    for mode in MODES {
        let dir = tempfile::tempdir().unwrap();
        let env = [
            ("RECIPE_VAR_a", r#"{"b":{"c":"own"}}"#),
            ("RECIPE_VAR_a__b__c", "stale-nested"), // deliberately stale (#1561)
        ];
        let (file, _) = read_both(mode, "a.b.c", &env, dir.path());
        assert_eq!(String::from_utf8_lossy(&file), "own", "{mode:?}");
    }
}

#[test]
fn dotted_key_field_with_jq_syntax_is_data_not_code() {
    if !jq_available() {
        return;
    }
    for mode in MODES {
        let dir = tempfile::tempdir().unwrap();
        // `-` is a legal key character; the field reaches jq through --arg, so
        // `a.b-c` is the field named "b-c", never `.b - c`.
        let env = [("RECIPE_VAR_a", r#"{"b-c":"dash-field","b":1,"c":1}"#)];
        let (file, _) = read_both(mode, "a.b-c", &env, dir.path());
        assert_eq!(String::from_utf8_lossy(&file), "dash-field", "{mode:?}");
    }
}

#[test]
fn script_never_builds_a_nested_recipe_var_name() {
    let text = std::fs::read_to_string(script()).unwrap();
    for (i, line) in text.lines().enumerate() {
        if line.trim_start().starts_with('#') {
            continue;
        }
        assert!(
            !line.contains("//./__"),
            "recipe_context_value.sh:{}: a dotted key must not be mapped to a \
             RECIPE_VAR_<a>__<b> name (#1561): {}",
            i + 1,
            line.trim()
        );
    }
}

#[test]
fn dotted_key_reads_nested_json_path() {
    if !jq_available() {
        return;
    }
    for mode in MODES {
        let dir = tempfile::tempdir().unwrap();
        let ctx = write_ctx(
            dir.path(),
            &serde_json::json!({"final_requirements": {
                "summary": "Fix E2BIG\nacross sites",
                "explicit_requirements": ["a", "b"],
            }}),
        );
        let env = [("AMPLIHACK_CONTEXT_FILE", ctx.to_str().unwrap())];
        let (file, _) = read_both(mode, "final_requirements.summary", &env, dir.path());
        assert_eq!(file, b"Fix E2BIG\nacross sites");
        let (file, _) = read_both(
            mode,
            "final_requirements.explicit_requirements",
            &env,
            dir.path(),
        );
        assert_eq!(file, br#"["a","b"]"#);
    }
}

#[test]
fn dash_in_key_maps_to_underscore_in_var_name() {
    if !jq_available() {
        return;
    }
    for mode in MODES {
        let dir = tempfile::tempdir().unwrap();
        let (file, _) = read_both(
            mode,
            "pr-body",
            &[("RECIPE_VAR_pr_body", "env")],
            dir.path(),
        );
        assert_eq!(file, b"env");

        // The JSON path splits on `.` only, so the JSON key keeps its dash.
        let ctx = write_ctx(dir.path(), &serde_json::json!({"pr-body": "file"}));
        let env = [("AMPLIHACK_CONTEXT_FILE", ctx.to_str().unwrap())];
        let (file, _) = read_both(mode, "pr-body", &env, dir.path());
        assert_eq!(file, b"file");
    }
}

#[test]
fn absent_key_exits_3_with_documented_message_and_writes_nothing() {
    if !jq_available() {
        return;
    }
    let want = "write_context_value: context key 'cycle_history' is not available \
                (RECIPE_VAR_cycle_history is unset and AMPLIHACK_CONTEXT_FILE is not set)";
    for mode in MODES {
        let dir = tempfile::tempdir().unwrap();
        let out_path = dir.path().join("out");
        let out = run(
            mode,
            dir.path(),
            &["cycle_history", out_path.to_str().unwrap()],
            &[],
        );
        assert_eq!(out.status.code(), Some(3), "{mode:?}: absent key is exit 3");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains(want), "{mode:?}: stderr was: {stderr}");
        assert!(
            !out_path.exists(),
            "{mode:?}: an absent key never yields an empty file"
        );
    }
}

#[test]
fn key_missing_from_context_file_exits_3() {
    if !jq_available() {
        return;
    }
    for mode in MODES {
        let dir = tempfile::tempdir().unwrap();
        let ctx = write_ctx(dir.path(), &serde_json::json!({"other": "secret-value"}));
        let out_path = dir.path().join("out");
        let out = run(
            mode,
            dir.path(),
            &["cycle_history", out_path.to_str().unwrap()],
            &[("AMPLIHACK_CONTEXT_FILE", ctx.to_str().unwrap())],
        );
        assert_eq!(out.status.code(), Some(3), "{mode:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("'cycle_history'"),
            "message names the key: {stderr}"
        );
        assert!(
            !stderr.contains("secret-value"),
            "message never includes values"
        );
        assert!(!out_path.exists());
    }
}

#[test]
fn bad_key_or_missing_argument_exits_2() {
    if !jq_available() {
        return;
    }
    let bad_keys = [
        "",
        "bad key",
        "$(touch pwned)",
        ".leading-dot",
        "a/b",
        "a;b",
        "-x",
    ];
    for mode in MODES {
        let dir = tempfile::tempdir().unwrap();
        let out_path = dir.path().join("out");
        for key in bad_keys {
            let out = run(
                mode,
                dir.path(),
                &[key, out_path.to_str().unwrap()],
                &[("RECIPE_VAR_x", "v")],
            );
            assert_eq!(
                out.status.code(),
                Some(2),
                "{mode:?}: key {key:?} is a usage error"
            );
        }
        let out = run(mode, dir.path(), &["cycle_history"], &[]);
        assert_eq!(
            out.status.code(),
            Some(2),
            "{mode:?}: missing OUT is a usage error"
        );
        let out = run(mode, dir.path(), &[], &[]);
        assert_eq!(
            out.status.code(),
            Some(2),
            "{mode:?}: no arguments is a usage error"
        );
        assert!(!dir.path().join("pwned").exists());
        assert!(!out_path.exists());
    }
}

#[test]
fn unreadable_context_file_exits_1() {
    if !jq_available() {
        return;
    }
    for mode in MODES {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("no-such-context.json");
        let out_path = dir.path().join("out");
        let out = run(
            mode,
            dir.path(),
            &["cycle_history", out_path.to_str().unwrap()],
            &[("AMPLIHACK_CONTEXT_FILE", missing.to_str().unwrap())],
        );
        assert_eq!(
            out.status.code(),
            Some(1),
            "{mode:?}: unreadable context file is exit 1; stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(!out_path.exists());
    }
}

#[test]
fn value_of_200_kib_from_context_file_is_byte_exact() {
    if !jq_available() {
        return;
    }
    let unit = "{\"cycle\": 1, \"note\": \"héllo € 😀 $(x) `y`\"}\n";
    let mut value = String::new();
    while value.len() < 200 * 1024 {
        value.push_str(unit);
    }
    for mode in MODES {
        let dir = tempfile::tempdir().unwrap();
        let ctx = write_ctx(dir.path(), &serde_json::json!({"cycle_history": value}));
        let env = [("AMPLIHACK_CONTEXT_FILE", ctx.to_str().unwrap())];
        let (file, stdout) = read_both(mode, "cycle_history", &env, dir.path());
        assert_eq!(file.len(), value.len(), "{mode:?}: length");
        assert!(file == value.as_bytes(), "{mode:?}: file not byte-exact");
        assert!(
            stdout == value.as_bytes(),
            "{mode:?}: stdout not byte-exact"
        );
    }
}

#[test]
fn out_file_is_0600_and_no_temp_files_are_left() {
    if !jq_available() {
        return;
    }
    for mode in MODES {
        let dir = tempfile::tempdir().unwrap();
        let out_dir = dir.path().join("out");
        std::fs::create_dir(&out_dir).unwrap();
        let out_path = out_dir.join("value");
        let ctx = write_ctx(dir.path(), &serde_json::json!({"k": "v"}));
        let out = run(
            mode,
            dir.path(),
            &["k", out_path.to_str().unwrap()],
            &[("AMPLIHACK_CONTEXT_FILE", ctx.to_str().unwrap())],
        );
        assert_ok(&out, "mode");
        let perm = std::fs::metadata(&out_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(perm, 0o600, "{mode:?}: OUT must be 0600, got {perm:o}");
        let left: Vec<_> = std::fs::read_dir(&out_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(left.len(), 1, "{mode:?}: stray files beside OUT: {left:?}");
        let tmp_left: Vec<_> = std::fs::read_dir(dir.path().join("private-tmp"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert!(
            tmp_left.is_empty(),
            "{mode:?}: temp files left: {tmp_left:?}"
        );
    }
}

#[test]
fn script_has_no_eval_and_passes_the_key_to_jq_with_arg() {
    let text = std::fs::read_to_string(script()).unwrap();
    let code: String = text
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    let eval = regex::Regex::new(r"\beval\b").unwrap();
    assert!(
        !eval.is_match(&code),
        "recipe_context_value.sh must not use eval"
    );
    assert!(code.contains("jq"), "the context file is read with jq");
    assert!(code.contains("--arg"), "the key reaches jq through --arg");
    assert!(code.contains("umask 077"), "OUT is created under umask 077");
    assert!(
        code.contains("mktemp"),
        "OUT is written through a mktemp file"
    );
}
