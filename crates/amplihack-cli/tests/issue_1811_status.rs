//! Issue #1811: the capacity governor's flags and `governor status`,
//! through the real `amplihack` binary. See `issue_1811_fixture` for the
//! isolation rules and docs/reference/capacity-governor.md for the contract.

#![cfg(unix)]

use crate::issue_1811_fixture::{Fixture, describe, text};
use std::fs;

#[test]
fn invalid_workstream_flag_is_a_usage_error() {
    let f = Fixture::new();
    let out = f.run(&["claude", "--workstream", "bad/id", "--", "-p", "hi"]);
    assert_eq!(out.status.code(), Some(2), "{}", describe(&out));
    assert_eq!(f.claude_runs(), 0);
    let out = f.run(&["claude", "--governor-wait", "1d", "--", "-p", "hi"]);
    assert_eq!(out.status.code(), Some(2), "{}", describe(&out));
}

#[test]
fn flags_after_the_separator_are_forwarded_to_claude() {
    let f = Fixture::new();
    let out = f.run(&[
        "claude",
        "--governor-class",
        "review",
        "--",
        "--workstream",
        "x",
    ]);
    assert!(out.status.success(), "{}", describe(&out));
    let args: String = fs::read_dir(&f.records)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("args.")
        })
        .map(|p| fs::read_to_string(p).unwrap())
        .collect();
    assert!(args.contains("--workstream\nx\n"), "{args}");
    assert!(!args.contains("--governor-class"), "{args}");
}

#[test]
fn status_json_has_the_documented_schema_and_writes_nothing() {
    let f = Fixture::new();
    f.readings(&[("alpha", 62.0, 71.0), ("beta", -1.0, 38.0)]);
    let before = f.tree(f.tmp.path());
    let out = f.run(&["governor", "status", "--json"]);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        f.tree(f.tmp.path()),
        before,
        "status must not create or change files"
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    for k in [
        "version",
        "generated_at",
        "enabled",
        "limit",
        "max",
        "profiles",
        "reserve",
        "slots",
        "queue",
        "workstreams",
        "fail_open_reason",
    ] {
        assert!(keys.contains(&k), "missing {k}: {v}");
    }
    assert_eq!(keys.len(), 11, "{keys:?}");
    let beta = v["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "beta")
        .unwrap();
    assert_eq!(beta["session"], serde_json::Value::Null);
    let table = f.run(&["governor", "status"]);
    assert!(
        text(&table.stdout).contains("no session limit"),
        "{}",
        describe(&table)
    );
}

#[test]
fn status_with_only_home_set_creates_nothing() {
    let f = Fixture::new();
    let before = f.tree(f.tmp.path());
    let out = f
        .cmd(&["governor", "status", "--json"])
        .env_remove("AMPLIHACK_CLAUDE_PROFILE_ROOT")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.tree(f.tmp.path()), before);
}

#[test]
fn eighty_is_not_used_by_any_other_exit_path() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut stack = vec![src];
    let mut hits = Vec::new();
    while let Some(dir) = stack.pop() {
        for e in fs::read_dir(&dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                let t = fs::read_to_string(&p).unwrap();
                if t.contains("exit_error(80)") || t.contains("process::exit(80)") {
                    hits.push(p.display().to_string());
                }
            }
        }
    }
    assert!(
        hits.is_empty(),
        "use EXIT_GOVERNOR_DENIED, not a bare 80: {hits:?}"
    );
    let gov = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/commands/governor/mod.rs");
    let t = fs::read_to_string(gov).expect("governor/mod.rs");
    assert!(
        t.contains("EXIT_GOVERNOR_DENIED: i32 = 80"),
        "the constant is defined once"
    );
}
