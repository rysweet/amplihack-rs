//! Issue #1811 (TDD): the governor never moves an account, never spawns a
//! process and writes only inside `.governor.d`. A scan of the non-test
//! sources keeps it that way.
//!
//! Wire with `#[cfg(test)] mod tests_source_scan;` in `governor/mod.rs`.

use std::fs;
use std::path::{Path, PathBuf};

fn src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// Non-test governor sources, plus the read-only profile facade.
fn sources() -> Vec<(String, String)> {
    let dir = src().join("commands/governor");
    let mut out: Vec<(String, String)> = fs::read_dir(&dir)
        .expect("governor module directory")
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "rs"))
        .filter(|p| {
            !p.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("tests_")
        })
        .map(|p| {
            let name = p.file_name().unwrap().to_str().unwrap().to_string();
            (name, fs::read_to_string(&p).unwrap())
        })
        .collect();
    let view = src().join("commands/profile/governor_view.rs");
    out.push((
        "governor_view.rs".into(),
        fs::read_to_string(view).expect("profile/governor_view.rs"),
    ));
    out.sort();
    assert!(
        out.len() >= 10,
        "expected the governor module files, got {}",
        out.len()
    );
    out
}

/// Lines of code with `//` comments removed, so documentation may name a
/// forbidden thing.
fn code(text: &str) -> String {
    text.lines()
        .map(|l| l.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

const FORBIDDEN: &[&str] = &[
    ".credentials.json",
    "CLAUDE_JSON",
    "swap",
    "rotate::",
    "claude-rotator",
    "pointer::write",
    "pointer::clear",
    "save_ledger",
    "save_usage",
    "write_sub_json",
    "Command::new",
    "set_var",
    "remove_var",
    "remove_dir",
    "set_permissions",
    "remove_file",
];

#[test]
fn no_account_moves_processes_or_environment_changes() {
    for (name, text) in sources() {
        let code = code(&text);
        for bad in FORBIDDEN {
            assert!(!code.contains(bad), "{name} uses forbidden `{bad}`");
        }
    }
}

#[test]
fn rename_only_in_the_store() {
    for (name, text) in sources() {
        if name != "store.rs" {
            assert!(!code(&text).contains("rename("), "{name} renames a file");
        }
    }
}

#[test]
fn only_liveness_signals_and_only_with_signal_zero() {
    for (name, text) in sources() {
        let code = code(&text);
        let calls: Vec<&str> = code
            .match_indices("kill(")
            .map(|(i, _)| &code[i..])
            .collect();
        if name != "liveness.rs" {
            assert!(calls.is_empty(), "{name} sends a signal");
            continue;
        }
        assert_eq!(calls.len(), 1, "liveness.rs must have one kill call");
        let args: String = calls[0].chars().take_while(|c| *c != ')').collect();
        assert!(
            args.trim_end().ends_with(", 0"),
            "kill must use signal 0: {args}"
        );
        assert_eq!(
            code.matches("unsafe").count(),
            1,
            "one unsafe block in liveness.rs"
        );
        assert!(
            text.contains("SAFETY:"),
            "the unsafe block needs a SAFETY comment"
        );
    }
}

#[test]
fn writes_go_only_into_the_governor_dir() {
    for (name, text) in sources() {
        let code = code(&text);
        if name != "store.rs" {
            for w in [
                "write_private_atomic",
                "fs::write",
                "File::create",
                "OpenOptions",
            ] {
                assert!(
                    !code.contains(w),
                    "{name} writes a file (`{w}`); only store.rs may"
                );
            }
            continue;
        }
        for (i, _) in code.match_indices("write_private_atomic(") {
            let call: String = code[i..].chars().take_while(|c| *c != ',').collect();
            assert!(
                call.contains("gov_dir") || call.contains("self.dir"),
                "write_private_atomic must target the .governor.d directory: {call}"
            );
        }
    }
}

#[test]
fn admission_result_is_never_discarded() {
    let mut stack = vec![src()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let p = entry.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs")
                && !p.ends_with("governor/tests_source_scan.rs")
            {
                let text = fs::read_to_string(&p).unwrap();
                assert!(
                    !text.contains("let _ = governor::admit") && !text.contains("let _ = admit("),
                    "{} drops the slot guard at once",
                    p.display()
                );
            }
        }
    }
}

#[test]
fn every_hook_keeps_the_guard_in_scope() {
    let hooks = ["commands/mod.rs", "commands/recipe/run/mod.rs"];
    let mut total = 0;
    for rel in hooks {
        let text = fs::read_to_string(src().join(rel)).unwrap();
        total += text.matches("let _slot = governor::admit(").count();
    }
    assert_eq!(total, 3, "launch, claude and recipe run each bind `_slot`");
}
