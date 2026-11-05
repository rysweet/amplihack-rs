//! Contracts for local pre-commit Artifact Guard wiring.
//!
//! The hook must scan repository state rather than only the filenames passed by
//! pre-commit, because issue #755 is about ignored/untracked generated artifacts
//! left in the parent worktree.

use serde_yaml::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// The wrapper the build hooks run through (issue #1278). It removes git's
/// inherited repository selection -- GIT_DIR and friends, which git exports
/// into every hook and which override `.current_dir()` -- and pins
/// CARGO_TARGET_DIR outside the worktree.
const HOOK_WRAPPER: &str = "scripts/git-hook-cargo.sh";

const VALID_CARGO_ENTRY: &str = "bash -c 'CARGO_TARGET_DIR=\"${TMPDIR:-/tmp}/amplihack-precommit-target\" cargo run --bin amplihack -- hygiene artifact-guard --repo . --mode pre-commit'";
const VALID_LOCKED_BEFORE_BIN_ENTRY: &str = "bash -c 'CARGO_TARGET_DIR=\"${TMPDIR:-/tmp}/amplihack-precommit-target\" cargo run --locked --bin amplihack -- hygiene artifact-guard --repo . --mode pre-commit'";
const VALID_LOCKED_AFTER_BIN_ENTRY: &str = "bash -c 'CARGO_TARGET_DIR=\"${TMPDIR:-/tmp}/amplihack-precommit-target\" cargo run --bin amplihack --locked -- hygiene artifact-guard --repo . --mode pre-commit'";
const VALID_BIN_EQUALS_ENTRY: &str = "bash -c 'CARGO_TARGET_DIR=\"${TMPDIR:-/tmp}/amplihack-precommit-target\" cargo run --bin=amplihack -- hygiene artifact-guard --repo . --mode pre-commit'";

fn workspace_root() -> &'static PathBuf {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.pop();
        path.pop();
        path
    })
}

fn pre_commit_config() -> &'static Value {
    static CONFIG: OnceLock<Value> = OnceLock::new();
    CONFIG.get_or_init(|| {
        let path = workspace_root().join(".pre-commit-config.yaml");
        let text =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
    })
}

fn local_hooks(config: &Value) -> Vec<&Value> {
    config
        .get("repos")
        .and_then(Value::as_sequence)
        .expect("pre-commit config must contain repos")
        .iter()
        .filter(|repo| repo.get("repo").and_then(Value::as_str) == Some("local"))
        .flat_map(|repo| {
            repo.get("hooks")
                .and_then(Value::as_sequence)
                .expect("local repo must contain hooks")
        })
        .collect()
}

fn hook<'a>(hooks: &'a [&Value], id: &str) -> &'a Value {
    hooks
        .iter()
        .copied()
        .find(|hook| hook.get("id").and_then(Value::as_str) == Some(id))
        .unwrap_or_else(|| panic!("missing local pre-commit hook `{id}`"))
}

fn artifact_guard_entry_from_config() -> String {
    let hooks = local_hooks(pre_commit_config());
    hook(&hooks, "artifact-guard")
        .get("entry")
        .and_then(Value::as_str)
        .expect("artifact guard hook must declare an entry")
        .to_string()
}

/// Split a leading `scripts/git-hook-cargo.sh` off a hook entry.
///
/// Returns whether the wrapper was present along with the command it wraps.
fn strip_hook_wrapper(tokens: &[String]) -> (bool, &[String]) {
    match tokens.first().map(String::as_str) {
        Some(HOOK_WRAPPER) => (true, &tokens[1..]),
        _ => (false, tokens),
    }
}

/// Disposable state for executing the real wrapper without inherited run state.
struct WrapperFixture {
    root: tempfile::TempDir,
    home: PathBuf,
    cache: PathBuf,
    tmp: PathBuf,
    npm: PathBuf,
    work: PathBuf,
}

impl WrapperFixture {
    fn new() -> Result<Self, String> {
        // The command harness selects the base through TMPDIR. In regression
        // runs its written and physical paths must both exclude `/tmp`.
        let root = tempfile::Builder::new()
            .prefix("amplihack-hook-contract-")
            .tempdir()
            .map_err(|e| format!("create wrapper fixture: {e}"))?;
        let physical = root
            .path()
            .canonicalize()
            .map_err(|e| format!("resolve fixture {}: {e}", root.path().display()))?;
        let checkout = workspace_root()
            .canonicalize()
            .map_err(|e| format!("resolve checkout {}: {e}", workspace_root().display()))?;
        if physical.starts_with(&checkout) {
            return Err(format!(
                "wrapper fixture must be outside {}: {}",
                checkout.display(),
                physical.display()
            ));
        }
        let fixture = Self {
            home: physical.join("home"),
            cache: physical.join("cache"),
            tmp: physical.join("tmp"),
            npm: physical.join("npm"),
            work: physical.join("work"),
            root,
        };
        for path in [
            &fixture.home,
            &fixture.cache,
            &fixture.tmp,
            &fixture.npm,
            &fixture.work,
        ] {
            fs::create_dir(path).map_err(|e| format!("create {}: {e}", path.display()))?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            fs::set_permissions(&fixture.work, fs::Permissions::from_mode(0o700))
                .map_err(|e| format!("make work root private {}: {e}", fixture.work.display()))?;
            let metadata = fs::symlink_metadata(&fixture.work)
                .map_err(|e| format!("inspect work root {}: {e}", fixture.work.display()))?;
            // SAFETY: geteuid has no arguments and does not access a profile.
            let uid = unsafe { libc::geteuid() };
            if !metadata.file_type().is_dir()
                || metadata.uid() != uid
                || metadata.permissions().mode() & 0o777 != 0o700
            {
                return Err(format!(
                    "work root must be a private, owned, non-symlink directory: {}",
                    fixture.work.display()
                ));
            }
        }
        let work = fixture
            .work
            .canonicalize()
            .map_err(|e| format!("resolve work root {}: {e}", fixture.work.display()))?;
        let home = fixture
            .home
            .canonicalize()
            .map_err(|e| format!("resolve fixture HOME {}: {e}", fixture.home.display()))?;
        if work.starts_with(&home) || home.starts_with(&work) {
            return Err(format!(
                "fixture work root and HOME must be separate: {} and {}",
                work.display(),
                home.display()
            ));
        }
        Ok(fixture)
    }

    fn target_dir(&self, entry: &str) -> Result<String, String> {
        let path = workspace_root().join(HOOK_WRAPPER);
        let script = fs::read_to_string(&path)
            .map_err(|e| format!("read {} for `{entry}`: {e}", path.display()))?;
        if !script
            .lines()
            .map(str::trim)
            .any(|line| line.starts_with("export CARGO_TARGET_DIR"))
        {
            return Err(format!(
                "{HOOK_WRAPPER} must export CARGO_TARGET_DIR; entry was `{entry}`"
            ));
        }
        let output = Command::new("bash")
            .arg(&path)
            .args(["printenv", "CARGO_TARGET_DIR"])
            .current_dir(workspace_root())
            .stdin(std::process::Stdio::null())
            // Exclude ambient target, run metadata, shell startup files,
            // profile selectors and credential-related configuration.
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("LC_ALL", "C")
            .env("HOME", &self.home)
            .env("XDG_CACHE_HOME", &self.cache)
            .env("TMPDIR", &self.tmp)
            .env("npm_config_prefix", &self.npm)
            .env("AMPLIHACK_WORK_ROOT", &self.work)
            .env("AMPLIHACK_PRECOMMIT_CACHE_TTL_DAYS", "0")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .map_err(|e| format!("run {} for `{entry}`: {e}", path.display()))?;
        if !output.status.success() {
            return Err(format!(
                "{HOOK_WRAPPER} exited {} instead of reporting CARGO_TARGET_DIR; entry was `{entry}`; stderr: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        let resolved = String::from_utf8(output.stdout)
            .map_err(|e| format!("{HOOK_WRAPPER} returned non-UTF-8 output for `{entry}`: {e}"))?
            .trim()
            .to_string();
        if resolved.is_empty() {
            return Err(format!(
                "{HOOK_WRAPPER} exported an empty CARGO_TARGET_DIR; entry was `{entry}`"
            ));
        }
        Ok(resolved)
    }

    fn close(self) -> Result<(), String> {
        let path = self.root.path().to_path_buf();
        self.root
            .close()
            .map_err(|e| format!("remove wrapper fixture {}: {e}", path.display()))
    }
}

/// Keep the wrapper export check and execute the checked-in script.
fn wrapper_target_dir(entry: &str) -> Result<String, String> {
    let fixture = WrapperFixture::new()?;
    let result = fixture.target_dir(entry);
    let cleanup = fixture.close();
    match (result, cleanup) {
        (Ok(target), Ok(())) => Ok(target),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(cleanup)) => Err(format!("{error}; {cleanup}")),
    }
}

/// A build cache is isolated when it lands OUTSIDE the worktree. Not when it
/// lands on /tmp.
///
/// This check used to be `target_dir.contains("/tmp")`, and that is how the
/// per-checkout caches came to fill the host's temp filesystem (issue #1440):
/// a contract meant to keep build output out of the repository also pinned it
/// to the smallest, most heavily shared filesystem on the machine, 12 dirs and
/// 122 GB at a time. Where the cache lives is a storage decision; all this
/// contract is entitled to require is that it is not in the checkout.
fn assert_isolated_target_dir(target_dir: &str, entry: &str) -> Result<(), String> {
    let outside_worktree = match target_dir.chars().next() {
        // An absolute path -- what the wrapper resolves to -- must not point
        // back into the checkout.
        Some('/') => !Path::new(target_dir).starts_with(workspace_root()),
        // An unexpanded shell expression, which an inline entry carries
        // verbatim: `${TMPDIR:-/tmp}/...`. The shell resolves it, not this
        // test, and it cannot land in the worktree without naming it.
        Some('$') => true,
        _ => false,
    };
    if !outside_worktree {
        return Err(format!(
            "cargo fallback must isolate CARGO_TARGET_DIR outside the repo; \
             got `{target_dir}` in `{entry}`"
        ));
    }
    Ok(())
}

fn assert_artifact_guard_entry_contract(entry: &str) -> Result<(), String> {
    let tokens = hook_command_tokens(entry)?;
    let (wrapped, tokens) = strip_hook_wrapper(&tokens);
    let (inline_target_dir, command) = split_env_assignments(tokens);

    // The wrapper supplies CARGO_TARGET_DIR for the entries that go through it,
    // so an entry may carry the assignment inline or inherit it from the script.
    let from_wrapper = if wrapped {
        Some(wrapper_target_dir(entry)?)
    } else {
        None
    };
    let cargo_target_dir = inline_target_dir.or(from_wrapper.as_deref());

    let guard_args = if command.first().map(String::as_str) == Some("cargo") {
        assert_cargo_fallback(command, cargo_target_dir, entry)?
    } else {
        command
    };

    assert_guard_invocation(guard_args, entry)
}

fn hook_command_tokens(entry: &str) -> Result<Vec<String>, String> {
    let outer = shell_words::split(entry).map_err(|e| format!("parse hook entry: {e}"))?;
    match outer.as_slice() {
        [shell, flag, command] if shell == "bash" && flag == "-c" => {
            shell_words::split(command).map_err(|e| format!("parse bash -c command: {e}"))
        }
        [shell, flag, ..] if shell == "bash" && flag == "-c" => Err(format!(
            "bash -c hook entry must contain exactly one command string; entry was `{entry}`"
        )),
        _ => Ok(outer),
    }
}

fn split_env_assignments(tokens: &[String]) -> (Option<&str>, &[String]) {
    let mut cargo_target_dir = None;
    let mut command_index = 0;
    while let Some((name, value)) = tokens
        .get(command_index)
        .and_then(|token| token.split_once('='))
        .filter(|(name, _)| is_shell_name(name))
    {
        if name == "CARGO_TARGET_DIR" {
            cargo_target_dir = Some(value);
        }
        command_index += 1;
    }
    (cargo_target_dir, &tokens[command_index..])
}

fn is_shell_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

fn assert_cargo_fallback<'a>(
    command: &'a [String],
    cargo_target_dir: Option<&str>,
    entry: &str,
) -> Result<&'a [String], String> {
    if command.get(1).map(String::as_str) != Some("run") {
        return Err(format!(
            "cargo fallback must invoke `cargo run`; entry was `{entry}`"
        ));
    }
    let target_dir = cargo_target_dir
        .ok_or_else(|| format!("cargo fallback must set CARGO_TARGET_DIR; entry was `{entry}`"))?;
    assert_isolated_target_dir(target_dir, entry)?;

    let separator = command[2..].iter().position(|arg| arg == "--").ok_or_else(|| {
        format!("cargo fallback must separate Cargo args from amplihack args with `--`; entry was `{entry}`")
    })?;
    let (cargo_args, guard_args) = command[2..].split_at(separator);
    assert_cargo_options(cargo_args, entry)?;
    Ok(&guard_args[1..])
}

fn assert_cargo_options(args: &[String], entry: &str) -> Result<(), String> {
    let mut bin_values = Vec::new();
    let mut locked_count = 0;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--locked" => {
                locked_count += 1;
                index += 1;
            }
            "--bin" => {
                let value = args.get(index + 1).ok_or_else(|| {
                    format!("Cargo option `--bin` must include a value; entry was `{entry}`")
                })?;
                bin_values.push(value.as_str());
                index += 2;
            }
            arg => {
                if let Some(value) = arg.strip_prefix("--bin=") {
                    bin_values.push(value);
                    index += 1;
                } else {
                    return Err(format!(
                        "cargo fallback only permits `--locked` and `--bin amplihack` before `--`; got `{arg}` in `{entry}`"
                    ));
                }
            }
        }
    }
    if locked_count > 1 {
        return Err(format!(
            "cargo fallback must pass `--locked` at most once; entry was `{entry}`"
        ));
    }
    assert_single_value(&bin_values, "--bin", "amplihack", entry)
}

fn assert_guard_invocation(args: &[String], entry: &str) -> Result<(), String> {
    if args.first().map(String::as_str) != Some("amplihack")
        && (args.first().map(String::as_str) != Some("hygiene")
            || args.get(1).map(String::as_str) != Some("artifact-guard"))
    {
        return Err(format!(
            "hook must invoke `hygiene artifact-guard`; entry was `{entry}`"
        ));
    }
    let option_start = if args.first().map(String::as_str) == Some("amplihack") {
        if args.get(1).map(String::as_str) != Some("hygiene")
            || args.get(2).map(String::as_str) != Some("artifact-guard")
        {
            return Err(format!(
                "hook must invoke `amplihack hygiene artifact-guard`; entry was `{entry}`"
            ));
        }
        3
    } else {
        2
    };

    let mut repo_values = Vec::new();
    let mut mode_values = Vec::new();
    let mut index = option_start;
    while index < args.len() {
        match args[index].as_str() {
            "--repo" => {
                let value = args.get(index + 1).ok_or_else(|| {
                    format!(
                        "Artifact Guard argument `--repo` must include a value; entry was `{entry}`"
                    )
                })?;
                repo_values.push(value.as_str());
                index += 2;
            }
            "--mode" => {
                let value = args.get(index + 1).ok_or_else(|| {
                    format!(
                        "Artifact Guard argument `--mode` must include a value; entry was `{entry}`"
                    )
                })?;
                mode_values.push(value.as_str());
                index += 2;
            }
            arg => {
                if let Some(value) = arg.strip_prefix("--repo=") {
                    repo_values.push(value);
                    index += 1;
                } else if let Some(value) = arg.strip_prefix("--mode=") {
                    mode_values.push(value);
                    index += 1;
                } else {
                    return Err(format!(
                        "Artifact Guard pre-commit hook must not pass unexpected argument `{arg}`; entry was `{entry}`"
                    ));
                }
            }
        }
    }

    assert_single_value(&repo_values, "--repo", ".", entry)?;
    assert_single_value(&mode_values, "--mode", "pre-commit", entry)
}

fn assert_single_value(
    values: &[&str],
    flag: &str,
    expected: &str,
    entry: &str,
) -> Result<(), String> {
    match values {
        [value] if *value == expected => Ok(()),
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
    let hooks = local_hooks(pre_commit_config());
    let hook = hook(&hooks, "artifact-guard");

    assert_eq!(
        hook.get("pass_filenames").and_then(Value::as_bool),
        Some(false),
        "Artifact Guard must scan full repo state, not only pre-commit filenames"
    );
    assert_eq!(
        hook.get("language").and_then(Value::as_str),
        Some("system"),
        "Artifact Guard should use the repo's system-command hook convention"
    );
    assert_eq!(
        hook.get("always_run").and_then(Value::as_bool),
        Some(true),
        "Artifact Guard must run even when only ignored/untracked artifacts are present"
    );
}

#[test]
fn pre_commit_artifact_guard_entry_uses_repo_cli_and_pre_commit_mode() {
    assert_artifact_guard_entry_contract(&artifact_guard_entry_from_config())
        .expect("checked-in Artifact Guard hook must satisfy the pre-commit contract");
}

#[test]
fn artifact_guard_contract_accepts_legal_cargo_option_ordering() {
    for entry in [
        VALID_CARGO_ENTRY,
        VALID_LOCKED_BEFORE_BIN_ENTRY,
        VALID_LOCKED_AFTER_BIN_ENTRY,
        VALID_BIN_EQUALS_ENTRY,
        "amplihack hygiene artifact-guard --repo . --mode pre-commit",
    ] {
        assert_artifact_guard_entry_contract(entry)
            .unwrap_or_else(|error| panic!("contract should accept `{entry}`: {error}"));
    }
}

#[test]
fn artifact_guard_contract_rejects_invalid_entries() {
    for (case, entry) in [
        (
            "missing target dir",
            "bash -c 'cargo run --bin amplihack -- hygiene artifact-guard --repo . --mode pre-commit'",
        ),
        (
            "target dir inside the worktree",
            "bash -c 'CARGO_TARGET_DIR=target cargo run --bin amplihack -- hygiene artifact-guard --repo . --mode pre-commit'",
        ),
        (
            "wrong cargo bin",
            "bash -c 'CARGO_TARGET_DIR=\"${TMPDIR:-/tmp}/amplihack-precommit-target\" cargo run --bin other -- hygiene artifact-guard --repo . --mode pre-commit'",
        ),
        (
            "unexpected cargo option",
            "bash -c 'CARGO_TARGET_DIR=\"${TMPDIR:-/tmp}/amplihack-precommit-target\" cargo run --manifest-path /tmp/Cargo.toml --bin amplihack -- hygiene artifact-guard --repo . --mode pre-commit'",
        ),
        (
            "duplicate locked option",
            "bash -c 'CARGO_TARGET_DIR=\"${TMPDIR:-/tmp}/amplihack-precommit-target\" cargo run --locked --locked --bin amplihack -- hygiene artifact-guard --repo . --mode pre-commit'",
        ),
        (
            "wrong guard command",
            "bash -c 'CARGO_TARGET_DIR=\"${TMPDIR:-/tmp}/amplihack-precommit-target\" cargo run --bin amplihack -- hygiene other --repo . --mode pre-commit'",
        ),
        (
            "wrong repo",
            "bash -c 'CARGO_TARGET_DIR=\"${TMPDIR:-/tmp}/amplihack-precommit-target\" cargo run --bin amplihack -- hygiene artifact-guard --repo /tmp --mode pre-commit'",
        ),
        (
            "wrong mode",
            "bash -c 'CARGO_TARGET_DIR=\"${TMPDIR:-/tmp}/amplihack-precommit-target\" cargo run --bin amplihack -- hygiene artifact-guard --repo . --mode pre-push'",
        ),
        (
            "extra guard argument",
            "bash -c 'CARGO_TARGET_DIR=\"${TMPDIR:-/tmp}/amplihack-precommit-target\" cargo run --bin amplihack -- hygiene artifact-guard --repo . --mode pre-commit --allowlist target'",
        ),
        (
            "extra shell command",
            "bash -c 'CARGO_TARGET_DIR=\"${TMPDIR:-/tmp}/amplihack-precommit-target\" cargo run --bin amplihack -- hygiene artifact-guard --repo . --mode pre-commit && true'",
        ),
    ] {
        assert!(
            assert_artifact_guard_entry_contract(entry).is_err(),
            "contract must fail for {case}"
        );
    }
}

#[test]
fn pre_commit_artifact_guard_hook_is_not_limited_by_files_filter() {
    let hooks = local_hooks(pre_commit_config());
    let hook = hook(&hooks, "artifact-guard");

    assert!(
        hook.get("files").is_none() && hook.get("types").is_none(),
        "Artifact Guard hook must not use files/types filters because ignored-present artifacts may not be in the commit file list"
    );
}

#[test]
fn pre_commit_hook_order_runs_artifact_guard_before_format_lint_and_tests() {
    let hooks = local_hooks(pre_commit_config());
    let artifact_index = hooks
        .iter()
        .position(|hook| hook.get("id").and_then(Value::as_str) == Some("artifact-guard"))
        .expect("artifact guard hook must exist");

    for later_hook in ["cargo-fmt", "cargo-clippy", "cargo-test"] {
        let later_index = hooks
            .iter()
            .position(|hook| hook.get("id").and_then(Value::as_str) == Some(later_hook))
            .unwrap_or_else(|| panic!("missing expected hook `{later_hook}`"));
        assert!(
            artifact_index < later_index,
            "Artifact Guard should fail fast before `{later_hook}`"
        );
    }
}

#[test]
fn pre_commit_build_hooks_use_isolated_target_dir() {
    let hooks = local_hooks(pre_commit_config());

    for id in ["cargo-clippy", "cargo-test"] {
        let hook = hook(&hooks, id);
        let entry = hook
            .get("entry")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("{id} must declare an entry"));
        let isolation = (|| -> Result<(), String> {
            let tokens = hook_command_tokens(entry)?;
            let (wrapped, tokens) = strip_hook_wrapper(&tokens);
            let (inline_target_dir, _) = split_env_assignments(tokens);
            // Execute every named wrapper, including entries with an inline
            // override, so a broken wrapper cannot pass the contract.
            let from_wrapper = if wrapped {
                Some(wrapper_target_dir(entry)?)
            } else {
                None
            };
            let target_dir = inline_target_dir
                .or(from_wrapper.as_deref())
                .ok_or_else(|| {
                    "hook must set CARGO_TARGET_DIR inline or through the wrapper".to_string()
                })?;
            assert_isolated_target_dir(target_dir, entry)
        })();
        isolation.unwrap_or_else(|error| {
            panic!(
                "{id} must isolate Cargo build output outside the parent worktree; entry was `{entry}`: {error}"
            )
        });
    }
}

#[test]
fn pre_commit_wrapper_uses_owned_custom_work_root() {
    let fixture = WrapperFixture::new().expect("create private wrapper fixture");
    let entry = "scripts/git-hook-cargo.sh printenv CARGO_TARGET_DIR";
    let target = fixture
        .target_dir(entry)
        .expect("actual wrapper must report a non-empty target successfully");
    let target_path = Path::new(&target);
    assert!(
        target_path.is_absolute(),
        "wrapper target must be absolute: {target}"
    );
    assert_isolated_target_dir(&target, entry)
        .expect("custom-root target must satisfy the existing isolation contract");

    // printenv does not create the final Cargo target. Resolve its existing
    // parent physically and append only the missing final component.
    let physical_target = match target_path.canonicalize() {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            assert_eq!(
                target_path.components().next_back(),
                target_path.file_name().map(std::path::Component::Normal),
                "target must end in a normal path component: {target}"
            );
            let parent = target_path.parent().expect("absolute target has a parent");
            parent
                .canonicalize()
                .unwrap_or_else(|e| panic!("resolve target parent {}: {e}", parent.display()))
                .join(
                    target_path
                        .file_name()
                        .expect("target has a final component"),
                )
        }
        Err(error) => panic!("resolve wrapper target {target}: {error}"),
    };
    let targets = fixture
        .work
        .join("targets")
        .canonicalize()
        .expect("wrapper must create custom work-root targets directory");
    assert!(
        physical_target.starts_with(&targets) && physical_target != targets,
        "wrapper must select a target strictly beneath the owned custom root {}; got {}",
        targets.display(),
        physical_target.display()
    );
    fixture.close().expect("remove private wrapper fixture");
}

#[test]
fn target_dir_contract_rejects_absolute_in_checkout_paths() {
    for target in [workspace_root().clone(), workspace_root().join("target")] {
        assert!(target.is_absolute(), "checkout fixture must be absolute");
        let target = target.to_str().expect("checkout path must be UTF-8");
        let error = assert_isolated_target_dir(target, "absolute checkout regression")
            .expect_err("contract must reject an absolute target inside the checkout");
        assert!(
            error.contains(target),
            "isolation error must name rejected target: {error}"
        );
    }
}
