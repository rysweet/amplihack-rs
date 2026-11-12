//! Issue #1658 (FS-1, FS-2): source guards for the limit-resume hook. It
//! makes directories one level at a time, never changes an existing path's
//! mode and never prints.
//!
//! Issue #2085: the hook waits on events, never on a timer, so no source it
//! runs sleeps. It starts one program, `tmux`, from `keys.rs` only, with a
//! fixed argv and no shell, and it never opens a login file.
//!
//! The hook turns a panic into a silent exit 0 with `catch_unwind`, which
//! only works while release builds unwind.

use std::fs;
use std::path::{Path, PathBuf};

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn rust_sources(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).unwrap_or_else(|_| panic!("{} must exist", d.display())) {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs")
                && !p
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("tests")
            {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

fn scanned_dirs() -> [PathBuf; 2] {
    let ws = workspace();
    [
        ws.join("crates/amplihack-hooks/src/limit_resume"),
        ws.join("crates/amplihack-utils/src/rotator_status"),
    ]
}

/// Every source the hook runs: the two directories and `file_event.rs`.
fn scanned_sources() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = scanned_dirs()
        .iter()
        .flat_map(|d| rust_sources(d))
        .collect();
    let file_event = workspace().join("crates/amplihack-utils/src/file_event.rs");
    assert!(file_event.is_file(), "{} must exist", file_event.display());
    out.push(file_event);
    out
}

/// `(path, line number, code before any `//`)` for every scanned line.
fn code_lines() -> Vec<(PathBuf, usize, String)> {
    let mut out = Vec::new();
    for path in scanned_sources() {
        let text = fs::read_to_string(&path).unwrap();
        for (i, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap_or_default().to_string();
            out.push((path.clone(), i + 1, code));
        }
    }
    out
}

#[test]
fn hook_sources_never_use_create_dir_all_or_chmod() {
    // FS-1/FS-2: directories are made one level at a time and an existing
    // path's mode is never changed.
    for (path, n, code) in code_lines() {
        for f in [
            "create_dir_all",
            "set_permissions",
            "ensure_private_directory",
            "open_private_append",
            "println!",
            "print!(",
        ] {
            assert!(!code.contains(f), "{}:{n}: `{f}`: {code}", path.display());
        }
    }
}

#[test]
fn hook_sources_never_sleep_or_poll_on_a_timer() {
    // Timing rule (#2085): every wake is an event; no sleep, no interval.
    for (path, n, code) in code_lines() {
        for f in [
            "thread::sleep",
            "POLL_INTERVAL",
            "SLOW_POLL",
            "fn poll_interval",
        ] {
            assert!(!code.contains(f), "{}:{n}: `{f}`: {code}", path.display());
        }
    }
}

#[test]
fn only_keys_rs_starts_a_program_and_never_a_shell() {
    let keys = workspace().join("crates/amplihack-hooks/src/limit_resume/keys.rs");
    assert!(keys.is_file(), "{} must exist", keys.display());
    let mut spawns = 0;
    for (path, n, code) in code_lines() {
        if code.contains("Command::new(") {
            assert_eq!(path, keys, "{}:{n}: only keys.rs runs tmux", path.display());
            spawns += 1;
        }
        for f in [
            "\"sh\"",
            "\"bash\"",
            "\"-c\"",
            "/bin/sh",
            "\"xdotool\"",
            "\"screen\"",
        ] {
            assert!(!code.contains(f), "{}:{n}: `{f}`: {code}", path.display());
        }
    }
    assert!(spawns >= 1, "keys.rs runs tmux with Command::new");
    let text = fs::read_to_string(&keys).unwrap();
    assert!(text.contains("\"tmux\""), "the program is tmux");
    for arg in [
        "\"send-keys\"",
        "\"display-message\"",
        "\"#{pane_tty}\"",
        "\"continue\"",
    ] {
        assert!(text.contains(arg), "keys.rs has the fixed argument {arg}");
    }
}

#[test]
fn hook_sources_never_open_a_login_file() {
    for (path, n, code) in code_lines() {
        for f in [
            ".credentials.json",
            "oauthAccount",
            "accessToken",
            "refreshToken",
        ] {
            assert!(!code.contains(f), "{}:{n}: `{f}`: {code}", path.display());
        }
    }
}

#[test]
fn release_builds_unwind_so_catch_unwind_works() {
    let manifest = fs::read_to_string(workspace().join("Cargo.toml")).unwrap();
    let release = manifest
        .split("[profile.release]")
        .nth(1)
        .expect("workspace has a [profile.release]");
    let section = release.split("\n[").next().unwrap();
    assert!(
        section.contains("panic = \"unwind\""),
        "limit-resume relies on catch_unwind; keep panic = \"unwind\""
    );
}
