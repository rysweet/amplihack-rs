//! Rust-only subprocess fixture compiled by issue_2053_stop_state_root.rs.
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::PathBuf;

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    assert_eq!(
        args,
        [
            "-p",
            "--permission-mode",
            "bypassPermissions",
            "--tools",
            "",
            "--no-session-persistence",
            "--setting-sources",
            "user",
        ]
    );
    let mut prompt = String::new();
    std::io::stdin().read_to_string(&mut prompt).unwrap();
    if let Ok(findings) = std::env::var("STOP_FIXTURE_UNIT_FINDINGS") {
        assert!(!prompt.trim().is_empty());
        let expected = PathBuf::from(std::env::var_os("STOP_FIXTURE_PROJECT").unwrap());
        assert_eq!(
            std::env::current_dir().unwrap(),
            expected.canonicalize().unwrap()
        );
        assert!(std::env::var_os("CLAUDECODE").is_none());
        println!("{findings}");
        return;
    }
    let expected = PathBuf::from(std::env::var_os("STOP_FIXTURE_PROJECT").unwrap());
    assert_eq!(
        std::env::current_dir().unwrap(),
        expected.canonicalize().unwrap()
    );
    assert!(std::env::var_os("CLAUDECODE").is_none());
    for required in [
        "fixture-template",
        "fixture-feedback-template",
        "fixture-preference",
        "fixture-task",
        &std::env::var("STOP_FIXTURE_EXTRA_PROMPT").unwrap(),
    ] {
        assert!(
            prompt.contains(required),
            "missing fixture prompt input: {required}"
        );
    }
    let evidence = PathBuf::from(std::env::var_os("STOP_FIXTURE_EVIDENCE").unwrap());
    assert!(
        !evidence.starts_with(expected),
        "evidence belongs outside the project"
    );
    writeln!(
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(evidence)
            .unwrap(),
        "invoked"
    )
    .unwrap();
    println!("{}", std::env::var("STOP_FIXTURE_FINDINGS").unwrap());
}
