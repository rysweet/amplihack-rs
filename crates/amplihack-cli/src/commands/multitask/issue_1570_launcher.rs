//! Issue #1570: the recipe launcher passes the workstream context with
//! `--context-file`, never as `-c` arguments built by jq, `read` and word
//! splitting.
//!
//! Contract assumed by these tests:
//!
//!   pub(super) fn recipe_launcher_script(recipe: &str) -> anyhow::Result<String>;
//!
//! The recipe name is validated before it is single-quoted into
//! `RECIPE='<name>'`; a name containing a single quote is rejected. The
//! launcher ends with
//! `exec amplihack recipe run "$RECIPE" --context-file "$CONTEXT_JSON" --verbose`
//! and `write_recipe_launcher` writes `context.json` with mode 0600.

use super::command_builder::recipe_launcher_script;
use super::command_builder_tests::{with_profile_env, workstream};
use super::launcher::write_recipe_launcher;
use crate::util::run_output_with_timeout;
use std::process::Command;
use std::time::Duration;

const EXEC_LINE: &str =
    r#"exec amplihack recipe run "$RECIPE" --context-file "$CONTEXT_JSON" --verbose"#;

fn without_profile<T>(f: impl FnOnce() -> T) -> T {
    with_profile_env(None, None, f)
}

#[test]
fn launcher_execs_recipe_run_with_context_file() {
    let script = recipe_launcher_script("default-workflow").expect("valid recipe name");
    assert!(script.contains("RECIPE='default-workflow'"), "{script}");
    assert!(script.contains(EXEC_LINE), "{script}");
    assert!(
        script.contains(r#"CONTEXT_JSON="$REPO_ROOT/context.json""#),
        "{script}"
    );
}

#[test]
fn launcher_has_no_jq_read_or_word_split_context_flags() {
    let script = recipe_launcher_script("default-workflow").unwrap();
    for forbidden in ["jq", "read -r", "CONTEXT_FLAGS", " -c "] {
        assert!(
            !script.contains(forbidden),
            "launcher must not contain {forbidden:?}:\n{script}"
        );
    }
}

#[test]
fn launcher_accepts_path_like_recipe_names() {
    let script = recipe_launcher_script("amplifier-bundle/recipes/default-workflow.yaml").unwrap();
    assert!(
        script.contains("RECIPE='amplifier-bundle/recipes/default-workflow.yaml'"),
        "{script}"
    );
}

#[test]
fn launcher_rejects_recipe_name_with_single_quote() {
    for name in ["it's", "x'; touch /tmp/pwned; '", "a\nb", ""] {
        assert!(
            recipe_launcher_script(name).is_err(),
            "recipe name {name:?} must be rejected before quoting"
        );
    }
}

#[test]
fn write_recipe_launcher_refuses_quoted_recipe_name_and_writes_no_launcher() {
    let tmp = tempfile::tempdir().unwrap();
    let mut ws = workstream(tmp.path());
    ws.recipe = "evil'; touch pwned; '".to_string();
    std::fs::create_dir_all(&ws.work_dir).unwrap();
    let result = without_profile(|| write_recipe_launcher(&ws, "amplihack claude"));
    assert!(result.is_err(), "hostile recipe name must be refused");
    assert!(!ws.work_dir.join("launcher.sh").exists());
}

#[cfg(unix)]
#[test]
fn context_json_is_written_with_mode_0600() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let ws = workstream(tmp.path());
    std::fs::create_dir_all(&ws.work_dir).unwrap();
    without_profile(|| write_recipe_launcher(&ws, "amplihack claude")).unwrap();
    let mode = std::fs::metadata(ws.work_dir.join("context.json"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600, "context.json mode {mode:o}");
}

/// Runs the generated launcher with a stub `amplihack` on `PATH` that
/// records its argv, using a 200 KiB task description.
#[cfg(unix)]
#[test]
fn launcher_end_to_end_keeps_200_kib_context_out_of_argv() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let mut ws = workstream(tmp.path());
    let unit = "task é 'q' \"dq\" $(touch pwned) `x` key=value\n";
    let mut task = String::new();
    while task.len() < 200 * 1024 {
        task.push_str(unit);
    }
    ws.task = task.clone();
    std::fs::create_dir_all(&ws.work_dir).unwrap();
    without_profile(|| write_recipe_launcher(&ws, "amplihack claude")).unwrap();

    let bin = tmp.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let record = tmp.path().join("argv.bin");
    let stub = bin.join("amplihack");
    std::fs::write(
        &stub,
        "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\0' \"$a\"; done > \"$RECORD\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();

    let mut cmd = Command::new("/bin/bash");
    cmd.arg(ws.work_dir.join("launcher.sh"))
        .current_dir(&ws.work_dir)
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env("RECORD", &record);
    let out = run_output_with_timeout(cmd, Duration::from_secs(60)).unwrap();
    assert!(
        out.status.success(),
        "launcher failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let raw = std::fs::read(&record).expect("stub amplihack must have run");
    let argv: Vec<String> = raw
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8(s.to_vec()).unwrap())
        .collect();
    for arg in &argv {
        assert!(arg.len() <= 131_072, "argv element of {} bytes", arg.len());
    }
    assert!(!tmp.path().join("pwned").exists());
    assert!(!ws.work_dir.join("pwned").exists());
    let context_path = ws.work_dir.join("context.json");
    assert_eq!(
        argv,
        vec![
            "recipe".to_string(),
            "run".to_string(),
            "default-workflow".to_string(),
            "--context-file".to_string(),
            context_path.display().to_string(),
            "--verbose".to_string(),
        ]
    );
    let context: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&context_path).unwrap()).unwrap();
    assert!(
        context["task_description"].as_str() == Some(task.as_str()),
        "task_description must arrive byte-exact"
    );
}
