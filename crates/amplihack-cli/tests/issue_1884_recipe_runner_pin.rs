//! Issue #1884: the plugin pins recipe-runner-rs v0.5.2.
//!
//! `claude-plugin/recipe-runner.rev` is the single source of the pin. None of
//! the tests below names a revision: they read the pin from that file, so a
//! correct pin bump needs no edit here. The CI "Recipe runner pin ancestry"
//! check is what stops the pin moving backwards.
//!
//! The tests check that the file is in the form the install scripts accept
//! and that the documentation never quotes a revision other than the current
//! pin where it talks about `recipe-runner.rev`. The unit-test fixtures in
//! `runner_validation.rs` and `runner_ancestry.rs` read the same file through
//! `freshness::RECIPE_RUNNER_REV`, so they cannot drift from it.
//!
//! No source file holds the pin as a literal, so the tc_sec_20 secret scan
//! (which reads `crates/amplihack-cli/src/**/*.rs`) never sees it and needs no
//! allowlist entry for it. A pin bump touches no security test.

use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root is two levels above the crate manifest")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

fn pin() -> String {
    read("claude-plugin/recipe-runner.rev")
        .trim_end()
        .to_string()
}

fn is_lower_hex(s: &str) -> bool {
    s.bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[test]
fn pin_file_is_a_full_lowercase_sha_and_one_newline() {
    let raw = read("claude-plugin/recipe-runner.rev");
    let sha = raw
        .strip_suffix('\n')
        .expect("recipe-runner.rev must end with a newline");
    assert_eq!(
        sha.len(),
        40,
        "recipe-runner.rev must hold a 40-character SHA, found {sha:?}"
    );
    assert!(
        is_lower_hex(sha),
        "recipe-runner.rev must be lowercase hex, found {sha:?}"
    );
}

/// Backticked tokens of 7 to 40 lowercase hex characters, the forms a commit
/// is quoted in.
fn quoted_revisions(text: &str) -> Vec<&str> {
    text.split('`')
        .skip(1)
        .step_by(2)
        .filter(|t| (7..=40).contains(&t.len()) && is_lower_hex(t))
        .collect()
}

#[test]
fn docs_quote_no_stale_pin() {
    let pin = pin();
    for doc in [
        "docs/reference/profile-command.md",
        "docs/reference/large-payload-delivery.md",
        "docs/concepts/large-payload-delivery.md",
    ] {
        let text = read(doc);
        // A paragraph (or table row) that names the pin file is one that
        // states which runner revision a claim was checked at.
        let quotes: Vec<&str> = text
            .split("\n\n")
            .filter(|para| para.contains("recipe-runner.rev"))
            .flat_map(quoted_revisions)
            .collect();
        assert!(
            !quotes.is_empty(),
            "{doc} no longer quotes the pinned runner revision beside recipe-runner.rev; \
             update the doc or this list"
        );
        for quoted in quotes {
            assert!(
                pin.starts_with(quoted),
                "{doc} quotes `{quoted}` beside recipe-runner.rev, but the pin is {pin}; \
                 recheck the claim at the new pin and update the quote"
            );
        }
    }
}

#[test]
fn quoted_revisions_finds_only_hex_tokens() {
    let text = "pinned in `claude-plugin/recipe-runner.rev` (`01cf296`), not `4d5187e` \
                or `AMPLIHACK_HOME` or `abc`";
    assert_eq!(quoted_revisions(text), ["01cf296", "4d5187e"]);
}
