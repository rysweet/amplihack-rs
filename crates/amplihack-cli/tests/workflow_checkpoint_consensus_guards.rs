//! Static guards from the #1554 consensus design that the existing
//! `workflow_checkpoint_static_guards.rs` and `_legacy_guards.rs` do not
//! cover: the P1 builder split, zero-A, the doc sections, the 918c sha pins,
//! EvidenceDir, the surrogateescape audit constant and the num.rs API.
//!
//! Pins are sha256 constants captured once from 918c256cf on 2026-11-07, so
//! the checks work in a shallow clone with no `git show`.

use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::support::d1_literals::{D1_INPUTS, d1_property_holds};

const ROOT_CARGO_TOML_918C: &str =
    "785015b0902b10ba7e244f1e4506ec60e237976e52f9e651e740b5a5ed46b06d";
const GUARD_SCRIPT_918C: &str = "b9f85e007703b3bb36d206fcdf672cdcdb4ccad7791167e1ddb49f66e05ffd39";
const PRE_COMMIT_918C: &str = "636a396606de0ec1363462e90c171a73a6258b4a741577ab8e89a91b0f3dcc55";

const MODULE: &str = "crates/amplihack-cli/src/workflow_checkpoint";
const DOC: &str = "docs/reference/workflow-checkpoint-cli.md";
const DOC_SECTIONS: [&str; 4] = [
    "Authorised changes (17.126)",
    "Decision 17.138",
    "D1/D2 dispositions",
    "Unresolved differences for owner disposition",
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn read(rel: &str) -> String {
    fs::read_to_string(root().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Source with `#[cfg(test)]` modules and comment lines removed.
fn production(src: &str) -> String {
    let cut = src.find("#[cfg(test)]").unwrap_or(src.len());
    src[..cut]
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The body of `fn name` (from its signature to the matching brace).
fn fn_body<'a>(src: &'a str, name: &str) -> &'a str {
    let at = src
        .find(&format!("fn {name}("))
        .unwrap_or_else(|| panic!("fn {name} not found"));
    let open = at + src[at..].find('{').unwrap();
    let mut depth = 0usize;
    for (i, ch) in src[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &src[at..open + i + 1];
                }
            }
            _ => {}
        }
    }
    &src[at..]
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        let name = e.file_name();
        if name == "target" || name == ".git" || name == "worktrees" {
            continue;
        }
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

fn all_rust_files() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for top in ["crates", "bins", "tests"] {
        rust_files(&root().join(top), &mut out);
    }
    out
}

// ------------------------------------------------------------- P1 builders

#[test]
fn selector_inheriting_command_is_a_public_amplihack_git_builder() {
    let lib = read("crates/amplihack-git/src/lib.rs");
    let body = fn_body(&lib, "selector_inheriting_command");
    assert!(
        body.starts_with("fn selector_inheriting_command() -> Command"),
        "{body}"
    );
    assert!(
        lib.contains("pub fn selector_inheriting_command() -> Command"),
        "must be public"
    );
    // 918c parity: STRICT_PATHS_PY never set GIT_TERMINAL_PROMPT, and the
    // committed P1 probe
    // `stage_strict_does_not_set_git_terminal_prompt_when_the_caller_did_not`
    // pins that. The builder therefore sets nothing; recorded in the PR body.
    assert!(
        !body.contains("TERMINAL_PROMPT_VAR") && !body.contains("GIT_TERMINAL_PROMPT"),
        "must not add GIT_TERMINAL_PROMPT (918c parity): {body}"
    );
    assert!(
        !body.contains("scrub(") && !body.contains("REPOSITORY_ENV_VARS"),
        "must not strip any selector: {body}"
    );
}

#[test]
fn amplihack_git_command_is_unchanged() {
    // #1278 and #1764 depend on command() stripping selectors.
    let lib = read("crates/amplihack-git/src/lib.rs");
    let body: String = fn_body(&lib, "command")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert_eq!(
        body,
        concat!(
            "fn command() -> Command { let mut command = interactive_",
            "command(); command.env(TERMINAL_PROMPT_VAR, \"0\"); command }"
        )
    );
}

#[test]
fn only_repo_rs_git_command_calls_selector_inheriting_command() {
    let mut callers = Vec::new();
    for p in all_rust_files() {
        let rel = p
            .strip_prefix(root())
            .unwrap()
            .to_string_lossy()
            .into_owned();
        if rel.starts_with("crates/amplihack-git/") {
            continue;
        }
        let src = fs::read_to_string(&p).unwrap_or_default();
        if production(&src).contains("selector_inheriting_command(") {
            callers.push(rel);
        }
    }
    assert_eq!(
        callers,
        vec![format!("{MODULE}/repo.rs")],
        "the only caller is workflow_checkpoint/repo.rs::git_command"
    );
    let repo = read(&format!("{MODULE}/repo.rs"));
    assert!(fn_body(&repo, "git_command").contains("selector_inheriting_command("));
}

#[test]
fn strict_rs_and_run_transport_do_not_use_the_stripping_builder() {
    let strict = production(&read(&format!("{MODULE}/strict.rs")));
    assert!(
        !strict.contains("amplihack_git::command("),
        "strict.rs is a Selectors::Ignored path (918c STRICT_PATHS_PY inherited os.environ)"
    );
    let repo = read(&format!("{MODULE}/repo.rs"));
    let transport = fn_body(&repo, "run_transport");
    assert!(
        !transport.contains("amplihack_git::command("),
        "{transport}"
    );
    assert!(
        transport.contains("GitEnv"),
        "run_transport takes a required GitEnv: {transport}"
    );
}

#[test]
fn history_rs_stays_on_the_stripping_builder() {
    let history = production(&read(&format!("{MODULE}/history.rs")));
    assert!(!history.contains("GitEnv::Inherit"));
    assert!(!history.contains("selector_inheriting_command"));
}

#[test]
fn git_env_is_derived_from_the_selectors_column() {
    let mut all = String::new();
    for f in ["mod.rs", "repo.rs"] {
        all += &read(&format!("{MODULE}/{f}"));
    }
    assert!(all.contains("enum GitEnv"), "GitEnv enum");
    assert!(
        all.contains("impl From<Selectors> for GitEnv"),
        "the row decides the builder"
    );
    let from = fn_body(&all, "from");
    assert!(
        from.contains("Selectors::Ignored => GitEnv::Inherit")
            && from.contains("Selectors::Context => GitEnv::Strip"),
        "{from}"
    );
}

// ---------------------------------------------------------------- pins

#[test]
fn root_cargo_toml_guard_script_and_pre_commit_config_match_918c() {
    for (rel, pin) in [
        ("Cargo.toml", ROOT_CARGO_TOML_918C),
        ("scripts/check-git-command-sanitised.sh", GUARD_SCRIPT_918C),
        (".pre-commit-config.yaml", PRE_COMMIT_918C),
    ] {
        assert_eq!(
            sha256(&fs::read(root().join(rel)).unwrap()),
            pin,
            "{rel} changed"
        );
    }
}

#[test]
fn no_big_integer_or_float_formatting_crate_is_added() {
    let manifest = read("crates/amplihack-cli/Cargo.toml");
    for banned in ["num-bigint", "ibig", "rug", "ryu", "dtoa", "lexical"] {
        assert!(
            !manifest
                .lines()
                .any(|l| l.trim_start().starts_with(&format!("{banned} "))
                    || l.trim_start().starts_with(&format!("{banned}="))),
            "{banned}: use decimal-string normalisation"
        );
    }
}

// ------------------------------------------------------------------ docs

fn doc_section<'a>(doc: &'a str, heading: &str) -> Option<&'a str> {
    let marker = format!("## {heading}\n");
    let start = doc.find(&marker)? + marker.len();
    let rest = &doc[start..];
    let end = rest.find("\n## ").map_or(rest.len(), |i| i + 1);
    Some(&rest[..end])
}

#[test]
fn reference_has_every_doc_section_and_no_deliberate_differences_heading() {
    let doc = read(DOC);
    for s in DOC_SECTIONS {
        assert!(doc_section(&doc, s).is_some(), "missing '## {s}'");
    }
    assert!(
        !doc.lines()
            .any(|l| l.starts_with('#') && l.contains("Deliberate differences")),
        "remove the deliberate-differences table"
    );
}

#[test]
fn decision_17_138_lists_six_rows_each_pinned_by_an_existing_test() {
    let doc = read(DOC);
    let section = doc_section(&doc, "Decision 17.138").expect("'## Decision 17.138'");
    let sources: String = all_rust_files()
        .iter()
        .filter_map(|p| fs::read_to_string(p).ok())
        .collect();
    let mut pinned = std::collections::BTreeSet::new();
    for line in section.lines() {
        for chunk in line.split('`').skip(1).step_by(2) {
            if chunk.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && sources.contains(&format!("fn {chunk}("))
            {
                pinned.insert(chunk.to_owned());
            }
        }
    }
    for n in 1..=6 {
        assert!(
            section.contains(&format!("{n}.")) || section.contains(&format!("| {n} ")),
            "row {n} missing"
        );
    }
    assert!(pinned.len() >= 6, "six named pinning tests: {pinned:?}");
    for must in [
        "owner_17_138_5_leading_dash_actions_are_unknown_private_checkpoint_operations",
        "owner_17_138_2_after_refuses_nd_digits_and_values_outside_i64",
    ] {
        assert!(pinned.contains(must), "{must} not cited");
    }
}

#[test]
fn unresolved_section_cites_every_unresolved_pinning_test() {
    let doc = read(DOC);
    let section = doc_section(&doc, "Unresolved differences for owner disposition")
        .expect("'## Unresolved differences for owner disposition'");
    let mut missing = Vec::new();
    for p in all_rust_files() {
        let src = fs::read_to_string(&p).unwrap_or_default();
        for line in src.lines() {
            // Only the pinning tests, not this guard's own name.
            if let Some(rest) = line.trim().strip_prefix("fn unresolved_")
                && !rest.starts_with("section_cites_")
            {
                let name = format!("unresolved_{}", rest.split('(').next().unwrap());
                if !section.contains(&name) {
                    missing.push(name);
                }
            }
        }
    }
    assert!(missing.is_empty(), "not listed: {missing:?}");
}

// ---------------------------------------------------------------- zero-A

#[test]
fn d1_literals_match_the_original_hashes_and_properties() {
    for (name, bytes, len, sha) in D1_INPUTS {
        assert_eq!(bytes.len(), len, "{name}");
        assert_eq!(sha256(bytes), sha, "{name}");
        assert!(d1_property_holds(name, bytes), "{name}");
    }
    // Each property is sensitive: the hook-fixed no-final-lf fails it.
    let mut fixed = D1_INPUTS[0].1.to_vec();
    fixed.push(b'\n');
    assert!(!d1_property_holds("no-final-lf", &fixed));
    assert!(!d1_property_holds("crlf", b"x\trefs/heads/a\n"));
    assert!(!d1_property_holds("mixed-oid-widths", D1_INPUTS[1].1));
}

#[test]
fn no_source_reads_the_deleted_legacy_parser_fixtures() {
    let mut users = Vec::new();
    for p in all_rust_files() {
        let src = fs::read_to_string(&p).unwrap_or_default();
        // Only the removed checkpoint fixtures: other crates have their own
        // SHA256SUMS files (the agent generator writes one) and are not ours.
        let removed = [
            "workflow_checkpoint/legacy_parser",
            "legacy_parser/no-final-lf.advert",
            "legacy_parser/crlf.advert",
            "legacy_parser/mixed-oid-widths.advert",
            "workflow_checkpoint/SHA256SUMS",
            "FIXTURES}/SHA256SUMS",
        ];
        if removed.iter().any(|path| src.contains(path)) {
            let rel = p
                .strip_prefix(root())
                .unwrap()
                .to_string_lossy()
                .into_owned();
            if !rel.ends_with("workflow_checkpoint_consensus_guards.rs")
                && !rel.ends_with("workflow_checkpoint_d1_literals.rs")
            {
                users.push(rel);
            }
        }
    }
    assert!(users.is_empty(), "still reading fixture files: {users:?}");
}

#[test]
fn fixture_readme_and_gitattributes_do_not_name_deleted_fixtures() {
    let dir = root().join("crates/amplihack-cli/tests/fixtures/workflow_checkpoint");
    for name in ["README.md", ".gitattributes"] {
        if let Ok(text) = fs::read_to_string(dir.join(name)) {
            assert!(
                !text.contains("legacy_parser") && !text.contains("SHA256SUMS"),
                "{name} is stale"
            );
        }
    }
    if let Ok(text) = fs::read_to_string(root().join(".gitattributes")) {
        assert!(!text.contains("legacy_parser"), ".gitattributes is stale");
    }
}

// ------------------------------------------------------------ EvidenceDir

#[test]
fn evidence_dir_is_per_test_when_unset_and_kept_on_panic() {
    let support = read("crates/amplihack-workflows/tests/support/canonical_staging.rs");
    assert!(
        !support.contains("expect(\"set CANONICAL_STAGING_EVIDENCE_DIR"),
        "an unset variable must give a private per-test directory, not a panic"
    );
    assert!(support.contains("struct EvidenceDir"), "EvidenceDir guard");
    assert!(
        support.contains("fn for_test("),
        "EvidenceDir::for_test(name)"
    );
    let drop = fn_body(&support, "drop");
    assert!(
        drop.contains("std::thread::panicking()"),
        "Drop keeps the directory after a panic: {drop}"
    );
    assert!(support.contains("0o700"), "private 0700 directory");
}

// --------------------------------------------------- 17.138 #1 and num.rs

#[test]
fn surrogateescape_audit_constant_exists_with_a_named_test() {
    let mut src = String::new();
    for e in fs::read_dir(root().join(MODULE)).unwrap().flatten() {
        src += &fs::read_to_string(e.path()).unwrap_or_default();
    }
    assert!(
        src.lines().any(|l| {
            let t = l.trim_start();
            t.starts_with("pub(crate) const SURROGATEESCAPE")
                || t.starts_with("pub const SURROGATEESCAPE")
                || t.starts_with("const SURROGATEESCAPE")
        }),
        "17.138 #1: add the SURROGATEESCAPE audit constant"
    );
    assert!(
        src.lines()
            .any(|l| l.trim_start().starts_with("fn surrogateescape_audit")),
        "pin it with a surrogateescape_audit* test"
    );
}

#[test]
fn num_rs_has_the_python_int_and_repr_api_without_arithmetic() {
    let num = read(&format!("{MODULE}/num.rs"));
    for item in [
        "fn py_int_normalise(",
        "enum IntPolicy",
        "AfterRc",
        "AdvertisedRc",
        "fn py_repr_float(",
        "unidata_version",
    ] {
        assert!(num.contains(item), "num.rs lacks {item}");
    }
    for banned in [
        "impl Add for PyInt",
        "impl Ord for PyInt",
        "impl PartialOrd for PyInt",
        "impl std::ops::Add",
    ] {
        assert!(!num.contains(banned), "PyInt must not expose {banned}");
    }
    let num_prod = production(&num);
    for io in ["std::fs", "std::process", "std::env"] {
        assert!(!num_prod.contains(io), "num.rs is a pure leaf: {io}");
    }
}

// --------------------------------------------------- test-harness hygiene

#[test]
fn the_wrong_oracle_validate_recipe_runner_test_is_renamed() {
    let src = read("bins/amplihack/tests/workflow_checkpoint_legacy_parity.rs");
    assert!(
        !src.contains("fn validate_recipe_runner_keeps_its_json_only_failure_and_status_1("),
        "rename to validate_recipe_runner_failure_prints_json_only"
    );
}

#[test]
fn checkpoint_test_files_never_clear_the_environment_themselves() {
    let dir = root().join("bins/amplihack/tests");
    let mut offenders = Vec::new();
    for e in fs::read_dir(&dir).unwrap().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with("workflow_checkpoint_") {
            let src = fs::read_to_string(e.path()).unwrap();
            if src.contains(".env_clear()") {
                offenders.push(name);
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "use checkpoint_cmd(sandbox): scrub(), isolated(), fixed PATH: {offenders:?}"
    );
}

#[test]
fn the_p1_and_restoration_suites_are_wired_into_the_test_binary() {
    let main = read("bins/amplihack/tests/main.rs");
    for m in [
        "mod workflow_checkpoint_p1_selectors;",
        "mod workflow_checkpoint_restorations;",
        "mod workflow_checkpoint_d1_literals;",
    ] {
        assert!(main.contains(m), "{m}");
    }
}
