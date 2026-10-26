//! Source guard for issue #1592: no rotate source file opens, reads, copies
//! or hashes a login file. Login files may only be passed to `lstat` and the
//! rename and exchange calls, so a line naming the login file next to any
//! read or copy call fails the build's tests.

use std::fs;
use std::path::{Path, PathBuf};

const FORBIDDEN: &[&str] = &[
    "File::open",
    "OpenOptions",
    "fs::read",
    "read_to_string",
    "read_to_end",
    "read_bounded",
    "fs::copy",
    "io::copy",
    "Sha256",
    "hash(",
];

fn rotate_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/commands/profile/rotate")
}

fn sources() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(rotate_dir())
        .expect("rotate/ exists")
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.extension().is_some_and(|e| e == "rs")
                && !p
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("tests_")
        })
        .collect();
    v.sort();
    v
}

#[test]
fn rotate_has_the_planned_files() {
    let names: Vec<String> = sources()
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    for want in [
        "mod.rs",
        "settings.rs",
        "decide.rs",
        "measure.rs",
        "ledger.rs",
        "identify.rs",
        "swap.rs",
        "cycle.rs",
        "log.rs",
        "daemon.rs",
        "damp.rs",
        "autostart.rs",
    ] {
        assert!(names.iter().any(|n| n == want), "missing rotate/{want}");
    }
}

#[test]
fn no_rotate_file_reads_or_copies_a_login() {
    for path in sources() {
        let text = fs::read_to_string(&path).unwrap();
        for (i, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap_or_default();
            if !(code.contains("CREDS") || code.contains(".credentials.json")) {
                continue;
            }
            for f in FORBIDDEN {
                assert!(
                    !code.contains(f),
                    "{}:{}: login file next to `{f}`: {line}",
                    path.display(),
                    i + 1
                );
            }
        }
    }
}

#[test]
fn rotate_files_stay_within_the_line_budget() {
    for path in sources() {
        let lines = fs::read_to_string(&path).unwrap().lines().count();
        assert!(lines <= 400, "{} has {lines} lines", path.display());
    }
}

#[test]
fn unsafe_blocks_carry_a_safety_comment() {
    for path in sources() {
        let text = fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            if !line.contains("unsafe {") && !line.contains("unsafe{") {
                continue;
            }
            let window = &lines[i.saturating_sub(4)..=i];
            assert!(
                window.iter().any(|l| l.contains("SAFETY:")),
                "{}:{}: unsafe without a SAFETY comment",
                path.display(),
                i + 1
            );
        }
    }
}
