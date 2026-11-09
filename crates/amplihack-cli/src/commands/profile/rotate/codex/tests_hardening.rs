//! Unit tests for the post-merge hardening of the Codex rotation (issue
//! #2005): the re-check before the rename (FS-3) and the one bounded wait
//! of `codex-resume` (EXEC-4). Wired from `rotate/codex/mod.rs` with:
//!
//! ```ignore
//! #[cfg(test)]
//! mod tests_hardening;
//! ```
//!
//! Contract these tests define (all `pub(super)`):
//!
//! - `link::relink_checked(link, expect: &Expect, new_target, fault) ->
//!   io::Result<()>` with `link::Expect { old_target: &Path, old_ino: u64,
//!   new_ino: u64 }`. Just before the rename it inspects the link again: the
//!   link must still point at `old_target` with inode `old_ino`, and
//!   `new_target` must still have inode `new_ino`. Otherwise it returns an
//!   error of kind `io::ErrorKind::Other` whose text contains
//!   `journal-mismatch`, renames nothing, and leaves no temp symlink.
//! - `resume::MAX_WAIT_SECS: i64 = 7 * 86_400`.
//! - `resume::wait_plan(earliest_reset: Option<i64>, now: i64) -> Wait`,
//!   `Wait::{Unknown, Now, Until(i64)}`: no reset known is `Unknown` (exit 4,
//!   `no-reset-known`); a reset at or before `now` is `Now` (no sleep); a
//!   later reset is `Until(min(reset, now + MAX_WAIT_SECS))`.

use super::link::{Expect, LinkState, inspect, relink_checked, swap_tmp};
use super::resume::{MAX_WAIT_SECS, Wait, wait_plan};
use std::fs;
use std::os::unix::fs::{MetadataExt, symlink};
use std::path::PathBuf;

const NOW: i64 = 1_794_200_000;

struct T {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    link: PathBuf,
}

fn setup() -> T {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let root = base.join("profiles");
    for a in ["a", "b", "c"] {
        fs::create_dir_all(root.join(a)).unwrap();
    }
    let link = base.join("home").join(".codex");
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    symlink(root.join("a"), &link).unwrap();
    T {
        _tmp: tmp,
        root,
        link,
    }
}

fn ino(p: &std::path::Path) -> u64 {
    fs::metadata(p).unwrap().ino()
}

fn is_mismatch(e: &std::io::Error) -> bool {
    e.to_string().contains("journal-mismatch")
}

// ---- FS-3: the link and the new target are checked again before the rename ----

#[test]
fn relink_checked_moves_the_link_when_nothing_changed() {
    let t = setup();
    let (a, b) = (t.root.join("a"), t.root.join("b"));
    let expect = Expect {
        old_target: &a,
        old_ino: ino(&a),
        new_ino: ino(&b),
    };
    relink_checked(&t.link, &expect, &b, None).unwrap();
    assert_eq!(fs::read_link(&t.link).unwrap(), b);
    assert!(fs::symlink_metadata(swap_tmp(&t.link)).is_err());
}

#[test]
fn relink_checked_refuses_when_the_link_moved_since_the_journal() {
    let t = setup();
    let (a, b, c) = (t.root.join("a"), t.root.join("b"), t.root.join("c"));
    let expect = Expect {
        old_target: &a,
        old_ino: ino(&a),
        new_ino: ino(&b),
    };
    // Someone else re-pointed the link after the journal was written.
    fs::remove_file(&t.link).unwrap();
    symlink(&c, &t.link).unwrap();

    let e = relink_checked(&t.link, &expect, &b, None).unwrap_err();
    assert!(is_mismatch(&e), "{e}");
    assert_eq!(fs::read_link(&t.link).unwrap(), c, "the link is left alone");
    assert!(fs::symlink_metadata(swap_tmp(&t.link)).is_err());
}

#[test]
fn relink_checked_refuses_when_the_new_account_was_replaced() {
    let t = setup();
    let (a, b) = (t.root.join("a"), t.root.join("b"));
    let expect = Expect {
        old_target: &a,
        old_ino: ino(&a),
        new_ino: ino(&b),
    };
    // The account directory was replaced: same name, another inode.
    fs::rename(&b, t.root.join("b-old")).unwrap();
    fs::create_dir(&b).unwrap();
    assert_ne!(ino(&b), expect.new_ino);

    let e = relink_checked(&t.link, &expect, &b, None).unwrap_err();
    assert!(is_mismatch(&e), "{e}");
    assert_eq!(fs::read_link(&t.link).unwrap(), a);
    assert!(fs::symlink_metadata(swap_tmp(&t.link)).is_err());
}

#[test]
fn relink_checked_refuses_when_the_old_account_was_replaced() {
    let t = setup();
    let (a, b) = (t.root.join("a"), t.root.join("b"));
    let expect = Expect {
        old_target: &a,
        old_ino: ino(&a),
        new_ino: ino(&b),
    };
    fs::rename(&a, t.root.join("a-old")).unwrap();
    fs::create_dir(&a).unwrap();

    let e = relink_checked(&t.link, &expect, &b, None).unwrap_err();
    assert!(is_mismatch(&e), "{e}");
    assert!(matches!(inspect(&t.link, &t.root), LinkState::Link { label, .. } if label == "a"));
}

#[test]
fn relink_checked_refuses_when_the_link_became_a_real_directory() {
    let t = setup();
    let (a, b) = (t.root.join("a"), t.root.join("b"));
    let expect = Expect {
        old_target: &a,
        old_ino: ino(&a),
        new_ino: ino(&b),
    };
    fs::remove_file(&t.link).unwrap();
    fs::create_dir(&t.link).unwrap();
    fs::write(t.link.join("keep"), "x").unwrap();

    let e = relink_checked(&t.link, &expect, &b, None).unwrap_err();
    assert!(is_mismatch(&e), "{e}");
    assert_eq!(fs::read_to_string(t.link.join("keep")).unwrap(), "x");
}

// ---- EXEC-4: one wait, bounded, never into the past ----

#[test]
fn the_longest_wait_is_seven_days() {
    assert_eq!(MAX_WAIT_SECS, 604_800);
}

#[test]
fn no_known_reset_is_unknown_and_never_sleeps() {
    assert_eq!(wait_plan(None, NOW), Wait::Unknown);
}

#[test]
fn a_reset_in_the_past_or_now_does_not_sleep() {
    assert_eq!(wait_plan(Some(NOW - 1), NOW), Wait::Now);
    assert_eq!(wait_plan(Some(NOW), NOW), Wait::Now);
    assert_eq!(wait_plan(Some(i64::MIN), NOW), Wait::Now);
}

#[test]
fn a_reset_within_seven_days_is_waited_for_exactly() {
    assert_eq!(wait_plan(Some(NOW + 1), NOW), Wait::Until(NOW + 1));
    assert_eq!(
        wait_plan(Some(NOW + MAX_WAIT_SECS), NOW),
        Wait::Until(NOW + MAX_WAIT_SECS)
    );
}

#[test]
fn a_reset_beyond_seven_days_is_cut_to_seven_days() {
    assert_eq!(
        wait_plan(Some(NOW + MAX_WAIT_SECS + 1), NOW),
        Wait::Until(NOW + MAX_WAIT_SECS)
    );
    assert_eq!(
        wait_plan(Some(i64::MAX), NOW),
        Wait::Until(NOW + MAX_WAIT_SECS),
        "a huge reset must not overflow"
    );
}

// EXEC-1 / S3: `bin_in_home(bin, root_c, link, live)`.

mod bin_in_home {
    use super::super::bin_in_home;
    use std::fs;
    use std::os::unix::fs::symlink;

    struct B {
        _tmp: tempfile::TempDir,
        root: std::path::PathBuf,
        link: std::path::PathBuf,
        live: std::path::PathBuf,
        base: std::path::PathBuf,
    }

    fn setup() -> B {
        let tmp = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(tmp.path()).unwrap();
        let root = base.join("root");
        let live = root.join("acct");
        fs::create_dir_all(&live).unwrap();
        let link = base.join(".codex");
        symlink(&live, &link).unwrap();
        B {
            _tmp: tmp,
            root,
            link,
            live,
            base,
        }
    }

    fn bin(at: &std::path::Path) -> std::path::PathBuf {
        let p = at.join("bin").join("codex");
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, "").unwrap();
        p
    }

    #[test]
    fn a_program_under_the_root_is_in_home() {
        let b = setup();
        let other = b.root.join("other");
        let p = bin(&other);
        assert!(bin_in_home(&p, &b.root, &b.link, None));
        assert!(bin_in_home(&p, &b.root, &b.link, Some(&b.live)));
    }

    #[test]
    fn a_program_under_the_link_is_in_home() {
        let b = setup();
        bin(&b.live);
        let p = b.link.join("bin").join("codex");
        assert!(bin_in_home(&p, &b.root, &b.link, None));
    }

    #[test]
    fn a_program_elsewhere_is_not() {
        let b = setup();
        let p = bin(&b.base.join("usr"));
        assert!(!bin_in_home(&p, &b.root, &b.link, Some(&b.live)));
    }

    #[test]
    fn a_program_that_cannot_be_resolved_fails_closed() {
        let b = setup();
        let p = b.base.join("usr").join("missing-codex");
        assert!(bin_in_home(&p, &b.root, &b.link, None));
    }
}
