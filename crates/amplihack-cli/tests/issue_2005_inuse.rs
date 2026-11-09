//! Issue #2005: before a swap the rotator looks in `/proc` for a Codex
//! process of the user's own that reaches its login through the default
//! home link. It fails closed: anything it cannot read counts as in use.
//! Every test uses a fake `/proc` (`AMPLIHACK_TEST_CODEX_PROC_ROOT`); no
//! real process is examined or signalled. See
//! docs/reference/profile-command.md, "Codex default home in use".

#![cfg(target_os = "linux")]

use crate::support::codex_fake::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;

fn swapped(c: &CodexFixture) -> bool {
    c.rotate();
    c.live().as_deref() == Some("ggrant")
}

fn last(c: &CodexFixture) -> String {
    c.codex_log().last().cloned().unwrap_or_default()
}

fn cwd(c: &CodexFixture) -> std::path::PathBuf {
    let d = c.base.join("proc-cwd");
    fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn an_empty_proc_allows_the_swap() {
    let c = CodexFixture::standard();
    assert!(swapped(&c), "{:?}", c.codex_log());
}

#[test]
fn a_node_process_running_codex_js_is_a_codex_process() {
    let c = CodexFixture::standard();
    let d = cwd(&c);
    fake_proc(
        &c.proc_root,
        501,
        "node",
        "/usr/bin/node",
        &[
            "node",
            "/usr/lib/node_modules/@openai/codex/bin/codex.js",
            "exec",
        ],
        &["PATH=/usr/bin"],
        &d,
    );
    assert!(!swapped(&c));
    assert_eq!(
        last(&c),
        "codex refuse reason=default-home-in-use detail=pid-501"
    );
}

#[test]
fn an_exe_whose_name_starts_with_codex_is_a_codex_process() {
    let c = CodexFixture::standard();
    let d = cwd(&c);
    fake_proc(
        &c.proc_root,
        502,
        "MainThread",
        "/opt/codex/codex-x86_64-unknown-linux-musl",
        &["codex"],
        &[],
        &d,
    );
    assert!(!swapped(&c));
    assert_eq!(
        last(&c),
        "codex refuse reason=default-home-in-use detail=pid-502"
    );
}

#[test]
fn other_programs_are_ignored() {
    let c = CodexFixture::standard();
    let d = cwd(&c);
    fake_proc(
        &c.proc_root,
        503,
        "bash",
        "/usr/bin/bash",
        &["bash"],
        &[],
        &d,
    );
    fake_proc(
        &c.proc_root,
        504,
        "node",
        "/usr/bin/node",
        &["node", "/srv/app/server.js"],
        &[],
        &d,
    );
    // A name that merely contains codex is not a Codex process.
    fake_proc(
        &c.proc_root,
        505,
        "mycodex",
        "/usr/bin/mycodex",
        &["mycodex"],
        &[],
        &d,
    );
    // Non-pid entries are skipped.
    fs::create_dir_all(c.proc_root.join("self")).unwrap();
    fs::write(c.proc_root.join("uptime"), "1 1\n").unwrap();
    assert!(swapped(&c), "{:?}", c.codex_log());
}

#[test]
fn a_relative_codex_home_is_resolved_against_the_process_cwd() {
    let c = CodexFixture::standard();
    // cwd = $HOME, CODEX_HOME=.codex: that is the link.
    fake_proc(
        &c.proc_root,
        506,
        "codex",
        "/usr/bin/codex",
        &["codex"],
        &["CODEX_HOME=.codex"],
        &c.f.home,
    );
    assert!(!swapped(&c));
    assert_eq!(
        last(&c),
        "codex refuse reason=default-home-in-use detail=pid-506"
    );
}

#[test]
fn a_relative_codex_home_elsewhere_is_not_the_default_home() {
    let c = CodexFixture::standard();
    // cwd = the root, CODEX_HOME=ffarmer: an explicit account, not the link,
    // even though the link points there now.
    fake_proc(
        &c.proc_root,
        507,
        "codex",
        "/usr/bin/codex",
        &["codex"],
        &["CODEX_HOME=ffarmer"],
        &c.root,
    );
    assert!(swapped(&c), "{:?}", c.codex_log());
}

#[test]
fn an_unreadable_environ_counts_as_in_use() {
    let c = CodexFixture::standard();
    let d = cwd(&c);
    let p = fake_proc(
        &c.proc_root,
        508,
        "codex",
        "/usr/bin/codex",
        &["codex"],
        &[],
        &d,
    );
    fs::set_permissions(p.join("environ"), fs::Permissions::from_mode(0o000)).unwrap();
    assert!(!swapped(&c));
    assert_eq!(
        last(&c),
        "codex refuse reason=default-home-in-use detail=pid-508"
    );
}

#[test]
fn an_unreadable_cwd_with_a_relative_codex_home_counts_as_in_use() {
    let c = CodexFixture::standard();
    let d = cwd(&c);
    let p = fake_proc(
        &c.proc_root,
        509,
        "codex",
        "/usr/bin/codex",
        &["codex"],
        &["CODEX_HOME=x"],
        &d,
    );
    fs::remove_file(p.join("cwd")).unwrap();
    assert!(!swapped(&c));
    assert_eq!(
        last(&c),
        "codex refuse reason=default-home-in-use detail=pid-509"
    );
}

#[test]
fn a_process_that_exits_during_the_scan_is_skipped() {
    let c = CodexFixture::standard();
    // A pid directory with no comm, exe or environ: the process is gone.
    fs::create_dir_all(c.proc_root.join("510")).unwrap();
    assert!(swapped(&c), "{:?}", c.codex_log());
}

#[test]
fn an_oversized_environ_is_capped_and_still_judged() {
    let c = CodexFixture::standard();
    let d = cwd(&c);
    let big = format!("FILLER={}", "x".repeat(300 * 1024));
    let home = format!("CODEX_HOME={}", c.account("hhale").display());
    // CODEX_HOME is first, so the capped read still sees an explicit home.
    fake_proc(
        &c.proc_root,
        511,
        "codex",
        "/usr/bin/codex",
        &["codex"],
        &[&home, &big],
        &d,
    );
    assert!(swapped(&c), "{:?}", c.codex_log());
}
