//! Legacy-parity guards for the checkpoint port (#1554) that a behavioural
//! test cannot see on its own: fixture integrity, the 918c manifest, the
//! documented difference list, wrapper portability, the legacy
//! `advertised-ref` call shape and the inventory mapping.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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

/// `git show 918c256cf:<path>`, or `None` when the object is unavailable
/// (a shallow CI clone). Callers keep a structural fallback.
fn at_918c(rel: &str) -> Option<Vec<u8>> {
    let out = amplihack_git::command_in(root())
        .args(["show", &format!("918c256cf:{rel}")])
        .output()
        .ok()?;
    out.status.success().then_some(out.stdout)
}

// The D1 inputs are byte literals checked in
// workflow_checkpoint_consensus_guards.rs (zero-A); no fixture file remains.

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
fn reference_lists_only_the_approved_17_126_change() {
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
    // The first table lists the authorised changes; the second lists the
    // eight allowlist rows with their pin tests.
    let rows: Vec<&str> = section
        .lines()
        .skip_while(|l| !l.starts_with('|'))
        .take_while(|l| l.starts_with('|'))
        .filter(|l| !l.contains("---"))
        .skip(1)
        .collect();
    // bb approved the destination-read 79 at both sites and the 17.126
    // stage-strict addendum (a real Git 79 passes through), nothing else.
    assert_eq!(rows.len(), 2, "exactly two authorised changes: {rows:#?}");
    assert!(
        rows[0].contains("destination-is-head") && rows[0].contains("79"),
        "{rows:#?}"
    );
    assert!(
        rows[1].contains("`stage-strict`") && rows[1].contains("79"),
        "{rows:#?}"
    );
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
    let checkpoint = main
        .split("arg == \"workflow-checkpoint\")")
        .nth(1)
        .expect("checkpoint branch")
        .split("if args.get(1).is_some_and(|arg| arg == \"internal\") {")
        .next()
        .unwrap();
    assert!(
        checkpoint.contains("command_error::exit_code(&error)"),
        "pass-through is not scoped: {checkpoint}"
    );
    let generic = main
        .split("if args.get(1).is_some_and(|arg| arg == \"internal\") {")
        .nth(1)
        .expect("generic internal branch")
        .split("return;")
        .next()
        .unwrap();
    assert!(!generic.contains("exit_code"), "{generic}");
    assert!(
        generic.contains("eprintln!(\"error: {error:#}\")"),
        "{generic}"
    );
    assert!(generic.contains("std::process::exit(1)"), "{generic}");
}

// ------------------------------------------------- inventory mapping
//
// The map's structure (one row per reference ID, exactly one of rust_test,
// disposition or pending_file, runnable targets, the eight dispositions) is
// checked by `validate()` in workflow_checkpoint_inventory.rs. These guards
// keep the two legacy obligations that `validate()` does not name.

const INVENTORY: &str =
    "crates/amplihack-cli/tests/fixtures/workflow_checkpoint/inventory_map.toml";
const PUBLICATION_BOUNDARIES: &str =
    "amplifier-bundle/recipes/tests/test-checkpoint-publication-boundaries.py";

fn inventory_rows(table: &str) -> Vec<toml::Value> {
    let doc: toml::Value = toml::from_str(&read(INVENTORY)).expect("inventory_map.toml parses");
    doc.get(table)
        .and_then(toml::Value::as_array)
        .cloned()
        .unwrap_or_default()
}

#[test]
fn inventory_map_keeps_provenance_and_audit_receipts_out_of_the_coverage_tables() {
    let provenance = inventory_rows("provenance");
    assert!(
        !provenance.is_empty(),
        "provenance must be listed separately"
    );
    for table in ["provenance", "audit_receipt"] {
        for row in inventory_rows(table) {
            assert!(
                row.get("ref_id").is_none(),
                "[[{table}]] {row:?} names a reference ID, so it would count as coverage"
            );
        }
    }
}

#[test]
fn the_twenty_owner_outgoing_only_controls_are_mapped_or_pending_with_their_file() {
    // The twelve original owner-control methods are among these twenty
    // (canonical-staging.md); the validator refuses deleting the file while
    // any of them is still pending, so all twenty are ported before it goes.
    let owner: Vec<toml::Value> = inventory_rows("python_test")
        .into_iter()
        .filter(|r| {
            r.get("ref_id")
                .and_then(toml::Value::as_str)
                .is_some_and(|id| id.contains("::OwnerOutgoingOnlyControls.test_"))
        })
        .collect();
    assert_eq!(owner.len(), 20, "OwnerOutgoingOnlyControls rows");
    for row in owner {
        let target = row.get("rust_test").and_then(toml::Value::as_str);
        let pending = row.get("pending_file").and_then(toml::Value::as_str);
        match (target, pending) {
            (Some(t), None) => assert!(
                t.contains("::") && !t.ends_with(".py"),
                "{row:?}: rust_test must name a Rust test"
            ),
            (None, Some(file)) => assert_eq!(file, PUBLICATION_BOUNDARIES, "{row:?}"),
            _ => panic!("{row:?}: an owner control is ported or pending, never a disposition"),
        }
    }
}
