//! TDD test for issue #341: `amplihack install` must stage the
//! `amplifier-bundle/` from the local checkout the user is installing from,
//! not from a stale/baked source (e.g. the workspace `CARGO_MANIFEST_DIR`).
//!
//! Scenario this guards against:
//!  - User clones amplihack-rs at HEAD where `recipes/smart-orchestrator.yaml`
//!    contains 0 references to the legacy Python entrypoint
//!    (`python3 -m amplihack.runtime_assets`).
//!  - User runs `amplihack install` from inside that checkout.
//!  - Expected: `~/.amplihack/amplifier-bundle/recipes/smart-orchestrator.yaml`
//!    also contains 0 such references (i.e. is a byte-faithful copy of the
//!    checkout's source file).
//!
//! ## Process-level invariants
//!
//! This test mutates `HOME`, `CWD`, `AMPLIHACK_HOME`, and
//! `AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH`. Each integration test binary is a
//! separate process, but Rust's default test harness runs `#[test]` fns on
//! multiple threads within that process. We therefore (a) only define a single
//! test in this file, and (b) serialize via a `OnceLock<Mutex<()>>` for
//! defensive consistency with sibling test files.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use amplihack_cli::commands::install::run_install;

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Walk up from `CARGO_MANIFEST_DIR` (the amplihack-cli crate dir) to the
/// workspace root that contains the real `amplifier-bundle/`.
fn workspace_root() -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut cur: &Path = &crate_dir;
    loop {
        if cur.join("amplifier-bundle").is_dir() && cur.join("Cargo.toml").is_file() {
            return cur.to_path_buf();
        }
        cur = cur
            .parent()
            .expect("walked above filesystem root looking for workspace");
    }
}

fn copy_dir_all(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let ty = entry.file_type().unwrap();
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&from, &to);
        } else if ty.is_symlink() {
            // Resolve symlinks to their target contents (good enough for asset
            // copying in tests).
            if let Ok(target_meta) = fs::metadata(&from)
                && target_meta.is_dir()
            {
                copy_dir_all(&from, &to);
                continue;
            }
            let _ = fs::copy(&from, &to);
        } else {
            fs::copy(&from, &to).unwrap();
        }
    }
}

/// Writes a minimal executable stub at `dir/amplihack-hooks` (mode 0700) and
/// returns its path. Content is padded > 1024 bytes to satisfy
/// `deploy_binaries`' sanity-check.
fn write_stub_hooks_binary(dir: &Path) -> PathBuf {
    let path = dir.join("amplihack-hooks");
    let content = format!("#!/bin/sh\nexit 0\n{}\n", "x".repeat(1100));
    fs::write(&path, content).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    path
}

const STALE_MARKER: &str = "python3 -m amplihack.runtime_assets";
/// Sentinel string injected into the fake checkout's smart-orchestrator.yaml.
/// Its presence in the staged file proves the install used the CWD checkout
/// rather than the workspace `CARGO_MANIFEST_DIR` bundle.
const CHECKOUT_SENTINEL: &str = "# bugfix-341-checkout-sentinel-7f3e9a1b\n";

#[test]
fn install_stages_amplifier_bundle_from_local_checkout_not_stale_source() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    // ----- Set up a fake "user checkout" tempdir containing a real bundle copy
    let checkout = tempfile::tempdir().expect("create checkout tempdir");
    let workspace = workspace_root();
    copy_dir_all(
        &workspace.join("amplifier-bundle"),
        &checkout.path().join("amplifier-bundle"),
    );
    // Fabricate a minimal `.claude/` skeleton with the ESSENTIAL_DIRS the
    // installer expects. The workspace itself doesn't contain these (its
    // assets live under `amplifier-bundle/`), so we create empty stubs to
    // get past the install's "no directories copied" failure mode. The
    // amplifier-bundle staging — which is what #341 is about — runs
    // independently of these.
    let claude = checkout.path().join(".claude");
    for dir in [
        "agents/amplihack",
        "commands/amplihack",
        "tools/amplihack",
        "tools/xpia",
        "context",
        "workflow",
        "skills",
        "templates",
        "scenarios",
        "docs",
        "schemas",
        "config",
    ] {
        fs::create_dir_all(claude.join(dir)).unwrap();
        // Drop a placeholder file so the dir is non-empty (some copy code
        // skips empty dirs).
        fs::write(claude.join(dir).join(".keep"), "").unwrap();
    }
    fs::write(claude.join("tools/statusline.sh"), "#!/bin/sh\necho hi\n").unwrap();
    fs::write(claude.join("AMPLIHACK.md"), "framework\n").unwrap();
    fs::write(claude.join("settings.json"), "{}\n").unwrap();
    fs::write(checkout.path().join("CLAUDE.md"), "root\n").unwrap();

    // Inject sentinel into the checkout's smart-orchestrator.yaml so we can
    // prove which source the install consumed.
    let checkout_yaml = checkout
        .path()
        .join("amplifier-bundle/recipes/smart-orchestrator.yaml");
    let original = fs::read_to_string(&checkout_yaml).expect("read checkout yaml");
    fs::write(&checkout_yaml, format!("{original}\n{CHECKOUT_SENTINEL}")).unwrap();

    // Preconditions: the checkout source must have 0 stale-marker lines.
    let pre_count = original
        .lines()
        .filter(|l| l.contains(STALE_MARKER))
        .count();
    assert_eq!(
        pre_count, 0,
        "precondition: checkout source must have 0 stale '{STALE_MARKER}' refs; \
         found {pre_count}. Update the workspace amplifier-bundle source."
    );

    // ----- Set HOME to a tempdir so install stages into an isolated location
    let home = tempfile::tempdir().expect("create home tempdir");
    let bin_dir = home.path().join("stub_bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let hooks_stub = write_stub_hooks_binary(&bin_dir);

    let prev_home = std::env::var_os("HOME");
    let prev_amplihack_home = std::env::var_os("AMPLIHACK_HOME");
    let prev_hooks = std::env::var_os("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH");
    let prev_cwd = std::env::current_dir().ok();

    // SAFETY: env mutation is gated by `env_lock()` above. Each integration
    // test binary is its own process; this file defines a single test.
    unsafe {
        std::env::set_var("HOME", home.path());
        std::env::remove_var("AMPLIHACK_HOME");
        std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", &hooks_stub);
    }
    std::env::set_current_dir(checkout.path()).expect("chdir into checkout");

    // ----- Act: install from the (CWD) checkout
    let install_result = run_install(None, false);

    // ----- Capture assertions before restoring env so a panic on restore
    //       doesn't mask the real failure.
    let staged_yaml = home
        .path()
        .join(".amplihack/amplifier-bundle/recipes/smart-orchestrator.yaml");
    let staged_exists = staged_yaml.is_file();
    let staged_contents = if staged_exists {
        Some(fs::read_to_string(&staged_yaml).expect("read staged yaml"))
    } else {
        None
    };

    // ----- Restore env
    if let Some(d) = prev_cwd {
        let _ = std::env::set_current_dir(d);
    }
    unsafe {
        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        match prev_amplihack_home {
            Some(v) => std::env::set_var("AMPLIHACK_HOME", v),
            None => std::env::remove_var("AMPLIHACK_HOME"),
        }
        match prev_hooks {
            Some(v) => std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", v),
            None => std::env::remove_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH"),
        }
    }

    // ----- Now check the captured state
    install_result.expect("run_install must succeed");
    assert!(
        staged_exists,
        "smart-orchestrator.yaml must be staged at ~/.amplihack/amplifier-bundle/recipes/"
    );
    let staged = staged_contents.unwrap();

    // R2 / #341: the staged file must contain 0 lines with the legacy Python
    // entrypoint, mirroring `grep -c "python3 -m amplihack.runtime_assets"`.
    let stale_count = staged.lines().filter(|l| l.contains(STALE_MARKER)).count();
    assert_eq!(
        stale_count, 0,
        "issue #341 regression: staged smart-orchestrator.yaml contains \
         {stale_count} stale '{STALE_MARKER}' line(s); install must stage \
         from the local checkout's source (which has 0)."
    );

    // Provenance check: the staged file must contain the sentinel we wrote
    // into the CWD checkout. If this fails, install consumed a different
    // bundle source (e.g. CARGO_MANIFEST_DIR workspace) and #341 is not
    // actually fixed even if the stale-count happens to be 0.
    assert!(
        staged.contains(CHECKOUT_SENTINEL.trim_end()),
        "staged smart-orchestrator.yaml does not contain the CWD-checkout \
         sentinel — install consumed a different bundle source than the \
         local checkout (issue #341 not fixed)."
    );
}
