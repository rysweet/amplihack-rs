//! Legacy-parity guards for the checkpoint port (#1554) that a behavioural
//! test cannot see on its own: fixture integrity, the 918c manifest, the
//! documented difference list, wrapper portability, the legacy
//! `advertised-ref` call shape and the inventory mapping.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn read(rel: &str) -> String {
    fs::read_to_string(root().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

const FIXTURES: &str = "crates/amplihack-cli/tests/fixtures/workflow_checkpoint";

const WRAPPERS: [&str; 4] = [
    "amplifier-bundle/tools/workflow_publish_checkpoint.sh",
    "amplifier-bundle/tools/workflow_stage_changes.sh",
    "amplifier-bundle/tools/workflow_pr_head_sync.sh",
    "amplifier-bundle/tools/workflow_cleanup_checkpoint.sh",
];

/// `git show 918c256cf:<path>`, or `None` when the object is unavailable
/// (a shallow CI clone). Callers keep a structural fallback.
fn at_918c(rel: &str) -> Option<Vec<u8>> {
    let out = amplihack_git::command_in(root())
        .args(["show", &format!("918c256cf:{rel}")])
        .output()
        .ok()?;
    out.status.success().then_some(out.stdout)
}

// ------------------------------------------------------------ fixtures

#[test]
fn legacy_fixture_bytes_match_their_sha256sums() {
    let sums = read(&format!("{FIXTURES}/SHA256SUMS"));
    let mut seen = 0;
    for line in sums.lines().filter(|l| !l.trim().is_empty()) {
        let (digest, name) = line.split_once("  ").expect("sha256sum format");
        let bytes = fs::read(root().join(FIXTURES).join(name)).unwrap();
        let got: String = Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(got, digest, "{name} changed (line-ending normalisation?)");
        seen += 1;
    }
    assert!(seen >= 3, "the three D1 inputs must be listed");
    for name in ["no-final-lf", "crlf", "mixed-oid-widths"] {
        assert!(
            sums.contains(&format!("legacy_parser/{name}.advert")),
            "{name} not checksummed"
        );
    }
}

#[test]
fn legacy_fixtures_are_protected_from_text_conversion() {
    let attrs = read(&format!("{FIXTURES}/.gitattributes"));
    assert!(
        attrs.lines().any(|l| l.trim() == "* -text"),
        "fixtures need `* -text`: {attrs}"
    );
}

#[test]
fn d1_packet_hashes_are_the_ones_cc_recorded() {
    // From cc's result.json (input_sha256 per row).
    let expected = [
        (
            "no-final-lf",
            "59161683958a9adeaacaf650be033f302e118cdf7b2f739b2b8ada2ac088b337",
        ),
        (
            "crlf",
            "3de27459abf113af2fb4595d897bb57b03c55592afae3ed74e8c85250f5e3f60",
        ),
        (
            "mixed-oid-widths",
            "6b3d32ef35f078ce6d11b8c3ba8e2162460c98892014accd2e5db0d74b499dea",
        ),
    ];
    let sums = read(&format!("{FIXTURES}/SHA256SUMS"));
    for (name, digest) in expected {
        assert!(
            sums.contains(&format!("{digest}  legacy_parser/{name}.advert")),
            "{name}"
        );
    }
}

// ------------------------------------------------------------ manifests

#[test]
fn root_cargo_toml_is_byte_identical_to_918c() {
    let now = fs::read(root().join("Cargo.toml")).unwrap();
    match at_918c("Cargo.toml") {
        Some(base) => assert!(now == base, "root Cargo.toml differs from 918c256cf"),
        None => {
            let text = String::from_utf8(now).unwrap();
            assert!(
                text.contains(
                    "[profile.release]\nlto = true\nstrip = true\npanic = \"unwind\"\ncodegen-units = 1\n"
                ),
                "release profile differs from 918c"
            );
            assert!(
                !text.contains("workflow-checkpoint"),
                "checkpoint comment in root manifest"
            );
        }
    }
}

#[test]
fn agents_md_is_unchanged_from_918c() {
    if let Some(base) = at_918c("AGENTS.md") {
        assert!(
            fs::read(root().join("AGENTS.md")).unwrap() == base,
            "AGENTS.md must not change"
        );
    }
    assert!(!read("AGENTS.md").contains("workflow-checkpoint"));
}

#[test]
fn base64_is_pinned_to_0_22_1_in_amplihack_cli() {
    let manifest = read("crates/amplihack-cli/Cargo.toml");
    let lines: Vec<&str> = manifest
        .lines()
        .filter(|l| l.trim_start().starts_with("base64"))
        .collect();
    assert_eq!(lines, ["base64 = \"0.22.1\""], "{lines:?}");
}

// ------------------------------------------------------------- docs

#[test]
fn reference_lists_only_the_two_17_126_authorised_changes() {
    let doc = read("docs/reference/workflow-checkpoint-cli.md");
    assert!(
        !doc.contains("Deliberate differences"),
        "the draft-only differences table must go"
    );
    for draft_only in [
        "no final line feed",
        "mixed object identity widths",
        "non-UTF-8 private path",
        "refuses an empty path",
    ] {
        assert!(
            !doc.to_lowercase().contains(&draft_only.to_lowercase()),
            "doc still describes the draft-only change {draft_only:?}"
        );
    }
    let section = doc
        .split("## Authorised changes (17.126)")
        .nth(1)
        .expect("the reference must have an Authorised changes (17.126) section");
    let section = section.split("\n## ").next().unwrap();
    let rows: Vec<&str> = section
        .lines()
        .filter(|l| l.starts_with('|') && !l.contains("---"))
        .skip(1)
        .collect();
    assert_eq!(rows.len(), 2, "exactly two authorised rows: {rows:#?}");
    assert!(rows.iter().all(|r| r.contains("79")), "{rows:#?}");
    assert!(read("docs/index.md").contains("reference/workflow-checkpoint-cli.md"));
}

#[test]
fn reference_documents_the_legacy_advertised_ref_argv() {
    let doc = read("docs/reference/workflow-checkpoint-cli.md");
    assert!(
        doc.contains("advertised-ref <saved-output-file> <ref> <ls-remote-rc>"),
        "advertised-ref must be documented with the 918c argv"
    );
}

// --------------------------------------------------------- wrappers

#[test]
fn wrappers_use_no_bash_4_constructs() {
    let banned: [(&str, &str); 6] = [
        ("declare -A", "associative array"),
        ("mapfile", "mapfile"),
        ("readarray", "readarray"),
        (",,}", "${var,,}"),
        ("^^}", "${var^^}"),
        ("|&", "|&"),
    ];
    for rel in WRAPPERS {
        for (n, l) in read(rel).lines().enumerate() {
            if l.trim_start().starts_with('#') {
                continue;
            }
            for (needle, what) in banned {
                assert!(!l.contains(needle), "{rel}:{}: {what}: {l}", n + 1);
            }
        }
    }
}

#[test]
fn wrappers_parse_under_bash_posix_mode() {
    for rel in WRAPPERS {
        let out = Command::new("bash")
            .args(["--posix", "-n"])
            .arg(root().join(rel))
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{rel}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn pr_head_sync_keeps_ls_remote_in_the_shell_and_passes_file_ref_rc() {
    let body = read("amplifier-bundle/tools/workflow_pr_head_sync.sh");
    assert!(
        body.contains("ls-remote --exit-code -- origin"),
        "the shell must still run ls-remote"
    );
    let calls: Vec<&str> = body
        .lines()
        .filter(|l| l.contains("advertised-ref"))
        .collect();
    assert_eq!(calls.len(), 1, "{calls:#?}");
    let l = calls[0];
    let file = l
        .find("\"$probe_output\"")
        .expect("saved output file argument");
    let target = l.find("\"refs/heads/$branch\"").expect("ref argument");
    let rc = l.find("\"$probe_rc\"").expect("transport status argument");
    assert!(
        file < target && target < rc,
        "918c order is file, ref, rc: {l}"
    );
    assert!(l.contains("|| inspection_rc=$?"), "{l}");
}

#[test]
fn both_destination_is_head_sites_capture_rc_with_the_wrapper_idiom() {
    // `rc=0; X=$(...) || rc=$?`, the idiom the wrappers already use.
    let body = read(WRAPPERS[0]);
    let sites: Vec<&str> = body
        .lines()
        .filter(|l| l.contains("destination-is-head") && l.contains("DESTINATION_IS_HEAD="))
        .collect();
    assert_eq!(sites.len(), 2, "{sites:#?}");
    for l in sites {
        assert!(
            l.contains("internal workflow-checkpoint destination-is-head")
                || l.contains("workflow_checkpoint destination-is-head"),
            "{l}"
        );
        assert!(
            l.trim_end().ends_with("|| rc=$?") || l.contains("|| rc=$?;"),
            "{l}"
        );
    }
}

// ------------------------------------------------- engine structure

#[test]
fn a_panic_is_not_mapped_to_78() {
    let m = read("crates/amplihack-cli/src/workflow_checkpoint/mod.rs");
    let body = m
        .split("fn catch_panic")
        .nth(1)
        .expect("catch_panic")
        .split("\n}\n")
        .next()
        .unwrap();
    assert!(
        !body.contains("exit_error(78)"),
        "panic -> 78 has no 17.126 cover"
    );
    assert!(body.contains("exit_error(1)"), "panic must exit 1: {body}");
}

#[test]
fn main_scopes_the_coded_exit_to_workflow_checkpoint() {
    let main = read("bins/amplihack/src/main.rs");
    let internal = main
        .split("if args.get(1).is_some_and(|arg| arg == \"internal\")")
        .nth(1)
        .expect("internal branch")
        .split("return;")
        .next()
        .unwrap();
    assert!(
        internal.contains("workflow-checkpoint"),
        "pass-through is not scoped: {internal}"
    );
    assert!(
        internal.contains("eprintln!(\"error: {error:#}\")"),
        "{internal}"
    );
    assert!(internal.contains("std::process::exit(1)"), "{internal}");
}

// ------------------------------------------------- inventory mapping

const INVENTORY: &str =
    "crates/amplihack-cli/tests/fixtures/workflow_checkpoint/inventory_map.toml";

/// `path::name` -> does `fn name(` exist in `path`, and is it `#[ignore]`d?
fn rust_test_state(spec: &str) -> Option<bool> {
    let (path, name) = spec.rsplit_once("::")?;
    let src = fs::read_to_string(root().join(path)).ok()?;
    let at = src.find(&format!("fn {name}("))?;
    let head = &src[..at];
    let attrs = head.rsplit("\n\n").next().unwrap_or("");
    let attrs = attrs.rsplit('}').next().unwrap_or(attrs);
    Some(attrs.contains("#[ignore"))
}

#[test]
fn inventory_map_covers_cc_inventory_with_runnable_tests_or_dispositions() {
    let text = read(INVENTORY);
    let doc: toml::Value = toml::from_str(&text).expect("inventory_map.toml parses");
    let filter = doc
        .get("filter")
        .and_then(|v| v.as_table())
        .expect("[filter] section");
    assert!(
        filter
            .get("rule")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.is_empty()),
        "record cc's review-plan.md filter"
    );
    let excluded = filter
        .get("excluded_keys")
        .and_then(|v| v.as_array())
        .expect("excluded_keys");
    let items = doc
        .get("item")
        .and_then(|v| v.as_array())
        .expect("[[item]] rows");

    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut keys = BTreeSet::new();
    let mut problems = Vec::new();
    for item in items {
        let t = item.as_table().expect("item table");
        let s = |k: &str| t.get(k).and_then(|v| v.as_str()).unwrap_or("");
        let key = s("inventory_key");
        let kind = s("kind");
        let class = s("class");
        if !keys.insert(key.to_owned()) {
            problems.push(format!("{key}: mapped twice"));
        }
        if !["behavioural", "provenance", "audit_receipt"].contains(&class) {
            problems.push(format!("{key}: class {class:?}"));
        }
        if !["method", "procedural", "role", "site"].contains(&kind) {
            problems.push(format!("{key}: kind {kind:?}"));
        }
        if class == "behavioural" {
            *counts.entry(kind.to_owned()).or_default() += 1;
        }
        for flag in ["runs_normally", "negative_case", "mutant_sensitive"] {
            if t.get(flag).and_then(|v| v.as_bool()).is_none() {
                problems.push(format!("{key}: {flag} must be a boolean"));
            }
        }
        let disposition = s("disposition");
        match (s("rust_test"), disposition) {
            ("", "") => problems.push(format!("{key}: no rust_test and no disposition")),
            ("", d) => {
                if !d.contains("owner approval required") && !d.contains("approved") {
                    problems.push(format!(
                        "{key}: disposition must need or cite owner approval"
                    ));
                }
            }
            (spec, _) => match rust_test_state(spec) {
                None => problems.push(format!("{key}: {spec} does not exist")),
                Some(ignored) => {
                    let runs = t.get("runs_normally").and_then(|v| v.as_bool()) == Some(true);
                    if ignored == runs {
                        problems.push(format!("{key}: runs_normally disagrees with #[ignore]"));
                    }
                    if ignored && disposition.is_empty() {
                        problems.push(format!("{key}: {spec} is ignored without a disposition"));
                    }
                    if kind == "method" {
                        let py = key.rsplit("::").next().unwrap_or("");
                        let want = py.strip_prefix("test_").unwrap_or(py);
                        let got = spec.rsplit("::").next().unwrap_or("");
                        if got != want {
                            problems.push(format!("{key}: ported name {got} must be {want}"));
                        }
                    }
                }
            },
        }
    }
    assert_eq!(
        counts.get("method"),
        Some(&190),
        "behavioural methods: {counts:?}"
    );
    assert_eq!(counts.get("role"), Some(&40), "roles: {counts:?}");
    assert_eq!(counts.get("site"), Some(&7), "checkpoint sites: {counts:?}");
    assert!(
        counts.get("procedural").copied().unwrap_or(0) > 0,
        "procedural assertions: {counts:?}"
    );
    // The reference has exactly 190 test_ methods; every excluded key must
    // still name an inventory entry.
    assert!(
        excluded
            .iter()
            .all(|k| k.as_str().is_some_and(|k| !k.is_empty())),
        "excluded_keys must be non-empty strings"
    );
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn inventory_map_keeps_provenance_and_audit_receipts_out_of_the_behavioural_count() {
    let text = read(INVENTORY);
    let doc: toml::Value = toml::from_str(&text).unwrap();
    let items = doc["item"].as_array().unwrap();
    let receipts = items
        .iter()
        .filter(|i| matches!(i["class"].as_str(), Some("provenance" | "audit_receipt")))
        .count();
    assert!(
        receipts > 0,
        "provenance and audit receipts must be listed separately"
    );
}

#[test]
fn twenty_owner_outgoing_only_controls_and_twelve_owner_controls_are_ported_by_name() {
    let text = read(INVENTORY);
    let doc: toml::Value = toml::from_str(&text).unwrap();
    let items = doc["item"].as_array().unwrap();
    let in_class = |class: &str| {
        items
            .iter()
            .filter(|i| {
                i["inventory_key"]
                    .as_str()
                    .is_some_and(|k| k.contains(&format!("::{class}::")))
                    && i.get("rust_test")
                        .and_then(|v| v.as_str())
                        .is_some_and(|s| !s.is_empty())
            })
            .count()
    };
    assert_eq!(in_class("OwnerOutgoingOnlyControls"), 20);
    assert!(
        items
            .iter()
            .filter(|i| i.get("owner_control").and_then(|v| v.as_bool()) == Some(true))
            .count()
            == 12,
        "mark the twelve original owner-control methods with owner_control = true"
    );
}
