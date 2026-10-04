//! One process asks "which claude?" at least twice — the update notice, then
//! the install decision — and each answer used to cost a `--version`
//! subprocess against a ~339 MB binary. This file pins the memo that removed
//! the repeat.
//!
//! Its own test binary, with exactly one test in it: the test sets `PATH` and
//! `HOME`, which is process-global and `unsafe` under edition 2024. Nothing
//! else runs here to race with it.

use std::path::Path;

/// A fake `claude` that records every invocation.
fn write_recording_claude(dir: &Path, ledger: &Path) {
    let script = format!(
        "#!/bin/sh\n\
         printf 'ran\\n' >> \"{ledger}\"\n\
         printf '9.9.9\\n'\n",
        ledger = ledger.display(),
    );
    let path = dir.join("claude");
    std::fs::write(&path, script).unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(&path, perms).unwrap();
}

fn probe_count(ledger: &Path) -> usize {
    std::fs::read_to_string(ledger)
        .map(|text| text.lines().count())
        .unwrap_or(0)
}

#[cfg(target_os = "linux")]
#[test]
fn resolve_probes_once_and_resolve_uncached_reprobes() {
    let temp = tempfile::tempdir().unwrap();
    let bin = temp.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let ledger = temp.path().join("probes");
    write_recording_claude(&bin, &ledger);

    // HOME points at the empty temp dir so the npm-prefix and fallback
    // candidates cannot find a real claude on this host.
    unsafe {
        std::env::set_var("PATH", &bin);
        std::env::set_var("HOME", temp.path());
    }

    let first = amplihack_utils::launch_target::resolve(
        "claude",
        amplihack_utils::launch_target::OverrideOrigin::User,
    );
    let second = amplihack_utils::launch_target::resolve(
        "claude",
        amplihack_utils::launch_target::OverrideOrigin::User,
    );

    assert_eq!(
        first.target.as_ref().map(|t| t.version.as_str()),
        Some("9.9.9"),
        "the fake claude should resolve"
    );
    assert_eq!(first, second, "the memoized answer must be the same answer");
    assert_eq!(
        probe_count(&ledger),
        1,
        "the second resolve must be served from the memo, not a second \
         --version subprocess"
    );

    // An install changes the filesystem, which the memo cannot see. That path
    // re-probes.
    let third = amplihack_utils::launch_target::resolve_uncached(
        "claude",
        amplihack_utils::launch_target::OverrideOrigin::User,
    );
    assert_eq!(third, first);
    assert_eq!(
        probe_count(&ledger),
        2,
        "resolve_uncached must re-probe rather than read the memo"
    );

    // ...and it leaves the memo holding the fresh answer.
    let fourth = amplihack_utils::launch_target::resolve(
        "claude",
        amplihack_utils::launch_target::OverrideOrigin::User,
    );
    assert_eq!(fourth, third);
    assert_eq!(
        probe_count(&ledger),
        2,
        "resolve_uncached must refresh the memo, not bypass it"
    );

    // The memo is validated against the candidate list, not just the tool
    // name. A changed $PATH is a different list, so it misses rather than
    // answering about a binary that is no longer the one we would launch.
    let other_bin = temp.path().join("other-bin");
    std::fs::create_dir(&other_bin).unwrap();
    let other_ledger = temp.path().join("other-probes");
    write_recording_claude(&other_bin, &other_ledger);
    let other_claude = other_bin.join("claude");
    let text = std::fs::read_to_string(&other_claude)
        .unwrap()
        .replace("9.9.9", "8.8.8");
    std::fs::write(&other_claude, text).unwrap();
    unsafe { std::env::set_var("PATH", &other_bin) };

    let after_path_change = amplihack_utils::launch_target::resolve(
        "claude",
        amplihack_utils::launch_target::OverrideOrigin::User,
    );
    assert_eq!(
        after_path_change
            .target
            .as_ref()
            .map(|t| t.version.as_str()),
        Some("8.8.8"),
        "a changed $PATH must miss the memo, not return the stale answer"
    );
    assert_eq!(probe_count(&other_ledger), 1);
}
