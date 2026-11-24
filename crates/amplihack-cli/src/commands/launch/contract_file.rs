//! Issue #2030: the operating contract reaches Claude through
//! `--append-system-prompt-file <0600 file>`, never as argv text.
//!
//! - [`ensure_contract_file`] writes the compiled-in
//!   [`FRAGMENT`](super::system_prompt_append::FRAGMENT) to
//!   `<amplihack home>/private/system-prompt-append.md` (0600, in a 0700
//!   directory owned by the current user) unless it is already exactly right.
//!   The file is never read back as a source of content; only a bounded byte
//!   comparison decides whether to rewrite it.
//! - [`require_append_prompt_file_capability`] fails the launch unless the
//!   resolved Claude Code is at least [`MIN_APPEND_PROMPT_FILE_VERSION`]. There
//!   is no argv fallback.
//!
//! See the `# Security` section of `system_prompt_append` for the threat model.

use super::system_prompt_append::{
    OPT_OUT_ENV, installed_fragment, should_inject_system_prompt_append,
};

use anyhow::{Context, Result, bail};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// Where the contract file lives, relative to the amplihack home.
pub(crate) const CONTRACT_RELATIVE_PATH: &str = "private/system-prompt-append.md";

/// First Claude Code release with `--append-system-prompt-file` and
/// `--system-prompt-file` (both are listed by `claude --help` in 2.1.319).
pub(crate) const MIN_APPEND_PROMPT_FILE_VERSION: (u32, u32, u32) = (2, 1, 319);

/// Internal error: the gate says inject, but no contract path was supplied.
/// The command is never built with contract text instead.
pub(crate) const CONTRACT_PATH_MISSING: &str = "internal error (CONTRACT_PATH_MISSING): the operating contract must be injected but no contract file path was supplied";

/// Timeout and output cap for the one `--version` probe the gate may run.
const VERSION_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const VERSION_PROBE_MAX_BYTES: u64 = 4096;

/// Does the contract go to this launch?
///
/// [`should_inject_system_prompt_append`] (pure, unchanged) plus one launch
/// fact: a `rusty`/`rustyclawd` executable gets no contract, because its
/// support for `--append-system-prompt-file` is unconfirmed and the text form
/// is never used. This also covers `claude` resolving to a `rustyclawd` file.
pub(crate) fn contract_injection_applies(
    binary_name: &str,
    executable: &Path,
    extra_args: &[String],
    opt_out: Option<&str>,
) -> bool {
    should_inject_system_prompt_append(binary_name, extra_args, opt_out)
        && !is_rustyclawd_name(binary_name)
        && !executable
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(is_rustyclawd_name)
}

fn is_rustyclawd_name(name: &str) -> bool {
    matches!(name, "rusty" | "rustyclawd")
}

/// Choose the amplihack home that holds the contract file
/// (`private/system-prompt-append.md`) from candidate values.
///
/// Returns `amplihack_home` when it is set, non-empty and
/// `amplihack_home_is_checkout` is `false`. Otherwise returns
/// `<home>/.amplihack`, and fails when `home` is unset or empty. A checkout
/// is never chosen, so the contract file is never written into the `-w`
/// checkout of a recipe run (#2361).
///
/// Only the directory choice lives here. The ownership checks, the 0700 and
/// 0600 modes, `O_NOFOLLOW` and the newline and NUL rejection applied by the
/// writer are unchanged, and apply to the `$HOME/.amplihack` fallback as
/// well. Relative non-empty values are still accepted; see the follow-up
/// issue linked from the #2361 PR.
///
/// Pure: reads neither the environment nor the file system.
pub(crate) fn contract_home_from(
    amplihack_home: Option<&OsStr>,
    home: Option<&OsStr>,
    amplihack_home_is_checkout: bool,
) -> Result<PathBuf> {
    if !amplihack_home_is_checkout && let Some(dir) = amplihack_home.filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    let home = home.filter(|v| !v.is_empty()).with_context(|| {
        if amplihack_home_is_checkout {
            "cannot locate the amplihack home: AMPLIHACK_HOME is a Git checkout, which never \
             holds runtime state (#2361), and HOME is not set"
        } else {
            "cannot locate the amplihack home: neither AMPLIHACK_HOME nor HOME is set"
        }
    })?;
    Ok(PathBuf::from(home).join(".amplihack"))
}

/// The amplihack home that holds the contract file.
///
/// Reads `AMPLIHACK_HOME` and `HOME`, probes a non-empty `AMPLIHACK_HOME`
/// with [`amplihack_types::is_git_checkout`] and applies
/// [`contract_home_from`]: `AMPLIHACK_HOME` when it is not a Git checkout,
/// otherwise `$HOME/.amplihack`. Fails when neither yields a directory.
pub(crate) fn contract_home() -> Result<PathBuf> {
    let amplihack_home = std::env::var_os("AMPLIHACK_HOME");
    let is_checkout = amplihack_home
        .as_deref()
        .filter(|v| !v.is_empty())
        .is_some_and(|v| amplihack_types::is_git_checkout(Path::new(v)));
    contract_home_from(
        amplihack_home.as_deref(),
        std::env::var_os("HOME").as_deref(),
        is_checkout,
    )
}

/// The first `MAJOR.MINOR.PATCH` in `output`. A part that overflows `u32`
/// makes the whole answer `None` (fail closed, never wrap).
pub(crate) fn parse_claude_version(output: &str) -> Option<(u32, u32, u32)> {
    let bytes = output.as_bytes();
    let digits_at = |mut i: usize| -> (usize, usize) {
        let start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        (start, i)
    };
    let mut i = 0;
    while i < bytes.len() {
        let starts_number = bytes[i].is_ascii_digit() && (i == 0 || !bytes[i - 1].is_ascii_digit());
        if !starts_number {
            i += 1;
            continue;
        }
        let mut parts = [0u32; 3];
        let mut pos = i;
        let mut ok = true;
        for (n, part) in parts.iter_mut().enumerate() {
            if n > 0 {
                if bytes.get(pos) != Some(&b'.') {
                    ok = false;
                    break;
                }
                pos += 1;
            }
            let (start, end) = digits_at(pos);
            if start == end {
                ok = false;
                break;
            }
            match output[start..end].parse::<u32>() {
                Ok(v) => *part = v,
                // Overflow: unparseable, not a smaller version.
                Err(_) => return None,
            }
            pos = end;
        }
        if ok {
            return Some((parts[0], parts[1], parts[2]));
        }
        i = digits_at(i).1;
    }
    None
}

fn version_text((a, b, c): (u32, u32, u32)) -> String {
    format!("{a}.{b}.{c}")
}

/// Fail closed unless `version` (the `--version` output of `binary`) is at
/// least [`MIN_APPEND_PROMPT_FILE_VERSION`].
///
/// The messages name the minimum and the opt-out, and never any prompt text.
pub(crate) fn require_append_prompt_file_capability(
    binary: &Path,
    version: Option<&str>,
) -> Result<()> {
    require_file_flag_capability(
        binary,
        version,
        "amplihack passes its operating contract with --append-system-prompt-file",
    )
}

/// [`require_append_prompt_file_capability`] for a launch whose passthrough
/// rewrite produced `--system-prompt-file` or `--append-system-prompt-file`.
pub(crate) fn require_prompt_file_flag_capability(
    binary: &Path,
    version: Option<&str>,
) -> Result<()> {
    require_file_flag_capability(
        binary,
        version,
        "amplihack passes system-prompt text with --system-prompt-file or --append-system-prompt-file",
    )
}

fn require_file_flag_capability(binary: &Path, version: Option<&str>, what: &str) -> Result<()> {
    let min = version_text(MIN_APPEND_PROMPT_FILE_VERSION);
    let fix =
        format!("Upgrade Claude Code, or set {OPT_OUT_ENV}=1 to launch without the contract.");
    match version.and_then(parse_claude_version) {
        Some(found) if found >= MIN_APPEND_PROMPT_FILE_VERSION => Ok(()),
        Some(found) => bail!(
            "{what}, which needs Claude Code {min} or later; found {}.\n{fix}",
            version_text(found)
        ),
        None => bail!(
            "{what}, which needs Claude Code {min} or later; could not read a Claude Code version from '{} --version'.\n{fix}",
            binary.display()
        ),
    }
}

/// Run `<binary> --version` once: stdin from `/dev/null` (a tty read would
/// stop the probe on SIGTTIN), a 10-second timeout and at most 4 KiB of
/// output. `None` when the probe fails, times out or prints nothing usable.
pub(crate) fn probe_claude_version(binary: &Path) -> Option<String> {
    use std::io::Read;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    // Its own process group, so `kill_probe_group` also stops any grandchild
    // that inherited stdout. Once they are gone the pipe closes and the
    // waiter thread below ends; nothing is left running after a failed probe.
    let mut child = Command::new(binary)
        .process_group(0)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| {
            tracing::warn!(binary = %binary.display(), %error, "Claude Code version probe failed to start");
        })
        .ok()?;
    let stdout = child.stdout.take()?;
    let pid = child.id();
    let (tx, rx) = std::sync::mpsc::channel();
    // One thread reads stdout to end of file and then reaps the child, both
    // blocking. The caller blocks on its message with one deadline; nothing
    // here polls.
    let _waiter = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let read = stdout.take(VERSION_PROBE_MAX_BYTES).read_to_end(&mut buf);
        let status = child.wait();
        let _ = tx.send((read.map(|_| buf), status));
    });
    match rx.recv_timeout(VERSION_PROBE_TIMEOUT) {
        Ok((Ok(bytes), Ok(_))) => Some(String::from_utf8_lossy(&bytes).into_owned()),
        Ok((Err(error), _)) => {
            tracing::warn!(binary = %binary.display(), %error, "Claude Code version probe output could not be read");
            None
        }
        Ok((_, Err(error))) => {
            tracing::warn!(binary = %binary.display(), %error, "Claude Code version probe wait failed");
            None
        }
        Err(_) => {
            // The child is still running, or it exited and a grandchild
            // still holds its stdout. Killing the group closes every copy
            // of the pipe, so the thread reads end of file, reaps the child
            // and ends.
            tracing::warn!(binary = %binary.display(), "Claude Code version probe timed out");
            kill_probe_group(pid);
            None
        }
    }
}

/// Kill the probe's whole process group. The waiter thread in
/// `probe_claude_version` then sees its stdout close and reaps the child.
fn kill_probe_group(pid: u32) {
    if let Ok(pid) = libc::pid_t::try_from(pid) {
        // SAFETY: `kill` has no memory preconditions; a negative pid names
        // the process group the child leads (`process_group(0)` above).
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
}

/// Write the compiled-in contract to `<home>/private/system-prompt-append.md`
/// with mode 0600 in a 0700 directory, unless it is already exactly right.
///
/// Runs at every launch that injects the contract, never at install. See the
/// module `# Security` section for the checks.
pub(crate) fn ensure_contract_file(home: &Path) -> Result<PathBuf> {
    // SAFETY: geteuid has no preconditions and cannot fail.
    let euid = unsafe { libc::geteuid() };
    ensure_contract_file_as(home, euid)
}

/// [`ensure_contract_file`] for an explicit effective uid, so tests can
/// exercise the foreign-owner refusal without a second account.
pub(crate) fn ensure_contract_file_as(home: &Path, euid: u32) -> Result<PathBuf> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let home_text = home.as_os_str().as_encoded_bytes();
    if home_text.contains(&b'\n') || home_text.contains(&0) {
        bail!("the amplihack home path contains a newline or NUL");
    }
    let fragment = installed_fragment().context("the compiled-in operating contract is empty")?;

    let private = home.join("private");
    match std::fs::symlink_metadata(&private) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(home)
                .with_context(|| format!("cannot create the amplihack home {}", home.display()))?;
            let mut builder = std::fs::DirBuilder::new();
            std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
            match builder.create(&private) {
                Ok(()) => {}
                // A concurrent first launch created it; the checks below decide.
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("cannot create {}", private.display()));
                }
            }
        }
        Err(error) => {
            return Err(error).with_context(|| format!("cannot inspect {}", private.display()));
        }
    }
    let meta = std::fs::symlink_metadata(&private)
        .with_context(|| format!("cannot inspect {}", private.display()))?;
    if !meta.file_type().is_dir() || meta.uid() != euid {
        bail!(
            "{} is not a directory owned by you; remove it and launch again",
            private.display()
        );
    }
    if meta.permissions().mode() & 0o7777 != 0o700 {
        std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o700))
            .with_context(|| format!("cannot set mode 0700 on {}", private.display()))?;
    }

    let target = home.join(CONTRACT_RELATIVE_PATH);
    if contract_is_current(&target, fragment.as_bytes(), euid) {
        return Ok(target);
    }
    replace_contract(&private, &target, fragment.as_bytes())?;
    Ok(target)
}

/// Is `target` a regular 0600 file owned by `euid` holding exactly `expected`?
/// Reads the file only when its length already matches.
fn contract_is_current(target: &Path, expected: &[u8], euid: u32) -> bool {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

    let Ok(meta) = std::fs::symlink_metadata(target) else {
        return false;
    };
    if !meta.file_type().is_file()
        || meta.uid() != euid
        || meta.permissions().mode() & 0o7777 != 0o600
        || meta.len() != expected.len() as u64
    {
        return false;
    }
    let Ok(file) = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(target)
    else {
        return false;
    };
    let mut buf = Vec::with_capacity(expected.len());
    match file.take(expected.len() as u64 + 1).read_to_end(&mut buf) {
        Ok(_) => buf == expected,
        Err(_) => false,
    }
}

/// Write `bytes` to a fresh 0600 temporary file in `dir` and rename it over
/// `target`. The temporary file is unlinked if any step fails.
fn replace_contract(dir: &Path, target: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let suffix = random_suffix()?;
    let tmp = dir.join(format!(".system-prompt-append.{suffix}.tmp"));
    let result = (|| -> Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&tmp)
            .with_context(|| format!("cannot create {}", tmp.display()))?;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("cannot set mode 0600 on {}", tmp.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("cannot write {}", tmp.display()))?;
        file.sync_all()
            .with_context(|| format!("cannot sync {}", tmp.display()))?;
        std::fs::rename(&tmp, target)
            .with_context(|| format!("cannot move the contract file into {}", target.display()))
    })();
    if result.is_err() {
        // Best effort: the error being returned is the one that matters.
        if let Err(error) = std::fs::remove_file(&tmp)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(path = %tmp.display(), %error, "could not remove the temporary contract file");
        }
    }
    result
}

/// 64 random bits from the kernel, as hex.
fn random_suffix() -> Result<String> {
    use std::io::Read;
    let mut buf = [0u8; 8];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut buf))
        .context("cannot read /dev/urandom for a temporary file name")?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}
