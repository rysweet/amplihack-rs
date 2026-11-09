//! `codex::inuse` (issue #2005) against a fake `/proc` tree.
//!
//! Contract: `default_home_in_use(proc_root: &Path, link: &Path, uid: u32)
//! -> InUse`, `InUse::{Free, Pid(u32), ProcUnreadable}`. Only entries owned
//! by `uid` are examined; unreadable data fails closed.

use super::inuse::{InUse, default_home_in_use};
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

fn uid() -> u32 {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() }
}

fn nul(v: &[&str]) -> Vec<u8> {
    let mut b = Vec::new();
    for s in v {
        b.extend_from_slice(s.as_bytes());
        b.push(0);
    }
    b
}

fn proc_entry(
    root: &Path,
    pid: u32,
    comm: &str,
    exe: &str,
    argv: &[&str],
    env: &[&str],
    cwd: &Path,
) -> PathBuf {
    let d = root.join(pid.to_string());
    fs::create_dir_all(&d).unwrap();
    fs::write(d.join("comm"), format!("{comm}\n")).unwrap();
    symlink(exe, d.join("exe")).unwrap();
    fs::write(d.join("cmdline"), nul(argv)).unwrap();
    fs::write(d.join("environ"), nul(env)).unwrap();
    symlink(cwd, d.join("cwd")).unwrap();
    d
}

struct T {
    tmp: tempfile::TempDir,
    proc_root: PathBuf,
    link: PathBuf,
}

fn setup() -> T {
    let tmp = tempfile::tempdir().unwrap();
    let proc_root = tmp.path().join("proc");
    fs::create_dir_all(&proc_root).unwrap();
    let link = tmp.path().join("home").join(".codex");
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    T {
        proc_root,
        link,
        tmp,
    }
}

#[test]
fn an_empty_proc_is_free_and_a_missing_one_is_unreadable() {
    let t = setup();
    assert_eq!(
        default_home_in_use(&t.proc_root, &t.link, uid()),
        InUse::Free
    );
    let gone = t.tmp.path().join("nope");
    assert_eq!(
        default_home_in_use(&gone, &t.link, uid()),
        InUse::ProcUnreadable
    );
}

#[test]
fn codex_without_codex_home_is_in_use() {
    let t = setup();
    proc_entry(
        &t.proc_root,
        10,
        "codex",
        "/usr/bin/codex",
        &["codex"],
        &["A=1"],
        t.tmp.path(),
    );
    assert_eq!(
        default_home_in_use(&t.proc_root, &t.link, uid()),
        InUse::Pid(10)
    );
}

#[test]
fn codex_home_equal_to_the_link_is_in_use_and_another_is_not() {
    let t = setup();
    let same = format!("CODEX_HOME={}", t.link.display());
    proc_entry(
        &t.proc_root,
        11,
        "codex",
        "/usr/bin/codex",
        &["codex"],
        &[&same],
        t.tmp.path(),
    );
    assert_eq!(
        default_home_in_use(&t.proc_root, &t.link, uid()),
        InUse::Pid(11)
    );

    let t = setup();
    let trailing = format!("CODEX_HOME={}/", t.link.display());
    proc_entry(
        &t.proc_root,
        12,
        "codex",
        "/usr/bin/codex",
        &["codex"],
        &[&trailing],
        t.tmp.path(),
    );
    assert_eq!(
        default_home_in_use(&t.proc_root, &t.link, uid()),
        InUse::Pid(12),
        "same path"
    );

    let t = setup();
    proc_entry(
        &t.proc_root,
        13,
        "codex",
        "/usr/bin/codex",
        &["codex"],
        &["CODEX_HOME=/r/acct"],
        t.tmp.path(),
    );
    assert_eq!(
        default_home_in_use(&t.proc_root, &t.link, uid()),
        InUse::Free
    );
}

#[test]
fn entries_of_another_uid_are_ignored() {
    let t = setup();
    proc_entry(
        &t.proc_root,
        14,
        "codex",
        "/usr/bin/codex",
        &["codex"],
        &[],
        t.tmp.path(),
    );
    assert_eq!(
        default_home_in_use(&t.proc_root, &t.link, uid().wrapping_add(1)),
        InUse::Free
    );
}

#[test]
fn node_running_codex_is_matched_by_its_first_argument() {
    let t = setup();
    proc_entry(
        &t.proc_root,
        15,
        "node",
        "/usr/bin/node",
        &["node", "/x/bin/codex"],
        &[],
        t.tmp.path(),
    );
    assert_eq!(
        default_home_in_use(&t.proc_root, &t.link, uid()),
        InUse::Pid(15)
    );
    let t = setup();
    proc_entry(
        &t.proc_root,
        16,
        "node",
        "/usr/bin/node",
        &["node", "/x/codex.js"],
        &[],
        t.tmp.path(),
    );
    assert_eq!(
        default_home_in_use(&t.proc_root, &t.link, uid()),
        InUse::Pid(16)
    );
    let t = setup();
    proc_entry(
        &t.proc_root,
        17,
        "node",
        "/usr/bin/node",
        &["node", "/x/codexish.js"],
        &[],
        t.tmp.path(),
    );
    assert_eq!(
        default_home_in_use(&t.proc_root, &t.link, uid()),
        InUse::Free
    );
}

#[test]
fn unreadable_environ_or_cwd_fails_closed() {
    let t = setup();
    let d = proc_entry(
        &t.proc_root,
        18,
        "codex",
        "/usr/bin/codex",
        &["codex"],
        &[],
        t.tmp.path(),
    );
    fs::set_permissions(d.join("environ"), fs::Permissions::from_mode(0o000)).unwrap();
    assert_eq!(
        default_home_in_use(&t.proc_root, &t.link, uid()),
        InUse::Pid(18)
    );

    let t = setup();
    let d = proc_entry(
        &t.proc_root,
        19,
        "codex",
        "/usr/bin/codex",
        &["codex"],
        &["CODEX_HOME=rel"],
        t.tmp.path(),
    );
    fs::remove_file(d.join("cwd")).unwrap();
    assert_eq!(
        default_home_in_use(&t.proc_root, &t.link, uid()),
        InUse::Pid(19)
    );
}

#[test]
fn a_relative_codex_home_resolves_against_the_process_cwd() {
    let t = setup();
    let home = t.link.parent().unwrap().to_path_buf();
    proc_entry(
        &t.proc_root,
        20,
        "codex",
        "/usr/bin/codex",
        &["codex"],
        &["CODEX_HOME=./.codex"],
        &home,
    );
    assert_eq!(
        default_home_in_use(&t.proc_root, &t.link, uid()),
        InUse::Pid(20)
    );
}

#[test]
fn a_vanished_process_is_skipped() {
    let t = setup();
    fs::create_dir_all(t.proc_root.join("21")).unwrap();
    assert_eq!(
        default_home_in_use(&t.proc_root, &t.link, uid()),
        InUse::Free
    );
}

/// The link as the rotator sees it, `<tmp>/real/.codex -> <tmp>/acct`, with
/// `<tmp>/alias -> <tmp>/real`.
fn aliased() -> T {
    let tmp = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(tmp.path()).unwrap();
    let proc_root = base.join("proc");
    fs::create_dir_all(&proc_root).unwrap();
    fs::create_dir_all(base.join("real")).unwrap();
    fs::create_dir_all(base.join("acct")).unwrap();
    let link = base.join("real").join(".codex");
    symlink(base.join("acct"), &link).unwrap();
    symlink(base.join("real"), base.join("alias")).unwrap();
    T {
        proc_root,
        link,
        tmp,
    }
}

fn one_codex(t: &T, pid: u32, home: &Path) -> InUse {
    let env = format!("CODEX_HOME={}", home.display());
    proc_entry(
        &t.proc_root,
        pid,
        "codex",
        "/usr/bin/codex",
        &["codex"],
        &[&env],
        t.tmp.path(),
    );
    default_home_in_use(&t.proc_root, &t.link, uid())
}

fn base(t: &T) -> PathBuf {
    fs::canonicalize(t.tmp.path()).unwrap()
}

#[test]
fn a_codex_home_through_a_symlinked_parent_is_the_link() {
    // S1: `$HOME` reached by another path names the same link.
    let t = aliased();
    let home = base(&t).join("alias").join(".codex");
    assert_eq!(one_codex(&t, 30, &home), InUse::Pid(30));
}

#[test]
fn a_codex_home_that_is_a_symlink_to_the_link_is_the_link() {
    let t = aliased();
    let alias = base(&t).join("codex-alias");
    symlink(&t.link, &alias).unwrap();
    assert_eq!(one_codex(&t, 31, &alias), InUse::Pid(31));

    // A relative alias, two hops away.
    let t = aliased();
    let b = base(&t);
    symlink("real/.codex", b.join("hop1")).unwrap();
    symlink("hop1", b.join("hop2")).unwrap();
    assert_eq!(one_codex(&t, 32, &b.join("hop2")), InUse::Pid(32));
}

#[test]
fn a_codex_home_that_is_the_account_itself_is_free() {
    // The link's target is not the link: a swap does not move it.
    let t = aliased();
    let acct = base(&t).join("acct");
    assert_eq!(one_codex(&t, 33, &acct), InUse::Free);
}

#[test]
fn a_symlink_loop_counts_as_in_use() {
    let t = aliased();
    let b = base(&t);
    symlink("loop2", b.join("loop1")).unwrap();
    symlink("loop1", b.join("loop2")).unwrap();
    assert_eq!(one_codex(&t, 34, &b.join("loop1")), InUse::Pid(34));
}

#[test]
fn the_link_by_its_own_text_is_still_matched() {
    // Regression: the plain text match is kept.
    let t = aliased();
    let home = PathBuf::from(format!(
        "{}/./../real/.codex",
        t.link.parent().unwrap().display()
    ));
    assert_eq!(one_codex(&t, 35, &home), InUse::Pid(35));
}
