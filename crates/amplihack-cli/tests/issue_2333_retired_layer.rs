//! Issue #2333, R4 and R5: the superseded GraphQL-then-REST read layer is
//! gone from the tree, and nothing is left pointing at it.
//!
//! #2637 retired the parsed merge-ready record, so the layer this branch
//! first built — `autodrive_gh*`, `autodrive_rest.sh`, `autodrive_waits*`,
//! the crusty `WAITING` loop result and its exit 76 — has no caller left.
//! What remains is the three-outcome head read in the completion dispatch.
//! A reference to a tool that is not there is not a documentation slip: the
//! shell reads it at run time and the round ends on a missing file.
//!
//! The merge-ready `WAITING` of #2575 shares the word and nothing else. It
//! is a different mechanism, it stays, and the last case here holds it in
//! place so the removal does not take it with it.

use std::path::{Path, PathBuf};

use crate::support::auto_drive_light::repo_root;

/// Every path the retired layer occupied. None may exist.
const REMOVED: &[&str] = &[
    "crates/amplihack-cli/src/commands/orch/autodrive_gh.rs",
    "crates/amplihack-cli/src/commands/orch/autodrive_gh",
    "crates/amplihack-cli/src/commands/orch/autodrive_gh_tests.rs",
    "crates/amplihack-cli/src/commands/orch/autodrive_waits.rs",
    "crates/amplihack-cli/src/commands/orch/autodrive_waits_tests.rs",
    "crates/amplihack-cli/src/commands/orch/autodrive/report_waiting_tests.rs",
    "crates/amplihack-cli/tests/issue_2333_gh_fallback.rs",
    "crates/amplihack-cli/tests/issue_2333_gh_fallback_dispatch.rs",
    "crates/amplihack-cli/tests/issue_2333_gh_fallback_loops.rs",
    "crates/amplihack-cli/tests/issue_2333_gh_fallback_reads.rs",
    "crates/amplihack-cli/tests/issue_2333_gh_fallback_steps.rs",
    "crates/amplihack-cli/tests/issue_2333_gh_fallback_waits.rs",
    "crates/amplihack-cli/tests/support/autodrive_gh_fake.rs",
    "amplifier-bundle/tools/autodrive_rest.sh",
    "amplifier-bundle/recipes/tests/test-issue-2333-graphql-unavailable.sh",
    "docs/reference/autodrive-rest-reads.md",
    "docs/howto/read-a-drive-that-waited-on-github.md",
];

/// Names that may not appear anywhere on the tree once the layer is gone.
/// Each is a run-time dependency somewhere — a sourced tool, a recipe
/// context key, a module declaration, a document link — so a survivor is a
/// fault waiting for the first round that takes that path.
const DANGLING: &[&str] = &[
    "autodrive_gh",
    "autodrive_rest",
    "autodrive_waits",
    "autodrive_prior_wait",
    "crusty-rereview-waiting",
    "autodrive-rest-reads",
    "read-a-drive-that-waited-on-github",
];

/// Directories worth sweeping. The whole tree is not walked: build output
/// and vendored sources are neither ours nor read at run time.
const SWEPT: &[&str] = &[
    "amplifier-bundle",
    "crates",
    "docs",
    "tests",
    ".github/workflows",
];

fn files_under(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') && name != ".github" {
                continue;
            }
            match entry.file_type() {
                Ok(t) if t.is_dir() => stack.push(path),
                Ok(t) if t.is_file() => out.push(path),
                _ => {}
            }
        }
    }
    out
}

// ── R4: the layer is gone ───────────────────────────────────────────────────

#[test]
fn every_file_of_the_retired_layer_is_removed() {
    let root = repo_root();
    for rel in REMOVED {
        let p = root.join(rel);
        assert!(
            !p.exists(),
            "{rel} belongs to the retired read layer and must not be on the tree"
        );
    }
}

/// The crusty loop has no `WAITING` result and no exit 76 any more. The
/// dispatch has three outcomes, none of them a wait the code declares, so
/// the before-round script that translated one has nothing to translate.
#[test]
fn the_crusty_wait_result_and_its_exit_code_are_gone() {
    let text = std::fs::read_to_string(
        repo_root().join("amplifier-bundle/tools/autodrive_crusty_rereview.sh"),
    )
    .expect("the re-review tool exists");
    assert!(
        !text.contains("crusty-rereview-waiting"),
        "the retired crusty WAITING path is still in autodrive_crusty_rereview.sh"
    );
    assert!(
        !text
            .lines()
            .any(|l| l.split('#').next().unwrap_or_default().contains("exit 76")),
        "exit 76 was the crusty WAITING code and has no producer left"
    );
    let rust = files_under(&repo_root().join("crates"));
    for path in rust
        .iter()
        // This file names the retired constant, by design.
        .filter(|p| !p.ends_with("issue_2333_retired_layer.rs"))
        .filter(|p| p.extension().is_some_and(|e| e == "rs"))
    {
        let body = std::fs::read_to_string(path).unwrap_or_default();
        assert!(
            !body.contains("EXIT_WAITING"),
            "{}: EXIT_WAITING is retired with the result it named",
            path.display()
        );
    }
}

// ── R5: nothing is left pointing at it ──────────────────────────────────────

#[test]
fn no_file_still_names_the_retired_layer() {
    let root = repo_root();
    let mut found: Vec<String> = Vec::new();
    for dir in SWEPT {
        for path in files_under(&root.join(dir)) {
            // This file names them all, by design.
            if path.ends_with("issue_2333_retired_layer.rs") {
                continue;
            }
            let Ok(body) = std::fs::read_to_string(&path) else {
                continue; // not text; nothing reads it as a tool or a link
            };
            for (n, line) in body.lines().enumerate() {
                for name in DANGLING {
                    if line.contains(name) {
                        found.push(format!(
                            "{}:{}: {name}: {}",
                            path.strip_prefix(&root).unwrap_or(&path).display(),
                            n + 1,
                            line.trim()
                        ));
                    }
                }
            }
        }
    }
    assert!(
        found.is_empty(),
        "references to the retired read layer remain:\n{}",
        found.join("\n")
    );
}

/// Every module declaration in the test tree, as the relative path it names.
/// Both forms count: `#[path = "x.rs"] mod x;` and a bare `mod x;`, which
/// Rust resolves to `x.rs` beside the declaring file. Attribute and `mod`
/// lines only — the harness's own instructions quote the form in a comment,
/// and a comment registers nothing.
fn declared_paths(dir: &Path) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    for file in files_under(dir) {
        if file.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let Ok(body) = std::fs::read_to_string(&file) else {
            continue;
        };
        let mut pathed = false;
        for line in body.lines() {
            let line = line.trim_start();
            if line.starts_with("//") {
                continue;
            }
            if let Some(rest) = line.strip_prefix("#[path = \"")
                && let Some(rel) = rest.split('"').next()
            {
                out.push((file.clone(), rel.to_string()));
                pathed = true;
                continue;
            }
            // A bare `mod x;` names `x.rs` beside the declaring file, unless
            // the line just before it gave the path.
            if let Some(rest) = line.strip_prefix("mod ")
                && let Some(name) = rest.strip_suffix(';')
                && !name.contains(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            {
                if !pathed {
                    out.push((file.clone(), format!("{name}.rs")));
                }
                pathed = false;
                continue;
            }
            // Other attributes may sit between the path and the `mod` line.
            if !line.is_empty() && !line.starts_with("#[") {
                pathed = false;
            }
        }
    }
    out
}

/// A module declared and not on the tree does not compile; a test file on
/// the tree and declared nowhere never runs. Both are faults of the same
/// removal, and this is where a removal that forgets one is caught.
///
/// `tests/main.rs` is the single test target, but a file may equally be a
/// module of another test file — the #2096, #2336 and #2575 groups are each
/// split that way — so reachability, not a line in `main.rs`, is what is
/// asserted.
#[test]
fn every_test_module_is_declared_and_every_declaration_exists() {
    let dir = repo_root().join("crates/amplihack-cli/tests");
    let declared = declared_paths(&dir);
    for (owner, rel) in &declared {
        let base = owner.parent().expect("a declaration has a directory");
        // A bare `mod x;` is satisfied by either `x.rs` or `x/mod.rs`.
        let dir_form = rel
            .strip_suffix(".rs")
            .map(|stem| base.join(stem).join("mod.rs"));
        assert!(
            base.join(rel).is_file() || dir_form.is_some_and(|p| p.is_file()),
            "{} declares {rel}, which is not on the tree",
            owner.display()
        );
    }
    let names: Vec<String> = declared
        .iter()
        .filter_map(|(_, rel)| rel.rsplit('/').next().map(str::to_owned))
        .collect();
    let orphans: Vec<String> = std::fs::read_dir(&dir)
        .expect("read the test directory")
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".rs") && n != "main.rs")
        .filter(|n| !names.contains(n))
        .collect();
    assert!(
        orphans.is_empty(),
        "test files that no module declaration reaches, so nothing runs them: {orphans:?}"
    );
}

// ── The merge-ready WAITING of #2575 is a different mechanism and stays ─────

/// Removing the crusty wait must not take the merge-ready one with it. It
/// is written by the round record and read by the merge loop runner, and
/// neither belongs to #2333.
#[test]
fn the_merge_ready_wait_of_2575_is_untouched() {
    let tools = repo_root().join("amplifier-bundle/tools");
    let record = std::fs::read_to_string(tools.join("autodrive_loop_record.sh"))
        .expect("the record tool exists");
    assert!(
        record.contains("\"WAITING\""),
        "the merge-ready round still records WAITING (#2575)"
    );
    let run = std::fs::read_to_string(tools.join("autodrive_merge_loop_run.sh"))
        .expect("the merge loop runner exists");
    assert!(
        run.contains("WAITING"),
        "the merge loop still reads WAITING (#2575)"
    );
}
