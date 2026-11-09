//! Issue #1978: recipe fixer, test and QA prompts send scratch output to
//! `$TMPDIR` (else `$AMPLIHACK_RUNTIME_ROOT`, else the recipe's own worktree),
//! never to a literal `/tmp` path, and remove each case when it finishes.
//!
//! On 5 to 6 Nov two auto-drive fixer steps filled the inodes on the host's
//! shared `/tmp`: one with mutation-test copies, one with
//! `pytest --basetemp /tmp/pr272/btN`. The runner gives each step its own
//! `TMPDIR`; a literal `/tmp/...` path in an agent's command bypasses it.
//!
//! Ported from PR #1994 and narrowed (#2043 D9): only
//! `amplifier-bundle/recipes/*.yaml` is scanned; skill markdown is out of
//! scope.
//!
//! Contract: docs/reference/recipe-scratch-policy.md.
//!
//! Run: cargo test -p amplihack-cli --test cli_integration issue_1978
//!
//! The guards only read files in the repository; the detector is proved on
//! text held in memory, so no test edits a repository file. The one
//! behavioural test of `workflow_runtime_root.sh` runs it with a cleared
//! environment inside its own `tempfile` directory (under the caller's
//! `TMPDIR`), which is removed when the test ends.

use regex::Regex;
use serde_yaml::Value;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The policy sentence, verbatim (docs/reference/recipe-scratch-policy.md).
const POLICY_SENTENCE: &str = "Scratch policy (#1978): put test --basetemp and tmp_path roots, \
mutation-test and other scratch copies, and any scratch worktree or clone under \"$TMPDIR\", \
else \"$AMPLIHACK_RUNTIME_ROOT\", else this recipe's own worktree, never under a literal /tmp \
path; remove each one when its case finishes.";

const POLICY_MARKER: &str = "Scratch policy (#1978)";

/// The first line of default-workflow's description is pinned elsewhere and
/// must stay byte-identical; the policy sentence goes after it.
const DEFAULT_WORKFLOW_FIRST_LINE: &str =
    "Composable 23-step development workflow — orchestrates 9 phase sub-recipes";

/// Top-level recipes whose `description` must state the policy.
const TOP_LEVEL: &[&str] = &["auto-drive-to-merge.yaml", "default-workflow.yaml"];

/// (recipe, step id) pairs whose agent prompt must carry the policy sentence.
/// Every id was checked against `main` at 7b8d32e7e; none was renamed or
/// removed since #1994.
const POLICY_STEPS: &[(&str, &str)] = &[
    ("autodrive-crusty-round.yaml", "step-04-address-concerns"),
    ("autodrive-merge-round.yaml", "step-04-address-blockers"),
    ("workflow-tdd.yaml", "step-07-write-tests"),
    ("workflow-tdd.yaml", "step-08-implement"),
    ("workflow-tdd.yaml", "step-08b-integration"),
    ("workflow-tdd.yaml", "step-08c-work-verifier"),
    ("workflow-precommit-test.yaml", "step-12-run-precommit"),
    ("workflow-precommit-test.yaml", "step-13-local-testing"),
    ("workflow-refactor-review.yaml", "step-09-refactor"),
    ("workflow-refactor-review.yaml", "step-09b-optimize"),
    (
        "workflow-refactor-review.yaml",
        "step-11b-implement-feedback",
    ),
    (
        "workflow-pr-review.yaml",
        "step-17e-address-blocking-issues",
    ),
    ("workflow-pr-review.yaml", "step-18b-implement-feedback"),
    ("consensus-test-fix.yaml", "step7-fix-issues"),
    ("consensus-test-fix.yaml", "step8-local-testing"),
    ("smart-execute-routing.yaml", "handle-ops-agent"),
];

/// (recipe file, line substring, reason). Every literal `/tmp` path left in a
/// scanned recipe needs an entry; an entry that matches nothing fails.
const ALLOWLIST: &[(&str, &str, &str)] = &[(
    "quality-audit-cycle.yaml",
    "${OUTPUT_DIR:-/tmp}/cycle_",
    "not on the fixer/test/QA path; output directory set by the caller",
)];

const MIN_ALLOW_SUBSTRING: usize = 12;

const BRICK_LINE_BUDGET: usize = 400;

// -------------------------------------------------------------------------
// Helpers
// -------------------------------------------------------------------------

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|p| p.join("amplifier-bundle/recipes").is_dir())
        .expect("workspace must contain amplifier-bundle/recipes")
        .to_path_buf()
}

fn recipes_dir() -> PathBuf {
    repo_root().join("amplifier-bundle/recipes")
}

fn read_lossy(path: &Path) -> String {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    String::from_utf8_lossy(&bytes).into_owned()
}

fn recipe_text(name: &str) -> String {
    read_lossy(&recipes_dir().join(name))
}

fn parse_recipe(name: &str) -> Value {
    serde_yaml::from_str(&recipe_text(name))
        .unwrap_or_else(|e| panic!("recipes/{name}: does not parse as YAML: {e}"))
}

/// The prompt of an agent step, or why there is none.
fn step_prompt(name: &str, step_id: &str) -> Result<String, String> {
    let recipe = parse_recipe(name);
    let steps = recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .ok_or_else(|| format!("recipes/{name}: has no steps sequence"))?;
    let step = steps
        .iter()
        .find(|s| s.get("id").and_then(Value::as_str) == Some(step_id))
        .ok_or_else(|| {
            format!("recipes/{name}: step `{step_id}` not found (renamed or removed?)")
        })?;
    step.get("prompt")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("recipes/{name}: step `{step_id}` has no agent prompt"))
}

/// The safe shell defaults: `/tmp` is reached only when the caller set no
/// `TMPDIR`.
fn tmpdir_default_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\$\{TMPDIR(?::-|-|:=)/tmp\}").expect("default regex"))
}

/// A literal `/tmp` path: `/tmp` followed by `/`, `}`, a quote, a glob or the
/// end of the line. Prose such as "a literal /tmp path" is not a path.
fn literal_tmp_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"/tmp(?:[/}"'*]|$)"#).expect("literal regex"))
}

/// A whole-line YAML comment.
fn is_comment(line: &str) -> bool {
    line.trim_start().starts_with('#')
}

/// (1-based line number, line) for every non-comment line that names a
/// literal `/tmp` path once the `${TMPDIR:-/tmp}` defaults are removed.
fn flagged_lines(text: &str) -> Vec<(usize, String)> {
    text.lines()
        .enumerate()
        .filter(|(_, l)| !is_comment(l))
        .filter(|(_, l)| {
            let stripped = tmpdir_default_re().replace_all(l, "");
            literal_tmp_re().is_match(&stripped)
        })
        .map(|(i, l)| (i + 1, l.to_string()))
        .collect()
}

/// Every `*.yaml` regular file directly in amplifier-bundle/recipes, sorted.
fn scan_files() -> Vec<String> {
    let dir = recipes_dir();
    let mut out: Vec<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()))
        .map(|e| e.expect("recipes entry").path())
        .filter(|p| {
            std::fs::symlink_metadata(p)
                .map(|m| m.is_file())
                .unwrap_or(false)
                && p.extension().is_some_and(|e| e == "yaml")
        })
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    out.sort();
    out
}

fn check_policy_text(label: &str, text: &str, failures: &mut Vec<String>) {
    if !text.contains(POLICY_MARKER) {
        failures.push(format!("{label}: missing {POLICY_MARKER}"));
        return;
    }
    if !text.contains(POLICY_SENTENCE) {
        failures.push(format!(
            "{label}: policy sentence is not verbatim; expected: {POLICY_SENTENCE}"
        ));
    }
}

/// Problems with an allowlist against a set of (file, text) inputs.
fn allowlist_problems(files: &[(String, String)], allow: &[(&str, &str, &str)]) -> Vec<String> {
    let mut problems = Vec::new();
    for (path, sub, reason) in allow {
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
    let mut used = vec![false; allow.len()];
    for (rel, text) in files {
        for (n, line) in flagged_lines(text) {
            let hits: Vec<usize> = allow
                .iter()
                .enumerate()
                .filter(|(_, (p, s, _))| p == rel && line.contains(s))
                .map(|(i, _)| i)
                .collect();
            if hits.is_empty() {
                problems.push(format!(
                    "recipes/{rel}:{n}: literal /tmp scratch path: {}",
                    line.trim()
                ));
            }
            for i in hits {
                used[i] = true;
            }
        }
    }
    for (i, (path, sub, _)) in allow.iter().enumerate() {
        if !used[i] {
            problems.push(format!(
                "allowlist entry matches nothing: recipes/{path} `{sub}`"
            ));
        }
    }
    problems
}

// -------------------------------------------------------------------------
// Positive guards: the policy is stated where agents read it
// -------------------------------------------------------------------------

#[test]
fn issue_1978_top_level_recipes_state_policy() {
    let mut failures = Vec::new();
    for name in TOP_LEVEL {
        match parse_recipe(name)
            .get("description")
            .and_then(Value::as_str)
        {
            Some(desc) => {
                check_policy_text(&format!("recipes/{name} description"), desc, &mut failures)
            }
            None => failures.push(format!("recipes/{name}: has no description")),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn issue_1978_default_workflow_description_keeps_its_first_line() {
    let recipe = parse_recipe("default-workflow.yaml");
    let desc = recipe
        .get("description")
        .and_then(Value::as_str)
        .expect("default-workflow.yaml has a description");
    assert_eq!(
        desc.lines().next(),
        Some(DEFAULT_WORKFLOW_FIRST_LINE),
        "the first line of the default-workflow description must stay byte-identical"
    );
}

#[test]
fn issue_1978_fixer_test_qa_steps_carry_policy() {
    let mut failures = Vec::new();
    for (name, id) in POLICY_STEPS {
        match step_prompt(name, id) {
            Ok(prompt) => check_policy_text(
                &format!("recipes/{name} step `{id}`"),
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
fn issue_1978_policy_steps_table_has_no_duplicates() {
    let mut seen = HashSet::new();
    for pair in POLICY_STEPS {
        assert!(seen.insert(pair), "duplicate POLICY_STEPS entry: {pair:?}");
    }
}

#[test]
fn issue_1978_policy_sentence_is_safe_to_embed() {
    assert!(POLICY_SENTENCE.starts_with(POLICY_MARKER));
    assert!(!POLICY_SENTENCE.contains("/tmp/"), "no literal /tmp/ path");
    assert!(!POLICY_SENTENCE.contains("{{"), "no template variables");
    assert!(
        flagged_lines(POLICY_SENTENCE).is_empty(),
        "the policy sentence must not trip its own detector"
    );
    for word in [
        "$TMPDIR",
        "$AMPLIHACK_RUNTIME_ROOT",
        "own worktree",
        "remove each",
    ] {
        assert!(
            POLICY_SENTENCE.contains(word),
            "sentence must name `{word}`"
        );
    }
}

/// D10: the sentence must not push a recipe past the brick budget.
#[test]
fn issue_1978_touched_recipes_stay_inside_the_brick_budget() {
    let mut names: Vec<&str> = POLICY_STEPS.iter().map(|(n, _)| *n).collect();
    names.extend(TOP_LEVEL);
    names.push("workflow-worktree.yaml");
    names.sort_unstable();
    names.dedup();
    for name in names {
        let lines = recipe_text(name).lines().count();
        assert!(
            lines <= BRICK_LINE_BUDGET,
            "recipes/{name} is {lines} lines; the brick budget is {BRICK_LINE_BUDGET}"
        );
    }
}

// -------------------------------------------------------------------------
// Negative guard: no literal /tmp scratch paths in recipes
// -------------------------------------------------------------------------

#[test]
fn issue_1978_no_recipe_names_a_literal_tmp_path() {
    let files: Vec<(String, String)> = scan_files()
        .into_iter()
        .map(|n| {
            let t = recipe_text(&n);
            (n, t)
        })
        .collect();
    assert!(
        files.iter().any(|(n, _)| n == "default-workflow.yaml"),
        "scan set is missing default-workflow.yaml"
    );
    let problems = allowlist_problems(&files, ALLOWLIST);
    assert!(
        problems.is_empty(),
        "{} problem(s); use \"${{TMPDIR:-/tmp}}/...\" (see docs/reference/recipe-scratch-policy.md):\n{}",
        problems.len(),
        problems.join("\n")
    );
}

#[test]
fn issue_1978_scan_scope_is_recipe_yaml_only() {
    let files = scan_files();
    assert!(!files.is_empty());
    let unique: HashSet<&String> = files.iter().collect();
    assert_eq!(unique.len(), files.len(), "duplicate scan paths");
    for f in &files {
        assert!(
            f.ends_with(".yaml") && !f.contains('/'),
            "out of scope: {f}"
        );
    }
}

// -------------------------------------------------------------------------
// Detector behaviour, in memory
// -------------------------------------------------------------------------

#[test]
fn issue_1978_detector_flags_incident_forms() {
    for line in [
        "pytest --basetemp /tmp/pr272/bt1",
        "git worktree add /tmp/wt649impl -b fix",
        "cargo mutants --in-place > /tmp/mutants.log",
        "echo done >> '/tmp/out.log'",
        "cp -r src /tmp/mutant-copy",
        "d=$(mktemp -d /tmp/wt.XXXXXX)",
        "rm -f /tmp/smart-orch-ws-*.json 2>/dev/null || true",
        r#"root="${XDG_RUNTIME_DIR:-/tmp}/amplihack-runtime/$BRANCH_NAME""#,
        r#"_cv_dir="${OUTPUT_DIR:-/tmp}/cycle_${CYCLE:-0}""#,
        r#"cd "/tmp""#,
    ] {
        assert_eq!(flagged_lines(line).len(), 1, "should flag: {line}");
    }
}

#[test]
fn issue_1978_detector_passes_safe_forms() {
    for line in [
        r#"pytest --basetemp "${TMPDIR:-/tmp}/bt1""#,
        r#"wt=$(mktemp -d "${TMPDIR:-/tmp}/wt-issue.XXXXXX")"#,
        r#"rm -f "${TMPDIR:-/tmp}"/smart-orch-ws-*.json 2>/dev/null || true"#,
        r#"selected_root=${TMPDIR-/tmp}; [ -n "$selected_root" ]"#,
        r#": "${TMPDIR:=/tmp}""#,
        r#"root="${AMPLIHACK_RUNTIME_ROOT:-${XDG_RUNTIME_DIR:-${TMPDIR:-/tmp}}/amplihack-runtime/$BRANCH_NAME}""#,
        "never under a literal /tmp path; remove each one",
        "  # old: pytest --basetemp /tmp/x",
        "",
    ] {
        assert!(flagged_lines(line).is_empty(), "should not flag: {line}");
    }
}

#[test]
fn issue_1978_flagged_lines_reports_one_based_line_numbers() {
    let text = "first\nsecond\npytest --basetemp /tmp/x\n";
    assert_eq!(
        flagged_lines(text),
        vec![(3, "pytest --basetemp /tmp/x".to_string())]
    );
}

#[test]
fn issue_1978_allowlist_rejects_stale_broad_and_unreasoned_entries() {
    let files = vec![("a.yaml".to_string(), "x=1\n".to_string())];
    let stale = allowlist_problems(&files, &[("a.yaml", "${OUTPUT_DIR:-/tmp}/c", "r")]);
    assert!(
        stale.iter().any(|p| p.contains("matches nothing")),
        "stale entry must fail: {stale:?}"
    );
    let flagged = vec![("a.yaml".to_string(), "cp -r s /tmp/abc\n".to_string())];
    let broad = allowlist_problems(&flagged, &[("a.yaml", "/tmp/abc", "r")]);
    assert!(broad.iter().any(|p| p.contains("too broad")), "{broad:?}");
    let empty = allowlist_problems(&flagged, &[("a.yaml", "cp -r s /tmp/abc", " ")]);
    assert!(
        empty.iter().any(|p| p.contains("empty reason")),
        "{empty:?}"
    );
    let other_file = allowlist_problems(&flagged, &[("b.yaml", "cp -r s /tmp/abc", "r")]);
    assert!(
        other_file.iter().any(|p| p.contains("recipes/a.yaml:1")),
        "an entry for another file must not cover this one: {other_file:?}"
    );
}

/// The edits a reviewer fears in `autodrive-crusty-round.yaml`
/// `step-04-address-concerns` are caught, without touching the file.
#[test]
fn issue_1978_bad_edits_to_a_policy_prompt_are_caught() {
    let prompt = step_prompt("autodrive-crusty-round.yaml", "step-04-address-concerns")
        .unwrap_or_else(|e| panic!("{e}"));
    for bad in ["pytest --basetemp /tmp/x", "git worktree add /tmp/wt"] {
        let edited = format!("{prompt}\n{bad}\n");
        assert!(
            !flagged_lines(&edited).is_empty(),
            "`{bad}` must be flagged"
        );
    }
    let mut failures = Vec::new();
    check_policy_text("unedited prompt", &prompt, &mut failures);
    assert!(failures.is_empty(), "{failures:?}");
    let removed = prompt.replace(POLICY_SENTENCE, "");
    check_policy_text("edited prompt", &removed, &mut failures);
    assert!(
        !failures.is_empty(),
        "deleting the policy sentence must fail the positive check"
    );
}

// -------------------------------------------------------------------------
// Targeted guards the scanner rule does not cover
// -------------------------------------------------------------------------

/// D8: `amplihack orch` writes `smart-orch-ws-*.json` with
/// `std::env::temp_dir()` ($TMPDIR, else /tmp); the safety-net cleaner in
/// smart-execute-routing must remove them from the same place.
#[test]
fn issue_1978_smart_orch_ws_writer_and_cleaner_agree() {
    let orch = read_lossy(&repo_root().join("crates/amplihack-cli/src/commands/orch.rs"));
    assert!(
        orch.contains("std::env::temp_dir()") && orch.contains("\"smart-orch-ws-\""),
        "orch.rs must still write smart-orch-ws-* under std::env::temp_dir(); \
         if the writer moved, move the cleaner with it"
    );
    let routing = recipe_text("smart-execute-routing.yaml");
    let cleaners: Vec<&str> = routing
        .lines()
        .filter(|l| !is_comment(l) && l.contains("smart-orch-ws-"))
        .collect();
    assert!(
        !cleaners.is_empty(),
        "smart-execute-routing has no smart-orch-ws cleaner"
    );
    for line in cleaners {
        assert!(
            line.contains(r#"rm -f "${TMPDIR:-/tmp}"/smart-orch-ws-*.json"#),
            "cleaner must be rm -f \"${{TMPDIR:-/tmp}}\"/smart-orch-ws-*.json: {}",
            line.trim()
        );
        assert!(
            !line.contains("rm -rf"),
            "cleaner must not recurse: {}",
            line.trim()
        );
    }
}

/// D7: the inline runtime-root fallback in workflow-worktree step-04 goes
/// through `$TMPDIR` before `/tmp`. The old `${XDG_RUNTIME_DIR:-/tmp}` form
/// is caught by the scanner too; this pins the exact new form.
#[test]
fn issue_1978_workflow_worktree_runtime_root_falls_back_through_tmpdir() {
    let text = recipe_text("workflow-worktree.yaml");
    assert!(
        text.contains(
            r#"AMPLIHACK_RUNTIME_ROOT="${AMPLIHACK_RUNTIME_ROOT:-${XDG_RUNTIME_DIR:-${TMPDIR:-/tmp}}/amplihack-runtime/$BRANCH_NAME}""#
        ),
        "workflow-worktree.yaml step-04 inline fallback must be \
         ${{XDG_RUNTIME_DIR:-${{TMPDIR:-/tmp}}}}/amplihack-runtime/$BRANCH_NAME"
    );
    assert!(
        !text.contains("${XDG_RUNTIME_DIR:-/tmp}"),
        "old fallback still present"
    );
}

/// D7 + security: the helper falls back to `${TMPDIR:-/tmp}/amplihack-runtime`,
/// creates that parent mode 0700, and refuses a symlinked or foreign root.
#[test]
fn issue_1978_runtime_root_helper_falls_back_through_tmpdir_safely() {
    let helper = read_lossy(&repo_root().join("amplifier-bundle/tools/workflow_runtime_root.sh"));
    let code: Vec<&str> = helper.lines().filter(|l| !is_comment(l)).collect();
    let code = code.join("\n");
    assert!(
        code.contains("${TMPDIR:-/tmp}/amplihack-runtime"),
        "helper fallback must be ${{TMPDIR:-/tmp}}/amplihack-runtime"
    );
    assert!(
        !code.contains("\"/tmp/amplihack-runtime"),
        "helper must not fall back to a literal /tmp/amplihack-runtime"
    );
    assert!(
        code.contains("mkdir -p -m 700"),
        "helper must create the fallback parent with mkdir -p -m 700"
    );
    assert!(code.contains("-L "), "helper must refuse a symlinked root");
    assert!(
        code.contains("-O "),
        "helper must refuse a root not owned by the caller"
    );
    assert!(code.contains("AMPLIHACK_RUNTIME_ROOT"), "pass-through kept");
    assert!(code.contains("XDG_RUNTIME_DIR"), "XDG tier kept");
}

fn run_helper(env: &[(&str, &Path)], branch: &str) -> std::process::Output {
    let helper = repo_root().join("amplifier-bundle/tools/workflow_runtime_root.sh");
    let mut cmd = std::process::Command::new("bash");
    cmd.arg(&helper).arg(branch).env_clear();
    cmd.env("PATH", std::env::var_os("PATH").expect("host tool PATH"));
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().expect("run workflow_runtime_root.sh")
}

#[cfg(unix)]
#[test]
fn issue_1978_runtime_root_helper_behaviour_under_owned_tmpdir() {
    use std::os::unix::fs::PermissionsExt;

    // Never run a helper that would still fall back to the shared /tmp: the
    // old helper ignores TMPDIR and would write /tmp/amplihack-runtime/<b>.
    let helper_text =
        read_lossy(&repo_root().join("amplifier-bundle/tools/workflow_runtime_root.sh"));
    assert!(
        helper_text.contains("${TMPDIR:-/tmp}/amplihack-runtime"),
        "workflow_runtime_root.sh must fall back to ${{TMPDIR:-/tmp}}/amplihack-runtime; \
         not running it, so this test writes nothing to the shared /tmp"
    );

    let owned = tempfile::Builder::new()
        .prefix("issue-1978-runtime-root-")
        .tempdir()
        .expect("owned tempdir");
    let tmp = owned.path().join("tmp");
    std::fs::create_dir(&tmp).expect("owned TMPDIR");

    // Third tier: neither AMPLIHACK_RUNTIME_ROOT nor XDG_RUNTIME_DIR set.
    let out = run_helper(&[("TMPDIR", &tmp)], "feat/x");
    assert!(
        out.status.success(),
        "helper failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let want = tmp.join("amplihack-runtime/feat/x");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim_end(),
        want.to_string_lossy()
    );
    for sub in ["locks", "reflection", "logs", "metrics", "provenance"] {
        assert!(want.join(sub).is_dir(), "missing {sub}");
    }
    let mode = std::fs::metadata(tmp.join("amplihack-runtime"))
        .expect("fallback parent")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        mode, 0o700,
        "fallback parent must be mode 0700, got {mode:o}"
    );

    // A symlinked root is refused.
    let elsewhere = owned.path().join("elsewhere");
    std::fs::create_dir(&elsewhere).expect("symlink target");
    std::os::unix::fs::symlink(&elsewhere, tmp.join("amplihack-runtime/evil"))
        .expect("plant symlink");
    let out = run_helper(&[("TMPDIR", &tmp)], "evil");
    assert_eq!(out.status.code(), Some(1), "symlinked root must exit 1");
    assert!(
        std::fs::read_dir(&elsewhere)
            .expect("list target")
            .next()
            .is_none(),
        "helper must not create folders through the symlink"
    );

    // First tier passes through untouched.
    let explicit = owned.path().join("explicit");
    let out = run_helper(
        &[("TMPDIR", &tmp), ("AMPLIHACK_RUNTIME_ROOT", &explicit)],
        "b",
    );
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim_end(),
        explicit.to_string_lossy()
    );
    assert!(explicit.join("locks").is_dir());

    // Usage errors are unchanged.
    let out = run_helper(&[("TMPDIR", &tmp)], "");
    assert_eq!(out.status.code(), Some(1), "empty branch must exit 1");
}
