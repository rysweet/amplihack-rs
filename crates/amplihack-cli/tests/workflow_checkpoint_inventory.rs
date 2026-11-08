//! Inventory validator (#1554, consensus step 9).
//!
//! Three stores, all read-only:
//!
//! * `inventory_ids.json`: derived from cc's
//!   `pr2020-rust-reference-inventory-918c/inventory.json` (sha256
//!   0c3e19b0…6c788f). IDs, summary counts and cc's sha256 only.
//! * `inventory_map.toml`: one row per reference ID, each resolving to
//!   exactly one Rust test that runs normally or one owner disposition.
//!   Provenance and audit receipts live in separate tables and never count.
//! * the Rust sources, scanned structurally. Execution is proved by the real
//!   `cargo test` run, not here.
//!
//! The validator always runs against the committed IDs. When cc's file is
//! readable it is also cross-checked; no path skips.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

const DIR: &str = "crates/amplihack-cli/tests/fixtures/workflow_checkpoint";
const CC_INVENTORY: &str = "/d0/ryan/cc-staging1554-parent-20261105/control/pr2020-rust-reference-inventory-918c/inventory.json";
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
    BothColumns(String),
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

pub fn validate(
    ids: &Ids,
    map: &toml::Value,
    index: &RustTestIndex,
    corpus: &Corpus,
    approved: &[ApprovedRow],
    doc: &str,
) -> Result<(), Vec<Violation>> {
    let mut v = Vec::new();
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
            match (test, disp) {
                (Some(_), Some(_)) => v.push(Violation::BothColumns(id.clone())),
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

#[test]
fn inventory_ids_json_matches_cc_inventory_when_readable() {
    let ids = load_ids(
        &fs::read_to_string(root().join(DIR).join("inventory_ids.json"))
            .expect("commit the derived inventory_ids.json"),
    );
    // cc's file is local-only (0400, outside the repository). In CI the
    // committed sha256 pin above is the check; here it is the same check
    // plus an exact ID comparison.
    let Ok(raw) = fs::read(CC_INVENTORY) else {
        assert_eq!(ids.source_sha256, CC_SHA256);
        return;
    };
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
    if let Err(violations) = validate(&ids, &map, &index, &corpus, &APPROVED_ROWS, &doc) {
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
    for i in 0..7 {
        ids.sites.insert(format!("asset::site-{i}"));
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
        validate(&ids, &map, &index, &corpus, TINY_APPROVED, &doc),
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
            |v| matches!(v, Violation::BothColumns(_)),
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
            validate(&ids, &map, &index, &corpus, TINY_APPROVED, &doc),
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
    let errs = validate(&ids, &map, &index, &corpus, TINY_APPROVED, &doc).unwrap_err();
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
            validate(&ids, &map, &index, &corpus, rows, &doc),
            want,
            what,
        );
    }
}
