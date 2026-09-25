//! Locate — and on a fresh machine, bootstrap — the Rust toolchain that
//! `cargo install` of `recipe-runner-rs` needs.
//!
//! `amplihack install` must work out of the box on a clean Linux/macOS host
//! (for example a fresh Ubuntu VM reached via `npx ... -- amplihack install`),
//! where neither `cargo` nor a C linker is present. With bootstrapping
//! allowed, this module:
//!
//! 1. Finds `cargo` on PATH or in `$CARGO_HOME/bin` (default `~/.cargo/bin`).
//! 2. Otherwise installs a minimal user-local toolchain with the official
//!    rustup installer (`rustup-init.sh`, `-y --no-modify-path --profile
//!    minimal`). Nothing outside `$CARGO_HOME`/`$RUSTUP_HOME` is touched and
//!    shell profiles are left alone — amplihack finds `~/.cargo/bin` itself.
//! 3. Finds a C compiler (`cc`, `gcc` or `clang`), which rustc uses as the
//!    linker. When none exists and `apt-get` is available, it installs
//!    `build-essential` as root or through *passwordless* `sudo -n`. It never
//!    prompts for a password; if sudo needs one, it fails with the exact
//!    command to run.
//!
//! Set `AMPLIHACK_NO_RUST_BOOTSTRAP=1` to disable steps 2 and 3.

use crate::util::run_with_timeout;
use anyhow::{Context, Result, bail};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

pub(crate) const NO_BOOTSTRAP_ENV: &str = "AMPLIHACK_NO_RUST_BOOTSTRAP";
const RUSTUP_INIT_URL: &str = "https://static.rust-lang.org/rustup/rustup-init.sh";
const BOOTSTRAP_TIMEOUT: Duration = Duration::from_secs(900);
const APT_UPDATE_DEADLINE: Duration = Duration::from_secs(300);
const APT_UPDATE_RETRY_DELAY: Duration = Duration::from_secs(5);
const C_COMPILERS: [&str; 3] = ["cc", "gcc", "clang"];
const LINKER_REMEDIATION: &str = "sudo apt-get install -y build-essential   \
     (Fedora: sudo dnf install -y gcc; macOS: xcode-select --install)";

/// `AMPLIHACK_NO_RUST_BOOTSTRAP` semantics: unset, empty and `0` leave
/// bootstrapping on; any other value turns it off.
fn bootstrap_disabled_by(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_some_and(|value| !value.is_empty() && value != "0")
}

fn bootstrap_disabled() -> bool {
    bootstrap_disabled_by(std::env::var_os(NO_BOOTSTRAP_ENV).as_deref())
}

/// `$CARGO_HOME`, defaulting to `~/.cargo`.
pub(crate) fn cargo_home() -> Option<PathBuf> {
    std::env::var_os("CARGO_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|value| !value.is_empty())
                .map(|home| PathBuf::from(home).join(".cargo"))
        })
}

fn find_in_dirs(dirs: &[PathBuf], names: &[&str]) -> Option<PathBuf> {
    dirs.iter().find_map(|dir| {
        names
            .iter()
            .map(|name| dir.join(name))
            .find(|candidate| candidate.is_file())
    })
}

/// `cargo` from `path_dirs`, else from `<cargo_home>/bin`.
pub(crate) fn find_cargo(path_dirs: &[PathBuf], cargo_home: Option<&Path>) -> Option<PathBuf> {
    find_in_dirs(path_dirs, &["cargo"]).or_else(|| {
        cargo_home
            .map(|home| home.join("bin").join("cargo"))
            .filter(|candidate| candidate.is_file())
    })
}

/// A C compiler rustc can use as its linker.
pub(crate) fn find_c_compiler(path_dirs: &[PathBuf]) -> Option<PathBuf> {
    find_in_dirs(path_dirs, &C_COMPILERS)
}

fn path_dirs() -> Vec<PathBuf> {
    amplihack_utils::launch_target::env_path_dirs()
}

/// The PATH to give a `cargo` child: `None` (inherit unchanged) when cargo's
/// directory is already among `entries`, otherwise `entries` with that
/// directory prepended — the `$CARGO_HOME/bin` fallback or a toolchain rustup
/// just installed, where cargo needs its own dir on PATH to find `rustc`.
fn path_for_cargo(cargo: &Path, entries: Vec<PathBuf>) -> Option<OsString> {
    let bin_dir = cargo.parent()?.to_path_buf();
    if entries.contains(&bin_dir) {
        return None;
    }
    std::env::join_paths(std::iter::once(bin_dir).chain(entries)).ok()
}

/// [`path_for_cargo`] against the process `$PATH`.
pub(crate) fn path_with_cargo_bin(cargo: &Path) -> Option<OsString> {
    // Rebuilding `$PATH` for a child, not choosing a file to run: keep the
    // user's entries verbatim (relative ones included) and only prepend.
    path_for_cargo(cargo, amplihack_utils::launch_target::env_path_entries())
}

/// Return a usable `cargo`, installing rustup first when `allow_bootstrap`
/// is set and no toolchain is found.
pub(crate) fn ensure_cargo(allow_bootstrap: bool) -> Result<PathBuf> {
    ensure_cargo_in(
        &path_dirs(),
        cargo_home(),
        allow_bootstrap && !bootstrap_disabled(),
    )
}

fn ensure_cargo_in(
    path_dirs: &[PathBuf],
    home: Option<PathBuf>,
    allow_bootstrap: bool,
) -> Result<PathBuf> {
    if let Some(cargo) = find_cargo(path_dirs, home.as_deref()) {
        return Ok(cargo);
    }
    if !allow_bootstrap {
        bail!("cargo is required to install recipe-runner-rs. Install Rust: https://rustup.rs/");
    }
    if !cfg!(unix) {
        bail!(
            "cargo is required to install recipe-runner-rs and cannot be installed automatically \
             on this platform. Install Rust: https://rustup.rs/"
        );
    }
    let home = home.context("cannot locate a home directory to install Rust into")?;

    println!("   ⏬ cargo not found — installing a minimal Rust toolchain with rustup");
    install_rustup()?;

    let cargo = home.join("bin").join("cargo");
    if !cargo.is_file() {
        bail!(
            "rustup finished but {} does not exist. Install Rust manually: https://rustup.rs/",
            cargo.display()
        );
    }
    println!("   ✅ Installed Rust toolchain ({})", cargo.display());
    Ok(cargo)
}

fn install_rustup() -> Result<()> {
    let dirs = &path_dirs()[..];
    let temp = tempfile::tempdir().context("failed to create a temp dir for rustup-init")?;
    let script = temp.path().join("rustup-init.sh");

    let fetch = if let Some(curl) = find_in_dirs(dirs, &["curl"]) {
        let mut cmd = Command::new(curl);
        cmd.args(["--proto", "=https", "--tlsv1.2", "-sSfL", "-o"])
            .arg(&script)
            .arg(RUSTUP_INIT_URL);
        cmd
    } else if let Some(wget) = find_in_dirs(dirs, &["wget"]) {
        let mut cmd = Command::new(wget);
        cmd.args(["--https-only", "-q", "-O"])
            .arg(&script)
            .arg(RUSTUP_INIT_URL);
        cmd
    } else {
        bail!(
            "installing Rust needs curl or wget, and neither is on PATH. \
             Install one (e.g. sudo apt-get install -y curl) or install Rust manually: \
             https://rustup.rs/"
        );
    };
    let status = run_with_timeout(fetch, BOOTSTRAP_TIMEOUT)
        .context("failed to download the rustup installer")?;
    if !status.success() {
        bail!("downloading {RUSTUP_INIT_URL} failed with status {status}");
    }

    let mut sh = Command::new("sh");
    sh.arg(&script)
        .args(["-y", "--no-modify-path", "--profile", "minimal"]);
    let status = run_with_timeout(sh, BOOTSTRAP_TIMEOUT).context("failed to run rustup-init")?;
    if !status.success() {
        bail!("rustup-init exited with status {status}");
    }
    Ok(())
}

/// Make sure a C compiler/linker exists, installing `build-essential` via
/// apt when that is possible without a password prompt.
pub(crate) fn ensure_c_linker(allow_bootstrap: bool) -> Result<()> {
    ensure_c_linker_in(&path_dirs(), allow_bootstrap && !bootstrap_disabled())
}

fn ensure_c_linker_in(dirs: &[PathBuf], allow_bootstrap: bool) -> Result<()> {
    if !cfg!(unix) || find_c_compiler(dirs).is_some() {
        return Ok(());
    }
    let missing = "no C compiler (cc/gcc/clang) found; Rust needs one as its linker to build \
                   recipe-runner-rs";
    if !allow_bootstrap {
        bail!("{missing}. Install one with: {LINKER_REMEDIATION}");
    }
    let Some(apt_get) = find_in_dirs(dirs, &["apt-get"]) else {
        bail!("{missing}. Install one with: {LINKER_REMEDIATION}");
    };
    let sudo = if is_root() {
        None
    } else {
        let Some(sudo) = find_in_dirs(dirs, &["sudo"]) else {
            bail!("{missing}, and sudo is unavailable. Install one with: {LINKER_REMEDIATION}");
        };
        let passwordless = Command::new(&sudo)
            .args(["-n", "true"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        if !passwordless {
            bail!(
                "{missing}, and sudo needs a password so amplihack will not install it for you. \
                 Run this, then re-run `amplihack install`:\n    {LINKER_REMEDIATION}"
            );
        }
        Some(sudo)
    };

    println!("   ⏬ No C linker found — installing build-essential with apt-get");
    let apt = |args: &[&str], stderr: Option<std::fs::File>, timeout: Duration| {
        let mut cmd = match &sudo {
            Some(sudo) => {
                let mut cmd = Command::new(sudo);
                cmd.args(["-n", "env", "DEBIAN_FRONTEND=noninteractive", "LC_ALL=C"])
                    .arg(&apt_get);
                cmd
            }
            None => {
                let mut cmd = Command::new(&apt_get);
                cmd.env("DEBIAN_FRONTEND", "noninteractive")
                    .env("LC_ALL", "C");
                cmd
            }
        };
        // A freshly booted VM usually has apt-daily/unattended-upgrades
        // holding the dpkg lock; wait for it (apt >= 1.9.11).
        cmd.args(["-o", "DPkg::Lock::Timeout=300"]).args(args);
        if let Some(stderr) = stderr {
            cmd.stderr(stderr);
        }
        run_with_timeout(cmd, timeout)
            .with_context(|| format!("failed to run apt-get {}", args.join(" ")))
    };

    // DPkg::Lock::Timeout does not cover /var/lib/apt/lists/lock, which
    // apt-daily's boot-time `update` holds. Retry `update` only while that
    // lock is the failure, and for at most APT_UPDATE_DEADLINE overall. Any
    // other update failure (e.g. a broken third-party repo) is not waited on:
    // the install is tried with the lists already on disk.
    let deadline = std::time::Instant::now() + APT_UPDATE_DEADLINE;
    // LC_ALL=C (set above) keeps apt's "Could not get lock" untranslated.
    let mut announced_wait = false;
    loop {
        let log = tempfile::tempfile().context("failed to create apt-get stderr log")?;
        // A timed-out or unrunnable update is not fatal either: fall through.
        let status = match apt(
            &["update", "-qq"],
            Some(log.try_clone()?),
            BOOTSTRAP_TIMEOUT,
        ) {
            Ok(status) => status,
            Err(err) => {
                println!("   ⚠️  apt-get update failed ({err:#}); trying the install anyway");
                break;
            }
        };
        let mut stderr = String::new();
        {
            use std::io::{Read, Seek};
            let mut log = log;
            log.rewind()?;
            let _ = log.read_to_string(&mut stderr);
        }
        if status.success() {
            break;
        }
        let lock_held = stderr.contains("Could not get lock");
        if !lock_held || std::time::Instant::now() + APT_UPDATE_RETRY_DELAY >= deadline {
            eprint!("{stderr}");
            println!("   ⚠️  apt-get update failed; trying the install with existing lists");
            break;
        }
        if !announced_wait {
            println!("   ⏳ apt is busy (package lists locked); waiting up to 5 minutes");
            announced_wait = true;
        }
        std::thread::sleep(APT_UPDATE_RETRY_DELAY);
    }
    let status = apt(
        &["install", "-y", "-qq", "build-essential"],
        None,
        BOOTSTRAP_TIMEOUT,
    )?;
    if !status.success() {
        bail!(
            "apt-get install build-essential exited with status {status}. Install a C \
             compiler manually: {LINKER_REMEDIATION}"
        );
    }
    if find_c_compiler(&path_dirs()).is_none() {
        bail!("{missing} even after installing build-essential. {LINKER_REMEDIATION}");
    }
    println!("   ✅ Installed build-essential");
    Ok(())
}

#[cfg(unix)]
fn is_root() -> bool {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() == 0 }
}

#[cfg(not(unix))]
fn is_root() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "").unwrap();
    }

    #[test]
    fn find_cargo_prefers_path_then_cargo_home() {
        let temp = tempfile::tempdir().unwrap();
        let path_dir = temp.path().join("path");
        let home = temp.path().join("cargo-home");
        touch(&home.join("bin/cargo"));

        assert_eq!(
            find_cargo(std::slice::from_ref(&path_dir), Some(&home)),
            Some(home.join("bin/cargo")),
            "fresh rustup installs are found without ~/.cargo/bin on PATH"
        );

        touch(&path_dir.join("cargo"));
        assert_eq!(
            find_cargo(std::slice::from_ref(&path_dir), Some(&home)),
            Some(path_dir.join("cargo"))
        );
    }

    #[test]
    fn find_cargo_none_when_absent() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(
            find_cargo(&[temp.path().to_path_buf()], Some(&temp.path().join("x"))),
            None
        );
    }

    #[test]
    fn find_c_compiler_accepts_cc_gcc_or_clang() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().to_path_buf();
        assert_eq!(find_c_compiler(std::slice::from_ref(&dir)), None);
        touch(&dir.join("clang"));
        assert_eq!(
            find_c_compiler(std::slice::from_ref(&dir)),
            Some(dir.join("clang"))
        );
        touch(&dir.join("cc"));
        assert_eq!(
            find_c_compiler(std::slice::from_ref(&dir)),
            Some(dir.join("cc"))
        );
    }

    #[test]
    fn path_for_cargo_leaves_path_alone_when_cargo_dir_is_on_it() {
        let entries = vec![
            PathBuf::from("/home/u/.local/bin"),
            PathBuf::from("/usr/bin"),
        ];
        assert_eq!(
            path_for_cargo(Path::new("/usr/bin/cargo"), entries),
            None,
            "distro cargo must not reorder the user's PATH"
        );
    }

    #[cfg(unix)]
    #[test]
    fn path_for_cargo_prepends_off_path_cargo_dir() {
        let entries = vec![PathBuf::from("/usr/bin"), PathBuf::from("")];
        let joined = path_for_cargo(Path::new("/h/.cargo/bin/cargo"), entries).unwrap();
        assert_eq!(
            joined,
            OsString::from("/h/.cargo/bin:/usr/bin:"),
            "only prepends; the user's own entries stay verbatim"
        );
    }

    #[test]
    fn opt_out_env_semantics() {
        use std::ffi::OsStr;
        assert!(!bootstrap_disabled_by(None));
        assert!(!bootstrap_disabled_by(Some(OsStr::new(""))));
        assert!(!bootstrap_disabled_by(Some(OsStr::new("0"))));
        assert!(bootstrap_disabled_by(Some(OsStr::new("1"))));
        assert!(bootstrap_disabled_by(Some(OsStr::new("yes"))));
    }

    /// The launch-time refresh passes `allow_bootstrap = false`: it must fail
    /// without downloading or installing anything.
    #[test]
    fn no_bootstrap_means_no_side_effects() {
        let temp = tempfile::tempdir().unwrap();
        let empty_path = vec![temp.path().join("empty-bin")];
        let home = temp.path().join("cargo-home");

        let err = ensure_cargo_in(&empty_path, Some(home.clone()), false)
            .expect_err("no cargo and no bootstrap must fail");
        assert!(format!("{err:#}").contains("cargo is required"));
        assert!(!home.exists(), "nothing may be installed into CARGO_HOME");
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
    }

    /// Unix only: elsewhere `ensure_c_linker_in` never needs a C compiler.
    #[cfg(unix)]
    #[test]
    fn no_bootstrap_means_no_linker_install() {
        let temp = tempfile::tempdir().unwrap();
        let empty_path = vec![temp.path().join("empty-bin")];
        let err = ensure_c_linker_in(&empty_path, false)
            .expect_err("no C compiler and no bootstrap must fail");
        assert!(format!("{err:#}").contains("build-essential"));
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
    }
}
