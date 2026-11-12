//! Issue #2042: `amplihack profile rotate --dry-run` reads the fake process
//! table, and a refused table is named on stderr instead of being replaced
//! by the host `/proc` without a word.
//!
//! The refusal line is printed by debug builds only, so its checks run only
//! when this test binary is a debug build, like the `amplihack` binary cargo
//! builds beside it. Fake pids lie above `pid_max`, so no signal can reach a
//! host process. Linux-only, like the readers under test.

#![cfg(target_os = "linux")]

use crate::support::fake_proc::{FAKE_PID_BASE, FakeProc};
use crate::support::profile_rotate::*;
use crate::support::wait::output_within;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Output;

use crate::isolated_amplihack::PROC_ROOT_REFUSED as IGNORED;

/// `amplihack ARGS` through the fixture, reading `tree` as `/proc`.
fn run_on(f: &Fixture, tree: &Path, args: &[&str]) -> Output {
    let mut c = f.cmd(args);
    c.env(crate::isolated_amplihack::PROC_ROOT_ENV, tree);
    let out = output_within(&format!("`amplihack {}`", args.join(" ")), &mut c);
    assert_no_secrets(&out);
    out
}

fn report(out: &Output) -> String {
    format!(
        "status {}\nstdout: {}\nstderr: {}",
        out.status,
        text(&out.stdout),
        text(&out.stderr)
    )
}

#[test]
fn rotate_dry_run_skips_a_target_with_a_fake_session() {
    let f = Fixture::new();
    f.usage_direct("work", 90, 20);
    f.usage_direct("spare", 10, 20);
    f.usage_direct("extra", 50, 60);
    let swap = "would swap work->spare";
    let empty = run_on(
        &f,
        crate::isolated_amplihack::shared_empty_proc_root(),
        &["profile", "rotate", "--dry-run"],
    );
    assert!(empty.status.success(), "{}", report(&empty));
    assert!(text(&empty.stdout).contains(swap), "{}", report(&empty));

    let fake = FakeProc::new();
    let dir = f.root.join("spare");
    fake.process(FAKE_PID_BASE)
        .cmdline(&["claude"])
        .status('S')
        .environ(&[("CLAUDE_CONFIG_DIR", dir.to_str().unwrap())]);
    let busy = run_on(&f, fake.path(), &["profile", "rotate", "--dry-run"]);
    assert!(busy.status.success(), "{}", report(&busy));
    assert!(
        !text(&busy.stdout).contains(swap),
        "the child read the host /proc, not the fake tree: {}",
        report(&busy)
    );
}

#[test]
fn a_refused_tree_is_named_on_stderr_and_an_accepted_one_is_not() {
    let f = Fixture::new();
    let accepted = f.run(&["profile", "rotate", "status"]);
    assert!(
        !text(&accepted.stderr).contains(IGNORED),
        "{}",
        report(&accepted)
    );

    let fake = FakeProc::new();
    fs::set_permissions(fake.path(), fs::Permissions::from_mode(0o777)).unwrap();
    let refused = run_on(&f, fake.path(), &["profile", "rotate", "status"]);
    let stderr = text(&refused.stderr);
    if cfg!(debug_assertions) {
        let line = format!("{IGNORED} (others can write the directory); reading /proc");
        assert_eq!(stderr.matches(IGNORED).count(), 1, "{}", report(&refused));
        assert!(stderr.contains(&line), "{}", report(&refused));
    } else {
        assert!(!stderr.contains(IGNORED), "{}", report(&refused));
    }
}
