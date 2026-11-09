//! Issue #2005: every state in which a Codex swap is refused writes one
//! `codex refuse reason=<code>[ detail=<word>]` line and changes nothing.
//! See docs/reference/profile-command.md, "Codex refusals" and "Default
//! Codex home link". All seven codes are covered, plus each `detail=`.

#![cfg(target_os = "linux")]

use crate::support::codex_fake::*;
use crate::support::exec_stub::copy_executable;
use crate::support::profile_rotate::{assert_log_grammar, text};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;

/// Runs one rotate and returns the Codex lines after its `codex cycle`
/// line, checking nothing moved.
fn refused(c: &CodexFixture) -> Vec<String> {
    let link_before = fs::read_link(&c.link).ok();
    let inodes = c.account_inodes();
    let traps = c.auth_traps();
    let state_before = fs::read(c.state_path()).ok();
    let out = c.rotate();
    assert!(out.status.success(), "a refusal never changes the status");
    assert_eq!(fs::read_link(&c.link).ok(), link_before, "the link moved");
    assert_eq!(c.account_inodes(), inodes);
    assert_eq!(c.auth_traps(), traps);
    if let Some(before) = state_before {
        assert_eq!(fs::read(c.state_path()).ok(), Some(before), "state changed");
    } else {
        assert!(
            c.state()
                .is_none_or(|s| s.get("journal").is_none_or(|j| j.is_null())),
            "a refusal wrote a journal"
        );
    }
    for l in c.f.log().lines() {
        assert_log_grammar(l, true);
    }
    let log = c.codex_log();
    let i = log
        .iter()
        .position(|l| l.starts_with("codex cycle "))
        .unwrap();
    log[i + 1..].to_vec()
}

fn one_refusal(c: &CodexFixture, want: &str) {
    let after = refused(c);
    assert_eq!(after, [want.to_string()], "{:?}", c.codex_log());
}

#[test]
fn a_real_home_directory_is_refused_and_never_adopted() {
    let c = CodexFixture::with(&["ffarmer", "ggrant"]);
    c.reading("ffarmer", 50, 50);
    c.reading("ggrant", 10, 10);
    trapped(&c.link);
    let traps_home = fs::symlink_metadata(c.link.join("auth.json")).unwrap();
    one_refusal(&c, "codex refuse reason=home-not-link");
    assert!(
        fs::symlink_metadata(&c.link).unwrap().is_dir(),
        "not adopted"
    );
    use std::os::unix::fs::MetadataExt;
    assert_eq!(
        fs::symlink_metadata(c.link.join("auth.json"))
            .unwrap()
            .ino(),
        traps_home.ino()
    );
}

#[test]
fn a_regular_file_at_the_link_path_is_refused() {
    let c = CodexFixture::with(&["ffarmer", "ggrant"]);
    c.reading("ffarmer", 50, 50);
    c.reading("ggrant", 10, 10);
    fs::write(&c.link, "not a link").unwrap();
    one_refusal(&c, "codex refuse reason=home-not-link");
    assert_eq!(fs::read_to_string(&c.link).unwrap(), "not a link");
}

#[test]
fn a_missing_link_is_refused_with_detail_missing() {
    let c = CodexFixture::with(&["ffarmer", "ggrant"]);
    c.reading("ffarmer", 50, 50);
    c.reading("ggrant", 10, 10);
    one_refusal(&c, "codex refuse reason=home-not-link detail=missing");
    assert!(
        fs::symlink_metadata(&c.link).is_err(),
        "no link was created"
    );
}

#[test]
fn a_link_outside_the_root_is_refused() {
    let c = CodexFixture::standard();
    let elsewhere = c.base.join("elsewhere");
    trapped(&elsewhere);
    fs::remove_file(&c.link).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &c.link).unwrap();
    one_refusal(&c, "codex refuse reason=link-outside-root");
}

#[test]
fn a_sibling_whose_name_starts_with_the_root_is_outside_it() {
    let c = CodexFixture::standard();
    let evil: PathBuf = format!("{}-evil", c.root.display()).into();
    trapped(&evil.join("ffarmer"));
    fs::remove_file(&c.link).unwrap();
    std::os::unix::fs::symlink(evil.join("ffarmer"), &c.link).unwrap();
    one_refusal(&c, "codex refuse reason=link-outside-root");
}

#[test]
fn a_nested_target_under_the_root_is_refused() {
    let c = CodexFixture::standard();
    let nested = c.account("ffarmer").join("inner");
    fs::create_dir_all(&nested).unwrap();
    fs::remove_file(&c.link).unwrap();
    std::os::unix::fs::symlink(&nested, &c.link).unwrap();
    one_refusal(&c, "codex refuse reason=link-outside-root");
}

#[test]
fn a_dangling_link_is_refused() {
    let c = CodexFixture::standard();
    fs::remove_file(&c.link).unwrap();
    std::os::unix::fs::symlink(c.account("gone"), &c.link).unwrap();
    one_refusal(&c, "codex refuse reason=link-dangling");
}

#[test]
fn a_group_writable_link_parent_is_refused() {
    let c = CodexFixture::standard();
    fs::set_permissions(&c.f.home, fs::Permissions::from_mode(0o770)).unwrap();
    let after = refused(&c);
    fs::set_permissions(&c.f.home, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(
        after,
        ["codex refuse reason=link-outside-root detail=parent-not-private"]
    );
}

#[test]
fn a_world_writable_root_is_refused() {
    let c = CodexFixture::standard();
    fs::set_permissions(&c.root, fs::Permissions::from_mode(0o707)).unwrap();
    let after = refused(&c);
    fs::set_permissions(&c.root, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(
        after,
        ["codex refuse reason=link-outside-root detail=root-not-private"]
    );
}

#[test]
fn a_temp_path_that_is_a_real_directory_is_left_and_refused() {
    let c = CodexFixture::standard();
    let tmp = c.link.with_file_name(".codex.amplihack-swap");
    fs::create_dir(&tmp).unwrap();
    fs::write(tmp.join("keep"), "x").unwrap();
    one_refusal(
        &c,
        "codex refuse reason=home-not-link detail=swap-path-not-link",
    );
    assert_eq!(fs::read_to_string(tmp.join("keep")).unwrap(), "x");
}

#[test]
fn a_temp_path_that_is_a_regular_file_is_left_and_refused() {
    let c = CodexFixture::standard();
    let tmp = c.link.with_file_name(".codex.amplihack-swap");
    fs::write(&tmp, "precious").unwrap();
    one_refusal(
        &c,
        "codex refuse reason=home-not-link detail=swap-path-not-link",
    );
    assert_eq!(fs::read_to_string(&tmp).unwrap(), "precious");
}

#[test]
fn a_leftover_temp_symlink_does_not_block_the_swap() {
    let c = CodexFixture::standard();
    let tmp = c.link.with_file_name(".codex.amplihack-swap");
    std::os::unix::fs::symlink(c.account("hhale"), &tmp).unwrap();
    c.rotate();
    assert_eq!(fs::read_link(&c.link).unwrap(), c.account("ggrant"));
    assert!(fs::symlink_metadata(&tmp).is_err());
}

#[test]
fn a_held_rotator_lock_refuses_with_lock_busy() {
    let c = CodexFixture::standard();
    let dir = c.f.rotate_dir();
    fs::create_dir_all(&dir).unwrap();
    for d in [c.f.root.join(".auto-switch.d"), dir.clone()] {
        fs::set_permissions(d, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let lock = dir.join("lock");
    fs::write(&lock, "").unwrap();
    fs::set_permissions(&lock, fs::Permissions::from_mode(0o600)).unwrap();
    let holder = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&lock)
        .unwrap();
    // SAFETY: a valid open descriptor.
    assert_eq!(
        unsafe { libc::flock(holder.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    let after = refused(&c);
    drop(holder);
    assert_eq!(after, ["codex refuse reason=lock-busy"]);
}

#[test]
fn a_codex_program_inside_the_live_account_is_refused() {
    let c = CodexFixture::standard();
    let inside = c.account("ffarmer").join("packages").join("codex");
    fs::create_dir_all(inside.parent().unwrap()).unwrap();
    copy_executable(&c.bin, &inside);
    let mut cmd = c.cmd(&["profile", "rotate"]);
    cmd.env("CODEX_BIN", &inside);
    let out = c.run_cmd("rotate with codex inside the live account", cmd);
    assert!(out.status.success());
    let log = c.codex_log();
    assert_eq!(
        log.last().map(String::as_str),
        Some("codex refuse reason=codex-bin-in-home"),
        "{log:?}"
    );
    assert_eq!(c.live().as_deref(), Some("ffarmer"));
}

#[test]
fn a_codex_program_reached_through_the_link_is_refused() {
    let c = CodexFixture::standard();
    let inside = c.account("ffarmer").join("bin").join("codex");
    fs::create_dir_all(inside.parent().unwrap()).unwrap();
    copy_executable(&c.bin, &inside);
    let mut cmd = c.cmd(&["profile", "rotate"]);
    cmd.env("CODEX_BIN", c.link.join("bin").join("codex"));
    let out = c.run_cmd("rotate with codex under the link", cmd);
    assert!(out.status.success());
    assert_eq!(
        c.codex_log().last().map(String::as_str),
        Some("codex refuse reason=codex-bin-in-home")
    );
    assert_eq!(c.live().as_deref(), Some("ffarmer"));
}

#[test]
fn a_codex_process_using_the_default_home_refuses_with_its_pid() {
    let c = CodexFixture::standard();
    let cwd = c.base.join("cwd-proc");
    fs::create_dir_all(&cwd).unwrap();
    fake_proc(
        &c.proc_root,
        31337,
        "codex",
        "/usr/local/bin/codex",
        &["codex"],
        &["PATH=/usr/bin", "OPENAI_API_KEY=sk-SENTINEL-2005-environ"],
        &cwd,
    );
    one_refusal(
        &c,
        "codex refuse reason=default-home-in-use detail=pid-31337",
    );
    assert!(
        !c.f.log().contains("SENTINEL-2005-environ"),
        "environ was logged"
    );
}

#[test]
fn a_codex_process_whose_codex_home_is_the_link_refuses() {
    let c = CodexFixture::standard();
    let cwd = c.base.join("cwd-proc");
    fs::create_dir_all(&cwd).unwrap();
    let home = format!("CODEX_HOME={}", c.link.display());
    fake_proc(
        &c.proc_root,
        777,
        "codex",
        "/usr/bin/codex",
        &["codex"],
        &[&home],
        &cwd,
    );
    one_refusal(&c, "codex refuse reason=default-home-in-use detail=pid-777");
}

#[test]
fn a_missing_proc_root_refuses_with_proc_unreadable() {
    let c = CodexFixture::standard();
    let mut cmd = c.cmd(&["profile", "rotate"]);
    cmd.env(PROC_ROOT_ENV, c.base.join("no-proc"));
    let out = c.run_cmd("rotate with no /proc", cmd);
    assert!(out.status.success());
    assert_eq!(
        c.codex_log().last().map(String::as_str),
        Some("codex refuse reason=proc-unreadable")
    );
    assert_eq!(c.live().as_deref(), Some("ffarmer"));
}

#[test]
fn an_unreadable_state_file_refuses_and_is_never_overwritten() {
    for (body, why) in [
        ("{not json".to_string(), "line_1_column_2"),
        (
            serde_json::json!({"version": 2, "resumes": []}).to_string(),
            "unknown_version",
        ),
    ] {
        let c = CodexFixture::standard();
        c.write_state(&serde_json::json!({}));
        fs::write(c.state_path(), &body).unwrap();
        one_refusal(
            &c,
            &format!("codex refuse reason=journal-mismatch detail=state-unreadable:{why}"),
        );
        assert_eq!(fs::read_to_string(c.state_path()).unwrap(), body);
    }
}

#[test]
fn a_state_file_with_a_wide_mode_is_refused_and_kept() {
    let c = CodexFixture::standard();
    c.write_state(&serde_json::json!({"version": 1, "resumes": []}));
    fs::set_permissions(c.state_path(), fs::Permissions::from_mode(0o644)).unwrap();
    one_refusal(
        &c,
        "codex refuse reason=journal-mismatch detail=state-unreadable:not_private__mode_0600_or_narrower_",
    );
    let m = fs::metadata(c.state_path()).unwrap();
    assert_eq!(m.permissions().mode() & 0o777, 0o644);
}

#[test]
fn a_state_file_that_is_a_symlink_is_refused_and_not_followed() {
    let c = CodexFixture::standard();
    c.write_state(&serde_json::json!({"version": 1, "resumes": []}));
    let real = c.base.join("elsewhere-state.json");
    fs::write(&real, r#"{"version":1,"resumes":[]}"#).unwrap();
    fs::set_permissions(&real, fs::Permissions::from_mode(0o600)).unwrap();
    fs::remove_file(c.state_path()).unwrap();
    std::os::unix::fs::symlink(&real, c.state_path()).unwrap();
    one_refusal(
        &c,
        "codex refuse reason=journal-mismatch detail=state-unreadable:is_a_symlink",
    );
    assert_eq!(
        fs::read_to_string(&real).unwrap(),
        r#"{"version":1,"resumes":[]}"#
    );
}

#[test]
fn a_dry_run_prints_the_refusal_a_real_cycle_would_write() {
    let c = CodexFixture::standard();
    let cwd = c.base.join("cwd-proc");
    fs::create_dir_all(&cwd).unwrap();
    fake_proc(
        &c.proc_root,
        99,
        "codex",
        "/usr/bin/codex",
        &["codex"],
        &[],
        &cwd,
    );
    let out = c.dry_run();
    let codex = codex_lines(&text(&out.stdout));
    assert_eq!(
        codex.last().map(String::as_str),
        Some("codex refuse reason=default-home-in-use detail=pid-99"),
        "{codex:?}"
    );
    assert!(!codex.iter().any(|l| l.starts_with("dry-run would-swap")));
    assert!(!c.f.rotate_dir().exists());
}
