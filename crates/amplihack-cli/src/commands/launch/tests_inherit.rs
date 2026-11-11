//! Issue #1883: unit tests for the pure inheritance logic in `inherit.rs`.
//!
//! These pin the contract the rest of the change is built on:
//!
//! - `resolve_parent`: the D3 source order for effort and model, trimming,
//!   value checks (S3, S4), the C2 trust rule for an ancestor's model, and the
//!   "ignoring" warning, which never echoes the rejected value (S9).
//! - `vet_for_binary`: what each agent binary may receive (D6, docs table).
//! - `effort_args`: what a launch adds for effort (D4, D5, C4), and the
//!   effort it exports; `export_to_child`: the exported variables (C1).
//! - `flag_value`, `codex_effort`, `codex_effort_from_argv`: argv parsing.
//! - The log lines, compared word for word with
//!   docs/reference/effort-and-model-inheritance.md.
//!
//! Every function here is pure apart from `inherited_from_env`, whose tests
//! hold the shared environment lock through `with_env`.

use super::inherit::{
    EffortArgs, InheritSource, Inherited, PARENT_EFFORT_ENV, PARENT_MODEL_ENV, ParentInputs,
    Resolved, cannot_pass_line, codex_effort, codex_effort_from_argv, copilot_effort, effort_args,
    export_to_child, flag_value, gateway_model_line, ignoring_line, inherit_summary_line,
    inherited_from_env, model_matches_binary, resolve_parent, run_printed_lines_for_run,
    vet_for_binary,
};
use crate::test_support::with_env;
use std::ffi::OsStr;
use std::process::Command;

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|a| a.to_string()).collect()
}

fn inherited(effort: Option<&str>, model: Option<&str>) -> Inherited {
    Inherited {
        effort: effort.map(str::to_string),
        model: model.map(str::to_string),
    }
}

/// Every value `option_value` must refuse, one per rule and one per unsafe
/// character range (S3). `"\u{202A}x"` and the rest are refused even though
/// they print as plain text.
const INVALID_VALUES: &[&str] = &[
    "",
    "   ",
    "-x",
    "--model",
    "a\nb",
    "a\0b",
    "a\u{1b}b",
    "a\u{200B}b",
    "a\u{200F}b",
    "a\u{202A}b",
    "a\u{202E}b",
    "a\u{2066}b",
    "a\u{2069}b",
    "a\u{FEFF}b",
];

// ---------------------------------------------------------------------------
// Constants and labels
// ---------------------------------------------------------------------------

#[test]
fn parent_variable_names_are_in_the_amplihack_namespace() {
    assert_eq!(PARENT_EFFORT_ENV, "AMPLIHACK_PARENT_EFFORT");
    assert_eq!(PARENT_MODEL_ENV, "AMPLIHACK_PARENT_MODEL");
}

/// The labels the docs' source tables promise.
#[test]
fn source_labels_match_the_docs() {
    assert_eq!(InheritSource::RecipeContext.label(), "the recipe context");
    assert_eq!(
        InheritSource::ParentLaunch.label(),
        "the parent amplihack launch"
    );
    assert_eq!(
        InheritSource::ParentClaude.label(),
        "the parent claude process"
    );
    assert_eq!(InheritSource::EffortEnv.label(), "CLAUDE_CODE_EFFORT_LEVEL");
    assert_eq!(InheritSource::ModelEnv.label(), "ANTHROPIC_MODEL");
}

// ---------------------------------------------------------------------------
// resolve_parent: D3 order
// ---------------------------------------------------------------------------

fn all_sources() -> ParentInputs<'static> {
    ParentInputs {
        context_effort: Some("ctx-effort"),
        context_model: Some("ctx-model"),
        parent_effort: Some("parent-effort"),
        parent_model: Some("parent-model"),
        ancestor_effort: Some("ancestor-effort"),
        ancestor_model: Some("ancestor-model"),
        ancestor_model_trusted: true,
        effort_env: Some("env-effort"),
        model_env: Some("env-model"),
    }
}

#[test]
fn resolve_parent_with_no_sources_inherits_nothing_and_warns_nothing() {
    let r = resolve_parent(&ParentInputs::default());
    assert_eq!(r.inherited, Inherited::default());
    assert_eq!(r.effort_source, None);
    assert_eq!(r.model_source, None);
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
}

#[test]
fn recipe_context_beats_every_other_source() {
    let r = resolve_parent(&all_sources());
    assert_eq!(
        r.inherited,
        inherited(Some("ctx-effort"), Some("ctx-model"))
    );
    assert_eq!(r.effort_source, Some(InheritSource::RecipeContext));
    assert_eq!(r.model_source, Some(InheritSource::RecipeContext));
}

#[test]
fn parent_launch_beats_the_ancestor_and_the_fallback_variables() {
    let r = resolve_parent(&ParentInputs {
        context_effort: None,
        context_model: None,
        ..all_sources()
    });
    assert_eq!(
        r.inherited,
        inherited(Some("parent-effort"), Some("parent-model"))
    );
    assert_eq!(r.effort_source, Some(InheritSource::ParentLaunch));
    assert_eq!(r.model_source, Some(InheritSource::ParentLaunch));
}

#[test]
fn ancestor_claude_beats_the_fallback_variables() {
    let r = resolve_parent(&ParentInputs {
        context_effort: None,
        context_model: None,
        parent_effort: None,
        parent_model: None,
        ..all_sources()
    });
    assert_eq!(
        r.inherited,
        inherited(Some("ancestor-effort"), Some("ancestor-model"))
    );
    assert_eq!(r.effort_source, Some(InheritSource::ParentClaude));
    assert_eq!(r.model_source, Some(InheritSource::ParentClaude));
}

#[test]
fn fallback_variables_are_the_last_tier() {
    let r = resolve_parent(&ParentInputs {
        effort_env: Some("env-effort"),
        model_env: Some("env-model"),
        ..ParentInputs::default()
    });
    assert_eq!(
        r.inherited,
        inherited(Some("env-effort"), Some("env-model"))
    );
    assert_eq!(r.effort_source, Some(InheritSource::EffortEnv));
    assert_eq!(r.model_source, Some(InheritSource::ModelEnv));
}

/// Effort and model are resolved independently: each takes its own first
/// valid source.
#[test]
fn effort_and_model_resolve_independently() {
    let r = resolve_parent(&ParentInputs {
        context_model: Some("ctx-model"),
        parent_effort: Some("high"),
        ..ParentInputs::default()
    });
    assert_eq!(r.inherited, inherited(Some("high"), Some("ctx-model")));
    assert_eq!(r.effort_source, Some(InheritSource::ParentLaunch));
    assert_eq!(r.model_source, Some(InheritSource::RecipeContext));
}

#[test]
fn values_are_trimmed_before_use() {
    let r = resolve_parent(&ParentInputs {
        context_effort: Some("  high\t"),
        parent_model: Some(" claude-opus-5-5 "),
        ..ParentInputs::default()
    });
    assert_eq!(
        r.inherited,
        inherited(Some("high"), Some("claude-opus-5-5"))
    );
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
}

/// Values that pass are used unchanged: there is no allowlist of efforts or
/// models (D9).
#[test]
fn any_valid_value_passes_unchanged() {
    let r = resolve_parent(&ParentInputs {
        context_effort: Some("max"),
        context_model: Some("claude-opus-5[1m]"),
        ..ParentInputs::default()
    });
    assert_eq!(
        r.inherited,
        inherited(Some("max"), Some("claude-opus-5[1m]"))
    );
}

// ---------------------------------------------------------------------------
// resolve_parent: value checks (S3, S4, S9)
// ---------------------------------------------------------------------------

/// An invalid value is treated as unset, the next source is tried, and one
/// "ignoring" line names the setting and the source, never the value.
#[test]
fn invalid_values_fall_through_to_the_next_source_with_one_warning() {
    for bad in INVALID_VALUES {
        let r = resolve_parent(&ParentInputs {
            context_effort: Some(bad),
            effort_env: Some("low"),
            ..ParentInputs::default()
        });
        assert_eq!(
            r.inherited.effort.as_deref(),
            Some("low"),
            "invalid effort {bad:?} was not skipped"
        );
        assert_eq!(r.effort_source, Some(InheritSource::EffortEnv), "{bad:?}");
        // D9: empty and whitespace-only values are refused like any other
        // invalid value, with exactly one warning.
        assert_eq!(
            r.warnings,
            vec![ignoring_line("effort", InheritSource::RecipeContext)],
            "{bad:?}"
        );
    }
}

#[test]
fn invalid_model_values_fall_through_too() {
    for bad in INVALID_VALUES {
        let r = resolve_parent(&ParentInputs {
            parent_model: Some(bad),
            model_env: Some("claude-sonnet-4-5"),
            ..ParentInputs::default()
        });
        assert_eq!(
            r.inherited.model.as_deref(),
            Some("claude-sonnet-4-5"),
            "invalid model {bad:?} was not skipped"
        );
        assert_eq!(r.model_source, Some(InheritSource::ModelEnv), "{bad:?}");
    }
}

#[test]
fn a_value_over_256_bytes_is_refused() {
    let long = "a".repeat(257);
    let ok = "a".repeat(256);
    let r = resolve_parent(&ParentInputs {
        context_model: Some(&long),
        ..ParentInputs::default()
    });
    assert_eq!(r.inherited.model, None);
    assert_eq!(r.warnings.len(), 1);
    let r = resolve_parent(&ParentInputs {
        context_model: Some(&ok),
        ..ParentInputs::default()
    });
    assert_eq!(r.inherited.model.as_deref(), Some(ok.as_str()));
}

/// S9: the rejected value never appears in a warning, so a secret pasted into
/// the wrong place is not printed.
#[test]
fn warnings_never_echo_the_rejected_value() {
    let secret = "-sk-ant-secret-value";
    let r = resolve_parent(&ParentInputs {
        context_model: Some(secret),
        parent_effort: Some("x\u{202E}secret"),
        ..ParentInputs::default()
    });
    assert_eq!(r.warnings.len(), 2, "{:?}", r.warnings);
    for w in &r.warnings {
        assert!(!w.contains("secret"), "warning echoed the value: {w}");
    }
}

#[test]
fn ignoring_line_matches_the_docs() {
    assert_eq!(
        ignoring_line("effort", InheritSource::RecipeContext),
        "amplihack: ignoring effort from the recipe context: not a valid option value"
    );
    assert_eq!(
        ignoring_line("model", InheritSource::ModelEnv),
        "amplihack: ignoring model from ANTHROPIC_MODEL: not a valid option value"
    );
}

// ---------------------------------------------------------------------------
// resolve_parent: C2, an ancestor's model under an older amplihack session
// ---------------------------------------------------------------------------

/// When `AMPLIHACK_SESSION_ID` is set the ancestor's `--model` may be
/// amplihack's own default, so it is not trusted. Its effort still is.
#[test]
fn an_untrusted_ancestor_model_is_skipped_but_its_effort_is_kept() {
    let r = resolve_parent(&ParentInputs {
        ancestor_effort: Some("high"),
        ancestor_model: Some("claude-opus-5[1m]"),
        ancestor_model_trusted: false,
        model_env: Some("claude-sonnet-4-5"),
        ..ParentInputs::default()
    });
    assert_eq!(r.inherited.effort.as_deref(), Some("high"));
    assert_eq!(r.effort_source, Some(InheritSource::ParentClaude));
    assert_eq!(r.inherited.model.as_deref(), Some("claude-sonnet-4-5"));
    assert_eq!(r.model_source, Some(InheritSource::ModelEnv));
    assert!(
        r.warnings.is_empty(),
        "skipping an untrusted ancestor model is silent: {:?}",
        r.warnings
    );
}

// ---------------------------------------------------------------------------
// vet_for_binary: D6
// ---------------------------------------------------------------------------

fn resolved(
    effort: Option<(&str, InheritSource)>,
    model: Option<(&str, InheritSource)>,
) -> Resolved {
    Resolved {
        inherited: inherited(effort.map(|e| e.0), model.map(|m| m.0)),
        effort_source: effort.map(|e| e.1),
        model_source: model.map(|m| m.1),
        warnings: Vec::new(),
    }
}

const CLAUDE_LIKE: [&str; 4] = ["claude", "rusty", "rustyclawd", "amplifier"];

#[test]
fn claude_compatible_binaries_take_both_values_with_no_lines() {
    let r = resolved(
        Some(("max", InheritSource::ParentLaunch)),
        Some(("claude-opus-5-5", InheritSource::ParentClaude)),
    );
    for binary in CLAUDE_LIKE {
        let (vetted, lines) = vet_for_binary(binary, &r, false, None);
        assert_eq!(
            vetted,
            inherited(Some("max"), Some("claude-opus-5-5")),
            "{binary}"
        );
        assert!(lines.is_empty(), "{binary}: {lines:?}");
    }
}

/// Under the LiteLLM gateway, Claude-compatible steps run the gateway model,
/// so the inherited model is dropped with one line and the summary does not
/// name it; the effort still passes (#1892 review).
#[test]
fn claude_compatible_binaries_drop_the_model_under_the_gateway() {
    let r = resolved(
        Some(("max", InheritSource::ParentLaunch)),
        Some(("claude-opus-5-5", InheritSource::ParentClaude)),
    );
    for binary in CLAUDE_LIKE {
        let (vetted, lines) = vet_for_binary(binary, &r, true, None);
        assert_eq!(vetted, inherited(Some("max"), None), "{binary}");
        assert_eq!(
            lines,
            vec![gateway_model_line("claude-opus-5-5", binary)],
            "{binary}"
        );
        let passed = Resolved {
            effort_source: r.effort_source,
            model_source: None,
            inherited: vetted,
            warnings: Vec::new(),
        };
        let summary = inherit_summary_line(&passed).unwrap_or_default();
        assert!(summary.contains("model=unset"), "{binary}: {summary}");
        assert!(!summary.contains("claude-opus-5-5"), "{binary}: {summary}");
    }
}

#[test]
fn nothing_resolved_vets_to_nothing_for_every_binary() {
    let r = resolved(None, None);
    for binary in ["claude", "rusty", "codex", "copilot", "something-else"] {
        let (vetted, lines) = vet_for_binary(binary, &r, false, None);
        assert_eq!(vetted, Inherited::default(), "{binary}");
        assert!(lines.is_empty(), "{binary}: {lines:?}");
    }
}

#[test]
fn codex_takes_low_medium_and_high_effort() {
    for level in ["low", "medium", "high"] {
        let r = resolved(Some((level, InheritSource::ParentLaunch)), None);
        let (vetted, lines) = vet_for_binary("codex", &r, false, None);
        assert_eq!(vetted.effort.as_deref(), Some(level));
        assert!(lines.is_empty(), "{level}: {lines:?}");
    }
}

#[test]
fn codex_drops_any_other_effort_with_one_line() {
    for level in ["max", "xhigh", "HIGH", "high,foo=bar"] {
        let r = resolved(Some((level, InheritSource::ParentLaunch)), None);
        let (vetted, lines) = vet_for_binary("codex", &r, false, None);
        assert_eq!(vetted.effort, None, "{level}");
        assert_eq!(lines, vec![cannot_pass_line("effort", level, "codex")]);
    }
}

/// An inherited model is a Claude model id; codex and copilot take only a
/// model given for this run in the recipe context.
#[test]
fn codex_and_copilot_drop_an_inherited_model_with_one_line() {
    for source in [
        InheritSource::ParentLaunch,
        InheritSource::ParentClaude,
        InheritSource::ModelEnv,
    ] {
        for binary in ["codex", "copilot"] {
            let r = resolved(None, Some(("claude-opus-5-5", source)));
            let (vetted, lines) = vet_for_binary(binary, &r, false, None);
            assert_eq!(vetted.model, None, "{binary} {source:?}");
            assert_eq!(
                lines,
                vec![cannot_pass_line("model", "claude-opus-5-5", binary)],
                "{binary} {source:?}"
            );
        }
    }
}

#[test]
fn codex_and_copilot_take_a_model_from_the_recipe_context() {
    for binary in ["codex", "copilot"] {
        let r = resolved(None, Some(("gpt-5.1", InheritSource::RecipeContext)));
        let (vetted, lines) = vet_for_binary(binary, &r, false, None);
        assert_eq!(vetted.model.as_deref(), Some("gpt-5.1"), "{binary}");
        assert!(lines.is_empty(), "{binary}: {lines:?}");
    }
}

/// S11: copilot never takes a model while a LiteLLM gateway variable is set,
/// not even one from the recipe context.
#[test]
fn copilot_refuses_any_model_under_the_gateway() {
    let r = resolved(None, Some(("gpt-5.1", InheritSource::RecipeContext)));
    let (vetted, lines) = vet_for_binary("copilot", &r, true, None);
    assert_eq!(vetted.model, None);
    assert_eq!(lines, vec![cannot_pass_line("model", "gpt-5.1", "copilot")]);
}

/// Copilot takes every level `copilot --help` lists for
/// `--reasoning-effort` (crusty review of #1892, round 2).
#[test]
fn copilot_takes_every_listed_reasoning_effort() {
    for level in ["none", "minimal", "low", "medium", "high", "xhigh", "max"] {
        let r = resolved(Some((level, InheritSource::ParentClaude)), None);
        let (vetted, lines) = vet_for_binary("copilot", &r, false, None);
        assert_eq!(vetted.effort.as_deref(), Some(level));
        assert!(lines.is_empty(), "{level}: {lines:?}");
    }
}

#[test]
fn copilot_drops_any_other_effort_with_one_line() {
    for level in ["HIGH", "ultra", "high,foo"] {
        let r = resolved(Some((level, InheritSource::RecipeContext)), None);
        let (vetted, lines) = vet_for_binary("copilot", &r, false, None);
        assert_eq!(vetted.effort, None, "{level}");
        assert_eq!(lines, vec![cannot_pass_line("effort", level, "copilot")]);
    }
}

/// A binary amplihack does not know takes no effort, and its model only from
/// the recipe context.
#[test]
fn an_unknown_binary_takes_no_effort_and_no_inherited_model() {
    let r = resolved(
        Some(("high", InheritSource::ParentLaunch)),
        Some(("claude-opus-5-5", InheritSource::ParentLaunch)),
    );
    let (vetted, lines) = vet_for_binary("mystery", &r, false, None);
    assert_eq!(vetted, Inherited::default());
    assert_eq!(
        lines,
        vec![
            cannot_pass_line("effort", "high", "mystery"),
            cannot_pass_line("model", "claude-opus-5-5", "mystery"),
        ]
    );
}

// ---------------------------------------------------------------------------
// effort_args: D4, D5, C4
// ---------------------------------------------------------------------------

fn env_of(args: &EffortArgs) -> Vec<(&str, &str)> {
    args.env.iter().map(|(k, v)| (*k, v.as_str())).collect()
}

/// F3 / C4: with no inherited effort a launch gains nothing, even when the
/// step passes its own `--effort`.
#[test]
fn no_inherited_effort_adds_nothing() {
    for binary in ["claude", "rusty", "codex", "copilot"] {
        for extra in [s(&[]), s(&["--effort", "low"]), s(&["-p", "x"])] {
            let got = effort_args(binary, &extra, &Inherited::default());
            assert!(got.argv.is_empty(), "{binary} {extra:?}: {:?}", got.argv);
            assert!(got.env.is_empty(), "{binary} {extra:?}: {:?}", got.env);
            assert_eq!(got.unsupported, None, "{binary} {extra:?}");
        }
    }
}

/// C1: what a launch exports is the effort its agent runs with. rusty,
/// rustyclawd and amplifier take it only through the variable, so it is not on
/// their argv, and they must still pass it on.
#[test]
fn exported_effort_is_the_effort_the_agent_runs_with() {
    for binary in ["claude", "rusty", "rustyclawd", "amplifier"] {
        let got = effort_args(binary, &[], &inherited(Some("high"), None));
        assert_eq!(got.exported.as_deref(), Some("high"), "{binary}");
        let got = effort_args(
            binary,
            &s(&["--effort=low"]),
            &inherited(Some("high"), None),
        );
        assert_eq!(got.exported.as_deref(), Some("low"), "{binary}");
        let got = effort_args(binary, &s(&["--effort", "low"]), &Inherited::default());
        assert_eq!(got.exported.as_deref(), Some("low"), "{binary}");
        let got = effort_args(binary, &[], &Inherited::default());
        assert_eq!(got.exported, None, "{binary}");
    }
    let got = effort_args("codex", &[], &inherited(Some("medium"), None));
    assert_eq!(got.exported.as_deref(), Some("medium"));
    let got = effort_args(
        "codex",
        &s(&["-c", "model_reasoning_effort=low"]),
        &inherited(Some("high"), None),
    );
    assert_eq!(got.exported.as_deref(), Some("low"));
    let got = effort_args("codex", &[], &inherited(Some("max"), None));
    assert_eq!(got.exported, None);
    let got = effort_args("copilot", &[], &inherited(Some("high"), None));
    assert_eq!(got.exported.as_deref(), Some("high"));
    let got = effort_args(
        "copilot",
        &s(&["--reasoning-effort=low"]),
        &inherited(Some("high"), None),
    );
    assert_eq!(got.exported.as_deref(), Some("low"));
    let got = effort_args("copilot", &[], &inherited(Some("ultra"), None));
    assert_eq!(got.exported, None);
}

fn parent_env_of(cmd: &Command) -> Vec<(String, Option<String>)> {
    let mut out: Vec<_> = cmd
        .get_envs()
        .filter(|(k, _)| *k == PARENT_EFFORT_ENV || *k == PARENT_MODEL_ENV)
        .map(|(k, v)| {
            (
                k.to_string_lossy().into_owned(),
                v.map(|v| v.to_string_lossy().into_owned()),
            )
        })
        .collect();
    out.sort();
    out
}

/// C1: a rusty step exports the effort it was given through
/// CLAUDE_CODE_EFFORT_LEVEL, although its argv has no `--effort`.
#[test]
fn export_to_child_passes_on_a_rusty_step_effort() {
    let effort = effort_args("rusty", &[], &inherited(Some("high"), None));
    assert!(effort.argv.is_empty());
    let mut cmd = Command::new("rusty");
    export_to_child(
        &mut cmd,
        &Inherited {
            effort: effort.exported,
            model: Some("claude-opus-5-5".to_string()),
        },
        "rusty",
    );
    assert_eq!(
        parent_env_of(&cmd),
        vec![
            (PARENT_EFFORT_ENV.to_string(), Some("high".to_string())),
            (
                PARENT_MODEL_ENV.to_string(),
                Some("claude-opus-5-5".to_string())
            ),
        ]
    );
}

/// I1, I2: an unset or invalid value removes the variable, so a stale parent
/// value is never passed on.
#[test]
fn export_to_child_removes_unset_and_invalid_values() {
    let mut cmd = Command::new("rusty");
    cmd.env(PARENT_EFFORT_ENV, "stale");
    cmd.env(PARENT_MODEL_ENV, "stale");
    export_to_child(&mut cmd, &inherited(Some("-bad"), None), "rusty");
    assert_eq!(
        parent_env_of(&cmd),
        vec![
            (PARENT_EFFORT_ENV.to_string(), None),
            (PARENT_MODEL_ENV.to_string(), None),
        ]
    );
}

#[test]
fn claude_gets_the_flag_and_the_variable() {
    let got = effort_args("claude", &s(&["-p", "x"]), &inherited(Some("high"), None));
    assert_eq!(got.argv, s(&["--effort", "high"]));
    assert_eq!(env_of(&got), vec![("CLAUDE_CODE_EFFORT_LEVEL", "high")]);
    assert_eq!(got.unsupported, None);
}

/// The step's own `--effort` wins in both spellings; the child's variable
/// holds the step's value, never the parent's.
#[test]
fn a_step_own_effort_wins_and_sets_the_variable_to_its_value() {
    for extra in [
        s(&["--effort", "low", "-p", "x"]),
        s(&["-p", "x", "--effort=low"]),
    ] {
        let got = effort_args("claude", &extra, &inherited(Some("high"), None));
        assert!(got.argv.is_empty(), "{extra:?}: {:?}", got.argv);
        assert_eq!(
            env_of(&got),
            vec![("CLAUDE_CODE_EFFORT_LEVEL", "low")],
            "{extra:?}"
        );
    }
}

/// `--effort` after `--` is prompt text, not the step's own effort (S7).
#[test]
fn effort_after_double_dash_is_not_the_step_own() {
    let extra = s(&["-p", "--", "--effort", "low"]);
    let got = effort_args("claude", &extra, &inherited(Some("high"), None));
    assert_eq!(got.argv, s(&["--effort", "high"]));
}

/// rusty, rustyclawd and amplifier may reject an unknown flag, so they get
/// only the variable, and no log line.
#[test]
fn other_claude_compatible_binaries_get_only_the_variable() {
    for binary in ["rusty", "rustyclawd", "amplifier"] {
        let got = effort_args(binary, &[], &inherited(Some("high"), None));
        assert!(got.argv.is_empty(), "{binary}: {:?}", got.argv);
        assert_eq!(
            env_of(&got),
            vec![("CLAUDE_CODE_EFFORT_LEVEL", "high")],
            "{binary}"
        );
        assert_eq!(got.unsupported, None, "{binary}");
    }
}

#[test]
fn codex_gets_a_config_override_and_no_variable() {
    for level in ["low", "medium", "high"] {
        let got = effort_args("codex", &[], &inherited(Some(level), None));
        assert_eq!(
            got.argv,
            s(&["-c", &format!("model_reasoning_effort={level}")])
        );
        assert!(got.env.is_empty(), "{level}: {:?}", got.env);
        assert_eq!(got.unsupported, None);
    }
}

#[test]
fn codex_reports_an_effort_it_cannot_take() {
    let got = effort_args("codex", &[], &inherited(Some("max"), None));
    assert!(got.argv.is_empty());
    assert!(got.env.is_empty());
    assert_eq!(
        got.unsupported,
        Some(cannot_pass_line("effort", "max", "codex"))
    );
}

/// A codex step that sets its own reasoning effort keeps it.
#[test]
fn codex_step_own_reasoning_effort_wins() {
    for extra in [
        s(&["-c", "model_reasoning_effort=low"]),
        s(&["--config", "model_reasoning_effort=low"]),
    ] {
        let got = effort_args("codex", &extra, &inherited(Some("high"), None));
        assert!(got.argv.is_empty(), "{extra:?}: {:?}", got.argv);
        assert_eq!(got.unsupported, None);
    }
}

/// Copilot takes the inherited effort as `--reasoning-effort L`, built from
/// a matched literal, and no Claude variable.
#[test]
fn copilot_takes_the_effort_as_reasoning_effort() {
    let got = effort_args("copilot", &s(&["-p", "x"]), &inherited(Some("xhigh"), None));
    assert_eq!(got.argv, s(&["--reasoning-effort", "xhigh"]));
    assert!(got.env.is_empty(), "{:?}", got.env);
    assert_eq!(got.unsupported, None);
}

/// The step's own `--reasoning-effort` wins and nothing is added (D5).
#[test]
fn copilot_own_reasoning_effort_wins() {
    for extra in [
        s(&["--reasoning-effort", "low"]),
        s(&["--reasoning-effort=low"]),
    ] {
        let got = effort_args("copilot", &extra, &inherited(Some("high"), None));
        assert!(got.argv.is_empty(), "{extra:?}: {:?}", got.argv);
        assert_eq!(got.unsupported, None);
    }
}

/// S5: a level copilot does not list is not passed, so copilot never refuses
/// to start over an inherited value.
#[test]
fn copilot_effort_maps_only_the_listed_literals() {
    for level in ["none", "minimal", "low", "medium", "high", "xhigh", "max"] {
        assert_eq!(copilot_effort(level), Some(level));
    }
    for level in ["", "HIGH", "ultra", "high,foo", " high"] {
        assert_eq!(copilot_effort(level), None, "{level:?}");
    }
    let got = effort_args("copilot", &[], &inherited(Some("ultra"), None));
    assert!(got.argv.is_empty(), "{:?}", got.argv);
    assert_eq!(
        got.unsupported,
        Some(cannot_pass_line("effort", "ultra", "copilot"))
    );
}

/// The run checks its model for its own agent binary. A launch of a
/// different binary under the run does not take it (crusty review of #1892,
/// round 2: quality-loop.yaml runs `amplihack copilot` under a Claude run).
#[test]
fn model_checked_only_for_the_runs_binary() {
    // Codex and Copilot require the same originating binary.
    assert!(model_matches_binary("copilot", Some("copilot")));
    assert!(model_matches_binary("codex", Some("codex")));
    assert!(!model_matches_binary("copilot", Some("claude")));
    assert!(!model_matches_binary("copilot", Some("codex")));
    assert!(!model_matches_binary("copilot", None));
    // Claude-compatible: unless the run's binary is not Claude-compatible.
    for binary in ["claude", "rusty", "rustyclawd", "amplifier"] {
        assert!(model_matches_binary(binary, None), "{binary}");
        assert!(model_matches_binary(binary, Some("claude")), "{binary}");
        assert!(model_matches_binary(binary, Some("amplifier")), "{binary}");
        assert!(!model_matches_binary(binary, Some("copilot")), "{binary}");
        assert!(!model_matches_binary(binary, Some("codex")), "{binary}");
    }
}

#[test]
fn unknown_binaries_report_effort_as_unsupported() {
    let got = effort_args("mystery", &[], &inherited(Some("high"), None));
    assert!(got.argv.is_empty());
    assert!(got.env.is_empty());
    assert_eq!(
        got.unsupported,
        Some(cannot_pass_line("effort", "high", "mystery"))
    );
}

// ---------------------------------------------------------------------------
// codex_effort, flag_value, codex_effort_from_argv
// ---------------------------------------------------------------------------

/// S5: the `-c` string is built from a matched literal, so nothing the
/// operator typed reaches codex's config parser.
#[test]
fn codex_effort_maps_only_the_three_literals() {
    assert_eq!(
        codex_effort("low").as_deref(),
        Some("model_reasoning_effort=low")
    );
    assert_eq!(
        codex_effort("medium").as_deref(),
        Some("model_reasoning_effort=medium")
    );
    assert_eq!(
        codex_effort("high").as_deref(),
        Some("model_reasoning_effort=high")
    );
    for bad in [
        "max",
        "High",
        " high",
        "high,foo=bar",
        "high\nfoo=bar",
        "high\"",
        "",
    ] {
        assert_eq!(codex_effort(bad), None, "{bad:?}");
    }
}

#[test]
fn flag_value_reads_both_spellings_and_the_last_wins() {
    assert_eq!(
        flag_value("claude", &s(&["--effort", "high"]), "--effort").as_deref(),
        Some("high")
    );
    assert_eq!(
        flag_value("claude", &s(&["--effort=high"]), "--effort").as_deref(),
        Some("high")
    );
    assert_eq!(
        flag_value(
            "claude",
            &s(&["--effort", "low", "-p", "x", "--effort=high"]),
            "--effort"
        )
        .as_deref(),
        Some("high")
    );
    assert_eq!(
        flag_value("claude", &s(&["--model", "a", "--model", "b"]), "--model").as_deref(),
        Some("b")
    );
}

#[test]
fn flag_value_with_a_trailing_flag_and_no_value_is_none() {
    assert_eq!(
        flag_value("claude", &s(&["-p", "x", "--effort"]), "--effort"),
        None
    );
    assert_eq!(flag_value("claude", &s(&[]), "--effort"), None);
}

/// S7: nothing after `--` is read, so prompt text is never taken as a flag.
#[test]
fn flag_value_stops_at_double_dash() {
    assert_eq!(
        flag_value("claude", &s(&["-p", "--", "--effort", "high"]), "--effort"),
        None
    );
    assert_eq!(
        flag_value(
            "claude",
            &s(&["--effort", "low", "--", "--effort=high"]),
            "--effort"
        )
        .as_deref(),
        Some("low")
    );
}

#[test]
fn flag_value_does_not_match_a_longer_flag() {
    assert_eq!(
        flag_value("claude", &s(&["--efforts", "high"]), "--effort"),
        None
    );
    assert_eq!(
        flag_value("claude", &s(&["--modelx=high"]), "--model"),
        None
    );
}

#[test]
fn codex_effort_from_argv_reads_config_overrides() {
    assert_eq!(
        codex_effort_from_argv(&s(&["-c", "model_reasoning_effort=high"])).as_deref(),
        Some("high")
    );
    assert_eq!(
        codex_effort_from_argv(&s(&["--config", "model_reasoning_effort=low"])).as_deref(),
        Some("low")
    );
    assert_eq!(codex_effort_from_argv(&s(&["-c", "model=gpt-5.1"])), None);
    assert_eq!(
        codex_effort_from_argv(&s(&["--", "-c", "model_reasoning_effort=high"])),
        None
    );
    assert_eq!(codex_effort_from_argv(&s(&["-c"])), None);
}

#[test]
fn codex_quoted_effort_is_decoded_before_export_and_nested_inheritance() {
    for args in [
        s(&["-c", r#"model_reasoning_effort="high""#]),
        s(&["--config", "model_reasoning_effort='high'"]),
        s(&[r#"--config=model_reasoning_effort="h\u0069gh""#]),
        s(&[r#"-cmodel_reasoning_effort="high""#]),
        s(&[r#"-c=model_reasoning_effort="high""#]),
        s(&["-c", " model_reasoning_effort = 'high' "]),
        s(&[
            "-c",
            "model_reasoning_effort=low",
            "-c",
            r#"model_reasoning_effort="high""#,
            "--",
            "-c",
            "model_reasoning_effort=medium",
        ]),
    ] {
        assert_eq!(
            codex_effort_from_argv(&args).as_deref(),
            Some("high"),
            "{args:?}"
        );
        let parent = effort_args("codex", &args, &inherited(Some("low"), None));
        assert!(parent.argv.is_empty(), "{args:?}: {:?}", parent.argv);
        assert_eq!(parent.exported.as_deref(), Some("high"), "{args:?}");
        assert!(parent.unsupported.is_none(), "{args:?}");

        let child = effort_args("codex", &[], &inherited(parent.exported.as_deref(), None));
        assert_eq!(
            child.argv,
            s(&["-c", "model_reasoning_effort=high"]),
            "{args:?}"
        );
        assert_eq!(child.exported.as_deref(), Some("high"), "{args:?}");
        assert!(child.unsupported.is_none(), "{args:?}");

        let child = effort_args("claude", &[], &inherited(parent.exported.as_deref(), None));
        assert_eq!(child.argv, s(&["--effort", "high"]), "{args:?}");
        assert_eq!(child.exported.as_deref(), Some("high"), "{args:?}");
        assert!(child.unsupported.is_none(), "{args:?}");
    }
}

// ---------------------------------------------------------------------------
// inherited_from_env: what each step's launch reads
// ---------------------------------------------------------------------------

fn from_env(effort: Option<&str>, model: Option<&str>) -> Inherited {
    with_env(
        &[
            (PARENT_EFFORT_ENV, effort.map(OsStr::new)),
            (PARENT_MODEL_ENV, model.map(OsStr::new)),
            // Steps never read the fallbacks (D1).
            (
                "CLAUDE_CODE_EFFORT_LEVEL",
                Some(OsStr::new("fallback-effort")),
            ),
            ("ANTHROPIC_MODEL", Some(OsStr::new("fallback-model"))),
        ],
        inherited_from_env,
    )
}

#[test]
fn inherited_from_env_reads_only_the_parent_variables() {
    assert_eq!(from_env(None, None), Inherited::default());
    assert_eq!(
        from_env(Some("high"), Some("claude-opus-5-5")),
        inherited(Some("high"), Some("claude-opus-5-5"))
    );
    assert_eq!(
        from_env(Some(" high "), None),
        inherited(Some("high"), None)
    );
}

/// A stale or tampered value in a step's environment is dropped silently: the
/// run already warned once, and a step must not repeat it.
#[test]
fn inherited_from_env_drops_invalid_values() {
    // An environment variable cannot hold a NUL, and `set_var` panics on one.
    for bad in INVALID_VALUES.iter().filter(|v| !v.contains('\0')) {
        assert_eq!(
            from_env(Some(bad), Some(bad)),
            Inherited::default(),
            "{bad:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Log lines, word for word with the docs
// ---------------------------------------------------------------------------

#[test]
fn cannot_pass_line_matches_the_docs() {
    assert_eq!(
        cannot_pass_line("effort", "high", "copilot"),
        "amplihack: effort 'high' cannot be passed to copilot; steps use copilot's default"
    );
    assert_eq!(
        cannot_pass_line("model", "claude-opus-5-5", "codex"),
        "amplihack: model 'claude-opus-5-5' cannot be passed to codex; steps use codex's default"
    );
}

#[test]
fn summary_line_is_absent_when_nothing_has_a_value() {
    assert_eq!(inherit_summary_line(&resolved(None, None)), None);
}

#[test]
fn summary_line_with_one_shared_source() {
    let r = resolved(
        Some(("high", InheritSource::ParentLaunch)),
        Some(("claude-opus-5-5", InheritSource::ParentLaunch)),
    );
    assert_eq!(
        inherit_summary_line(&r).as_deref(),
        Some(
            "amplihack: steps inherit effort=high model=claude-opus-5-5 \
             (from the parent amplihack launch)"
        )
    );
}

#[test]
fn summary_line_from_the_recipe_context() {
    let r = resolved(
        Some(("medium", InheritSource::RecipeContext)),
        Some(("claude-sonnet-4-5", InheritSource::RecipeContext)),
    );
    assert_eq!(
        inherit_summary_line(&r).as_deref(),
        Some(
            "amplihack: steps inherit effort=medium model=claude-sonnet-4-5 \
             (from the recipe context)"
        )
    );
}

#[test]
fn summary_line_with_only_effort_names_model_unset() {
    let r = resolved(Some(("low", InheritSource::RecipeContext)), None);
    assert_eq!(
        inherit_summary_line(&r).as_deref(),
        Some("amplihack: steps inherit effort=low model=unset (from the recipe context)")
    );
    let r = resolved(Some(("high", InheritSource::ParentClaude)), None);
    assert_eq!(
        inherit_summary_line(&r).as_deref(),
        Some("amplihack: steps inherit effort=high model=unset (from the parent claude process)")
    );
}

#[test]
fn summary_line_with_only_model_names_effort_unset() {
    let r = resolved(None, Some(("claude-opus-5-5", InheritSource::ModelEnv)));
    assert_eq!(
        inherit_summary_line(&r).as_deref(),
        Some("amplihack: steps inherit effort=unset model=claude-opus-5-5 (from ANTHROPIC_MODEL)")
    );
}

/// Mixed sources read `<effort source> and <model source>`.
#[test]
fn summary_line_with_mixed_sources_names_both() {
    let r = resolved(
        Some(("high", InheritSource::ParentLaunch)),
        Some(("claude-sonnet-4-5", InheritSource::RecipeContext)),
    );
    assert_eq!(
        inherit_summary_line(&r).as_deref(),
        Some(
            "amplihack: steps inherit effort=high model=claude-sonnet-4-5 \
             (from the parent amplihack launch and the recipe context)"
        )
    );
}

/// D7, crusty review of #1892 round 3: the run printed the "cannot be passed"
/// effort line only for its own binary. A launch of another binary under the
/// run, or any launch outside a run, prints its own.
#[test]
fn run_printed_lines_only_for_the_runs_own_binary() {
    assert!(run_printed_lines_for_run("codex", Some("codex"), true));
    assert!(run_printed_lines_for_run("copilot", Some("copilot"), true));
    assert!(!run_printed_lines_for_run("codex", Some("claude"), true));
    assert!(!run_printed_lines_for_run("copilot", Some("claude"), true));
    assert!(!run_printed_lines_for_run("codex", None, true));
    assert!(!run_printed_lines_for_run("codex", Some("codex"), false));
}

#[test]
fn same_binary_parent_models_are_vetted_and_cross_binary_models_are_refused() {
    for binary in ["codex", "copilot"] {
        let r = resolved(None, Some(("gpt-5.1", InheritSource::ParentLaunch)));
        for origin in ["codex", "copilot", "claude"] {
            let (got, lines) = vet_for_binary(binary, &r, false, Some(origin));
            if origin == binary {
                assert_eq!(got.model.as_deref(), Some("gpt-5.1"));
                assert!(lines.is_empty());
            } else {
                assert_eq!(got.model, None);
                assert_eq!(lines, vec![cannot_pass_line("model", "gpt-5.1", binary)]);
            }
        }
        let (got, lines) = vet_for_binary("claude", &r, false, Some(binary));
        assert_eq!(got.model, None);
        assert_eq!(lines, vec![cannot_pass_line("model", "gpt-5.1", "claude")]);
    }
}

#[test]
fn codex_quoted_effort_is_decoded_before_export_and_subsequent_inheritance() {
    for arg in [
        r#"model_reasoning_effort="high""#,
        "model_reasoning_effort='high'",
        r#"model_reasoning_effort="h\u0069gh""#,
        "model_reasoning_effort = \"high\"",
        " model_reasoning_effort = 'high' ",
        " model_reasoning_effort = high ",
    ] {
        for args in [
            s(&["-c", arg]),
            s(&["--config", arg]),
            s(&[&format!("--config={arg}")]),
        ] {
            let parent = effort_args("codex", &args, &inherited(Some("low"), None));
            assert_eq!(parent.exported.as_deref(), Some("high"), "{args:?}");
            assert!(parent.argv.is_empty(), "own effort must win: {args:?}");
            let mut cmd = Command::new("codex");
            export_to_child(
                &mut cmd,
                &inherited(parent.exported.as_deref(), None),
                "codex",
            );
            let parent_effort = cmd
                .get_envs()
                .find(|(key, _)| *key == PARENT_EFFORT_ENV)
                .and_then(|(_, value)| value.and_then(OsStr::to_str));
            let resolved = resolve_parent(&ParentInputs {
                parent_effort,
                ..ParentInputs::default()
            });
            assert_eq!(resolved.effort_source, Some(InheritSource::ParentLaunch));
            assert!(resolved.warnings.is_empty(), "{args:?}");
            let (vetted, lines) = vet_for_binary("codex", &resolved, false, None);
            assert_eq!(vetted.effort.as_deref(), Some("high"), "{args:?}");
            assert!(lines.is_empty(), "{args:?}: {lines:?}");
            let child = effort_args("codex", &[], &vetted);
            assert_eq!(child.argv, s(&["-c", "model_reasoning_effort=high"]));
            assert_eq!(child.exported.as_deref(), Some("high"));
        }
    }
}

#[test]
fn model_provenance_is_exported_with_model_and_removed_with_it() {
    use super::inherit::PARENT_MODEL_BINARY_ENV;
    let mut cmd = Command::new("copilot");
    export_to_child(&mut cmd, &inherited(None, Some("gpt-5.1")), "copilot");
    assert!(
        cmd.get_envs()
            .any(|(k, v)| k == PARENT_MODEL_BINARY_ENV && v == Some(OsStr::new("copilot")))
    );
    export_to_child(&mut cmd, &Inherited::default(), "claude");
    assert!(
        cmd.get_envs()
            .any(|(k, v)| k == PARENT_MODEL_BINARY_ENV && v.is_none())
    );
}

#[test]
fn consumed_option_values_cannot_set_model_effort_or_config() {
    for prompt in ["--model=haiku", "--effort=low", "--model", "--effort"] {
        for option in ["--system-prompt", "--append-system-prompt", "--settings"] {
            let args = s(&[option, prompt]);
            assert_eq!(flag_value("claude", &args, "--model"), None);
            assert_eq!(flag_value("claude", &args, "--effort"), None);
            let effort = effort_args("claude", &args, &inherited(Some("high"), None));
            assert_eq!(effort.exported.as_deref(), Some("high"));
            assert_eq!(effort.argv, s(&["--effort", "high"]));
        }
    }
    assert_eq!(
        codex_effort_from_argv(&s(&["--profile", "--config=model_reasoning_effort=low"])),
        None,
    );
    assert_eq!(
        flag_value("codex", &s(&["--config", "--model=phantom"]), "--model"),
        None,
    );
    assert_eq!(
        flag_value("codex", &s(&["--model=first", "-m", "last"]), "--model").as_deref(),
        Some("last"),
    );
    assert_eq!(
        flag_value("codex", &s(&["-mfirst", "--model", "last"]), "--model").as_deref(),
        Some("last"),
    );
    assert_eq!(
        flag_value("codex", &s(&["--", "-m", "prompt"]), "--model"),
        None
    );
}
