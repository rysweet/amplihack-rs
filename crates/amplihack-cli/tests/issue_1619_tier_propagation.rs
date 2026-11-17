//! Issue #1619 (TDD): a tier word on a launch becomes a real model, and on
//! Codex a real reasoning effort, on the harness's command line.
//!
//! This is the behaviour evidence the issue's 3 November comment asks for:
//! the real chain is exercised — the `amplihack` binary is started with the
//! argv the recipe runner builds, and a fake harness on `PATH` records its
//! own argv. Nothing here reimplements a command builder, because a copied
//! implementation would have passed while the shipped one was broken.
//!
//! The recipe runner puts the step's `model:` value on the passthrough side
//! itself, so what amplihack receives is, for example:
//!
//! ```text
//! amplihack codex -- exec --output-last-message <path> --model strong -
//! amplihack claude -- --model strong -p
//! ```
//!
//! Every path the run may read or write is under one temp dir, and the
//! environment is set on the child only: no test here touches the real
//! `~/.claude`, `~/.codex` or `~/.amplihack`.

#![cfg(target_os = "linux")]
use std::fs;

/// The fake harness, the isolated stage and the argv reader are
/// `support::tier_harness`'s: `issue_1619_tier_argv_safety` asks a different
/// question of the same recording, so the stage set is built once there
/// rather than copied into each.
use crate::support::tier_harness::{FAKE_HARNESS, Fixture};

/// The argv a Codex agent step arrives with, as the recipe runner builds it.
fn codex_step(model: &str) -> Vec<String> {
    vec![
        "exec".to_string(),
        "--model".to_string(),
        model.to_string(),
        "-".to_string(),
    ]
}

// -------------------------------------------------------------------------
// F3 — a tier resolves to a model, per harness
// -------------------------------------------------------------------------

#[test]
fn claude_strong_resolves_to_the_shipped_strong_model() {
    let f = Fixture::new();
    let launch = f.run("claude", &["--model", "strong", "-p"]);
    assert_eq!(
        launch.model(),
        Some("claude-opus-5[1m]"),
        "a `strong` step on a Claude-compatible harness runs on the shipped strong \
         model. argv: {:?}",
        launch.argv
    );
}

#[test]
fn claude_standard_and_fast_resolve_to_their_own_models() {
    let f = Fixture::new();
    for (tier, expected) in [
        ("standard", "claude-sonnet-5-5"),
        ("fast", "claude-haiku-5-5"),
    ] {
        let launch = f.run("claude", &["--model", tier, "-p"]);
        assert_eq!(
            launch.model(),
            Some(expected),
            "`{tier}` on claude must resolve to {expected}; argv: {:?}",
            launch.argv
        );
    }
}

#[test]
fn a_tier_word_never_reaches_a_harness_as_a_model() {
    // The whole point: an unresolved tier word on the command line would ask
    // the CLI for a model called "strong" and break the step.
    let f = Fixture::new();
    for (harness, passthrough) in [
        ("claude", vec!["--model", "strong", "-p"]),
        ("copilot", vec!["--model", "strong", "-p", "go"]),
    ] {
        let launch = f.run(harness, &passthrough);
        assert_ne!(
            launch.model(),
            Some("strong"),
            "{harness} was handed the literal tier word as a model. argv: {:?}",
            launch.argv
        );
    }
    let launch = f.run("codex", &["exec", "--model", "strong", "-"]);
    assert_ne!(
        launch.model(),
        Some("strong"),
        "codex was handed the literal tier word as a model. argv: {:?}",
        launch.argv
    );
}

#[test]
fn tier_words_are_matched_case_insensitively() {
    let f = Fixture::new();
    let launch = f.run("claude", &["--model", "Strong", "-p"]);
    assert_eq!(
        launch.model(),
        Some("claude-opus-5[1m]"),
        "a trimmed, case-insensitive match is documented; argv: {:?}",
        launch.argv
    );
}

// -------------------------------------------------------------------------
// F7 — Codex effort reaches the child
// -------------------------------------------------------------------------

#[test]
fn codex_tiers_differ_by_reasoning_effort_and_name_no_model() {
    let f = Fixture::new();
    for (tier, effort) in [("strong", "xhigh"), ("standard", "high"), ("fast", "low")] {
        let args: Vec<String> = codex_step(tier);
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let launch = f.run("codex", &refs);
        assert_eq!(
            launch.effort(),
            Some(effort),
            "codex `{tier}` must pass -c model_reasoning_effort={effort}; argv: {:?}",
            launch.argv
        );
        assert_eq!(
            launch.model(),
            None,
            "the shipped codex column names no model — Opus and Sonnet do not exist \
             there, so the tier changes the effort and leaves the model to codex. \
             argv: {:?}",
            launch.argv
        );
    }
}

#[test]
fn codex_effort_is_spliced_without_disturbing_the_exec_subcommand() {
    let f = Fixture::new();
    let args = codex_step("strong");
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let launch = f.run("codex", &refs);
    assert!(
        launch.argv.contains(&"exec".to_string()),
        "the exec subcommand must survive; argv: {:?}",
        launch.argv
    );
    // Issue #1985's injections are decided from the same argv and must be
    // unaffected by the tier rewrite.
    assert!(
        launch.argv.contains(&"--sandbox".to_string())
            && launch.argv.contains(&"danger-full-access".to_string()),
        "the agent-step sandbox injection must still happen; argv: {:?}",
        launch.argv
    );
    assert!(
        launch
            .argv
            .contains(&"--dangerously-bypass-hook-trust".to_string()),
        "the hook-trust bypass must still happen; argv: {:?}",
        launch.argv
    );
}

#[test]
fn a_callers_own_reasoning_effort_is_left_alone() {
    let f = Fixture::new();
    let launch = f.run(
        "codex",
        &[
            "exec",
            "-c",
            "model_reasoning_effort=medium",
            "--model",
            "strong",
            "-",
        ],
    );
    assert_eq!(
        launch.effort(),
        Some("medium"),
        "amplihack must not override a reasoning effort the caller chose; argv: {:?}",
        launch.argv
    );
    assert_eq!(
        launch
            .argv
            .iter()
            .filter(|a| a.starts_with("model_reasoning_effort="))
            .count(),
        1,
        "exactly one reasoning-effort setting reaches codex; argv: {:?}",
        launch.argv
    );
}

#[test]
fn an_effort_value_amplihack_does_not_accept_is_dropped_and_the_launch_proceeds() {
    let f = Fixture::new();
    f.write_override("version = 1\n\n[codex.fast]\nmodel = \"gpt-6.1-sol\"\neffort = \"turbo\"\n");
    let args = codex_step("fast");
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let launch = f.run("codex", &refs);
    assert!(
        launch.output.status.success(),
        "a bad effort value must not stop the launch; stderr:\n{}",
        launch.stderr()
    );
    assert_eq!(
        launch.effort(),
        None,
        "an effort outside the accepted list is dropped; argv: {:?}",
        launch.argv
    );
    assert_eq!(
        launch.model(),
        Some("gpt-6.1-sol"),
        "the model still applies when the effort is dropped; argv: {:?}",
        launch.argv
    );
}

// -------------------------------------------------------------------------
// F4 — the fall-back removes the tier, and never fails the step
// -------------------------------------------------------------------------

#[test]
fn a_harness_with_no_column_passes_no_model_and_still_launches() {
    let f = Fixture::new();
    let launch = f.run("copilot", &["--model", "standard", "-p", "go"]);
    assert!(
        launch.output.status.success(),
        "a missing mapping must never fail the step; stderr:\n{}",
        launch.stderr()
    );
    assert_eq!(
        launch.model(),
        None,
        "the shipped copilot column has no entries, so the tier word is removed and \
         copilot uses its own default. argv: {:?}",
        launch.argv
    );
}

#[test]
fn a_claude_tier_with_no_mapping_falls_back_to_amplihacks_own_default() {
    let f = Fixture::new();
    // An override file that empties the claude strong cell: a cell with no
    // model is exactly what "that model is not available here" looks like.
    f.write_override("version = 1\n\n[claude.strong]\n");
    let launch = f.run("claude", &["--model", "strong", "-p"]);
    assert!(
        launch.output.status.success(),
        "stderr:\n{}",
        launch.stderr()
    );
    assert_eq!(
        launch.model(),
        Some("claude-opus-5[1m]"),
        "falling back removes the tier word and lets amplihack's existing chain decide; \
         with AMPLIHACK_DEFAULT_MODEL unset that is the built-in default. argv: {:?}",
        launch.argv
    );
}

#[test]
fn the_fall_back_honours_amplihack_default_model() {
    let f = Fixture::new();
    f.write_override("version = 1\n\n[claude.strong]\n");
    let launch = f.finish(
        f.launch("claude", &["--model", "strong", "-p"])
            .env("AMPLIHACK_DEFAULT_MODEL", "claude-sonnet-5-5"),
    );
    assert_eq!(
        launch.model(),
        Some("claude-sonnet-5-5"),
        "the fall-back substitutes no model of its own; it hands the decision back to \
         the audited chain. argv: {:?}",
        launch.argv
    );
}

// -------------------------------------------------------------------------
// F5 — one log record per falling-back launch, and none otherwise
// -------------------------------------------------------------------------

#[test]
fn a_falling_back_launch_says_so_once() {
    let f = Fixture::new();
    let launch = f.run("copilot", &["--model", "strong", "-p", "go"]);
    let stderr = launch.stderr();
    let mentions = stderr
        .lines()
        .filter(|l| l.contains("model tier") || l.contains("model-tiers"))
        .count();
    assert_eq!(
        mentions, 1,
        "exactly one line names the fall-back: one launch, one record, no attempts to \
         count. stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("strong") && stderr.contains("copilot"),
        "the line must name the tier that fell back and the harness it fell back on. \
         stderr:\n{stderr}"
    );
}

#[test]
fn a_launch_that_does_not_fall_back_writes_no_fall_back_line() {
    let f = Fixture::new();
    let launch = f.run("claude", &["--model", "fast", "-p"]);
    let stderr = launch.stderr();
    for word in ["falls back", "fell back", "fall-back", "fallback"] {
        assert!(
            !stderr.to_lowercase().contains(word),
            "a resolved tier is not a fall-back and writes no fall-back line. \
             stderr:\n{stderr}"
        );
    }
}

// -------------------------------------------------------------------------
// F6 — overrides
// -------------------------------------------------------------------------

#[test]
fn an_instance_override_replaces_one_cell_and_leaves_the_rest() {
    let f = Fixture::new();
    f.write_override("version = 1\n\n[claude.standard]\nmodel = \"claude-opus-5[1m]\"\n");
    let standard = f.run("claude", &["--model", "standard", "-p"]);
    assert_eq!(
        standard.model(),
        Some("claude-opus-5[1m]"),
        "the overridden cell applies; argv: {:?}",
        standard.argv
    );
    let fast = f.run("claude", &["--model", "fast", "-p"]);
    assert_eq!(
        fast.model(),
        Some("claude-haiku-5-5"),
        "entries are merged cell by cell, so an untouched cell keeps the shipped value; \
         argv: {:?}",
        fast.argv
    );
}

#[test]
fn an_override_can_give_the_codex_column_a_model() {
    let f = Fixture::new();
    f.write_override(
        "version = 1\n\n[codex.strong]\nmodel = \"gpt-6.1-sol\"\neffort = \"xhigh\"\n",
    );
    let args = codex_step("strong");
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let launch = f.run("codex", &refs);
    assert_eq!(
        launch.model(),
        Some("gpt-6.1-sol"),
        "argv: {:?}",
        launch.argv
    );
    assert_eq!(launch.effort(), Some("xhigh"), "argv: {:?}", launch.argv);
}

#[test]
fn a_global_force_wins_over_every_tier_and_keeps_the_tiers_effort() {
    let f = Fixture::new();
    f.write_override(
        "version = 1\n\n[codex.strong]\nmodel = \"gpt-6.1-sol\"\neffort = \"xhigh\"\n\
         \n[codex.fast]\nmodel = \"gpt-6.1-sol\"\neffort = \"low\"\n",
    );
    f.write_user_config("[model]\nforce = \"gpt-6.1-mini\"\n");
    for (tier, effort) in [("strong", "xhigh"), ("fast", "low")] {
        let args = codex_step(tier);
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let launch = f.run("codex", &refs);
        assert_eq!(
            launch.model(),
            Some("gpt-6.1-mini"),
            "model.force applies to every step whatever its tier; argv: {:?}",
            launch.argv
        );
        assert_eq!(
            launch.effort(),
            Some(effort),
            "forcing one model must not quietly flatten the tier's reasoning effort to \
             one value; argv: {:?}",
            launch.argv
        );
    }
}

#[test]
fn the_force_environment_variable_works_without_a_config_file() {
    let f = Fixture::new();
    let launch = f.finish(
        f.launch("claude", &["--model", "fast", "-p"])
            .env("AMPLIHACK_FORCE_MODEL", "claude-sonnet-5-5"),
    );
    assert_eq!(
        launch.model(),
        Some("claude-sonnet-5-5"),
        "AMPLIHACK_FORCE_MODEL is the one-run form of model.force; argv: {:?}",
        launch.argv
    );
}

#[test]
fn a_forced_model_replaces_even_an_explicit_concrete_id() {
    // The one documented exception to issue #1527. It fires only from the
    // operator's own user config or environment.
    let f = Fixture::new();
    f.write_user_config("[model]\nforce = \"claude-haiku-5-5\"\n");
    let launch = f.run("claude", &["--model", "claude-opus-5[1m]", "-p"]);
    assert_eq!(
        launch.model(),
        Some("claude-haiku-5-5"),
        "argv: {:?}",
        launch.argv
    );
}

// -------------------------------------------------------------------------
// The LiteLLM gateway: tiers do not route past it
// -------------------------------------------------------------------------

/// A gateway launch is refused outright when its explicit `--model` is not
/// `AMPLIHACK_LITELLM_MODEL`. So once every bundled step declares a tier, a
/// tier word left in place would not merely route badly — it would stop every
/// gateway launch before it started. Deleting the pair is what lets
/// amplihack's own chain write the gateway's model in its place.
#[test]
fn a_gateway_launch_uses_the_gateway_model_and_not_the_tier() {
    let f = Fixture::new();
    // Two version gates that predate this issue and disagree: a gateway
    // launch of Claude Code requires one of
    // `VERIFIED_CLAUDE_CODE_VERSIONS` (exact match, 2.1.247), and the
    // contract file requires 2.1.319 or later (issue #2030). No single
    // version satisfies both, so this fixture reports the gateway-verified
    // one and takes the documented way out of the contract gate. The
    // assertion below is about the model, which is what this issue decides.
    crate::support::exec_stub::write_executable(
        &f.tools.join("claude"),
        FAKE_HARNESS.replace("2.1.319", "2.1.247"),
        0o755,
    );
    let launch = f.finish(
        f.launch("claude", &["--model", "strong", "-p"])
            .env("AMPLIHACK_LITELLM_MODEL", "gateway-routed-model")
            .env("AMPLIHACK_LITELLM_ENDPOINT", "http://127.0.0.1:1")
            .env("AMPLIHACK_LITELLM_API_KEY", "test-key")
            .env("AMPLIHACK_NO_SYSTEM_PROMPT_APPEND", "1"),
    );
    assert_eq!(
        launch.model(),
        Some("gateway-routed-model"),
        "the gateway routes on the model name and was never configured to route the \
         shipped ids, so a tier resolves by deletion and the gateway's own model is \
         written in its place. argv: {:?}",
        launch.argv
    );
}

#[test]
fn a_gateway_launch_of_copilot_is_not_refused_by_its_own_tier() {
    // Copilot CLI is a gateway target that `is_claude_compatible_tool`
    // excludes. Deriving "through the gateway" from Claude-compatibility
    // instead of from the target left a filled `[copilot]` column resolving
    // to a model the gateway was never configured to route, and
    // `validate_launch_args` then refused the launch outright — every tiered
    // Copilot step, not just this one. The shipped column is empty; the
    // shipped file invites an operator to fill it.
    let f = Fixture::new();
    crate::support::exec_stub::write_executable(
        &f.tools.join("copilot"),
        FAKE_HARNESS.replace("'0.0.400'", "'1.0.83-2'"),
        0o755,
    );
    f.write_override("version = 1\n\n[copilot.strong]\nmodel = \"gpt-5-codex\"\n");
    let launch = f.finish(
        f.launch("copilot", &["--model", "strong", "-p", "go"])
            .env("AMPLIHACK_RUN_DIR", &f.run_dir)
            .env("AMPLIHACK_RUN_ID", "20261116-000000-test")
            .env("AMPLIHACK_LITELLM_MODEL", "gateway-routed-model")
            .env("AMPLIHACK_LITELLM_ENDPOINT", "http://127.0.0.1:1")
            .env("AMPLIHACK_LITELLM_API_KEY", "test-key"),
    );
    assert!(
        launch.output.status.success(),
        "a tiered copilot step under the gateway must launch; stderr:\n{}",
        launch.stderr()
    );
    assert_eq!(
        launch.model(),
        None,
        "the gateway routes on the model name, so the tier resolves by deletion and \
         copilot runs on the model the gateway routes. argv: {:?}",
        launch.argv
    );
    let text = fs::read_to_string(f.run_dir.join("model-tiers.jsonl")).expect("a record");
    let record: serde_json::Value =
        serde_json::from_str(text.lines().next().expect("one line")).expect("JSON");
    assert_eq!(
        record["fallback"],
        serde_json::json!("gateway"),
        "the record names the gateway as the reason, as it does for claude. \
         record: {record}"
    );
}

// -------------------------------------------------------------------------
// The tier file passes the same trust gate as the config file
// -------------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn a_group_writable_tier_file_is_ignored_and_the_shipped_mapping_applies() {
    // `model.force` and `model.tier_map` are dropped from a config file
    // another user can write; the settings that moved into this file when the
    // feature consolidated to one override file get the same gate, or a
    // group-writable home chooses the model every agent step runs on.
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    f.write_override("version = 1\n\n[claude.strong]\nmodel = \"planted-model\"\n");
    let path = f.home.join(".amplihack").join("model-tiers.toml");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
    let launch = f.run("claude", &["--model", "strong", "-p"]);
    assert_eq!(
        launch.model(),
        Some("claude-opus-5[1m]"),
        "the shipped mapping applies when the override file cannot be trusted. \
         argv: {:?}",
        launch.argv
    );
    let stderr = launch.stderr();
    assert!(
        stderr.contains("writable by group or others"),
        "the operator is told why their file was ignored:\n{stderr}"
    );
    assert_eq!(
        stderr.matches("model-tiers.toml").count(),
        1,
        "one diagnostic, once:\n{stderr}"
    );
    assert!(
        stderr.contains("~/.amplihack/model-tiers.toml"),
        "the path is named home-relative: these lines get pasted into issues, and the \
         absolute path carries the operator's username.\n{stderr}"
    );
}

// -------------------------------------------------------------------------
// F8/F11 — the measurement record
// -------------------------------------------------------------------------

#[test]
fn every_launch_appends_one_record_with_the_documented_fields() {
    let f = Fixture::new();
    let output = f
        .launch("claude", &["--model", "standard", "-p"])
        .env("AMPLIHACK_RUN_DIR", &f.run_dir)
        .env("AMPLIHACK_RUN_ID", "20261116-000000-test")
        .output()
        .expect("run amplihack");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let path = f.run_dir.join("model-tiers.jsonl");
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("no record at {}: {e}", path.display()));
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 1, "one launch, one record. file:\n{text}");

    let record: serde_json::Value =
        serde_json::from_str(lines[0]).expect("a record is one JSON object");
    let object = record.as_object().expect("a JSON object");
    let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
    keys.sort_unstable();
    let mut expected = [
        "v",
        "seq",
        "run_id",
        "harness",
        "column",
        "tier",
        "requested_model",
        "effective_model",
        "requested_effort",
        "effective_effort",
        "fallback",
    ];
    expected.sort_unstable();
    assert_eq!(
        keys, expected,
        "the record schema is exactly these eleven scalars. Run artefacts are pasted \
         into issues and PRs, so no argv, environment, prompt, working directory or \
         path field may be added here."
    );
    assert_eq!(record["tier"], serde_json::json!("standard"));
    assert_eq!(record["harness"], serde_json::json!("claude"));
    assert_eq!(record["column"], serde_json::json!("claude"));
    assert_eq!(
        record["effective_model"],
        serde_json::json!("claude-sonnet-5-5")
    );
    assert_eq!(
        record["fallback"],
        serde_json::Value::Null,
        "`fallback` is null when the launch did not fall back, which is what lets the \
         summary render both kinds from one file"
    );
    assert_eq!(record["run_id"], serde_json::json!("20261116-000000-test"));
    assert_eq!(record["seq"], serde_json::json!(1));
}

#[test]
fn a_falling_back_record_names_the_reason() {
    let f = Fixture::new();
    let output = f
        .launch("copilot", &["--model", "strong", "-p", "go"])
        .env("AMPLIHACK_RUN_DIR", &f.run_dir)
        .env("AMPLIHACK_RUN_ID", "20261116-000000-test")
        .output()
        .expect("run amplihack");
    assert!(output.status.success());
    let text = fs::read_to_string(f.run_dir.join("model-tiers.jsonl")).expect("a record");
    let record: serde_json::Value =
        serde_json::from_str(text.lines().next().expect("one line")).expect("JSON");
    assert_eq!(
        record["fallback"],
        serde_json::json!("no_mapping"),
        "the copilot column ships with no entries, so a tier there has no mapping. \
         record: {record}"
    );
    assert_eq!(record["effective_model"], serde_json::Value::Null);
}

#[test]
fn nothing_is_written_when_there_is_no_run_directory() {
    let f = Fixture::new();
    // AMPLIHACK_RUN_DIR set to the empty string is how amplihack marks a
    // launch outside a run scope, and empty means unset to every consumer.
    let output = f
        .launch("claude", &["--model", "fast", "-p"])
        .env("AMPLIHACK_RUN_DIR", "")
        .output()
        .expect("run amplihack");
    assert!(output.status.success());
    assert!(
        !f.run_dir.join("model-tiers.jsonl").exists(),
        "amplihack never creates a run directory for this file"
    );
}

#[test]
fn a_record_is_never_written_through_a_symlink() {
    let f = Fixture::new();
    let decoy = f.home.join("decoy.json");
    fs::write(&decoy, "untouched\n").unwrap();
    std::os::unix::fs::symlink(&decoy, f.run_dir.join("model-tiers.jsonl")).unwrap();
    let output = f
        .launch("claude", &["--model", "fast", "-p"])
        .env("AMPLIHACK_RUN_DIR", &f.run_dir)
        .env("AMPLIHACK_RUN_ID", "20261116-000000-test")
        .output()
        .expect("run amplihack");
    assert!(
        output.status.success(),
        "a planted symlink must not stop the launch"
    );
    assert_eq!(
        fs::read_to_string(&decoy).unwrap(),
        "untouched\n",
        "the record file is opened without following a symlink, so a planted link \
         cannot be followed into another file"
    );
}
