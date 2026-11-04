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
        // Issue #1870: the freshness gate and swap-back damping.
        "fresh.rs",
        "fresh_msg.rs",
        "swap_back.rs",
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
    // Issue #1844: `claude` is looked up lazily inside measuring, so the
    // gate must come before `measure_and_decide`, and the cycle itself never
    // looks it up.
    let cycle = body_of(&source("cycle.rs"), "run_cycle");
    assert!(!cycle.contains("resolve_claude_binary"), "run_cycle");
    assert_before(&cycle, "pin::gate", "measure_and_decide", "run_cycle");
    let dry = body_of(&source("dry_run.rs"), "dry_run");
    assert!(!dry.contains("resolve_claude_binary"), "dry_run");
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
    // The held path is `held::held_cycle`, called by `run_cycle`.
    assert!(squeezed(&body_of(&source("cycle.rs"), "run_cycle")).contains("held_cycle("));
    let held = squeezed(&body_of(&source("held.rs"), "held_cycle"));
    assert!(
        held.contains("CycleOutcome::Held,t,None"),
        "held_cycle must record a held cycle with measured_at None"
    );
    // Its spend-limit check records a block and nothing else: no cycle
    // record, so no `measured_at`, and no swap.
    let watch = body_of(&source("held.rs"), "watch_pinned_spend");
    for f in ["save(", "swap::", "last_cycle"] {
        assert!(!watch.contains(f), "watch_pinned_spend must not call `{f}`");
    }
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
        // `target_os = "linux"` is a narrower guard than `unix` (every Linux
        // target is a unix one), used by tests that read Linux-only state.
        assert!(
            guard.starts_with("#[cfg(")
                && (guard.contains("unix") || guard.contains("target_os = \"linux\"")),
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

// --- Issue #1886: recovery identifies by the credential, never .claude.json --

/// The code lines of `text`: every line that does not start with `//`
/// (after indentation), with any trailing `//` comment removed.
fn code_lines(text: &str) -> Vec<&str> {
    text.lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .map(|l| l.split("//").next().unwrap_or_default())
        .collect()
}

/// The `.claude.json` account helpers that must not drive a recovery
/// decision. `Account::of_dir`/`from_value` read `.claude.json`; the
/// `home_*` helpers compared it with each profile's.
const CLAUDE_JSON_IDENTITY: &[&str] = &[
    "Account::from_value",
    "Account::of_dir",
    "from_value(",
    "of_dir(",
    "home_key",
    "home_still_matches",
    "home_holders",
];

/// The one `.claude.json` read the recovery path may make, and where:
/// `account::home_readable` is the #1702 `HomeUnreadable`/`HomeStuck` guard.
/// It asks only whether `~/.claude.json` names an account, stops the swap
/// or gives fix lines, and never leads to a move. It reaches `of_dir`
/// through `account.rs`, which the scan below does not list, so this test
/// pins where it is called.
const HOME_READABLE_ALLOWED: &[&str] = &["handfix_plan.rs", "identify.rs"];

#[test]
fn recovery_never_identifies_by_claude_json() {
    let profile = rotate_dir().parent().unwrap().to_path_buf();
    let files = [
        rotate_dir().join("handfix.rs"),
        rotate_dir().join("handfix_plan.rs"),
        rotate_dir().join("cred_identity.rs"),
        rotate_dir().join("identify.rs"),
        rotate_dir().join("recover.rs"),
        profile.join("usage_live.rs"),
    ];
    for path in files {
        let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        for (i, code) in code_lines(&text).iter().enumerate() {
            for f in CLAUDE_JSON_IDENTITY {
                assert!(
                    !code.contains(f),
                    "{}:{}: `{f}` in the recovery path: {code}",
                    path.display(),
                    i + 1
                );
            }
            let name = path.file_name().unwrap().to_string_lossy();
            assert!(
                !code.contains("home_readable(") || HOME_READABLE_ALLOWED.contains(&name.as_ref()),
                "{}:{}: `home_readable` outside the #1702 guard: {code}",
                path.display(),
                i + 1
            );
        }
    }
}

#[test]
fn the_claude_json_holders_module_is_gone() {
    assert!(
        !rotate_dir().join("holders.rs").exists(),
        "rotate/holders.rs (home_holders, home_key, home_still_matches) must be deleted"
    );
    let m = fs::read_to_string(rotate_dir().join("mod.rs")).unwrap();
    assert!(!code_lines(&m).iter().any(|l| l.contains("mod holders")));
}

#[test]
fn rotate_has_the_issue_1886_files() {
    for want in [
        "cred_identity.rs",
        "account.rs",
        "adopt.rs",
        "handfix_plan.rs",
    ] {
        assert!(rotate_dir().join(want).exists(), "missing rotate/{want}");
    }
}

/// SEC-1, SEC-4: the identity path passes directory paths to
/// `usage_direct` and never handles a token, nor starts a process.
#[test]
fn identity_and_recovery_never_touch_a_token_or_start_a_process() {
    for name in ["cred_identity.rs", "handfix.rs", "handfix_plan.rs"] {
        let text = source(name);
        for (i, code) in code_lines(&text).iter().enumerate() {
            for f in [
                "Token",
                "Creds",
                "load(",
                "header(",
                "Command::new",
                "accessToken",
                "claudeAiOauth",
            ] {
                assert!(!code.contains(f), "rotate/{name}:{}: `{f}`: {code}", i + 1);
            }
        }
    }
}

/// SEC-2 and the owner decision of 3 November: identity reads ask the
/// OAuth profile endpoint (`usage_direct::account`, which never refreshes),
/// never the usage read, whose reset times do not tell accounts apart.
#[test]
fn identity_reads_ask_the_profile_endpoint_and_never_refresh() {
    let text = source("cred_identity.rs");
    let code = code_lines(&text).join("\n");
    assert!(
        code.contains("usage_direct::account("),
        "usage_direct::account"
    );
    assert!(!code.contains("allow_refresh"));
    assert!(!code.contains("usage_direct::read") && !code.contains("direct::read("));
    for f in [
        "resets_at",
        "UsageReport",
        "usage_passive",
        "passive",
        "probe_one",
        "claude -p",
        "auth status",
    ] {
        assert!(!code.contains(f), "cred_identity.rs uses `{f}`");
    }
    let direct =
        fs::read_to_string(rotate_dir().parent().unwrap().join("usage_direct.rs")).unwrap();
    let account = body_of(&direct, "account");
    assert!(
        account.contains("profile_url()"),
        "account reads /api/oauth/profile"
    );
    assert!(!account.contains("refresh("), "account never refreshes");
}

/// SEC-16: an account read for identity is never written or shown by the
/// rotator.
#[test]
fn identities_are_never_serialized_or_displayed() {
    for name in ["cred_identity.rs", "account.rs"] {
        let code = code_lines(&source(name)).join("\n");
        assert!(!code.contains("Serialize"), "{name}: must not serialize");
        assert!(
            !code.contains("impl std::fmt::Display") && !code.contains("impl Display"),
            "{name}: no Display"
        );
    }
    for name in [
        "cred_identity.rs",
        "handfix.rs",
        "handfix_plan.rs",
        "handfix_msg.rs",
        "guide.rs",
    ] {
        let code = code_lines(&source(name)).join("\n");
        assert!(!code.contains(".shown()"), "{name} prints an account");
    }
}

/// SEC-9: the inode re-check is the last thing before the move.
#[test]
fn move_aside_rechecks_inodes_last() {
    let body = body_of(&source("handfix.rs"), "move_aside");
    assert!(!body.contains("home_still_matches"));
    assert_before(&body, ".same(", "verify_profile_dir", "move_aside");
    // The checks that can refuse come before the backup directory is made.
    assert_before(&body, "verify_profile_dir", "fresh_backup", "move_aside");
    assert_before(&body, "fresh_backup", "ino_ctime", "move_aside");
    assert_before(&body, "verify_profile_dir", "ino", "move_aside");
    assert_before(&body, "ino", "move_login", "move_aside");
}

/// The hand-change plan has no catch-all arm that could fall into a move.
#[test]
fn the_hand_change_plan_has_no_wildcard_move() {
    let code = code_lines(&source("handfix_plan.rs")).join("\n");
    for l in code.lines() {
        let t = l.trim_start();
        assert!(
            !(t.starts_with("_ =>") && t.contains("Move")),
            "handfix_plan.rs: wildcard arm returns a move: {l}"
        );
    }
}

/// The #1873 files are out of scope for #1886 and keep their roles.
#[test]
fn the_issue_1873_files_are_not_used_for_identity() {
    for name in ["fresh.rs", "damp.rs", "swap_back.rs"] {
        let code = code_lines(&source(name)).join("\n");
        assert!(!code.contains("cred_identity"), "{name} uses cred_identity");
        assert!(!code.contains("hand_suspect"), "{name} uses hand_suspect");
    }
}
