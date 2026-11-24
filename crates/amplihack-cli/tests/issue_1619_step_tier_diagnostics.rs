//! Issue #1619: what `[step_tiers]` could not act on reaches the operator.
//!
//! The per-step override is read in the `amplihack recipe run` process, which
//! performs no launch of its own, so its diagnostics were gathered and
//! dropped there: a tier file that would not parse, a value that is not a
//! tier and a step id no recipe declares all passed in silence.
//!
//! Runs the real `amplihack` binary with stdout and stderr in one file,
//! against a stub recipe runner, under one temp directory. The stub writes
//! down its own argv, which is how the delivery route is pinned: the staged
//! copies reach the runner as its first `-R` and as the positional recipe
//! argument, and the assertion is over the order those arguments appear in,
//! not over the name of any constant.

#![cfg(unix)]

use crate::support::amplihack_bin::amplihack_bin;
use std::fs;
use std::path::PathBuf;
use std::process::Stdio;

/// `amplihack recipe run` probes the runner with `--capabilities` first.
const CAPABILITIES: &str = r#"case "$*" in --capabilities) printf '%b\n' '\173"schema_version":1,"version":"fixture","capabilities":["codex_exec","durable_recovery_v1"]\175'; exit 0 ;; esac
"#;

/// A runner that writes down its own argv, reports one successful step and
/// does nothing else. It executes no step, so only the command line
/// amplihack built is observable here; `issue_1619_real_runner_step_tier.rs`
/// is where the real runner acts on it.
const STUB_RUNNER: &str = r#"if [ -n "${AMPLIHACK_1619_ARGV:-}" ]; then
  : > "$AMPLIHACK_1619_ARGV"
  for a in "$@"; do printf '%s\n' "$a" >> "$AMPLIHACK_1619_ARGV"; done
fi
cat <<'JSON'
{"recipe_name":"issue-1619-step-tiers","success":true,"step_results":[{"step_id":"step-08-implement","status":"success","output":"done"}],"context":{}}
JSON
exit 0
"#;

const RECIPE: &str = r#"name: issue-1619-step-tiers
description: One agent step and one bash step
version: 1.0.0
context: {}
steps:
  - id: step-08-implement
    agent: "amplihack:builder"
    prompt: |
      Build it.
  - id: run-the-build
    type: bash
    command: "true"
"#;

/// Run `amplihack recipe run` with `tier_file` as the instance's
/// `model-tiers.toml`, and return everything it printed.
fn run_with(tier_file: &str) -> String {
    run_with_args(tier_file, &[]).text
}

/// What one run of the fixture produced.
struct Run {
    /// Everything the run printed, stdout and stderr in one file.
    text: String,
    /// The stub runner's own argv, one element per line.
    argv: Vec<String>,
    /// The directory the recipe named on the command line sits in.
    recipes_dir: PathBuf,
}

/// As `run_with`, with `extra` appended to the `recipe run` arguments, and
/// the runner's own argv alongside what the run printed.
fn run_with_args(tier_file: &str, extra: &[&str]) -> Run {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let (home, tools) = (root.join("home"), root.join("tools"));
    let amplihack_home = home.join(".amplihack");
    for dir in [&home, &tools, &amplihack_home] {
        fs::create_dir_all(dir).unwrap();
    }
    fs::write(amplihack_home.join("model-tiers.toml"), tier_file).unwrap();

    let runner: PathBuf = tools.join("recipe-runner-rs");
    crate::support::exec_stub::write_executable(
        &runner,
        format!("#!/bin/sh\n{CAPABILITIES}{STUB_RUNNER}"),
        0o755,
    );
    let recipe = root.join("recipe.yaml");
    fs::write(&recipe, RECIPE).unwrap();
    let log_path = root.join("round.log");
    let log = fs::File::create(&log_path).unwrap();
    let argv_path = root.join("runner.argv");

    let status = crate::isolated_amplihack::isolated(amplihack_bin())
        .args(["recipe", "run"])
        .arg(&recipe)
        .arg("-w")
        .arg(root)
        .args(extra)
        .current_dir(root)
        .env(
            "PATH",
            format!("{}:/usr/local/bin:/usr/bin:/bin", tools.display()),
        )
        .env("HOME", &home)
        .env("RECIPE_RUNNER_RS_PATH", &runner)
        .env("AMPLIHACK_1619_ARGV", &argv_path)
        .env("AMPLIHACK_AGENT_BINARY", "claude")
        .env("AMPLIHACK_HOME", &amplihack_home)
        .env("AMPLIHACK_NONINTERACTIVE", "1")
        .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
        .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
        .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
        .env("AMPLIHACK_SKIP_REFLECTION", "1")
        .stdin(Stdio::null())
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .status()
        .expect("failed to run amplihack");
    let text = fs::read_to_string(&log_path).unwrap();
    assert!(
        status.success(),
        "a tier preference must never stop a run; exit {:?}:\n{text}",
        status.code()
    );
    let argv = fs::read_to_string(&argv_path)
        .map(|raw| raw.lines().map(str::to_string).collect())
        .unwrap_or_default();
    Run {
        text,
        argv,
        recipes_dir: root.to_path_buf(),
    }
}

/// The values of every `-R` argument, in the order they appear.
fn search_dirs(argv: &[String]) -> Vec<String> {
    argv.windows(2)
        .filter(|pair| pair[0] == "-R")
        .map(|pair| pair[1].clone())
        .collect()
}

#[test]
fn a_step_tiers_value_that_is_not_a_tier_is_reported() {
    let text = run_with("version = 1\n\n[step_tiers]\nstep-08-implement = \"fastt\"\n");
    assert!(
        text.contains("step_tiers.step-08-implement") && text.contains("is not a tier"),
        "the operator is told which entry was dropped and why:\n{text}"
    );
}

#[test]
fn a_step_id_no_recipe_declares_is_reported() {
    let text = run_with("version = 1\n\n[step_tiers]\nstep-99-polish = \"fast\"\n");
    assert!(
        text.contains("step_tiers.step-99-polish") && text.contains("staged no recipe declaring"),
        "a typo'd step id has no other symptom, so it must have this one:\n{text}"
    );
}

#[test]
fn a_step_id_that_is_not_an_identifier_is_reported() {
    let text = run_with("version = 1\n\n[step_tiers]\n\"../../escape\" = \"fast\"\n");
    assert!(
        text.contains("a step id is an identifier"),
        "a rejected id is named back rather than dropped in silence:\n{text}"
    );
}

#[test]
fn a_tier_file_that_will_not_parse_is_reported() {
    let text = run_with("this is not toml = = =\n");
    assert!(
        text.contains("model-tiers.toml") && text.contains("not valid TOML"),
        "the file the operator wrote is named, home-relative:\n{text}"
    );
    assert!(
        text.contains("~/.amplihack/model-tiers.toml"),
        "run logs get pasted into issues; the absolute path carries the operator's \
         username:\n{text}"
    );
}

#[test]
fn a_step_id_a_recipe_declares_is_applied_without_a_word() {
    let text = run_with("version = 1\n\n[step_tiers]\nstep-08-implement = \"fast\"\n");
    assert!(
        !text.contains("step_tiers.step-08-implement"),
        "an override that matched has nothing to report:\n{text}"
    );
}

#[test]
fn a_run_with_no_step_tier_override_says_nothing_at_all() {
    let text = run_with("version = 1\n\n[claude.fast]\nmodel = \"claude-haiku-5-5\"\n");
    assert!(
        !text.contains("model tiers:"),
        "with nothing to report the run behaves as it did before this feature:\n{text}"
    );
}

#[test]
fn a_run_with_no_durable_directory_says_the_override_was_not_staged() {
    // A dry run owns scratch space only, so there is nowhere to stage the
    // patched recipe copies. Dropping the table there in silence is the one
    // failure this feature must not have: a dry run is where an operator
    // checks their `[step_tiers]`, and a mistyped step id has no other
    // symptom than the line it would otherwise print.
    let text = run_with_args(
        "version = 1\n\n[step_tiers]\nstep-08-implement = \"fast\"\n",
        &["--dry-run"],
    )
    .text;
    assert!(
        text.contains("no durable directory"),
        "a configured override that could not be staged is reported, not dropped:\n{text}"
    );
}

#[test]
fn a_step_id_that_names_a_bash_step_is_reported() {
    // A valid id of the wrong kind of step used to pass in silence: `patch`
    // matched `- id:` whatever the step was, wrote a `model:` the runner
    // ignores, and counted the id as matched, so there was no notice and no
    // effect.
    let text = run_with("version = 1\n\n[step_tiers]\nrun-the-build = \"fast\"\n");
    assert!(
        text.contains("not as an agent step"),
        "the operator is told the id exists but the step cannot take a model:\n{text}"
    );
    assert!(
        !text.contains("staged no recipe declaring"),
        "and is not also told the id is a typo:\n{text}"
    );
}

// -------------------------------------------------------------------------
// The delivery route, asserted as an order rather than as a constant
// -------------------------------------------------------------------------

#[test]
fn the_staging_directory_is_the_runners_first_search_directory() {
    let Run { text, argv, .. } = run_with_args(
        "version = 1\n\n[step_tiers]\nstep-08-implement = \"fast\"\n",
        &[],
    );
    let dirs = search_dirs(&argv);
    let first = dirs
        .first()
        .unwrap_or_else(|| panic!("the runner was given no -R at all:\n{text}\n{argv:?}"));
    assert!(
        first.ends_with("/recipe-overrides"),
        "the runner searches its -R list in order and amplihack forwards the \
         bundle, so the staged copies must come first or they are outranked \
         by the recipes they were patched from: {dirs:?}"
    );
    assert!(
        dirs.len() > 1,
        "the bundle is still forwarded after it, so an unpatched sub-recipe \
         is still found: {dirs:?}"
    );
}

#[test]
fn the_top_level_recipe_argument_is_the_staged_copy() {
    // `-R` is searched for a name; the top-level recipe is passed as a path,
    // so `-R` alone never reaches it. Without the substitution a
    // `[step_tiers]` entry naming a step of the recipe the operator ran has
    // no effect.
    let Run {
        text,
        argv,
        recipes_dir,
    } = run_with_args(
        "version = 1\n\n[step_tiers]\nstep-08-implement = \"fast\"\n",
        &[],
    );
    let recipe = argv
        .first()
        .unwrap_or_else(|| panic!("the runner was given no arguments:\n{text}"));
    assert!(
        recipe.contains("/recipe-overrides/"),
        "the positional recipe argument must be the patched copy: {recipe}"
    );
    let dirs = search_dirs(&argv);
    assert!(
        dirs.iter().any(|dir| recipes_dir.starts_with(dir)),
        "substituting the positional argument moves the runner's anchored \
         search to the staging directory, so the directory the original \
         recipe sits in has to stay reachable by name: {dirs:?}"
    );
}

#[test]
fn a_run_with_no_override_adds_no_search_directory_and_no_substitution() {
    let Run { text, argv, .. } = run_with_args(
        "version = 1\n\n[claude.fast]\nmodel = \"claude-haiku-5-5\"\n",
        &[],
    );
    let dirs = search_dirs(&argv);
    assert!(
        dirs.iter().all(|dir| !dir.ends_with("/recipe-overrides")),
        "with nothing configured the run behaves byte for byte as before: {dirs:?}\n{text}"
    );
    assert!(
        argv.first()
            .is_some_and(|recipe| !recipe.contains("/recipe-overrides/")),
        "and the recipe argument is the one the operator named: {argv:?}"
    );
}
