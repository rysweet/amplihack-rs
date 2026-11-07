pub(crate) use super::archive::extract_archive;
pub(super) use super::archive::{binary_filename, find_binary};
use super::checksum::verify_sha256;
use super::*;
use std::process::Command;
use std::time::Duration;

pub(super) fn download_and_replace(release: &UpdateRelease) -> Result<PathBuf> {
    let archive_bytes = super::network::http_get_with_retry(&release.asset_url)?;

    if let Some(checksum_url) = &release.checksum_url {
        verify_sha256(&archive_bytes, checksum_url)
            .context("binary download checksum verification failed")?;
        println!("SHA-256 checksum verified.");
    } else {
        tracing::warn!(
            "no checksum file found for release {}; skipping SHA-256 verification",
            release.version
        );
    }
    let temp_dir = tempfile::tempdir().context("failed to create update temp directory")?;
    extract_archive(&archive_bytes, temp_dir.path())?;

    // Issue #1954 D6: the binaries change only while this process holds the
    // install lock, so no install or self-heal stages at the same time. The
    // guard drops when this function returns, before `run_update` starts the
    // post-update install child, which takes the lock itself.
    let _install_lock = crate::install_lock::acquire()?;

    let new_amplihack = find_binary(temp_dir.path(), binary_filename("amplihack"))?;
    let new_hooks = find_binary(temp_dir.path(), binary_filename("amplihack-hooks"))?;
    let current_exe =
        std::env::current_exe().context("cannot determine current executable path")?;
    let decision = current_process_install_target_decision(&current_exe)?;
    let plan = plan_downloaded_binary_install(
        InstallArchiveLayout {
            amplihack: new_amplihack.clone(),
            hooks: new_hooks.clone(),
        },
        decision,
    )?;
    let install_dir = plan
        .amplihack_destination
        .parent()
        .context("selected amplihack destination has no parent directory")?;
    fs::create_dir_all(install_dir).with_context(|| {
        format!(
            "failed to create update install dir {}",
            install_dir.display()
        )
    })?;
    let hooks_dest = plan.hooks_destination.clone();
    let amplihack_dest = plan.amplihack_destination.clone();

    // Issue #1954: a user-level install is a set of links into
    // ~/.amplihack/releases. The new binaries go into a release of their
    // own and the links switch to it; the installed files are never rewritten
    // under a running process.
    if let Some(home) = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|home| !home.as_os_str().is_empty())
        && install_dir == crate::path_conflicts::preferred_user_bin(&home)
    {
        install_into_release(
            &home,
            release,
            &new_amplihack,
            &new_hooks,
            &amplihack_dest,
            &hooks_dest,
        )?;
        invalidate_update_cache();
        return Ok(amplihack_dest);
    }

    // Preflight: verify the destination filesystem has enough space for the
    // new binaries. Without this, a low-disk host can crash `fs::copy` with a
    // partial write that leaves a corrupt `.tmp` file behind — the user sees
    // "Updated amplihack" but the rename actually failed (or wrote a
    // truncated binary), and every subsequent launch re-prompts for update.
    let new_hooks_size = fs::metadata(&new_hooks)
        .with_context(|| format!("stat {}", new_hooks.display()))?
        .len();
    let new_amplihack_size = fs::metadata(&new_amplihack)
        .with_context(|| format!("stat {}", new_amplihack.display()))?
        .len();
    let existing_hooks_size = fs::metadata(&hooks_dest).map(|m| m.len()).unwrap_or(0);
    let existing_amplihack_size = fs::metadata(&amplihack_dest).map(|m| m.len()).unwrap_or(0);
    // Worst case: both destinations keep their current contents while the
    // `.tmp` files are written, so we need headroom for every new_* byte
    // plus a safety margin. `saturating_sub` keeps us honest if a future
    // build shrinks the binaries below the existing sizes.
    let required_free = new_hooks_size.saturating_sub(existing_hooks_size)
        + new_amplihack_size.saturating_sub(existing_amplihack_size)
        + 8 * 1024 * 1024;
    check_free_space(install_dir, required_free)?;

    install_binary_atomic(&new_hooks, &hooks_dest)?;
    install_binary_atomic(&new_amplihack, &amplihack_dest)?;

    report_installed_versions(release, &amplihack_dest, &hooks_dest);
    invalidate_update_cache();

    Ok(amplihack_dest)
}

/// Issue #1954: copies the downloaded binaries into a new release, checks
/// what the copies report, then switches `amplihack-hooks` and `amplihack`
/// to them. `amplihack-asset-resolver` and `amplifier-bundle` stay on their
/// earlier release until the post-update install child switches them.
fn install_into_release(
    home: &Path,
    release: &UpdateRelease,
    new_amplihack: &Path,
    new_hooks: &Path,
    amplihack_link: &Path,
    hooks_link: &Path,
) -> Result<()> {
    use crate::commands::install::release_layout::{self, ReleaseTxn};
    let amplihack_dir = home.join(".amplihack");
    let releases = release_layout::releases_dir(&amplihack_dir);
    let full_size = fs::metadata(new_amplihack)
        .with_context(|| format!("stat {}", new_amplihack.display()))?
        .len()
        + fs::metadata(new_hooks)
            .with_context(|| format!("stat {}", new_hooks.display()))?
            .len();
    let mut txn = ReleaseTxn::allocate(&amplihack_dir, &release.version)
        .with_context(|| format!("cannot create a release for v{}", release.version))?;
    // Measured on the release, which now exists, for both binaries in full:
    // nothing already installed is replaced in place.
    check_free_space(&releases, full_size + 8 * 1024 * 1024)?;

    let bin_dir = txn.bin_dir();
    fs::create_dir_all(&bin_dir)
        .with_context(|| format!("failed to create {}", bin_dir.display()))?;
    let staged_hooks = bin_dir.join(binary_filename("amplihack-hooks"));
    let staged_amplihack = bin_dir.join(binary_filename("amplihack"));
    install_binary_atomic(new_hooks, &staged_hooks)?;
    install_binary_atomic(new_amplihack, &staged_amplihack)?;

    // Checked before the switch (S24), on the copies the links will name.
    report_installed_versions(release, &staged_amplihack, &staged_hooks);

    txn.plan_link(hooks_link, &staged_hooks)?;
    txn.plan_link(amplihack_link, &staged_amplihack)?;
    let root = txn.root().to_path_buf();
    let report = txn.commit()?;
    if let Some((link, reason)) = &report.failed {
        bail!(
            "release {} is complete but switching {} to it failed: {reason}. Run \
             `amplihack update` again to finish",
            root.display(),
            link.display()
        );
    }
    for switch in &report.switched {
        println!(
            "Switched {} -> {}",
            switch.link.display(),
            switch.target.display()
        );
    }
    Ok(())
}

/// Execs both new binaries with `--version` and reports the outcome.
///
/// Catches the "release asset was built from an un-bumped Cargo.toml"
/// failure mode where the update claims success but the new binary still
/// self-reports the old version — which then retriggers the update prompt
/// forever. A mismatch warns rather than fails, as it always has: the user
/// can re-run or ignore. The hooks binary is checked too, because a version
/// skew between amplihack and amplihack-hooks can silently break hooks.
fn report_installed_versions(release: &UpdateRelease, amplihack: &Path, hooks: &Path) {
    let amplihack_ok = verify_installed_version(amplihack, &release.version);
    let hooks_ok = verify_installed_version(hooks, &release.version);

    match (&amplihack_ok, &hooks_ok) {
        (Ok(()), Ok(())) => {
            println!(
                "Updated amplihack: {} -> {}",
                CURRENT_VERSION, release.version
            );
            println!("Restart amplihack to use the new version.");
        }
        _ => {
            if let Err(err) = &amplihack_ok {
                eprintln!(
                    "⚠️  amplihack was written to {} but reports an unexpected version: {err}",
                    amplihack.display()
                );
            }
            if let Err(err) = &hooks_ok {
                eprintln!(
                    "⚠️  amplihack-hooks was written to {} but reports an unexpected version: {err}",
                    hooks.display()
                );
            }
            eprintln!(
                "   Expected v{} in both binaries. If the next launch still offers an update, the release asset may have been built without a version bump.",
                release.version
            );
            eprintln!(
                "   To stop the update loop, set AMPLIHACK_NO_UPDATE_CHECK=1 and report the mismatch."
            );
        }
    }
}

/// Invalidates the startup update cache whatever the outcome — its entry
/// was written by the previous (pre-swap) binary and may lie about what
/// version is actually installed now. Leaving it in place causes the new
/// binary to trust the old cache for up to 24h.
fn invalidate_update_cache() {
    match super::cache_path() {
        Ok(path) => match fs::remove_file(&path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => tracing::warn!(
                "failed to remove stale update cache {}: {err}",
                path.display()
            ),
        },
        Err(err) => tracing::warn!("failed to resolve update cache path for invalidation: {err}"),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct InstallArchiveLayout {
    pub(super) amplihack: PathBuf,
    pub(super) hooks: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BinaryInstallPlan {
    pub(super) amplihack_destination: PathBuf,
    pub(super) hooks_destination: PathBuf,
}

pub(super) fn plan_downloaded_binary_install(
    archive: InstallArchiveLayout,
    decision: crate::path_conflicts::InstallTargetDecision,
) -> Result<BinaryInstallPlan> {
    let install_dir = match decision {
        crate::path_conflicts::InstallTargetDecision::CurrentExeDir {
            install_dir,
            reason,
        }
        | crate::path_conflicts::InstallTargetDecision::PreferredUserBin {
            install_dir,
            reason,
        } => {
            tracing::debug!(%reason, install_dir = %install_dir.display(), "selected update install target");
            install_dir
        }
        crate::path_conflicts::InstallTargetDecision::ManualRepairRequired {
            conflicts, ..
        } => {
            bail!(
                "manual repair required before amplihack update can replace binaries:\n{}",
                update_manual_repair_guidance(&conflicts)
            );
        }
    };

    fs::metadata(&archive.amplihack)
        .with_context(|| format!("stat {}", archive.amplihack.display()))?;
    fs::metadata(&archive.hooks).with_context(|| format!("stat {}", archive.hooks.display()))?;

    Ok(BinaryInstallPlan {
        amplihack_destination: install_dir.join(binary_filename("amplihack")),
        hooks_destination: install_dir.join(binary_filename("amplihack-hooks")),
    })
}

fn update_manual_repair_guidance(conflicts: &[PathBuf]) -> String {
    let mut guidance = String::new();
    guidance.push_str("amplihack will not write to privileged system locations automatically.\n");
    if !conflicts.is_empty() {
        guidance.push_str("Conflicting PATH candidates:\n");
        for conflict in conflicts {
            guidance.push_str(&format!("  - {}\n", repair_guidance_path_display(conflict)));
        }
    }
    guidance.push_str(
        "Move the user-level install earlier in PATH:\n  export PATH=\"$HOME/.local/bin:$PATH\"\n",
    );
    guidance.push_str(
        "If /usr/local/bin contains stale amplihack binaries, remove them manually with sudo:\n  sudo rm /usr/local/bin/amplihack /usr/local/bin/amplihack-hooks",
    );
    guidance
}

fn repair_guidance_path_display(path: &Path) -> String {
    let normalized = path.to_string_lossy().replace('\\', "/");
    if normalized.contains("/usr/local/bin/amplihack-hooks") {
        "/usr/local/bin/amplihack-hooks".to_string()
    } else if normalized.contains("/usr/local/bin/amplihack") {
        "/usr/local/bin/amplihack".to_string()
    } else {
        normalized
    }
}

fn current_process_install_target_decision(
    current_exe: &Path,
) -> Result<crate::path_conflicts::InstallTargetDecision> {
    let home_dir = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .context("HOME is not set; cannot determine safe user-level update target")?;
    // Issue #1954: a binary running from a release was started through its
    // link in the user bin dir; that link, not the release, is what an update
    // replaces.
    let releases =
        crate::commands::install::release_layout::releases_dir(&home_dir.join(".amplihack"));
    let current_exe =
        if crate::commands::install::release_layout::is_release_path(&releases, current_exe) {
            crate::path_conflicts::preferred_user_bin(&home_dir).join(binary_filename("amplihack"))
        } else {
            current_exe.to_path_buf()
        };
    let report =
        crate::path_conflicts::analyze_current_process_path_conflicts(home_dir, current_exe)?;
    let probes = crate::path_conflicts::probe_candidates_without_exec(&report);
    let decision = crate::path_conflicts::decide_update_install_target(
        crate::path_conflicts::TargetDecisionInput {
            report: report.clone(),
            candidate_probes: probes,
            denied_system_prefixes: crate::path_conflicts::default_denied_system_prefixes(),
        },
    )?;
    if let Some(notice) = crate::path_conflicts::update_path_conflict_notice(&report, &decision) {
        println!("{notice}");
    }
    Ok(decision)
}

/// Invoke `<binary> --version` and confirm the output contains `expected`.
///
/// `clap`'s default `--version` output is `"<name> <version>"`, so we match
/// on substring rather than equality. Runs with a short timeout so a hung
/// or broken binary cannot stall the caller.
fn verify_installed_version(binary: &Path, expected: &str) -> Result<()> {
    let mut child = Command::new(binary)
        .arg("--version")
        .env("AMPLIHACK_NO_UPDATE_CHECK", "1")
        .env("AMPLIHACK_NONINTERACTIVE", "1")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("failed to exec updated binary with --version")?;

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if child
            .try_wait()
            .context("failed while waiting for updated binary --version")?
            .is_some()
        {
            break;
        }

        if std::time::Instant::now() >= deadline {
            if let Err(err) = child.kill() {
                tracing::warn!("failed to kill timed-out version check process: {err}");
            }
            if let Err(err) = child.wait() {
                tracing::warn!("failed to reap timed-out version check process: {err}");
            }
            bail!("timed out waiting for --version from updated binary");
        }

        std::thread::sleep(Duration::from_millis(50));
    }

    let output = child
        .wait_with_output()
        .context("failed while collecting output from updated binary --version")?;

    if !output.status.success() {
        bail!(
            "updated binary exited with status {} when run with --version",
            output.status
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout.contains(expected) {
        bail!(
            "expected `{expected}` in --version output, got: {}",
            stdout.trim()
        );
    }
    Ok(())
}

/// Refuse to start the binary swap when the destination filesystem can't
/// hold the new binaries plus a small safety margin.
///
/// On Unix we use `statvfs`. On other platforms we soft-fail open (log the
/// attempt and let the copy run) — better to let the error surface from
/// `fs::copy` than to block updates on hosts we can't probe.
#[cfg(unix)]
fn check_free_space(dir: &Path, required_bytes: u64) -> Result<()> {
    let Some(available) = available_space_bytes(dir)? else {
        tracing::warn!(
            "statvfs({}) failed; skipping free-space preflight",
            dir.display()
        );
        return Ok(());
    };

    if available < required_bytes {
        bail!(
            "not enough free space to install update: {} needs {} bytes free but has {} bytes. Free up disk and re-run `amplihack update`.",
            dir.display(),
            required_bytes,
            available
        );
    }
    Ok(())
}

#[cfg(unix)]
fn available_space_bytes(dir: &Path) -> Result<Option<u64>> {
    use std::ffi::CString;
    use std::mem::MaybeUninit;
    use std::os::unix::ffi::OsStrExt;

    let c_path =
        CString::new(dir.as_os_str().as_bytes()).context("install dir contains a NUL byte")?;
    let mut stat = MaybeUninit::<libc::statvfs>::uninit();
    let rc = unsafe { libc::statvfs(c_path.as_ptr(), stat.as_mut_ptr()) };
    if rc != 0 {
        return Ok(None);
    }
    // SAFETY: statvfs returned success, so it initialized `stat`.
    let stat = unsafe { stat.assume_init() };
    // `statvfs` field widths differ by OS: Linux uses `c_ulong` (u64 on our
    // 64-bit targets), macOS uses `u32`. Cast both to `u64` so the math works
    // on every supported host. Clippy flags the cast as unnecessary on Linux
    // but it's mandatory on macOS, so silence the lint on the one line.
    #[allow(clippy::unnecessary_cast)]
    let available: u64 = (stat.f_bavail as u64).saturating_mul(stat.f_frsize as u64);
    Ok(Some(available))
}

#[cfg(not(unix))]
fn check_free_space(_dir: &Path, _required_bytes: u64) -> Result<()> {
    Ok(())
}

fn install_binary_atomic(source: &Path, destination: &Path) -> Result<()> {
    let temp_destination = destination.with_extension("tmp");
    fs::copy(source, &temp_destination).with_context(|| {
        format!(
            "failed to copy {} to {}",
            source.display(),
            temp_destination.display()
        )
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temp_destination, fs::Permissions::from_mode(0o755))
            .with_context(|| format!("failed to chmod {}", temp_destination.display()))?;
    }

    fs::rename(&temp_destination, destination).with_context(|| {
        format!(
            "failed to replace {} with {}",
            destination.display(),
            temp_destination.display()
        )
    })?;
    Ok(())
}
