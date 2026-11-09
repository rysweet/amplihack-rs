//! Issue #1671: a git hook that rejects a checkpoint or publish commit, or a
//! publish push, goes to a fixing agent and the step retries, until the
//! loop-health evaluator ends the loop.
//!
//! Contract: `docs/reference/recipe-commit-hook-repair.md`.
//!
//! What is pinned here:
//!
//! - the fixing recipe, `hook-rejection-repair.yaml`, exists, is registered in
//!   `_recipe_manifest.json`, has exactly one agent step and no bash step, has
//!   no per-step timeout (#439), forbids hook bypasses in its prompt, and
//!   carries no routing marker (#1333);
//! - the driver and staging helper exist beside `git-identity.sh`;
//! - the three recipes the change touches stay inside the brick budget;
//! - the reference page is linked from the nav, the index and the executor
//!   environment page, and the shell test is wired into CI;
//! - the executable shell contract passes;
//! - #2043: the shell suite removes each case's repositories when the case
//!   ends, so its entry count stays flat from one case boundary to the next,
//!   and the fixture lives off tmpfs when the caller allows it
//!   (`docs/reference/recipe-scratch-policy.md`, "Bulk test fixtures").
//!
//! The shell test drives the steps with a stub `amplihack` it writes itself,
//! so this file never starts the real binary and needs no `isolated(..)`.

use serde_yaml::Value;
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const BRICK_LINE_BUDGET: usize = 400;

/// The same list as `issue_1333_leaf_steps_do_not_orchestrate.rs`. A leaf
/// agent step told to orchestrate starts a nested run from inside a commit
/// step, which is the failure #1333 was about.
const ROUTING_MARKERS: &[&str] = &[
    "auto-intent-router",
    "auto-routed",
    "AMPLIHACK_CONTEXT_START",
    "recipe run smart-orchestrator",
    "Skill(skill=\"dev-orchestrator\")",
    "invoke the dev-orchestrator skill",
    "launching dev-orchestrator",
];

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|path| path.join("amplifier-bundle/recipes").is_dir())
        .map(Path::to_path_buf)
        .expect("workspace must contain amplifier-bundle/recipes")
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn recipe_path() -> PathBuf {
    workspace_root().join("amplifier-bundle/recipes/hook-rejection-repair.yaml")
}

fn recipe_text() -> String {
    read(&recipe_path())
}

fn recipe_yaml() -> Value {
    serde_yaml::from_str(&recipe_text())
        .unwrap_or_else(|e| panic!("parse {}: {e}", recipe_path().display()))
}

fn steps(recipe: &Value) -> Vec<Value> {
    recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .cloned()
        .expect("hook-rejection-repair must declare top-level steps")
}

fn only_step() -> Value {
    let steps = steps(&recipe_yaml());
    assert_eq!(
        steps.len(),
        1,
        "hook-rejection-repair must have exactly one step, the fixing agent"
    );
    steps.into_iter().next().unwrap()
}

fn str_field<'a>(step: &'a Value, name: &str) -> Option<&'a str> {
    step.get(name).and_then(Value::as_str)
}

fn prompt() -> String {
    str_field(&only_step(), "prompt")
        .expect("the fixing step must have a prompt")
        .to_string()
}

#[test]
fn fixing_recipe_exists_and_fits_the_brick_budget() {
    let lines = recipe_text().lines().count();
    assert!(
        lines <= BRICK_LINE_BUDGET,
        "hook-rejection-repair.yaml is {lines} lines; the brick budget is {BRICK_LINE_BUDGET}"
    );
    assert_eq!(
        recipe_yaml().get("name").and_then(Value::as_str),
        Some("hook-rejection-repair"),
        "`name:` must match the filename stem; `recipe run` resolves by stem"
    );
}

#[test]
fn fixing_recipe_has_one_agent_step_and_no_bash() {
    let step = only_step();
    assert_eq!(str_field(&step, "id"), Some("fix-hook-rejection"));
    assert_eq!(
        str_field(&step, "agent"),
        Some("amplihack:pre-commit-diagnostic")
    );
    assert_eq!(
        str_field(&step, "working_dir"),
        Some("{{hook_repair_worktree}}"),
        "the agent works in the task worktree"
    );
    assert_ne!(
        str_field(&step, "type"),
        Some("bash"),
        "SR-2: no bash step may interpolate hook_repair_* values"
    );
    assert!(
        step.get("command").is_none(),
        "SR-2: the fixing step must not carry a shell command"
    );
}

#[test]
fn fixing_recipe_has_no_timeout_or_iteration_cap() {
    let step = only_step();
    assert!(
        step.get("timeout").is_none() && step.get("timeout_seconds").is_none(),
        "#439: no per-step timeout on the fixing agent"
    );
    let text = recipe_text();
    for cap in [
        "max_attempts",
        "max_rounds",
        "max_iterations",
        "max_retries",
    ] {
        assert!(
            !text.contains(cap),
            "the loop is ended by the evaluator, not a count; found `{cap}`"
        );
    }
}

#[test]
fn fixing_recipe_receives_hook_output_only_through_the_evidence_file() {
    let text = recipe_text();
    for input in [
        "hook_repair_worktree",
        "hook_repair_evidence_file",
        "hook_repair_operation",
        "hook_repair_step",
        "hook_repair_hook_ids",
    ] {
        assert!(
            text.contains(input),
            "the fixing recipe must declare and use `{input}`"
        );
    }
    // SR-10: until single-pass template rendering is confirmed, hook output
    // containing `{{...}}` must never be rendered into the prompt.
    assert!(
        !text.contains("hook_repair_output_tail"),
        "the output tail must not reach the fixing recipe until SR-10 is confirmed"
    );
}

#[test]
fn fixing_prompt_forbids_bypasses_and_states_the_output_contract() {
    let prompt = prompt();
    for needle in [
        "--no-verify",
        "SKIP=",
        "core.hooksPath",
        "git -c",
        ".pre-commit-config.yaml",
        "allowlist",
        "not_fixable_in_code",
        "environment",
        "\"hook_repair\"",
        "\"status\"",
        "\"class\"",
        "\"summary\"",
        "hook_repair_evidence_file",
    ] {
        assert!(
            prompt.contains(needle),
            "the fixing prompt must mention `{needle}`"
        );
    }
    let lower = prompt.to_lowercase();
    for git_write in ["commit", "push", "stash", "reset", "update-ref"] {
        assert!(
            lower.contains(git_write),
            "the fixing prompt must forbid `git {git_write}`; the driver owns git writes"
        );
    }
}

#[test]
fn fixing_recipe_carries_no_routing_marker() {
    let text = recipe_text();
    for marker in ROUTING_MARKERS {
        assert!(
            !text.contains(marker),
            "#1333: the fixing recipe must not contain the routing marker `{marker}`"
        );
    }
    assert!(
        !text.contains("recipe run"),
        "the fixing agent must not start nested recipe runs"
    );
}

#[test]
fn fixing_recipe_is_registered_in_the_recipe_manifest() {
    let path = workspace_root().join("amplifier-bundle/recipes/_recipe_manifest.json");
    let manifest: serde_json::Value =
        serde_json::from_str(&read(&path)).expect("_recipe_manifest.json must be valid JSON");
    let hash = manifest
        .get("hook-rejection-repair")
        .and_then(serde_json::Value::as_str)
        .expect("hook-rejection-repair must be registered in _recipe_manifest.json");
    assert!(
        hash.len() == 16 && hash.chars().all(|c| c.is_ascii_hexdigit()),
        "the manifest entry must be a 16-hex hash like its neighbours, got `{hash}`"
    );
}

#[test]
fn helpers_live_beside_git_identity() {
    let tools = workspace_root().join("amplifier-bundle/tools");
    assert!(tools.join("git-identity.sh").is_file());
    for helper in ["workflow_hook_repair.sh", "workflow_stage_changes.sh"] {
        let path = tools.join(helper);
        assert!(path.is_file(), "missing {}", path.display());
        let text = read(&path);
        assert!(
            !text.contains("--no-verify"),
            "{helper} must never pass --no-verify"
        );
        assert!(
            !text.lines().any(|l| {
                let l = l.trim_start();
                !l.starts_with('#') && (l.contains("export SKIP") || l.contains("unset SKIP"))
            }),
            "SR-1: {helper} must neither export nor unset SKIP"
        );
        assert!(
            !text.lines().any(|l| {
                let l = l.trim_start();
                !l.starts_with('#') && l.contains("trap ")
            }),
            "{helper} must install no traps; step-15 owns its EXIT trap"
        );
        assert!(
            !text.lines().any(|l| {
                let l = l.trim_start();
                !l.starts_with('#') && (l.contains("eval ") || l.contains("sh -c"))
            }),
            "SR-9: {helper} must not run commands through eval or sh -c"
        );
    }
}

#[test]
fn touched_recipes_stay_inside_the_brick_budget() {
    let recipes = workspace_root().join("amplifier-bundle/recipes");
    for name in [
        "workflow-tdd.yaml",
        "workflow-refactor-review.yaml",
        "workflow-publish.yaml",
        "hook-rejection-repair.yaml",
    ] {
        let lines = read(&recipes.join(name)).lines().count();
        assert!(
            lines <= BRICK_LINE_BUDGET,
            "{name} is {lines} lines; the brick budget is {BRICK_LINE_BUDGET}"
        );
    }
}

#[test]
fn reference_page_is_linked_and_names_the_follow_up() {
    let root = workspace_root();
    let page = root.join("docs/reference/recipe-commit-hook-repair.md");
    let text = read(&page);
    for needle in [
        "not_fixable_in_code",
        "#1673",
        "Claude Code hooks",
        "launched `amplihack recipe run`",
    ] {
        assert!(
            text.contains(needle),
            "the reference page must cover `{needle}`"
        );
    }
    for (file, link) in [
        ("mkdocs.yml", "reference/recipe-commit-hook-repair.md"),
        ("docs/index.md", "recipe-commit-hook-repair.md"),
        (
            "docs/reference/recipe-executor-environment.md",
            "recipe-commit-hook-repair.md",
        ),
    ] {
        assert!(
            read(&root.join(file)).contains(link),
            "{file} must link the hook-repair reference page"
        );
    }
}

#[test]
fn shell_test_is_wired_into_ci() {
    let ci = read(&workspace_root().join(".github/workflows/ci.yml"));
    assert!(
        ci.contains("amplifier-bundle/recipes/tests/test-issue-1671-hook-rejection-repair.sh"),
        "ci.yml must run the #1671 shell contract like the other recipe shell tests"
    );
}

/// #2043: cases in the 1671 suite: `mutation_canary` plus 13 `section_*`.
const EXPECTED_CASES: usize = 14;

/// #2043: must equal `CASE_SLACK` in the shell suite. Covers small writes to
/// the shared home config and stub files between case boundaries.
const CASE_SLACK: usize = 64;

/// The 14 case names, in run order.
const CASE_NAMES: [&str; EXPECTED_CASES] = [
    "mutation_canary",
    "section_a",
    "section_b",
    "section_c",
    "section_d",
    "section_e",
    "section_f",
    "section_g",
    "section_h_static",
    "section_h_dynamic",
    "section_i",
    "section_j",
    "section_k",
    "section_l",
];

fn script_path() -> PathBuf {
    workspace_root().join("amplifier-bundle/recipes/tests/test-issue-1671-hook-rejection-repair.sh")
}

/// The root that holds this test's bulk fixture: `TMPDIR` unless it is on
/// tmpfs, then `CARGO_TARGET_TMPDIR`, then `/tmp`.
fn fixture_root() -> PathBuf {
    let tmpdir = std::env::var_os("TMPDIR").map(PathBuf::from);
    let root = pick_fixture_root(
        tmpdir.as_deref(),
        &is_tmpfs,
        option_env!("CARGO_TARGET_TMPDIR"),
    );
    assert!(
        root == Path::new("/tmp") || !is_tmpfs(&root),
        "#2043: fixture root {} is on tmpfs; set TMPDIR to a directory on a disk",
        root.display()
    );
    root
}

/// #2043 D5: the root for a bulk test fixture.
///
/// `TMPDIR` when it is set, non-empty and not on tmpfs; otherwise
/// `target_tmp` (the build's `CARGO_TARGET_TMPDIR`); otherwise `/tmp`. A
/// suite that creates thousands of repositories must not fill the inode
/// table of a tmpfs `/tmp` shared by every process on the host. Pure:
/// `is_tmpfs` is injected so the choice can be tested without real mounts.
fn pick_fixture_root(
    tmpdir: Option<&Path>,
    is_tmpfs: &dyn Fn(&Path) -> bool,
    target_tmp: Option<&str>,
) -> PathBuf {
    if let Some(dir) = tmpdir.filter(|d| !d.as_os_str().is_empty() && !is_tmpfs(d)) {
        return dir.to_path_buf();
    }
    match target_tmp.filter(|t| !t.is_empty()) {
        Some(target) => PathBuf::from(target),
        None => PathBuf::from("/tmp"),
    }
}

/// Linux tmpfs magic number from `statfs(2)`.
#[cfg(target_os = "linux")]
const TMPFS_MAGIC: i64 = 0x0102_1994;

/// #2043 D5: whether `path` is on a tmpfs mount. Any failure (a missing
/// path, a NUL byte, a `statfs` error) answers false, which keeps today's
/// behaviour of using the path as given.
#[cfg(target_os = "linux")]
fn is_tmpfs(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c_path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    let mut buf: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c_path` is a valid NUL-terminated string that outlives the
    // call, and `buf` is a properly aligned, writable `statfs` struct.
    let rc = unsafe { libc::statfs(c_path.as_ptr(), &mut buf) };
    #[allow(clippy::unnecessary_cast)]
    let f_type = buf.f_type as i64;
    rc == 0 && f_type == TMPFS_MAGIC
}

/// Off Linux no path counts as tmpfs.
#[cfg(not(target_os = "linux"))]
fn is_tmpfs(_path: &Path) -> bool {
    false
}

/// #2043 D2/D3: check a boundary log written by the 1671 suite.
///
/// Each case writes `peak <case> <n>` then `after <case> <n>`. Returns the
/// rule violations (empty when the counts stay flat): `peak` below `after`,
/// or an `after` count above the first case's `after` count plus `slack`.
/// A malformed log is `Err`: an empty log, a line without exactly three
/// fields, an unknown kind, a count that is not a `usize`, or an `after`
/// without the same case's `peak` just before it.
fn boundary_violations(log: &str, slack: usize) -> Result<Vec<String>, String> {
    let mut violations = Vec::new();
    let mut pending: Option<(String, usize)> = None;
    let mut first_after: Option<usize> = None;
    let mut cases = 0usize;
    for (index, line) in log.lines().enumerate() {
        let n = index.saturating_add(1);
        let fields: Vec<&str> = line.split(' ').collect();
        let [kind, name, count] = fields.as_slice() else {
            return Err(format!(
                "line {n}: expected `<kind> <case> <count>`: {line:?}"
            ));
        };
        let count: usize = count
            .parse()
            .map_err(|e| format!("line {n}: count {count:?} is not a number: {e}"))?;
        match *kind {
            "peak" => {
                if let Some((prev, _)) = &pending {
                    return Err(format!(
                        "line {n}: `peak {name}` follows `peak {prev}` with no `after`"
                    ));
                }
                pending = Some((name.to_string(), count));
            }
            "after" => {
                let Some((peak_name, peak)) = pending.take() else {
                    return Err(format!("line {n}: `after {name}` has no `peak` before it"));
                };
                if peak_name != *name {
                    return Err(format!(
                        "line {n}: `after {name}` follows `peak {peak_name}`"
                    ));
                }
                cases = cases.saturating_add(1);
                if peak < count {
                    violations.push(format!("{name}: peak {peak} is below after {count}"));
                }
                match first_after {
                    None => first_after = Some(count),
                    Some(base) => {
                        let ceiling = base.saturating_add(slack);
                        if count > ceiling {
                            violations.push(format!(
                                "{name}: {count} entries after removal, above {base} + {slack} = {ceiling}"
                            ));
                        }
                    }
                }
            }
            other => return Err(format!("line {n}: unknown kind {other:?}")),
        }
    }
    if let Some((name, _)) = pending {
        return Err(format!("`peak {name}` has no `after`"));
    }
    if cases == 0 {
        return Err("the boundary log is empty".to_string());
    }
    Ok(violations)
}

/// Names under `root` that a 1671 run or this harness creates.
fn hook_repair_leftovers(root: &Path) -> BTreeSet<OsString> {
    fs::read_dir(root)
        .map(|it| {
            it.filter_map(Result::ok)
                .map(|e| e.file_name())
                .filter(|n| {
                    let n = n.to_string_lossy();
                    n.starts_with("issue-1671-hook-repair-")
                        || n.starts_with("issue-1937-hook-repair-")
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A bash command for the suite with all real configuration kept out.
fn suite_command(owned: &Path) -> Command {
    for name in ["tmp", "home", "cwd"] {
        fs::create_dir_all(owned.join(name)).expect("create owned fixture directory");
    }
    let mut cmd = Command::new("bash");
    cmd.arg(script_path())
        .current_dir(owned.join("cwd"))
        .env_clear()
        .env("PATH", std::env::var_os("PATH").expect("host tool PATH"))
        .env("HOME", owned.join("home"))
        .env("XDG_CONFIG_HOME", owned.join("home/.config"))
        .env("TMPDIR", owned.join("tmp"))
        .env("GIT_CONFIG_NOSYSTEM", "1");
    cmd
}

fn after_lines(log: &str) -> Vec<(String, usize)> {
    log.lines()
        .filter_map(|l| {
            let mut f = l.split(' ');
            match (f.next(), f.next(), f.next()) {
                (Some("after"), Some(name), Some(n)) => Some((name.to_string(), n.parse().ok()?)),
                _ => None,
            }
        })
        .collect()
}

#[test]
fn hook_rejection_repair_shell_contract_passes() {
    let script = script_path();
    assert!(script.is_file(), "missing {}", script.display());

    let root = fixture_root();

    // Repository selectors exported by an enclosing hook override even an
    // absolute `git init <fixture>` target. A failed init can leave an empty
    // .git outside TMPDIR (#1937). Both poisoned paths belong to this test.
    let fixture = tempfile::Builder::new()
        .prefix("issue-1937-hook-repair-")
        .tempdir_in(&root)
        .expect("create owned fixture parent");
    // #2043: the boundary log sits beside the fixture, never inside it, so
    // the #1937 "no new names" check below is unchanged.
    let boundary = tempfile::Builder::new()
        .prefix("issue-2043-boundary-")
        .tempdir_in(&root)
        .expect("create owned boundary-log directory");
    let log_path = boundary.path().join("boundary.log");
    for name in ["tmp", "home", "cwd"] {
        fs::create_dir(fixture.path().join(name)).expect("create owned fixture directory");
    }
    let common = fixture.path().join("common-file");
    fs::write(&common, b"owned common-directory blocker\n").expect("write owned blocker");
    let names = |path: &Path| -> BTreeSet<OsString> {
        fs::read_dir(path)
            .expect("list owned fixture")
            .map(|entry| entry.expect("fixture entry").file_name())
            .collect()
    };
    let before = names(fixture.path());
    let out = suite_command(fixture.path())
        // Deliberately inject owned repository selectors the harness must drop.
        .env("GIT_DIR", fixture.path().join(".git"))
        .env("GIT_COMMON_DIR", &common)
        .env("ISSUE_1671_BOUNDARY_LOG", &log_path)
        .output()
        .expect("run the #1671 shell contract");

    assert_eq!(
        names(fixture.path()),
        before,
        "#1937: the harness wrote beside TMPDIR (including an escaped .git)"
    );
    for name in ["tmp", "home", "cwd"] {
        assert!(
            names(&fixture.path().join(name)).is_empty(),
            "#1937: the harness left data in its caller's {name}"
        );
    }
    assert!(
        out.status.success(),
        "test-issue-1671-hook-rejection-repair.sh failed ({:?})\n--- stdout ---\n{}\n--- stderr ---\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    // #2043: one boundary per case, and no growth from case to case.
    let log = read(&log_path);
    let afters = after_lines(&log);
    assert_eq!(
        afters.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
        CASE_NAMES.to_vec(),
        "#2043: expected one `after` line per case, in run order\n{log}"
    );
    let violations = boundary_violations(&log, CASE_SLACK)
        .unwrap_or_else(|e| panic!("#2043: malformed boundary log: {e}\n{log}"));
    assert!(
        violations.is_empty(),
        "#2043: entries built up across case boundaries:\n{}\n--- log ---\n{log}",
        violations.join("\n")
    );

    let fixture_path = fixture.path().to_path_buf();
    let ours: BTreeSet<OsString> = [fixture.path(), boundary.path()]
        .iter()
        .filter_map(|p| p.file_name().map(OsString::from))
        .collect();
    fixture.close().expect("remove owned fixture");
    boundary.close().expect("remove owned boundary log");
    assert!(
        !fixture_path.exists(),
        "#2043: the fixture must be gone after close()"
    );
    // Other runs may share this root and hold their own live fixtures, so
    // only the names this test created are checked; the suite's own $WORK
    // lives inside the fixture's tmp, which the #1937 checks above cover.
    let left: Vec<OsString> = hook_repair_leftovers(&root)
        .intersection(&ours)
        .cloned()
        .collect();
    assert!(
        left.is_empty(),
        "#2043: this run left hook-repair entries in {}: {left:?}",
        root.display()
    );
}

/// #2043 D6: `ISSUE_1671_KEEP_WORK` turns off every new removal, so the
/// selected cases' directories stay; the test removes them itself.
#[test]
fn keep_work_leaves_case_directories() {
    let owned = tempfile::Builder::new()
        .prefix("issue-2043-keep-work-")
        .tempdir_in(fixture_root())
        .expect("create owned root");
    let log_path = owned.path().join("boundary.log");
    let out = suite_command(owned.path())
        .env("ISSUE_1671_KEEP_WORK", "1")
        .env("ISSUE_1671_ONLY", "mutation_canary,section_j")
        .env("ISSUE_1671_BOUNDARY_LOG", &log_path)
        .output()
        .expect("run the selected cases");
    assert!(
        out.status.success(),
        "selected run failed ({:?})\n--- stderr ---\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr),
    );

    let works: Vec<PathBuf> = fs::read_dir(owned.path().join("tmp"))
        .expect("list owned TMPDIR")
        .map(|e| e.expect("entry").path())
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("issue-1671-hook-repair-"))
        })
        .collect();
    assert_eq!(
        works.len(),
        1,
        "KEEP_WORK must keep exactly one $WORK: {works:?}"
    );
    let cases: BTreeSet<String> = fs::read_dir(&works[0])
        .expect("list $WORK")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("case-"))
        .collect();
    let want: BTreeSet<String> = ["case-mutation_canary", "case-section_j"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(
        cases, want,
        "only the selected cases run, and both are kept"
    );
    for case in &want {
        assert!(
            fs::read_dir(works[0].join(case))
                .expect("list case")
                .next()
                .is_some(),
            "{case} must not be empty under KEEP_WORK"
        );
    }
    assert_eq!(
        after_lines(&read(&log_path))
            .iter()
            .map(|(n, _)| n.as_str())
            .collect::<Vec<_>>(),
        vec!["mutation_canary", "section_j"],
        "the boundary log covers the selected cases only, in run order"
    );
    // `owned` is removed on drop, together with the kept $WORK.
}

/// #2043 security: bad selections and a relative log path exit 2 before any
/// case runs, and leave nothing behind.
#[test]
fn suite_rejects_bad_selection_and_relative_log() {
    for (var, value) in [
        ("ISSUE_1671_ONLY", "section_a;rm"),
        ("ISSUE_1671_ONLY", "../section_a"),
        ("ISSUE_1671_ONLY", "Section_A"),
        ("ISSUE_1671_ONLY", "no_such_case"),
        ("ISSUE_1671_BOUNDARY_LOG", "relative/boundary.log"),
    ] {
        let owned = tempfile::Builder::new()
            .prefix("issue-2043-reject-")
            .tempdir_in(fixture_root())
            .expect("create owned root");
        let out = suite_command(owned.path())
            .env(var, value)
            .output()
            .expect("run the suite");
        assert_eq!(
            out.status.code(),
            Some(2),
            "{var}={value:?} must exit 2\n--- stderr ---\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        for name in ["tmp", "home", "cwd"] {
            assert!(
                fs::read_dir(owned.path().join(name))
                    .expect("list")
                    .next()
                    .is_none(),
                "{var}={value:?} left data in {name}"
            );
        }
    }
}

// --- #2043 D5: fixture root choice (pure, `is_tmpfs` injected) ---------

#[test]
fn pick_fixture_root_uses_tmpdir_when_it_is_on_disk() {
    let root = pick_fixture_root(
        Some(Path::new("/disk/tmp")),
        &|_| false,
        Some("/target/tmp"),
    );
    assert_eq!(root, PathBuf::from("/disk/tmp"));
}

#[test]
fn pick_fixture_root_skips_tmpdir_on_tmpfs() {
    let root = pick_fixture_root(
        Some(Path::new("/tmp")),
        &|p| p == Path::new("/tmp"),
        Some("/target/tmp"),
    );
    assert_eq!(root, PathBuf::from("/target/tmp"));
}

#[test]
fn pick_fixture_root_uses_target_tmp_when_tmpdir_is_unset() {
    let root = pick_fixture_root(None, &|_| false, Some("/target/tmp"));
    assert_eq!(root, PathBuf::from("/target/tmp"));
}

#[test]
fn pick_fixture_root_falls_back_to_tmp_last() {
    assert_eq!(
        pick_fixture_root(None, &|_| false, None),
        PathBuf::from("/tmp")
    );
    assert_eq!(
        pick_fixture_root(Some(Path::new("/run/shm")), &|_| true, None),
        PathBuf::from("/tmp"),
        "TMPDIR on tmpfs and no target dir: the /tmp fallback"
    );
}

// --- #2043 D5: tmpfs detection ------------------------------------------

#[test]
fn is_tmpfs_is_false_for_missing_and_nul_paths() {
    assert!(!is_tmpfs(Path::new("/no/such/path/issue-2043")));
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let nul = Path::new(std::ffi::OsStr::from_bytes(b"/tm\0p"));
        assert!(!is_tmpfs(nul), "a path with a NUL byte is not tmpfs");
    }
}

#[cfg(target_os = "linux")]
#[test]
fn is_tmpfs_matches_the_mount_table_on_linux() {
    assert!(!is_tmpfs(Path::new("/proc")), "procfs is not tmpfs");
    // Use the host's own mount table, not an assumption about /tmp.
    let mounts = fs::read_to_string("/proc/self/mounts").unwrap_or_default();
    if let Some(point) = mounts
        .lines()
        .map(|l| l.split(' ').collect::<Vec<_>>())
        .find(|f| {
            f.len() > 2 && f[2] == "tmpfs" && !f[1].contains('\\') && Path::new(f[1]).is_dir()
        })
        .map(|f| PathBuf::from(f[1]))
    {
        assert!(is_tmpfs(&point), "{} is a tmpfs mount", point.display());
    }
}

#[cfg(not(target_os = "linux"))]
#[test]
fn is_tmpfs_is_false_off_linux() {
    assert!(!is_tmpfs(Path::new("/tmp")));
}

// --- #2043 D2/D3: boundary log rule (pure, strict parser) ---------------

#[test]
fn boundary_violations_passes_a_flat_series() {
    let log = "peak mutation_canary 900\nafter mutation_canary 40\n\
               peak section_a 3000\nafter section_a 60\n\
               peak section_b 2500\nafter section_b 104\n";
    assert_eq!(boundary_violations(log, 64), Ok(Vec::new()));
}

#[test]
fn boundary_violations_names_the_case_that_built_up() {
    let log = "peak mutation_canary 900\nafter mutation_canary 40\n\
               peak section_a 3000\nafter section_a 2900\n\
               peak section_b 5000\nafter section_b 4900\n";
    let v = boundary_violations(log, 64).expect("well-formed log");
    assert_eq!(v.len(), 2, "both growing cases are reported: {v:?}");
    assert!(
        v[0].contains("section_a") && v[1].contains("section_b"),
        "{v:?}"
    );
}

#[test]
fn boundary_violations_allows_exactly_the_slack() {
    let log = "peak mutation_canary 50\nafter mutation_canary 40\n\
               peak section_a 200\nafter section_a 104\n\
               peak section_b 200\nafter section_b 105\n";
    let v = boundary_violations(log, 64).expect("well-formed log");
    assert_eq!(v.len(), 1, "40 + 64 = 104 passes, 105 fails: {v:?}");
    assert!(v[0].contains("section_b"));
}

#[test]
fn boundary_violations_reports_peak_below_after() {
    let log = "peak mutation_canary 10\nafter mutation_canary 40\n";
    let v = boundary_violations(log, 64).expect("well-formed log");
    assert!(
        v.iter().any(|m| m.contains("mutation_canary")),
        "peak must be at least after: {v:?}"
    );
}

#[test]
fn boundary_violations_rejects_malformed_logs() {
    for bad in [
        "",
        "after mutation_canary\n",
        "after mutation_canary 40 extra\n",
        "peek mutation_canary 40\nafter mutation_canary 40\n",
        "peak mutation_canary forty\nafter mutation_canary 40\n",
        "peak mutation_canary -1\nafter mutation_canary 40\n",
        "after mutation_canary 40\n",
        "peak mutation_canary 50\nafter section_a 40\n",
    ] {
        assert!(
            boundary_violations(bad, 64).is_err(),
            "must be Err, never skipped: {bad:?}"
        );
    }
}

#[test]
fn case_slack_matches_the_shell_suite() {
    let text = read(&script_path());
    assert!(
        text.lines()
            .any(|l| l.trim() == format!("CASE_SLACK={CASE_SLACK}")),
        "the shell suite must define CASE_SLACK={CASE_SLACK}, the same value this harness checks"
    );
    for name in CASE_NAMES {
        assert!(
            text.contains(&format!("run_case {name} "))
                || text.contains(&format!("run_case \"{name}\"")),
            "the shell suite must run `{name}` through run_case"
        );
    }
}

/// #2043 D11: the two other suites that hold more than 3 repositories until
/// exit remove each case's directories, behind their own keep switch, and
/// `drop_case` refuses a path outside `$WORK`.
#[test]
fn other_bulk_suites_remove_each_case() {
    let tests = workspace_root().join("amplifier-bundle/recipes/tests");
    for (suite, keep) in [
        (
            "test-default-workflow-reliability.sh",
            "DEFAULT_WORKFLOW_RELIABILITY_KEEP_WORK",
        ),
        ("test-auto-drive-to-merge.sh", "AUTODRIVE_TEST_KEEP_WORK"),
    ] {
        let text = read(&tests.join(suite));
        assert!(
            text.contains("drop_case()"),
            "{suite} must define drop_case"
        );
        assert!(
            text.lines()
                .filter(|l| l.trim_start().starts_with("drop_case "))
                .count()
                >= 2,
            "{suite} must call drop_case after its cases"
        );
        assert!(text.contains(keep), "{suite} must honour {keep}");
        assert!(
            text.contains("${WORK:?}/"),
            "{suite}: drop_case must refuse paths outside ${{WORK:?}}/"
        );
        for line in text
            .lines()
            .filter(|l| !l.trim_start().starts_with('#') && l.contains("rm -rf"))
        {
            assert!(
                line.contains("-- "),
                "{suite}: every rm -rf needs `--`: {}",
                line.trim()
            );
        }
    }
}
