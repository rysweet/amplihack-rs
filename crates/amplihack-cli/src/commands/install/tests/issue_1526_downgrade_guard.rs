//! Issue #1526 — an implicit re-stage must never deploy an older build over a
//! newer install.
//!
//! A `cargo install --git` build of the v0.18.39 commit reported `0.18.0`, and
//! a plain `amplihack recipe run` from it re-staged over the v0.18.39 install:
//! the binaries in `~/.local/bin` were replaced and `~/.claude/settings.json`
//! was rewritten, with no install command anywhere in sight.
//!
//! `downgrade_guard` is the one place that decides "is this an implicit
//! downgrade?" and the one place that says so. These tests pin its decision
//! table, its exact refusal line, its lack of side effects, the launch
//! bootstrap entry point (`ensure_framework_installed_with`), and the fact
//! that every implicit trigger consults it and fails closed.

use super::super::downgrade_guard::{is_implicit_downgrade, warn_if_implicit_downgrade};
use super::*;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// A stamp above every real release. Whatever `crate::VERSION` is in this
/// build — `<pkg>-dev` unstamped, or a release tag — it is lower.
const NEWER: &str = "9999.0.0";

/// Holds the crate-wide env lock and points `HOME` at a temp dir.
///
/// Field order is load-bearing: `HOME` is restored before the lock is
/// released, even on panic, so no other test can observe this test's HOME and
/// nothing here can resolve to the developer's real one.
struct TempHome {
    _home: crate::test_support::HomeGuard,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl TempHome {
    fn set(path: &Path) -> Self {
        let lock = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = crate::test_support::HomeGuard::set(path);
        Self {
            _home: home,
            _lock: lock,
        }
    }
}

/// The exact D8 refusal line, byte for byte, including the trailing newline.
fn expected_refusal_line(running: &str, stamp: &str, home: &Path) -> String {
    let exe = std::env::current_exe().expect("the test binary has a current_exe");
    let installed = home
        .join(".local")
        .join("bin")
        .join(crate::path_conflicts::binary_filename("amplihack"));
    format!(
        "amplihack: refusing implicit re-stage: running {exe:?} is v{running}, older than \
         installed {installed:?} v{stamp}; nothing was changed. Run 'amplihack install' to \
         deploy this build explicitly.\n"
    )
}

/// Every regular file under `root`, keyed by path relative to `root`, with its
/// bytes. Symlinks are recorded by target rather than followed.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            let meta = fs::symlink_metadata(&path).unwrap();
            let rel = path.strip_prefix(root).unwrap().to_path_buf();
            if meta.file_type().is_symlink() {
                let target = fs::read_link(&path).unwrap();
                out.insert(rel, target.to_string_lossy().into_owned().into_bytes());
            } else if meta.is_dir() {
                out.insert(rel, b"<dir>".to_vec());
                walk(root, &path, out);
            } else {
                out.insert(rel, fs::read(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

/// Settings and install-time backups that a re-stage writes. A refusal must
/// leave none behind. Searched recursively: the install backup really lives at
/// `~/.amplihack/.claude/runtime/sessions/`, not where the name suggests.
fn backups_under(root: &Path) -> Vec<PathBuf> {
    snapshot(root)
        .into_keys()
        .filter(|rel| {
            let name = rel
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            name.starts_with("settings.json.backup.")
                || (name.starts_with("install_") && name.ends_with("_backup.json"))
        })
        .collect()
}

// ── Decision table (D2) ──

#[test]
fn is_implicit_downgrade_table() {
    let refuse: &[(Option<&str>, &str)] = &[
        // Valid stamp, unparseable running version: fail safe.
        (Some(NEWER), "not-a-version"),
        (Some(NEWER), ""),
        // Valid stamp, running lower.
        (Some("0.18.39"), "0.18.0"),    // the #1526 reporter's exact pair
        (Some("0.18.0"), "0.18.0-dev"), // a -dev build sits below its release
        (Some("0.18.39"), "0.18.9"),    // numeric, not lexical: 9 < 39
        (Some("1.0.0-rc.10"), "1.0.0-rc.2"), // numeric prerelease identifiers
        (Some(NEWER), "0.18.0-dev"),    // the regression-test stamp
    ];
    let proceed: &[(Option<&str>, &str)] = &[
        // No usable stamp: the #502 first-install / repair path.
        (None, "0.18.0-dev"),
        (None, "not-a-version"),
        // Passes the stamp regex, fails strict semver: same as malformed.
        (Some("01.2.3"), "0.0.1"),    // leading zero
        (Some("1.2.3-a_b"), "0.0.1"), // underscore in prerelease
        // C3: Unicode the stamp regex's `\d` / `\w` accept but semver rejects.
        (Some("١.2.3"), "0.0.1"),            // Arabic-Indic digit
        (Some("1.2.3-a\u{200d}b"), "0.0.1"), // zero-width joiner
        // Equal precedence: existing behaviour, including the #1271 repair.
        (Some("0.18.0"), "0.18.0"),
        (Some("0.18.0"), "0.18.0+snapshot.abc"), // build metadata is ignored
        (Some("1.0.0+zzz"), "1.0.0+aaa"),        // cmp_precedence, never Ord
        // Running higher: the upgrade re-stage (#488, #885, #911).
        (Some("0.8.55"), "0.18.0-dev"), // a string compare would refuse this
        (Some("0.18.9"), "0.18.39"),    // numeric, not lexical: 39 > 9
        (Some("0.18.0-dev"), "0.18.0"), // a release over its own dev build
        (Some("1.0.0-rc.2"), "1.0.0-rc.10"), // numeric prerelease identifiers
    ];

    let rows = refuse
        .iter()
        .map(|(stamp, running)| (*stamp, *running, true))
        .chain(
            proceed
                .iter()
                .map(|(stamp, running)| (*stamp, *running, false)),
        );
    let failures: Vec<String> = rows
        .filter_map(|(stamp, running, want)| {
            let got = is_implicit_downgrade(stamp, running);
            (got != want)
                .then(|| format!("stamp={stamp:?} running={running:?}: want {want}, got {got}"))
        })
        .collect();
    assert!(
        failures.is_empty(),
        "D2 decision-table violations:\n{}",
        failures.join("\n")
    );
}

// ── The refusal line (D8) ──

#[test]
fn refusal_writes_exactly_the_d8_line_and_touches_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let _home = TempHome::set(tmp.path());

    let mut buf = Vec::new();
    let refused = warn_if_implicit_downgrade(Some(NEWER), "0.18.0-dev", &mut buf)
        .expect("the guard must not error on a plain downgrade");

    assert!(refused, "a lower running version must be refused");
    assert_eq!(
        String::from_utf8(buf).unwrap(),
        expected_refusal_line("0.18.0-dev", NEWER, tmp.path()),
        "the refusal line is a spec, not a suggestion"
    );
    assert!(
        snapshot(tmp.path()).is_empty(),
        "the guard must not create anything under HOME (SR4/SR6); found {:?}",
        snapshot(tmp.path()).keys().collect::<Vec<_>>()
    );
}

#[test]
fn proceeding_returns_false_and_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let _home = TempHome::set(tmp.path());

    for (stamp, running) in [
        (None, "0.18.0-dev"),
        (Some("01.2.3"), "0.0.1"),
        (Some("0.18.0"), "0.18.0"),
        (Some("0.8.55"), "0.18.0-dev"),
    ] {
        let mut buf = Vec::new();
        let refused = warn_if_implicit_downgrade(stamp, running, &mut buf)
            .expect("the guard must not error when it proceeds");
        assert!(!refused, "stamp={stamp:?} running={running:?} must proceed");
        assert!(
            buf.is_empty(),
            "stamp={stamp:?} running={running:?} must stay silent; wrote {:?}",
            String::from_utf8_lossy(&buf)
        );
    }
}

/// C3 / SR5: paths are printed with `{:?}`, so a hostile HOME can neither forge
/// a second log line nor smuggle a terminal escape or a bidi override.
#[test]
fn refusal_line_is_single_line_with_hostile_home() {
    let tmp = tempfile::tempdir().unwrap();
    let hostile = tmp.path().join("evil\n\x1b[2J\u{202e}dir");
    let _home = TempHome::set(&hostile);

    let mut buf = Vec::new();
    let refused = warn_if_implicit_downgrade(Some(NEWER), "0.0.1", &mut buf)
        .expect("a hostile HOME must not make the guard error");

    assert!(refused);
    assert_eq!(
        buf.iter().filter(|b| **b == b'\n').count(),
        1,
        "exactly one newline, the terminator; got {:?}",
        String::from_utf8_lossy(&buf)
    );
    assert_eq!(buf.last(), Some(&b'\n'), "the line must be terminated");
    assert!(
        !buf.contains(&0x1b),
        "a raw ESC byte reached the terminal: {:?}",
        String::from_utf8_lossy(&buf)
    );
    let line = String::from_utf8(buf).unwrap();
    assert!(
        !line.contains('\u{202e}'),
        "a raw RIGHT-TO-LEFT OVERRIDE reached the terminal: {line:?}"
    );
    assert!(
        line.contains(r"evil\n\u{1b}[2J\u{202e}dir"),
        "the hostile path must still be named, escaped: {line:?}"
    );
}

// ── The launch bootstrap (D5) ──

/// `amplihack launch` reaches `ensure_framework_installed`, which used to
/// bootstrap and repair settings.json whenever `~/.amplihack/.claude` looked
/// incomplete — regardless of which binary was asking. With a newer stamp on
/// disk it must refuse before any of that.
#[test]
fn bootstrap_refuses_and_leaves_settings_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let _home = TempHome::set(home);

    version_stamp::write_installed_version(NEWER).unwrap();
    // A settings.json with no amplihack hooks: exactly what the bootstrap's
    // auto-repair would rewrite. No `~/.amplihack/.claude`: exactly what would
    // make it run a full install first.
    let settings = home.join(".claude").join("settings.json");
    fs::create_dir_all(settings.parent().unwrap()).unwrap();
    let sentinel: &[u8] = br#"{"sentinel":true}"#;
    fs::write(&settings, sentinel).unwrap();
    let before = snapshot(home);

    let mut buf = Vec::new();
    ensure_framework_installed_with(&mut buf).expect("a refusal is not a launch failure");

    assert_eq!(
        String::from_utf8(buf).unwrap(),
        expected_refusal_line(crate::VERSION, NEWER, home),
        "the bootstrap must emit exactly one D8 refusal line"
    );
    assert_eq!(
        fs::read(&settings).unwrap(),
        sentinel,
        "an implicit re-stage must never rewrite ~/.claude/settings.json"
    );
    assert_eq!(
        version_stamp::read_installed_version().unwrap().as_deref(),
        Some(NEWER),
        "a refusal must not rewrite the stamp"
    );
    assert!(
        !home.join(".amplihack").join(".claude").exists(),
        "a refusal must not stage framework assets"
    );
    assert!(
        !home
            .join(".claude")
            .join("commands")
            .join("amplihack")
            .exists(),
        "a refusal must not stage slash commands"
    );
    assert!(
        !home.join(".local").join("bin").exists(),
        "a refusal must not deploy binaries"
    );
    assert_eq!(
        backups_under(home),
        Vec::<PathBuf>::new(),
        "a refusal must not leave settings or install backups"
    );
    assert_eq!(
        snapshot(home),
        before,
        "nothing was changed — the refusal line says so"
    );
}

// ── Structure: one decision, every trigger guarded, fail closed (SR1–SR4) ──

/// Source with whole-line `//` comments removed, so prose that names a symbol
/// cannot satisfy or trip a structural check.
fn code_only(src: &str) -> String {
    src.lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// For every `warn_if_implicit_downgrade(...)` call in `src`, the first
/// non-whitespace character after its matching closing parenthesis.
fn guard_call_suffixes(src: &str) -> Vec<Option<char>> {
    const CALL: &str = "warn_if_implicit_downgrade(";
    let mut out = Vec::new();
    let mut rest = src;
    while let Some(at) = rest.find(CALL) {
        let args = &rest[at + CALL.len()..];
        let mut depth = 1usize;
        let close = args
            .char_indices()
            .find(|(_, c)| {
                match c {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    _ => {}
                }
                depth == 0
            })
            .map(|(i, _)| i)
            .expect("unbalanced parentheses after warn_if_implicit_downgrade(");
        let tail = &args[close + 1..];
        out.push(tail.chars().find(|c| !c.is_whitespace()));
        rest = tail;
    }
    out
}

#[test]
fn every_implicit_trigger_is_guarded_and_fails_closed() {
    let self_heal_src = include_str!("../../../self_heal.rs");
    let self_heal_prod = code_only(
        self_heal_src
            .split("#[cfg(test)]\nmod tests {")
            .next()
            .unwrap(),
    );
    let install_src = code_only(include_str!("../mod.rs"));

    // SR1: three guard sites — self-heal before the lock, self-heal under the
    // lock, and the launch bootstrap.
    let self_heal_calls = guard_call_suffixes(&self_heal_prod);
    assert_eq!(
        self_heal_calls.len(),
        2,
        "self_heal must guard both before the lock and under it (D3, D4)"
    );
    let install_calls = guard_call_suffixes(&install_src);
    assert_eq!(
        install_calls.len(),
        1,
        "ensure_framework_installed_with must guard exactly once (D5)"
    );

    // SR2: every call propagates with `?`. `.ok()`, `.unwrap_or(false)` or
    // `if let Ok(true)` would turn a guard error into "go ahead and install".
    for (file, suffixes) in [
        ("self_heal.rs", &self_heal_calls),
        ("install/mod.rs", &install_calls),
    ] {
        for suffix in suffixes {
            assert_eq!(
                *suffix,
                Some('?'),
                "{file}: every warn_if_implicit_downgrade(..) call must be followed by `?`"
            );
        }
    }

    // D7: one decision. No call site carries its own version comparison.
    for (file, src) in [
        ("self_heal.rs", &self_heal_prod),
        ("install/mod.rs", &install_src),
    ] {
        assert!(
            !src.contains("cmp_precedence") && !src.contains("is_implicit_downgrade("),
            "{file} must delegate the downgrade decision to downgrade_guard, not repeat it"
        );
    }
}

#[test]
fn the_guard_module_has_no_fs_process_or_tracing_side_effects() {
    let guard = code_only(include_str!("../downgrade_guard.rs"));
    for forbidden in [
        "std::fs",
        "std::process",
        "Command::new",
        "tracing::",
        "eprintln!",
        "println!",
    ] {
        assert!(
            !guard.contains(forbidden),
            "downgrade_guard.rs must not use `{forbidden}` (SR4/SR5): it decides and writes \
             one line to the injected writer, nothing else"
        );
    }
    assert!(
        guard.contains("cmp_precedence"),
        "the decision must use semver precedence (SR3)"
    );
}
