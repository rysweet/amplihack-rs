//! Issue #2496: the gate every secondary implicit publisher consults.
//!
//! Launch hook settings, profile seeding, the rotator and the launcher
//! preparation all ask one question before writing to the host: may this
//! executable publish installation assets implicitly? The gate folds three
//! independent conditions, and each must refuse on its own:
//!
//! - the automatic-install opt-out, which is advisory and may be absent;
//! - the executable's origin, which holds whether or not the opt-out is set;
//! - the install stamp, which never authorises a publication the first two
//!   have refused.
//!
//! The gate is named for what it decides rather than for the stamp it
//! happens to read, because version ordering is the weakest of the three.

use super::issue_1526_implicit_downgrade::IsolatedHome;
use super::*;

/// A checkout, with nothing else wrong: the stamp matches this binary
/// exactly, so only origin can refuse.
#[test]
fn a_checkout_may_not_publish_implicitly() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let home = temp.path();
    let _env = IsolatedHome::new(home);
    version_stamp::write_installed_version(crate::VERSION).unwrap();

    assert!(
        !may_publish_implicitly(),
        "a matching stamp is not authority to publish from a checkout"
    );
}

#[test]
fn a_complete_installed_release_may_publish_implicitly() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let home = temp.path();
    let _env = IsolatedHome::installed(home);
    version_stamp::write_installed_version(crate::VERSION).unwrap();

    assert!(
        may_publish_implicitly(),
        "an eligible release with a current stamp publishes as before"
    );
}

/// A3: the opt-out and the origin guard are independent controls. This case
/// isolates the opt-out by making origin eligible.
#[test]
fn the_opt_out_alone_refuses_publication_from_an_eligible_release() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let home = temp.path();
    let mut env = IsolatedHome::installed(home);
    env.set("AMPLIHACK_SKIP_AUTO_INSTALL", "1");
    version_stamp::write_installed_version(crate::VERSION).unwrap();

    assert!(
        !may_publish_implicitly(),
        "the opt-out suppresses hook, profile and provider publication too"
    );
}

/// The documented rule is "any non-empty value disables". An empty value is
/// not a request for silence, and must not quietly disable publication.
#[test]
fn an_empty_opt_out_value_does_not_refuse_publication() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let home = temp.path();
    let mut env = IsolatedHome::installed(home);
    env.set("AMPLIHACK_SKIP_AUTO_INSTALL", "");
    version_stamp::write_installed_version(crate::VERSION).unwrap();

    assert!(
        may_publish_implicitly(),
        "only a non-empty opt-out value disables automatic installation"
    );
}

/// The stamp still refuses on its own, from an eligible release without the
/// opt-out: issue #1526 is not weakened by the origin guard.
#[test]
fn a_newer_stamp_alone_still_refuses_publication() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let home = temp.path();
    let _env = IsolatedHome::installed(home);
    version_stamp::write_installed_version(&super::issue_1955_support::newer_than_binary())
        .unwrap();

    assert!(
        !may_publish_implicitly(),
        "an older binary never publishes over a newer install"
    );
}
