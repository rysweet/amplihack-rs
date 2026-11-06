//! Issue #1978: recipe prompts and skills must send test and scratch output
//! to `$TMPDIR`, never to a literal `/tmp/` path.
//!
//! On 5 to 6 Nov two auto-drive fixer steps filled the inodes on the host's
//! shared `/tmp`: one with mutation-test copies, one with
//! `pytest --basetemp /tmp/pr272/btN` (about 139,000 files a run). A later
//! smart-orchestrator run kept its scratch worktree at `/tmp/wt649impl.*`. The
//! runner already gives each step its own `TMPDIR`; a literal `/tmp/...` path
//! in an agent's command bypasses it.
//!
//! Contract: docs/reference/recipe-scratch-policy.md.
//!
//! Run: cargo test -p amplihack-cli --test cli_integration issue_1978_recipe_prompts_use_tmpdir::
//!
//! The guards:
//! - the fixed policy sentence is in the `description` of the two top-level
//!   recipes and in the prompt of every fixer, test and QA step listed in
//!   `POLICY_STEPS`;
//! - no line in `amplifier-bundle/recipes/*.yaml` or
//!   `amplifier-bundle/skills/**/*.md` tells an agent to write scratch output
//!   under a literal `/tmp/` path, except the reasoned `ALLOWLIST` entries.
//!
//! Steps deliberately not in `POLICY_STEPS`:
//! - `qa-workflow.yaml`: its three architect steps are question and answer
//!   only; they run no tests and make no scratch copies.
//! - `autodrive-crusty-round.yaml` `step-02-crusty-review` and
//!   `autodrive-merge-round.yaml` `step-02-merge-ready-assessment`: read-only
//!   reviewers.
//! - The bash steps in `autodrive-merge-evidence.yaml`: they have no prompt.
//!
//! The test is read-only. It reads files inside the repository, writes
//! nothing, sets no environment variables and creates no temporary folder.

use regex::Regex;
use serde_yaml::Value;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The policy sentence, verbatim. Every listed prompt and both top-level
/// descriptions must contain it.
const POLICY_SENTENCE: &str = "Scratch policy (#1978): put test --basetemp and tmp_path roots, \
mutation-test and other scratch copies, and any scratch worktree or clone under \"$TMPDIR\" \
(or this recipe's own worktree), never under a literal /tmp path.";

const POLICY_MARKER: &str = "Scratch policy (#1978)";

/// Top-level recipes whose `description` must state the policy.
const TOP_LEVEL: &[&str] = &[
    "recipes/auto-drive-to-merge.yaml",
    "recipes/default-workflow.yaml",
];

/// (recipe, step id) pairs whose agent prompt must carry the policy sentence.
const POLICY_STEPS: &[(&str, &str)] = &[
    (
        "recipes/autodrive-crusty-round.yaml",
        "step-04-address-concerns",
    ),
    (
        "recipes/autodrive-merge-round.yaml",
        "step-04-address-blockers",
    ),
    ("recipes/workflow-tdd.yaml", "step-07-write-tests"),
    ("recipes/workflow-tdd.yaml", "step-08-implement"),
    ("recipes/workflow-tdd.yaml", "step-08b-integration"),
    ("recipes/workflow-tdd.yaml", "step-08c-work-verifier"),
    (
        "recipes/workflow-precommit-test.yaml",
        "step-12-run-precommit",
    ),
    (
        "recipes/workflow-precommit-test.yaml",
        "step-13-local-testing",
    ),
    ("recipes/workflow-refactor-review.yaml", "step-09-refactor"),
    ("recipes/workflow-refactor-review.yaml", "step-09b-optimize"),
    (
        "recipes/workflow-refactor-review.yaml",
        "step-11b-implement-feedback",
    ),
    (
        "recipes/workflow-pr-review.yaml",
        "step-17e-address-blocking-issues",
    ),
    (
        "recipes/workflow-pr-review.yaml",
        "step-18b-implement-feedback",
    ),
    ("recipes/consensus-test-fix.yaml", "step7-fix-issues"),
    ("recipes/consensus-test-fix.yaml", "step8-local-testing"),
    ("recipes/smart-execute-routing.yaml", "handle-ops-agent"),
];

/// Scratch vocabulary: a `/tmp/` line is flagged only if it also contains one
/// of these (regex fragments). The list matches the reference page exactly;
/// `clone` covers `git clone`, and the redirect covers `>` and `>>`.
const SCRATCH_VOCAB: &[&str] = &[
    r"basetemp",
    r"tmp_path",
    r"pytest",
    r"mutat",
    r"scratch",
    r"cp -r",
    r"worktree",
    r"clone",
    r"mktemp",
    r"--shadow-log",
    r"mkdir",
    r">>?\s*/tmp/",
];

/// (path relative to amplifier-bundle/, line substring, reason).
///
/// Only container paths, cleanup of files the runtime writes, security
/// fixtures quoting `/tmp` as an attack example, and documented runtime
/// hardcodes qualify. Every entry must match at least one flagged line.
const ALLOWLIST: &[(&str, &str, &str)] = &[
    (
        "skills/eval-recipes-runner/SKILL.md",
        "git /tmp/amplihack",
        "Dockerfile RUN step: the clone path is inside the benchmark container",
    ),
    (
        "skills/shadow-testing/SKILL.md",
        "my-lib /tmp/test",
        "runs under docker exec: the clone path is inside the shadow container",
    ),
    (
        "skills/code-atlas/tests/test_security_controls.md",
        "\" > /tmp/test.env",
        "secret-redaction fixture quoting /tmp as the attack example",
    ),
    (
        "skills/code-atlas/tests/test_security_controls.md",
        "\" >> /tmp/test.env",
        "secret-redaction fixture quoting /tmp as the attack example",
    ),
];

const MIN_ALLOW_SUBSTRING: usize = 12;

// -------------------------------------------------------------------------
// Helpers
// -------------------------------------------------------------------------

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root is two levels above the crate manifest")
        .to_path_buf()
}

fn bundle_root() -> PathBuf {
    repo_root().join("amplifier-bundle")
}

fn read_lossy(path: &Path) -> String {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    String::from_utf8_lossy(&bytes).into_owned()
}

fn bundle_file(rel: &str) -> String {
    read_lossy(&bundle_root().join(rel))
}

fn parse_recipe(rel: &str) -> Value {
    serde_yaml::from_str(&bundle_file(rel))
        .unwrap_or_else(|e| panic!("amplifier-bundle/{rel}: does not parse as YAML: {e}"))
}

/// The prompt of an agent step, or a description of why there is none.
fn step_prompt(recipe_rel: &str, step_id: &str) -> Result<String, String> {
    let recipe = parse_recipe(recipe_rel);
    let steps = recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .ok_or_else(|| format!("amplifier-bundle/{recipe_rel}: has no steps sequence"))?;
    let step = steps
        .iter()
        .find(|s| s.get("id").and_then(Value::as_str) == Some(step_id))
        .ok_or_else(|| {
            format!(
                "amplifier-bundle/{recipe_rel}: step `{step_id}` not found (renamed or removed?)"
            )
        })?;
    step.get("prompt")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            format!("amplifier-bundle/{recipe_rel}: step `{step_id}` has no agent prompt")
        })
}

fn vocab_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(&SCRATCH_VOCAB.join("|")).expect("SCRATCH_VOCAB compiles"))
}

/// A whole-line YAML comment. Markdown lines are never comments.
fn is_comment(line: &str, is_yaml: bool) -> bool {
    is_yaml && line.trim_start().starts_with('#')
}

/// (1-based line number, line) for every line that directs scratch output to
/// a literal `/tmp/` path.
fn flagged_lines(text: &str, is_yaml: bool) -> Vec<(usize, String)> {
    text.lines()
        .enumerate()
        .filter(|(_, l)| !is_comment(l, is_yaml))
        .filter(|(_, l)| l.contains("/tmp/"))
        .filter(|(_, l)| vocab_re().is_match(l))
        .map(|(i, l)| (i + 1, l.to_string()))
        .collect()
}

fn walk_md(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries =
        std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()));
    for entry in entries {
        let entry = entry.unwrap_or_else(|e| panic!("read_dir entry in {}: {e}", dir.display()));
        let path = entry.path();
        let meta = std::fs::symlink_metadata(&path)
            .unwrap_or_else(|e| panic!("symlink_metadata {}: {e}", path.display()));
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            let name = entry.file_name();
            if name == "docker-compose" || name == "templates" {
                continue;
            }
            walk_md(&path, out);
        } else if meta.is_file() && path.extension().is_some_and(|e| e == "md") {
            out.push(path);
        }
    }
}

/// Every file in the scan scope, sorted, as paths relative to amplifier-bundle/.
fn scan_files() -> Vec<String> {
    let root = bundle_root();
    let mut files = Vec::new();

    let recipes = root.join("recipes");
    for entry in std::fs::read_dir(&recipes)
        .unwrap_or_else(|e| panic!("read_dir {}: {e}", recipes.display()))
    {
        let path = entry.expect("recipes entry").path();
        let meta = std::fs::symlink_metadata(&path)
            .unwrap_or_else(|e| panic!("symlink_metadata {}: {e}", path.display()));
        if meta.is_file() && path.extension().is_some_and(|e| e == "yaml") {
            files.push(path);
        }
    }

    let skills = root.join("skills");
    let mut md = Vec::new();
    walk_md(&skills, &mut md);

    let mut rel: Vec<String> = files
        .iter()
        .map(|p| {
            p.strip_prefix(&root)
                .expect("recipe under bundle root")
                .to_string_lossy()
                .into_owned()
        })
        .chain(md.iter().map(|p| {
            format!(
                "skills/{}",
                p.strip_prefix(&skills)
                    .expect("skill under skills root")
                    .to_string_lossy()
            )
        }))
        .collect();
    rel.sort();
    rel
}

fn check_policy_text(label: &str, text: &str, failures: &mut Vec<String>) {
    if !text.contains(POLICY_MARKER) {
        failures.push(format!(
            "{label}: missing the policy marker `{POLICY_MARKER}`"
        ));
        return;
    }
    if !text.contains(POLICY_SENTENCE) {
        failures.push(format!(
            "{label}: policy sentence is not verbatim; expected: {POLICY_SENTENCE}"
        ));
    }
}

// -------------------------------------------------------------------------
// Positive guards: the policy is stated where agents read it
// -------------------------------------------------------------------------

#[test]
fn top_level_recipes_state_policy() {
    let mut failures = Vec::new();
    for rel in TOP_LEVEL {
        let recipe = parse_recipe(rel);
        match recipe.get("description").and_then(Value::as_str) {
            Some(desc) => check_policy_text(
                &format!("amplifier-bundle/{rel} description"),
                desc,
                &mut failures,
            ),
            None => failures.push(format!("amplifier-bundle/{rel}: has no description")),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn default_workflow_description_keeps_its_first_line() {
    let recipe = parse_recipe("recipes/default-workflow.yaml");
    let desc = recipe
        .get("description")
        .and_then(Value::as_str)
        .expect("default-workflow.yaml has a description");
    assert_eq!(
        desc.lines().next(),
        Some("Composable 23-step development workflow — orchestrates 9 phase sub-recipes"),
        "the first line of the default-workflow description must stay byte-identical"
    );
}

#[test]
fn fixer_test_qa_steps_carry_policy() {
    let mut failures = Vec::new();
    for (rel, id) in POLICY_STEPS {
        match step_prompt(rel, id) {
            Ok(prompt) => check_policy_text(
                &format!("amplifier-bundle/{rel} step `{id}`"),
                &prompt,
                &mut failures,
            ),
            Err(e) => failures.push(e),
        }
    }
    assert!(
        failures.is_empty(),
        "{} policy failure(s):\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn policy_steps_table_has_no_duplicates() {
    let mut seen = std::collections::HashSet::new();
    for pair in POLICY_STEPS {
        assert!(seen.insert(pair), "duplicate POLICY_STEPS entry: {pair:?}");
    }
    assert_eq!(POLICY_STEPS.len(), 16, "issue #1978 lists 16 policy steps");
}

// -------------------------------------------------------------------------
// Negative guard: no literal /tmp/ scratch targets
// -------------------------------------------------------------------------

#[test]
fn no_prompt_or_skill_directs_scratch_to_literal_tmp() {
    let mut problems = Vec::new();

    for (path, sub, reason) in ALLOWLIST {
        if reason.trim().is_empty() {
            problems.push(format!(
                "allowlist entry {path} `{sub}` has an empty reason"
            ));
        }
        if sub.len() < MIN_ALLOW_SUBSTRING {
            problems.push(format!(
                "allowlist entry {path} `{sub}` is too broad (needs >= {MIN_ALLOW_SUBSTRING} chars)"
            ));
        }
    }

    let files = scan_files();
    assert!(
        files.iter().any(|f| f.starts_with("recipes/")),
        "scan set has no recipes; is amplifier-bundle/recipes present?"
    );
    assert!(
        files.iter().any(|f| f.starts_with("skills/")),
        "scan set has no skills; is amplifier-bundle/skills present?"
    );

    let mut used = vec![false; ALLOWLIST.len()];
    for rel in &files {
        let text = bundle_file(rel);
        let is_yaml = rel.ends_with(".yaml");
        for (n, line) in flagged_lines(&text, is_yaml) {
            let allowed = ALLOWLIST
                .iter()
                .enumerate()
                .filter(|(_, (p, s, _))| p == rel && line.contains(s))
                .map(|(i, _)| i)
                .collect::<Vec<_>>();
            if allowed.is_empty() {
                problems.push(format!(
                    "amplifier-bundle/{rel}:{n}: directs scratch output to a literal /tmp path: {}",
                    line.trim()
                ));
            }
            for i in allowed {
                used[i] = true;
            }
        }
    }

    for (i, (path, sub, _)) in ALLOWLIST.iter().enumerate() {
        if !used[i] {
            problems.push(format!(
                "stale allowlist entry: amplifier-bundle/{path} `{sub}` matches no flagged line"
            ));
        }
    }

    assert!(
        problems.is_empty(),
        "{} problem(s); use \"${{TMPDIR:-/tmp}}/...\" (see docs/reference/recipe-scratch-policy.md):\n{}",
        problems.len(),
        problems.join("\n")
    );
}

#[test]
fn policy_sentence_does_not_trip_scanner() {
    assert!(
        !POLICY_SENTENCE.contains("/tmp/"),
        "the policy sentence must not contain `/tmp/`"
    );
    assert!(flagged_lines(POLICY_SENTENCE, true).is_empty());
    assert!(flagged_lines(POLICY_SENTENCE, false).is_empty());
    assert!(POLICY_SENTENCE.starts_with(POLICY_MARKER));
}

// -------------------------------------------------------------------------
// Detector behaviour (unit level, in memory)
// -------------------------------------------------------------------------

#[test]
fn detector_flags_incident_forms() {
    let bad = [
        "pytest --basetemp /tmp/pr272/bt1",
        "git worktree add /tmp/wt649impl -b fix",
        "cargo mutants --in-place > /tmp/mutants.log",
        "cargo test >> /tmp/out.log",
        "cp -r src /tmp/mutant-copy",
        "git clone https://example.invalid/r.git /tmp/clone",
        "d=$(mktemp -d /tmp/wt.XXXXXX)",
        "mkdir -p /tmp/scratch/run1",
        "amplihack qa --shadow-log /tmp/shadow.jsonl",
        "use tmp_path rooted at /tmp/pytest-of-me",
    ];
    for line in bad {
        assert_eq!(
            flagged_lines(line, true).len(),
            1,
            "should flag (yaml): {line}"
        );
        assert_eq!(
            flagged_lines(line, false).len(),
            1,
            "should flag (md): {line}"
        );
    }
}

#[test]
fn detector_passes_safe_forms() {
    let ok = [
        r#"pytest --basetemp "${TMPDIR:-/tmp}/bt1""#,
        r#"wt=$(mktemp -d "${TMPDIR:-/tmp}/wt-issue.XXXXXX")"#,
        r#"rm -rf "${TMPDIR:-/tmp}/amplihack-workstreams/"ws-*"#,
        "The runtime keeps its socket in /tmp/amplihack.sock.",
        "ls -t /tmp/*.agent*.log",
        "rm -f /tmp/smart-orch-ws-*.json",
        "",
    ];
    for line in ok {
        assert!(
            flagged_lines(line, true).is_empty(),
            "should not flag: {line}"
        );
    }
}

#[test]
fn yaml_comments_are_skipped_but_markdown_hash_lines_are_not() {
    let line = "  # old: pytest --basetemp /tmp/x";
    assert!(
        flagged_lines(line, true).is_empty(),
        "YAML comment must be skipped"
    );
    assert_eq!(
        flagged_lines(line, false).len(),
        1,
        "a `#` line in markdown (e.g. in a fenced shell block) is still scanned"
    );
}

#[test]
fn flagged_lines_reports_one_based_line_numbers() {
    let text = "first\nsecond\npytest --basetemp /tmp/x\n";
    assert_eq!(
        flagged_lines(text, true),
        vec![(3, "pytest --basetemp /tmp/x".to_string())]
    );
}

/// AC4 in memory: the edits that the manual check makes to
/// `autodrive-crusty-round.yaml` `step-04-address-concerns` are caught.
#[test]
fn bad_edits_to_a_policy_prompt_are_caught() {
    let prompt = step_prompt(
        "recipes/autodrive-crusty-round.yaml",
        "step-04-address-concerns",
    )
    .unwrap_or_else(|e| panic!("{e}"));

    for bad in ["pytest --basetemp /tmp/x", "git worktree add /tmp/wt"] {
        let edited = format!("{prompt}\n{bad}\n");
        assert!(
            !flagged_lines(&edited, true).is_empty(),
            "inserting `{bad}` must be flagged"
        );
    }

    let removed = prompt.replace(POLICY_SENTENCE, "");
    let mut failures = Vec::new();
    check_policy_text("edited prompt", &removed, &mut failures);
    assert!(
        !failures.is_empty(),
        "deleting the policy sentence must fail the positive check"
    );
}

#[test]
fn scan_scope_skips_symlinks_and_excluded_folders() {
    let files = scan_files();
    let mut dedup = files.clone();
    dedup.dedup();
    assert_eq!(files.len(), dedup.len(), "scan set has duplicate paths");
    for f in &files {
        assert!(
            !f.contains("/docker-compose/") && !f.contains("/templates/"),
            "excluded folder scanned: {f}"
        );
        assert!(
            f.starts_with("recipes/") || f.starts_with("skills/"),
            "out of scope: {f}"
        );
        let meta = std::fs::symlink_metadata(bundle_root().join(f))
            .unwrap_or_else(|e| panic!("symlink_metadata {f}: {e}"));
        assert!(!meta.file_type().is_symlink(), "symlink scanned: {f}");
    }
    assert!(
        !files
            .iter()
            .any(|f| f.starts_with("skills/outside-in-testing/README.md")),
        "symlinked skill file was scanned"
    );
}

// -------------------------------------------------------------------------
// Targeted guards for rewrites the scanner rule does not cover
// -------------------------------------------------------------------------

/// The workflow runtime root falls back through `$TMPDIR` before `/tmp`, and
/// the root is created mode 0700 before its subfolders. The old form
/// `${XDG_RUNTIME_DIR:-/tmp}/...` contains no `/tmp/`, so the scanner alone
/// cannot catch it.
#[test]
fn workflow_runtime_root_falls_back_through_tmpdir() {
    let text = bundle_file("recipes/workflow-worktree.yaml");
    let lines: Vec<&str> = text.lines().collect();
    let idx = lines
        .iter()
        .position(|l| l.contains("AMPLIHACK_RUNTIME_ROOT=\"${AMPLIHACK_RUNTIME_ROOT:-"))
        .expect(
            "amplifier-bundle/recipes/workflow-worktree.yaml: runtime-root assignment not found",
        );
    let assign = lines[idx];
    assert!(
        assign.contains("${XDG_RUNTIME_DIR:-${TMPDIR:-/tmp}}/amplihack-runtime/"),
        "amplifier-bundle/recipes/workflow-worktree.yaml:{}: runtime-root fallback must be \
         ${{XDG_RUNTIME_DIR:-${{TMPDIR:-/tmp}}}}/amplihack-runtime/...: {}",
        idx + 1,
        assign.trim()
    );

    // The root itself is created 0700 before any subfolder.
    let sub = lines
        .iter()
        .enumerate()
        .skip(idx)
        .find(|(_, l)| l.contains("\"$AMPLIHACK_RUNTIME_ROOT\"/{"))
        .map(|(i, _)| i)
        .expect("runtime-root subfolder mkdir not found");
    let secured = lines[idx..sub].iter().any(|l| {
        (l.contains("mkdir") && l.contains("-m 700") && l.contains("\"$AMPLIHACK_RUNTIME_ROOT\""))
            || (l.contains("chmod 700") && l.contains("\"$AMPLIHACK_RUNTIME_ROOT\""))
    });
    assert!(
        secured,
        "amplifier-bundle/recipes/workflow-worktree.yaml:{}-{}: create \"$AMPLIHACK_RUNTIME_ROOT\" \
         with mode 0700 (mkdir -p -m 700 or chmod 700) before its subfolders",
        idx + 1,
        sub + 1
    );
}

/// The multitask skill's cleanup lines remove the folder the runtime uses,
/// `std::env::temp_dir()/amplihack-workstreams`, not a literal /tmp path.
/// `rm` is not in SCRATCH_VOCAB, so the scanner does not see these lines.
#[test]
fn multitask_cleanup_targets_tmpdir_workstreams() {
    let mut problems = Vec::new();
    for rel in [
        "skills/multitask/SKILL.md",
        "skills/multitask/examples.md",
        "skills/multitask/reference.md",
        "skills/multitask/TIMEOUT_LIFECYCLE.md",
    ] {
        for (i, line) in bundle_file(rel).lines().enumerate() {
            if line.contains("rm -rf") && line.contains("/tmp/amplihack-workstreams") {
                problems.push(format!("amplifier-bundle/{rel}:{}: {}", i + 1, line.trim()));
            }
        }
    }
    assert!(
        problems.is_empty(),
        "multitask cleanup must use rm -rf \"${{TMPDIR:-/tmp}}/amplihack-workstreams/\"...:\n{}",
        problems.join("\n")
    );
    let skill = bundle_file("skills/multitask/SKILL.md");
    assert!(
        skill.contains("rm -rf \"${TMPDIR:-/tmp}/amplihack-workstreams/"),
        "amplifier-bundle/skills/multitask/SKILL.md: expected a quoted \
         rm -rf \"${{TMPDIR:-/tmp}}/amplihack-workstreams/...\" cleanup line"
    );
}
