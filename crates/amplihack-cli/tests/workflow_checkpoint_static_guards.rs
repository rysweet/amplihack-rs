//! Structural guards for the Rust port of the V23 checkpoint engine (#1554).
//!
//! These are class (b) checks: each asserts an absence or a structural
//! property that a behavioural test cannot see on its own (no python3, no
//! status-hiding shell, one dispatch table, one dependency edge).

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

use amplihack_cli::workflow_checkpoint::{ACTIONS as ACTION_TABLE, FORBIDDEN_SELECTORS};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn read(rel: &str) -> String {
    fs::read_to_string(root().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

const WRAPPERS: [&str; 4] = [
    "amplifier-bundle/tools/workflow_publish_checkpoint.sh",
    "amplifier-bundle/tools/workflow_stage_changes.sh",
    "amplifier-bundle/tools/workflow_pr_head_sync.sh",
    "amplifier-bundle/tools/workflow_cleanup_checkpoint.sh",
];

const RECIPES: [&str; 2] = [
    "amplifier-bundle/recipes/workflow-publish.yaml",
    "amplifier-bundle/recipes/workflow-pr-review.yaml",
];

/// The 19 .py files V23 (5f3073bef) tracked.
const V23_PY: [&str; 19] = [
    "amplifier-bundle/recipes/tests/canonical_staging_checkpoint_audit.py",
    "amplifier-bundle/recipes/tests/canonical_staging_publication.py",
    "amplifier-bundle/recipes/tests/test-canonical-staging-harness.py",
    "amplifier-bundle/recipes/tests/test-checkpoint-conservative-boundaries.py",
    "amplifier-bundle/recipes/tests/test-checkpoint-publication-boundaries.py",
    "amplifier-bundle/recipes/tests/test-checkpoint-rebase-identity.py",
    "amplifier-bundle/recipes/tests/test-checkpoint-refusal-evidence.py",
    "amplifier-bundle/recipes/tests/test-checkpoint-review-boundaries.py",
    "amplifier-bundle/recipes/tests/test-checkpoint-scope-history.py",
    "amplifier-bundle/recipes/tests/test-checkpoint-scope.py",
    "amplifier-bundle/recipes/tests/test-checkpoint-synthesis-alias.py",
    "amplifier-bundle/recipes/tests/test-checkpoint-value-transport.py",
    "amplifier-bundle/recipes/tests/test-minimalist-auto-stage.py",
    "amplifier-bundle/recipes/tests/test-pragmatic-auto-stage.py",
    "amplifier-bundle/recipes/tests/test-pragmatic-checkpoints.py",
    "amplifier-bundle/recipes/tests/test-pragmatic-crusty-directories.py",
    "amplifier-bundle/recipes/tests/test-pragmatic-evidence.py",
    "crates/amplihack-workflows/tests/fixtures/canonical_staging/audit_receipts.py",
    "crates/amplihack-workflows/tests/fixtures/canonical_staging/private_adapter.py",
];

/// Action names from the engine's closed table: one source for the
/// dispatch and these guards.
fn action_names() -> BTreeSet<String> {
    ACTION_TABLE.iter().map(|a| a.name.to_owned()).collect()
}

const MODULE: &str = "crates/amplihack-cli/src/workflow_checkpoint";

/// Every `workflow_checkpoint/*.rs`, read once and shared by the guards.
static MODULE_SOURCES: LazyLock<Vec<(PathBuf, String)>> = LazyLock::new(|| {
    let mut out = Vec::new();
    for e in fs::read_dir(root().join(MODULE)).unwrap().flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "rs") {
            out.push((p.clone(), fs::read_to_string(&p).unwrap()));
        }
    }
    out.sort();
    assert!(!out.is_empty());
    out
});

fn module_sources() -> &'static [(PathBuf, String)] {
    &MODULE_SOURCES
}

fn module_file(name: &str) -> &'static str {
    module_sources()
        .iter()
        .find(|(p, _)| p.file_name().is_some_and(|f| f == name))
        .map(|(_, s)| s.as_str())
        .unwrap_or_else(|| panic!("missing {name}"))
}

/// Production text of a module file: everything before its first
/// `#[cfg(test)]`.
fn production(src: &str) -> &str {
    src.split("#[cfg(test)]").next().unwrap()
}

// ------------------------------------------------------------- no python

#[test]
fn no_v23_python_file_is_tracked() {
    let out = amplihack_git::command_in(root())
        .args(["ls-files", "--", "*.py"])
        .output()
        .unwrap();
    let tracked = String::from_utf8(out.stdout).unwrap();
    let left: Vec<_> = V23_PY
        .iter()
        .filter(|p| tracked.lines().any(|l| l == **p))
        .collect();
    assert!(left.is_empty(), "V23 .py files still tracked: {left:#?}");
}

#[test]
fn no_wrapper_or_recipe_in_v23_scope_invokes_python3() {
    for rel in WRAPPERS.iter().chain(RECIPES.iter()) {
        for (n, l) in read(rel).lines().enumerate() {
            let code = l.trim_start();
            if code.starts_with('#') {
                continue;
            }
            assert!(!code.contains("python3"), "{rel}:{}: {l}", n + 1);
            assert!(!code.contains("hashlib"), "{rel}:{}: {l}", n + 1);
        }
    }
}

#[test]
fn recipes_use_the_worktree_path_action_at_all_four_sites() {
    let mut total = 0;
    for rel in RECIPES {
        let body = read(rel);
        let n = body
            .matches("internal workflow-checkpoint worktree-path")
            .count();
        assert_eq!(
            n, 2,
            "{rel}: expected two worktree-path extractions, found {n}"
        );
        assert!(!body.contains("python3 -I -c"), "{rel}");
        total += n;
    }
    assert_eq!(total, 4);
}

#[test]
fn check_no_python_assets_script_is_byte_identical_to_base_and_passes() {
    let diff = amplihack_git::command_in(root())
        .args([
            "diff",
            "--quiet",
            "918c256cf",
            "--",
            "scripts/check-no-python-assets.sh",
        ])
        .status()
        .unwrap();
    assert!(
        diff.success(),
        "scripts/check-no-python-assets.sh was edited"
    );
    let run = Command::new("bash")
        .current_dir(root())
        .arg("scripts/check-no-python-assets.sh")
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "check-no-python-assets.sh failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
}

// --------------------------------------------------- dispatch meta-test

fn wrapper_action_calls() -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let sources: Vec<String> = WRAPPERS
        .iter()
        .chain(RECIPES.iter())
        .map(|r| read(r))
        .collect();
    for body in &sources {
        for needle in ["workflow_checkpoint ", "internal workflow-checkpoint "] {
            for (i, _) in body.match_indices(needle) {
                let rest = &body[i + needle.len()..];
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_ascii_lowercase() || *c == '-' || c.is_ascii_digit())
                    .collect();
                if !name.is_empty() {
                    found.insert(name);
                }
            }
        }
    }
    found
}

#[test]
fn every_wrapper_call_has_an_action_and_every_action_has_a_caller() {
    let calls = wrapper_action_calls();
    let actions = action_names();
    let unknown: Vec<_> = calls.difference(&actions).collect();
    let uncalled: Vec<_> = actions.difference(&calls).collect();
    assert!(
        unknown.is_empty(),
        "wrapper calls with no action: {unknown:?}"
    );
    assert!(uncalled.is_empty(), "actions with no caller: {uncalled:?}");
}

#[test]
fn every_action_in_the_table_has_a_dispatch_arm() {
    let actions = production(module_file("actions.rs"));
    let dispatch = actions
        .split("pub(crate) fn dispatch(")
        .nth(1)
        .expect("dispatch function")
        .split("\n}\n")
        .next()
        .unwrap();
    for name in action_names() {
        assert!(
            dispatch.contains(&format!("\"{name}\" =>")),
            "no dispatch arm for {name}"
        );
    }
    let arms = dispatch.matches("\" =>").count();
    assert_eq!(
        arms,
        ACTION_TABLE.len(),
        "dispatch has arms outside the table"
    );
}

#[test]
fn the_python_dispatch_function_is_gone() {
    for rel in WRAPPERS {
        assert!(
            !read(rel).contains("workflow_checkpoint_python"),
            "{rel} still calls workflow_checkpoint_python"
        );
    }
}

#[test]
fn every_rust_action_is_in_the_documented_contract() {
    let doc = read("docs/reference/workflow-checkpoint-cli.md");
    for action in action_names() {
        assert!(
            doc.contains(&format!("`{action}")),
            "docs omit action {action}"
        );
    }
}

// ---------------------------------------------- shell status discipline

#[test]
fn wrappers_resolve_amplihack_bin_and_stop_when_it_is_missing() {
    for rel in WRAPPERS {
        let body = read(rel);
        if !body.contains("internal workflow-checkpoint")
            && !body.contains(". \"${BASH_SOURCE[0]%/*}/workflow_publish_checkpoint.sh\"")
        {
            continue;
        }
        if body.contains("internal workflow-checkpoint") {
            assert!(
                body.contains("AMPLIHACK_BIN"),
                "{rel} does not resolve AMPLIHACK_BIN"
            );
        }
    }
    let publish = read(WRAPPERS[0]);
    assert!(
        publish.contains("internal workflow-checkpoint \"$@\""),
        "no workflow_checkpoint() wrapper"
    );
}

#[test]
fn both_destination_is_head_sites_exit_with_the_real_status() {
    let body = read(WRAPPERS[0]);
    let sites: Vec<&str> = body
        .lines()
        .filter(|l| l.contains("destination-is-head") && l.contains("DESTINATION_IS_HEAD="))
        .collect();
    assert_eq!(
        sites.len(),
        2,
        "expected two destination-is-head sites: {sites:#?}"
    );
    for l in sites {
        assert!(l.contains("rc=$?"), "status not captured: {l}");
        assert!(l.contains("exit \"$rc\""), "status not passed through: {l}");
        assert!(!l.contains("exit 1"), "exit 1 flattens 79: {l}");
        assert!(!l.contains("|| true"), "status hidden: {l}");
    }
}

#[test]
fn gated_calls_never_hide_their_status() {
    for rel in WRAPPERS {
        for (n, l) in read(rel).lines().enumerate() {
            if !l.contains("workflow_checkpoint ") {
                continue;
            }
            let at = format!("{rel}:{}: {l}", n + 1);
            assert!(!l.contains("|| true"), "{at}");
            assert!(!l.contains("|| :"), "{at}");
            // A status-masking declaration builtin around the substitution.
            for builtin in ["local ", "declare ", "readonly ", "export ", "typeset "] {
                assert!(
                    !l.trim_start().starts_with(builtin) || !l.contains("$("),
                    "{at}"
                );
            }
            assert!(!l.contains("eval "), "{at}");
            assert!(!l.contains("source <("), "{at}");
        }
    }
}

#[test]
fn no_wrapper_evals_or_sources_binary_output() {
    for rel in WRAPPERS {
        let body = read(rel);
        assert!(!body.contains("eval \"$("), "{rel}");
        assert!(!body.contains("eval $("), "{rel}");
        assert!(!body.contains(". <("), "{rel}");
    }
}

#[test]
fn nul_records_are_read_with_fixed_reads_and_an_eof_check() {
    let body = read(WRAPPERS[0]);
    let reads = body.matches("IFS= read -r -d ''").count();
    // 918c: 4 admit-only fields + 4 shared fields, then one EOF check.
    assert!(reads >= 9, "found {reads} fixed NUL reads");
    assert!(
        body.contains(
            "if IFS= read -r -d '' trailing || [ -n \"${trailing-}\" ]; then return 78; fi"
        ),
        "no EOF check after the fixed reads"
    );
}

#[test]
fn pr_head_sync_accepts_exactly_0_or_3_from_advertised_ref() {
    let body = read(WRAPPERS[2]);
    assert!(
        body.contains("workflow_checkpoint advertised-ref")
            || body.contains("internal workflow-checkpoint advertised-ref")
    );
    assert!(body.contains("-eq 3"), "absent (3) is not distinguished");
    assert!(!body.contains("ORIGIN_REF_PY"));
}

#[test]
fn production_tmpdir_checks_are_unchanged() {
    let body = read(WRAPPERS[2]);
    assert!(body.matches("${TMPDIR:?assigned TMPDIR required}").count() >= 2);
}

// ------------------------------------------------------- Rust structure

/// main.rs's two internal branches: the raw-argv checkpoint route, then
/// every other internal command.
fn main_internal_branches() -> (String, String) {
    let main = read("bins/amplihack/src/main.rs");
    let checkpoint_start = main
        .find("arg == \"workflow-checkpoint\")")
        .expect("raw-argv checkpoint branch");
    let rest = &main[checkpoint_start..];
    let generic_start = rest
        .find("if args.get(1).is_some_and(|arg| arg == \"internal\") {")
        .expect("generic internal branch");
    let checkpoint = rest[..generic_start].to_owned();
    let generic = rest[generic_start..]
        .split("return;")
        .next()
        .unwrap()
        .to_owned();
    (checkpoint, generic)
}

#[test]
fn main_passes_only_checkpoint_statuses_through() {
    let (checkpoint, generic) = main_internal_branches();
    // 17.138 #5: the checkpoint argv bypasses clap and reaches the gate.
    assert!(
        checkpoint.contains("workflow_checkpoint::run_argv(&args[3..])"),
        "{checkpoint}"
    );
    assert!(
        !checkpoint.contains("Cli::parse_from"),
        "clap must not see the checkpoint argv: {checkpoint}"
    );
    // The coded status is passed through only here.
    assert!(
        checkpoint.contains("command_error::exit_code(&error)"),
        "{checkpoint}"
    );
    // Every other internal command keeps main's print-and-exit-1.
    assert!(!generic.contains("exit_code"), "{generic}");
    assert!(
        generic.contains("eprintln!(\"error: {error:#}\");"),
        "{generic}"
    );
    assert!(generic.contains("std::process::exit(1);"), "{generic}");
}

#[test]
fn checkpoint_never_calls_process_exit_and_the_validator_keeps_its_exits() {
    for (p, src) in module_sources() {
        assert!(!src.contains("process::exit"), "{}", p.display());
    }
    // validate-recipe-runner keeps main's own exits byte for byte.
    let rv = read("crates/amplihack-cli/src/runner_validation.rs");
    let base = amplihack_git::command_in(root())
        .args([
            "show",
            "918c256cf:crates/amplihack-cli/src/runner_validation.rs",
        ])
        .output()
        .unwrap();
    if base.status.success() {
        let base = String::from_utf8(base.stdout).unwrap();
        assert_eq!(
            rv.matches("std::process::exit(1)").count(),
            base.matches("std::process::exit(1)").count(),
            "runner_validation.rs changed its exits"
        );
    }
}

#[test]
fn no_profile_aborts_on_panic() {
    // run() maps a panic to status 1 with catch_unwind, which needs unwinding.
    let manifest = read("Cargo.toml");
    for l in manifest
        .lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#'))
    {
        assert!(
            !(l.starts_with("panic") && l.contains("abort")),
            "panic = abort: {l}"
        );
    }
}

/// Files with no file-system or process access.
const PURE_MODULES: [&str; 3] = ["json.rs", "json_errors.rs", "advert.rs"];

#[test]
fn checkpoint_module_has_the_planned_layout() {
    let expected: BTreeSet<&str> = [
        "mod.rs",
        "actions.rs",
        "advert.rs",
        "authority.rs",
        "history.rs",
        "json.rs",
        "json_errors.rs",
        "legacy_parity_tests.rs",
        "num.rs",
        "py_printable.rs",
        "repo.rs",
        "spec_tests.rs",
        "strict.rs",
    ]
    .into_iter()
    .collect();
    let actual: BTreeSet<&str> = module_sources()
        .iter()
        .map(|(p, _)| p.file_name().unwrap().to_str().unwrap())
        .collect();
    assert_eq!(actual, expected);
}

#[test]
fn every_module_file_is_declared_in_mod_rs() {
    let m = module_file("mod.rs");
    for (p, _) in module_sources() {
        let stem = p.file_stem().unwrap().to_str().unwrap();
        if stem == "mod" {
            continue;
        }
        let declared = m.lines().any(|l| {
            let l = l.trim();
            l == format!("mod {stem};") || l == format!("pub(crate) mod {stem};")
        });
        assert!(declared, "{stem}.rs is not declared in mod.rs");
    }
}

#[test]
fn pure_modules_touch_no_file_system_or_process() {
    for f in PURE_MODULES {
        let src = production(module_file(f));
        for banned in ["std::fs", "fs::", "Command", "std::process", "std::env"] {
            assert!(!src.contains(banned), "{f}: {banned}");
        }
    }
}

#[test]
fn checkpoint_module_denies_unwrap_and_catches_panics() {
    let m = module_file("mod.rs");
    assert!(m.contains("clippy::unwrap_used"), "no deny(unwrap_used)");
    assert!(m.contains("clippy::expect_used"), "no deny(expect_used)");
    assert!(m.contains("catch_unwind"), "no catch_unwind at run()");
    assert!(m.contains("pub fn run(action: &OsStr, args: Vec<OsString>) -> anyhow::Result<()>"));
}

#[test]
fn decision_paths_have_no_silent_defaults() {
    for (p, src) in module_sources() {
        if p.ends_with("spec_tests.rs") || p.ends_with("legacy_parity_tests.rs") {
            continue;
        }
        let prod = production(src);
        for banned in [
            ".unwrap()",
            ".expect(",
            "unwrap_or_default()",
            "unwrap_or(false)",
            ".ok()?",
        ] {
            assert!(!prod.contains(banned), "{}: {banned}", p.display());
        }
    }
}

#[test]
fn identity_modules_have_no_lossy_utf8() {
    for f in [
        "advert.rs",
        "json.rs",
        "authority.rs",
        "actions.rs",
        "repo.rs",
        "history.rs",
        "strict.rs",
    ] {
        let src = production(module_file(f));
        for (n, l) in src.lines().enumerate() {
            if l.contains("to_string_lossy") || l.contains("from_utf8_lossy") {
                // Diagnostics only: the line must build a message, not a value.
                assert!(
                    l.contains("Refusal(") || l.contains("format!(") || l.contains("eprintln!"),
                    "{f}:{}: lossy conversion outside a diagnostic: {l}",
                    n + 1
                );
            }
        }
    }
}

#[test]
fn alias_rewrite_does_not_replace_substrings_of_raw_output() {
    let a = module_file("actions.rs");
    assert!(
        !a.contains(".replace(&alias"),
        "substring replace over raw output"
    );
    assert!(
        !a.contains("replace(alias"),
        "substring replace over raw output"
    );
}

#[test]
fn forbidden_selectors_const_matches_the_918c_python_list() {
    let out = amplihack_git::command_in(root())
        .args([
            "show",
            "918c256cf:amplifier-bundle/tools/workflow_publish_checkpoint.sh",
        ])
        .output()
        .unwrap();
    let base = String::from_utf8(out.stdout).unwrap();
    let tuple = base
        .split("forbidden = (")
        .nth(1)
        .expect("918c selector tuple")
        .split(')')
        .next()
        .unwrap();
    let python: Vec<&str> = tuple
        .split(',')
        .map(|n| n.trim().trim_matches('\''))
        .filter(|n| !n.is_empty())
        .collect();
    assert_eq!(
        python, FORBIDDEN_SELECTORS,
        "selector list differs from 918c"
    );
}

#[test]
fn no_hand_written_base64_codec_remains() {
    for (p, src) in module_sources() {
        assert!(
            !src.contains("ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"),
            "{}: hand-written base64 alphabet",
            p.display()
        );
    }
    assert!(module_file("json.rs").contains("base64::engine::general_purpose::STANDARD"));
}

// ---------------------------------------------------------- dependencies

#[test]
fn base64_is_a_direct_dependency_of_amplihack_cli_only() {
    let manifest = read("crates/amplihack-cli/Cargo.toml");
    let deps = manifest
        .split("[dependencies]")
        .nth(1)
        .unwrap()
        .split("\n[")
        .next()
        .unwrap();
    assert!(
        deps.contains("base64 = \"0.22.1\""),
        "no base64 0.22.1 in [dependencies]"
    );
    // The root manifest is byte-identical to 918c256cf.
    let out = amplihack_git::command_in(root())
        .args(["diff", "--quiet", "918c256cf", "--", "Cargo.toml"])
        .status()
        .unwrap();
    assert!(out.success(), "root Cargo.toml differs from 918c256cf");
}

#[test]
fn cargo_lock_diff_is_the_single_base64_line() {
    let out = amplihack_git::command_in(root())
        .args(["diff", "-U0", "918c256cf", "--", "Cargo.lock"])
        .output()
        .unwrap();
    let diff = String::from_utf8(out.stdout).unwrap();
    let changed: Vec<&str> = diff
        .lines()
        .filter(|l| {
            (l.starts_with('+') || l.starts_with('-'))
                && !l.starts_with("+++")
                && !l.starts_with("---")
        })
        .collect();
    assert_eq!(changed, ["+ \"base64\","], "Cargo.lock diff: {diff}");
    let lock = read("Cargo.lock");
    let cli = lock
        .split("[[package]]\nname = \"amplihack-cli\"")
        .nth(1)
        .unwrap()
        .split("[[package]]")
        .next()
        .unwrap();
    assert!(
        cli.contains("\"base64\""),
        "amplihack-cli does not depend on base64 in Cargo.lock"
    );
}

// --------------------------------------------------------- docs and tests

#[test]
fn exit_contract_is_documented_once_and_linked() {
    let doc = read("docs/reference/workflow-checkpoint-cli.md");
    for needle in [
        "checkpoint scope refusal: ",
        "| 0 ",
        "| 2 ",
        "| 3 ",
        "| 1 ",
        "| 78 ",
        "| 79 ",
        "verify-pushed-ref",
        "unknown private checkpoint operation",
    ] {
        assert!(doc.contains(needle), "contract doc lacks {needle:?}");
    }
    assert!(read("docs/index.md").contains("reference/workflow-checkpoint-cli.md"));
}

#[test]
fn mapping_table_covers_every_python_method_with_one_existing_rust_test() {
    let doc = read("amplifier-bundle/recipes/tests/canonical-staging.md");
    let table = doc
        .split("## Python to Rust test mapping")
        .nth(1)
        .expect("mapping section missing");
    let mut rust_names = BTreeSet::new();
    let mut rows = 0;
    for l in table.lines().filter(|l| l.starts_with("| `")) {
        let cells: Vec<&str> = l.split('|').map(str::trim).collect();
        let py = cells[1].trim_matches('`');
        let rust = cells[2].trim_matches('`');
        assert!(py.contains("::"), "row without class::method: {l}");
        let (path, name) = rust.rsplit_once("::").expect("rust cell is path::name");
        let src =
            fs::read_to_string(root().join(path)).unwrap_or_else(|_| panic!("{path} missing"));
        assert!(
            src.contains(&format!("fn {name}(")),
            "{rust} does not exist"
        );
        assert!(rust_names.insert(rust.to_owned()), "{rust} mapped twice");
        rows += 1;
    }
    assert!(
        rows >= 32,
        "only {rows} mapped methods; need the 20 + 12 plus harness methods"
    );
}

#[test]
fn the_five_controls_are_no_longer_ignored_and_every_ignore_has_a_reason() {
    let ck = read("crates/amplihack-workflows/tests/canonical_staging_checkpoint_test.rs");
    let ev = read("crates/amplihack-workflows/tests/canonical_staging_evidence_test.rs");
    let ignored = |src: &str, name: &str| {
        let i = src
            .find(&format!("fn {name}("))
            .unwrap_or_else(|| panic!("{name} missing"));
        src[..i].rsplit("\n\n").next().unwrap().contains("#[ignore")
    };
    for (src, name) in [
        (&ck, "actual_destination_evidence_detects_an_omitted_push"),
        (
            &ck,
            "publication_controls_retain_deleted_content_modes_and_unreachable_commits",
        ),
        (
            &ev,
            "scope_history_retains_replaced_transient_commits_and_refuses_grafts",
        ),
        (
            &ev,
            "scope_history_refuses_a_shallow_tip_even_when_the_base_object_is_present",
        ),
        (
            &ev,
            "dependency_baseline_ignores_replacements_and_refuses_forged_bytes",
        ),
    ] {
        assert!(!ignored(src, name), "{name} is still #[ignore]");
    }
    for src in [
        &ck,
        &ev,
        &read("crates/amplihack-workflows/tests/canonical_staging_runner_test.rs"),
    ] {
        for l in src
            .lines()
            .filter(|l| l.trim_start().starts_with("#[ignore"))
        {
            assert!(l.contains("#[ignore = \""), "ignore without a reason: {l}");
            assert!(
                !l.contains("task-private evidence"),
                "stale reason string: {l}"
            );
        }
    }
}

#[test]
fn step20b_sync_tests_own_their_tmpdir() {
    // Every step20b test runs the tools through Remote::run, which owns the
    // TMPDIR; the production `${TMPDIR:?...}` requirement keeps a negative test.
    let remote = read("tests/integration/issue_1714_finalization_handoff/step20b_remote.rs");
    let run = remote
        .split("pub fn run(&self, id: &str)")
        .nth(1)
        .expect("Remote::run")
        .split("\n    }\n")
        .next()
        .unwrap();
    assert!(
        run.contains(".env(\"TMPDIR\", self.tmpdir())"),
        "Remote::run does not set a test-owned TMPDIR"
    );
    let src = read("tests/integration/issue_1714_finalization_handoff/step20b.rs");
    for name in [
        "sync_leaves_a_worktree_alone_when_the_remote_is_not_ahead",
        "sync_then_push_lands_on_the_pr_head_with_or_without_an_upstream",
    ] {
        assert!(src.contains(&format!("fn {name}(")), "{name} missing");
    }
    assert!(
        src.contains("assigned TMPDIR required") && src.contains("run_without_tmpdir("),
        "no negative TMPDIR-unset test"
    );
}

#[test]
fn no_test_spawns_python3_or_includes_a_py_fixture() {
    let dirs = [
        "crates/amplihack-workflows/tests",
        "crates/amplihack-cli/tests",
        "tests/integration",
    ];
    for d in dirs {
        let mut stack = vec![root().join(d)];
        while let Some(dir) = stack.pop() {
            for e in fs::read_dir(&dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                if p.extension().is_none_or(|x| x != "rs") {
                    continue;
                }
                if p.ends_with("workflow_checkpoint_static_guards.rs") {
                    continue;
                }
                let src = fs::read_to_string(&p).unwrap();
                let checkpoint_related =
                    src.contains("checkpoint") || src.contains("canonical_staging");
                if !checkpoint_related {
                    continue;
                }
                assert!(
                    !src.contains("Command::new(\"python3\")"),
                    "{} spawns python3",
                    p.display()
                );
                assert!(
                    !src.contains(".py\")"),
                    "{} includes a .py fixture",
                    p.display()
                );
            }
        }
    }
}

// ------------------------------------------------- C-a: main.rs and tokens

fn rs_files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            rs_files_under(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// The checkpoint route flushes stdout and stderr before its one
/// `std::process::exit`, which skips destructors.
#[test]
fn the_checkpoint_route_flushes_both_streams_before_it_exits() {
    let (checkpoint, _) = main_internal_branches();
    let exits: Vec<usize> = checkpoint
        .match_indices("process::exit(")
        .map(|(i, _)| i)
        .collect();
    assert_eq!(exits.len(), 1, "{checkpoint}");
    for flush in ["stdout().flush()", "stderr().flush()"] {
        let at = checkpoint
            .find(flush)
            .unwrap_or_else(|| panic!("no {flush}: {checkpoint}"));
        assert!(at < exits[0], "{flush} after the exit: {checkpoint}");
    }
}

/// R2(b): the concat!-built token is confined to test sources.
#[test]
fn no_production_source_builds_the_interactive_token_with_concat() {
    let mut files = Vec::new();
    for top in ["crates", "bins"] {
        rs_files_under(&root().join(top), &mut files);
    }
    let offenders: Vec<_> = files
        .iter()
        .filter(|p| {
            p.strip_prefix(root())
                .unwrap()
                .components()
                .any(|c| c.as_os_str() == "src")
        })
        .filter(|p| {
            fs::read_to_string(p)
                .unwrap_or_default()
                .contains("concat!(\"interactive_\"")
        })
        .collect();
    assert!(offenders.is_empty(), "{offenders:?}");
}

// ------------------------------------------- R12: selector commands

/// selector_inheriting_command() changes no environment variable;
/// command() still strips and sets the prompt variable.
#[test]
fn selector_inheriting_command_touches_no_environment_variable() {
    let inherit = amplihack_git::selector_inheriting_command();
    assert_eq!(inherit.get_envs().count(), 0);
    assert_eq!(inherit.get_program(), "git");
    let strip = amplihack_git::command();
    let envs: std::collections::BTreeMap<_, _> = strip.get_envs().collect();
    assert!(
        envs.get(std::ffi::OsStr::new("GIT_DIR"))
            .is_some_and(|v| v.is_none())
    );
    assert_eq!(
        envs.get(std::ffi::OsStr::new("GIT_TERMINAL_PROMPT")),
        Some(&Some(std::ffi::OsStr::new("0")))
    );
}

// ------------------------------------------- R8: D1 fixtures

#[test]
fn the_legacy_parser_fixture_directory_is_gone() {
    let fixtures = root().join("crates/amplihack-cli/tests/fixtures/workflow_checkpoint");
    let dir = fixtures.join("legacy_parser");
    assert!(!dir.exists(), "{} still exists", dir.display());
    for name in [".gitattributes", "README.md"] {
        if let Ok(text) = fs::read_to_string(fixtures.join(name)) {
            assert!(
                !text.contains(".advert"),
                "{name} names a deleted .advert fixture"
            );
        }
    }
}

// ------------------------------------------- R4/R10: docs

#[test]
fn the_reference_lists_exactly_the_eight_rows_each_with_its_pin_test() {
    let doc = read("docs/reference/workflow-checkpoint-cli.md");
    assert!(!doc.contains("Deliberate differences"));
    for id in [
        "17.138-1",
        "17.138-2",
        "17.138-3",
        "17.138-4",
        "17.138-6",
        "17.126-dest-read-79-site-1",
        "17.126-dest-read-79-site-2",
        "17.126-stage-strict-79",
    ] {
        let line = doc
            .lines()
            .find(|l| l.contains(id))
            .unwrap_or_else(|| panic!("docs: {id} not listed"));
        assert!(line.contains('`'), "docs: {id} cites no pin_test: {line}");
    }
}

// ------------------------------------------- R18: manifests

#[test]
fn root_cargo_toml_has_no_base64_and_no_added_comment() {
    let root_toml = read("Cargo.toml");
    assert!(
        !root_toml.contains("base64"),
        "base64 belongs in amplihack-cli only"
    );
    let cli = read("crates/amplihack-cli/Cargo.toml");
    assert!(cli.lines().any(|l| l.trim() == "base64 = \"0.22.1\""));
}

// ------------------------------------------- 17.116: push-range provenance

/// The functions that derive or check the outgoing range. The range comes
/// only from the destination advertisement read at push time, so none of
/// them may read a tracking ref or FETCH_HEAD. The sync paths in `after`,
/// `sync_fetch_mode` and `publication` use FETCH_HEAD and refs/remotes as
/// 918c's workflow_pr_head_sync.sh did, which is why this guard names
/// function bodies rather than whole modules. A new range helper must be
/// added here; the poisoned-ref behavioural test is the backstop.
const PUSH_RANGE_FNS: [(&str, &str); 5] = [
    ("actions.rs", "before"),
    ("actions.rs", "destination_is_head"),
    ("actions.rs", "push_selection"),
    ("history.rs", "outgoing"),
    ("repo.rs", "destination_tip"),
];

/// The code of `fn name(`'s body with comments and string contents blanked:
/// braces are matched outside `//` and `/* */` comments, string literals
/// (including raw strings) and char literals. String contents are kept,
/// because a forbidden ref name would appear inside a string argument.
/// Panics if the function is missing, so a rename cannot empty the guard.
fn code_of_fn(text: &str, name: &str) -> String {
    let start = text
        .find(&format!("fn {name}("))
        .unwrap_or_else(|| panic!("fn {name} not found"));
    let bytes = text.as_bytes();
    let mut out = String::new();
    let (mut depth, mut i, mut opened) = (0usize, start, false);
    while i < bytes.len() {
        let rest = &text[i..];
        if rest.starts_with("//") {
            i += rest.find('\n').unwrap_or(rest.len());
            continue;
        }
        if rest.starts_with("/*") {
            i += rest.find("*/").map_or(rest.len(), |e| e + 2);
            continue;
        }
        if rest.starts_with("r#\"") || rest.starts_with("r\"") {
            let hashes = rest[1..].bytes().take_while(|&b| b == b'#').count();
            let open = 1 + hashes + 1;
            let close = format!("\"{}", "#".repeat(hashes));
            let end = rest[open..]
                .find(&close)
                .map_or(rest.len(), |e| open + e + close.len());
            out.push_str(&rest[..end]);
            i += end;
            continue;
        }
        match bytes[i] {
            b'"' => {
                let mut j = i + 1;
                while j < bytes.len() && bytes[j] != b'"' {
                    j += if bytes[j] == b'\\' { 2 } else { 1 };
                }
                out.push_str(&text[i..=j.min(bytes.len() - 1)]);
                i = j + 1;
                continue;
            }
            b'\'' if bytes.get(i + 2) == Some(&b'\'') => {
                i += 3;
                continue;
            }
            b'\'' if bytes.get(i + 1) == Some(&b'\\') && bytes.get(i + 3) == Some(&b'\'') => {
                i += 4;
                continue;
            }
            b'{' => {
                depth += 1;
                opened = true;
            }
            b'}' => {
                depth -= 1;
                if opened && depth == 0 {
                    out.push('}');
                    return out;
                }
            }
            _ => {}
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    panic!("fn {name}: unbalanced braces");
}

#[test]
fn the_push_range_never_reads_tracking_refs_or_fetch_head() {
    let forbidden = [
        "FETCH_HEAD",
        "refs/remotes",
        "@{u}",
        "@{upstream}",
        "origin/",
    ];
    let mut hits = Vec::new();
    for (file, name) in PUSH_RANGE_FNS {
        let text = read(&format!(
            "crates/amplihack-cli/src/workflow_checkpoint/{file}"
        ));
        let code = code_of_fn(&text, name);
        for pattern in forbidden {
            if code.contains(pattern) {
                hits.push(format!("{file}::{name} contains {pattern:?}"));
            }
        }
        // A name split across a macro's arguments would hide from the
        // substring check, so the range code may not build strings that way.
        if code.contains("concat!(") {
            hits.push(format!("{file}::{name} builds a string with concat!"));
        }
    }
    assert!(hits.is_empty(), "{hits:#?}");
}

/// Negative half: the extractor finds a planted pattern in code and in a
/// string argument, ignores it in comments, and is not ended early by
/// braces inside strings, raw strings or char literals.
#[test]
fn code_of_fn_sees_planted_patterns_and_skips_comments() {
    let text = concat!(
        "fn a() { let s = \"}{\"; let r = r#\"}\"#; let c = '}';\n",
        "  // FETCH_HEAD in a comment\n",
        "  /* refs/remotes { */ git(&[\"rev-parse\", \"FETCH_HEAD\"]) }\n",
        "fn b() { /* FETCH_HEAD */ }\n",
    );
    let a = code_of_fn(text, "a");
    assert!(a.contains("FETCH_HEAD"), "{a}");
    assert!(!a.contains("refs/remotes"), "{a}");
    assert!(!a.contains("in a comment"), "{a}");
    assert!(!code_of_fn(text, "b").contains("FETCH_HEAD"));
}

/// Macro case: a pattern passed to a macro is code, and so is a `concat!`
/// that could assemble one from pieces; braces inside the macro's strings do
/// not end the body early.
#[test]
fn code_of_fn_sees_patterns_inside_macro_invocations() {
    let text = concat!(
        "fn m() { let a = format!(\"{}\", \"refs/remotes/x\"); ",
        "let b = concat!(\"FETCH_\", \"HEAD\"); git(&[&a, b]) }\n",
        "fn after() {}\n",
    );
    let m = code_of_fn(text, "m");
    assert!(m.contains("refs/remotes"), "{m}");
    assert!(m.contains("concat!("), "{m}");
    assert!(m.ends_with("git(&[&a, b]) }"), "{m}");
    assert!(!m.contains("after"), "{m}");
}

#[test]
#[should_panic(expected = "fn missing not found")]
fn code_of_fn_refuses_a_missing_function() {
    code_of_fn("fn present() {}", "missing");
}
