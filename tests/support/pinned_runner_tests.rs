//! Unit tests for `tests/support/pinned_runner.rs` (issue #2658).
//!
//! Included by `#[path]` from `tests/integration/main.rs` only, so they run
//! in one test binary. `crates/amplihack-cli/tests/main.rs` includes the
//! helper but not these tests.

use crate::pinned_runner::*;
use std::ffi::OsStr;
use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const SOURCE: &str = "git+https://github.com/rysweet/amplihack-recipe-runner";
const OLD_PIN: &str = "90286bc50b84ded773795d2c8301b689f680db0b";
const PIN_MESSAGE: &str = "recipe-runner.rev is not a 40-character lowercase revision";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn real_pin() -> String {
    fs::read_to_string(repo_root().join("claude-plugin/recipe-runner.rev"))
        .expect("read claude-plugin/recipe-runner.rev")
        .trim()
        .to_owned()
}

fn panic_message(f: impl FnOnce()) -> String {
    let payload = catch_unwind(AssertUnwindSafe(f)).expect_err("expected a panic");
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
        .unwrap_or_default()
}

fn rev_key(pin: &str) -> String {
    format!("recipe-runner-rs 0.7.0 ({SOURCE}?rev={pin}#{pin})")
}

/// An install root `<dir>/root` with `bin/recipe-runner-rs` and a
/// `.crates2.json` holding `key` with `bins`.
fn install(dir: &Path, key: &str, bins: &[&str], with_binary: bool) -> (PathBuf, PathBuf) {
    let root = dir.join("root");
    fs::create_dir_all(root.join("bin")).unwrap();
    let bin = root.join("bin/recipe-runner-rs");
    if with_binary {
        fs::write(&bin, b"#!/bin/sh\n").unwrap();
    }
    let receipt = root.join(".crates2.json");
    let body = serde_json::json!({
        "installs": { key: { "version_req": null, "bins": bins, "features": [] } }
    });
    fs::write(&receipt, serde_json::to_vec(&body).unwrap()).unwrap();
    (bin, receipt)
}

fn assert_rejected(key: &str, bins: &[&str], pin: &str) {
    let dir = tempfile::tempdir().unwrap();
    let (bin, receipt) = install(dir.path(), key, bins, true);
    let message = panic_message(|| assert_receipt(&bin, &receipt, pin));
    let expected = format!(
        "receipt {} does not name recipe-runner-rs at #{pin}",
        receipt.display()
    );
    assert!(
        message.contains(&expected),
        "want {expected:?} in panic, got {message:?}"
    );
}

// ---- validate_pin --------------------------------------------------

#[test]
fn validate_pin_accepts_the_checked_in_pin() {
    let pin = real_pin();
    assert_eq!(validate_pin(&pin), pin);
}

#[test]
fn validate_pin_rejects_malformed_revisions() {
    let pin = real_pin();
    for bad in [
        "../x".to_owned(),
        "2833a0b8".to_owned(),
        pin.to_uppercase(),
        format!("-{}", &pin[1..]),
        String::new(),
        format!("{pin}0"),
        pin[..39].to_owned(),
        format!("{}g", &pin[..39]),
        format!("{} {}", &pin[..20], &pin[21..]),
    ] {
        let message = panic_message(|| {
            validate_pin(&bad);
        });
        assert!(
            message.contains(PIN_MESSAGE),
            "{bad:?} must be rejected with {PIN_MESSAGE:?}, got {message:?}"
        );
    }
}

// ---- assert_receipt ------------------------------------------------

#[test]
fn assert_receipt_accepts_cargo_rev_style_key() {
    let pin = real_pin();
    let dir = tempfile::tempdir().unwrap();
    let (bin, receipt) = install(dir.path(), &rev_key(&pin), &["recipe-runner-rs"], true);
    assert_receipt(&bin, &receipt, &pin);
}

#[test]
fn assert_receipt_accepts_plain_and_dot_git_source_forms() {
    let pin = real_pin();
    for key in [
        format!("recipe-runner-rs 0.7.0 ({SOURCE}#{pin})"),
        format!("recipe-runner-rs 0.7.0 ({SOURCE}.git?rev={pin}#{pin})"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let (bin, receipt) = install(dir.path(), &key, &["recipe-runner-rs"], true);
        assert_receipt(&bin, &receipt, &pin);
    }
}

#[test]
fn assert_receipt_accepts_receipt_with_other_installs_alongside() {
    let pin = real_pin();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    fs::create_dir_all(root.join("bin")).unwrap();
    let bin = root.join("bin/recipe-runner-rs");
    fs::write(&bin, b"").unwrap();
    let receipt = root.join(".crates2.json");
    let body = serde_json::json!({ "installs": {
        "ripgrep 14.1.0 (registry+https://github.com/rust-lang/crates.io-index)": { "bins": ["rg"] },
        rev_key(&pin): { "bins": ["recipe-runner-rs"] },
    }});
    fs::write(&receipt, serde_json::to_vec(&body).unwrap()).unwrap();
    assert_receipt(&bin, &receipt, &pin);
}

#[test]
fn assert_receipt_rejects_the_old_host_pin() {
    // The failure mode of #2658: a receipt for 90286bc must never pass for
    // 2833a0b8.
    let pin = real_pin();
    assert_rejected(&rev_key(OLD_PIN), &["recipe-runner-rs"], &pin);
}

#[test]
fn assert_receipt_rejects_pin_as_prefix_of_longer_revision_text() {
    let pin = real_pin();
    let key = format!("recipe-runner-rs 0.7.0 ({SOURCE}?rev={pin}#{pin}x)");
    assert_rejected(&key, &["recipe-runner-rs"], &pin);
}

#[test]
fn assert_receipt_rejects_missing_runner_bin() {
    let pin = real_pin();
    assert_rejected(&rev_key(&pin), &["recipe-runner"], &pin);
    assert_rejected(&rev_key(&pin), &[], &pin);
}

#[test]
fn assert_receipt_rejects_other_crate_name() {
    let pin = real_pin();
    let key = format!("recipe-runner-rs-extra 0.7.0 ({SOURCE}?rev={pin}#{pin})");
    assert_rejected(&key, &["recipe-runner-rs"], &pin);
}

#[test]
fn assert_receipt_rejects_fork_and_lookalike_sources() {
    let pin = real_pin();
    for source in [
        "git+https://github.com/rysweet/amplihack-recipe-runner-fork",
        "git+https://github.com/someone/amplihack-recipe-runner",
        "git+http://github.com/rysweet/amplihack-recipe-runner",
        "git+file:///home/x/amplihack-recipe-runner",
        "git+ssh://git@github.com/rysweet/amplihack-recipe-runner",
    ] {
        let key = format!("recipe-runner-rs 0.7.0 ({source}?rev={pin}#{pin})");
        assert_rejected(&key, &["recipe-runner-rs"], &pin);
    }
}

#[test]
fn assert_receipt_rejects_source_url_present_only_as_substring() {
    let pin = real_pin();
    let key = format!(
        "recipe-runner-rs 0.7.0 (registry+https://evil.example/?u={SOURCE}?rev={pin}#{pin})"
    );
    assert_rejected(&key, &["recipe-runner-rs"], &pin);
}

#[test]
fn assert_receipt_panics_on_malformed_json_naming_the_receipt() {
    let pin = real_pin();
    let dir = tempfile::tempdir().unwrap();
    let (bin, receipt) = install(dir.path(), &rev_key(&pin), &["recipe-runner-rs"], true);
    fs::write(&receipt, b"{ not json").unwrap();
    let message = panic_message(|| assert_receipt(&bin, &receipt, &pin));
    assert!(
        message.contains(&receipt.display().to_string()),
        "malformed receipt must panic naming {}, got {message:?}",
        receipt.display()
    );
}

#[test]
fn assert_receipt_panics_on_missing_installs_map() {
    let pin = real_pin();
    let dir = tempfile::tempdir().unwrap();
    let (bin, receipt) = install(dir.path(), &rev_key(&pin), &["recipe-runner-rs"], true);
    fs::write(&receipt, br#"{"v1": {}}"#).unwrap();
    let message = panic_message(|| assert_receipt(&bin, &receipt, &pin));
    assert!(
        message.contains(&receipt.display().to_string()),
        "receipt without installs must panic naming {}, got {message:?}",
        receipt.display()
    );
}

#[test]
fn assert_receipt_panics_on_missing_receipt_file() {
    let pin = real_pin();
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin/recipe-runner-rs");
    let receipt = dir.path().join(".crates2.json");
    let message = panic_message(|| assert_receipt(&bin, &receipt, &pin));
    assert!(
        message.contains(&receipt.display().to_string()),
        "missing receipt must panic naming {}, got {message:?}",
        receipt.display()
    );
}

// ---- existing_install ----------------------------------------------

#[test]
fn existing_install_reuses_finished_install_of_the_pin() {
    let pin = real_pin();
    let dir = tempfile::tempdir().unwrap();
    let (bin, _) = install(dir.path(), &rev_key(&pin), &["recipe-runner-rs"], true);
    assert_eq!(existing_install(&dir.path().join("root"), &pin), Some(bin));
}

#[test]
fn existing_install_misses_when_binary_is_absent() {
    let pin = real_pin();
    let dir = tempfile::tempdir().unwrap();
    install(dir.path(), &rev_key(&pin), &["recipe-runner-rs"], false);
    assert_eq!(existing_install(&dir.path().join("root"), &pin), None);
}

#[test]
fn existing_install_misses_when_root_does_not_exist() {
    let pin = real_pin();
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(existing_install(&dir.path().join("absent"), &pin), None);
}

#[test]
fn existing_install_misses_when_receipt_names_another_revision() {
    let pin = real_pin();
    let dir = tempfile::tempdir().unwrap();
    install(dir.path(), &rev_key(OLD_PIN), &["recipe-runner-rs"], true);
    assert_eq!(existing_install(&dir.path().join("root"), &pin), None);
}

// ---- resolve_override ----------------------------------------------

#[test]
fn resolve_override_is_none_when_nothing_is_set() {
    assert_eq!(resolve_override(None, None, None, &real_pin()), None);
}

#[test]
fn resolve_override_accepts_qa_binary_with_default_receipt() {
    let pin = real_pin();
    let dir = tempfile::tempdir().unwrap();
    let (bin, _) = install(dir.path(), &rev_key(&pin), &["recipe-runner-rs"], true);
    assert_eq!(
        resolve_override(Some(bin.as_os_str()), None, None, &pin),
        Some(bin)
    );
}

#[test]
fn resolve_override_uses_explicit_qa_receipt() {
    let pin = real_pin();
    let dir = tempfile::tempdir().unwrap();
    // The default receipt beside the binary names the old pin; the explicit
    // receipt names the current one and must be the one checked.
    let (bin, _) = install(dir.path(), &rev_key(OLD_PIN), &["recipe-runner-rs"], true);
    let other = tempfile::tempdir().unwrap();
    let (_, receipt) = install(other.path(), &rev_key(&pin), &["recipe-runner-rs"], true);
    assert_eq!(
        resolve_override(Some(bin.as_os_str()), Some(receipt.as_os_str()), None, &pin),
        Some(bin)
    );
}

#[test]
fn resolve_override_qa_binary_takes_precedence_over_rs_path() {
    let pin = real_pin();
    let dir = tempfile::tempdir().unwrap();
    let (bin, _) = install(dir.path(), &rev_key(&pin), &["recipe-runner-rs"], true);
    let bogus = dir.path().join("does-not-exist/recipe-runner-rs");
    assert_eq!(
        resolve_override(Some(bin.as_os_str()), None, Some(bogus.as_os_str()), &pin),
        Some(bin)
    );
}

#[test]
fn resolve_override_accepts_rs_path_with_receipt_beside_it() {
    // The CI shape: scripts/install-pinned-recipe-runner.sh installs into
    // $RUNNER_TEMP/recipe-runner and exports RECIPE_RUNNER_RS_PATH.
    let pin = real_pin();
    let dir = tempfile::tempdir().unwrap();
    let (bin, _) = install(dir.path(), &rev_key(&pin), &["recipe-runner-rs"], true);
    assert_eq!(
        resolve_override(None, None, Some(bin.as_os_str()), &pin),
        Some(bin)
    );
}

#[test]
fn resolve_override_rs_path_ignores_qa_receipt() {
    let pin = real_pin();
    let dir = tempfile::tempdir().unwrap();
    let (bin, _) = install(dir.path(), &rev_key(&pin), &["recipe-runner-rs"], true);
    let other = tempfile::tempdir().unwrap();
    let (_, stale) = install(other.path(), &rev_key(OLD_PIN), &["recipe-runner-rs"], true);
    assert_eq!(
        resolve_override(None, Some(stale.as_os_str()), Some(bin.as_os_str()), &pin),
        Some(bin)
    );
}

#[test]
fn resolve_override_fails_closed_on_stale_rs_path() {
    // An override pointing at an old host install must panic loudly, never
    // fall through to the cache.
    let pin = real_pin();
    let dir = tempfile::tempdir().unwrap();
    let (bin, receipt) = install(dir.path(), &rev_key(OLD_PIN), &["recipe-runner-rs"], true);
    let message = panic_message(|| {
        resolve_override(None, None, Some(bin.as_os_str()), &pin);
    });
    let expected = format!(
        "receipt {} does not name recipe-runner-rs at #{pin}",
        receipt.display()
    );
    assert!(message.contains(&expected), "got {message:?}");
}

#[test]
fn resolve_override_fails_closed_on_stale_qa_binary() {
    let pin = real_pin();
    let dir = tempfile::tempdir().unwrap();
    let (bin, receipt) = install(dir.path(), &rev_key(OLD_PIN), &["recipe-runner-rs"], true);
    let message = panic_message(|| {
        resolve_override(Some(bin.as_os_str()), None, None, &pin);
    });
    let expected = format!(
        "receipt {} does not name recipe-runner-rs at #{pin}",
        receipt.display()
    );
    assert!(message.contains(&expected), "got {message:?}");
}

#[test]
fn resolve_override_rejects_relative_paths() {
    let pin = real_pin();
    for (qa, rs) in [
        (Some(OsStr::new("bin/recipe-runner-rs")), None),
        (None, Some(OsStr::new("recipe-runner-rs"))),
    ] {
        let message = panic_message(|| {
            resolve_override(qa, None, rs, &pin);
        });
        assert!(
            message.contains("absolute"),
            "a relative override must be refused as not absolute, got {message:?}"
        );
    }
}

#[test]
fn resolve_override_rejects_missing_or_non_file_targets() {
    let pin = real_pin();
    let dir = tempfile::tempdir().unwrap();
    let (_, _) = install(dir.path(), &rev_key(&pin), &["recipe-runner-rs"], false);
    let missing = dir.path().join("root/bin/recipe-runner-rs");
    let directory = dir.path().join("root/bin");
    for target in [&missing, &directory] {
        let message = panic_message(|| {
            resolve_override(None, None, Some(target.as_os_str()), &pin);
        });
        assert!(
            message.contains(&target.display().to_string()),
            "override {} must be refused naming it, got {message:?}",
            target.display()
        );
    }
}

// ---- resolve_once ----------------------------------------------------

#[test]
fn resolve_once_repeats_a_failure_without_running_init_again() {
    let cell = OnceLock::new();
    let calls = std::cell::Cell::new(0);
    let fail = || -> PathBuf {
        calls.set(calls.get() + 1);
        panic!("cargo install of recipe-runner-rs failed");
    };
    let first = panic_message(|| {
        resolve_once(&cell, fail);
    });
    let second = panic_message(|| {
        resolve_once(&cell, fail);
    });
    assert_eq!(
        calls.get(),
        1,
        "a failed resolution must not run cargo again"
    );
    assert!(
        first.contains("cargo install of recipe-runner-rs failed"),
        "{first:?}"
    );
    assert_eq!(first, second);
}

#[test]
fn resolve_once_returns_the_first_success() {
    let cell = OnceLock::new();
    let path = PathBuf::from("/runner/bin/recipe-runner-rs");
    assert_eq!(resolve_once(&cell, || path.clone()), path);
    assert_eq!(
        resolve_once(&cell, || panic!("init must not run twice")),
        path
    );
}

// ---- pinned_runner (builds or reuses the pinned runner) -------------

/// Exercises the real resolution: with no override this builds the pinned
/// revision on a cold cache (network or a warm ~/.cargo/git needed). Every
/// real-runner test in the same test binary shares that one resolution
/// through the OnceLock, so the build cost is paid once per binary.
#[test]
fn pinned_runner_returns_receipt_checked_binary_independent_of_host() {
    let pin = real_pin();
    let runner = pinned_runner(&repo_root());
    assert!(
        runner.is_absolute(),
        "{} must be absolute",
        runner.display()
    );
    assert!(runner.is_file(), "{} must be a file", runner.display());

    let overridden = std::env::var_os("RECIPE_RUNNER_QA_BINARY")
        .or_else(|| std::env::var_os("RECIPE_RUNNER_RS_PATH"));
    match overridden {
        Some(path) => assert_eq!(runner, PathBuf::from(path)),
        None => {
            let cache = Path::new(env!("CARGO_TARGET_TMPDIR")).join("pinned-recipe-runner");
            assert_eq!(runner, cache.join(&pin).join("bin/recipe-runner-rs"));
            assert!(
                !cache.join(format!("{pin}.partial")).exists(),
                "a finished resolution must not leave {pin}.partial behind"
            );
            assert_receipt(&runner, &cache.join(&pin).join(".crates2.json"), &pin);
            if let Some(home) = std::env::var_os("HOME") {
                assert!(
                    !runner.starts_with(Path::new(&home).join(".cargo/bin")),
                    "without an override the host install must never be used: {}",
                    runner.display()
                );
            }
        }
    }

    let version = std::process::Command::new(&runner)
        .arg("--version")
        .output()
        .expect("run pinned recipe-runner-rs --version");
    assert!(
        version.status.success(),
        "{} --version failed: {}",
        runner.display(),
        String::from_utf8_lossy(&version.stderr)
    );

    assert_eq!(
        pinned_runner(&repo_root()),
        runner,
        "repeat calls in one process must return the cached path"
    );
}
