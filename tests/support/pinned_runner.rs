//! Shared test support: a `recipe-runner-rs` binary at the revision pinned in
//! `claude-plugin/recipe-runner.rev` (issue #2658).
//!
//! Tests that drive the real runner must never depend on what the host has
//! installed in `~/.cargo/bin` or on `PATH`. `pinned_runner` resolves, in order:
//!
//! 1. `RECIPE_RUNNER_QA_BINARY` (receipt from `RECIPE_RUNNER_QA_RECEIPT`, else
//!    `<root>/.crates2.json`, where `<root>` is the directory above `bin/`);
//! 2. `RECIPE_RUNNER_RS_PATH` (receipt at `<root>/.crates2.json`);
//! 3. a build of the pinned revision cached under
//!    `$CARGO_TARGET_TMPDIR/pinned-recipe-runner/<pin>/`, made at most once per
//!    pin under a blocking file lock shared by every test binary.
//!
//! Every source must pass the receipt check, which is a consistency check on
//! the Cargo install receipt, not proof of provenance. An override that fails
//! it panics; there is no fallback to the cache or to the host.
//!
//! Included by `#[path]` from `tests/integration/main.rs` and
//! `crates/amplihack-cli/tests/main.rs`. Its unit tests are in
//! `pinned_runner_tests.rs`, which only `tests/integration/main.rs` includes,
//! so they run in one test binary.

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
/// Resolved once per test process; later calls return the same path, or
/// repeat the first failure without running cargo again.
pub(crate) fn pinned_runner(repo_root: &Path) -> PathBuf {
    static RUNNER: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    resolve_once(&RUNNER, || resolve(repo_root))
}

/// Run `init` at most once per `cell`. A panic in `init` is stored, so later
/// callers panic with the same message instead of retrying the build
/// (`OnceLock` alone leaves the cell empty after a panic).
pub(crate) fn resolve_once(
    cell: &OnceLock<Result<PathBuf, String>>,
    init: impl FnOnce() -> PathBuf,
) -> PathBuf {
    cell.get_or_init(|| {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(init)).map_err(|payload| {
            payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                .unwrap_or_else(|| "pinned recipe-runner-rs resolution panicked".to_owned())
        })
    })
    .clone()
    .unwrap_or_else(|message| panic!("{message}"))
}

fn resolve(repo_root: &Path) -> PathBuf {
    let rev_file = repo_root.join("claude-plugin/recipe-runner.rev");
    let raw = fs::read_to_string(&rev_file)
        .unwrap_or_else(|error| panic!("read {}: {error}", rev_file.display()));
    let pin = validate_pin(raw.trim());
    let env = std::env::var_os;
    resolve_override(
        env("RECIPE_RUNNER_QA_BINARY").as_deref(),
        env("RECIPE_RUNNER_QA_RECEIPT").as_deref(),
        env("RECIPE_RUNNER_RS_PATH").as_deref(),
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
pub(crate) fn validate_pin(raw: &str) -> &str {
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
pub(crate) fn assert_receipt(bin: &Path, receipt: &Path, pin: &str) {
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
pub(crate) fn existing_install(install_root: &Path, pin: &str) -> Option<PathBuf> {
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
pub(crate) fn resolve_override(
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
    // The build directory (~280 MB) is only needed while cargo compiles; a
    // later rebuild of the same pin happens only after the install is lost.
    remove_tree(&build);
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
