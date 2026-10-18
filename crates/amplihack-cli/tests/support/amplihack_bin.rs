//! The `amplihack` binary, built once for the whole `cli_integration` binary.
//!
//! It lives in `bins/amplihack`, not in this crate, so
//! `CARGO_BIN_EXE_amplihack` is not defined here, and `cargo test -p
//! amplihack-cli` does not build it. A leftover binary may be stale, so the
//! first caller runs `cargo build -p amplihack` (a no-op when current) and
//! every later caller reuses the result.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// Path of a current `amplihack` binary. `AMPLIHACK_TEST_BINARY` overrides it.
pub fn amplihack_bin() -> PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        if let Some(p) = std::env::var_os("AMPLIHACK_TEST_BINARY") {
            return PathBuf::from(p);
        }
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let status = Command::new(cargo)
            .args(["build", "--quiet", "-p", "amplihack", "--bin", "amplihack"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .status()
            .expect("failed to run cargo build for the amplihack binary");
        assert!(status.success(), "cargo build -p amplihack failed");
        let bin = std::env::current_exe()
            .unwrap()
            .parent() // deps
            .and_then(Path::parent) // debug
            .unwrap()
            .join("amplihack");
        assert!(bin.is_file(), "no amplihack binary at {}", bin.display());
        bin
    })
    .clone()
}
