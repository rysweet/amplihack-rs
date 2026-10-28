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
        "handfix.rs",
        "pin.rs",
        "prelude.rs",
        "switch.rs",
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

/// Issue #1693: hand-change recovery moves a login by rename only, so it
/// must name the login file only next to `lstat`-style checks and the move.
#[test]
fn handfix_moves_logins_only_through_the_shared_move() {
    let text = fs::read_to_string(rotate_dir().join("handfix.rs")).expect("rotate/handfix.rs");
    for f in [
        "fs::rename",
        "remove_file",
        "remove_dir",
        "fs::write",
        "hard_link",
    ] {
        assert!(
            !text
                .lines()
                .any(|l| l.split("//").next().unwrap_or_default().contains(f)),
            "handfix.rs calls `{f}`"
        );
    }
    assert!(
        text.contains("move_login"),
        "the login is moved with move_login"
    );
}

// --- Issue #1691: the rotation pin and `profile switch NAME` ----------------

fn source(name: &str) -> String {
    fs::read_to_string(rotate_dir().join(name)).unwrap_or_else(|e| panic!("rotate/{name}: {e}"))
}

/// The code lines (comments removed) of `fn <name>` in `text`, up to the
/// next top-level `fn` or the end of the file.
fn body_of(text: &str, name: &str) -> String {
    let start = text
        .find(&format!("fn {name}("))
        .unwrap_or_else(|| panic!("fn {name} not found"));
    let rest = &text[start..];
    let end = rest[1..]
        .find("\nfn ")
        .into_iter()
        .chain(rest[1..].find("\npub fn "))
        .chain(rest[1..].find("\npub(super) fn "))
        .chain(rest[1..].find("\npub(crate) fn "))
        .chain(rest[1..].find("\npub(in "))
        .chain(rest[1..].find("\n/// "))
        .min()
        .map_or(rest.len(), |i| i + 1);
    rest[..end]
        .lines()
        .map(|l| l.split("//").next().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n")
}

#[track_caller]
fn assert_before(body: &str, first: &str, then: &str, what: &str) {
    let a = body
        .find(first)
        .unwrap_or_else(|| panic!("{what}: `{first}` not found"));
    let b = body
        .find(then)
        .unwrap_or_else(|| panic!("{what}: `{then}` not found"));
    assert!(a < b, "{what}: `{first}` must come before `{then}`");
}

#[test]
fn the_pin_gate_runs_before_claude_is_looked_for() {
    // No measuring, and so no usage or OAuth call, while pinned.
    let cycle = body_of(&source("cycle.rs"), "run_cycle");
    assert_before(&cycle, "pin::gate", "resolve_claude_binary", "run_cycle");
    assert_before(&cycle, "pin::gate", "measure_and_decide", "run_cycle");
    let dry = body_of(&source("observe.rs"), "dry_run");
    assert_before(&dry, "pin::gate", "resolve_claude_binary", "dry_run");
    assert_before(&dry, "pin::gate", "measure_and_decide", "dry_run");
}

#[test]
fn run_cycle_shares_the_locked_prelude_with_switch() {
    let cycle = body_of(&source("cycle.rs"), "run_cycle");
    assert!(
        cycle.contains("locked_prelude"),
        "run_cycle uses the prelude"
    );
    assert!(
        !cycle.contains("recover_auto_switch"),
        "recovery lives in prelude.rs only, not a second copy in run_cycle"
    );
    let switch = source("switch.rs");
    assert!(switch.contains("locked_prelude"), "switch uses the prelude");
    assert!(
        switch.contains("swap::swap"),
        "switch reuses the rotator's swap"
    );
    for (file, text) in [("switch.rs", &switch), ("pin.rs", &source("pin.rs"))] {
        for f in [
            "exchange_logins",
            "renameat2",
            "RENAME_EXCHANGE",
            "move_login",
        ] {
            assert!(
                !text.contains(f),
                "{file}: a second swap implementation (`{f}`)"
            );
        }
    }
}

#[test]
fn switch_never_runs_claude_or_the_daemon() {
    let text = source("switch.rs");
    for f in [
        "resolve_claude_binary",
        "measure_and_decide",
        "measure::",
        "daemon::",
        "Command::new",
    ] {
        assert!(!text.contains(f), "switch.rs must not use `{f}`");
    }
}

#[test]
fn switch_writes_the_pin_and_pointer_before_dropping_the_lock() {
    let text: String = source("switch.rs")
        .lines()
        .map(|l| l.split("//").next().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");
    assert_before(&text, "swap::swap", "pin::write", "switch.rs");
    assert_before(&text, "pin::write", "pointer::write", "switch.rs");
    assert_before(&text, "pointer::write", "drop(lock)", "switch.rs");
    assert_before(&text, "drop(lock)", "ensure_for", "switch.rs");
}

#[test]
fn only_pin_rs_touches_the_pin_file() {
    for path in sources() {
        let file = path.file_name().unwrap().to_string_lossy().into_owned();
        let text = fs::read_to_string(&path).unwrap();
        for (i, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap_or_default();
            if file != "ledger.rs" {
                assert!(
                    !code.contains("join(\"pin\")"),
                    "{file}:{}: the pin path is built only in ledger.rs",
                    i + 1
                );
            }
            if file != "pin.rs" {
                assert!(
                    !code.contains("rs.pin"),
                    "{file}:{}: the pin file is read and written only in pin.rs",
                    i + 1
                );
            }
        }
    }
}

#[test]
fn pin_and_switch_never_remove_a_tree() {
    for file in ["pin.rs", "switch.rs", "prelude.rs"] {
        assert!(
            !source(file).contains("remove_dir_all"),
            "{file} must never call remove_dir_all"
        );
    }
}

#[test]
fn pin_rs_has_its_own_trusted_read() {
    let text = source("pin.rs");
    assert!(
        !text.contains("pointer::read"),
        "pin.rs reads with its own checks"
    );
    assert!(
        text.contains("read_bounded_nofollow"),
        "never through a symlink"
    );
    assert!(
        text.contains("parse_name"),
        "the name parser is shared with pointer.rs"
    );
}

#[test]
fn the_word_hold_is_not_an_identifier_in_pin_rs() {
    // `--hold` and `settings.hold` already mean upgrade damping (D2).
    let text = source("pin.rs");
    for (i, line) in text.lines().enumerate() {
        let code = line.split("//").next().unwrap_or_default();
        // String literals are prose; only identifiers count.
        let mut in_str = false;
        let mut ident = String::new();
        for c in code.chars().chain(std::iter::once(' ')) {
            if c == '"' {
                in_str = !in_str;
            }
            if !in_str && (c.is_ascii_alphanumeric() || c == '_') {
                ident.push(c);
                continue;
            }
            assert!(
                !ident.split('_').any(|w| w.eq_ignore_ascii_case("hold")),
                "pin.rs:{}: identifier `{ident}` uses `hold`",
                i + 1
            );
            ident.clear();
        }
    }
}

fn squeezed(body: &str) -> String {
    body.chars().filter(|c| !c.is_whitespace()).collect()
}

#[test]
fn a_held_cycle_records_no_measurement() {
    // D3/#1659: `measured_at` is `None` on the held path, so the limit-resume
    // hook never wakes sessions on a login nobody measured. The prefix also
    // matches the five-argument `record` of #1695 (`.., t, None, None)`).
    let cycle = squeezed(&body_of(&source("cycle.rs"), "run_cycle"));
    assert!(
        cycle.contains("CycleOutcome::Held,t,None"),
        "run_cycle must record a held cycle with measured_at None"
    );
}

#[test]
fn the_hold_warning_follows_the_success_lines_only() {
    // D4: warn after the switch is known to have succeeded, never on the
    // after-swap failure path or the default-only refusal.
    let held = body_of(&source("mod.rs"), "print_held");
    assert_before(&held, "Rotation held on", "warn_switch_hold", "print_held");
    assert_eq!(held.matches("warn_switch_hold").count(), 1);
    let facade = body_of(&source("mod.rs"), "switch_live");
    assert!(
        !facade.contains("warn_switch_hold"),
        "only through print_held"
    );
    assert_eq!(
        facade.matches("print_held").count(),
        2,
        "the moved and already-live paths"
    );
    assert_before(&facade, "DefaultOnly", "print_held", "rotate::switch_live");
    assert_before(&facade, "print_held", "Err(f)", "rotate::switch_live");
}

#[test]
fn the_hold_warning_takes_proc_as_a_seam() {
    // D4: tests pass a fake `/proc`; only the thin wrapper names the real one.
    let seam = body_of(&source("mod.rs"), "foreign_note_for");
    assert!(seam.contains("foreign_rotator"), "{seam}");
    assert!(seam.contains("FOREIGN_IGNORES_HOLD"), "{seam}");
    assert!(
        !seam.contains("\"/proc\""),
        "the seam must not hard-code /proc"
    );
}

/// Every rotate submodule is unix-only (they use `std::os::unix` and each
/// other). No CI target builds for non-unix any more, so a merge that drops
/// one `#[cfg(unix)]` would go unnoticed until someone builds elsewhere —
/// which is what happened to `mod pin;` in the merge for #1693. Check the
/// guard in the source instead.
#[test]
fn every_rotate_submodule_is_unix_only() {
    let text = fs::read_to_string(rotate_dir().join("mod.rs")).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let mut checked = 0;
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim_start();
        let t = t.strip_prefix("pub(super) ").unwrap_or(t);
        let t = t.strip_prefix("pub ").unwrap_or(t);
        if !(t.starts_with("mod ") && t.ends_with(';')) {
            continue;
        }
        // This scan only reads source text, so it runs on every platform.
        if t == "mod tests_source_scan;" {
            continue;
        }
        checked += 1;
        let guard = i.checked_sub(1).map(|j| lines[j].trim()).unwrap_or("");
        assert!(
            guard.starts_with("#[cfg(") && guard.contains("unix"),
            "rotate/mod.rs:{}: `{}` needs #[cfg(unix)] (or #[cfg(all(test, unix))]) on the line above",
            i + 1,
            line.trim()
        );
    }
    assert!(
        checked > 20,
        "found only {checked} mod lines in rotate/mod.rs"
    );
}
