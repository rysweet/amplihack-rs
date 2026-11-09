//! Issue #1957: a caller can name the commit a task worktree starts from with
//! the recipe inputs `base_branch` and `base_sha`.
//!
//! Most checks here are static contract checks on the bundle files. The full
//! behaviour (the tool, both step bodies, cases a–h, the PR tools) is covered
//! by `amplifier-bundle/recipes/tests/test-issue-1957-explicit-worktree-source.sh`,
//! which CI runs directly. The env-budget rule is a unit test in
//! `src/commands/recipe/run/tests_execute.rs`.
//!
//! The behavioural cases at the end run the tool against temporary `file://`
//! repositories through `crate::isolated_amplihack::isolated`, with `HOME`
//! set to a temp directory, `GIT_CONFIG_GLOBAL=/dev/null` and
//! `GIT_CONFIG_NOSYSTEM=1` on every command. They never touch the network or
//! the real `~/.claude`, `~/.amplihack` or profile root, never sleep, and set
//! no timeouts.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root")
}

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The lines of the step whose `- id:` line contains `step_id`, up to the next step.
fn step_block(yaml: &str, step_id: &str) -> String {
    let mut out = Vec::new();
    let mut inside = false;
    for line in yaml.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("- id:") {
            if inside {
                break;
            }
            inside = trimmed.contains(step_id);
            continue;
        }
        if inside {
            out.push(line);
        }
    }
    assert!(!out.is_empty(), "step {step_id} not found");
    out.join("\n")
}

/// Lines of `block` that contain `printf '{` (the JSON contract lines).
fn json_printf_lines(block: &str) -> Vec<&str> {
    block.lines().filter(|l| l.contains("printf '{")).collect()
}

#[test]
fn both_workflows_declare_the_inputs_after_existing_branch() {
    for rel in [
        "amplifier-bundle/recipes/default-workflow.yaml",
        "amplifier-bundle/recipes/consensus-workflow.yaml",
    ] {
        let text = read(rel);
        let lines: Vec<&str> = text.lines().collect();
        let existing = lines
            .iter()
            .position(|l| *l == "  existing_branch: \"\"")
            .unwrap_or_else(|| panic!("{rel}: no top-level existing_branch input"));
        for key in ["base_branch", "base_sha"] {
            let decl = format!("  {key}: \"\"");
            let at = lines
                .iter()
                .position(|l| *l == decl)
                .unwrap_or_else(|| panic!("{rel}: missing input `{decl}`"));
            assert!(
                at > existing,
                "{rel}: `{key}` should be declared after existing_branch"
            );
        }
        assert!(
            text.contains("1957"),
            "{rel}: the input comment should reference issue #1957 and the precedence rule"
        );
    }
}

#[test]
fn orchestrators_forward_the_inputs_wherever_they_forward_existing_branch() {
    for rel in [
        "amplifier-bundle/recipes/smart-orchestrator.yaml",
        "amplifier-bundle/recipes/autodrive-build.yaml",
        "amplifier-bundle/recipes/auto-drive-to-merge.yaml",
    ] {
        let text = read(rel);
        let count = |needle: &str| text.lines().filter(|l| l.trim() == needle).count();
        let decl = count("existing_branch: \"\"");
        let fwd = count("existing_branch: \"{{existing_branch}}\"");
        assert!(decl + fwd > 0, "{rel}: expected existing_branch handling");
        assert_eq!(
            count("base_branch: \"\""),
            decl,
            "{rel}: declare base_branch wherever existing_branch is declared"
        );
        assert_eq!(
            count("base_sha: \"\""),
            decl,
            "{rel}: declare base_sha wherever existing_branch is declared"
        );
        assert_eq!(
            count("base_branch: \"{{base_branch}}\""),
            fwd,
            "{rel}: forward base_branch next to every existing_branch forward"
        );
        assert_eq!(
            count("base_sha: \"{{base_sha}}\""),
            fwd,
            "{rel}: forward base_sha next to every existing_branch forward"
        );
    }
}

#[test]
fn step04_json_carries_the_source_keys_on_both_paths() {
    let block = step_block(
        &read("amplifier-bundle/recipes/workflow-worktree.yaml"),
        "step-04-setup-worktree",
    );
    let printfs = json_printf_lines(&block);
    assert_eq!(
        printfs.len(),
        2,
        "step-04 has one JSON printf per path: {printfs:?}"
    );
    for line in &printfs {
        for key in [
            "\"base_ref\"",
            "\"base_branch\"",
            "\"base_sha\"",
            "\"source_verified\"",
            "\"source_rule\"",
        ] {
            assert!(
                line.contains(key),
                "step-04 JSON printf lacks {key}: {line}"
            );
        }
        assert!(
            !line.contains("origin_fingerprint"),
            "the origin fingerprint must never reach the JSON: {line}"
        );
    }
    let existing = printfs
        .iter()
        .find(|l| l.contains("\"created\": %s%s"))
        .expect("existing-branch printf");
    assert!(
        existing.contains("\"source_verified\": \"not-applicable\"")
            && existing.contains("\"source_rule\": \"existing-branch\"")
            && existing.contains("\"base_sha\": \"\""),
        "the existing-branch JSON reports not-applicable/existing-branch: {existing}"
    );
    assert!(
        block.contains("INFO: base_branch/base_sha ignored on the existing-branch path"),
        "step-04 says when the base inputs are ignored"
    );
}

#[test]
fn step04_lists_the_tool_in_step_tools() {
    let text = read("amplifier-bundle/recipes/workflow-worktree.yaml");
    let line = text
        .lines()
        .find(|l| l.trim_start().starts_with("STEP_TOOLS="))
        .expect("STEP_TOOLS line");
    assert!(
        line.contains("workflow_worktree_source.sh"),
        "STEP_TOOLS must list workflow_worktree_source.sh so a partial bundle is not preferred: {line}"
    );
}

#[test]
fn consensus_step3_json_and_documented_shape_carry_the_source_keys() {
    let block = step_block(
        &read("amplifier-bundle/recipes/consensus-issue-worktree.yaml"),
        "step3-setup-worktree",
    );
    let printfs = json_printf_lines(&block);
    assert_eq!(
        printfs.len(),
        2,
        "step3 has one JSON printf per path: {printfs:?}"
    );
    for line in &printfs {
        for key in [
            "\"base_ref\"",
            "\"base_branch\"",
            "\"base_sha\"",
            "\"source_verified\"",
            "\"source_rule\"",
        ] {
            assert!(line.contains(key), "step3 JSON printf lacks {key}: {line}");
        }
    }
    let shape = block
        .split("```json")
        .nth(1)
        .and_then(|rest| rest.split("```").next())
        .expect("documented ```json shape in the step3 prompt");
    for key in [
        "\"base_ref\"",
        "\"base_branch\"",
        "\"base_sha\"",
        "\"source_verified\"",
        "\"source_rule\"",
    ] {
        assert!(shape.contains(key), "documented step3 shape lacks {key}");
    }
    assert!(
        block.contains("jesc"),
        "step3 escapes the new JSON values with jesc"
    );
}

#[test]
fn both_steps_capture_the_inputs_before_anything_else() {
    for (rel, step) in [
        (
            "amplifier-bundle/recipes/workflow-worktree.yaml",
            "step-04-setup-worktree",
        ),
        (
            "amplifier-bundle/recipes/consensus-issue-worktree.yaml",
            "step3-setup-worktree",
        ),
    ] {
        let block = step_block(&read(rel), step);
        let branch = block
            .find("REQ_BASE_BRANCH=\"${BASE_BRANCH:-}\"")
            .unwrap_or_else(|| panic!("{rel}: no REQ_BASE_BRANCH capture"));
        let sha = block
            .find("REQ_BASE_SHA=\"${BASE_SHA:-}\"")
            .unwrap_or_else(|| panic!("{rel}: no REQ_BASE_SHA capture"));
        if let Some(local) = block.find("BASE_BRANCH=\"${BASE_REF") {
            assert!(
                branch < local && sha < local,
                "{rel}: the inputs must be captured before the local BASE_BRANCH is assigned"
            );
        }
        assert!(
            block.contains("${BASE_SHA:-$BASE_REF}"),
            "{rel}: the worktree is created from ${{BASE_SHA:-$BASE_REF}}"
        );
    }
}

#[test]
fn essential_context_keys_include_both_inputs() {
    let src = read("crates/amplihack-cli/src/commands/recipe/run/execute.rs");
    let start = src
        .find("fn is_essential_context_key")
        .expect("is_essential_context_key");
    let body = &src[start..start + src[start..].find("\n}\n").expect("end of fn")];
    for key in ["\"base_branch\"", "\"base_sha\""] {
        assert!(
            body.contains(key),
            "is_essential_context_key must keep {key} under the #1023 env budget"
        );
    }
}

#[test]
fn later_steps_ask_for_the_verified_base() {
    for tool in [
        "workflow_publish_pr.sh",
        "workflow_pr_ready.sh",
        "workflow_final_status.sh",
    ] {
        let text = read(&format!("amplifier-bundle/tools/{tool}"));
        assert!(
            text.contains("workflow_worktree_source.sh") && text.contains("verified-base"),
            "{tool} must use `workflow_worktree_source.sh verified-base` for the PR base"
        );
        assert!(
            text.contains("requested-inputs"),
            "{tool} must ask `requested-inputs` when verified-base exits 3 (D3)"
        );
        for line in text.lines().filter(|l| l.contains("verified-base")) {
            assert!(
                !line.contains("2>/dev/null") && !line.contains("|| true"),
                "{tool} must not mask verified-base (SR-2): {line}"
            );
        }
        assert!(
            text.contains(
                "base_branch/base_sha given but no verified base in worktree_setup; refusing to fall back"
            ),
            "{tool} must refuse with the D3 ERROR text"
        );
    }
}

#[test]
fn the_tool_and_its_doc_exist_and_the_tool_never_evals() {
    let tool = read("amplifier-bundle/tools/workflow_worktree_source.sh");
    read("amplifier-bundle/tools/workflow_worktree_source.md");
    for line in tool.lines().map(str::trim).filter(|l| !l.starts_with('#')) {
        let words: Vec<&str> = line
            .split(|c: char| c.is_whitespace() || c == ';' || c == '&' || c == '|')
            .collect();
        assert!(!words.contains(&"eval"), "the tool must never eval: {line}");
        assert!(
            !line.contains("set -x") && !line.contains("set -o xtrace"),
            "the tool must never trace (the origin URL could leak): {line}"
        );
    }
    for sub in ["resolve", "verify-worktree", "verified-base"] {
        assert!(tool.contains(sub), "the tool lacks the `{sub}` subcommand");
    }
}

#[test]
fn ci_runs_the_shell_test() {
    let ci = read(".github/workflows/ci.yml");
    assert!(
        ci.contains(
            "bash amplifier-bundle/recipes/tests/test-issue-1957-explicit-worktree-source.sh"
        ),
        "ci.yml must run the #1957 shell test"
    );
}

#[test]
fn agents_md_carries_no_1957_guidance() {
    let agents = read("AGENTS.md");
    assert!(
        !agents.contains("1957") && !agents.contains("base_sha"),
        "AGENTS.md must not carry #1957 guidance (#1333/#1336)"
    );
}

// ---------------------------------------------------------------------------
// Behaviour: the tool against temporary file:// repositories.
// ---------------------------------------------------------------------------

const TOOL: &str = "amplifier-bundle/tools/workflow_worktree_source.sh";

/// An isolated command with the hermetic git settings this test needs.
fn hermetic(bin: &str, home: &Path) -> Command {
    let mut c = crate::isolated_amplihack::isolated(bin);
    c.env("HOME", home)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_ALLOW_PROTOCOL", "file")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
        .env("LC_ALL", "C");
    c
}

fn git(home: &Path, args: &[&str]) -> String {
    let out = hermetic("git", home)
        .args(args)
        .output()
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A bare origin with `main` (one commit) and `dev` (two more), and a clone of
/// it whose origin is a `file://` URL. Returns (tempdir, home, bare, clone).
fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let tmp = tempfile::Builder::new()
        .prefix("i1957-rs-")
        .tempdir()
        .expect("tempdir");
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).expect("home");
    let bare = tmp.path().join("origin.git");
    let seed = tmp.path().join("seed");
    let clone = tmp.path().join("clone");
    let s = seed.to_str().unwrap();
    git(
        &home,
        &["init", "-q", "--bare", "-b", "main", bare.to_str().unwrap()],
    );
    git(&home, &["init", "-q", "-b", "main", s]);
    for (i, branch) in [(1, "main"), (2, "dev"), (3, "dev")] {
        if i == 2 {
            git(&home, &["-C", s, "checkout", "-q", "-b", branch]);
        }
        fs::write(seed.join(format!("f{i}.txt")), format!("{i}\n")).unwrap();
        git(&home, &["-C", s, "add", "."]);
        git(&home, &["-C", s, "commit", "-q", "-m", &format!("c{i}")]);
    }
    let url = format!("file://{}", bare.display());
    git(&home, &["-C", s, "push", "-q", &url, "main", "dev"]);
    git(&home, &["clone", "-q", &url, clone.to_str().unwrap()]);
    (tmp, home, bare, clone)
}

fn run_tool(home: &Path, cwd: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut c = hermetic("bash", home);
    c.current_dir(cwd).arg(repo_root().join(TOOL)).args(args);
    for (k, v) in env {
        c.env(k, v);
    }
    c.output().expect("spawn bash")
}

fn kv<'a>(stdout: &'a str, key: &str) -> Option<&'a str> {
    stdout
        .lines()
        .find_map(|l| l.strip_prefix(key).and_then(|r| r.strip_prefix('=')))
}

fn last_stderr_line(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr)
        .lines()
        .rfind(|l| !l.trim().is_empty())
        .unwrap_or_default()
        .to_string()
}

#[test]
fn resolve_verifies_a_fetched_branch_and_refuses_when_the_fetch_fails() {
    let (_tmp, home, bare, clone) = fixture();
    let repo = clone.to_str().unwrap();
    let dev_tip = git(
        &home,
        &["-C", bare.to_str().unwrap(), "rev-parse", "refs/heads/dev"],
    );

    let ok = run_tool(
        &home,
        &clone,
        &[
            "resolve",
            "--recipe",
            "default",
            "--repo",
            repo,
            "--base-branch",
            "dev",
        ],
        &[],
    );
    let stdout = String::from_utf8_lossy(&ok.stdout);
    assert!(
        ok.status.success(),
        "resolve --base-branch dev should succeed: {}",
        String::from_utf8_lossy(&ok.stderr)
    );
    assert_eq!(kv(&stdout, "source_verified"), Some("verified"), "{stdout}");
    assert_eq!(kv(&stdout, "base_ref"), Some("origin/dev"), "{stdout}");
    assert_eq!(kv(&stdout, "base_sha"), Some(dev_tip.as_str()), "{stdout}");
    assert!(
        !stdout.contains(bare.to_str().unwrap()),
        "the origin URL must never be printed: {stdout}"
    );

    // D1: point origin at a path that does not exist. The cached
    // refs/remotes/origin/dev still names the right commit, and must not be
    // trusted: every input combination is refused.
    let missing = format!("file://{}", bare.with_extension("missing").display());
    git(
        &home,
        &["-C", repo, "remote", "set-url", "origin", &missing],
    );
    for extra in [
        vec!["--base-branch", "dev"],
        vec!["--base-sha", dev_tip.as_str()],
        vec!["--base-branch", "dev", "--base-sha", dev_tip.as_str()],
    ] {
        let mut args = vec!["resolve", "--recipe", "default", "--repo", repo];
        args.extend(extra.iter().copied());
        let refused = run_tool(&home, &clone, &args, &[]);
        let stderr = String::from_utf8_lossy(&refused.stderr);
        assert_eq!(
            refused.status.code(),
            Some(1),
            "{extra:?} with a failing fetch must be refused: {stderr}"
        );
        assert!(
            refused.stdout.is_empty(),
            "{extra:?}: no result on stdout when refused"
        );
        assert!(
            stderr.lines().any(|l| l == "RULE: refuse-fetch-failed"),
            "{extra:?}: expected RULE: refuse-fetch-failed, got: {stderr}"
        );
        assert_eq!(
            stderr.lines().filter(|l| l.starts_with("ERROR: ")).count(),
            1,
            "{extra:?}: exactly one ERROR line: {stderr}"
        );
        assert!(
            last_stderr_line(&refused).starts_with("ERROR: "),
            "{extra:?}: the ERROR is the last stderr line: {stderr}"
        );
        assert!(
            !stderr.contains("verified-offline"),
            "{extra:?}: no offline rule exists any more: {stderr}"
        );
    }
}

#[test]
fn verified_base_and_requested_inputs_exit_codes() {
    let (_tmp, home, _bare, clone) = fixture();
    let ws = |json: &str| ("RECIPE_VAR_worktree_setup".to_string(), json.to_string());
    let vb = |env: Vec<(String, String)>| {
        let pairs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        run_tool(&home, &clone, &["verified-base"], &pairs)
    };

    let out = vb(vec![ws(
        r#"{"source_verified":"verified","base_branch":"dev"}"#,
    )]);
    assert_eq!(out.status.code(), Some(0), "verified + dev: exit 0");
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "dev");

    let out = vb(vec![ws(
        r#"{"source_verified":"verified","base_branch":"gone-1957"}"#,
    )]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "verified + a branch with no remote ref: exit 1, never a fallback"
    );
    assert!(out.stdout.is_empty());
    assert!(last_stderr_line(&out).starts_with("ERROR: "));

    for sv in ["legacy", "not-applicable", "verified-offline", ""] {
        let json = format!(r#"{{"source_verified":"{sv}","base_branch":"dev"}}"#);
        let out = vb(vec![ws(&json)]);
        assert_eq!(
            out.status.code(),
            Some(3),
            "source_verified '{sv}' means no verified base: exit 3"
        );
        assert!(out.stdout.is_empty(), "'{sv}': nothing on stdout");
    }
    let out = vb(vec![ws(
        r#"{"source_verified":"verified","base_branch":""}"#,
    )]);
    assert_eq!(
        out.status.code(),
        Some(3),
        "a SHA-only run has no base branch: exit 3"
    );
    let out = vb(vec![]);
    assert_eq!(out.status.code(), Some(3), "no context at all: exit 3");
    let out = run_tool(&home, &clone, &["verified-base", "extra"], &[]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "verified-base takes no arguments"
    );

    let ri = |env: &[(&str, &str)]| run_tool(&home, &clone, &["requested-inputs"], env);
    assert_eq!(ri(&[]).status.code(), Some(3), "no inputs: exit 3");
    assert_eq!(
        ri(&[("RECIPE_VAR_base_branch", ""), ("RECIPE_VAR_base_sha", "")])
            .status
            .code(),
        Some(3),
        "empty inputs: exit 3"
    );
    let given = ri(&[("RECIPE_VAR_base_branch", "dev")]);
    assert_eq!(given.status.code(), Some(0), "base_branch given: exit 0");
    assert!(given.stdout.is_empty(), "requested-inputs prints nothing");
    let bad_ctx = clone.parent().unwrap().join("ctx-bad.json");
    fs::write(&bad_ctx, "not json{").unwrap();
    let unreadable = ri(&[("AMPLIHACK_CONTEXT_FILE", bad_ctx.to_str().unwrap())]);
    assert_eq!(
        unreadable.status.code(),
        Some(1),
        "an unreadable context is a read failure, never 'not requested'"
    );
    assert_eq!(
        run_tool(&home, &clone, &["requested-inputs", "extra"], &[])
            .status
            .code(),
        Some(2),
        "requested-inputs takes no arguments"
    );
}

#[test]
fn the_origin_url_is_read_only_for_the_hash_or_the_existence_check() {
    let tool = read(TOOL);
    for line in tool
        .lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#') && l.contains("remote get-url"))
    {
        let hashed = line.contains("| git -C") && line.contains("hash-object --stdin");
        let existence = line.contains(">/dev/null 2>&1");
        assert!(
            hashed || existence,
            "`remote get-url` may only feed hash-object or an existence check: {line}"
        );
    }
}

#[test]
fn no_offline_source_rule_is_left_in_the_tool() {
    let tool = read(TOOL);
    for needle in [
        "verified-offline",
        "offline-cached-matches-base-sha",
        "offline-base-sha-present",
    ] {
        assert!(
            !tool.contains(needle),
            "D1 removed the offline rules; the tool still mentions `{needle}`"
        );
    }
    assert!(
        tool.contains("cmd_verified_base \"$@\"; exit $?"),
        "verified-base must pass its 0/1/3 status on"
    );
    assert!(
        tool.contains("requested-inputs)"),
        "the tool lacks the requested-inputs subcommand"
    );
}

#[test]
fn strict_fetch_is_one_attempt_and_the_legacy_loop_is_unchanged() {
    let fetcher = read("amplifier-bundle/tools/workflow_fetch_origin.sh");
    let loop_at = fetcher
        .find("for attempt in 1 2 3; do")
        .expect("the legacy 3-attempt loop");
    let strict_at = fetcher[..loop_at].rfind("\"$strict\" -eq 1").expect(
        "the --strict branch must come before the legacy loop, so strict mode never sleeps",
    );
    let strict_branch = &fetcher[strict_at..loop_at];
    assert!(
        !strict_branch.contains("sleep"),
        "strict mode must not sleep: {strict_branch}"
    );
    assert!(
        strict_branch.contains("WARNING: git fetch origin failed (exit $")
            && !strict_branch.contains("retrying"),
        "strict mode prints one WARNING with git's status and never retries: {strict_branch}"
    );
    let legacy = &fetcher[loop_at..];
    assert!(
        legacy.contains("sleep \"$delay\"") && legacy.contains("continuing with local refs"),
        "the legacy loop keeps its bytes (#1808 F1, prep-git-recovery)"
    );
}
