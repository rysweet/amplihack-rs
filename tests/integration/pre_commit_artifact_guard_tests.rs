//! tests/integration/pre_commit_artifact_guard_tests.rs
//!
//! Contracts for local pre-commit Artifact Guard wiring.
//!
//! The hook must scan repository state rather than only the filenames passed by
//! pre-commit, because issue #755 is about ignored/untracked generated artifacts
//! left in the parent worktree.

use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

const VALID_ARTIFACT_GUARD_ENTRY: &str = "bash -c 'CARGO_TARGET_DIR=\"${TMPDIR:-/tmp}/amplihack-precommit-target\" cargo run --bin amplihack -- hygiene artifact-guard --repo . --mode pre-commit'";
const VALID_LOCKED_ARTIFACT_GUARD_ENTRY: &str = "bash -c 'CARGO_TARGET_DIR=\"${TMPDIR:-/tmp}/amplihack-precommit-target\" cargo run --locked --bin amplihack -- hygiene artifact-guard --repo . --mode pre-commit'";

#[derive(Debug, Deserialize)]
struct PreCommitConfig {
    repos: Vec<PreCommitRepo>,
}

#[derive(Debug, Deserialize)]
struct PreCommitRepo {
    repo: String,
    #[serde(default)]
    hooks: Vec<PreCommitHook>,
}

#[derive(Debug, Default, Deserialize)]
struct PreCommitHook {
    id: String,
    entry: Option<String>,
    language: Option<String>,
    pass_filenames: Option<bool>,
    always_run: Option<bool>,
    files: Option<String>,
    types: Option<Vec<String>>,
}

#[derive(Debug)]
struct ParsedHookEntry {
    cargo_target_dir: Option<String>,
    cargo_args: Vec<String>,
    amplihack_args: Vec<String>,
}

fn workspace_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path
}

fn pre_commit_config() -> PreCommitConfig {
    let path = workspace_root().join(".pre-commit-config.yaml");
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

fn local_hooks(config: &PreCommitConfig) -> Vec<&PreCommitHook> {
    config
        .repos
        .iter()
        .filter(|repo| repo.repo == "local")
        .flat_map(|repo| repo.hooks.iter())
        .collect()
}

fn hook<'a>(hooks: &'a [&PreCommitHook], id: &str) -> &'a PreCommitHook {
    hooks
        .iter()
        .copied()
        .find(|hook| hook.id == id)
        .unwrap_or_else(|| panic!("missing local pre-commit hook `{id}`"))
}

fn valid_artifact_guard_hook(entry: &str) -> PreCommitHook {
    PreCommitHook {
        id: "artifact-guard".to_string(),
        entry: Some(entry.to_string()),
        ..PreCommitHook::default()
    }
}

fn contract_config(repo: &str, hooks: Vec<PreCommitHook>) -> PreCommitConfig {
    PreCommitConfig {
        repos: vec![PreCommitRepo {
            repo: repo.to_string(),
            hooks,
        }],
    }
}

fn assert_artifact_guard_contract(config: &PreCommitConfig) -> Result<(), String> {
    let matches: Vec<(&PreCommitRepo, &PreCommitHook)> = config
        .repos
        .iter()
        .flat_map(|repo| {
            repo.hooks
                .iter()
                .filter(|hook| hook.id == "artifact-guard")
                .map(move |hook| (repo, hook))
        })
        .collect();

    if matches.len() != 1 {
        return Err(format!(
            "expected exactly one Artifact Guard hook, found {}",
            matches.len()
        ));
    }

    let (repo, hook) = matches[0];
    if repo.repo != "local" {
        return Err(format!(
            "Artifact Guard hook must be defined under repo: local, got {}",
            repo.repo
        ));
    }

    let entry = hook
        .entry
        .as_deref()
        .ok_or_else(|| "Artifact Guard hook must declare an entry".to_string())?;
    assert_artifact_guard_entry_contract(entry)
}

fn assert_artifact_guard_entry_contract(entry: &str) -> Result<(), String> {
    let parsed = parse_hook_entry(entry)?;
    assert_cargo_target_dir_is_isolated(&parsed, entry)?;
    assert_cargo_bin_is_amplihack(&parsed, entry)?;
    assert_artifact_guard_args(&parsed, entry)?;
    Ok(())
}

fn parse_hook_entry(entry: &str) -> Result<ParsedHookEntry, String> {
    let outer_tokens =
        shell_words::split(entry).map_err(|e| format!("parse hook entry shell tokens: {e}"))?;
    let command_tokens = match outer_tokens.as_slice() {
        [shell, flag, command] if shell == "bash" && flag == "-c" => shell_words::split(command)
            .map_err(|e| format!("parse bash -c hook command shell tokens: {e}"))?,
        [shell, flag, ..] if shell == "bash" && flag == "-c" => {
            return Err(format!(
                "bash -c hook entry must contain exactly one command string; entry was `{entry}`"
            ));
        }
        tokens => tokens.to_vec(),
    };

    let mut cargo_target_dir = None;
    let mut command_index = 0;
    while let Some((name, value)) = command_tokens
        .get(command_index)
        .and_then(|token| env_assignment(token))
    {
        if name == "CARGO_TARGET_DIR" {
            cargo_target_dir = Some(value.to_string());
        }
        command_index += 1;
    }

    let command = command_tokens
        .get(command_index..)
        .ok_or_else(|| format!("hook entry must invoke cargo run; entry was `{entry}`"))?;
    if command.first().map(String::as_str) != Some("cargo")
        || command.get(1).map(String::as_str) != Some("run")
    {
        return Err(format!(
            "hook must invoke repo-local cargo fallback with `cargo run`; entry was `{entry}`"
        ));
    }

    let args_after_run = &command[2..];
    let separator_index = args_after_run
        .iter()
        .position(|arg| arg == "--")
        .ok_or_else(|| {
            format!("cargo fallback must separate Cargo args from amplihack args with `--`; entry was `{entry}`")
        })?;

    Ok(ParsedHookEntry {
        cargo_target_dir,
        cargo_args: args_after_run[..separator_index].to_vec(),
        amplihack_args: args_after_run[separator_index + 1..].to_vec(),
    })
}

fn env_assignment(token: &str) -> Option<(&str, &str)> {
    let (name, value) = token.split_once('=')?;
    let mut chars = name.chars();
    let first = chars.next()?;
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return None;
    }
    if !chars.all(|c| c == '_' || c.is_ascii_alphanumeric()) {
        return None;
    }
    Some((name, value))
}

fn assert_cargo_target_dir_is_isolated(
    parsed: &ParsedHookEntry,
    entry: &str,
) -> Result<(), String> {
    let target_dir = parsed
        .cargo_target_dir
        .as_deref()
        .ok_or_else(|| format!("cargo fallback must set CARGO_TARGET_DIR; entry was `{entry}`"))?;
    if !is_isolated_target_dir(target_dir) {
        return Err(format!(
            "cargo fallback must isolate CARGO_TARGET_DIR outside the repo; got `{target_dir}` in `{entry}`"
        ));
    }
    Ok(())
}

fn is_isolated_target_dir(value: &str) -> bool {
    if value == "${TMPDIR:-/tmp}/amplihack-precommit-target" {
        return true;
    }
    let path = Path::new(value);
    path.is_absolute() && !path.starts_with(workspace_root())
}

fn assert_cargo_bin_is_amplihack(parsed: &ParsedHookEntry, entry: &str) -> Result<(), String> {
    let bin_values = arg_values(&parsed.cargo_args, "--bin", "Cargo option", entry)?;
    match bin_values.as_slice() {
        [bin] if bin == "amplihack" => Ok(()),
        [] => Err(format!(
            "cargo fallback must invoke the repo-local amplihack binary with `--bin amplihack`; entry was `{entry}`"
        )),
        [bin] => Err(format!(
            "cargo fallback must invoke `--bin amplihack`, got `--bin {bin}` in `{entry}`"
        )),
        values => Err(format!(
            "cargo fallback must declare exactly one `--bin amplihack`, got {values:?} in `{entry}`"
        )),
    }
}

fn assert_artifact_guard_args(parsed: &ParsedHookEntry, entry: &str) -> Result<(), String> {
    if parsed.amplihack_args.first().map(String::as_str) != Some("hygiene")
        || parsed.amplihack_args.get(1).map(String::as_str) != Some("artifact-guard")
    {
        return Err(format!(
            "hook must invoke `hygiene artifact-guard` after the Cargo `--` separator; entry was `{entry}`"
        ));
    }

    assert_single_artifact_guard_arg(&parsed.amplihack_args[2..], "--repo", ".", entry)?;
    assert_single_artifact_guard_arg(&parsed.amplihack_args[2..], "--mode", "pre-commit", entry)?;
    Ok(())
}

fn arg_values(
    args: &[String],
    flag: &str,
    description: &str,
    entry: &str,
) -> Result<Vec<String>, String> {
    let mut values = Vec::new();
    let equals_prefix = format!("{flag}=");
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == flag {
            let value = args.get(index + 1).ok_or_else(|| {
                format!("{description} `{flag}` must include a value; entry was `{entry}`")
            })?;
            values.push(value.clone());
            index += 2;
        } else if let Some(value) = arg.strip_prefix(&equals_prefix) {
            values.push(value.to_string());
            index += 1;
        } else {
            index += 1;
        }
    }
    Ok(values)
}

fn assert_single_artifact_guard_arg(
    args: &[String],
    flag: &str,
    expected: &str,
    entry: &str,
) -> Result<(), String> {
    let values = arg_values(args, flag, "Artifact Guard argument", entry)?;
    match values.as_slice() {
        [value] if value == expected => Ok(()),
        [] => Err(format!(
            "Artifact Guard pre-commit hook must pass `{flag} {expected}`; entry was `{entry}`"
        )),
        [value] => Err(format!(
            "Artifact Guard pre-commit hook must pass `{flag} {expected}`, got `{flag} {value}` in `{entry}`"
        )),
        values => Err(format!(
            "Artifact Guard pre-commit hook must pass `{flag} {expected}` exactly once, got {values:?} in `{entry}`"
        )),
    }
}

#[test]
fn pre_commit_config_has_full_repo_artifact_guard_hook() {
    let config = pre_commit_config();
    let hooks = local_hooks(&config);
    let hook = hook(&hooks, "artifact-guard");

    assert_eq!(
        hook.pass_filenames,
        Some(false),
        "Artifact Guard must scan full repo state, not only pre-commit filenames"
    );
    assert_eq!(
        hook.language.as_deref(),
        Some("system"),
        "Artifact Guard should use the repo's system-command hook convention"
    );
    assert_eq!(
        hook.always_run,
        Some(true),
        "Artifact Guard must run even when only ignored/untracked artifacts are present"
    );
}

#[test]
fn pre_commit_artifact_guard_entry_uses_repo_cli_and_pre_commit_mode() {
    let config = pre_commit_config();

    assert_artifact_guard_contract(&config)
        .expect("checked-in Artifact Guard hook must satisfy the pre-commit contract");
}

#[test]
fn artifact_guard_contract_accepts_locked_cargo_option_between_run_and_bin() {
    let config = contract_config(
        "local",
        vec![valid_artifact_guard_hook(VALID_LOCKED_ARTIFACT_GUARD_ENTRY)],
    );

    assert_artifact_guard_contract(&config)
        .expect("cargo run --locked --bin amplihack must be a valid repo-local fallback");
}

#[test]
fn artifact_guard_contract_rejects_missing_or_nonlocal_hook_definition() {
    let missing = contract_config("local", vec![]);
    assert!(
        assert_artifact_guard_contract(&missing).is_err(),
        "contract must fail when the Artifact Guard hook is absent"
    );

    let remote = contract_config(
        "https://github.com/pre-commit/pre-commit-hooks",
        vec![valid_artifact_guard_hook(VALID_ARTIFACT_GUARD_ENTRY)],
    );
    assert!(
        assert_artifact_guard_contract(&remote).is_err(),
        "contract must fail when Artifact Guard is not defined as a repo-local hook"
    );

    let duplicate = contract_config(
        "local",
        vec![
            valid_artifact_guard_hook(VALID_ARTIFACT_GUARD_ENTRY),
            valid_artifact_guard_hook(VALID_ARTIFACT_GUARD_ENTRY),
        ],
    );
    assert!(
        assert_artifact_guard_contract(&duplicate).is_err(),
        "contract must fail when more than one Artifact Guard hook is defined"
    );
}

#[test]
fn artifact_guard_contract_rejects_missing_or_wrong_required_entry_parts() {
    let invalid_entries = [
        ("missing entry", ""),
        (
            "missing isolated target dir",
            "bash -c 'cargo run --bin amplihack -- hygiene artifact-guard --repo . --mode pre-commit'",
        ),
        (
            "non-isolated target dir",
            "bash -c 'CARGO_TARGET_DIR=target/precommit cargo run --bin amplihack -- hygiene artifact-guard --repo . --mode pre-commit'",
        ),
        (
            "missing repo-local amplihack bin",
            "bash -c 'CARGO_TARGET_DIR=\"${TMPDIR:-/tmp}/amplihack-precommit-target\" cargo run -- hygiene artifact-guard --repo . --mode pre-commit'",
        ),
        (
            "wrong cargo bin",
            "bash -c 'CARGO_TARGET_DIR=\"${TMPDIR:-/tmp}/amplihack-precommit-target\" cargo run --bin other -- hygiene artifact-guard --repo . --mode pre-commit'",
        ),
        (
            "missing artifact-guard command",
            "bash -c 'CARGO_TARGET_DIR=\"${TMPDIR:-/tmp}/amplihack-precommit-target\" cargo run --bin amplihack -- hygiene other --repo . --mode pre-commit'",
        ),
        (
            "missing repo argument",
            "bash -c 'CARGO_TARGET_DIR=\"${TMPDIR:-/tmp}/amplihack-precommit-target\" cargo run --bin amplihack -- hygiene artifact-guard --mode pre-commit'",
        ),
        (
            "wrong repo argument",
            "bash -c 'CARGO_TARGET_DIR=\"${TMPDIR:-/tmp}/amplihack-precommit-target\" cargo run --bin amplihack -- hygiene artifact-guard --repo /tmp --mode pre-commit'",
        ),
        (
            "missing mode argument",
            "bash -c 'CARGO_TARGET_DIR=\"${TMPDIR:-/tmp}/amplihack-precommit-target\" cargo run --bin amplihack -- hygiene artifact-guard --repo .'",
        ),
        (
            "wrong mode argument",
            "bash -c 'CARGO_TARGET_DIR=\"${TMPDIR:-/tmp}/amplihack-precommit-target\" cargo run --bin amplihack -- hygiene artifact-guard --repo . --mode pre-push'",
        ),
    ];

    for (case, entry) in invalid_entries {
        let config = contract_config("local", vec![valid_artifact_guard_hook(entry)]);
        assert!(
            assert_artifact_guard_contract(&config).is_err(),
            "contract must fail for {case}"
        );
    }
}

#[test]
fn pre_commit_artifact_guard_hook_is_not_limited_by_files_filter() {
    let config = pre_commit_config();
    let hooks = local_hooks(&config);
    let hook = hook(&hooks, "artifact-guard");

    assert!(
        hook.files.is_none() && hook.types.is_none(),
        "Artifact Guard hook must not use files/types filters because ignored-present artifacts may not be in the commit file list"
    );
}

#[test]
fn pre_commit_hook_order_runs_artifact_guard_before_format_lint_and_tests() {
    let config = pre_commit_config();
    let hooks = local_hooks(&config);
    let artifact_index = hooks
        .iter()
        .position(|hook| hook.id == "artifact-guard")
        .expect("artifact guard hook must exist");

    for later_hook in ["cargo-fmt", "cargo-clippy", "cargo-test"] {
        let later_index = hooks
            .iter()
            .position(|hook| hook.id == later_hook)
            .unwrap_or_else(|| panic!("missing expected hook `{later_hook}`"));
        assert!(
            artifact_index < later_index,
            "Artifact Guard should fail fast before `{later_hook}`"
        );
    }
}

#[test]
fn pre_commit_build_hooks_use_isolated_target_dir() {
    let config = pre_commit_config();
    let hooks = local_hooks(&config);

    for id in ["cargo-clippy", "cargo-test"] {
        let hook = hook(&hooks, id);
        let entry = hook
            .entry
            .as_deref()
            .unwrap_or_else(|| panic!("{id} must declare an entry"));
        assert!(
            entry.contains("CARGO_TARGET_DIR") && entry.contains("/tmp"),
            "{id} must isolate Cargo build output outside the parent worktree; entry was `{entry}`"
        );
    }
}
