//! D1 (#1554, zero-A): the legacy `advertised-ref` parser accepts three
//! inputs that 918c accepted. The inputs are byte literals from
//! `crates/amplihack-cli/tests/support/d1_literals.rs`, written into each
//! test's own tempdir at run time. Before each call the test checks the
//! length, the full sha256 (by the system `sha256sum`, so no dependency is
//! added) and the D1 property. No fixture file is read.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::workflow_checkpoint_support::*;

mod literals {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../crates/amplihack-cli/tests/support/d1_literals.rs"
    ));
}
use literals::*;

fn sha256_hex(bytes: &[u8]) -> String {
    let mut child = Command::new("sha256sum")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("sha256sum");
    child.stdin.take().unwrap().write_all(bytes).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap()[..64].to_owned()
}

/// Check the literal, write it, re-read it, and check the bytes on disk.
fn write_d1(dir: &Path, name: &str) -> PathBuf {
    let (_, bytes, len, sha) = D1_INPUTS
        .iter()
        .find(|(n, ..)| *n == name)
        .copied()
        .unwrap_or_else(|| panic!("no D1 literal {name}"));
    assert_eq!(bytes.len(), len, "{name}: literal length");
    assert_eq!(sha256_hex(bytes), sha, "{name}: literal sha256");
    assert!(d1_property_holds(name, bytes), "{name}: D1 property");
    let path = dir.join(format!("{name}.advert"));
    fs::write(&path, bytes).unwrap();
    let on_disk = fs::read(&path).unwrap();
    assert_eq!(on_disk, bytes, "{name}: bytes on disk");
    path
}

fn advertised(sb: &Sandbox, saved: &Path, target: &str) -> std::process::Output {
    sb.checkpoint(
        "advertised-ref",
        [saved.as_os_str(), target.as_ref(), "0".as_ref()],
    )
    .output()
    .unwrap()
}

#[test]
fn d1_no_final_lf_literal_is_accepted_with_status_0() {
    let sb = Sandbox::new();
    let saved = write_d1(&sb.tmp(), "no-final-lf");
    let bytes = fs::read(&saved).unwrap();
    assert!(!bytes.ends_with(b"\n"), "the input must not end in LF");
    let out = advertised(&sb, &saved, "refs/heads/task");
    assert_eq!(status(&out), 0, "918c exit 0; stderr: {}", stderr(&out));
    assert!(out.stdout.is_empty() && out.stderr.is_empty());
}

#[test]
fn d1_crlf_literal_is_accepted_with_status_0() {
    let sb = Sandbox::new();
    let saved = write_d1(&sb.tmp(), "crlf");
    let bytes = fs::read(&saved).unwrap();
    assert!(
        bytes.windows(2).any(|w| w == b"\r\n"),
        "input must hold CRLF"
    );
    let out = advertised(&sb, &saved, "refs/heads/task");
    assert_eq!(status(&out), 0, "918c exit 0; stderr: {}", stderr(&out));
    assert!(out.stdout.is_empty() && out.stderr.is_empty());
}

#[test]
fn d1_mixed_oid_widths_literal_is_accepted_with_status_0() {
    let sb = Sandbox::new();
    let saved = write_d1(&sb.tmp(), "mixed-oid-widths");
    for target in ["refs/heads/a", "refs/heads/b"] {
        let out = advertised(&sb, &saved, target);
        assert_eq!(status(&out), 0, "{target}: stderr {}", stderr(&out));
        assert!(out.stdout.is_empty() && out.stderr.is_empty());
    }
}

#[test]
fn d1_leniency_is_not_vacuous_truncated_and_non_hex_oids_are_still_78() {
    // Negative companions: the parser is lenient only in the three D1 ways.
    let rows: [(&str, &[u8]); 3] = [
        ("mid-oid truncation", b"aaaaaaaaaaaaaaaaaaaa"),
        (
            "non-hex oid",
            b"gggggggggggggggggggggggggggggggggggggggg\trefs/heads/task\n",
        ),
        (
            "41-digit oid",
            b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\trefs/heads/task\n",
        ),
    ];
    for (what, body) in rows {
        let sb = Sandbox::new();
        let saved = sb.tmp().join("bad.advert");
        fs::write(&saved, body).unwrap();
        let out = advertised(&sb, &saved, "refs/heads/task");
        assert_eq!(status(&out), 78, "{what}: {}", stderr(&out));
    }
}

#[test]
fn d1_fixture_files_and_sha256sums_are_not_tracked() {
    // zero-A: the four staged files are removed from the index and deleted.
    let out = amplihack_git::command_in(repo_root())
        .args([
            "ls-files",
            "--cached",
            "--",
            "crates/amplihack-cli/tests/fixtures/workflow_checkpoint/legacy_parser",
            "crates/amplihack-cli/tests/fixtures/workflow_checkpoint/SHA256SUMS",
        ])
        .output()
        .expect("git ls-files");
    assert!(out.status.success());
    assert!(
        out.stdout.is_empty(),
        "still in the index: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let dir = repo_root().join("crates/amplihack-cli/tests/fixtures/workflow_checkpoint");
    for name in [
        "legacy_parser/no-final-lf.advert",
        "legacy_parser/crlf.advert",
        "legacy_parser/mixed-oid-widths.advert",
        "SHA256SUMS",
    ] {
        assert!(!dir.join(name).exists(), "{name} must be deleted");
    }
}
