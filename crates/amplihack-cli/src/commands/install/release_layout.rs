//! The versioned install layout (issue #1954).
//!
//! An install never rewrites or deletes a file that a running recipe run may
//! still exec or read. Instead every install writes a fresh release directory
//!
//! ```text
//! ~/.amplihack/releases/<version>[-<UTC yyyymmddTHHMMSSZ>-<pid>[-<n>]]/
//!     bin/{amplihack,amplihack-hooks,amplihack-asset-resolver}
//!     amplifier-bundle/
//!     .installed-by
//!     .complete
//! ```
//!
//! and then switches the published paths to it, each with one `rename(2)` of a
//! symlink, so a published path is never missing:
//!
//! - `~/.local/bin/amplihack-hooks`, `amplihack-asset-resolver`, `amplihack`
//! - `~/.amplihack/amplifier-bundle`
//!
//! Old releases, and trees an install moved aside (`releases/.retired/`), are
//! removed only once no process references them ([`cleanup_unused`]). Nothing
//! here ever signals a process.
//!
//! This module is the only code that builds or deletes paths under
//! `~/.amplihack/releases`. Every path read back from a link or a manifest is
//! confined with [`is_release_path`] before it is used (S7, S8).

use anyhow::{Context, Result, anyhow, bail};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

/// Directory under `~/.amplihack` that holds the releases.
pub const RELEASES_DIR: &str = "releases";
/// Directory under `releases/` that holds trees moved aside by an install.
pub const RETIRED_DIR: &str = ".retired";
/// Marker written into a release once it is complete, before any switch.
pub const COMPLETE_MARKER: &str = ".complete";
/// Provenance record written into a release before [`COMPLETE_MARKER`].
pub const INSTALLED_BY: &str = ".installed-by";
/// Temporary name the provenance record is written under, then renamed.
const INSTALLED_BY_TMP: &str = ".installed-by.tmp";
/// The largest provenance record the reader accepts (S-V6).
const INSTALLED_BY_MAX_BYTES: u64 = 4 * 1024;

/// The longest accepted version string (S6).
const MAX_VERSION_LEN: usize = 64;
/// How many temporary link names a switch tries before giving up.
const TMP_NAME_ATTEMPTS: u32 = 64;
/// Above this many releases kept for running processes, an install warns.
const KEPT_IN_USE_WARN_ABOVE: usize = 5;

/// The published links, in the order a commit switches them. Hooks first, so
/// a hook started mid-switch always finds a binary at least as new as the
/// `amplihack` that started it; the bundle last.
const SWITCH_ORDER: &[&str] = &[
    "amplihack-hooks",
    "amplihack-asset-resolver",
    "amplihack",
    "amplifier-bundle",
];

/// `<amplihack_dir>/releases`.
pub fn releases_dir(amplihack_dir: &Path) -> PathBuf {
    amplihack_dir.join(RELEASES_DIR)
}

/// The current UTC time as `yyyymmddTHHMMSSZ`.
fn utc_stamp() -> String {
    chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string()
}

#[cfg(unix)]
pub(crate) fn euid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

#[cfg(unix)]
fn owned_by_euid(meta: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    meta.uid() == euid()
}

#[cfg(not(unix))]
fn owned_by_euid(_meta: &fs::Metadata) -> bool {
    true
}

// ------------------------------------------------------------ naming (S6)

/// Accepts `^[0-9A-Za-z][0-9A-Za-z._+-]{0,63}$`. Anything else is refused,
/// never sanitised: the version becomes a directory name.
pub fn validate_version(version: &str) -> Result<()> {
    let bytes = version.as_bytes();
    let Some(first) = bytes.first() else {
        bail!("refusing an empty release version");
    };
    if bytes.len() > MAX_VERSION_LEN {
        bail!(
            "refusing release version of {} bytes (at most {MAX_VERSION_LEN})",
            bytes.len()
        );
    }
    if !first.is_ascii_alphanumeric() {
        bail!(
            "refusing release version {:?}: it must start with a letter or digit",
            version.escape_debug().to_string()
        );
    }
    if let Some(bad) = bytes
        .iter()
        .find(|&&b| !(b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'+' | b'-')))
    {
        bail!(
            "refusing release version {:?}: byte {bad:#04x} is not allowed",
            version.escape_debug().to_string()
        );
    }
    Ok(())
}

// ------------------------------------------------------- confinement (S7)

/// The release root (`<releases>/<name>`, as given, not canonical) that
/// `path` resolves into, if any. Canonicalises both sides and compares by
/// component; `releases/` itself, `releases/.retired/…`, anything outside and
/// anything that fails to resolve give `None`.
pub fn release_root_of(releases: &Path, path: &Path) -> Option<PathBuf> {
    let canonical_releases = fs::canonicalize(releases).ok()?;
    let canonical = fs::canonicalize(path).ok()?;
    let suffix = canonical.strip_prefix(&canonical_releases).ok()?;
    let Some(Component::Normal(name)) = suffix.components().next() else {
        return None;
    };
    if name == RETIRED_DIR {
        return None;
    }
    Some(releases.join(name))
}

/// Whether `path` resolves to a release root or to something inside one.
pub fn is_release_path(releases: &Path, path: &Path) -> bool {
    release_root_of(releases, path).is_some()
}

/// Whether `path` resolves into a release that has [`COMPLETE_MARKER`].
pub fn is_complete_release_path(releases: &Path, path: &Path) -> bool {
    release_root_of(releases, path).is_some_and(|root| {
        fs::symlink_metadata(root.join(COMPLETE_MARKER)).is_ok_and(|m| m.is_file())
    })
}

// --------------------------------------------- implicit install origin (#2496)

/// Stable slugs naming the check that refused an origin. They are what the
/// `debug` diagnostic prints, and each corresponds to a row of the
/// "Diagnosing a refusal" table in
/// `docs/features/self-heal-asset-restage.md`. Treat them as a published
/// vocabulary rather than free text: users paste them into issues.
mod refusal {
    pub(super) const HOME_UNRESOLVED: &str = "home-directory-unresolved";
    pub(super) const EXECUTABLE_PATH_UNKNOWN: &str = "executable-path-unknown";
    #[cfg(unix)]
    pub(super) const EXECUTABLE_PATH_UNRESOLVED: &str = "executable-path-unresolved";
    #[cfg(unix)]
    pub(super) const EXECUTABLE_OUTSIDE_RELEASE: &str = "executable-outside-installed-release";
    #[cfg(unix)]
    pub(super) const EXECUTABLE_NOT_IN_RELEASE_BIN: &str = "executable-not-in-release-bin";
    #[cfg(unix)]
    pub(super) const EXECUTABLE_NOT_A_REGULAR_FILE: &str = "executable-not-a-regular-file";
    #[cfg(unix)]
    pub(super) const EXECUTABLE_NOT_OWNED: &str = "executable-not-owned-by-this-user";
    #[cfg(unix)]
    pub(super) const EXECUTABLE_WRITABLE: &str = "executable-writable-by-group-or-others";
    #[cfg(unix)]
    pub(super) const DIRECTORY_UNREADABLE: &str = "directory-missing-or-unreadable";
    #[cfg(unix)]
    pub(super) const DIRECTORY_IS_A_SYMLINK: &str = "directory-is-a-symlink";
    #[cfg(unix)]
    pub(super) const DIRECTORY_NOT_OWNED: &str = "directory-not-owned-by-this-user";
    #[cfg(unix)]
    pub(super) const DIRECTORY_WRITABLE: &str = "directory-writable-by-group-or-others";
    #[cfg(unix)]
    pub(super) const RELEASE_INCOMPLETE: &str = "release-incomplete";
    #[cfg(unix)]
    pub(super) const RELEASE_RETIRED: &str = "release-retired";
    #[cfg(not(unix))]
    pub(super) const UNSUPPORTED_PLATFORM: &str = "unsupported-platform";
}

/// Why an executable may not install, re-stage or publish implicitly.
///
/// [`OriginRefusal::check`] names the rule that refused and
/// [`OriginRefusal::subject`] the path it was applied to, so one `debug`
/// record tells a user which of the documented causes they have hit. The
/// path is raw here; everything that prints it passes it through
/// [`super::paths::sanitize_path`] first.
#[derive(Debug, Clone)]
pub(crate) struct OriginRefusal {
    check: &'static str,
    subject: Option<PathBuf>,
}

impl OriginRefusal {
    /// Every path-bearing cause comes from a Unix ownership or mode check.
    #[cfg(unix)]
    fn of(check: &'static str, subject: &Path) -> Self {
        Self {
            check,
            subject: Some(subject.to_path_buf()),
        }
    }

    /// For causes that have no path to name: no `HOME`, no `current_exe()`,
    /// or a platform whose confinement cannot be assessed at all.
    fn pathless(check: &'static str) -> Self {
        Self {
            check,
            subject: None,
        }
    }

    pub(crate) fn check(&self) -> &'static str {
        self.check
    }

    pub(crate) fn subject(&self) -> Option<&Path> {
        self.subject.as_deref()
    }
}

/// Causes already recorded by this process, so a run that consults the oracle
/// at several boundaries reports each distinct cause once. This memoises the
/// *diagnostic*, never the verdict: every boundary still recomputes
/// [`installed_executable`] against the tree as it stands (S4).
static REPORTED_CAUSES: std::sync::Mutex<BTreeSet<String>> = std::sync::Mutex::new(BTreeSet::new());

fn report_origin_refusal(refusal: &OriginRefusal) {
    let subject = refusal
        .subject()
        .map_or_else(|| "<unknown>".to_owned(), super::paths::sanitize_path);
    let fresh = REPORTED_CAUSES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(format!("{} at {subject}", refusal.check()));
    if fresh {
        tracing::debug!(
            check = %refusal.check(),
            path = %subject,
            "implicit installation refused by executable-origin check"
        );
    }
}

/// [`installed_executable`] with the single observability point attached: on
/// refusal it records the specific cause at `debug`, which is what
/// `RUST_LOG=amplihack_cli=debug` surfaces. Eligibility is silent.
pub(crate) fn assess_implicit_install_origin(
    home: &Path,
    executable: &Path,
) -> Result<(), OriginRefusal> {
    match installed_executable(home, executable) {
        Ok(()) => Ok(()),
        Err(refusal) => {
            report_origin_refusal(&refusal);
            Err(refusal)
        }
    }
}

/// Only an executable in a complete installed release may repair the host
/// implicitly. This assessment is read-only and deliberately ignores source
/// selection overrides such as `AMPLIHACK_HOME`.
///
/// Eligibility is derived from `HOME` alone and may only ever be *subtracted*
/// from. No environment variable, flag, configuration key or argument may be
/// wired in here to grant it: a caller-supplied trust anchor would let any
/// process declare itself an installed release, which is the defect #2496
/// exists to close. The automatic-install opt-out is a separate, advisory
/// control that can only refuse further.
pub(crate) fn executable_allows_implicit_install() -> bool {
    #[cfg(test)]
    if let Some(eligible) = TEST_ELIGIBILITY.with_borrow(Clone::clone) {
        return match eligible {
            TestOrigin::Installed => true,
            TestOrigin::Executable(path) => super::paths::home_dir()
                .is_ok_and(|home| assess_implicit_install_origin(&home, &path).is_ok()),
        };
    }
    let Ok(home) = super::paths::home_dir() else {
        report_origin_refusal(&OriginRefusal::pathless(refusal::HOME_UNRESOLVED));
        return false;
    };
    let Ok(executable) = std::env::current_exe() else {
        report_origin_refusal(&OriginRefusal::pathless(refusal::EXECUTABLE_PATH_UNKNOWN));
        return false;
    };
    assess_implicit_install_origin(&home, &executable).is_ok()
}

// Unit tests of installation behaviour can supply eligibility without moving
// their running harness. Process-level origin regressions use current_exe().
#[cfg(test)]
thread_local! {
    static TEST_ELIGIBILITY: std::cell::RefCell<Option<TestOrigin>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
#[derive(Clone)]
enum TestOrigin {
    Installed,
    Executable(PathBuf),
}

#[cfg(test)]
pub(crate) struct TestEligibility(Option<TestOrigin>);

#[cfg(test)]
impl TestEligibility {
    pub(crate) fn installed() -> Self {
        Self(TEST_ELIGIBILITY.replace(Some(TestOrigin::Installed)))
    }

    pub(crate) fn executable(path: PathBuf) -> Self {
        Self(TEST_ELIGIBILITY.replace(Some(TestOrigin::Executable(path))))
    }
}

#[cfg(test)]
impl Drop for TestEligibility {
    fn drop(&mut self) {
        TEST_ELIGIBILITY.replace(self.0.take());
    }
}

/// Issue #2496: the checks that make an origin trustworthy are the Unix
/// ownership and file-mode ones. Where they do not exist, path, confinement
/// and completeness checks alone would let any writable tree pass, so the
/// oracle refuses outright and explicit `amplihack install` is the only
/// route. This is deliberately not a silent degradation.
#[cfg(not(unix))]
fn installed_executable(_home: &Path, _executable: &Path) -> Result<(), OriginRefusal> {
    Err(OriginRefusal::pathless(refusal::UNSUPPORTED_PLATFORM))
}

/// A directory on the trusted path: a real directory, owned by this user and
/// not writable by group or others. Mirrors [`check_private_dir`] but reports
/// a typed cause instead of an `anyhow` message holding an unsanitised path.
#[cfg(unix)]
fn private_dir_for_origin(dir: &Path) -> Result<(), OriginRefusal> {
    use std::os::unix::fs::PermissionsExt;
    let meta = fs::symlink_metadata(dir)
        .map_err(|_| OriginRefusal::of(refusal::DIRECTORY_UNREADABLE, dir))?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(OriginRefusal::of(refusal::DIRECTORY_IS_A_SYMLINK, dir));
    }
    if !owned_by_euid(&meta) {
        return Err(OriginRefusal::of(refusal::DIRECTORY_NOT_OWNED, dir));
    }
    if meta.permissions().mode() & 0o022 != 0 {
        return Err(OriginRefusal::of(refusal::DIRECTORY_WRITABLE, dir));
    }
    Ok(())
}

/// Whether `canonical` sits under `releases/.retired/`, which
/// [`release_root_of`] reports the same way as "outside the tree". A
/// superseded binary must not be able to resurrect itself (S9), and saying so
/// is more useful than calling it a checkout.
#[cfg(unix)]
fn resolves_into_retired(releases: &Path, canonical: &Path) -> bool {
    let Ok(canonical_releases) = fs::canonicalize(releases) else {
        return false;
    };
    let Ok(suffix) = canonical.strip_prefix(&canonical_releases) else {
        return false;
    };
    matches!(
        suffix.components().next(),
        Some(Component::Normal(name)) if name == RETIRED_DIR
    )
}

/// Whether `executable` is an installed release binary of the install tree
/// under `home`, with the specific cause when it is not.
///
/// Every step is deny-by-default: a failure to stat, canonicalise or confine
/// a path is a refusal, never a permissive default.
#[cfg(unix)]
fn installed_executable(home: &Path, executable: &Path) -> Result<(), OriginRefusal> {
    use std::os::unix::fs::PermissionsExt;

    let install = home.join(".amplihack");
    let releases = releases_dir(&install);
    // Canonical confinement alone would accept redirected installation roots.
    for dir in [&install, &releases] {
        private_dir_for_origin(dir)?;
    }
    let canonical = fs::canonicalize(executable)
        .map_err(|_| OriginRefusal::of(refusal::EXECUTABLE_PATH_UNRESOLVED, executable))?;
    let Some(root) = release_root_of(&releases, executable) else {
        return Err(OriginRefusal::of(
            if resolves_into_retired(&releases, &canonical) {
                refusal::RELEASE_RETIRED
            } else {
                refusal::EXECUTABLE_OUTSIDE_RELEASE
            },
            executable,
        ));
    };
    private_dir_for_origin(&root)?;
    if !is_complete_release_path(&releases, executable) {
        return Err(OriginRefusal::of(refusal::RELEASE_INCOMPLETE, &root));
    }
    let bin = root.join("bin");
    private_dir_for_origin(&bin)?;
    let canonical_bin = fs::canonicalize(&bin)
        .map_err(|_| OriginRefusal::of(refusal::DIRECTORY_UNREADABLE, &bin))?;
    if canonical.parent() != Some(canonical_bin.as_path()) {
        return Err(OriginRefusal::of(
            refusal::EXECUTABLE_NOT_IN_RELEASE_BIN,
            executable,
        ));
    }
    let metadata = fs::symlink_metadata(&canonical)
        .map_err(|_| OriginRefusal::of(refusal::EXECUTABLE_PATH_UNRESOLVED, executable))?;
    if !metadata.is_file() {
        return Err(OriginRefusal::of(
            refusal::EXECUTABLE_NOT_A_REGULAR_FILE,
            executable,
        ));
    }
    if !owned_by_euid(&metadata) {
        return Err(OriginRefusal::of(refusal::EXECUTABLE_NOT_OWNED, executable));
    }
    if metadata.permissions().mode() & 0o022 != 0 {
        return Err(OriginRefusal::of(refusal::EXECUTABLE_WRITABLE, executable));
    }
    Ok(())
}

/// For a published link at `link` (`<amp>/amplifier-bundle` or similar): the
/// canonical directory it names, when that is a real directory inside a
/// complete release under `<link's parent>/releases`. `None` otherwise,
/// including when that `releases` is a symlink, is not owned by this user or
/// is writable by group or others: a source checkout could otherwise point
/// `releases` anywhere and have that tree staged (#243).
pub fn resolve_bundle_link(link: &Path) -> Option<PathBuf> {
    let meta = fs::symlink_metadata(link).ok()?;
    if !meta.file_type().is_symlink() {
        return None;
    }
    let releases = releases_dir(link.parent()?);
    check_private_dir(&releases, &fs::symlink_metadata(&releases).ok()?).ok()?;
    if !is_complete_release_path(&releases, link) {
        return None;
    }
    let target = fs::canonicalize(link).ok()?;
    let target_meta = fs::symlink_metadata(&target).ok()?;
    target_meta.is_dir().then_some(target)
}

// ---------------------------------------------------- private dirs (S1)

/// Creates `dir` 0700 when missing. An existing `dir` must be a real
/// directory owned by this user and not writable by group or others; it is
/// never repaired.
fn ensure_private_dir(dir: &Path) -> Result<()> {
    match fs::symlink_metadata(dir) {
        Ok(meta) => check_private_dir(dir, &meta),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if let Some(parent) = dir.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("failed to create {}", parent.display()))?;
            }
            match create_dir_0700(dir) {
                Ok(()) => {}
                // Created by someone else since the check: inspect it below.
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => {
                    return Err(e).with_context(|| format!("failed to create {}", dir.display()));
                }
            }
            let meta = fs::symlink_metadata(dir)
                .with_context(|| format!("failed to inspect {}", dir.display()))?;
            check_private_dir(dir, &meta)
        }
        Err(e) => Err(e).with_context(|| format!("failed to inspect {}", dir.display())),
    }
}

fn check_private_dir(dir: &Path, meta: &fs::Metadata) -> Result<()> {
    if meta.file_type().is_symlink() || !meta.is_dir() {
        bail!(
            "{} is not a real directory; refusing to use it for amplihack releases",
            dir.display()
        );
    }
    if !owned_by_euid(meta) {
        bail!(
            "{} is not owned by this user; refusing to use it for amplihack releases",
            dir.display()
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode() & 0o7777;
        if mode & 0o022 != 0 {
            bail!(
                "{} is writable by group or others (mode {mode:o}); refusing to use it for \
                 amplihack releases. Fix it with `chmod 700 {}`",
                dir.display(),
                dir.display()
            );
        }
    }
    Ok(())
}

fn create_dir_0700(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(0o700).create(dir)
    }
    #[cfg(not(unix))]
    {
        fs::create_dir(dir)
    }
}

// ------------------------------------------------------- transaction (D1)

/// One planned link switch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkSwitch {
    /// The published path.
    pub link: PathBuf,
    /// What it names now.
    pub target: PathBuf,
    /// What it named before, when it was a symlink.
    pub previous: Option<PathBuf>,
}

/// What [`ReleaseTxn::commit`] switched. A failure stops the switching and
/// nothing is rolled back: every link still names a complete release.
#[derive(Debug, Clone, Default)]
pub struct SwitchReport {
    pub switched: Vec<LinkSwitch>,
    pub failed: Option<(PathBuf, String)>,
}

impl SwitchReport {
    /// The previous target of every switched link.
    pub fn previous_targets(&self) -> Vec<PathBuf> {
        self.switched
            .iter()
            .filter_map(|s| s.previous.clone())
            .collect()
    }
}

/// A release being built. Dropped without [`ReleaseTxn::commit`], it removes
/// its directory, which no link ever named.
#[derive(Debug)]
pub struct ReleaseTxn {
    amplihack_dir: PathBuf,
    root: PathBuf,
    version: String,
    links: Vec<(PathBuf, PathBuf)>,
    committed: bool,
}

impl ReleaseTxn {
    /// Creates a new, empty release directory for `version`.
    ///
    /// `releases/<version>` when free, else `releases/<version>-<ts>-<pid>`,
    /// else that name with `-<n>`. An existing directory is never reused.
    ///
    /// Call this only while holding the install lock
    /// ([`crate::install_lock`]). [`cleanup_unused`] removes any release
    /// without [`COMPLETE_MARKER`] that is not in the keep set, with no age
    /// check, so a release allocated outside the lock could be removed while
    /// it is still being staged.
    pub fn allocate(amplihack_dir: &Path, version: &str) -> Result<Self> {
        validate_version(version)?;
        let releases = releases_dir(amplihack_dir);
        ensure_private_dir(&releases)?;
        let stamped = format!("{version}-{}-{}", utc_stamp(), std::process::id());
        let candidates = std::iter::once(version.to_string())
            .chain(std::iter::once(stamped.clone()))
            .chain((1..1000).map(|n| format!("{stamped}-{n}")));
        for name in candidates {
            let root = releases.join(&name);
            match create_dir_0700(&root) {
                Ok(()) => {
                    tracing::debug!("release: allocated {}", root.display());
                    return Ok(Self {
                        amplihack_dir: amplihack_dir.to_path_buf(),
                        root,
                        version: version.to_string(),
                        links: Vec::new(),
                        committed: false,
                    });
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => {
                    return Err(e).with_context(|| {
                        format!("failed to create release directory {}", root.display())
                    });
                }
            }
        }
        bail!(
            "could not find a free release directory name for {version} under {}",
            releases.display()
        )
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn bin_dir(&self) -> PathBuf {
        self.root.join("bin")
    }

    pub fn bundle_dir(&self) -> PathBuf {
        self.root.join("amplifier-bundle")
    }

    /// Plans `link` to name `target`, which must be inside this release.
    pub fn plan_link(&mut self, link: &Path, target: &Path) -> Result<()> {
        let inside = target.starts_with(&self.root)
            && target != self.root
            && !target
                .components()
                .any(|c| matches!(c, Component::ParentDir | Component::CurDir));
        if !inside {
            bail!(
                "refusing to link {} to {}: the target is not inside release {}",
                link.display(),
                target.display(),
                self.root.display()
            );
        }
        self.links.retain(|(l, _)| l != link);
        self.links.push((link.to_path_buf(), target.to_path_buf()));
        Ok(())
    }

    /// Writes the provenance record, marks the release complete, fixes its
    /// modes and switches every planned link. A failure before the switches
    /// returns `Err` and the release, which no link names yet, is removed on
    /// drop. A failed switch stops there; it is reported, not rolled back.
    pub fn commit(mut self) -> Result<SwitchReport> {
        write_installed_by(&self.root, &self.version)?;
        let marker = self.root.join(COMPLETE_MARKER);
        fs::write(&marker, format!("{}\n", utc_stamp()))
            .with_context(|| format!("failed to write {}", marker.display()))?;
        normalize_release_modes(&self.root)?;
        seal_installed_by(&self.root)?;
        // From here the release may be named by a link: never remove it on drop.
        self.committed = true;

        let mut links = std::mem::take(&mut self.links);
        links.sort_by_key(|(link, _)| switch_rank(link));
        let mut report = SwitchReport::default();
        for (link, target) in links {
            match switch_link(&self.amplihack_dir, &link, &target) {
                Ok(switch) => {
                    tracing::info!(
                        "release: {} -> {}",
                        switch.link.display(),
                        switch.target.display()
                    );
                    report.switched.push(switch);
                }
                Err(e) => {
                    tracing::warn!("release: failed to switch {}: {e:#}", link.display());
                    report.failed = Some((link, format!("{e:#}")));
                    break;
                }
            }
        }
        Ok(report)
    }
}

impl Drop for ReleaseTxn {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        // A release no link ever named, built by this process under the lock.
        if let Err(e) = remove_tree(&self.root) {
            tracing::warn!(
                "release: could not remove unpublished release {}: {e:#}",
                self.root.display()
            );
            eprintln!(
                "amplihack: warning: could not remove unpublished release {}: {e:#}",
                self.root.display()
            );
        }
    }
}

// ------------------------------------------- provenance record (C6)

/// Which process committed a release, read from [`INSTALLED_BY`].
///
/// This record is evidence for people debugging an install (for example,
/// which binary re-staged on the first update). It is **not a trust input**:
/// production code never reads it, never signals the recorded pid and never
/// runs the recorded exe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledBy {
    pub version: String,
    /// The canonical path of the committing executable, if it was known and
    /// safe to write on one line.
    pub exe: Option<PathBuf>,
    pub pid: u32,
    /// Whether `AMPLIHACK_POST_UPDATE_INSTALL` was exactly `1`.
    pub post_update: bool,
}

/// The exact file text for `record`. An exe path that is not UTF-8 or holds
/// `\n`, `\r` or `\0` is written empty, so it can never forge a line.
pub fn format_installed_by(record: &InstalledBy) -> String {
    let exe = record
        .exe
        .as_deref()
        .and_then(Path::to_str)
        .filter(|s| !s.contains(['\n', '\r', '\0']))
        .unwrap_or("");
    format!(
        "format=1\nversion={}\nexe={exe}\npid={}\npost_update={}\n",
        record.version,
        record.pid,
        if record.post_update { 1 } else { 0 }
    )
}

/// Writes [`INSTALLED_BY`] into `release_root` for this process.
///
/// The text goes to a temporary file created with `O_EXCL`, which is then
/// renamed over the record. A stale temporary file or symlink is removed
/// first, never followed; a directory there is never removed and fails the
/// write.
pub fn write_installed_by(release_root: &Path, version: &str) -> Result<()> {
    validate_version(version)?;
    let exe = std::env::current_exe()
        .and_then(fs::canonicalize)
        .map_err(|e| {
            tracing::info!("release: exe for {INSTALLED_BY} unknown: {e}");
            e
        })
        .ok();
    let record = InstalledBy {
        version: version.to_string(),
        exe,
        pid: std::process::id(),
        post_update: std::env::var("AMPLIHACK_POST_UPDATE_INSTALL").as_deref() == Ok("1"),
    };
    let text = format_installed_by(&record);

    let tmp = release_root.join(INSTALLED_BY_TMP);
    match fs::symlink_metadata(&tmp) {
        Ok(meta) if meta.is_file() || meta.file_type().is_symlink() => {
            fs::remove_file(&tmp)
                .with_context(|| format!("failed to remove stale {}", tmp.display()))?;
        }
        Ok(_) => bail!(
            "refusing to write {}: {} exists and is not a regular file or symlink",
            INSTALLED_BY,
            tmp.display()
        ),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(e).with_context(|| format!("failed to inspect {}", tmp.display()));
        }
    }
    write_new_file_0644(&tmp, text.as_bytes())
        .with_context(|| format!("failed to write {}", tmp.display()))?;
    let dest = release_root.join(INSTALLED_BY);
    fs::rename(&tmp, &dest)
        .with_context(|| format!("failed to rename {} to {}", tmp.display(), dest.display()))?;
    tracing::debug!(
        "release: wrote {} (pid {}, post_update {})",
        dest.display(),
        record.pid,
        record.post_update
    );
    Ok(())
}

/// Makes the committed record read-only (0444), so nothing rewrites it in
/// place.
#[cfg(unix)]
fn seal_installed_by(release_root: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let path = release_root.join(INSTALLED_BY);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o444))
        .with_context(|| format!("failed to set mode 444 on {}", path.display()))
}

#[cfg(not(unix))]
fn seal_installed_by(_release_root: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn write_new_file_0644(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    file.write_all(bytes)?;
    // The umask may have narrowed the mode; the record is world-readable.
    file.set_permissions(fs::Permissions::from_mode(0o644))?;
    file.sync_all()
}

#[cfg(not(unix))]
fn write_new_file_0644(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// Reads [`INSTALLED_BY`] from `release_root`.
///
/// Returns `None` unless it is a regular file (a symlink is not followed) of
/// at most 4 KiB of UTF-8 with a non-empty `version` and a numeric `pid`.
/// The first occurrence of each key wins; unknown keys are ignored. Never
/// panics. Not a trust input: see [`InstalledBy`].
pub fn read_installed_by(release_root: &Path) -> Option<InstalledBy> {
    use std::io::Read;
    let path = release_root.join(INSTALLED_BY);
    let file = open_no_follow(&path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(INSTALLED_BY_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > INSTALLED_BY_MAX_BYTES {
        return None;
    }
    let text = String::from_utf8(bytes).ok()?;

    let mut fields: BTreeMap<&str, &str> = BTreeMap::new();
    for line in text.lines() {
        if let Some((key, value)) = line.split_once('=') {
            fields.entry(key).or_insert(value);
        }
    }
    let version = fields.get("version").copied().unwrap_or("");
    if version.is_empty() {
        return None;
    }
    let pid = fields.get("pid")?.parse::<u32>().ok()?;
    let exe = fields
        .get("exe")
        .filter(|e| !e.is_empty())
        .map(PathBuf::from);
    let post_update = fields.get("post_update").copied() == Some("1");
    Some(InstalledBy {
        version: version.to_string(),
        exe,
        pid,
        post_update,
    })
}

#[cfg(unix)]
fn open_no_follow(path: &Path) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
}

#[cfg(not(unix))]
fn open_no_follow(path: &Path) -> io::Result<fs::File> {
    if fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(io::Error::other("refusing to follow a symlink"));
    }
    fs::File::open(path)
}

fn switch_rank(link: &Path) -> usize {
    let name = link.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let name = name.strip_suffix(".exe").unwrap_or(name);
    SWITCH_ORDER
        .iter()
        .position(|n| *n == name)
        .unwrap_or(SWITCH_ORDER.len())
}

/// No file in a release is writable by group or others (S16); the root is
/// 0755. Symlinks are skipped, never followed.
#[cfg(unix)]
fn normalize_release_modes(root: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        let meta = fs::symlink_metadata(&path)
            .with_context(|| format!("failed to inspect {}", path.display()))?;
        if meta.file_type().is_symlink() {
            continue;
        }
        let mode = meta.permissions().mode() & 0o7777;
        let wanted = if path == root {
            0o755
        } else if meta.is_dir() {
            (mode & !0o022) | 0o755
        } else {
            (mode & !0o022) | 0o444
        };
        if wanted != mode {
            fs::set_permissions(&path, fs::Permissions::from_mode(wanted))
                .with_context(|| format!("failed to set mode {wanted:o} on {}", path.display()))?;
        }
        if meta.is_dir() {
            for entry in
                fs::read_dir(&path).with_context(|| format!("failed to read {}", path.display()))?
            {
                let entry = entry.with_context(|| format!("failed to read {}", path.display()))?;
                pending.push(entry.path());
            }
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn normalize_release_modes(_root: &Path) -> Result<()> {
    Ok(())
}

// --------------------------------------------------------- switching (D3)

/// Points `link` at `target` with one `rename(2)`, so `link` is never missing.
///
/// - missing, a symlink or a regular file at `link`: a temporary symlink is
///   renamed over it. A running process keeps the inode it already has.
/// - a real directory (the legacy bundle, D8): it is exchanged with the
///   temporary symlink (`RENAME_EXCHANGE`) and then retired, contents intact.
pub fn switch_link(amplihack_dir: &Path, link: &Path, target: &Path) -> Result<LinkSwitch> {
    let parent = link
        .parent()
        .ok_or_else(|| anyhow!("link {} has no parent directory", link.display()))?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    let name = link
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow!("link {} has no usable file name", link.display()))?;

    let existing = match fs::symlink_metadata(link) {
        Ok(meta) => Some(meta),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).with_context(|| format!("failed to inspect {}", link.display())),
    };
    let previous = match &existing {
        Some(meta) if meta.file_type().is_symlink() => {
            let raw = fs::read_link(link)
                .with_context(|| format!("failed to read link {}", link.display()))?;
            Some(if raw.is_absolute() {
                raw
            } else {
                parent.join(raw)
            })
        }
        _ => None,
    };

    let tmp = create_tmp_symlink(parent, name, target)?;
    let result = match &existing {
        Some(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {
            replace_dir_with_link(amplihack_dir, link, &tmp, name, meta)
        }
        Some(meta) if !(meta.is_file() || meta.file_type().is_symlink()) => Err(anyhow!(
            "refusing to replace {}: it is neither a file, a symlink nor a directory",
            link.display()
        )),
        _ => fs::rename(&tmp, link).with_context(|| {
            format!(
                "failed to switch {} to {}",
                link.display(),
                target.display()
            )
        }),
    };
    if let Err(e) = result {
        remove_own_tmp_symlink(&tmp);
        return Err(e);
    }
    Ok(LinkSwitch {
        link: link.to_path_buf(),
        target: target.to_path_buf(),
        previous,
    })
}

/// Creates `<dir>/.<name>.tmp-<pid>-<n>` naming `target`. A leftover at a
/// candidate name is removed only when it is a symlink (S14); a regular file
/// there is never touched and the next name is tried.
fn create_tmp_symlink(dir: &Path, name: &str, target: &Path) -> Result<PathBuf> {
    let pid = std::process::id();
    for n in 0..TMP_NAME_ATTEMPTS {
        let tmp = dir.join(format!(".{name}.tmp-{pid}-{n}"));
        match make_symlink(target, &tmp) {
            Ok(()) => return Ok(tmp),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                let is_link = fs::symlink_metadata(&tmp).is_ok_and(|m| m.file_type().is_symlink());
                if !is_link {
                    continue;
                }
                // Our own pid's leftover from a crashed earlier run.
                fs::remove_file(&tmp).with_context(|| {
                    format!("failed to remove stale temporary link {}", tmp.display())
                })?;
                make_symlink(target, &tmp).with_context(|| {
                    format!("failed to create temporary link {}", tmp.display())
                })?;
                return Ok(tmp);
            }
            Err(e) => {
                return Err(e)
                    .with_context(|| format!("failed to create temporary link {}", tmp.display()));
            }
        }
    }
    bail!(
        "no free temporary link name for {} in {} after {TMP_NAME_ATTEMPTS} attempts",
        name,
        dir.display()
    )
}

fn remove_own_tmp_symlink(tmp: &Path) {
    if fs::symlink_metadata(tmp).is_ok_and(|m| m.file_type().is_symlink())
        && let Err(e) = fs::remove_file(tmp)
    {
        tracing::warn!("could not remove temporary link {}: {e}", tmp.display());
    }
}

#[cfg(unix)]
fn make_symlink(target: &Path, at: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, at)
}

#[cfg(not(unix))]
fn make_symlink(_target: &Path, _at: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "the versioned install layout needs symlinks",
    ))
}

/// The legacy bundle directory at `link` becomes the link; the directory ends
/// at the temporary name and is then retired, contents intact.
fn replace_dir_with_link(
    amplihack_dir: &Path,
    link: &Path,
    tmp: &Path,
    label: &str,
    meta: &fs::Metadata,
) -> Result<()> {
    if !owned_by_euid(meta) {
        bail!(
            "refusing to replace {}: the directory is not owned by this user",
            link.display()
        );
    }
    match exchange_raw(tmp, link) {
        Ok(()) => {}
        Err(e) if is_exchange_unsupported(&e) => {
            tracing::warn!(
                "RENAME_EXCHANGE unavailable ({e}); replacing {} with two renames",
                link.display()
            );
            eprintln!(
                "amplihack: warning: this filesystem cannot exchange {} atomically; it is \
                 missing for a moment while it is replaced",
                link.display()
            );
            let aside = free_sibling(link, "aside")?;
            fs::rename(link, &aside).with_context(|| {
                format!("failed to move {} to {}", link.display(), aside.display())
            })?;
            if let Err(e) = fs::rename(tmp, link) {
                // Put the directory back: the link was never published.
                if let Err(back) = fs::rename(&aside, link) {
                    tracing::warn!(
                        "could not restore {} from {}: {back}",
                        link.display(),
                        aside.display()
                    );
                }
                return Err(e).with_context(|| format!("failed to switch {}", link.display()));
            }
            report_retired(retire(amplihack_dir, &aside, label)?);
            return Ok(());
        }
        Err(e) => {
            return Err(e)
                .with_context(|| format!("failed to exchange {} with a link", link.display()));
        }
    }
    // The old directory now sits at the temporary name.
    report_retired(retire(amplihack_dir, tmp, label)?);
    Ok(())
}

fn report_retired(outcome: Retired) {
    match outcome {
        Retired::Moved(to) => tracing::info!("retired {}", to.display()),
        Retired::LeftInPlace(at) => {
            eprintln!(
                "amplihack: warning: left {} in place: it is on another filesystem than \
                 ~/.amplihack/releases/.retired",
                at.display()
            );
        }
    }
}

fn free_sibling(path: &Path, what: &str) -> Result<PathBuf> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("{} has no parent directory", path.display()))?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow!("{} has no usable file name", path.display()))?;
    let pid = std::process::id();
    for n in 0..TMP_NAME_ATTEMPTS {
        let candidate = parent.join(format!(".{name}.{what}-{pid}-{n}"));
        if fs::symlink_metadata(&candidate).is_err() {
            return Ok(candidate);
        }
    }
    bail!("no free name beside {}", path.display())
}

// --------------------------------------------------------- exchange (D3)

/// How [`exchange_dirs`] swapped the two names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exchange {
    /// One atomic `renameat2(RENAME_EXCHANGE)`.
    Exchanged,
    /// Three renames, with a moment in which one name is missing.
    FellBack,
}

/// Swaps two real directories owned by this user.
pub fn exchange_dirs(a: &Path, b: &Path) -> Result<Exchange> {
    exchange_dirs_with(a, b, exchange_raw)
}

/// [`exchange_dirs`] with the exchange call injected. Only `EINVAL` and
/// `ENOSYS` fall back to renames; any other error is returned unchanged
/// (S15) and nothing moves.
pub fn exchange_dirs_with(
    a: &Path,
    b: &Path,
    exchange: impl FnOnce(&Path, &Path) -> io::Result<()>,
) -> Result<Exchange> {
    for path in [a, b] {
        let meta = fs::symlink_metadata(path)
            .with_context(|| format!("failed to inspect {}", path.display()))?;
        if meta.file_type().is_symlink() || !meta.is_dir() {
            bail!(
                "refusing to exchange {}: it is not a real directory",
                path.display()
            );
        }
        if !owned_by_euid(&meta) {
            bail!(
                "refusing to exchange {}: it is not owned by this user",
                path.display()
            );
        }
    }
    match exchange(a, b) {
        Ok(()) => Ok(Exchange::Exchanged),
        Err(e) if is_exchange_unsupported(&e) => {
            tracing::warn!(
                "RENAME_EXCHANGE unavailable ({e}); swapping {} and {} with three renames",
                a.display(),
                b.display()
            );
            let aside = free_sibling(b, "swap")?;
            fs::rename(b, &aside)
                .with_context(|| format!("failed to move {} aside", b.display()))?;
            if let Err(e) = fs::rename(a, b) {
                if let Err(back) = fs::rename(&aside, b) {
                    tracing::warn!("could not restore {}: {back}", b.display());
                }
                return Err(e)
                    .with_context(|| format!("failed to move {} to {}", a.display(), b.display()));
            }
            fs::rename(&aside, a).with_context(|| {
                format!("failed to move {} to {}", aside.display(), a.display())
            })?;
            Ok(Exchange::FellBack)
        }
        Err(e) => Err(anyhow::Error::new(e).context(format!(
            "failed to exchange {} and {}",
            a.display(),
            b.display()
        ))),
    }
}

fn is_exchange_unsupported(e: &io::Error) -> bool {
    matches!(e.raw_os_error(), Some(code) if code == libc::EINVAL || code == libc::ENOSYS)
}

#[cfg(target_os = "linux")]
fn exchange_raw(a: &Path, b: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let cstr = |p: &Path| {
        CString::new(p.as_os_str().as_bytes())
            .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))
    };
    let (a, b) = (cstr(a)?, cstr(b)?);
    // SAFETY: both paths are valid NUL-terminated strings that outlive the
    // call; RENAME_EXCHANGE swaps two existing names atomically and creates
    // or removes nothing.
    let rc = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            a.as_ptr(),
            libc::AT_FDCWD,
            b.as_ptr(),
            libc::RENAME_EXCHANGE,
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(target_os = "linux"))]
fn exchange_raw(_a: &Path, _b: &Path) -> io::Result<()> {
    Err(io::Error::from_raw_os_error(libc::ENOSYS))
}

// ------------------------------------------------------------ retire (D4)

/// Where [`retire`] left a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Retired {
    /// Moved to `releases/.retired/<ts>-<pid>-<label>`.
    Moved(PathBuf),
    /// Left where it was: it is on another filesystem (`EXDEV`).
    LeftInPlace(PathBuf),
}

/// Moves `path` (inside `amplihack_dir`, a real file or directory owned by
/// this user) into `releases/.retired/`, where [`cleanup_unused`] removes it
/// once no process uses it. It is never deleted here.
pub fn retire(amplihack_dir: &Path, path: &Path, label: &str) -> Result<Retired> {
    retire_with(amplihack_dir, path, label, |from, to| fs::rename(from, to))
}

/// [`retire`] with the rename injected.
pub fn retire_with(
    amplihack_dir: &Path,
    path: &Path,
    label: &str,
    rename: impl FnOnce(&Path, &Path) -> io::Result<()>,
) -> Result<Retired> {
    retire_checked(amplihack_dir, path, label, true, rename)
}

/// Retires a scratch directory that the installer itself created outside
/// `amplihack_dir`: the old slash-command trees under `~/.claude` and the
/// Copilot plugin directory (#1954 D4). Every check of [`retire`] applies
/// except the one that confines `path` to `amplihack_dir`, so callers must
/// pass only paths they built themselves, never a path read from disk.
pub fn retire_scratch(amplihack_dir: &Path, path: &Path, label: &str) -> Result<Retired> {
    retire_checked(amplihack_dir, path, label, false, |from, to| {
        fs::rename(from, to)
    })
}

fn retire_checked(
    amplihack_dir: &Path,
    path: &Path,
    label: &str,
    confine: bool,
    rename: impl FnOnce(&Path, &Path) -> io::Result<()>,
) -> Result<Retired> {
    if label.is_empty()
        || label.starts_with('.')
        || !label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        bail!("refusing retire label {label:?}");
    }
    let no_dots = !path
        .components()
        .any(|c| matches!(c, Component::ParentDir | Component::CurDir));
    let lexically_inside = path.starts_with(amplihack_dir) && path != amplihack_dir && no_dots;
    if !no_dots || (confine && !lexically_inside) {
        bail!(
            "refusing to retire {}: it is not inside {}",
            path.display(),
            amplihack_dir.display()
        );
    }
    let meta = fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect {}", path.display()))?;
    if meta.file_type().is_symlink() || !(meta.is_dir() || meta.is_file()) {
        bail!(
            "refusing to retire {}: it is not a real file or directory",
            path.display()
        );
    }
    if !owned_by_euid(&meta) {
        bail!(
            "refusing to retire {}: it is not owned by this user",
            path.display()
        );
    }
    let releases = releases_dir(amplihack_dir);
    ensure_private_dir(&releases)?;
    let retired = releases.join(RETIRED_DIR);
    ensure_private_dir(&retired)?;

    let base = format!("{}-{}-{label}", utc_stamp(), std::process::id());
    let mut destination = retired.join(&base);
    let mut n = 1;
    while fs::symlink_metadata(&destination).is_ok() {
        destination = retired.join(format!("{base}-{n}"));
        n += 1;
    }
    match rename(path, &destination) {
        Ok(()) => Ok(Retired::Moved(destination)),
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
            tracing::warn!(
                "{} is on another filesystem than {}; left in place",
                path.display(),
                retired.display()
            );
            Ok(Retired::LeftInPlace(path.to_path_buf()))
        }
        Err(e) => Err(e).with_context(|| {
            format!(
                "failed to retire {} to {}",
                path.display(),
                destination.display()
            )
        }),
    }
}

/// Retires `path` when it is a real file or directory; removes it when it is
/// a symlink (removing a link deletes nothing a process holds). Missing is
/// fine.
pub fn retire_leftover(amplihack_dir: &Path, path: &Path, label: &str) -> Result<()> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("failed to inspect {}", path.display())),
        Ok(meta) if meta.file_type().is_symlink() => fs::remove_file(path)
            .with_context(|| format!("failed to remove stale link {}", path.display())),
        Ok(_) => {
            report_retired(retire(amplihack_dir, path, label)?);
            Ok(())
        }
    }
}

// ------------------------------------------------------- keep set (D7)

/// The release roots that must stay: the one each link names now, and the
/// previous target of each link. Paths outside `releases` are ignored.
pub fn keep_set(releases: &Path, links: &[PathBuf], previous: &[PathBuf]) -> Vec<PathBuf> {
    let mut keep: Vec<PathBuf> = Vec::new();
    for path in links.iter().chain(previous) {
        if let Some(root) = release_root_of(releases, path)
            && !keep.contains(&root)
        {
            keep.push(root);
        }
    }
    keep
}

// ------------------------------------------------- in-use scan (D7, S22)

/// Which candidates running processes use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InUse {
    /// The scan completed: each used candidate with the pids using it.
    #[cfg(any(target_os = "linux", test))]
    Known(BTreeMap<PathBuf, Vec<u32>>),
    /// A same-user process could not be inspected; nothing may be removed.
    #[cfg(any(target_os = "linux", test))]
    Unknown,
    /// No process scan on this platform; nothing may be removed.
    #[cfg_attr(target_os = "linux", allow(dead_code))]
    Unsupported,
}

/// Scans every process on the host.
pub fn in_use_paths(candidates: &[PathBuf]) -> InUse {
    #[cfg(target_os = "linux")]
    {
        let entries = match fs::read_dir("/proc") {
            Ok(entries) => entries,
            Err(e) => {
                tracing::warn!("release cleanup: cannot list /proc: {e}");
                return InUse::Unknown;
            }
        };
        let pids: Vec<u32> = entries
            .flatten()
            .filter_map(|e| e.file_name().to_str().and_then(|n| n.parse().ok()))
            .collect();
        scan_pids(candidates, &pids)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = candidates;
        InUse::Unsupported
    }
}

/// Scans `pids` only. Processes of other users are skipped; an exited pid is
/// not using anything.
///
/// A same-user process whose `/proc` entries cannot be read (it is
/// non-dumpable, such as `ssh-agent`, the systemd user manager, or a setgid
/// or file-capability program like `ping`) cannot be inspected, but its
/// `stat` can. Its start (fork) time decides what it may use:
///
/// - A release that existed when it started may hold its working directory,
///   root, open files, maps or environment, inherited from its parent. That
///   release is kept. Holding privileges does not change this: a privileged
///   executable is never a release file, but its `cwd` and inherited file
///   descriptors can still be inside one.
/// - A release created after it started was not inherited. The process has
///   not exec'd a release binary since either, because that exec would have
///   made it dumpable again. That release can be removed.
///
/// The one exception is the systemd user manager and its `(sd-pam)` helper,
/// recognised by their cgroup (see [`proc_scan::is_user_manager_cgroup`]).
/// They run from `/` and the system's own binaries, never from a release, so
/// they keep nothing. Without this, every release on disk when the manager
/// started (at boot with lingering on) would be kept for good.
///
/// When the start time cannot be read, the result is [`InUse::Unknown`].
#[cfg(target_os = "linux")]
pub fn scan_pids(candidates: &[PathBuf], pids: &[u32]) -> InUse {
    #[cfg(target_os = "linux")]
    {
        let forms: Vec<(PathBuf, Vec<PathBuf>)> = candidates
            .iter()
            .map(|c| {
                let mut forms = vec![c.clone()];
                if let Ok(canonical) = fs::canonicalize(c)
                    && canonical != *c
                {
                    forms.push(canonical);
                }
                (c.clone(), forms)
            })
            .collect();
        let created: Vec<Option<std::time::SystemTime>> =
            candidates.iter().map(|c| birth_time(c)).collect();
        scan_with(candidates, &created, pids, |pid| {
            proc_scan::scan_pid(pid, &forms).map_err(|proc_scan::Unreadable { entry, error }| {
                tracing::debug!(
                    "release cleanup: pid {pid} /proc/{pid}/{entry} is unreadable ({error})"
                );
                proc_scan::classify_opaque(pid)
            })
        })
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (candidates, pids);
        InUse::Unsupported
    }
}

/// When `path` came into being, from its birth time (`statx` `STATX_BTIME`)
/// only, or `None` when the filesystem does not record one (NFS, some FUSE
/// or ecryptfs mounts).
///
/// There is deliberately no fallback to the modification time. A write into
/// a release after it was committed moves its mtime later than its birth;
/// [`may_predate`] would then read an old release as created after an
/// opaque process started and remove it from under that process (#1954).
/// `None` makes every opaque process keep the release instead.
#[cfg(target_os = "linux")]
pub(crate) fn birth_time(path: &Path) -> Option<std::time::SystemTime> {
    fs::symlink_metadata(path).ok()?.created().ok()
}

/// The decision part of [`scan_pids`], with the process lookup injected so
/// tests can drive it with fixed start and creation times.
///
/// `created[i]` is when `candidates[i]` came into being. `look(pid)` returns
/// the indexes of the candidates `pid` references, or, for a process that
/// cannot be inspected, what its `status` and `stat` tell.
#[cfg(target_os = "linux")]
pub(crate) fn scan_with(
    candidates: &[PathBuf],
    created: &[Option<std::time::SystemTime>],
    pids: &[u32],
    mut look: impl FnMut(u32) -> Result<Vec<usize>, proc_scan::Opaque>,
) -> InUse {
    let mut used: BTreeMap<PathBuf, Vec<u32>> = BTreeMap::new();
    for &pid in pids {
        match look(pid) {
            Ok(hits) => {
                for index in hits {
                    used.entry(candidates[index].clone()).or_default().push(pid);
                }
            }
            Err(proc_scan::Opaque::Gone) => {}
            Err(proc_scan::Opaque::UserManager) => {
                tracing::debug!(
                    "release cleanup: pid {pid} is the systemd user manager; it uses no release"
                );
            }
            Err(proc_scan::Opaque::StartedAt(started)) => {
                tracing::debug!(
                    "release cleanup: pid {pid} cannot be inspected; keeping releases that \
                     existed when it started"
                );
                for (index, created) in created.iter().enumerate() {
                    if may_predate(*created, started) {
                        used.entry(candidates[index].clone()).or_default().push(pid);
                    }
                }
            }
            Err(proc_scan::Opaque::Unknown) => {
                tracing::info!(
                    "release cleanup: pid {pid} cannot be inspected and its start time is \
                     unknown; keeping every release"
                );
                return InUse::Unknown;
            }
        }
    }
    InUse::Known(used)
}

/// Clock slack for [`may_predate`]: the kernel's boot time is whole seconds.
///
/// It covers rounding only, not a wall-clock step. The process start time is
/// `btime` plus ticks since boot, and `btime` is recomputed from the current
/// wall clock on every read; the release side is a wall-clock birth time. A
/// backward step of more than this slack plus the gap between creating a
/// release and starting the process makes that process read as older than
/// the release, and the release can be removed from under it. A forward step
/// errs toward keeping. This limit is documented under "Known limits" in
/// `docs/concepts/versioned-install-layout.md`.
#[cfg(target_os = "linux")]
const START_TIME_SLACK: std::time::Duration = std::time::Duration::from_secs(2);

/// Whether a candidate created at `created` might already have existed when
/// a process started at `started`, so the process may have inherited a path
/// into it. An unknown creation time might.
#[cfg(target_os = "linux")]
pub(crate) fn may_predate(
    created: Option<std::time::SystemTime>,
    started: std::time::SystemTime,
) -> bool {
    match created {
        Some(created) => created <= started + START_TIME_SLACK,
        None => true,
    }
}

#[cfg(target_os = "linux")]
pub(crate) mod proc_scan {
    use std::ffi::OsStr;
    use std::fs;
    use std::io;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    use std::path::{Path, PathBuf};

    /// A `/proc` entry of a same-user process that could not be read. Only
    /// the entry name and the error are kept: never what the entry held.
    pub(super) struct Unreadable {
        pub(super) entry: &'static str,
        pub(super) error: io::Error,
    }

    /// Indexes of the candidates `pid` references. `Ok(vec![])` for a pid that
    /// exited or belongs to another user.
    pub(super) fn scan_pid(
        pid: u32,
        forms: &[(PathBuf, Vec<PathBuf>)],
    ) -> Result<Vec<usize>, Unreadable> {
        let dir = PathBuf::from(format!("/proc/{pid}"));
        let meta = match fs::metadata(&dir) {
            Ok(meta) => meta,
            Err(e) if gone(&e) => return Ok(Vec::new()),
            Err(error) => return Err(Unreadable { entry: "", error }),
        };
        if meta.uid() != super::euid() {
            return Ok(Vec::new());
        }
        let mut hits = vec![false; forms.len()];
        let mark = |path: &Path, hits: &mut Vec<bool>| {
            for (index, (_, candidate_forms)) in forms.iter().enumerate() {
                if candidate_forms.iter().any(|f| path.starts_with(f)) {
                    hits[index] = true;
                }
            }
        };

        for entry in ["exe", "cwd", "root"] {
            match fs::read_link(dir.join(entry)) {
                Ok(target) => mark(&strip_deleted(&target), &mut hits),
                Err(e) if gone(&e) => {}
                Err(error) => return Err(Unreadable { entry, error }),
            }
        }

        match fs::read_dir(dir.join("fd")) {
            Ok(fds) => {
                for fd in fds {
                    let fd = match fd {
                        Ok(fd) => fd,
                        Err(e) if gone(&e) => continue,
                        Err(error) => return Err(Unreadable { entry: "fd", error }),
                    };
                    match fs::read_link(fd.path()) {
                        Ok(target) => mark(&strip_deleted(&target), &mut hits),
                        Err(e) if gone(&e) => {}
                        Err(error) => return Err(Unreadable { entry: "fd", error }),
                    }
                }
            }
            Err(e) if gone(&e) => {}
            Err(error) => return Err(Unreadable { entry: "fd", error }),
        }

        match fs::read(dir.join("maps")) {
            Ok(maps) => {
                for line in maps.split(|b| *b == b'\n') {
                    if let Some(start) = line.iter().position(|b| *b == b'/') {
                        let path = Path::new(OsStr::from_bytes(&line[start..]));
                        mark(&strip_deleted(path), &mut hits);
                    }
                }
            }
            Err(e) if gone(&e) => {}
            Err(error) => {
                return Err(Unreadable {
                    entry: "maps",
                    error,
                });
            }
        }

        match fs::read(dir.join("environ")) {
            Ok(environ) => {
                // Reduced to one boolean per candidate on the spot (S22): no
                // variable name or value leaves this block.
                for (index, (_, candidate_forms)) in forms.iter().enumerate() {
                    if !hits[index]
                        && candidate_forms
                            .iter()
                            .any(|f| environ_mentions(&environ, f.as_os_str().as_bytes()))
                    {
                        hits[index] = true;
                    }
                }
            }
            Err(e) if gone(&e) => {}
            Err(error) => {
                return Err(Unreadable {
                    entry: "environ",
                    error,
                });
            }
        }

        Ok(hits
            .iter()
            .enumerate()
            .filter_map(|(i, hit)| hit.then_some(i))
            .collect())
    }

    /// What `/proc/<pid>/stat` tells about a process whose other entries
    /// could not be read.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Opaque {
        /// It exited meanwhile.
        Gone,
        /// It is the systemd user manager or its `(sd-pam)` helper, which use
        /// no release.
        UserManager,
        /// It started at this time.
        StartedAt(std::time::SystemTime),
        /// Its start time could not be read.
        Unknown,
    }

    pub(crate) fn classify_opaque(pid: u32) -> Opaque {
        classify_opaque_at(Path::new("/proc"), pid)
    }

    /// Classifies an opaque process using only the supplied table, including
    /// its boot-time file. Tests can exercise the real classifier without
    /// enumerating or reading any host process.
    pub(crate) fn classify_opaque_at(root: &Path, pid: u32) -> Opaque {
        let dir = root.join(pid.to_string());
        // `cgroup` is mode 0444, readable when `cwd` and `fd` are not.
        if let Ok(cgroup) = fs::read_to_string(dir.join("cgroup"))
            && is_user_manager_cgroup(&cgroup, super::euid())
        {
            return Opaque::UserManager;
        }
        let stat = match fs::read_to_string(dir.join("stat")) {
            Ok(stat) => stat,
            Err(e) if gone(&e) => return Opaque::Gone,
            Err(_) => return Opaque::Unknown,
        };
        let boot = fs::read_to_string(root.join("stat"))
            .ok()
            .and_then(|s| boot_time_secs(&s));
        // SAFETY: sysconf has no preconditions; it returns -1 on error, which
        // the check below rejects.
        let ticks_per_sec = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        match (start_ticks(&stat), boot) {
            (Some(ticks), Some(boot)) if ticks_per_sec > 0 => {
                let since_boot =
                    std::time::Duration::from_secs_f64(ticks as f64 / ticks_per_sec as f64);
                Opaque::StartedAt(
                    std::time::UNIX_EPOCH + std::time::Duration::from_secs(boot) + since_boot,
                )
            }
            _ => Opaque::Unknown,
        }
    }

    /// Whether `/proc/<pid>/cgroup` content is exactly the cgroup v2 path of
    /// the systemd user manager of `uid`:
    /// `0::/user.slice/user-<uid>.slice/user@<uid>.service/init.scope`.
    ///
    /// Only the manager and `(sd-pam)` live there; the services and scopes it
    /// starts (recipe runs, tmux, terminals) are in `app.slice`, session
    /// scopes or other units below `user@<uid>.service`, and are inspected as
    /// usual. A nested manager (a container's `systemd`) has a longer path,
    /// and a cgroup v1 or hybrid host lists several lines: neither matches,
    /// so those processes stay under the start-time rule. A same-user process
    /// could move itself into `init.scope` on purpose, but such a process can
    /// delete the releases directly anyway.
    pub(crate) fn is_user_manager_cgroup(cgroup: &str, uid: u32) -> bool {
        let expected = format!("0::/user.slice/user-{uid}.slice/user@{uid}.service/init.scope");
        let mut lines = cgroup.lines().filter(|l| !l.trim().is_empty());
        lines.next().map(str::trim) == Some(expected.as_str()) && lines.next().is_none()
    }

    /// Field 22 of `/proc/<pid>/stat`: start time in clock ticks after boot.
    /// The command name (field 2) may hold spaces and parentheses, so fields
    /// are counted from its last `)`.
    pub(crate) fn start_ticks(stat: &str) -> Option<u64> {
        let rest = &stat[stat.rfind(')')? + 1..];
        rest.split_whitespace().nth(19)?.parse().ok()
    }

    /// The `btime` line of `/proc/stat`: boot time in seconds since the epoch.
    pub(crate) fn boot_time_secs(proc_stat: &str) -> Option<u64> {
        proc_stat
            .lines()
            .find_map(|line| line.strip_prefix("btime "))?
            .trim()
            .parse()
            .ok()
    }

    /// The process exited (or is a zombie with no files) while being read.
    fn gone(e: &io::Error) -> bool {
        e.kind() == io::ErrorKind::NotFound || e.raw_os_error() == Some(libc::ESRCH)
    }

    fn strip_deleted(path: &Path) -> PathBuf {
        let bytes = path.as_os_str().as_bytes();
        let bytes = bytes.strip_suffix(b" (deleted)").unwrap_or(bytes);
        PathBuf::from(OsStr::from_bytes(bytes))
    }

    /// Whether any `NAME=value` in `environ` holds `needle` as a whole path
    /// (followed by `/`, `:` or the end of the value).
    fn environ_mentions(environ: &[u8], needle: &[u8]) -> bool {
        if needle.is_empty() {
            return false;
        }
        environ.split(|b| *b == 0).any(|var| {
            let value = match var.iter().position(|b| *b == b'=') {
                Some(eq) => &var[eq + 1..],
                None => var,
            };
            let mut rest = value;
            while let Some(at) = find(rest, needle) {
                let after = rest.get(at + needle.len()).copied();
                let before_ok = at == 0 || matches!(rest[at - 1], b':' | b'=' | b' ');
                if before_ok && matches!(after, None | Some(b'/') | Some(b':')) {
                    return true;
                }
                rest = &rest[at + 1..];
            }
            false
        })
    }

    fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack.windows(needle.len()).position(|w| w == needle)
    }
}

// ------------------------------------------------------------- cleanup (D7)

/// What [`cleanup_unused`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CleanupReport {
    /// Release roots and retired trees removed.
    pub removed: Vec<PathBuf>,
    /// Candidates kept because these pids use them.
    pub kept_in_use: Vec<(PathBuf, Vec<u32>)>,
    /// The process scan was incomplete, so nothing was removed.
    pub unknown: bool,
    /// This platform has no process scan, so nothing was removed.
    pub unsupported: bool,
    /// Candidates whose removal failed, with the reason.
    pub failed: Vec<(PathBuf, String)>,
}

/// Removes every release not in `keep` and every retired tree, unless a
/// process uses it. Runs under the install lock.
pub fn cleanup_unused(amplihack_dir: &Path, keep: &[PathBuf]) -> Result<CleanupReport> {
    cleanup_unused_with(amplihack_dir, keep, in_use_paths)
}

/// [`cleanup_unused`] with the process scan injected.
pub fn cleanup_unused_with(
    amplihack_dir: &Path,
    keep: &[PathBuf],
    scan: impl FnOnce(&[PathBuf]) -> InUse,
) -> Result<CleanupReport> {
    let releases = releases_dir(amplihack_dir);
    let mut report = CleanupReport::default();
    match fs::symlink_metadata(&releases) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(report),
        Err(e) => {
            return Err(e).with_context(|| format!("failed to inspect {}", releases.display()));
        }
        Ok(meta) => check_private_dir(&releases, &meta)?,
    }
    // Keep entries read back from links or manifests are confined (S8): an
    // outside path neither protects nor exposes anything.
    let keep_names: Vec<std::ffi::OsString> = keep
        .iter()
        .filter_map(|k| release_root_of(&releases, k))
        .filter_map(|root| root.file_name().map(|n| n.to_os_string()))
        .collect();

    let mut candidates = Vec::new();
    for entry in
        fs::read_dir(&releases).with_context(|| format!("failed to read {}", releases.display()))?
    {
        let entry = entry.with_context(|| format!("failed to read {}", releases.display()))?;
        let name = entry.file_name();
        if name == RETIRED_DIR || keep_names.contains(&name) {
            continue;
        }
        let path = releases.join(&name);
        if is_removable_candidate(&path, true) {
            candidates.push(path);
        }
    }
    let retired = releases.join(RETIRED_DIR);
    if let Ok(meta) = fs::symlink_metadata(&retired) {
        check_private_dir(&retired, &meta)?;
        for entry in fs::read_dir(&retired)
            .with_context(|| format!("failed to read {}", retired.display()))?
        {
            let entry = entry.with_context(|| format!("failed to read {}", retired.display()))?;
            let path = retired.join(entry.file_name());
            if is_removable_candidate(&path, false) {
                candidates.push(path);
            }
        }
    }
    if candidates.is_empty() {
        return Ok(report);
    }

    #[cfg(any(target_os = "linux", test))]
    {
        let used = match scan(&candidates) {
            InUse::Known(map) => map,
            InUse::Unknown => {
                report.unknown = true;
                return Ok(report);
            }
            InUse::Unsupported => {
                report.unsupported = true;
                return Ok(report);
            }
        };
        for candidate in candidates {
            if let Some(pids) = used.get(&candidate) {
                report.kept_in_use.push((candidate, pids.clone()));
                continue;
            }
            // Checked again right before deletion (S13).
            let is_dir = candidate.parent() == Some(releases.as_path());
            if !is_removable_candidate(&candidate, is_dir) {
                continue;
            }
            match remove_tree(&candidate) {
                Ok(()) => report.removed.push(candidate),
                Err(e) => report.failed.push((candidate, format!("{e:#}"))),
            }
        }
        Ok(report)
    }
    #[cfg(not(any(target_os = "linux", test)))]
    {
        let InUse::Unsupported = scan(&candidates);
        report.unsupported = true;
        Ok(report)
    }
}

/// A direct child that is a real directory (or, in `.retired/`, a real file)
/// owned by this user. Never a symlink.
fn is_removable_candidate(path: &Path, dir_only: bool) -> bool {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return false;
    };
    let kind = meta.file_type();
    if kind.is_symlink() {
        return false;
    }
    let kind_ok = if dir_only {
        kind.is_dir()
    } else {
        kind.is_dir() || kind.is_file()
    };
    kind_ok && owned_by_euid(&meta)
}

/// Removes a tree this module owns: made writable first (#1955 modes leave
/// directories read-only), then `std::fs::remove_dir_all`, which does not
/// follow symlinks.
pub(crate) fn remove_tree(path: &Path) -> Result<()> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).with_context(|| format!("failed to inspect {}", path.display())),
    };
    if meta.file_type().is_symlink() || meta.is_file() {
        return fs::remove_file(path)
            .with_context(|| format!("failed to remove {}", path.display()));
    }
    make_tree_writable(path)
        .with_context(|| format!("failed to make {} writable", path.display()))?;
    fs::remove_dir_all(path).with_context(|| format!("failed to remove {}", path.display()))
}

/// Gives the owner `rwx` on every directory under `root` that this user owns.
/// Symlinks are never followed and other filesystems are never entered (S11).
pub fn make_tree_writable(root: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let root_meta = fs::symlink_metadata(root)?;
        if !root_meta.is_dir() {
            return Ok(());
        }
        let device = root_meta.dev();
        let mut pending = vec![root.to_path_buf()];
        while let Some(dir) = pending.pop() {
            let meta = fs::symlink_metadata(&dir)?;
            if meta.file_type().is_symlink() || !meta.is_dir() || meta.dev() != device {
                continue;
            }
            if owned_by_euid(&meta) {
                let mode = meta.permissions().mode() & 0o7777;
                if mode & 0o700 != 0o700 {
                    fs::set_permissions(&dir, fs::Permissions::from_mode(mode | 0o700))?;
                }
            }
            for entry in fs::read_dir(&dir)? {
                let entry = entry?;
                if entry.file_type()?.is_dir() {
                    pending.push(entry.path());
                }
            }
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = root;
        Ok(())
    }
}

// -------------------------------------------------------- message text

/// The warning printed when more than five releases are kept for running
/// processes. Exact text: it is documented.
pub fn kept_in_use_warning(count: usize) -> Option<String> {
    (count > KEPT_IN_USE_WARN_ABOVE).then(|| {
        format!(
            "⚠️  {count} amplihack releases kept because running processes use them \
             (~/.amplihack/releases); they are removed by a later install once unused"
        )
    })
}

/// The `uninstall` lines naming the releases it kept. Exact text.
pub fn format_kept_in_use(home: &Path, kept: &[(PathBuf, Vec<u32>)]) -> Option<String> {
    if kept.is_empty() {
        return None;
    }
    let count = kept.len();
    let noun = if count == 1 { "release" } else { "releases" };
    let mut out = format!("ℹ Kept {count} {noun} in use by running processes:\n");
    for (path, pids) in kept {
        let shown = match path.strip_prefix(home) {
            Ok(rest) => format!("~/{}", rest.display()),
            Err(_) => path.display().to_string(),
        };
        let pid_word = if pids.len() == 1 { "pid" } else { "pids" };
        let pid_list: Vec<String> = pids.iter().map(u32::to_string).collect();
        out.push_str(&format!(
            "    {shown} ({pid_word} {})\n",
            pid_list.join(", ")
        ));
    }
    let pronoun = if count == 1 { "it" } else { "them" };
    out.push_str(&format!(
        "  Run `amplihack uninstall` again after those processes exit to remove {pronoun}."
    ));
    Some(out)
}

/// One line for the install output when cleanup could not inspect every
/// process, so kept releases are not a mystery.
pub fn unknown_scan_notice() -> &'static str {
    "ℹ Old amplihack releases kept: a running process of this user could not be inspected, \
     so no release was removed (~/.amplihack/releases)"
}

/// One line for the install output on a platform with no process scan, where
/// every old release is kept.
pub fn unsupported_scan_notice() -> &'static str {
    "ℹ Old amplihack releases kept: this platform cannot tell which releases running \
     processes use, so no release was removed (~/.amplihack/releases)"
}

/// Prints what a cleanup did. Removal failures are warnings: the install
/// itself succeeded.
pub fn report_cleanup(report: &CleanupReport) {
    if !report.removed.is_empty() {
        println!(
            "   Removed {} unused release(s) or retired tree(s)",
            report.removed.len()
        );
    }
    if report.unknown {
        println!("   {}", unknown_scan_notice());
    }
    if report.unsupported {
        println!("   {}", unsupported_scan_notice());
    }
    if let Some(warning) = kept_in_use_warning(report.kept_in_use.len()) {
        println!("   {warning}");
    }
    for (path, reason) in &report.failed {
        eprintln!(
            "amplihack: warning: could not remove unused {}: {reason}",
            path.display()
        );
    }
}

#[cfg(all(test, unix))]
mod origin_tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn release(home: &Path, name: &str) -> PathBuf {
        let root = home.join(".amplihack/releases").join(name);
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::write(root.join(COMPLETE_MARKER), b"complete\n").unwrap();
        let exe = root.join("bin/amplihack");
        fs::write(&exe, b"private fixture executable\n").unwrap();
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
        exe
    }

    /// The refusal a fixture must produce, with its cause.
    fn refusal(home: &Path, executable: &Path) -> OriginRefusal {
        installed_executable(home, executable).expect_err("this fixture must be refused")
    }

    #[test]
    fn canonical_installed_origin_accepts_published_links_but_refuses_escapes() {
        let temp = crate::test_support::tempdir_outside_framework_roots();
        let home = temp.path();
        let executable = release(home, "fixture");
        let link = home.join("published");
        symlink(&executable, &link).unwrap();
        assert!(installed_executable(home, &executable).is_ok());
        assert!(installed_executable(home, &link).is_ok());

        let outside = home.join("checkout");
        fs::write(&outside, b"checkout\n").unwrap();
        fs::remove_file(&executable).unwrap();
        symlink(&outside, &executable).unwrap();
        assert_eq!(
            refusal(home, &executable).check(),
            "executable-outside-installed-release"
        );
        assert_eq!(
            refusal(home, &home.join("missing")).check(),
            "executable-path-unresolved"
        );
        assert_eq!(fs::read(&outside).unwrap(), b"checkout\n");
    }

    /// Every cause named in the "Diagnosing a refusal" table of
    /// `docs/features/self-heal-asset-restage.md` must be reported
    /// distinguishably, so that one `RUST_LOG` run tells a user which check
    /// failed and on which path. Ownership causes need a second account and
    /// are exercised only by `check_private_dir`'s own path.
    #[test]
    fn each_documented_refusal_cause_is_reported_distinguishably() {
        // (fixture mutation, expected check slug, expected subject)
        #[derive(Clone, Copy)]
        enum Subject {
            Framework,
            Releases,
            Root,
            Bin,
            Executable,
        }

        for (case, expected_check, expected_subject) in [
            (
                "checkout",
                "executable-outside-installed-release",
                Subject::Executable,
            ),
            ("missing", "executable-path-unresolved", Subject::Executable),
            (
                "framework-symlink",
                "directory-is-a-symlink",
                Subject::Framework,
            ),
            (
                "releases-symlink",
                "directory-is-a-symlink",
                Subject::Releases,
            ),
            (
                "framework-writable",
                "directory-writable-by-group-or-others",
                Subject::Framework,
            ),
            (
                "releases-writable",
                "directory-writable-by-group-or-others",
                Subject::Releases,
            ),
            (
                "root-writable",
                "directory-writable-by-group-or-others",
                Subject::Root,
            ),
            (
                "bin-writable",
                "directory-writable-by-group-or-others",
                Subject::Bin,
            ),
            ("incomplete", "release-incomplete", Subject::Root),
            ("marker-link", "release-incomplete", Subject::Root),
            ("retired", "release-retired", Subject::Executable),
            // A symlinked release root or `bin` resolves outside the trusted
            // tree, so confinement rejects it before any directory check.
            (
                "root-symlink",
                "executable-outside-installed-release",
                Subject::Executable,
            ),
            (
                "bin-symlink",
                "executable-outside-installed-release",
                Subject::Executable,
            ),
            (
                "sibling-link",
                "executable-not-in-release-bin",
                Subject::Executable,
            ),
            (
                "directory",
                "executable-not-a-regular-file",
                Subject::Executable,
            ),
            (
                "writable-binary",
                "executable-writable-by-group-or-others",
                Subject::Executable,
            ),
        ] {
            let temp = crate::test_support::tempdir_outside_framework_roots();
            let home = temp.path();
            let mut executable = release(
                home,
                if case == "retired" {
                    ".retired/old"
                } else {
                    "current"
                },
            );
            let root = executable.parent().unwrap().parent().unwrap().to_path_buf();
            let bin = root.join("bin");
            let framework = home.join(".amplihack");
            let releases = framework.join(RELEASES_DIR);
            match case {
                "checkout" => {
                    let outside = home.join("target/debug");
                    fs::create_dir_all(&outside).unwrap();
                    executable = outside.join("amplihack");
                    fs::write(&executable, b"cargo build\n").unwrap();
                }
                "missing" => executable = home.join("no/such/amplihack"),
                "framework-symlink" => {
                    let moved = home.join("elsewhere-amplihack");
                    fs::rename(&framework, &moved).unwrap();
                    symlink(&moved, &framework).unwrap();
                }
                "releases-symlink" => {
                    let moved = home.join("elsewhere-releases");
                    fs::rename(&releases, &moved).unwrap();
                    symlink(&moved, &releases).unwrap();
                }
                "framework-writable" => {
                    fs::set_permissions(&framework, fs::Permissions::from_mode(0o777)).unwrap();
                }
                "releases-writable" => {
                    fs::set_permissions(&releases, fs::Permissions::from_mode(0o775)).unwrap();
                }
                "root-writable" => {
                    fs::set_permissions(&root, fs::Permissions::from_mode(0o707)).unwrap();
                }
                "bin-writable" => {
                    fs::set_permissions(&bin, fs::Permissions::from_mode(0o770)).unwrap();
                }
                "incomplete" => fs::remove_file(root.join(COMPLETE_MARKER)).unwrap(),
                "marker-link" => {
                    fs::remove_file(root.join(COMPLETE_MARKER)).unwrap();
                    symlink(&executable, root.join(COMPLETE_MARKER)).unwrap();
                }
                "retired" => {}
                "root-symlink" => {
                    let moved = home.join("elsewhere-release");
                    fs::rename(&root, &moved).unwrap();
                    symlink(&moved, &root).unwrap();
                }
                "bin-symlink" => {
                    let moved = home.join("elsewhere-bin");
                    fs::rename(&bin, &moved).unwrap();
                    symlink(&moved, &bin).unwrap();
                }
                "sibling-link" => {
                    // Inside the release, but not inside its `bin`.
                    let sibling = root.join("amplihack");
                    fs::write(&sibling, b"staged beside bin\n").unwrap();
                    fs::set_permissions(&sibling, fs::Permissions::from_mode(0o755)).unwrap();
                    fs::remove_file(&executable).unwrap();
                    symlink(&sibling, &executable).unwrap();
                }
                "directory" => {
                    fs::remove_file(&executable).unwrap();
                    fs::create_dir(&executable).unwrap();
                }
                "writable-binary" => {
                    fs::set_permissions(&executable, fs::Permissions::from_mode(0o777)).unwrap();
                }
                _ => unreachable!(),
            }

            let refusal = refusal(home, &executable);
            assert_eq!(refusal.check(), expected_check, "{case}");
            let want = match expected_subject {
                Subject::Framework => framework.clone(),
                Subject::Releases => releases.clone(),
                Subject::Root => root.clone(),
                Subject::Bin => bin.clone(),
                Subject::Executable => executable.clone(),
            };
            let actual = refusal
                .subject()
                .unwrap_or_else(|| panic!("{case}: the cause must name the path it checked"));
            assert_eq!(
                fs::canonicalize(actual).unwrap_or_else(|_| actual.to_path_buf()),
                fs::canonicalize(&want).unwrap_or(want),
                "{case}: cause names the wrong path"
            );
            // Restore modes the fixture loosened, so the tempdir can be
            // removed without surprising the next case.
            for dir in [&framework, &releases, &root, &bin] {
                let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
            }
        }
    }

    /// The cause is observable without a second run: one `debug` record per
    /// assessment, naming the check and a sanitised path.
    #[test]
    fn the_refusal_cause_is_logged_at_debug_with_a_sanitised_path() {
        let temp = crate::test_support::tempdir_outside_framework_roots();
        let home = temp.path();
        // A release name carrying a control character: whatever reaches the
        // log must have been through `sanitize_path`, as the refusal line is.
        let executable = release(home, "fix\u{1}ture");
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o777)).unwrap();

        let (result, logged) =
            captured_at_debug(|| assess_implicit_install_origin(home, &executable));

        let refusal = result.expect_err("a group-writable binary is refused");
        assert_eq!(refusal.check(), "executable-writable-by-group-or-others");
        let lines: Vec<_> = logged
            .lines()
            .filter(|line| line.contains("executable-writable-by-group-or-others"))
            .collect();
        assert_eq!(lines.len(), 1, "exactly one cause record: {logged:?}");
        assert!(
            !lines[0].chars().any(char::is_control),
            "the cause must be sanitised: {lines:?}"
        );
        assert!(
            lines[0].contains("fix?ture"),
            "the cause names the sanitised path: {lines:?}"
        );
        assert!(
            !lines[0].contains('\u{1}'),
            "a raw control character reached the log: {lines:?}"
        );
    }

    #[test]
    fn an_eligible_release_logs_no_refusal_cause() {
        let temp = crate::test_support::tempdir_outside_framework_roots();
        let home = temp.path();
        let executable = release(home, "fixture");

        let (result, logged) =
            captured_at_debug(|| assess_implicit_install_origin(home, &executable));

        result.expect("a complete private release is eligible");
        assert!(
            logged.is_empty(),
            "eligibility is not a diagnostic: {logged:?}"
        );
    }

    /// Run `f` under a `debug`-level subscriber scoped to this crate and
    /// return what it recorded.
    fn captured_at_debug<T>(f: impl FnOnce() -> T) -> (T, String) {
        use std::sync::{Arc, Mutex};
        use tracing_subscriber::fmt::MakeWriter;

        #[derive(Clone, Default)]
        struct Buffer(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Buffer {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        impl<'a> MakeWriter<'a> for Buffer {
            type Writer = Buffer;
            fn make_writer(&'a self) -> Self::Writer {
                self.clone()
            }
        }

        let buffer = Buffer::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(buffer.clone())
            .with_ansi(false)
            .with_max_level(tracing::Level::DEBUG)
            .finish();
        let out = tracing::subscriber::with_default(subscriber, f);
        let logged = String::from_utf8_lossy(&buffer.0.lock().unwrap()).into_owned();
        (out, logged)
    }
}

/// Issue #2496: the confinement checks that make an origin trustworthy are
/// Unix file-mode and ownership checks. Where they do not exist, the oracle
/// refuses outright rather than accepting a tree it cannot assess. Runs on
/// Windows CI only.
#[cfg(all(test, not(unix)))]
mod origin_tests_other_platforms {
    use super::*;

    #[test]
    fn implicit_installation_is_refused_where_confinement_cannot_be_assessed() {
        let temp = crate::test_support::tempdir_outside_framework_roots();
        let home = temp.path();
        let root = home.join(".amplihack/releases/current");
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::write(root.join(COMPLETE_MARKER), b"complete\n").unwrap();
        let executable = root.join("bin/amplihack.exe");
        fs::write(&executable, b"fixture executable\n").unwrap();

        let refusal = installed_executable(home, &executable)
            .expect_err("an unassessable platform must not be eligible");
        assert_eq!(refusal.check(), "unsupported-platform");
        assert!(!executable_allows_implicit_install());
    }
}
