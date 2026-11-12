//! Issue #1962, D1 and S1: the login directory rule, with the private-group and ACL
//! answers passed in, so both answers are tested on every host.
//!
//! Contract the implementation must meet:
//! - `mode_lets_others_write(mode: u32, group_is_private: bool) -> bool`
//! - `dir_rule(mode: u32, group_is_private: bool,
//!   acl: impl FnOnce() -> std::io::Result<()>) -> bool`. It is `true` when others
//!   can write. `acl` reads `system.posix_acl_access`, and `Ok(())` means an ACL is
//!   present. It is consulted only when the group write bit is set.

use super::{dir_rule, mode_lets_others_write};
use std::io;

fn errno(code: i32) -> io::Result<()> {
    Err(io::Error::from_raw_os_error(code))
}

fn never() -> io::Result<()> {
    panic!("the ACL is read only when the group write bit is set")
}

#[test]
fn mode_table() {
    for (mode, private, want) in [
        (0o700, false, false),
        (0o755, false, false),
        (0o775, true, false),
        (0o775, false, true),
        (0o2775, true, false),
        (0o770, true, false),
        (0o770, false, true),
        (0o777, true, true),
        (0o757, true, true),
        (0o702, true, true),
    ] {
        assert_eq!(
            mode_lets_others_write(mode, private),
            want,
            "{mode:o} private={private}"
        );
    }
}

#[test]
fn no_group_write_reads_no_acl() {
    assert!(!dir_rule(0o700, false, never));
    assert!(!dir_rule(0o755, true, never));
    assert!(
        dir_rule(0o707, true, never),
        "world write refuses without the ACL"
    );
}

#[test]
fn shared_group_is_refused_without_an_acl_read_mattering() {
    assert!(dir_rule(0o775, false, || errno(libc::ENODATA)));
}

#[test]
fn private_group_with_no_acl_is_accepted() {
    assert!(!dir_rule(0o775, true, || errno(libc::ENODATA)));
    assert!(
        !dir_rule(0o775, true, || errno(libc::ENOTSUP)),
        "no ACL support is no ACL"
    );
}

#[test]
fn private_group_with_an_acl_is_refused() {
    assert!(dir_rule(0o775, true, || Ok(())));
}

#[test]
fn an_acl_that_cannot_be_checked_is_refused() {
    for code in [
        libc::EACCES,
        libc::EIO,
        libc::ELOOP,
        libc::ENOENT,
        libc::ENOMEM,
    ] {
        assert!(dir_rule(0o775, true, || errno(code)), "errno {code}");
    }
}
