//! Issue #1883: `inherit_for_run`, which `amplihack recipe run` calls once per
//! run to work out the effort and model its steps inherit (D1, D3, D7, D8).
//!
//! `/proc` is a synthetic tree under a private temp dir, installed for the
//! calling thread only with `procfs::override_proc_root` (issue #2196), so the
//! developer's own `claude` session (which is usually an ancestor of
//! `cargo test`) is never read and no other test sees the tree. These tests
//! never set `AMPLIHACK_TEST_PROC_ROOT` or the test home guard. Every variable
//! the function reads is set or cleared under the shared environment lock and
//! restored afterwards.

#![cfg(unix)]

use super::inherit_inputs::inherit_for_run;
use crate::commands::launch::inherit::Inherited;
use crate::test_support::with_env;
use amplihack_utils::secure_files::euid;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::path::Path;

/// A synthetic `/proc`: `self` is pid 100 (`amplihack recipe run`), whose
/// parent is pid 50 running `ancestor`, whose parent is pid 10 (`bash`).
/// With `ancestor` empty, pid 50 is a `bash` too, so there is no claude.
struct FakeProc {
    _tmp: tempfile::TempDir,
    root: std::path::PathBuf,
}

fn add(root: &Path, pid: u32, ppid: u32, comm: &str, argv: &[&str]) {
    let d = root.join(pid.to_string());
    fs::create_dir_all(&d).unwrap();
    fs::write(
        d.join("stat"),
        format!(
            "{pid} ({comm}) S {ppid} {pid} {pid} 0 -1 4194560 0 0 0 0 0 0 0 0 20 0 1 0 5 1000 100\n"
        ),
    )
    .unwrap();
    let uid = euid();
    fs::write(
        d.join("status"),
        format!("Name:\t{comm}\nState:\tS (x)\nUid:\t{uid}\t{uid}\t{uid}\t{uid}\n"),
    )
    .unwrap();
    let cmdline: String = argv.iter().map(|a| format!("{a}\0")).collect();
    fs::write(d.join("cmdline"), cmdline).unwrap();
    fs::write(d.join("environ"), "PATH=/usr/bin\0").unwrap();
}

fn fake_proc(ancestor: &[&str]) -> FakeProc {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("proc");
    fs::create_dir_all(&root).unwrap();
    add(&root, 10, 1, "bash", &["bash"]);
    if ancestor.is_empty() {
        add(&root, 50, 10, "bash", &["bash"]);
    } else {
        add(&root, 50, 10, "claude", ancestor);
    }
    add(
        &root,
        100,
        50,
        "amplihack",
        &["amplihack", "recipe", "run", "default-workflow"],
    );
    std::os::unix::fs::symlink("100", root.join("self")).unwrap();
    FakeProc { _tmp: tmp, root }
}

const READ_VARS: [&str; 8] = [
    "AMPLIHACK_PARENT_EFFORT",
    "AMPLIHACK_PARENT_MODEL",
    "AMPLIHACK_PARENT_MODEL_BINARY",
    "AMPLIHACK_AGENT_BINARY",
    "CLAUDE_CODE_EFFORT_LEVEL",
    "ANTHROPIC_MODEL",
    "AMPLIHACK_SESSION_ID",
    "AMPLIHACK_RECIPE_RUN_ID",
];

/// `inherit_for_run(context, binary)` with exactly `env` set among the
/// variables it reads, no LiteLLM gateway, and `/proc` replaced by `proc` on
/// this thread.
fn run(
    proc: &FakeProc,
    context: &[(&str, &str)],
    env: &[(&str, &str)],
    binary: &str,
) -> (Inherited, Vec<String>) {
    let mut vars: Vec<(&str, Option<&OsStr>)> = READ_VARS.iter().map(|k| (*k, None)).collect();
    for k in [
        amplihack_utils::litellm_proxy::ENDPOINT_ENV,
        amplihack_utils::litellm_proxy::API_KEY_ENV,
        amplihack_utils::litellm_proxy::MODEL_ENV,
    ] {
        vars.push((k, None));
    }
    for (k, v) in env {
        vars.retain(|(name, _)| name != k);
        vars.push((k, Some(OsStr::new(v))));
    }
    let context: BTreeMap<String, String> = context
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    // This thread only, and removed when the guard drops (issue #2196).
    let _proc = crate::commands::agent::procfs::override_proc_root(&proc.root);
    with_env(&vars, || inherit_for_run(&context, binary))
}

fn inherited(effort: Option<&str>, model: Option<&str>) -> Inherited {
    Inherited {
        effort: effort.map(str::to_string),
        model: model.map(str::to_string),
    }
}

/// F3: nothing to inherit gives nothing and prints nothing.
#[test]
fn nothing_to_inherit_gives_no_values_and_no_lines() {
    let p = fake_proc(&[]);
    let (got, lines) = run(&p, &[], &[], "claude");
    assert_eq!(got, Inherited::default());
    assert!(lines.is_empty(), "{lines:?}");
}

#[test]
fn context_keys_effort_and_model_are_the_first_tier() {
    let p = fake_proc(&["claude", "--effort", "low", "--model", "claude-haiku-4-5"]);
    let (got, lines) = run(
        &p,
        &[("effort", "medium"), ("model", "claude-sonnet-4-5")],
        &[
            ("AMPLIHACK_PARENT_EFFORT", "high"),
            ("AMPLIHACK_PARENT_MODEL", "claude-opus-5-5"),
        ],
        "claude",
    );
    assert_eq!(got, inherited(Some("medium"), Some("claude-sonnet-4-5")));
    assert_eq!(
        lines,
        vec![
            "amplihack: steps inherit effort=medium model=claude-sonnet-4-5 (from the recipe context)"
                .to_string()
        ]
    );
}

#[test]
fn parent_launch_variables_beat_the_ancestor() {
    let p = fake_proc(&["claude", "--effort", "low"]);
    let (got, lines) = run(
        &p,
        &[],
        &[
            ("AMPLIHACK_PARENT_EFFORT", "high"),
            ("AMPLIHACK_PARENT_MODEL", "claude-opus-5-5"),
        ],
        "claude",
    );
    assert_eq!(got, inherited(Some("high"), Some("claude-opus-5-5")));
    assert_eq!(
        lines,
        vec![
            "amplihack: steps inherit effort=high model=claude-opus-5-5 (from the parent amplihack launch)"
                .to_string()
        ]
    );
}

/// The `/proc` scan is skipped only when both settings are already settled.
/// A valid parent effort alone does not settle the model, so the ancestor's
/// model is still read.
#[test]
fn a_parent_effort_alone_still_reads_the_ancestor_model() {
    let p = fake_proc(&["claude", "--effort", "low", "--model", "claude-haiku-4-5"]);
    let (got, lines) = run(&p, &[], &[("AMPLIHACK_PARENT_EFFORT", "high")], "claude");
    assert_eq!(got, inherited(Some("high"), Some("claude-haiku-4-5")));
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(!lines[0].contains("ignoring"), "{lines:?}");
}

/// An invalid parent effort does not settle the effort, so the ancestor is
/// read for it, with one "ignoring" line that does not echo the value (S9).
#[test]
fn an_invalid_parent_effort_falls_back_to_the_ancestor_with_one_warning() {
    let p = fake_proc(&["claude", "--effort", "low"]);
    let (got, lines) = run(
        &p,
        &[],
        &[
            ("AMPLIHACK_PARENT_EFFORT", "hi\u{1b}gh"),
            ("AMPLIHACK_PARENT_MODEL", "claude-opus-5-5"),
        ],
        "claude",
    );
    assert_eq!(got, inherited(Some("low"), Some("claude-opus-5-5")));
    assert_eq!(
        lines,
        vec![
            "amplihack: ignoring effort from the parent amplihack launch: not a valid option value"
                .to_string(),
            "amplihack: steps inherit effort=low model=claude-opus-5-5 \
             (from the parent claude process and the parent amplihack launch)"
                .to_string(),
        ]
    );
    assert!(lines.iter().all(|l| !l.contains("hi\u{1b}gh")), "{lines:?}");
}

/// An invalid context model with no parent model leaves the model unsettled,
/// even though the context effort is valid, so the ancestor's model is used.
#[test]
fn an_invalid_context_model_is_not_settled_by_itself() {
    let p = fake_proc(&["claude", "--model", "claude-haiku-4-5"]);
    let (got, lines) = run(
        &p,
        &[("effort", "medium"), ("model", "-bad")],
        &[],
        "claude",
    );
    assert_eq!(got, inherited(Some("medium"), Some("claude-haiku-4-5")));
    assert_eq!(
        lines.iter().filter(|l| l.contains("ignoring")).count(),
        1,
        "{lines:?}"
    );
    assert!(lines[0].starts_with("amplihack: ignoring model from the recipe context"));
    assert!(lines[1].starts_with("amplihack: steps inherit effort=medium model=claude-haiku-4-5"));
}

/// D2: a plain `claude` session's argv, both spellings, last value wins.
#[test]
fn the_nearest_claude_ancestor_is_read_when_no_variable_is_set() {
    let p = fake_proc(&[
        "claude",
        "--effort",
        "low",
        "--model=claude-opus-5-5",
        "--effort=high",
    ]);
    let (got, lines) = run(
        &p,
        &[],
        &[
            ("CLAUDE_CODE_EFFORT_LEVEL", "medium"),
            ("ANTHROPIC_MODEL", "claude-sonnet-4-5"),
        ],
        "claude",
    );
    assert_eq!(got, inherited(Some("high"), Some("claude-opus-5-5")));
    assert_eq!(
        lines,
        vec![
            "amplihack: steps inherit effort=high model=claude-opus-5-5 (from the parent claude process)"
                .to_string()
        ]
    );
}

/// S7: nothing after `--` on the ancestor's argv is read.
#[test]
fn the_ancestor_argv_is_not_read_after_double_dash() {
    let p = fake_proc(&["claude", "-p", "--", "--effort", "high"]);
    let (got, lines) = run(&p, &[], &[], "claude");
    assert_eq!(got, Inherited::default());
    assert!(lines.is_empty(), "{lines:?}");
}

/// C2: under an older amplihack session (`AMPLIHACK_SESSION_ID` set) the
/// ancestor's `--model` may be amplihack's own default, so it is skipped and
/// the next tier is used. Its effort is still taken.
#[test]
fn an_ancestor_model_is_not_trusted_inside_an_amplihack_session() {
    let p = fake_proc(&["claude", "--effort", "high", "--model", "claude-opus-5[1m]"]);
    let (got, lines) = run(
        &p,
        &[],
        &[
            ("AMPLIHACK_SESSION_ID", "older-session"),
            ("ANTHROPIC_MODEL", "claude-sonnet-4-5"),
        ],
        "claude",
    );
    assert_eq!(got, inherited(Some("high"), Some("claude-sonnet-4-5")));
    assert_eq!(
        lines,
        vec![
            "amplihack: steps inherit effort=high model=claude-sonnet-4-5 \
             (from the parent claude process and ANTHROPIC_MODEL)"
                .to_string()
        ]
    );
}

/// The fallback variables are the last tier.
#[test]
fn the_fallback_variables_are_used_last() {
    let p = fake_proc(&[]);
    let (got, _) = run(
        &p,
        &[],
        &[
            ("CLAUDE_CODE_EFFORT_LEVEL", "medium"),
            ("ANTHROPIC_MODEL", "claude-sonnet-4-5"),
        ],
        "claude",
    );
    assert_eq!(got, inherited(Some("medium"), Some("claude-sonnet-4-5")));
}

/// D7: lines come in a fixed order: warnings, then "cannot be passed"
/// notices, then the summary. The summary names only values the steps
/// receive, so it is absent when every value was refused.
#[test]
fn codex_drops_effort_with_one_line_after_the_warnings() {
    let p = fake_proc(&[]);
    let (got, lines) = run(
        &p,
        &[("effort", "-bad")],
        &[("AMPLIHACK_PARENT_EFFORT", "xhigh")],
        "codex",
    );
    assert_eq!(got, Inherited::default());
    assert_eq!(
        lines,
        vec![
            "amplihack: ignoring effort from the recipe context: not a valid option value"
                .to_string(),
            "amplihack: effort 'xhigh' cannot be passed to codex; steps use codex's default"
                .to_string(),
        ]
    );
}

/// Copilot steps receive the run's effort (as `--reasoning-effort`), and the
/// summary names it (crusty review of #1892, round 2).
#[test]
fn copilot_steps_inherit_the_effort() {
    let p = fake_proc(&[]);
    let (got, lines) = run(&p, &[], &[("AMPLIHACK_PARENT_EFFORT", "high")], "copilot");
    assert_eq!(got, inherited(Some("high"), None));
    assert_eq!(
        lines,
        vec![
            "amplihack: steps inherit effort=high model=unset (from the parent amplihack launch)"
                .to_string()
        ]
    );
}

#[test]
fn codex_keeps_a_context_model_and_maps_effort() {
    let p = fake_proc(&[]);
    let (got, lines) = run(
        &p,
        &[("model", "gpt-5.1")],
        &[("AMPLIHACK_PARENT_EFFORT", "medium")],
        "codex",
    );
    assert_eq!(got, inherited(Some("medium"), Some("gpt-5.1")));
    assert_eq!(lines.len(), 1, "{lines:?}");
}

#[test]
fn codex_drops_an_inherited_claude_model() {
    let p = fake_proc(&[]);
    let (got, lines) = run(
        &p,
        &[],
        &[("AMPLIHACK_PARENT_MODEL", "claude-opus-5-5")],
        "codex",
    );
    assert_eq!(got.model, None);
    assert!(
        lines.contains(
            &"amplihack: model 'claude-opus-5-5' cannot be passed to codex; steps use codex's default"
                .to_string()
        ),
        "{lines:?}"
    );
}

/// When one value is refused and the other kept, the summary shows the
/// refused one as `unset` and its source is not named.
#[test]
fn a_refused_model_shows_as_unset_in_the_summary() {
    let p = fake_proc(&[]);
    let (got, lines) = run(
        &p,
        &[],
        &[
            ("AMPLIHACK_PARENT_EFFORT", "medium"),
            ("AMPLIHACK_PARENT_MODEL", "claude-opus-5-5"),
        ],
        "codex",
    );
    assert_eq!(got, inherited(Some("medium"), None));
    assert_eq!(
        lines,
        vec![
            "amplihack: model 'claude-opus-5-5' cannot be passed to codex; steps use codex's default"
                .to_string(),
            "amplihack: steps inherit effort=medium model=unset (from the parent amplihack launch)"
                .to_string(),
        ]
    );
}

/// No failure in this path stops a run: an unreadable process table is
/// skipped silently.
#[test]
fn an_unreadable_process_table_is_skipped_silently() {
    let p = fake_proc(&[]);
    fs::remove_file(p.root.join("self")).unwrap();
    let (got, lines) = run(&p, &[], &[("AMPLIHACK_PARENT_EFFORT", "high")], "claude");
    assert_eq!(got, inherited(Some("high"), None));
    assert_eq!(lines.len(), 1, "{lines:?}");
}

// ---------------------------------------------------------------------------
// S6: the parent variables cannot be set from recipe context
// ---------------------------------------------------------------------------

#[test]
fn parent_variables_are_reserved_context_keys_in_any_case() {
    for key in [
        "AMPLIHACK_PARENT_EFFORT",
        "AMPLIHACK_PARENT_MODEL",
        "AMPLIHACK_PARENT_MODEL_BINARY",
        "amplihack_parent_model",
        "Amplihack_Parent_Effort",
    ] {
        assert!(
            super::execute::is_reserved_context_key(key),
            "{key} must be reserved"
        );
    }
}

/// A context key named like a parent variable is not read as the run's
/// effort or model.
#[test]
fn a_parent_variable_in_the_context_is_not_an_inheritance_source() {
    let p = fake_proc(&[]);
    let (got, lines) = run(
        &p,
        &[
            ("AMPLIHACK_PARENT_EFFORT", "high"),
            ("amplihack_parent_model", "claude-opus-5-5"),
        ],
        &[],
        "claude",
    );
    assert_eq!(got, Inherited::default());
    assert!(lines.is_empty(), "{lines:?}");
}

#[test]
fn same_binary_parent_models_are_preserved_by_recipe_resolution() {
    let p = fake_proc(&[]);
    for binary in ["codex", "copilot"] {
        let (got, lines) = run(
            &p,
            &[],
            &[
                ("AMPLIHACK_PARENT_MODEL", "gpt-5.1"),
                ("AMPLIHACK_PARENT_MODEL_BINARY", binary),
            ],
            binary,
        );
        assert_eq!(got.model.as_deref(), Some("gpt-5.1"));
        assert_eq!(
            lines,
            vec![
                "amplihack: steps inherit effort=unset model=gpt-5.1 (from the parent amplihack launch)"
            ]
        );
    }
}

/// Internal callers may supply an empty context map rather than merging YAML
/// defaults. The boundary still refuses reserved YAML before any runner spawn.
#[test]
fn runner_boundary_checks_yaml_independently_of_the_supplied_context_map() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let recipe = temp.path().join("reserved.yaml");
    for key in [
        "amplihack_parent_effort",
        "AMPLIHACK_PARENT_MODEL",
        "Amplihack_Parent_Model_Binary",
    ] {
        fs::write(
            &recipe,
            format!("name: reserved\ncontext: {{{key}: low}}\nsteps: []\n"),
        )
        .unwrap();
        let error = super::execute_recipe_via_rust_for_test(
            &recipe,
            &BTreeMap::new(),
            false,
            false,
            temp.path(),
            &[],
            None,
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("namespace cannot be set from recipe context"),
            "{error:#}"
        );
    }
}
