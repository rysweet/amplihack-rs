//! Issue #2005: each `profile rotate` cycle also reads every Codex account
//! live once and writes one `codex cycle` (or `codex skip`) line. See
//! docs/reference/profile-command.md, "Rotate Codex".
//!
//! Every run uses `codex_fake::CodexFixture`: a temporary HOME, a Codex
//! profiles root of trapped account directories (FIFO `auth.json`), a fake
//! `codex app-server` and an empty fake `/proc`. Linux-only, as the Claude
//! part of a one-shot rotate is.

#![cfg(target_os = "linux")]

use crate::support::codex_fake::*;
use crate::support::profile_rotate::{assert_log_grammar, text};
use std::fs;
use std::os::unix::fs::PermissionsExt;

const READ_AT: &str = r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z";

fn stdout_lines(out: &std::process::Output) -> Vec<String> {
    text(&out.stdout).lines().map(String::from).collect()
}

#[test]
fn the_cycle_line_lists_every_account_live_first_and_decides_a_swap() {
    let c = CodexFixture::standard();
    let r = reset_text(RESET_AT);
    let out = c.dry_run();
    let codex = codex_lines(&text(&out.stdout));
    let line = cycle_line(&codex);
    let want = format!(
        r"^codex cycle live=ffarmer ffarmer:5h=12%@{r},7d=41%@{r},room=59 ggrant:7d=34%@{r},room=66 hhale:5h=100%@{r},credits=4200,room=0\.5 read={READ_AT} -> swap ffarmer->ggrant$"
    );
    assert!(
        regex::Regex::new(&want).unwrap().is_match(&line),
        "cycle line:\n{line}\nwant:\n{want}"
    );
    for l in &codex {
        assert_log_grammar(l, false);
    }
}

#[test]
fn the_live_account_is_read_once_and_the_home_entry_is_dropped() {
    let c = CodexFixture::standard();
    c.dry_run();
    let mut reads = c.reads();
    reads.sort();
    assert_eq!(
        reads,
        ["ffarmer", "ggrant", "hhale"],
        "one read per account"
    );
    let line = cycle_line(&codex_lines(&text(&c.dry_run().stdout)));
    assert!(!line.contains("~/.codex"), "{line}");
}

#[test]
fn codex_off_reads_nothing_and_writes_no_codex_line() {
    let c = CodexFixture::standard();
    let mut cmd = c.cmd(&["profile", "rotate"]);
    cmd.env("AMPLIHACK_ROTATE_CODEX", "off");
    let out = c.run_cmd("rotate with codex off", cmd);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(c.codex_log().is_empty(), "{:?}", c.codex_log());
    assert!(c.reads().is_empty(), "codex off read {:?}", c.reads());
    assert_eq!(c.live().as_deref(), Some("ffarmer"));

    let out = c.ok(&["profile", "rotate", "--dry-run", "--codex", "off"]);
    assert!(codex_lines(&text(&out.stdout)).is_empty());
    let out = c.ok(&["profile", "rotate", "--dry-run", "--codex", "0"]);
    assert!(codex_lines(&text(&out.stdout)).is_empty());
    assert!(c.reads().is_empty());
}

#[test]
fn a_flag_beats_the_variable_for_codex_on() {
    let c = CodexFixture::standard();
    let mut cmd = c.cmd(&["profile", "rotate", "--dry-run", "--codex", "on"]);
    cmd.env("AMPLIHACK_ROTATE_CODEX", "off");
    let out = c.run_cmd("dry run with --codex on", cmd);
    assert!(out.status.success());
    cycle_line(&codex_lines(&text(&out.stdout)));
}

#[test]
fn no_accounts_writes_one_skip_line() {
    let c = CodexFixture::with(&[]);
    let out = c.rotate();
    assert!(out.status.success());
    assert_eq!(c.codex_log(), ["codex skip reason=no-accounts"]);
    let out = c.dry_run();
    assert_eq!(
        codex_lines(&text(&out.stdout)),
        ["codex skip reason=no-accounts"]
    );
}

#[test]
fn a_missing_codex_program_writes_one_skip_line() {
    let c = CodexFixture::standard();
    let mut cmd = c.cmd(&["profile", "rotate"]);
    cmd.env("CODEX_BIN", c.base.join("no-such-codex"));
    let out = c.run_cmd("rotate with no codex program", cmd);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(c.codex_log(), ["codex skip reason=no-codex-bin"]);
    assert_eq!(c.live().as_deref(), Some("ffarmer"));

    // No CODEX_BIN and no `codex` on PATH: the same skip.
    let mut cmd = c.cmd(&["profile", "rotate", "--dry-run"]);
    cmd.env_remove("CODEX_BIN");
    let out = c.run_cmd("dry run with codex not on PATH", cmd);
    assert_eq!(
        codex_lines(&text(&out.stdout)),
        ["codex skip reason=no-codex-bin"]
    );
}

#[test]
fn the_live_account_with_the_most_room_is_kept() {
    let c = CodexFixture::standard();
    c.link_to("ggrant");
    let out = c.rotate();
    assert!(out.status.success());
    let log = c.codex_log();
    let line = cycle_line(&log);
    assert!(
        line.starts_with("codex cycle live=ggrant ggrant:"),
        "{line}"
    );
    assert!(line.ends_with(" -> keep"), "{line}");
    assert_eq!(log.len(), 1, "keep writes no other codex line: {log:?}");
    assert_eq!(c.live().as_deref(), Some("ggrant"));
    assert!(c.state().is_none_or(|s| s.get("journal").is_none()));
}

#[test]
fn no_account_with_room_decides_none_no_room() {
    let c = CodexFixture::standard();
    c.reading("ffarmer", 100, 100);
    c.reading("ggrant", 100, 50);
    c.reading_json(
        "hhale",
        serde_json::json!({"primary": window(100, 300, RESET_AT)}),
    );
    let out = c.dry_run();
    let line = cycle_line(&codex_lines(&text(&out.stdout)));
    assert!(line.ends_with(" -> none(no-room)"), "{line}");
    assert!(line.contains("ffarmer:5h=100%"), "{line}");
    assert!(line.contains(",room=-1"), "{line}");
}

#[test]
fn an_account_that_is_not_signed_in_is_listed_as_unread() {
    let c = CodexFixture::with(&["ffarmer", "ggrant", "jjones"]);
    c.reading("ffarmer", 50, 50);
    c.reading("ggrant", 10, 10);
    c.link_to("ffarmer");
    let line = cycle_line(&codex_lines(&text(&c.dry_run().stdout)));
    assert!(line.contains(" unread=jjones(not-signed-in) "), "{line}");
    assert!(!line.contains("jjones:"), "{line}");
    assert!(line.ends_with(" -> swap ffarmer->ggrant"), "{line}");
}

#[test]
fn a_live_account_that_cannot_be_read_counts_as_having_no_room() {
    let c = CodexFixture::with(&["ffarmer", "ggrant"]);
    c.reading("ggrant", 98, 10);
    c.link_to("ffarmer");
    // ffarmer is not signed in; a 2-point room still beats it, with no
    // margin or hold applied.
    let mut cmd = c.cmd(&["profile", "rotate", "--dry-run", "--margin", "50"]);
    cmd.env("AMPLIHACK_ROTATE_HOLD", "999999");
    let out = c.run_cmd("dry run with an unread live account", cmd);
    let line = cycle_line(&codex_lines(&text(&out.stdout)));
    assert!(line.starts_with("codex cycle live=ffarmer "), "{line}");
    assert!(line.contains("unread=ffarmer(not-signed-in)"), "{line}");
    assert!(line.ends_with(" -> swap ffarmer->ggrant"), "{line}");
}

#[test]
fn a_link_not_under_the_root_reads_home_as_its_own_account() {
    let c = CodexFixture::with(&["ffarmer", "ggrant"]);
    c.reading("ffarmer", 50, 50);
    c.reading("ggrant", 10, 10);
    // `~/.codex` is a real directory: read as `~/.codex`, decided no-live.
    trapped(&c.link);
    c.reading(".codex", 20, 20);
    let out = c.dry_run();
    let codex = codex_lines(&text(&out.stdout));
    let line = cycle_line(&codex);
    assert!(line.starts_with("codex cycle live=- "), "{line}");
    assert!(line.contains(" ~/.codex:5h=20%"), "{line}");
    assert!(line.ends_with(" -> none(no-live)"), "{line}");
    assert!(
        codex.contains(&"codex refuse reason=home-not-link".to_string()),
        "{codex:?}"
    );
}

#[test]
fn the_credit_floor_decides_whether_a_full_account_has_room() {
    let c = CodexFixture::standard();
    let line = cycle_line(&codex_lines(&text(&c.dry_run().stdout)));
    assert!(line.contains("hhale:5h=100%"), "{line}");
    assert!(line.contains(",credits=4200,room=0.5 "), "{line}");

    let mut cmd = c.cmd(&["profile", "rotate", "--dry-run"]);
    cmd.env("AMPLIHACK_ROTATE_CODEX_FLOOR", "5000");
    let out = c.run_cmd("dry run with floor 5000", cmd);
    let line = cycle_line(&codex_lines(&text(&out.stdout)));
    assert!(line.contains(",credits=4200,room=-1 "), "{line}");

    let out = c.ok(&["profile", "rotate", "--dry-run", "--codex-floor", "0"]);
    let line = cycle_line(&codex_lines(&text(&out.stdout)));
    assert!(line.contains(",credits=4200,room=0.5 "), "{line}");
}

#[test]
fn labels_are_cleaned_so_they_cannot_forge_a_field_or_a_line() {
    let evil = "a b(c),d";
    let c = CodexFixture::with(&["ffarmer", evil, "x\ny"]);
    c.reading("ffarmer", 50, 50);
    c.reading(evil, 10, 10);
    c.reading("x\ny", 90, 90);
    c.link_to("ffarmer");
    let out = c.dry_run();
    let stdout = text(&out.stdout);
    let codex = codex_lines(&stdout);
    let line = cycle_line(&codex);
    assert!(line.contains(" a_b_c__d:5h=10%"), "{line}");
    assert!(line.contains(" x_y:5h=90%"), "{line}");
    assert!(line.ends_with(" -> swap ffarmer->a_b_c__d"), "{line}");
    assert!(
        !stdout.lines().any(|l| l.starts_with("y:")),
        "a newline in a label started a line: {stdout}"
    );
}

#[test]
fn a_custom_home_link_is_the_one_inspected_and_swapped() {
    let c = CodexFixture::standard();
    fs::remove_file(&c.link).unwrap();
    let other = c.f.home.join("links").join("codex-default");
    fs::create_dir_all(other.parent().unwrap()).unwrap();
    fs::set_permissions(other.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
    std::os::unix::fs::symlink(c.account("ffarmer"), &other).unwrap();
    let mut cmd = c.cmd(&["profile", "rotate"]);
    cmd.env("AMPLIHACK_ROTATE_CODEX_HOME_LINK", &other);
    let out = c.run_cmd("rotate with a custom link", cmd);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(fs::read_link(&other).unwrap(), c.account("ggrant"));
    assert!(!c.link.exists() && fs::symlink_metadata(&c.link).is_err());
    let swap = format!(
        "codex swap ffarmer->ggrant link={} reason=room",
        other.display()
    );
    assert!(c.codex_log().contains(&swap), "{:?}", c.codex_log());
}

#[test]
fn a_real_home_directory_beside_a_custom_link_never_blocks_a_swap() {
    // Review of #2162: with the link configured elsewhere, a signed-in
    // `~/.codex` directory that ranks first must not be chosen (a swap to it
    // is refused as link-outside-root every cycle); the next account is.
    let c = CodexFixture::standard();
    fs::remove_file(&c.link).unwrap();
    let other = c.f.home.join("links").join("codex-default");
    fs::create_dir_all(other.parent().unwrap()).unwrap();
    fs::set_permissions(other.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
    std::os::unix::fs::symlink(c.account("ffarmer"), &other).unwrap();
    trapped(&c.link);
    c.reading(".codex", 1, 1);
    let mut cmd = c.cmd(&["profile", "rotate"]);
    cmd.env("AMPLIHACK_ROTATE_CODEX_HOME_LINK", &other);
    let out = c.run_cmd("rotate with a custom link and a real ~/.codex", cmd);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let log = c.codex_log();
    let line = cycle_line(&log);
    assert!(!line.contains("~/.codex"), "{line}");
    assert!(line.ends_with(" -> swap ffarmer->ggrant"), "{line}");
    assert!(
        !log.iter().any(|l| l.contains("link-outside-root")),
        "{log:?}"
    );
    assert_eq!(fs::read_link(&other).unwrap(), c.account("ggrant"));
    assert!(fs::symlink_metadata(&c.link).unwrap().is_dir());
}

#[test]
fn invalid_codex_settings_are_rotate_config_errors_naming_the_source() {
    let c = CodexFixture::standard();
    type Case<'a> = (Vec<&'a str>, Option<(&'a str, &'a str)>, &'a str);
    let cases: Vec<Case> = vec![
        (
            vec!["--codex-home-link", "codex"],
            None,
            "invalid rotate setting --codex-home-link \"codex\": expected an absolute path with no .. component",
        ),
        (
            vec!["--codex-home-link", "/tmp/../etc/codex"],
            None,
            "invalid rotate setting --codex-home-link",
        ),
        (
            vec![],
            Some(("AMPLIHACK_ROTATE_CODEX_HOME_LINK", "rel/link")),
            "invalid rotate setting AMPLIHACK_ROTATE_CODEX_HOME_LINK",
        ),
        (
            vec!["--codex", "maybe"],
            None,
            "invalid rotate setting --codex \"maybe\"",
        ),
        (
            vec![],
            Some(("AMPLIHACK_ROTATE_CODEX", "yes")),
            "invalid rotate setting AMPLIHACK_ROTATE_CODEX \"yes\"",
        ),
        (
            vec!["--codex-floor", "-1"],
            None,
            "invalid rotate setting --codex-floor \"-1\"",
        ),
        (
            vec![],
            Some(("AMPLIHACK_ROTATE_CODEX_FLOOR", "lots")),
            "invalid rotate setting AMPLIHACK_ROTATE_CODEX_FLOOR \"lots\"",
        ),
    ];
    for (flags, env, want) in cases {
        let mut args = vec!["profile", "rotate"];
        args.extend(flags.iter());
        let mut cmd = c.cmd(&args);
        if let Some((k, v)) = env {
            cmd.env(k, v);
        }
        let out = c.run_cmd(&format!("{args:?} {env:?}"), cmd);
        assert_eq!(out.status.code(), Some(1), "{args:?} {env:?}");
        let err = text(&out.stderr);
        assert!(err.contains(want), "{args:?} {env:?}: {err}");
        assert!(
            err.contains("hint: run `amplihack profile rotate --help`"),
            "{err}"
        );
    }
    assert_eq!(c.live().as_deref(), Some("ffarmer"), "nothing moved");
    assert!(c.reads().is_empty(), "a config error reads nothing");
}

#[test]
fn an_empty_codex_variable_counts_as_unset() {
    let c = CodexFixture::standard();
    let mut cmd = c.cmd(&["profile", "rotate", "--dry-run"]);
    cmd.env("AMPLIHACK_ROTATE_CODEX", "")
        .env("AMPLIHACK_ROTATE_CODEX_FLOOR", "")
        .env("AMPLIHACK_ROTATE_CODEX_HOME_LINK", "");
    let out = c.run_cmd("dry run with empty codex variables", cmd);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let line = cycle_line(&codex_lines(&text(&out.stdout)));
    assert!(line.starts_with("codex cycle live=ffarmer "), "{line}");
}

#[test]
fn a_codex_error_is_a_skip_line_and_never_changes_the_claude_cycle_or_status() {
    let c = CodexFixture::standard();
    fs::set_permissions(&c.root, fs::Permissions::from_mode(0o000)).unwrap();
    let out = c.run(&["profile", "rotate"]);
    fs::set_permissions(&c.root, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    let log = c.f.log();
    assert!(
        log.lines()
            .any(|l| l.contains(" skip: ") && l.to_lowercase().contains("codex")),
        "{log}"
    );
    // The Claude part still swapped work (90%) away.
    assert!(log.lines().any(|l| l.contains(" swap work->")), "{log}");
}

#[test]
fn a_dry_run_keeps_claude_lines_as_they_were_and_changes_nothing() {
    let c = CodexFixture::standard();
    c.f.usage_direct("work", 90, 20);
    c.f.usage_direct("spare", 10, 20);
    c.f.usage_direct("extra", 50, 60);
    let inodes = c.account_inodes();
    let traps = c.auth_traps();
    let out = c.dry_run();
    let ls = stdout_lines(&out);
    for l in &ls {
        assert!(
            l.starts_with("would ")
                || l.starts_with("cycle ")
                || l.starts_with("codex ")
                || l.starts_with("dry-run would-"),
            "{l:?}"
        );
        assert_log_grammar(l, false);
    }
    assert!(
        ls.iter().any(|l| l.starts_with("would swap work->spare")),
        "{ls:?}"
    );
    assert!(
        ls.contains(&"dry-run would-swap ffarmer->ggrant".to_string()),
        "{ls:?}"
    );
    assert!(!c.f.rotate_dir().exists(), "a dry run wrote rotate state");
    assert_eq!(c.live().as_deref(), Some("ffarmer"));
    assert_eq!(c.account_inodes(), inodes);
    assert_eq!(c.auth_traps(), traps);
}

#[test]
fn every_rotate_log_line_stays_in_the_documented_grammar() {
    let c = CodexFixture::standard();
    c.rotate();
    for l in c.f.log().lines() {
        assert_log_grammar(l, true);
    }
}
