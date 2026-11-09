//! Inventory validator (#1554, consensus step 9).
//!
//! Three stores, all read-only:
//!
//! * `inventory_ids.json`: derived from the pinned copy of cc's
//!   `pr2020-rust-reference-inventory-918c/inventory.json` (sha256
//!   0c3e19b0…6c788f). IDs, summary counts and cc's sha256 only.
//! * `inventory_map.toml`: one row per reference ID, each resolving to
//!   exactly one Rust test that runs normally or one owner disposition.
//!   Provenance and audit receipts live in separate tables and never count.
//! * the Rust sources, scanned structurally. Execution is proved by the real
//!   `cargo test` run, not here.
//!
//! The validator always runs against the committed IDs, and those IDs are
//! cross-checked against the pinned copy of cc's inventory
//! (`inventory.json.part-1` + `part-2`). A missing pinned file fails; no path
//! skips.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

const DIR: &str = "crates/amplihack-cli/tests/fixtures/workflow_checkpoint";
/// cc's inventory, pinned in the repository. It is committed in two parts,
/// split at a line boundary, because it exceeds the 500 KB
/// check-added-large-files limit; the concatenation is checked against
/// `CC_SHA256`.
const PINNED_INVENTORY_PARTS: [&str; 2] = ["inventory.json.part-1", "inventory.json.part-2"];
const CC_SHA256: &str = "0c3e19b0c72c92a3bd2a140acedcf03f311cb11e9b2c336abf8d3797cc6c788f";
const DOC: &str = "docs/reference/workflow-checkpoint-cli.md";

/// Shared with the doc guard in `workflow_checkpoint_consensus_guards.rs`.
pub const DOC_SECTIONS: [&str; 4] = [
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

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

// ------------------------------------------------------------------ model

#[derive(Debug, Default, Clone)]
pub struct Ids {
    pub source_sha256: String,
    pub python_tests: BTreeSet<String>,
    pub procedural: BTreeSet<String>,
    /// `asset::step_id` -> original_authorised
    pub roles: BTreeMap<String, bool>,
    pub sites: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Violation {
    Missing(String),
    Duplicate(String),
    Invented(String),
    /// More than one of `rust_test`, `disposition` and `pending_file`.
    MultipleColumns(String),
    NeitherColumn(String),
    IgnoredTarget(String),
    NoSuchTest(String),
    CfgGated(String),
    EnvGatedEarlyReturn(String),
    UnknownDisposition(String),
    MissingDocSection(String),
    WrongSha(String),
    ProvenanceCounted(String),
    Cardinality(String),
    AssertionCount(String),
    /// A `corpus_<action>#<case>` target whose case is not in the corpus.
    NoSuchCorpusCase(String),
    /// The corpus case exists but its `inventory_ref` is not this row's ID.
    CorpusRefMismatch(String),
    /// A disposition ID that is not a recorded owner decision (for example
    /// an invented "pending" row).
    UnrecordedDisposition(String),
    /// An approved row's `pin_test` resolves to more than one test.
    AmbiguousPinTest(String),
    /// Two approved rows name the same `pin_test`.
    SharedPinTest(String),
    /// An approved row covers an ID that is not in the inventory.
    UnknownCover(String),
    /// A `pending_file` that is not a normalised, tracked repository path.
    PendingFileNotTracked(String),
    /// A `pending_file` that is not the row's own source file.
    PendingFileMismatch(String),
    /// A role row still pending once the removal control (`REMOVAL_CONTROL`)
    /// exists in the index.
    PendingRoleAfterControl(String),
    /// A tracked `.py` file outside the expected set, an expected file
    /// outside the pinned set, or an expected helper that is not tracked.
    UnaccountedPython(String),
}

/// One owner-approved departure from 918c parity, and the reference
/// obligations (inventory IDs) it is recorded as resolving.
#[derive(Debug, Clone, Copy)]
pub struct ApprovedRow {
    pub id: &'static str,
    pub decision: &'static str,
    /// Inventory IDs this row may resolve. Empty means the row is approved
    /// as a behaviour difference but resolves no reference obligation, so a
    /// map row that cites it for coverage is refused.
    pub covers: &'static [&'static str],
    /// The test that pins the departure, as `<module path>::<fn>`: either
    /// `amplihack_cli::<path under crates/amplihack-cli/src>::<fn>` or
    /// `<integration test file stem>::<fn>`. Mandatory: it must resolve to
    /// exactly one runnable `#[test]`, no other row may share it, and it must
    /// appear as `ok` in the ledger of the final head.
    pub pin_test: &'static str,
    /// The result of searching cc's 190 reference test IDs for an
    /// obligation this row might resolve. A candidate is recorded here, not
    /// in `covers`, until the owner records it as resolved.
    pub inventory_search: &'static str,
}

/// The exact allowlist bb (owner) confirmed on 2026-11-08: decision 17.138
/// items 1, 2, 3, 4 and 6, the 17.126 destination-read 79 at its two sites
/// (both `destination-is-head` assignments in workflow_publish_checkpoint.sh
/// keep a real 79), and the 17.126 stage-strict addendum (a real Git 79 from
/// the staging child passes through as 79). Item 5 is a restoration, not an
/// exception; 17.139, 17.140, 17.141 and every other 17.x approve no
/// departure. No row is recorded as approving the omission of a Python test
/// method, so `covers` is empty and each row is pinned by its `pin_test`
/// instead; an obligation is added to `covers` only with the owner's record.
pub const APPROVED_ROWS: [ApprovedRow; 8] = [
    ApprovedRow {
        id: "17.138-1",
        decision: "17.138",
        covers: &[],
        pin_test: "amplihack_cli::workflow_checkpoint::spec_tests::surrogateescape_audit_matches_decision_17_138_1",
        inventory_search: "searched 'utf', 'surrogate': test-checkpoint-value-transport.py::CheckpointValueTransport.test_non_utf8_bytes_remain_data and test-checkpoint-scope.py::FilenameCapabilityTests.test_unconstructible_invalid_utf8_is_explicitly_unexecuted are candidates; neither is recorded as resolved by this row",
    },
    ApprovedRow {
        id: "17.138-2",
        decision: "17.138",
        covers: &[],
        pin_test: "workflow_checkpoint_restorations::owner_17_138_2_after_refuses_nd_digits_and_values_outside_i64",
        inventory_search: "searched 'after' and 'rc': the hits concern history and boundaries; no reference test exercises the `after` status argument; no candidate",
    },
    ApprovedRow {
        id: "17.138-3",
        decision: "17.138",
        covers: &[],
        pin_test: "workflow_checkpoint_restorations::owner_17_138_3_a_top_level_lone_surrogate_worktree_path_exits_1",
        inventory_search: "searched 'surrogate' and 'worktree': test-checkpoint-scope.py::ProjectionJSONTests.test_unicode_pairs_pass_and_lone_surrogates_remain_refused tests JSON projection, not worktree-path; no candidate",
    },
    ApprovedRow {
        id: "17.138-4",
        decision: "17.138",
        covers: &[],
        pin_test: "amplihack_cli::workflow_checkpoint::spec_tests::records_refuse_a_field_containing_nul",
        inventory_search: "searched 'nul' and 'admit': test-checkpoint-value-transport.py::CheckpointValueTransport.test_admit_preserves_hostile_values_without_evaluating_them is a candidate (it passes no NUL); not recorded as resolved by this row",
    },
    ApprovedRow {
        id: "17.138-6",
        decision: "17.138",
        covers: &[],
        pin_test: "amplihack_cli::workflow_checkpoint::legacy_parity_tests::a_panic_maps_to_status_1_like_an_uncaught_python_exception",
        inventory_search: "searched 'panic', 'exception' and 'status': the two 'status' hits test fetch and hook statuses, not an uncaught failure; no candidate",
    },
    ApprovedRow {
        id: "17.126-dest-read-79-site-1",
        decision: "17.126",
        covers: &[],
        pin_test: "workflow_checkpoint_exit79::site_1_reports_a_terminal_79_from_the_destination_read_as_79",
        inventory_search: "searched '79', 'destination': test-checkpoint-review-boundaries.py::AdditionalReviewBoundaries.test_fetch_terminal_79_remains_terminal and test-checkpoint-scope.py::RepairBoundaryTests.test_terminal_79_is_not_retried_or_reclassified_as_success test a fetch 79, not the destination read; no candidate",
    },
    ApprovedRow {
        id: "17.126-dest-read-79-site-2",
        decision: "17.126",
        covers: &[],
        pin_test: "workflow_checkpoint_exit79::site_2_reports_a_terminal_79_from_the_destination_read_as_79",
        inventory_search: "searched '79', 'destination': test-checkpoint-review-boundaries.py::AdditionalReviewBoundaries.test_fetch_terminal_79_remains_terminal and test-checkpoint-scope.py::RepairBoundaryTests.test_terminal_79_is_not_retried_or_reclassified_as_success test a fetch 79, not the destination read; no candidate",
    },
    ApprovedRow {
        id: "17.126-stage-strict-79",
        decision: "17.126",
        covers: &[],
        pin_test: "workflow_checkpoint_p1_selectors::stage_strict_passes_a_real_git_79_through",
        inventory_search: "searched '79', 'strict': the two fetch-79 tests and test-checkpoint-scope.py::StrictStagingTests (none runs a staging child that exits 79); no candidate",
    },
];

fn approved_row<'a>(
    approved: &'a [ApprovedRow],
    id: &str,
    decision: &str,
) -> Option<&'a ApprovedRow> {
    approved
        .iter()
        .find(|r| r.id == id && r.decision == decision)
}

/// The one index key a `pin_test` names, or every candidate it matched.
/// `amplihack_cli::a::b::f` is `f` in `crates/amplihack-cli/src/a/b.rs` (or
/// `a/b/mod.rs`); any other `m::f` is `f` in a file `tests/m.rs`, or `m.rs`
/// at the top of the scanned tree. A function of that name in any other
/// module does not match.
pub fn resolve_pin(pin: &str, index: &RustTestIndex) -> Result<String, Vec<String>> {
    let Some((module, name)) = pin.rsplit_once("::") else {
        return Err(Vec::new());
    };
    let files: Vec<String> = match module.strip_prefix("amplihack_cli::") {
        Some(rest) => {
            let path = format!("crates/amplihack-cli/src/{}", rest.replace("::", "/"));
            vec![format!("{path}.rs"), format!("{path}/mod.rs")]
        }
        None if !module.contains("::") => vec![format!("{module}.rs")],
        None => return Err(Vec::new()),
    };
    let crate_path = module.starts_with("amplihack_cli::");
    let hits: Vec<String> = index
        .keys()
        .filter(|key| {
            key.strip_suffix(name)
                .and_then(|k| k.strip_suffix("::"))
                .is_some_and(|file| {
                    files.iter().any(|f| {
                        file == f || (!crate_path && file.ends_with(&format!("/tests/{f}")))
                    })
                })
        })
        .cloned()
        .collect();
    match hits.as_slice() {
        [one] => Ok(one.clone()),
        _ => Err(hits),
    }
}

/// action -> case name -> `inventory_ref` (if any), from the corpus files.
pub type Corpus = BTreeMap<String, BTreeMap<String, Option<String>>>;

pub fn load_corpus(dir: &Path) -> Corpus {
    let mut corpus = Corpus::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return corpus;
    };
    for e in entries.flatten() {
        let p = e.path();
        let Some(action) = p
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".toml"))
        else {
            continue;
        };
        let text = fs::read_to_string(&p).unwrap_or_default();
        let value: toml::Value =
            toml::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        let cases = corpus.entry(action.to_owned()).or_default();
        for case in value
            .get("case")
            .and_then(|c| c.as_array())
            .into_iter()
            .flatten()
        {
            let name = case.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let reference = case
                .get("inventory_ref")
                .and_then(|r| r.as_str())
                .map(str::to_owned);
            cases.insert(name.to_owned(), reference);
        }
    }
    corpus
}

#[derive(Debug, Default, Clone)]
pub struct TestFacts {
    pub ignored: bool,
    pub cfg_gated: bool,
    pub env_gated_early_return: bool,
}

/// `path::fn` -> facts, from a structural scan.
pub type RustTestIndex = BTreeMap<String, TestFacts>;

pub fn load_ids(text: &str) -> Ids {
    let v: serde_json::Value = serde_json::from_str(text).expect("inventory_ids.json parses");
    let set = |k: &str| -> BTreeSet<String> {
        v[k].as_array()
            .unwrap_or_else(|| panic!("{k} array"))
            .iter()
            .map(|s| s.as_str().unwrap().to_owned())
            .collect()
    };
    let roles = v["roles"]
        .as_array()
        .expect("roles array")
        .iter()
        .map(|r| {
            (
                format!(
                    "{}::{}",
                    r["asset"].as_str().unwrap(),
                    r["step_id"].as_str().unwrap()
                ),
                r["original_authorised"].as_bool().unwrap(),
            )
        })
        .collect();
    Ids {
        source_sha256: v["source_sha256"].as_str().unwrap_or("").to_owned(),
        python_tests: set("python_tests"),
        procedural: set("procedural"),
        roles,
        sites: set("checkpoint_sites"),
    }
}

/// Scan Rust sources for `fn name(` preceded by `#[test]`.
pub fn scan(files: &[(String, String)]) -> RustTestIndex {
    let mut index = RustTestIndex::new();
    for (path, src) in files {
        let mut attrs: Vec<&str> = Vec::new();
        let mut lines = src.lines().peekable();
        while let Some(line) = lines.next() {
            let t = line.trim();
            if t.starts_with("#[") {
                attrs.push(t);
                continue;
            }
            if let Some(rest) = t
                .strip_prefix("fn ")
                .or_else(|| t.strip_prefix("pub fn "))
                .or_else(|| t.strip_prefix("async fn "))
            {
                let name = rest.split(['(', '<']).next().unwrap_or("").to_owned();
                if attrs.iter().any(|a| {
                    *a == "#[test]" || a.starts_with("#[rstest") || a.starts_with("#[tokio::test")
                }) {
                    let body_start: Vec<&str> = lines
                        .clone()
                        .take(4)
                        .map(str::trim)
                        .filter(|l| !l.is_empty() && !l.starts_with("//"))
                        .take(2)
                        .collect();
                    let env_gate = body_start.first().is_some_and(|l| {
                        (l.contains("env::var") || l.contains("env::var_os"))
                            && (l.starts_with("if ")
                                || l.starts_with("let Some")
                                || l.starts_with("let Ok"))
                    }) && body_start.iter().any(|l| l.contains("return"));
                    index.insert(
                        format!("{path}::{name}"),
                        TestFacts {
                            ignored: attrs.iter().any(|a| a.starts_with("#[ignore")),
                            cfg_gated: attrs.iter().any(|a| {
                                a.starts_with("#[cfg(")
                                    && !a.contains("unix")
                                    && !a.contains("target_os = \"linux\"")
                            }),
                            env_gated_early_return: env_gate,
                        },
                    );
                }
            }
            if !t.is_empty() && !t.starts_with("//") {
                attrs.clear();
            }
        }
    }
    index
}

const KINDS: [(&str, &str); 4] = [
    ("python_test", "python_tests"),
    ("procedural", "procedural"),
    ("role", "roles"),
    ("site", "sites"),
];

// ------------------------------------------- decision 17.146-2 (partial map)

/// The `.py` files tracked when `inventory_map.toml` was created (C-s). The
/// expected set of every later commit is a subset of these; a new `.py` file
/// is never accounted for, even with a pending row.
pub const PINNED_19: [&str; 19] = [
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

/// The five shared Python files. A helper is expected only while a file that
/// is itself expected uses it; its own pending rows never keep it.
pub const HELPERS: [&str; 5] = [
    "amplifier-bundle/recipes/tests/canonical_staging_checkpoint_audit.py",
    "amplifier-bundle/recipes/tests/canonical_staging_publication.py",
    "amplifier-bundle/recipes/tests/test-canonical-staging-harness.py",
    "crates/amplihack-workflows/tests/fixtures/canonical_staging/audit_receipts.py",
    "crates/amplihack-workflows/tests/fixtures/canonical_staging/private_adapter.py",
];

/// Each helper and the tracked `.py` files that name it (checked against
/// `git grep -l -F` in both directions while the helper is tracked).
pub const HELPER_USERS: [(&str, &[&str]); 5] = [
    (
        "amplifier-bundle/recipes/tests/canonical_staging_checkpoint_audit.py",
        &[
            "amplifier-bundle/recipes/tests/test-checkpoint-review-boundaries.py",
            "amplifier-bundle/recipes/tests/test-checkpoint-scope.py",
            "amplifier-bundle/recipes/tests/test-minimalist-auto-stage.py",
            "amplifier-bundle/recipes/tests/test-pragmatic-auto-stage.py",
        ],
    ),
    (
        "amplifier-bundle/recipes/tests/canonical_staging_publication.py",
        &[
            "amplifier-bundle/recipes/tests/test-canonical-staging-harness.py",
            "amplifier-bundle/recipes/tests/test-checkpoint-publication-boundaries.py",
            "amplifier-bundle/recipes/tests/test-checkpoint-scope.py",
            "amplifier-bundle/recipes/tests/test-minimalist-auto-stage.py",
            "amplifier-bundle/recipes/tests/test-pragmatic-checkpoints.py",
            "amplifier-bundle/recipes/tests/test-pragmatic-evidence.py",
        ],
    ),
    (
        "amplifier-bundle/recipes/tests/test-canonical-staging-harness.py",
        &["amplifier-bundle/recipes/tests/test-checkpoint-scope.py"],
    ),
    (
        "crates/amplihack-workflows/tests/fixtures/canonical_staging/audit_receipts.py",
        &["amplifier-bundle/recipes/tests/test-checkpoint-scope.py"],
    ),
    (
        "crates/amplihack-workflows/tests/fixtures/canonical_staging/private_adapter.py",
        &["amplifier-bundle/recipes/tests/test-checkpoint-scope.py"],
    ),
];

/// Role rows stay pending with this file until the removal control exists.
pub const ROLE_PENDING_FILE: &str = "amplifier-bundle/recipes/tests/test-pragmatic-checkpoints.py";

/// The source-level removal control (decision 17.146-1, step 1). Once it is
/// in the index, no role row may stay pending.
pub const REMOVAL_CONTROL: &str = "crates/amplihack-workflows/tests/canonical_staging_policy_test.rs::removing_any_authorised_opt_out_is_named_by_the_policy_check";

/// Each checkpoint site and the last file in port order that names its step.
/// This is a derivation, not inventory data; it is documented in
/// `docs/reference/workflow-checkpoint-inventory-map.md` and checked against
/// `git grep` while the files are tracked.
pub const SITE_PENDING_FILES: [(&str, &str); 7] = [
    (
        "autodrive-crusty-round::step-04-address-concerns",
        "amplifier-bundle/recipes/tests/test-minimalist-auto-stage.py",
    ),
    (
        "workflow-tdd::checkpoint-after-implementation",
        "amplifier-bundle/recipes/tests/test-minimalist-auto-stage.py",
    ),
    (
        "workflow-refactor-review::checkpoint-after-review-feedback",
        "amplifier-bundle/recipes/tests/test-minimalist-auto-stage.py",
    ),
    (
        "workflow-publish::step-16b-outside-in-fix-loop",
        "amplifier-bundle/recipes/tests/test-minimalist-auto-stage.py",
    ),
    (
        "workflow-publish::step-15-commit-push",
        "amplifier-bundle/recipes/tests/test-checkpoint-scope.py",
    ),
    (
        "workflow-pr-review::step-18c-push-feedback-changes",
        "amplifier-bundle/recipes/tests/test-checkpoint-scope.py",
    ),
    (
        "workflow-finalize::step-20b-push-cleanup",
        "amplifier-bundle/recipes/tests/test-checkpoint-scope.py",
    ),
];

/// The Python files a validation run accounts for.
#[derive(Debug, Clone)]
pub struct PythonFiles<'a> {
    /// `git ls-files -z -- '*.py'` (see `tracked_python`).
    pub tracked: BTreeSet<String>,
    /// `PINNED_19` in production.
    pub pinned: &'a [&'a str],
    /// `HELPER_USERS` in production.
    pub helper_users: &'a [(&'a str, &'a [&'a str])],
}

/// The tracked `.py` files, read with `git ls-files -z` through
/// `amplihack_git::command_in`. A failed listing or a non-UTF-8 path panics.
pub fn tracked_python(root: &Path) -> BTreeSet<String> {
    let out = amplihack_git::command_in(root)
        .args(["ls-files", "-z", "--", "*.py"])
        .output()
        .expect("run git ls-files");
    assert!(
        out.status.success(),
        "git ls-files failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| {
            String::from_utf8(p.to_vec())
                .unwrap_or_else(|e| panic!("a tracked .py path is not UTF-8: {e}"))
        })
        .collect()
}

/// The file part of a Python or procedural reference ID: the text before the
/// first `::`, which must be a `.py` path of `[A-Za-z0-9_./-]` characters,
/// followed by a non-empty remainder. Anything else does not parse.
pub fn ref_file(ref_id: &str) -> Option<&str> {
    let (file, rest) = ref_id.split_once("::")?;
    let stem = file.strip_suffix(".py")?;
    let allowed = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | '-');
    (!stem.is_empty() && file.chars().all(allowed) && !rest.is_empty()).then_some(file)
}

/// A repository-relative path as `git ls-files` prints it: not empty, not
/// absolute, no `.` or `..` segment, no empty segment, no backslash or NUL.
/// It is compared byte for byte and never canonicalised.
fn is_normalised_repo_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains(['\\', '\0'])
        && path
            .split('/')
            .all(|seg| !seg.is_empty() && seg != "." && seg != "..")
}

/// The one rule every guard shares for which `.py` files may be tracked: the
/// `pending_file` of every pending row that is not a helper, plus each
/// helper that a file already in the set uses, repeated to a fixed point.
pub fn expected_tracked_python(
    map: &toml::Value,
    helper_users: &[(&str, &[&str])],
) -> BTreeSet<String> {
    let is_helper = |f: &str| helper_users.iter().any(|(h, _)| *h == f);
    let mut expected: BTreeSet<String> = KINDS
        .iter()
        .flat_map(|(table, _)| {
            map.get(*table)
                .and_then(|r| r.as_array())
                .into_iter()
                .flatten()
        })
        .filter_map(|row| row.get("pending_file").and_then(|f| f.as_str()))
        .filter(|f| !is_helper(f))
        .map(str::to_owned)
        .collect();
    loop {
        let before = expected.len();
        for (helper, users) in helper_users {
            if users.iter().any(|u| expected.contains(*u)) {
                expected.insert((*helper).to_owned());
            }
        }
        if expected.len() == before {
            return expected;
        }
    }
}

pub fn validate(
    ids: &Ids,
    map: &toml::Value,
    index: &RustTestIndex,
    corpus: &Corpus,
    approved: &[ApprovedRow],
    doc: &str,
    py: &PythonFiles<'_>,
) -> Result<(), Vec<Violation>> {
    let mut v = Vec::new();
    let control_exists = index.contains_key(REMOVAL_CONTROL);
    let meta = map.get("meta").and_then(|m| m.as_table());
    if meta
        .and_then(|m| m.get("inventory_source_sha256"))
        .and_then(|s| s.as_str())
        != Some(ids.source_sha256.as_str())
    {
        v.push(Violation::WrongSha("meta.inventory_source_sha256".into()));
    }
    // Dispositions first: each must name a doc section that exists.
    let mut dispositions = BTreeSet::new();
    for d in map
        .get("disposition")
        .and_then(|d| d.as_array())
        .into_iter()
        .flatten()
    {
        let id = d
            .get("id")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_owned();
        let section = d.get("doc_section").and_then(|s| s.as_str()).unwrap_or("");
        let heading_present = doc
            .lines()
            .any(|l| l.trim_start_matches('#').trim() == section);
        if !DOC_SECTIONS.contains(&section) || !heading_present {
            v.push(Violation::MissingDocSection(format!("{id}: {section}")));
        }
        let decision = d.get("decision").and_then(|s| s.as_str()).unwrap_or("");
        if approved_row(approved, &id, decision).is_none() {
            v.push(Violation::UnrecordedDisposition(format!(
                "{id}: {decision}"
            )));
        }
        let pin = d.get("pinning_test").and_then(|s| s.as_str()).unwrap_or("");
        match index.get(pin) {
            None => v.push(Violation::NoSuchTest(format!("{id}: {pin}"))),
            Some(f) if f.ignored && id != "needs-native-runner" => {
                v.push(Violation::IgnoredTarget(format!("{id}: {pin}")))
            }
            _ => {}
        }
        dispositions.insert(id);
    }
    // Each approved row: its covers exist, and its pin_test resolves to
    // exactly one runnable test that no other row names.
    let mut pins = BTreeSet::new();
    for row in approved {
        for cover in row.covers {
            let known = ids.python_tests.contains(*cover)
                || ids.procedural.contains(*cover)
                || ids.roles.contains_key(*cover)
                || ids.sites.contains(*cover);
            if !known {
                v.push(Violation::UnknownCover(format!("{}: {cover}", row.id)));
            }
        }
        if !pins.insert(row.pin_test) {
            v.push(Violation::SharedPinTest(format!(
                "{}: {}",
                row.id, row.pin_test
            )));
        }
        match resolve_pin(row.pin_test, index) {
            Err(hits) if hits.is_empty() => v.push(Violation::NoSuchTest(format!(
                "{}: {}",
                row.id, row.pin_test
            ))),
            Err(hits) => v.push(Violation::AmbiguousPinTest(format!(
                "{}: {} matches {hits:?}",
                row.id, row.pin_test
            ))),
            Ok(key) => match &index[&key] {
                f if f.ignored => v.push(Violation::IgnoredTarget(row.id.into())),
                f if f.cfg_gated => v.push(Violation::CfgGated(row.id.into())),
                f if f.env_gated_early_return => {
                    v.push(Violation::EnvGatedEarlyReturn(row.id.into()))
                }
                _ => {}
            },
        }
    }
    for (table, _) in KINDS {
        let expected: BTreeSet<String> = match table {
            "python_test" => ids.python_tests.clone(),
            "procedural" => ids.procedural.clone(),
            "role" => ids.roles.keys().cloned().collect(),
            _ => ids.sites.clone(),
        };
        let mut seen = BTreeSet::new();
        for row in map
            .get(table)
            .and_then(|r| r.as_array())
            .into_iter()
            .flatten()
        {
            let id = row
                .get("ref_id")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_owned();
            if !seen.insert(id.clone()) {
                v.push(Violation::Duplicate(id.clone()));
            }
            if !expected.contains(&id) {
                v.push(Violation::Invented(id.clone()));
            }
            let test = row.get("rust_test").and_then(|s| s.as_str());
            let disp = row.get("disposition").and_then(|s| s.as_str());
            let pending = row.get("pending_file").and_then(|s| s.as_str());
            let columns = [
                row.get("rust_test").is_some(),
                row.get("disposition").is_some(),
                row.get("pending_file").is_some(),
            ]
            .into_iter()
            .filter(|present| *present)
            .count();
            if columns > 1 {
                v.push(Violation::MultipleColumns(id.clone()));
                continue;
            }
            if let Some(file) = pending {
                // 17.146-2: the row's behaviour is still in a tracked .py
                // file. A pending file is never a target or a disposition.
                if !is_normalised_repo_path(file) || !py.tracked.contains(file) {
                    v.push(Violation::PendingFileNotTracked(format!("{id}: {file:?}")));
                }
                let own = match table {
                    "role" => Some(ROLE_PENDING_FILE),
                    "site" => SITE_PENDING_FILES
                        .iter()
                        .find(|(site, _)| *site == id)
                        .map(|(_, f)| *f),
                    _ => ref_file(&id),
                };
                if own != Some(file) {
                    v.push(Violation::PendingFileMismatch(format!(
                        "{id}: pending with {file:?}, its own file is {own:?}"
                    )));
                }
                if table == "role" && control_exists {
                    v.push(Violation::PendingRoleAfterControl(id.clone()));
                }
                continue;
            }
            match (test, disp) {
                (Some(_), Some(_)) => v.push(Violation::MultipleColumns(id.clone())),
                (None, None) => v.push(Violation::NeitherColumn(id.clone())),
                (None, Some(d)) => {
                    if !dispositions.contains(d) {
                        v.push(Violation::UnknownDisposition(format!("{id}: {d}")));
                    } else if !approved
                        .iter()
                        .any(|r| r.id == d && r.covers.contains(&id.as_str()))
                    {
                        // A real decision row is not enough: it must be
                        // recorded as resolving this very obligation.
                        v.push(Violation::UnrecordedDisposition(format!(
                            "{id}: {d} does not cover this reference"
                        )));
                    }
                }
                (Some(t), None) => {
                    // Corpus-case form: `<path>::corpus_<action>#<case>`. The
                    // case must exist in that action's corpus file and name
                    // this row in its `inventory_ref`; the runner test then
                    // runs every case, so a resolved case is a case that runs.
                    let key = t.split('#').next().unwrap_or(t);
                    if let Some((_, case)) = t.split_once('#') {
                        let action = key
                            .rsplit("::")
                            .next()
                            .and_then(|f| f.strip_prefix("corpus_"))
                            .unwrap_or("")
                            .replace('_', "-");
                        match corpus.get(&action).and_then(|cases| cases.get(case)) {
                            None => v.push(Violation::NoSuchCorpusCase(format!("{id}: {t}"))),
                            Some(reference) if reference.as_deref() != Some(id.as_str()) => {
                                v.push(Violation::CorpusRefMismatch(format!(
                                    "{id}: {t} has {reference:?}"
                                )))
                            }
                            Some(_) => {}
                        }
                    }
                    match index.get(key) {
                        None => v.push(Violation::NoSuchTest(format!("{id}: {t}"))),
                        Some(f) if f.ignored => v.push(Violation::IgnoredTarget(id.clone())),
                        Some(f) if f.cfg_gated => v.push(Violation::CfgGated(id.clone())),
                        Some(f) if f.env_gated_early_return => {
                            v.push(Violation::EnvGatedEarlyReturn(id.clone()))
                        }
                        _ => {}
                    }
                }
            }
            if table == "python_test" {
                let py = row.get("assertions_python").and_then(|n| n.as_integer());
                let rs = row.get("assertions_rust").and_then(|n| n.as_integer());
                if test.is_some() && !matches!((py, rs), (Some(p), Some(r)) if r >= p) {
                    v.push(Violation::AssertionCount(id.clone()));
                }
            }
        }
        for id in expected.difference(&seen) {
            v.push(Violation::Missing(id.clone()));
        }
    }
    // 17.146-2: the tracked .py files are exactly the expected set, and the
    // expected set stays inside the files pinned when the map was created.
    let expected = expected_tracked_python(map, py.helper_users);
    for file in py.tracked.difference(&expected) {
        v.push(Violation::UnaccountedPython(format!(
            "{file}: tracked, but no row is pending with it and no pending file uses it"
        )));
    }
    for file in &expected {
        if !py.pinned.contains(&file.as_str()) {
            v.push(Violation::UnaccountedPython(format!(
                "{file}: expected, but not one of the pinned files"
            )));
        }
        let helper = py.helper_users.iter().any(|(h, _)| *h == file);
        if helper && !py.tracked.contains(file) {
            v.push(Violation::UnaccountedPython(format!(
                "{file}: a pending file still uses this helper, but it is not tracked"
            )));
        }
    }
    // Provenance and audit receipts never stand in for coverage.
    for table in ["provenance", "audit_receipt"] {
        for row in map
            .get(table)
            .and_then(|r| r.as_array())
            .into_iter()
            .flatten()
        {
            if row.get("ref_id").is_some() {
                let id = row["ref_id"].as_str().unwrap_or("").to_owned();
                v.push(Violation::ProvenanceCounted(id));
            }
        }
    }
    let authorised = ids.roles.values().filter(|a| **a).count();
    if ids.roles.len() != 41 || authorised != 40 {
        v.push(Violation::Cardinality(format!(
            "roles {} (authorised {authorised})",
            ids.roles.len()
        )));
    }
    if ids.python_tests.len() != 190 || ids.sites.len() != 7 {
        v.push(Violation::Cardinality(format!(
            "python_tests {} sites {}",
            ids.python_tests.len(),
            ids.sites.len()
        )));
    }
    if v.is_empty() { Ok(()) } else { Err(v) }
}

fn workspace_test_sources() -> Vec<(String, String)> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            let name = e.file_name();
            if name == "target" || name == ".git" || name == "worktrees" || name == "node_modules" {
                continue;
            }
            if p.is_dir() {
                walk(base, &p, out);
            } else if p.extension().is_some_and(|x| x == "rs")
                && let Ok(src) = fs::read_to_string(&p)
            {
                let rel = p.strip_prefix(base).unwrap().to_string_lossy().into_owned();
                out.push((rel, src));
            }
        }
    }
    let base = root();
    let mut out = Vec::new();
    for top in ["crates", "bins", "tests"] {
        walk(&base, &base.join(top), &mut out);
    }
    out
}

// ------------------------------------------------------------ the real map

#[test]
fn inventory_ids_json_is_derived_from_cc_and_has_the_declared_counts() {
    let text = fs::read_to_string(root().join(DIR).join("inventory_ids.json"))
        .expect("commit the derived inventory_ids.json");
    let ids = load_ids(&text);
    assert_eq!(ids.source_sha256, CC_SHA256);
    assert_eq!(ids.python_tests.len(), 190);
    assert_eq!(ids.sites.len(), 7);
    assert_eq!(ids.roles.len(), 41);
    assert_eq!(ids.roles.values().filter(|a| **a).count(), 40);
    assert!(
        ids.roles
            .iter()
            .any(|(k, a)| !a && k == "workflow-terminal-adjudication::terminal-state-adjudicator"),
        "the 41st row is cc's terminal-state-adjudicator, mapped but not original_authorised"
    );
    assert_eq!(
        ids.procedural.len(),
        207,
        "cc's support_function assertions"
    );
    assert!(ids.procedural.iter().all(|p| p.contains("@L")));
}

/// The pinned copy of cc's inventory, reassembled. Panics (fails the test)
/// when a part is missing; there is no skip.
fn pinned_inventory() -> Vec<u8> {
    let mut raw = Vec::new();
    for part in PINNED_INVENTORY_PARTS {
        let path = root().join(DIR).join(part);
        raw.extend(fs::read(&path).unwrap_or_else(|e| {
            panic!("pinned inventory {} must be committed: {e}", path.display())
        }));
    }
    raw
}

#[test]
fn pinned_inventory_parts_reassemble_to_cc_s_sha256() {
    assert_eq!(
        sha256(&pinned_inventory()),
        CC_SHA256,
        "pinned inventory changed"
    );
    for part in PINNED_INVENTORY_PARTS {
        let bytes = fs::read(root().join(DIR).join(part)).unwrap();
        assert!(
            bytes.len() < 500 * 1024,
            "{part} exceeds check-added-large-files"
        );
        assert_eq!(bytes.last(), Some(&b'\n'), "{part} ends at a line boundary");
    }
}

#[test]
fn inventory_ids_json_matches_the_pinned_cc_inventory() {
    let ids = load_ids(
        &fs::read_to_string(root().join(DIR).join("inventory_ids.json"))
            .expect("commit the derived inventory_ids.json"),
    );
    assert_eq!(ids.source_sha256, CC_SHA256);
    let raw = pinned_inventory();
    assert_eq!(sha256(&raw), CC_SHA256, "cc's inventory changed");
    let cc: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let tests: BTreeSet<String> = cc["python_assets"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|a| a["tests"].as_array().unwrap().iter())
        .map(|t| t["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(ids.python_tests, tests);
    let procedural: BTreeSet<String> = cc["python_assets"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|a| a["support_functions"].as_array().unwrap().iter())
        .flat_map(|f| {
            let id = f["id"].as_str().unwrap().to_owned();
            f["assertions"]
                .as_array()
                .unwrap()
                .iter()
                .map(move |s| format!("{id}@L{}", s["line"]))
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(ids.procedural, procedural);
    let sites: BTreeSet<String> = cc["checkpoint_sites"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            format!(
                "{}::{}",
                s["asset"].as_str().unwrap(),
                s["step_id"].as_str().unwrap()
            )
        })
        .collect();
    assert_eq!(ids.sites, sites);
}

#[test]
fn inventory_map_is_complete_and_every_row_resolves() {
    let ids = load_ids(
        &fs::read_to_string(root().join(DIR).join("inventory_ids.json"))
            .expect("commit the derived inventory_ids.json"),
    );
    let map: toml::Value = toml::from_str(
        &fs::read_to_string(root().join(DIR).join("inventory_map.toml"))
            .expect("commit inventory_map.toml"),
    )
    .expect("inventory_map.toml parses");
    let doc = fs::read_to_string(root().join(DOC)).unwrap();
    let index = scan(&workspace_test_sources());
    let corpus = load_corpus(&root().join(DIR).join("corpus"));
    let py = PythonFiles {
        tracked: tracked_python(&root()),
        pinned: &PINNED_19,
        helper_users: &HELPER_USERS,
    };
    if let Err(violations) = validate(&ids, &map, &index, &corpus, &APPROVED_ROWS, &doc, &py) {
        let mut shown: Vec<String> = violations
            .iter()
            .take(40)
            .map(|v| format!("{v:?}"))
            .collect();
        shown.push(format!("({} violations)", violations.len()));
        panic!("{}", shown.join("\n"));
    }
}

#[test]
fn inventory_map_does_not_use_the_invented_217_or_27_counts() {
    let map: toml::Value = toml::from_str(
        &fs::read_to_string(root().join(DIR).join("inventory_map.toml"))
            .expect("commit inventory_map.toml"),
    )
    .unwrap();
    let meta = map.get("meta").and_then(|m| m.as_table()).expect("[meta]");
    for (key, value) in meta {
        assert!(
            !matches!(value.as_integer(), Some(217 | 27)),
            "invented count in meta.{key}"
        );
    }
}

// ---------------------------------------- validator meta-tests (synthetic)

/// The synthetic map's allowlist: the production row shape, with one
/// covered obligation so the disposition path is exercised.
const TINY_APPROVED: &[ApprovedRow] = &[ApprovedRow {
    id: "17.138-6",
    decision: "17.138",
    covers: &["a.py::f@L1"],
    pin_test: "t::ok",
    inventory_search: "synthetic",
}];

fn tiny() -> (Ids, toml::Value, RustTestIndex, Corpus, String) {
    let mut ids = Ids {
        source_sha256: "abc".into(),
        ..Ids::default()
    };
    for i in 0..190 {
        ids.python_tests.insert(format!("a.py::C.test_{i}"));
    }
    ids.procedural.insert("a.py::f@L1".into());
    for i in 0..41 {
        ids.roles.insert(format!("asset::step-{i}"), i != 40);
    }
    // The real site IDs, so the fixed site table of 17.146-2 applies.
    for (site, _) in SITE_PENDING_FILES {
        ids.sites.insert(site.to_owned());
    }
    let mut map = String::from("[meta]\ninventory_source_sha256 = \"abc\"\n");
    for id in &ids.python_tests {
        map += &format!(
            "[[python_test]]\nref_id = \"{id}\"\nrust_test = \"t.rs::ok\"\nassertions_python = 2\nassertions_rust = 2\n"
        );
    }
    map += "[[procedural]]\nref_id = \"a.py::f@L1\"\ndisposition = \"17.138-6\"\n";
    for id in ids.roles.keys() {
        map += &format!("[[role]]\nref_id = \"{id}\"\nrust_test = \"t.rs::ok\"\n");
    }
    for (n, id) in ids.sites.iter().enumerate() {
        map += &format!("[[site]]\nref_id = \"{id}\"\nrust_test = \"t.rs::corpus_x#case{n}\"\n");
    }
    map += "[[disposition]]\nid = \"17.138-6\"\ndecision = \"17.138\"\ndoc_section = \"Decision 17.138\"\npinning_test = \"t.rs::ok\"\n";
    map += "[[provenance]]\nid = \"p1\"\nsource = \"cc\"\nevidence = \"x\"\n";
    let index = scan(&[(
        "t.rs".into(),
        "#[test]\nfn ok() {}\n\n#[test]\n#[ignore = \"x\"]\nfn ign() {}\n\n#[cfg(windows)]\n#[test]\nfn gated() {}\n\n#[test]\nfn env_gated() {\n    if std::env::var(\"X\").is_err() {\n        return;\n    }\n}\n\n#[test]\nfn corpus_x() {}\n".into(),
    )]);
    let mut corpus = Corpus::new();
    let cases = corpus.entry("x".into()).or_default();
    for (n, id) in ids.sites.iter().enumerate() {
        cases.insert(format!("case{n}"), Some(id.clone()));
    }
    let doc = DOC_SECTIONS.iter().map(|s| format!("## {s}\n")).collect();
    (ids, toml::from_str(&map).unwrap(), index, corpus, doc)
}

fn mutate(map: &mut toml::Value, table: &str, f: impl FnOnce(&mut toml::value::Table)) {
    let row = map[table].as_array_mut().unwrap()[0]
        .as_table_mut()
        .unwrap();
    f(row);
}

fn expect(v: Result<(), Vec<Violation>>, want: impl Fn(&Violation) -> bool, what: &str) {
    let errs = v.expect_err(what);
    assert!(errs.iter().any(want), "{what}: {errs:?}");
}

#[test]
fn validator_accepts_the_synthetic_complete_map() {
    let (ids, map, index, corpus, doc) = tiny();
    assert_eq!(
        validate(
            &ids,
            &map,
            &index,
            &corpus,
            TINY_APPROVED,
            &doc,
            &no_python()
        ),
        Ok(())
    );
}

#[test]
fn validator_rejects_each_negative_case_with_a_named_violation() {
    type Case = (
        &'static str,
        fn(&mut Ids, &mut toml::Value, &mut Corpus, &mut String),
        fn(&Violation) -> bool,
    );
    let cases: Vec<Case> = vec![
        (
            "missing id",
            |_, m, _, _| {
                m["python_test"].as_array_mut().unwrap().remove(0);
            },
            |v| matches!(v, Violation::Missing(_)),
        ),
        (
            "duplicate id",
            |_, m, _, _| {
                let first = m["role"].as_array().unwrap()[0].clone();
                m["role"].as_array_mut().unwrap().push(first);
            },
            |v| matches!(v, Violation::Duplicate(_)),
        ),
        (
            "invented id",
            |_, m, _, _| {
                mutate(m, "site", |r| {
                    r.insert("ref_id".into(), "asset::invented".into());
                });
            },
            |v| matches!(v, Violation::Invented(_)),
        ),
        (
            "both columns",
            |_, m, _, _| {
                mutate(m, "role", |r| {
                    r.insert("disposition".into(), "17.138-6".into());
                });
            },
            |v| matches!(v, Violation::MultipleColumns(_)),
        ),
        (
            "neither column",
            |_, m, _, _| {
                mutate(m, "role", |r| {
                    r.remove("rust_test");
                });
            },
            |v| matches!(v, Violation::NeitherColumn(_)),
        ),
        (
            "ignored target",
            |_, m, _, _| {
                mutate(m, "role", |r| {
                    r.insert("rust_test".into(), "t.rs::ign".into());
                });
            },
            |v| matches!(v, Violation::IgnoredTarget(_)),
        ),
        (
            "nonexistent target",
            |_, m, _, _| {
                mutate(m, "role", |r| {
                    r.insert("rust_test".into(), "t.rs::nope".into());
                });
            },
            |v| matches!(v, Violation::NoSuchTest(_)),
        ),
        (
            "cfg-gated target",
            |_, m, _, _| {
                mutate(m, "role", |r| {
                    r.insert("rust_test".into(), "t.rs::gated".into());
                });
            },
            |v| matches!(v, Violation::CfgGated(_)),
        ),
        (
            "env-gated early return",
            |_, m, _, _| {
                mutate(m, "role", |r| {
                    r.insert("rust_test".into(), "t.rs::env_gated".into());
                });
            },
            |v| matches!(v, Violation::EnvGatedEarlyReturn(_)),
        ),
        (
            "missing doc section",
            |_, _, _, d| {
                *d = d.replace("## Decision 17.138\n", "");
            },
            |v| matches!(v, Violation::MissingDocSection(_)),
        ),
        (
            "wrong sha",
            |_, m, _, _| {
                m["meta"]
                    .as_table_mut()
                    .unwrap()
                    .insert("inventory_source_sha256".into(), "zzz".into());
            },
            |v| matches!(v, Violation::WrongSha(_)),
        ),
        (
            "provenance counted as coverage",
            |_, m, _, _| {
                mutate(m, "provenance", |r| {
                    r.insert("ref_id".into(), "a.py::C.test_0".into());
                });
            },
            |v| matches!(v, Violation::ProvenanceCounted(_)),
        ),
        (
            "lossy port (fewer assertions)",
            |_, m, _, _| {
                mutate(m, "python_test", |r| {
                    r.insert("assertions_rust".into(), toml::Value::Integer(1));
                });
            },
            |v| matches!(v, Violation::AssertionCount(_)),
        ),
        (
            "41st role demoted",
            |i, _, _, _| {
                i.roles.insert("asset::step-40".into(), true);
            },
            |v| matches!(v, Violation::Cardinality(_)),
        ),
        (
            "unknown disposition",
            |_, m, _, _| {
                mutate(m, "procedural", |r| {
                    r.insert("disposition".into(), "17.999".into());
                });
            },
            |v| matches!(v, Violation::UnknownDisposition(_)),
        ),
        (
            "corpus case does not exist",
            |_, _, c, _| {
                c.get_mut("x").unwrap().remove("case0");
            },
            |v| matches!(v, Violation::NoSuchCorpusCase(_)),
        ),
        (
            "corpus case names another reference",
            |_, _, c, _| {
                c.get_mut("x")
                    .unwrap()
                    .insert("case0".into(), Some("asset::other".into()));
            },
            |v| matches!(v, Violation::CorpusRefMismatch(_)),
        ),
        (
            "unresolved disposition standing in for coverage (cc probe)",
            |_, m, _, d| {
                m["disposition"]
                    .as_array_mut()
                    .unwrap()
                    .push(toml::Value::Table(
                        toml::from_str(
                            "id = \"unresolved-999\"\ndecision = \"pending, no approval\"\n\
                             doc_section = \"Unresolved differences for owner disposition\"\n\
                             pinning_test = \"t.rs::ok\"\n",
                        )
                        .unwrap(),
                    ));
                mutate(m, "python_test", |r| {
                    r.remove("rust_test");
                    r.insert("disposition".into(), "unresolved-999".into());
                });
                d.push_str("");
            },
            |v| matches!(v, Violation::UnrecordedDisposition(_)),
        ),
        (
            "unresolved id even with a recorded decision column",
            |_, m, _, _| {
                m["disposition"].as_array_mut().unwrap()[0]
                    .as_table_mut()
                    .unwrap()
                    .insert("id".into(), "unresolved-1".into());
                mutate(m, "procedural", |r| {
                    r.insert("disposition".into(), "unresolved-1".into());
                });
            },
            |v| matches!(v, Violation::UnrecordedDisposition(_)),
        ),
        (
            "fabricated row under a real decision number (cc 03:46Z)",
            |_, m, _, _| {
                m["disposition"]
                    .as_array_mut()
                    .unwrap()
                    .push(toml::Value::Table(
                        toml::from_str(
                            "id = \"17.141-cc-invented-coverage\"\ndecision = \"17.141\"\n\
                             doc_section = \"Decision 17.138\"\npinning_test = \"t.rs::ok\"\n",
                        )
                        .unwrap(),
                    ));
                mutate(m, "python_test", |r| {
                    r.remove("rust_test");
                    r.insert("disposition".into(), "17.141-cc-invented-coverage".into());
                });
            },
            |v| matches!(v, Violation::UnrecordedDisposition(_)),
        ),
        (
            "approved row used for an obligation it does not cover",
            |_, m, _, _| {
                mutate(m, "python_test", |r| {
                    r.remove("rust_test");
                    r.insert("disposition".into(), "17.138-6".into());
                });
            },
            |v| matches!(v, Violation::UnrecordedDisposition(_)),
        ),
        (
            "restoration 17.138-5 is not an exception",
            |_, m, _, _| {
                m["disposition"].as_array_mut().unwrap()[0]
                    .as_table_mut()
                    .unwrap()
                    .insert("id".into(), "17.138-5".into());
                mutate(m, "procedural", |r| {
                    r.insert("disposition".into(), "17.138-5".into());
                });
            },
            |v| matches!(v, Violation::UnrecordedDisposition(_)),
        ),
        (
            "invented pending disposition",
            |_, m, _, _| {
                m["disposition"].as_array_mut().unwrap()[0]
                    .as_table_mut()
                    .unwrap()
                    .insert("id".into(), "17.138-pending".into());
                mutate(m, "procedural", |r| {
                    r.insert("disposition".into(), "17.138-pending".into());
                });
            },
            |v| matches!(v, Violation::UnrecordedDisposition(_)),
        ),
    ];
    for (what, apply, want) in cases {
        let (mut ids, mut map, index, mut corpus, mut doc) = tiny();
        apply(&mut ids, &mut map, &mut corpus, &mut doc);
        expect(
            validate(
                &ids,
                &map,
                &index,
                &corpus,
                TINY_APPROVED,
                &doc,
                &no_python(),
            ),
            want,
            what,
        );
    }
}

#[test]
fn validator_is_mutant_sensitive_to_one_renamed_reference_id() {
    let (mut ids, map, index, corpus, doc) = tiny();
    let first = ids.python_tests.iter().next().unwrap().clone();
    ids.python_tests.remove(&first);
    ids.python_tests.insert(format!("{first}_renamed"));
    let errs = validate(
        &ids,
        &map,
        &index,
        &corpus,
        TINY_APPROVED,
        &doc,
        &no_python(),
    )
    .unwrap_err();
    assert!(errs.contains(&Violation::Missing(format!("{first}_renamed"))));
    assert!(errs.contains(&Violation::Invented(first)));
}

#[test]
fn the_allowlist_is_exactly_the_eight_rows_bb_confirmed() {
    let ids: Vec<(&str, &str)> = APPROVED_ROWS.iter().map(|r| (r.id, r.decision)).collect();
    assert_eq!(
        ids,
        [
            ("17.138-1", "17.138"),
            ("17.138-2", "17.138"),
            ("17.138-3", "17.138"),
            ("17.138-4", "17.138"),
            ("17.138-6", "17.138"),
            ("17.126-dest-read-79-site-1", "17.126"),
            ("17.126-dest-read-79-site-2", "17.126"),
            ("17.126-stage-strict-79", "17.126"),
        ]
    );
    // A covered obligation must be a real inventory ID. The committed IDs
    // are read only when some row covers one.
    let inventory = APPROVED_ROWS.iter().any(|r| !r.covers.is_empty()).then(|| {
        load_ids(
            &fs::read_to_string(root().join(DIR).join("inventory_ids.json"))
                .expect("commit the derived inventory_ids.json"),
        )
    });
    let index = scan(&workspace_test_sources());
    let mut pins = BTreeSet::new();
    for row in APPROVED_ROWS {
        for cover in row.covers {
            let ids = inventory.as_ref().expect("loaded when a row covers an ID");
            assert!(
                ids.python_tests.contains(*cover)
                    || ids.procedural.contains(*cover)
                    || ids.roles.contains_key(*cover)
                    || ids.sites.contains(*cover),
                "{}: covers unknown ID {cover}",
                row.id
            );
        }
        assert!(
            !row.inventory_search.is_empty(),
            "{}: no search recorded",
            row.id
        );
        assert!(pins.insert(row.pin_test), "{}: pin_test shared", row.id);
        // The pin_test is exactly one #[test] that runs normally: not
        // ignored, not cfg-gated off on Linux, not skipped by an env check.
        let key = resolve_pin(row.pin_test, &index)
            .unwrap_or_else(|hits| panic!("{}: {} resolves to {hits:?}", row.id, row.pin_test));
        let facts = &index[&key];
        assert!(
            !facts.ignored && !facts.cfg_gated && !facts.env_gated_early_return,
            "{}: pin_test {key} is not a runnable #[test]",
            row.id
        );
    }
}

#[test]
fn pin_test_resolution_is_exact_and_unique() {
    let index = scan(&[
        (
            "crates/amplihack-cli/src/a/b.rs".into(),
            "#[test]\nfn f() {}\n".into(),
        ),
        (
            "crates/amplihack-cli/src/a/c.rs".into(),
            "#[test]\nfn f() {}\n".into(),
        ),
        ("bins/x/tests/m.rs".into(), "#[test]\nfn g() {}\n".into()),
        ("crates/y/tests/m.rs".into(), "#[test]\nfn g() {}\n".into()),
        (
            "bins/x/tests/n.rs".into(),
            "#[test]\nfn g() {}\n#[test]\nfn gg() {}\n".into(),
        ),
    ]);
    assert_eq!(
        resolve_pin("amplihack_cli::a::b::f", &index),
        Ok("crates/amplihack-cli/src/a/b.rs::f".into())
    );
    assert_eq!(
        resolve_pin("n::gg", &index),
        Ok("bins/x/tests/n.rs::gg".into())
    );
    // Wrong module: `f` exists, but not in a::d.
    assert_eq!(resolve_pin("amplihack_cli::a::d::f", &index), Err(vec![]));
    // A name suffix is not a match.
    assert_eq!(
        resolve_pin("n::g", &index),
        Ok("bins/x/tests/n.rs::g".into())
    );
    assert_eq!(resolve_pin("n::ggg", &index), Err(vec![]));
    // Two test files with one stem: ambiguous.
    assert_eq!(resolve_pin("m::g", &index).unwrap_err().len(), 2);
}

#[test]
fn validator_checks_every_approved_row() {
    type Rows = &'static [ApprovedRow];
    type Case = (&'static str, Rows, fn(&Violation) -> bool);
    const WRONG_MODULE: Rows = &[ApprovedRow {
        pin_test: "u::ok",
        ..TINY_APPROVED[0]
    }];
    const SHARED: Rows = &[
        TINY_APPROVED[0],
        ApprovedRow {
            id: "17.138-1",
            covers: &[],
            ..TINY_APPROVED[0]
        },
    ];
    const UNKNOWN_COVER: Rows = &[ApprovedRow {
        covers: &["a.py::f@L1", "a.py::nope@L9"],
        ..TINY_APPROVED[0]
    }];
    const IGNORED_PIN: Rows = &[ApprovedRow {
        pin_test: "t::ign",
        ..TINY_APPROVED[0]
    }];
    let cases: [Case; 4] = [
        ("pin_test in the wrong module", WRONG_MODULE, |v| {
            matches!(v, Violation::NoSuchTest(_))
        }),
        ("two rows share a pin_test", SHARED, |v| {
            matches!(v, Violation::SharedPinTest(_))
        }),
        ("a cover that is not an inventory ID", UNKNOWN_COVER, |v| {
            matches!(v, Violation::UnknownCover(_))
        }),
        ("an ignored pin_test", IGNORED_PIN, |v| {
            matches!(v, Violation::IgnoredTarget(_))
        }),
    ];
    for (what, rows, want) in cases {
        let (ids, map, index, corpus, doc) = tiny();
        expect(
            validate(&ids, &map, &index, &corpus, rows, &doc, &no_python()),
            want,
            what,
        );
    }
}

// ============================================================================
// Decision 17.146-2: a partial map whose unported rows are pending (C-s).
//
// Contract for the implementation (TDD; these fail until it exists):
//
// * `validate(ids, map, index, corpus, approved, doc, &PythonFiles)`.
// * `PythonFiles { tracked, pinned, helper_users }`: `tracked` is the `.py`
//   set from `git ls-files -z` (read by `tracked_python(root)` through
//   `amplihack_git::command_in`); `pinned` is `PINNED_19` in production;
//   `helper_users` is `HELPER_USERS`.
// * A coverage row holds exactly one of `rust_test`, `pending_file` and
//   `disposition` (`MultipleColumns`, `NeitherColumn`).
// * `pending_file` is a normalised repository path that is tracked
//   (`PendingFileNotTracked`) and is the row's own file
//   (`PendingFileMismatch`): `ref_file(ref_id)` for Python and procedural
//   rows, `ROLE_PENDING_FILE` for roles, `SITE_PENDING_FILES` for sites.
// * A role row may not be pending once `REMOVAL_CONTROL` is in the index
//   (`PendingRoleAfterControl`).
// * Every tracked `.py` file is in `expected_tracked_python(map,
//   helper_users)`, and that set is inside `pinned` (`UnaccountedPython`).
// * `pending_file` is never a disposition.
// ============================================================================

const ROLE_FILE_LITERAL: &str = "amplifier-bundle/recipes/tests/test-pragmatic-checkpoints.py";
const MINIMALIST: &str = "amplifier-bundle/recipes/tests/test-minimalist-auto-stage.py";
const SCOPE: &str = "amplifier-bundle/recipes/tests/test-checkpoint-scope.py";
const HARNESS: &str = "amplifier-bundle/recipes/tests/test-canonical-staging-harness.py";
const PUBLICATION: &str = "amplifier-bundle/recipes/tests/canonical_staging_publication.py";
const AUDIT: &str = "amplifier-bundle/recipes/tests/canonical_staging_checkpoint_audit.py";
const RECEIPTS: &str =
    "crates/amplihack-workflows/tests/fixtures/canonical_staging/audit_receipts.py";
const ADAPTER: &str =
    "crates/amplihack-workflows/tests/fixtures/canonical_staging/private_adapter.py";

/// The port order of decision 17.146 (smallest first), as file names under
/// `amplifier-bundle/recipes/tests/`.
const PORT_ORDER: [&str; 14] = [
    "test-checkpoint-conservative-boundaries.py",
    "test-pragmatic-crusty-directories.py",
    "test-checkpoint-rebase-identity.py",
    "test-checkpoint-value-transport.py",
    "test-checkpoint-scope-history.py",
    "test-checkpoint-refusal-evidence.py",
    "test-checkpoint-synthesis-alias.py",
    "test-checkpoint-review-boundaries.py",
    "test-pragmatic-checkpoints.py",
    "test-pragmatic-auto-stage.py",
    "test-pragmatic-evidence.py",
    "test-minimalist-auto-stage.py",
    "test-checkpoint-publication-boundaries.py",
    "test-checkpoint-scope.py",
];

/// The synthetic pinned set: the synthetic test file, a spare test file, a
/// synthetic helper, and the real fixed files that role and site rows use.
const SYN_PINNED: &[&str] = &["a.py", "b.py", "h.py", ROLE_FILE_LITERAL, MINIMALIST, SCOPE];
const SYN_HELPERS: &[(&str, &[&str])] = &[("h.py", &["a.py"])];

fn no_python() -> PythonFiles<'static> {
    PythonFiles {
        tracked: BTreeSet::new(),
        pinned: SYN_PINNED,
        helper_users: SYN_HELPERS,
    }
}

fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

/// `tiny()` with every coverage row pending with its own file, and the
/// tracked set that this map accounts for.
fn tiny_pending() -> (
    Ids,
    toml::Value,
    RustTestIndex,
    Corpus,
    String,
    PythonFiles<'static>,
) {
    let (ids, mut map, index, corpus, doc) = tiny();
    for table in ["python_test", "procedural", "role", "site"] {
        for row in map[table].as_array_mut().unwrap() {
            let row = row.as_table_mut().unwrap();
            let id = row["ref_id"].as_str().unwrap().to_owned();
            for key in [
                "rust_test",
                "disposition",
                "assertions_python",
                "assertions_rust",
            ] {
                row.remove(key);
            }
            let file = match table {
                "role" => ROLE_PENDING_FILE.to_owned(),
                "site" => site_file(&id).to_owned(),
                _ => ref_file(&id).expect("synthetic ref parses").to_owned(),
            };
            row.insert("pending_file".into(), file.into());
        }
    }
    let py = PythonFiles {
        tracked: set(&["a.py", "h.py", ROLE_FILE_LITERAL, MINIMALIST, SCOPE]),
        pinned: SYN_PINNED,
        helper_users: SYN_HELPERS,
    };
    (ids, map, index, corpus, doc, py)
}

fn site_file(site: &str) -> &'static str {
    SITE_PENDING_FILES
        .iter()
        .find(|(s, _)| *s == site)
        .map(|(_, f)| *f)
        .unwrap_or_else(|| panic!("{site} is not in SITE_PENDING_FILES"))
}

fn row_mut<'a>(map: &'a mut toml::Value, table: &str, ref_id: &str) -> &'a mut toml::value::Table {
    map[table]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .map(|r| r.as_table_mut().unwrap())
        .find(|r| r["ref_id"].as_str() == Some(ref_id))
        .unwrap_or_else(|| panic!("no [[{table}]] row {ref_id}"))
}

fn push_row(map: &mut toml::Value, table: &str, row: &str) {
    map[table]
        .as_array_mut()
        .unwrap()
        .push(toml::Value::Table(toml::from_str(row).unwrap()));
}

#[test]
fn validator_accepts_a_partial_map_whose_rows_are_pending_with_tracked_files() {
    let (ids, map, index, corpus, doc, py) = tiny_pending();
    assert_eq!(
        validate(&ids, &map, &index, &corpus, TINY_APPROVED, &doc, &py),
        Ok(())
    );
}

#[test]
fn validator_accepts_a_map_with_both_mapped_and_pending_rows() {
    let (ids, mut map, index, corpus, doc, py) = tiny_pending();
    // Map one Python row: the rest stay pending, so a.py is still expected.
    let first = ids.python_tests.iter().next().unwrap().clone();
    let row = row_mut(&mut map, "python_test", &first);
    row.remove("pending_file");
    row.insert("rust_test".into(), "t.rs::ok".into());
    row.insert("assertions_python".into(), toml::Value::Integer(2));
    row.insert("assertions_rust".into(), toml::Value::Integer(2));
    assert_eq!(
        validate(&ids, &map, &index, &corpus, TINY_APPROVED, &doc, &py),
        Ok(())
    );
}

#[test]
fn validator_rejects_each_pending_row_defect_with_a_named_violation() {
    type Case = (
        &'static str,
        fn(&mut Ids, &mut toml::Value, &mut RustTestIndex, &mut PythonFiles<'static>),
        fn(&Violation) -> bool,
    );
    let cases: Vec<Case> = vec![
        (
            "pending_file and rust_test",
            |_, m, _, _| {
                mutate(m, "python_test", |r| {
                    r.insert("rust_test".into(), "t.rs::ok".into());
                });
            },
            |v| matches!(v, Violation::MultipleColumns(_)),
        ),
        (
            "pending_file and disposition",
            |_, m, _, _| {
                mutate(m, "procedural", |r| {
                    r.insert("disposition".into(), "17.138-6".into());
                });
            },
            |v| matches!(v, Violation::MultipleColumns(_)),
        ),
        (
            "no column at all",
            |_, m, _, _| {
                mutate(m, "role", |r| {
                    r.remove("pending_file");
                });
            },
            |v| matches!(v, Violation::NeitherColumn(_)),
        ),
        (
            "pending file is not tracked",
            |_, _, _, p| {
                p.tracked.remove("a.py");
            },
            |v| matches!(v, Violation::PendingFileNotTracked(_)),
        ),
        (
            "absolute pending path, even if some listing held it",
            |_, m, _, p| {
                p.tracked.insert("/a.py".into());
                mutate(m, "python_test", |r| {
                    r.insert("pending_file".into(), "/a.py".into());
                });
            },
            |v| matches!(v, Violation::PendingFileNotTracked(_)),
        ),
        (
            "a .. segment is refused, never canonicalised",
            |_, m, _, p| {
                p.tracked.insert("x/../a.py".into());
                mutate(m, "python_test", |r| {
                    r.insert("pending_file".into(), "x/../a.py".into());
                });
            },
            |v| matches!(v, Violation::PendingFileNotTracked(_)),
        ),
        (
            "a . segment is refused, never canonicalised",
            |_, m, _, p| {
                p.tracked.insert("./a.py".into());
                mutate(m, "python_test", |r| {
                    r.insert("pending_file".into(), "./a.py".into());
                });
            },
            |v| matches!(v, Violation::PendingFileNotTracked(_)),
        ),
        (
            "a backslash is refused",
            |_, m, _, p| {
                p.tracked.insert("x\\a.py".into());
                mutate(m, "python_test", |r| {
                    r.insert("pending_file".into(), "x\\a.py".into());
                });
            },
            |v| matches!(v, Violation::PendingFileNotTracked(_)),
        ),
        (
            "a Python row pending with another tracked file",
            |_, m, _, _| {
                mutate(m, "python_test", |r| {
                    r.insert("pending_file".into(), ROLE_FILE_LITERAL.into());
                });
            },
            |v| matches!(v, Violation::PendingFileMismatch(_)),
        ),
        (
            "a procedural row pending with another tracked file",
            |_, m, _, _| {
                mutate(m, "procedural", |r| {
                    r.insert("pending_file".into(), SCOPE.into());
                });
            },
            |v| matches!(v, Violation::PendingFileMismatch(_)),
        ),
        (
            "a role row pending with a file other than the fixed role file",
            |_, m, _, _| {
                mutate(m, "role", |r| {
                    r.insert("pending_file".into(), "a.py".into());
                });
            },
            |v| matches!(v, Violation::PendingFileMismatch(_)),
        ),
        (
            "a scope site pending with the minimalist file",
            |_, m, _, _| {
                let row = row_mut(m, "site", "workflow-publish::step-15-commit-push");
                row.insert("pending_file".into(), MINIMALIST.into());
            },
            |v| matches!(v, Violation::PendingFileMismatch(_)),
        ),
        (
            "a minimalist site pending with the scope file",
            |_, m, _, _| {
                let row = row_mut(m, "site", "workflow-tdd::checkpoint-after-implementation");
                row.insert("pending_file".into(), SCOPE.into());
            },
            |v| matches!(v, Violation::PendingFileMismatch(_)),
        ),
        (
            "a role row pending after the removal control exists",
            |_, _, i, _| {
                i.insert(REMOVAL_CONTROL.to_owned(), TestFacts::default());
            },
            |v| matches!(v, Violation::PendingRoleAfterControl(_)),
        ),
        (
            "a tracked test file with no pending row",
            |_, _, _, p| {
                p.tracked.insert("b.py".into());
            },
            |v| matches!(v, Violation::UnaccountedPython(f) if f.contains("b.py")),
        ),
        (
            "a new .py file outside the pinned set, even with a pending row",
            |i, m, _, p| {
                i.python_tests.insert("new.py::C.test_new".into());
                push_row(
                    m,
                    "python_test",
                    "ref_id = \"new.py::C.test_new\"\npending_file = \"new.py\"\n",
                );
                p.tracked.insert("new.py".into());
            },
            |v| matches!(v, Violation::UnaccountedPython(f) if f.contains("new.py")),
        ),
        (
            "a helper kept only by its own pending rows",
            |i, m, _, p| {
                i.procedural.insert("h.py::g@L2".into());
                push_row(
                    m,
                    "procedural",
                    "ref_id = \"h.py::g@L2\"\npending_file = \"h.py\"\n",
                );
                p.helper_users = &[("h.py", &["gone.py"])];
            },
            |v| matches!(v, Violation::UnaccountedPython(f) if f.contains("h.py")),
        ),
        (
            "a ref_id whose file part does not parse",
            |i, m, _, p| {
                i.python_tests.insert("noext::C.test_x".into());
                push_row(
                    m,
                    "python_test",
                    "ref_id = \"noext::C.test_x\"\npending_file = \"noext\"\n",
                );
                p.tracked.insert("noext".into());
            },
            |v| matches!(v, Violation::PendingFileMismatch(_)),
        ),
        (
            "a pending file named as a disposition",
            |_, m, _, _| {
                mutate(m, "procedural", |r| {
                    r.remove("pending_file");
                    r.insert("disposition".into(), "a.py".into());
                });
            },
            |v| matches!(v, Violation::UnknownDisposition(_)),
        ),
    ];
    for (what, apply, want) in cases {
        let (mut ids, mut map, mut index, corpus, doc, mut py) = tiny_pending();
        apply(&mut ids, &mut map, &mut index, &mut py);
        expect(
            validate(&ids, &map, &index, &corpus, TINY_APPROVED, &doc, &py),
            want,
            what,
        );
    }
}

/// The same defects that refuse a pending row never let a pending row stand
/// in for a target: a pending row gives no `NoSuchTest`, but it is also not
/// counted by `assertions_rust`, and an invented disposition is still refused.
#[test]
fn pending_rows_leave_the_disposition_rules_unchanged() {
    let (ids, mut map, index, corpus, doc, py) = tiny_pending();
    m_disposition_row(&mut map, "17.138-pending");
    mutate(&mut map, "procedural", |r| {
        r.remove("pending_file");
        r.insert("disposition".into(), "17.138-pending".into());
    });
    expect(
        validate(&ids, &map, &index, &corpus, TINY_APPROVED, &doc, &py),
        |v| matches!(v, Violation::UnrecordedDisposition(_)),
        "an invented pending disposition beside pending rows",
    );
}

fn m_disposition_row(map: &mut toml::Value, id: &str) {
    map["disposition"].as_array_mut().unwrap()[0]
        .as_table_mut()
        .unwrap()
        .insert("id".into(), id.into());
}

#[test]
fn ref_file_takes_the_part_before_the_first_separator_and_refuses_the_rest() {
    assert_eq!(
        ref_file("amplifier-bundle/recipes/tests/test-checkpoint-scope.py::GitFixtureTests.test_x"),
        Some("amplifier-bundle/recipes/tests/test-checkpoint-scope.py")
    );
    assert_eq!(
        ref_file(
            "amplifier-bundle/recipes/tests/canonical_staging_checkpoint_audit.py::restored_auxiliary_command@L47"
        ),
        Some("amplifier-bundle/recipes/tests/canonical_staging_checkpoint_audit.py")
    );
    assert_eq!(ref_file("a.py::C.f::nested@L3"), Some("a.py"));
    for bad in [
        "noext::C.test_x",
        "a.txt::f@L1",
        "::f@L1",
        "a b.py::f@L1",
        "dir/x.py",
        "",
        "x.py::",
    ] {
        assert_eq!(ref_file(bad), None, "{bad:?} must not parse");
    }
}

// --------------------------------------------- the expected tracked set

fn pending_map(rows: &[(&str, &str)]) -> toml::Value {
    let mut text = String::new();
    for (id, file) in rows {
        text += &format!("[[procedural]]\nref_id = \"{id}\"\npending_file = \"{file}\"\n");
    }
    toml::from_str(&text).unwrap()
}

#[test]
fn expected_set_is_empty_when_nothing_is_pending() {
    let map: toml::Value =
        toml::from_str("[[procedural]]\nref_id = \"a.py::f@L1\"\nrust_test = \"t.rs::ok\"\n")
            .unwrap();
    assert!(expected_tracked_python(&map, &HELPER_USERS).is_empty());
}

#[test]
fn expected_set_follows_helper_chains_to_a_fixed_point() {
    // Listed so that a single pass in list order would miss the publication
    // helper: it is used only by the harness, which is used by scope.
    const USERS: &[(&str, &[&str])] = &[
        ("pub.py", &["harness.py", "other.py"]),
        ("harness.py", &["scope.py"]),
    ];
    let map = pending_map(&[("scope.py::f@L1", "scope.py")]);
    assert_eq!(
        expected_tracked_python(&map, USERS),
        set(&["scope.py", "harness.py", "pub.py"])
    );
}

#[test]
fn a_helper_s_own_pending_rows_do_not_keep_it_expected() {
    const USERS: &[(&str, &[&str])] = &[("h.py", &["user.py"])];
    let map = pending_map(&[("h.py::f@L1", "h.py")]);
    assert!(
        expected_tracked_python(&map, USERS).is_empty(),
        "a helper is expected only while an expected file uses it"
    );
    let map = pending_map(&[("h.py::f@L1", "h.py"), ("user.py::g@L2", "user.py")]);
    assert_eq!(
        expected_tracked_python(&map, USERS),
        set(&["user.py", "h.py"])
    );
}

#[test]
fn expected_set_for_every_row_pending_is_exactly_the_pinned_19() {
    // The C-s state: every inventory row pending with its own file. All 19
    // files are expected: 14 tests with rows, the harness and the two audit
    // helpers through their users, and the two fixtures through scope.
    let ids = load_ids(&fs::read_to_string(root().join(DIR).join("inventory_ids.json")).unwrap());
    let mut text = String::new();
    for id in ids.python_tests.iter().chain(ids.procedural.iter()) {
        let file = ref_file(id).unwrap_or_else(|| panic!("{id} parses"));
        text += &format!("[[procedural]]\nref_id = \"{id}\"\npending_file = \"{file}\"\n");
    }
    for id in ids.roles.keys() {
        text += &format!("[[role]]\nref_id = \"{id}\"\npending_file = \"{ROLE_PENDING_FILE}\"\n");
    }
    for id in &ids.sites {
        text += &format!(
            "[[site]]\nref_id = \"{id}\"\npending_file = \"{}\"\n",
            site_file(id)
        );
    }
    let map: toml::Value = toml::from_str(&text).unwrap();
    let expected = expected_tracked_python(&map, &HELPER_USERS);
    let pinned: BTreeSet<String> = PINNED_19.iter().map(|s| (*s).to_owned()).collect();
    assert_eq!(expected, pinned);
}

#[test]
fn expected_set_after_each_port_drops_only_that_file_until_scope() {
    // Port files 1..=13 one by one: each removes exactly its own file, and no
    // helper, because every helper's last user is test-checkpoint-scope.py.
    let ids = load_ids(&fs::read_to_string(root().join(DIR).join("inventory_ids.json")).unwrap());
    let rows: Vec<(String, String)> = ids
        .python_tests
        .iter()
        .chain(ids.procedural.iter())
        .map(|id| (id.clone(), ref_file(id).unwrap().to_owned()))
        .collect();
    let map_without = |ported: &[&str]| -> toml::Value {
        let mut text = String::new();
        for (id, file) in &rows {
            let name = file.rsplit('/').next().unwrap();
            if !ported.contains(&name) {
                text += &format!("[[procedural]]\nref_id = \"{id}\"\npending_file = \"{file}\"\n");
            }
        }
        toml::from_str(&text).unwrap()
    };
    let mut previous = expected_tracked_python(&map_without(&[]), &HELPER_USERS);
    for n in 1..PORT_ORDER.len() {
        let now = expected_tracked_python(&map_without(&PORT_ORDER[..n]), &HELPER_USERS);
        let gone: Vec<_> = previous.difference(&now).cloned().collect();
        assert_eq!(
            gone,
            vec![format!(
                "amplifier-bundle/recipes/tests/{}",
                PORT_ORDER[n - 1]
            )],
            "port {n} must remove only its own file"
        );
        previous = now;
    }
    // Porting scope's rows alone leaves the helpers' own rows pending, and
    // the helpers become unused: the expected set is then empty, so the
    // helpers must go in the same commit.
    assert!(expected_tracked_python(&map_without(&PORT_ORDER), &HELPER_USERS).is_empty());
}

// ------------------------------------------------------- pinned constants

#[test]
fn pinned_19_is_the_python_set_tracked_when_the_map_was_created() {
    let mut want = vec![ADAPTER, RECEIPTS, AUDIT, PUBLICATION, HARNESS];
    let tests: Vec<String> = PORT_ORDER
        .iter()
        .map(|f| format!("amplifier-bundle/recipes/tests/{f}"))
        .collect();
    want.extend(tests.iter().map(String::as_str));
    let want: BTreeSet<&str> = want.into_iter().collect();
    let got: BTreeSet<&str> = PINNED_19.iter().copied().collect();
    assert_eq!(got.len(), 19, "PINNED_19 has duplicates");
    assert_eq!(got, want);
}

#[test]
fn helpers_are_exactly_the_five_shared_files_and_are_pinned() {
    assert_eq!(HELPERS.len(), 5);
    assert_eq!(
        HELPERS.iter().copied().collect::<BTreeSet<_>>(),
        [ADAPTER, RECEIPTS, AUDIT, PUBLICATION, HARNESS]
            .into_iter()
            .collect()
    );
    assert!(HELPERS.iter().all(|h| PINNED_19.contains(h)));
    let listed: Vec<&str> = HELPER_USERS.iter().map(|(h, _)| *h).collect();
    assert_eq!(
        listed.iter().copied().collect::<BTreeSet<_>>(),
        HELPERS.iter().copied().collect(),
        "HELPER_USERS lists each helper once"
    );
    assert_eq!(listed.len(), 5);
    for (helper, users) in HELPER_USERS {
        assert!(!users.is_empty(), "{helper} has no user");
        for user in users {
            assert!(
                PINNED_19.contains(user),
                "{helper}: user {user} is not pinned"
            );
        }
    }
}

#[test]
fn the_fixed_role_and_site_files_are_the_documented_derivation() {
    assert_eq!(ROLE_PENDING_FILE, ROLE_FILE_LITERAL);
    assert_eq!(
        REMOVAL_CONTROL,
        "crates/amplihack-workflows/tests/canonical_staging_policy_test.rs::removing_any_authorised_opt_out_is_named_by_the_policy_check"
    );
    let want: BTreeMap<&str, &str> = [
        (
            "autodrive-crusty-round::step-04-address-concerns",
            MINIMALIST,
        ),
        ("workflow-tdd::checkpoint-after-implementation", MINIMALIST),
        (
            "workflow-refactor-review::checkpoint-after-review-feedback",
            MINIMALIST,
        ),
        ("workflow-publish::step-16b-outside-in-fix-loop", MINIMALIST),
        ("workflow-publish::step-15-commit-push", SCOPE),
        ("workflow-pr-review::step-18c-push-feedback-changes", SCOPE),
        ("workflow-finalize::step-20b-push-cleanup", SCOPE),
    ]
    .into_iter()
    .collect();
    let got: BTreeMap<&str, &str> = SITE_PENDING_FILES.iter().copied().collect();
    assert_eq!(got, want);
    let ids = load_ids(&fs::read_to_string(root().join(DIR).join("inventory_ids.json")).unwrap());
    assert_eq!(
        ids.sites
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        want.keys().copied().collect(),
        "the table names exactly the seven inventory sites"
    );
}

fn git_grep_py(needle: &str) -> BTreeSet<String> {
    let out = amplihack_git::command_in(root())
        .args(["grep", "-l", "-z", "-F", "-e", needle, "--", "*.py"])
        .output()
        .expect("git grep");
    // 1 means no match; anything else is an error.
    assert!(
        matches!(out.status.code(), Some(0 | 1)),
        "git grep {needle}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| String::from_utf8(p.to_vec()).unwrap())
        .collect()
}

#[test]
fn each_site_is_pending_with_the_last_tracked_file_in_port_order_that_names_it() {
    let tracked = tracked_python(&root());
    for (site, file) in SITE_PENDING_FILES {
        if !tracked.contains(file) {
            continue; // ported: its rows are mapped and the validator checks them
        }
        let step = site.split_once("::").unwrap().1;
        let users = git_grep_py(step);
        assert!(users.contains(file), "{file} does not name {step}");
        let position = |f: &str| {
            PORT_ORDER
                .iter()
                .position(|p| f.ends_with(&format!("/{p}")))
        };
        let mine = position(file).unwrap();
        for user in &users {
            if let Some(n) = position(user) {
                assert!(
                    n <= mine,
                    "{site}: {user} names it and is ported after {file}"
                );
            }
        }
    }
}

#[test]
fn helper_users_match_git_grep_in_both_directions() {
    let tracked = tracked_python(&root());
    for (helper, users) in HELPER_USERS {
        if !tracked.contains(helper) {
            continue;
        }
        let stem = helper
            .rsplit('/')
            .next()
            .unwrap()
            .strip_suffix(".py")
            .unwrap();
        let mut found = git_grep_py(stem);
        found.remove(helper);
        let listed: BTreeSet<String> = users
            .iter()
            .filter(|u| tracked.contains(**u))
            .map(|u| (*u).to_owned())
            .collect();
        assert_eq!(found, listed, "{helper}: users differ from git grep");
    }
}

#[test]
fn tracked_python_reads_git_ls_files_and_matches_the_expected_set() {
    let tracked = tracked_python(&root());
    assert!(tracked.iter().all(|p| p.ends_with(".py")));
    let pinned: BTreeSet<String> = PINNED_19.iter().map(|s| (*s).to_owned()).collect();
    assert!(
        tracked.is_subset(&pinned),
        "a .py file outside PINNED_19 is tracked"
    );
    let map: toml::Value = toml::from_str(
        &fs::read_to_string(root().join(DIR).join("inventory_map.toml"))
            .expect("commit inventory_map.toml"),
    )
    .unwrap();
    assert_eq!(tracked, expected_tracked_python(&map, &HELPER_USERS));
}

#[test]
fn a_real_pending_row_naming_an_untracked_file_is_refused() {
    let ids = load_ids(&fs::read_to_string(root().join(DIR).join("inventory_ids.json")).unwrap());
    let mut map: toml::Value = toml::from_str(
        &fs::read_to_string(root().join(DIR).join("inventory_map.toml"))
            .expect("commit inventory_map.toml"),
    )
    .unwrap();
    let doc = fs::read_to_string(root().join(DOC)).unwrap();
    let index = scan(&workspace_test_sources());
    let corpus = load_corpus(&root().join(DIR).join("corpus"));
    let py = PythonFiles {
        tracked: tracked_python(&root()),
        pinned: &PINNED_19,
        helper_users: &HELPER_USERS,
    };
    let first = ids.python_tests.iter().next().unwrap().clone();
    let row = row_mut(&mut map, "python_test", &first);
    for key in ["rust_test", "assertions_python", "assertions_rust", "note"] {
        row.remove(key);
    }
    row.insert(
        "pending_file".into(),
        "amplifier-bundle/recipes/tests/test-never-tracked.py".into(),
    );
    let errs = validate(&ids, &map, &index, &corpus, &APPROVED_ROWS, &doc, &py).unwrap_err();
    assert!(
        errs.iter()
            .any(|v| matches!(v, Violation::PendingFileNotTracked(_))),
        "{errs:?}"
    );
}

#[test]
fn the_index_covers_workflows_tests_so_role_targets_resolve() {
    let index = scan(&workspace_test_sources());
    for name in [
        "every_authorised_agent_explicitly_defers_staging",
        "nested_adjudicator_is_a_visible_scope_exclusion",
    ] {
        let key =
            format!("crates/amplihack-workflows/tests/canonical_staging_policy_test.rs::{name}");
        let facts = index
            .get(&key)
            .unwrap_or_else(|| panic!("{key} not indexed"));
        assert!(!facts.ignored && !facts.cfg_gated && !facts.env_gated_early_return);
    }
}

#[test]
fn mapped_role_rows_target_only_the_two_source_policy_tests() {
    let ids = load_ids(&fs::read_to_string(root().join(DIR).join("inventory_ids.json")).unwrap());
    let map: toml::Value = toml::from_str(
        &fs::read_to_string(root().join(DIR).join("inventory_map.toml"))
            .expect("commit inventory_map.toml"),
    )
    .unwrap();
    let policy = "crates/amplihack-workflows/tests/canonical_staging_policy_test.rs";
    let mut pending = 0;
    for row in map["role"].as_array().expect("[[role]] rows") {
        let id = row["ref_id"].as_str().unwrap();
        match row.get("rust_test").and_then(toml::Value::as_str) {
            Some(target) => {
                let want = if ids.roles[id] {
                    format!("{policy}::every_authorised_agent_explicitly_defers_staging")
                } else {
                    format!("{policy}::nested_adjudicator_is_a_visible_scope_exclusion")
                };
                assert_eq!(target, want, "{id}");
            }
            None => {
                assert_eq!(
                    row["pending_file"].as_str(),
                    Some(ROLE_PENDING_FILE),
                    "{id}"
                );
                pending += 1;
            }
        }
    }
    // All or nothing: the role rows move together when the control lands.
    assert!(
        pending == 0 || pending == 41,
        "{pending} of 41 role rows pending"
    );
}
