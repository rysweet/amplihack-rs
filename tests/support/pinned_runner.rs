//! Shared test support: a `recipe-runner-rs` binary at the revision pinned in
//! `claude-plugin/recipe-runner.rev` (issue #2658).
//!
//! Tests that drive the real runner must never depend on what the host has
//! installed in `~/.cargo/bin` or on `PATH`. `pinned_runner` resolves, in order:
//!
//! 1. `RECIPE_RUNNER_QA_BINARY` (receipt from `RECIPE_RUNNER_QA_RECEIPT`, else
//!    `<bin>/../.crates2.json`);
//! 2. `RECIPE_RUNNER_RS_PATH` (receipt at `<bin>/../.crates2.json`);
//! 3. a build of the pinned revision cached under
//!    `$CARGO_TARGET_TMPDIR/pinned-recipe-runner/<pin>/`, made at most once per
//!    pin under a blocking file lock shared by every test binary.
//!
//! Every source must pass the receipt check, which is a consistency check on
//! the Cargo install receipt, not proof of provenance. An override that fails
//! it panics; there is no fallback to the cache or to the host.
//!
//! Included by `#[path]` from `tests/integration/main.rs` and
//! `crates/amplihack-cli/tests/main.rs`.

use std::ffi::{OsStr, OsString};
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::ErrorKind;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// Upstream repository the pin refers to. Fetched anonymously over HTTPS.
const REPO_URL: &str = "https://github.com/rysweet/amplihack-recipe-runner";
/// Source prefix Cargo writes into `.crates2.json` for a git install of it.
const RECEIPT_SOURCE: &str = "git+https://github.com/rysweet/amplihack-recipe-runner";
const CRATE: &str = "recipe-runner-rs";
const INVALID_PIN: &str = "recipe-runner.rev is not a 40-character lowercase revision";

/// Absolute path to a receipt-checked `recipe-runner-rs` at the pinned
/// revision read from `<repo_root>/claude-plugin/recipe-runner.rev`.
///
/// Resolved once per test process; later calls return the same path.
pub(crate) fn pinned_runner(repo_root: &Path) -> PathBuf {
    static RUNNER: OnceLock<PathBuf> = OnceLock::new();
    RUNNER.get_or_init(|| resolve(repo_root)).clone()
}

fn resolve(repo_root: &Path) -> PathBuf {
    let rev_file = repo_root.join("claude-plugin/recipe-runner.rev");
    let raw = fs::read_to_string(&rev_file)
        .unwrap_or_else(|error| panic!("read {}: {error}", rev_file.display()));
    let pin = validate_pin(raw.trim());
    let env = |name: &str| std::env::var_os(name);
    let qa_binary: Option<OsString> = env("RECIPE_RUNNER_QA_BINARY");
    let qa_receipt: Option<OsString> = env("RECIPE_RUNNER_QA_RECEIPT");
    let rs_path: Option<OsString> = env("RECIPE_RUNNER_RS_PATH");
    resolve_override(
        qa_binary.as_deref(),
        qa_receipt.as_deref(),
        rs_path.as_deref(),
        pin,
    )
    .unwrap_or_else(|| {
        build_or_reuse(
            &Path::new(env!("CARGO_TARGET_TMPDIR")).join("pinned-recipe-runner"),
            pin,
        )
    })
}

/// Accept exactly 40 lowercase hexadecimal characters; otherwise panic with
/// `recipe-runner.rev is not a 40-character lowercase revision`.
fn validate_pin(raw: &str) -> &str {
    let valid = raw.len() == 40 && raw.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    assert!(valid, "{INVALID_PIN}: {raw:?}");
    raw
}

/// Panic with `receipt <path> does not name recipe-runner-rs at #<pin>` unless
/// `receipt` holds an `installs` key that starts with `recipe-runner-rs `,
/// contains `(git+https://github.com/rysweet/amplihack-recipe-runner` followed
/// by `?`, `#` or `.git`, ends with `#<pin>)`, and whose `bins` include
/// `recipe-runner-rs`. A missing or malformed receipt also panics, naming the
/// receipt path.
///
/// This is a consistency check on Cargo's install receipt, not proof of
/// where the binary came from.
fn assert_receipt(bin: &Path, receipt: &Path, pin: &str) {
    let names_pin = receipt_names_pin(receipt, pin).unwrap_or_else(|error| panic!("{error}"));
    assert!(
        names_pin,
        "receipt {} does not name recipe-runner-rs at #{pin} (runner {}); \
         install that revision, or unset the override",
        receipt.display(),
        bin.display()
    );
}

/// `Ok(true)` when the receipt has a matching install entry, `Ok(false)` when
/// it parses but has none, `Err` when it cannot be read or parsed.
fn receipt_names_pin(receipt: &Path, pin: &str) -> Result<bool, String> {
    let bytes = fs::read(receipt)
        .map_err(|error| format!("read runner receipt {}: {error}", receipt.display()))?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|error| {
        format!(
            "runner receipt {} is not valid JSON: {error}",
            receipt.display()
        )
    })?;
    let installs = value["installs"]
        .as_object()
        .ok_or_else(|| format!("runner receipt {} has no installs map", receipt.display()))?;
    Ok(installs.iter().any(|(key, install)| {
        key_names_pin(key, pin)
            && install["bins"]
                .as_array()
                .is_some_and(|bins| bins.iter().any(|name| name == CRATE))
    }))
}

/// Cargo writes keys as `<crate> <version> (<source>)`.
fn key_names_pin(key: &str, pin: &str) -> bool {
    let Some(rest) = key.strip_prefix("recipe-runner-rs ") else {
        return false;
    };
    let Some((_version, source)) = rest.split_once(" (") else {
        return false;
    };
    let Some(after) = source.strip_prefix(RECEIPT_SOURCE) else {
        return false;
    };
    let after = after.strip_prefix(".git").unwrap_or(after);
    (after.starts_with('?') || after.starts_with('#')) && source.ends_with(&format!("#{pin})"))
}

/// `Some(<root>/bin/recipe-runner-rs)` when `install_root` is a finished
/// install of `pin` (receipt names the pin and the binary exists), else `None`.
/// A receipt that exists but cannot be parsed panics rather than counting as
/// a miss.
fn existing_install(install_root: &Path, pin: &str) -> Option<PathBuf> {
    let bin = install_root.join("bin").join(CRATE);
    let receipt = install_root.join(".crates2.json");
    if !bin.is_file() || !receipt.is_file() {
        return None;
    }
    let names_pin = receipt_names_pin(&receipt, pin).unwrap_or_else(|error| panic!("{error}"));
    names_pin.then_some(bin)
}

/// Resolve the two overrides from explicit values, so the order and the
/// fail-closed rule are testable without touching the process environment.
/// `None` means no override is set and the pinned cache must be used.
fn resolve_override(
    qa_binary: Option<&OsStr>,
    qa_receipt: Option<&OsStr>,
    rs_path: Option<&OsStr>,
    pin: &str,
) -> Option<PathBuf> {
    if let Some(value) = qa_binary {
        let bin = checked_override("RECIPE_RUNNER_QA_BINARY", value);
        let receipt = match qa_receipt {
            Some(receipt) => PathBuf::from(receipt),
            None => default_receipt(&bin),
        };
        assert_receipt(&bin, &receipt, pin);
        return Some(bin);
    }
    if let Some(value) = rs_path {
        let bin = checked_override("RECIPE_RUNNER_RS_PATH", value);
        assert_receipt(&bin, &default_receipt(&bin), pin);
        return Some(bin);
    }
    None
}

/// An override must be an absolute path that resolves to a regular file.
fn checked_override(name: &str, value: &OsStr) -> PathBuf {
    let bin = PathBuf::from(value);
    assert!(
        bin.is_absolute(),
        "{name}={} is not an absolute path",
        bin.display()
    );
    let is_file = fs::metadata(&bin).is_ok_and(|meta| meta.is_file());
    assert!(
        is_file,
        "{name}={} does not resolve to a regular file",
        bin.display()
    );
    bin
}

/// Cargo's receipt for `<root>/bin/<bin>` is `<root>/.crates2.json`.
fn default_receipt(bin: &Path) -> PathBuf {
    bin.parent()
        .and_then(Path::parent)
        .map(|root| root.join(".crates2.json"))
        .unwrap_or_else(|| panic!("runner {} has no install root above it", bin.display()))
}

/// Reuse `<cache>/<pin>` or build the pin into it, under a blocking lock on
/// `<cache>/<pin>.lock` shared by every test binary in the workspace.
fn build_or_reuse(cache: &Path, pin: &str) -> PathBuf {
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(cache)
        .unwrap_or_else(|error| panic!("create {}: {error}", cache.display()));
    let lock_path = cache.join(format!("{pin}.lock"));
    let lock = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .unwrap_or_else(|error| panic!("open {}: {error}", lock_path.display()));
    lock.lock()
        .unwrap_or_else(|error| panic!("lock {}: {error}", lock_path.display()));

    let root = cache.join(pin);
    if let Some(bin) = existing_install(&root, pin) {
        return bin;
    }

    let partial = cache.join(format!("{pin}.partial"));
    let build = cache.join(format!("{pin}.build"));
    remove_tree(&partial);
    eprintln!(
        "building pinned recipe-runner-rs {pin} into {}",
        cache.display()
    );
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let output = Command::new(&cargo)
        .arg("install")
        .arg("--locked")
        .arg("--git")
        .arg(REPO_URL)
        .arg("--rev")
        .arg(pin)
        .arg("--root")
        .arg(&partial)
        .arg("--target-dir")
        .arg(&build)
        .arg(CRATE)
        .env_remove("CARGO_INSTALL_ROOT")
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("CARGO_BUILD_TARGET_DIR")
        .stdin(Stdio::null())
        .output()
        .unwrap_or_else(|error| panic!("run {}: {error}", Path::new(&cargo).display()));
    assert!(
        output.status.success(),
        "cargo install of recipe-runner-rs {pin} failed ({})\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    remove_tree(&root);
    fs::rename(&partial, &root).unwrap_or_else(|error| {
        panic!(
            "rename {} to {}: {error}",
            partial.display(),
            root.display()
        )
    });
    let bin = root.join("bin").join(CRATE);
    assert!(
        bin.is_file(),
        "cargo install did not produce {}",
        bin.display()
    );
    assert_receipt(&bin, &root.join(".crates2.json"), pin);
    drop(lock);
    bin
}

/// Remove a cache entry this helper owns. Refuses to follow a symlink.
fn remove_tree(path: &Path) {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == ErrorKind::NotFound => return,
        Err(error) => panic!("inspect {}: {error}", path.display()),
    };
    assert!(
        !meta.file_type().is_symlink(),
        "refusing to remove {}: it is a symlink",
        path.display()
    );
    let removed = if meta.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    removed.unwrap_or_else(|error| panic!("remove {}: {error}", path.display()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::panic::{AssertUnwindSafe, catch_unwind};

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
    #[should_panic(expected = "recipe-runner.rev is not a 40-character lowercase revision")]
    fn validate_pin_rejects_path_traversal() {
        validate_pin("../x");
    }

    #[test]
    #[should_panic(expected = "recipe-runner.rev is not a 40-character lowercase revision")]
    fn validate_pin_rejects_short_sha() {
        validate_pin("2833a0b8");
    }

    #[test]
    #[should_panic(expected = "recipe-runner.rev is not a 40-character lowercase revision")]
    fn validate_pin_rejects_upper_case() {
        validate_pin(&real_pin().to_uppercase());
    }

    #[test]
    #[should_panic(expected = "recipe-runner.rev is not a 40-character lowercase revision")]
    fn validate_pin_rejects_leading_dash() {
        let pin = real_pin();
        validate_pin(&format!("-{}", &pin[1..]));
    }

    #[test]
    fn validate_pin_rejects_wrong_lengths_and_non_hex() {
        let pin = real_pin();
        for bad in [
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

    // ---- pinned_runner (builds or reuses the pinned runner) -------------

    /// Exercises the real resolution: with no override this builds the pinned
    /// revision on a cold cache (network or a warm ~/.cargo/git needed), the
    /// same cost the eight runner-backed tests pay through the shared OnceLock.
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
            }
        }

        if let Some(home) = std::env::var_os("HOME") {
            assert!(
                !runner.starts_with(Path::new(&home).join(".cargo/bin")),
                "the host install must never be used: {}",
                runner.display()
            );
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
}
