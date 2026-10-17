//! Issue #1567: generated workstream scripts pass the pinned Claude login
//! profile down to the nested `amplihack` launch.

use super::command_builder::{
    classic_run_script, profile_exports, recipe_run_script, sh_single_quote,
};
use super::models::Workstream;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

const PROFILE_VAR: &str = "AMPLIHACK_CLAUDE_PROFILE";
const ROOT_VAR: &str = "AMPLIHACK_CLAUDE_PROFILE_ROOT";

fn getter(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
    let map: HashMap<String, OsString> = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), OsString::from(v)))
        .collect();
    move |k| map.get(k).cloned()
}

/// The auto-switch substitution seam with no substitution (switch off).
fn keep(_inherited: &str) -> Option<String> {
    None
}

fn workstream(dir: &Path) -> Workstream {
    Workstream::new(
        1567,
        "fix/1567".to_string(),
        "Pin profile".to_string(),
        "Keep the parent profile".to_string(),
        "default-workflow".to_string(),
        dir,
        dir,
    )
}

/// Runs `f` with both profile variables set to `values` (or removed),
/// restoring the previous values afterwards.
fn with_profile_env<T>(profile: Option<&str>, root: Option<&str>, f: impl FnOnce() -> T) -> T {
    crate::test_support::with_env(
        &[
            (PROFILE_VAR, profile.map(std::ffi::OsStr::new)),
            (ROOT_VAR, root.map(std::ffi::OsStr::new)),
        ],
        f,
    )
}

#[test]
fn sh_single_quote_wraps_and_escapes_quotes() {
    assert_eq!(sh_single_quote("work"), "'work'");
    assert_eq!(sh_single_quote(""), "''");
    assert_eq!(sh_single_quote("it's"), "'it'\\''s'");
}

#[test]
fn profile_exports_writes_quoted_lines_when_set() {
    let out = profile_exports(
        getter(&[(PROFILE_VAR, "work"), (ROOT_VAR, "/home/u/profiles")]),
        keep,
    )
    .unwrap();
    assert!(
        out.contains("export AMPLIHACK_CLAUDE_PROFILE='work'\n"),
        "{out}"
    );
    assert!(
        out.contains("export AMPLIHACK_CLAUDE_PROFILE_ROOT='/home/u/profiles'\n"),
        "{out}"
    );
}

#[test]
fn profile_exports_skips_unset_or_empty_values() {
    assert_eq!(profile_exports(getter(&[]), keep).unwrap(), "");
    assert_eq!(
        profile_exports(getter(&[(PROFILE_VAR, ""), (ROOT_VAR, "")]), keep).unwrap(),
        ""
    );
    let only_name = profile_exports(getter(&[(PROFILE_VAR, "work")]), keep).unwrap();
    assert!(only_name.contains("AMPLIHACK_CLAUDE_PROFILE='work'"));
    assert!(!only_name.contains(ROOT_VAR), "{only_name}");
}

#[test]
fn profile_exports_never_exports_claude_config_dir() {
    let out = profile_exports(
        getter(&[
            (PROFILE_VAR, "work"),
            (ROOT_VAR, "/r"),
            ("CLAUDE_CONFIG_DIR", "/r/work"),
        ]),
        keep,
    )
    .unwrap();
    assert!(!out.contains("CLAUDE_CONFIG_DIR"), "{out}");
}

#[cfg(unix)]
#[test]
fn hostile_root_round_trips_through_bash_without_running_anything() {
    let tmp = tempfile::tempdir().unwrap();
    let root = "/tmp/it's $(touch pwned) `touch pwned2`\nnext";
    let exports =
        profile_exports(getter(&[(PROFILE_VAR, "work"), (ROOT_VAR, root)]), keep).unwrap();
    let script = format!("{exports}printf '%s' \"$AMPLIHACK_CLAUDE_PROFILE_ROOT\"");
    let out = Command::new("bash")
        .arg("-c")
        .arg(&script)
        .current_dir(tmp.path())
        .env_remove(PROFILE_VAR)
        .env_remove(ROOT_VAR)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(out.stdout, root.as_bytes());
    assert!(!tmp.path().join("pwned").exists());
    assert!(!tmp.path().join("pwned2").exists());
}

#[test]
fn recipe_run_script_exports_profile_when_set() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = workstream(tmp.path());
    let script = with_profile_env(Some("work"), Some("/p/it's"), || {
        recipe_run_script(&ws, "amplihack claude").unwrap()
    });
    assert!(
        script.contains("export AMPLIHACK_CLAUDE_PROFILE='work'\n"),
        "{script}"
    );
    assert!(
        script.contains("export AMPLIHACK_CLAUDE_PROFILE_ROOT='/p/it'\\''s'\n"),
        "{script}"
    );
    assert!(!script.contains("CLAUDE_CONFIG_DIR"), "{script}");
    let exports_at = script.find("AMPLIHACK_CLAUDE_PROFILE=").unwrap();
    let exec_at = script.find("exec bash launcher.sh").unwrap();
    assert!(exports_at < exec_at, "exports must precede the launch");
}

#[test]
fn classic_run_script_exports_profile_when_set() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = workstream(tmp.path());
    let script = with_profile_env(Some("work"), Some("/p"), || {
        classic_run_script(&ws, "amplihack claude").unwrap()
    });
    assert!(
        script.contains("export AMPLIHACK_CLAUDE_PROFILE='work'\n"),
        "{script}"
    );
    assert!(
        script.contains("export AMPLIHACK_CLAUDE_PROFILE_ROOT='/p'\n"),
        "{script}"
    );
    assert!(!script.contains("CLAUDE_CONFIG_DIR"), "{script}");
    let exports_at = script.find("AMPLIHACK_CLAUDE_PROFILE=").unwrap();
    let launch_at = script.find("amplihack claude --subprocess-safe").unwrap();
    assert!(exports_at < launch_at, "exports must precede the launch");
}

#[test]
fn run_scripts_have_no_profile_exports_when_unset_or_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = workstream(tmp.path());
    for (profile, root) in [(None, None), (Some(""), Some(""))] {
        let (recipe, classic) = with_profile_env(profile, root, || {
            (
                recipe_run_script(&ws, "amplihack claude").unwrap(),
                classic_run_script(&ws, "amplihack claude").unwrap(),
            )
        });
        for script in [recipe, classic] {
            assert!(!script.contains(PROFILE_VAR), "{script}");
        }
    }
}

#[cfg(unix)]
#[test]
fn profile_exports_refuses_non_utf8_values_without_printing_them() {
    use std::os::unix::ffi::OsStringExt;
    for bad_key in [PROFILE_VAR, ROOT_VAR] {
        let get = move |k: &str| match k {
            k if k == bad_key => Some(OsString::from_vec(vec![b'/', 0xff, b'x'])),
            PROFILE_VAR => Some(OsString::from("work")),
            _ => None,
        };
        let err = profile_exports(get, keep).expect_err("non-UTF-8 value must be refused");
        let msg = format!("{err:#}");
        assert!(msg.contains(bad_key), "{msg}");
        assert!(msg.contains("not valid UTF-8"), "{msg}");
        assert!(
            !msg.contains('\u{fffd}'),
            "value must not be printed: {msg}"
        );
        assert!(!msg.contains("export "), "{msg}");
    }
}

#[cfg(unix)]
#[test]
fn run_scripts_refuse_a_non_utf8_profile_root() {
    use std::os::unix::ffi::OsStrExt;
    let tmp = tempfile::tempdir().unwrap();
    let ws = workstream(tmp.path());
    let bad_root = std::ffi::OsStr::from_bytes(&[b'/', 0xff]);
    let (recipe, classic) = crate::test_support::with_env(
        &[
            (PROFILE_VAR, Some(std::ffi::OsStr::new("work"))),
            (ROOT_VAR, Some(bad_root)),
        ],
        || {
            (
                recipe_run_script(&ws, "amplihack claude"),
                classic_run_script(&ws, "amplihack claude"),
            )
        },
    );
    for result in [recipe, classic] {
        let msg = format!("{:#}", result.expect_err("script must not be built"));
        assert!(msg.contains(ROOT_VAR), "{msg}");
    }
}

// Issue #1569: with auto-switch on, the exported profile is the candidate the
// substitution seam picks; the root and everything else are unchanged.

#[test]
fn profile_exports_writes_the_substituted_profile() {
    let out = profile_exports(
        getter(&[(PROFILE_VAR, "work"), (ROOT_VAR, "/r")]),
        |inherited: &str| (inherited == "work").then(|| "spare".to_string()),
    )
    .unwrap();
    assert!(
        out.contains("export AMPLIHACK_CLAUDE_PROFILE='spare'\n"),
        "{out}"
    );
    assert!(!out.contains("'work'"), "{out}");
    assert!(
        out.contains("export AMPLIHACK_CLAUDE_PROFILE_ROOT='/r'\n"),
        "{out}"
    );
}

#[test]
fn profile_exports_does_not_consult_the_seam_without_an_inherited_profile() {
    let out = profile_exports(getter(&[(ROOT_VAR, "/r")]), |_: &str| -> Option<String> {
        panic!("substitution must only be asked about an inherited profile")
    })
    .unwrap();
    assert!(!out.contains("export AMPLIHACK_CLAUDE_PROFILE="), "{out}");
}

#[test]
fn profile_exports_quotes_the_substituted_name() {
    let out = profile_exports(getter(&[(PROFILE_VAR, "work")]), |_: &str| {
        Some("it's".to_string())
    })
    .unwrap();
    assert!(
        out.contains("export AMPLIHACK_CLAUDE_PROFILE='it'\\''s'\n"),
        "{out}"
    );
}

// -------------------------------------------------------------------------
// Issue #1570: the recipe launcher passes the workstream context with
// `--context-file`, never as `-c` arguments built by jq, `read` and word
// splitting.
//
// Contract assumed by these tests:
//
//   pub(super) fn recipe_launcher_script(recipe: &str) -> anyhow::Result<String>;
//
// The recipe name is validated before it is single-quoted into
// `RECIPE='<name>'`; a name containing a single quote is rejected. The
// launcher ends with
// `exec amplihack recipe run "$RECIPE" --context-file "$CONTEXT_JSON" --verbose`
// and `write_recipe_launcher` writes `context.json` with mode 0600.
// -------------------------------------------------------------------------

mod issue_1570_launcher {
    use super::super::command_builder::recipe_launcher_script;
    use super::super::launcher::write_recipe_launcher;
    use super::workstream;
    use crate::util::run_output_with_timeout;
    use std::process::Command;
    use std::time::Duration;

    const EXEC_LINE: &str =
        r#"exec amplihack recipe run "$RECIPE" --context-file "$CONTEXT_JSON" --verbose"#;

    fn without_profile<T>(f: impl FnOnce() -> T) -> T {
        super::with_profile_env(None, None, f)
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
        let script =
            recipe_launcher_script("amplifier-bundle/recipes/default-workflow.yaml").unwrap();
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
}
