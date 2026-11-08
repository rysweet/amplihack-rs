//! Issue #1972 D2 to D4, D15: the release manifest format, when an embedded
//! manifest is active, and how a mismatch is rendered.

use super::super::bundle_manifest::{
    BundleManifest, ManifestDiff, compare, parse, render_mismatch, render_release_mismatch,
    sha256_of_file,
};
use super::super::framework_source::BinaryIdentity;
use super::super::release_manifest::{
    MalformedReleaseManifest, ManifestFinding, ReleaseManifest, embedded_text,
};
use super::issue_1972_support::*;
use std::collections::BTreeMap;
use std::path::Path;

const HASH_A: &str = concat!(
    "aaaaaaaaaaaaaaaa",
    "aaaaaaaaaaaaaaaa",
    "aaaaaaaaaaaaaaaa",
    "aaaaaaaaaaaaaaaa"
);
const HASH_B: &str = concat!(
    "0123456789abcdef",
    "0123456789abcdef",
    "0123456789abcdef",
    "0123456789abcdef"
);

fn line(hash: &str, path: &str) -> String {
    format!("{hash}  {path}\n")
}

fn parse_err(text: &str) -> (usize, &'static str) {
    let err = parse(text).expect_err("must be rejected");
    (err.line, err.reason)
}

// ── Accepted input ──

#[test]
fn issue_1972_empty_text_is_an_empty_manifest() {
    assert_eq!(parse("").unwrap(), BundleManifest::default());
}

#[test]
fn issue_1972_canonical_text_parses_and_round_trips() {
    let text = [
        line(HASH_A, "CLAUDE.md"),
        line(HASH_B, "agents/core/architect.md"),
        line(HASH_A, "agents/é.md"),
        line(HASH_B, "recipes/x.yaml"),
    ]
    .concat();
    let manifest = parse(&text).unwrap();
    assert_eq!(manifest.files.len(), 4);
    assert_eq!(manifest.files["agents/é.md"], HASH_A);
    assert_eq!(
        manifest.to_text(),
        text,
        "parse then write is byte-identical"
    );
}

// ── Rejected input, one case per rule ──

#[test]
fn issue_1972_a_missing_final_newline_is_rejected() {
    let text = format!("{}{HASH_B}  b", line(HASH_A, "a"));
    assert_eq!(parse_err(&text).0, 2);
}

#[test]
fn issue_1972_carriage_returns_and_blank_lines_are_rejected() {
    assert_eq!(parse_err(&format!("{HASH_A}  a\r\n")).1, "carriage return");
    assert_eq!(
        parse_err(&format!("{}\n{}", line(HASH_A, "a"), line(HASH_B, "b"))),
        (2, "blank line")
    );
    assert_eq!(parse_err("\n"), (1, "blank line"));
}

#[test]
fn issue_1972_hash_shape_and_separator_are_strict() {
    let upper = HASH_A.to_uppercase();
    for text in [
        line(&upper, "a"),
        line(&HASH_A[..63], "a"),
        format!("{HASH_A}a  a\n"),
        "short\n".to_string(),
    ] {
        assert!(parse(&text).is_err(), "{text:?}");
    }
    assert!(parse(&format!("{HASH_A} a\n")).is_err(), "one space");
    assert!(parse(&format!("{HASH_A}   a\n")).is_err(), "three spaces");
    assert!(parse(&format!("{HASH_A}\ta\n")).is_err(), "tab");
    assert!(parse(&format!("{HASH_A}  \n")).is_err(), "no path");
}

#[test]
fn issue_1972_binary_mode_and_escaped_lines_are_rejected() {
    assert!(parse(&format!("{HASH_A} *a\n")).is_err());
    assert!(parse(&line(HASH_A, "*a")).is_err());
    assert!(parse(&format!("\\{HASH_A}  a\n")).is_err());
}

#[test]
fn issue_1972_unclean_paths_are_rejected() {
    for path in [
        "/abs",
        "a//b",
        "a/",
        "./a",
        "a/./b",
        "a/../b",
        "..",
        "a\\b",
        "a\u{7f}b",
        "a\u{1}b",
        "a\u{fffd}b",
    ] {
        assert!(parse(&line(HASH_A, path)).is_err(), "{path:?}");
    }
}

#[test]
fn issue_1972_order_must_be_strictly_ascending_by_byte() {
    // `B` (0x42) sorts before `a` (0x61), and `a.b` before `a/b`.
    assert!(parse(&[line(HASH_A, "B"), line(HASH_A, "a")].concat()).is_ok());
    assert!(parse(&[line(HASH_A, "a.b"), line(HASH_A, "a/b")].concat()).is_ok());
    assert_eq!(
        parse_err(&[line(HASH_A, "b"), line(HASH_A, "a")].concat()).0,
        2
    );
    assert_eq!(
        parse_err(&[line(HASH_A, "a"), line(HASH_B, "a")].concat()).0,
        2,
        "a duplicate is rejected"
    );
}

#[test]
fn issue_1972_multibyte_input_never_panics() {
    // Multi-byte characters inside the first 66 bytes would panic a `&str`
    // slice at byte 64; the parser checks bytes first.
    for text in [
        format!("{}é  a\n", &HASH_A[..63]),
        format!("{HASH_A}é a\n"),
        format!("{HASH_A} é\n"),
        "é".repeat(40) + "\n",
        "\u{fffd}\n".to_string(),
    ] {
        assert!(parse(&text).is_err(), "{text:?}");
    }
}

#[test]
fn issue_1972_errors_name_the_line_and_never_echo_input() {
    let text = [line(HASH_A, "a"), line(HASH_A, "secret\u{1b}[31m")].concat();
    let err = parse(&text).unwrap_err();
    assert_eq!(err.line, 2);
    let shown = err.to_string();
    assert!(shown.starts_with("line 2: "), "{shown}");
    assert!(!shown.contains("secret"), "{shown}");
}

// ── Activation (D2, D3) ──

fn build(may_fetch: bool) -> BinaryIdentity {
    BinaryIdentity {
        version: BINARY.to_string(),
        may_fetch,
    }
}

#[test]
fn issue_1972_the_committed_placeholder_is_empty() {
    assert_eq!(
        embedded_text(),
        "",
        "the placeholder must be committed empty"
    );
    assert_eq!(ReleaseManifest::embedded(&build(true)).unwrap(), None);
    assert_eq!(
        ReleaseManifest::embedded(&BinaryIdentity::current()).unwrap(),
        None
    );
}

#[test]
fn issue_1972_a_manifest_is_active_only_in_a_build_that_may_fetch() {
    let text = line(HASH_A, "CLAUDE.md");
    let active = ReleaseManifest::from_text(&text, &build(true))
        .unwrap()
        .expect("active");
    assert_eq!(active.version, BINARY);
    assert_eq!(active.files.files.len(), 1);
    assert_eq!(
        ReleaseManifest::from_text(&text, &build(false)).unwrap(),
        None
    );
    // Not even parsed in a build that may not fetch.
    assert_eq!(
        ReleaseManifest::from_text("junk", &build(false)).unwrap(),
        None
    );
}

#[test]
fn issue_1972_a_malformed_active_manifest_is_an_error_not_none() {
    let err = ReleaseManifest::from_text("junk\n", &build(true)).unwrap_err();
    assert_eq!(err.line, 1);
    let shown = anyhow::Error::new(err.clone()).to_string();
    assert!(
        shown.contains("the release manifest embedded in this binary is malformed (line 1:"),
        "{shown}"
    );
    let as_anyhow = anyhow::Error::new(err);
    assert!(
        as_anyhow
            .downcast_ref::<super::super::tag_fetch::SourceFetchFailed>()
            .is_none(),
        "self-heal must not keep the old bundle for a malformed manifest"
    );
    assert!(
        as_anyhow
            .downcast_ref::<MalformedReleaseManifest>()
            .is_some()
    );
}

#[test]
fn issue_1972_whitespace_only_text_is_parsed_not_ignored() {
    assert!(ReleaseManifest::from_text("\n", &build(true)).is_err());
    assert!(ReleaseManifest::from_text(" ", &build(true)).is_err());
}

// ── Rendering (D15, D16) ──

fn many(prefix: &str, n: usize) -> Vec<String> {
    (0..n).map(|i| format!("{prefix}{i:02}")).collect()
}

#[test]
fn issue_1972_release_mismatch_lists_at_most_20_paths_per_category() {
    let diff = ManifestDiff {
        missing: many("m", 25),
        extra: vec!["x".to_string()],
        changed: Vec::new(),
    };
    let text = render_release_mismatch("the tree", BINARY, &diff);
    assert!(
        text.starts_with(&format!(
            "the tree does not match the release v{BINARY} manifest embedded in this binary \
             (25 missing, 1 extra, 0 changed):"
        )),
        "{text}"
    );
    assert_eq!(text.matches("\n  missing: m").count(), 20);
    assert!(text.contains("\n  missing: and 5 more"), "{text}");
    assert!(text.contains("\n  extra: x"), "{text}");
}

#[test]
fn issue_1972_rendered_paths_are_sanitised() {
    let diff = ManifestDiff {
        extra: vec!["evil\u{1b}[31m\nline".to_string()],
        ..ManifestDiff::default()
    };
    let text = render_release_mismatch("s", BINARY, &diff);
    assert!(only_newlines_are_control(&text), "{text:?}");
    assert_eq!(
        text.lines().count(),
        2,
        "a path must not add a line: {text:?}"
    );

    let unreadable = ManifestFinding::Unreadable("failed\u{1b}\nto read".to_string());
    let text = unreadable.render("s", BINARY);
    assert!(only_newlines_are_control(&text), "{text:?}");
    assert!(!text.contains('\n'), "{text:?}");
}

#[test]
fn issue_1972_render_mismatch_is_unchanged() {
    let diff = ManifestDiff {
        missing: vec!["a".to_string()],
        extra: vec!["b".to_string()],
        changed: vec!["c".to_string()],
    };
    assert_eq!(
        render_mismatch(Path::new("/src"), &diff),
        "the staged bundle does not match its source /src (1 missing, 1 extra, 1 changed):\n  \
         missing: a\n  extra: b\n  changed: c"
    );
}

#[test]
fn issue_1972_compare_counts_from_the_release_side() {
    let release = BundleManifest {
        files: BTreeMap::from([
            ("gone".to_string(), HASH_A.to_string()),
            ("same".to_string(), HASH_A.to_string()),
            ("edited".to_string(), HASH_A.to_string()),
        ]),
    };
    let actual = BundleManifest {
        files: BTreeMap::from([
            ("same".to_string(), HASH_A.to_string()),
            ("edited".to_string(), HASH_B.to_string()),
            ("new".to_string(), HASH_A.to_string()),
        ]),
    };
    let diff = compare(&release, &actual);
    assert_eq!(diff.missing, ["gone"]);
    assert_eq!(diff.extra, ["new"]);
    assert_eq!(diff.changed, ["edited"]);
}

// ── Hashing ──

#[cfg(unix)]
#[test]
fn issue_1972_hashing_a_fifo_fails_promptly_instead_of_blocking() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let fifo = temp.path().join("pipe");
    let status = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap();
    assert!(status.success());
    let started = std::time::Instant::now();
    let err = sha256_of_file(&fifo).expect_err("a FIFO is not a regular file");
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    assert!(err.to_string().contains("not a regular file"), "{err:#}");
}

#[test]
fn issue_1972_hashing_a_directory_fails() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    assert!(sha256_of_file(temp.path()).is_err());
}

#[test]
fn issue_1972_hashing_a_regular_file_is_unchanged() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let file = temp.path().join("f");
    std::fs::write(&file, "abc").unwrap();
    assert_eq!(
        sha256_of_file(&file).unwrap(),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}
