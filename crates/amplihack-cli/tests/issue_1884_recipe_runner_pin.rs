//! Issue #1884: the plugin pins recipe-runner-rs v0.5.2
//! (`01cf296f21089045c8fd4d1564360354fa664fb2`).
//!
//! `claude-plugin/recipe-runner.rev` is the single source of the pin. The
//! tests below check that the pin is that exact revision in the form the
//! install scripts accept, and that the unit-test fixtures, the tc_sec_20
//! allowlist and the documentation quote the same revision.

use std::fs;
use std::path::{Path, PathBuf};

const V0_5_2: &str = "01cf296f21089045c8fd4d1564360354fa664fb2";

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

#[test]
fn pin_is_recipe_runner_v0_5_2() {
    assert_eq!(
        read("claude-plugin/recipe-runner.rev"),
        format!("{V0_5_2}\n"),
        "recipe-runner.rev must hold the v0.5.2 commit followed by one newline"
    );
}

#[test]
fn unit_test_fixtures_name_the_pinned_revision() {
    let pin = pin();
    let validation = read("crates/amplihack-cli/src/runner_validation.rs");
    assert!(
        validation.contains(&format!("const SHA: &str = \"{pin}\";")),
        "runner_validation.rs installed_revision_tests::SHA must equal the pin {pin}"
    );
    let ancestry = read("crates/amplihack-cli/src/runner_ancestry.rs");
    assert!(
        ancestry.contains(&format!("let pin = \"{pin}\";")),
        "runner_ancestry.rs unknown_and_equal_need_no_git must use the pin {pin}"
    );
}

#[test]
fn security_allowlist_names_the_pin_once() {
    let pin = pin();
    let source = read("tests/integration/security_hygiene_test.rs");
    let marker = "Public recipe-runner revision pin";
    let start = source
        .find(marker)
        .expect("tc_sec_20 keeps its recipe-runner pin comment");
    // The entry is the first quoted literal after the comment.
    let rest = &source[start..];
    let open = rest.find('"').expect("allowlist entry after the comment") + 1;
    let close = open + rest[open..].find('"').expect("closing quote");
    assert_eq!(
        &rest[open..close],
        pin,
        "the tc_sec_20 allowlist entry must be the current pin, as an exact string"
    );
    assert_eq!(
        source.matches(marker).count(),
        1,
        "only one recipe-runner pin entry"
    );
}

#[test]
fn docs_quote_the_short_form_of_the_pin() {
    let short = format!("`{}`", &pin()[..7]);
    for (doc, expected) in [
        ("docs/reference/profile-command.md", 1),
        ("docs/reference/large-payload-delivery.md", 2),
        ("docs/concepts/large-payload-delivery.md", 1),
    ] {
        let found = read(doc).matches(short.as_str()).count();
        assert_eq!(
            found, expected,
            "{doc} must quote the pinned runner as {short} {expected} time(s), found {found}"
        );
    }
}
