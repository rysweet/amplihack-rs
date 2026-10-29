//! Issue #1592: amplihack remembers the profile root in
//! `~/.amplihack/profile-root`, so `amplihack profile ...` and the rotator
//! find the profiles when `AMPLIHACK_CLAUDE_PROFILE_ROOT` is not set. The
//! variable still wins when set and non-empty, and a running rotator keeps
//! the root it started with.
//!
//! Every command runs with `env_clear()` and a temporary HOME (see
//! `support/profile_rotate.rs`), so nothing touches the real `~/.claude`,
//! `~/.claude-profiles` or `~/.amplihack`.

#![cfg(unix)]

use crate::support::profile_rotate::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Output;

const ROOT_ENV: &str = "AMPLIHACK_CLAUDE_PROFILE_ROOT";
const SAVED_HINT: &str =
    "hint: fix or remove ~/.amplihack/profile-root, or run: amplihack profile root <absolute dir>";

fn saved_file(f: &Fixture) -> PathBuf {
    f.home.join(".amplihack").join("profile-root")
}

fn other_root(f: &Fixture) -> PathBuf {
    f.home.join("other-profiles")
}

fn ok_no_env(f: &Fixture, args: &[&str]) -> Output {
    let out = f.cmd_no_root_env(args).output().unwrap();
    assert_no_secrets(&out);
    assert!(
        out.status.success(),
        "`amplihack {}` failed: {}\nstdout: {}\nstderr: {}",
        args.join(" "),
        out.status,
        text(&out.stdout),
        text(&out.stderr),
    );
    out
}

fn fail_no_env(f: &Fixture, args: &[&str]) -> Output {
    let out = f.cmd_no_root_env(args).output().unwrap();
    assert!(
        !out.status.success(),
        "`amplihack {}` should fail\nstdout: {}",
        args.join(" "),
        text(&out.stdout)
    );
    out
}

fn first_line(out: &Output) -> String {
    text(&out.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .to_string()
}

fn same_path(a: &str, b: &Path) -> bool {
    Path::new(a).canonicalize().ok().as_deref() == b.canonicalize().ok().as_deref()
}

/// The first `add` (in `Fixture::new`, run with the variable set) saved the
/// root it used, mode 0600, one path and one newline.
#[test]
fn first_add_records_the_root_it_used() {
    let f = Fixture::new();
    f.assert_isolated();
    let raw = fs::read_to_string(saved_file(&f)).unwrap();
    assert!(raw.ends_with('\n') && !raw.ends_with("\n\n"), "{raw:?}");
    assert!(same_path(raw.trim_end(), &f.root), "{raw:?}");
    let mode = fs::metadata(saved_file(&f)).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);

    fs::remove_file(saved_file(&f)).unwrap();
    let out = f.ok(&["profile", "add", "fourth"]);
    let want = format!(
        "recorded profile root {} in {}",
        f.root.display(),
        saved_file(&f).display()
    );
    assert!(text(&out.stdout).contains(&want), "{}", text(&out.stdout));

    // A later add, even under another root, leaves the saved root alone.
    let before = fs::read(saved_file(&f)).unwrap();
    let out = f
        .cmd(&["profile", "add", "fifth"])
        .env(ROOT_ENV, other_root(&f))
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(!text(&out.stdout).contains("recorded profile root"));
    assert_eq!(fs::read(saved_file(&f)).unwrap(), before);
}

#[test]
fn list_without_the_variable_uses_the_saved_root() {
    let f = Fixture::new();
    let out = ok_no_env(&f, &["profile", "list"]);
    let head = first_line(&out);
    let path = head
        .strip_prefix("root: ")
        .and_then(|r| r.strip_suffix(" (source: config)"))
        .unwrap_or_else(|| panic!("root line: {head:?}"));
    assert!(same_path(path, &f.root), "{head}");
    let rest: Vec<String> = text(&out.stdout)
        .lines()
        .skip(1)
        .map(String::from)
        .collect();
    for p in PROFILES {
        assert!(
            // `<mark> <name>  <kind>` (issue #1700 added the KIND column).
            rest.iter()
                .any(|l| l.trim_start_matches(['*', ' ']).split_whitespace().next() == Some(p)),
            "{rest:?}"
        );
    }
    let root = ok_no_env(&f, &["profile", "root"]);
    assert_eq!(first_line(&root), head);
}

#[test]
fn the_variable_wins_over_the_saved_root() {
    let f = Fixture::new();
    let out = f.ok(&["profile", "list"]);
    assert_eq!(
        first_line(&out),
        format!("root: {} (source: env)", f.root.display())
    );
    let other = other_root(&f);
    let out = f
        .cmd(&["profile", "root"])
        .env(ROOT_ENV, &other)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(
        text(&out.stdout),
        format!("root: {} (source: env)\n", other.display())
    );
}

#[test]
fn an_empty_variable_falls_through_to_the_saved_root() {
    let f = Fixture::new();
    let out = f
        .cmd_no_root_env(&["profile", "list"])
        .env(ROOT_ENV, "")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(
        first_line(&out).ends_with(" (source: config)"),
        "{}",
        text(&out.stdout)
    );
}

#[test]
fn default_root_when_nothing_is_saved() {
    let f = Fixture::new();
    fs::remove_file(saved_file(&f)).unwrap();
    let default = f.home.join(".amplihack").join("claude-profiles");
    let out = ok_no_env(&f, &["profile", "root"]);
    assert_eq!(
        text(&out.stdout),
        format!("root: {} (source: default)\n", default.display())
    );
    let out = ok_no_env(&f, &["profile", "list"]);
    assert_eq!(
        text(&out.stdout),
        format!(
            "root: {} (source: default)\nno profiles; create one with `amplihack profile add <name>`\n",
            default.display()
        )
    );
    assert!(!saved_file(&f).exists(), "list never saves the root");
}

#[test]
fn root_dir_saves_and_list_follows_it() {
    let f = Fixture::new();
    let other = other_root(&f);
    let arg = format!("{}/", other.display());
    let out = ok_no_env(&f, &["profile", "root", &arg]);
    assert_eq!(
        text(&out.stdout),
        format!(
            "saved profile root: {} ({})\n",
            other.display(),
            saved_file(&f).display()
        )
    );
    assert!(
        !text(&out.stderr).contains("note:"),
        "{}",
        text(&out.stderr)
    );
    assert_eq!(
        fs::read_to_string(saved_file(&f)).unwrap(),
        format!("{}\n", other.display())
    );
    assert!(
        !other.exists(),
        "`profile root <dir>` never creates the directory"
    );
    let out = ok_no_env(&f, &["profile", "list"]);
    assert_eq!(
        first_line(&out),
        format!("root: {} (source: config)", other.display())
    );
    ok_no_env(&f, &["profile", "add", "moved"]);
    assert!(other.join("moved").is_dir());
}

#[test]
fn root_dir_under_the_variable_saves_and_prints_a_note() {
    let f = Fixture::new();
    let other = other_root(&f);
    let out = f.ok(&["profile", "root", other.to_str().unwrap()]);
    let note = format!(
        "note: AMPLIHACK_CLAUDE_PROFILE_ROOT={} overrides the saved root in this shell",
        f.root.display()
    );
    assert!(text(&out.stderr).contains(&note), "{}", text(&out.stderr));
    assert_eq!(
        fs::read_to_string(saved_file(&f)).unwrap(),
        format!("{}\n", other.display())
    );
}

/// A bad argument exits non-zero with `InvalidRoot` and writes nothing.
#[test]
fn a_bad_root_argument_is_refused_and_nothing_is_written() {
    let f = Fixture::new();
    fs::remove_file(saved_file(&f)).unwrap();
    for bad in ["relative/path", "~/x", "/srv/a/../b", ""] {
        let out = fail_no_env(&f, &["profile", "root", bad]);
        let err = text(&out.stderr);
        assert!(err.contains("invalid profile root"), "{bad:?}: {err}");
        assert!(
            err.contains("hint: use an absolute path: set AMPLIHACK_CLAUDE_PROFILE_ROOT to one, or run: amplihack profile root <absolute dir>"),
            "{bad:?}: {err}"
        );
        assert!(!saved_file(&f).exists(), "{bad:?} wrote the file");
    }
    let inside = f.home.join(".claude").join("profiles");
    fail_no_env(&f, &["profile", "root", inside.to_str().unwrap()]);
    let link = f.home.join("link-root");
    std::os::unix::fs::symlink(&f.root, &link).unwrap();
    fail_no_env(&f, &["profile", "root", link.to_str().unwrap()]);
    assert!(!saved_file(&f).exists());
}

/// A damaged saved root fails closed with a hint naming the file; the
/// variable bypasses it, and `profile root <dir>` repairs it.
#[test]
fn a_corrupt_saved_root_fails_closed_and_root_dir_repairs_it() {
    let f = Fixture::new();
    private_file(&saved_file(&f), "SENTINEL/relative\n");
    let want = format!(
        "saved profile root in {:?} is invalid: not an absolute path",
        saved_file(&f).display().to_string()
    );
    for args in [
        &["profile", "list"][..],
        &["profile", "root"],
        &["profile", "add", "never"],
    ] {
        let out = fail_no_env(&f, args);
        let err = text(&out.stderr);
        assert!(err.contains(&want), "{args:?}: {err}");
        assert!(err.contains(SAVED_HINT), "{args:?}: {err}");
        assert_no_secrets(&out);
    }
    assert!(!f.root.join("never").exists());
    f.ok(&["profile", "list"]);

    ok_no_env(&f, &["profile", "root", f.root.to_str().unwrap()]);
    let out = ok_no_env(&f, &["profile", "list"]);
    assert_eq!(
        first_line(&out),
        format!("root: {} (source: config)", f.root.display())
    );
}

#[test]
fn a_saved_root_others_can_write_is_refused() {
    let f = Fixture::new();
    fs::set_permissions(saved_file(&f), fs::Permissions::from_mode(0o666)).unwrap();
    let out = fail_no_env(&f, &["profile", "list"]);
    let err = text(&out.stderr);
    assert!(err.contains("is invalid: writable by others"), "{err}");
}

/// A launch passes the root to its children only when the root came from the
/// variable. An inherited empty variable is passed through unchanged, so the
/// child reads the saved root itself.
#[test]
fn a_launch_pins_the_root_only_when_it_came_from_the_variable() {
    let f = Fixture::new();
    private_file(&saved_file(&f), &format!("{}\n", f.root.display()));
    let claude = f.tools.join("claude");
    crate::support::exec_stub::write_executable(
        &claude,
        "#!/bin/sh\n[ \"$1\" = --version ] && { echo '2.1.301 (Claude Code)'; exit 0; }\n\
         printf '%s\\n' \"${AMPLIHACK_CLAUDE_PROFILE_ROOT-<unset>}\" > \"$RECORD_DIR/root.env\"\n",
        0o755,
    );
    let record = f.records.join("root.env");
    let args = ["profile", "run", "spare", "--", "-p", "hi"];
    let mut cases = vec![
        (f.cmd_no_root_env(&args), "<unset>".to_string()),
        (f.cmd(&args), f.root.display().to_string()),
    ];
    let mut empty = f.cmd_no_root_env(&args);
    empty.env(ROOT_ENV, "");
    cases.push((empty, String::new()));
    for (mut cmd, want) in cases {
        let _ = fs::remove_file(&record);
        let out = cmd.output().unwrap();
        assert!(out.status.success(), "{}", text(&out.stderr));
        let got = fs::read_to_string(&record).unwrap();
        assert_eq!(got, format!("{want}\n"), "want {want:?}");
    }
}
