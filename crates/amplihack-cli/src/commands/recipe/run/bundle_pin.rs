//! Issue #2528: one `amplifier-bundle` per recipe run.
//!
//! A run selects one bundle (the installed one by default, a checkout's only
//! when asked for), checks it with the auto-drive trust checks (#1551,
//! #2340), and copies it into `<work root>/bundle-pins/<key>/amplifier-bundle`.
//! Every step then reads recipes, agents and tools from that snapshot, so an
//! upgrade of the installed bundle during the run does not reach it. A
//! nested run reuses its parent's snapshot.
//!
//! `<key>` is derived from the bundle's contents (see [`store`]), so a
//! relaunch of an unchanged bundle gets the same snapshot path and a resumed
//! run the same runner arguments as the original run (#1741). The run's
//! [`PinnedBundle`] holds a lease on the snapshot; the last run to release it
//! removes the snapshot. This module starts no process, thread or timer.
//!
//! Reference: `docs/reference/recipe-run-bundle.md`.

mod store;

use super::correlation::BundleRecord;
use crate::commands::cleanup::run_dir::read_capped_nofollow;
use crate::commands::orch::autodrive::trusted::{
    LAUNCH_SOURCE_FILE, LinkPolicy, check_bundle, copy_bundle, pin_bundle, trusted_directory,
};
use anyhow::{Context, Result, bail};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use store::{PINS_DIR, PinLease, Placement, StoreLock};

/// Names the outer run's snapshot home for nested runs.
pub(crate) const RUN_BUNDLE_ENV: &str = "AMPLIHACK_RUN_BUNDLE";
/// The choice the outermost run made.
pub(crate) const RUN_BUNDLE_CHOICE_ENV: &str = "AMPLIHACK_RUN_BUNDLE_CHOICE";
/// The bundle folder inside a home.
const BUNDLE_DIR: &str = "amplifier-bundle";
/// Written by the launch that creates a snapshot, next to its bundle, so a
/// nested run can record the same source and version.
const PIN_RECORD: &str = "pin.json";
/// At most this many bytes of a version or record file are read.
const MAX_READ: u64 = 64 * 1024;

/// Which bundle a run uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BundleChoice {
    Installed,
    Checkout,
}

impl BundleChoice {
    /// Parses `installed` or `checkout`; anything else is an error (E2).
    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value {
            "installed" => Ok(Self::Installed),
            "checkout" => Ok(Self::Checkout),
            other => bail!("-c bundle={other} is not valid; use installed or checkout"),
        }
    }

    /// Merges the `--bundle` flag and the `-c bundle=` key (E1, E2).
    /// Returns `None` when neither is given.
    pub(crate) fn from_inputs(flag: Option<&str>, key: Option<&str>) -> Result<Option<Self>> {
        let key_choice = key.map(Self::parse).transpose()?;
        let flag_choice = flag
            .map(|value| {
                Self::parse(value).map_err(|_| {
                    anyhow::anyhow!("--bundle {value} is not valid; use installed or checkout")
                })
            })
            .transpose()?;
        match (flag_choice, key_choice) {
            (Some(flag), Some(key)) if flag != key => bail!(
                "--bundle {} conflicts with -c bundle={}; give one value",
                flag.as_str(),
                key.as_str()
            ),
            (Some(choice), _) | (None, Some(choice)) => Ok(Some(choice)),
            (None, None) => Ok(None),
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Installed => "installed",
            Self::Checkout => "checkout",
        }
    }
}

/// Everything selection reads, passed in so [`select_from`] reads nothing
/// from the process environment.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SelectInputs<'a> {
    /// The merged `--bundle` / `-c bundle=` choice.
    pub(crate) choice: Option<BundleChoice>,
    /// The run's working directory (absolute).
    pub(crate) working_dir: &'a Path,
    /// `AMPLIHACK_HOME` from the environment.
    pub(crate) amplihack_home: Option<&'a OsStr>,
    /// `paths.home` from the user configuration.
    pub(crate) configured_home: Option<&'a Path>,
    /// `HOME` from the environment.
    pub(crate) user_home: Option<&'a OsStr>,
    /// `AMPLIHACK_RUN_BUNDLE` from the environment.
    pub(crate) run_bundle: Option<&'a OsStr>,
    /// `AMPLIHACK_RUN_BUNDLE_CHOICE` from the environment.
    pub(crate) run_bundle_choice: Option<&'a OsStr>,
    /// This run's work root, the folder that holds `runs/` and
    /// `bundle-pins/`. A marker is accepted only under its `bundle-pins`;
    /// with no work root, every marker is ignored.
    pub(crate) work_root: Option<&'a Path>,
}

/// An outer run's snapshot that a nested run reuses.
#[derive(Debug, Clone)]
struct Inherited {
    /// Canonical `<work root>/bundle-pins/<key>`.
    home: PathBuf,
    /// The outer run's canonical source bundle, from its pin record.
    source: PathBuf,
}

/// The checked source bundle, or an inherited snapshot.
#[derive(Debug, Clone)]
pub(crate) struct Selection {
    /// Canonical bundle root the run reads from before pinning.
    source: PathBuf,
    recipes: PathBuf,
    choice: BundleChoice,
    version: String,
    inherited: Option<Inherited>,
    notes: Vec<String>,
}

impl Selection {
    /// Canonical `<home>/amplifier-bundle` (for a nested run, the outer
    /// run's snapshot bundle).
    pub(crate) fn source(&self) -> &Path {
        &self.source
    }

    /// `<source>/recipes`.
    pub(crate) fn recipes_dir(&self) -> &Path {
        &self.recipes
    }

    pub(crate) fn choice(&self) -> BundleChoice {
        self.choice
    }

    /// Sanitised version, or `unknown`.
    pub(crate) fn version(&self) -> &str {
        &self.version
    }

    /// True when this run reuses an outer run's snapshot.
    #[cfg(test)]
    pub(crate) fn inherited(&self) -> bool {
        self.inherited.is_some()
    }

    /// Lines for stderr: a skipped checkout or an ignored marker.
    pub(crate) fn notes(&self) -> &[String] {
        &self.notes
    }
}

/// Reads the environment and the user configuration, then calls
/// [`select_from`].
pub(crate) fn select(choice: Option<BundleChoice>, working_dir: &Path) -> Result<Selection> {
    let amplihack_home = std::env::var_os("AMPLIHACK_HOME").map(|value| {
        let path = PathBuf::from(&value);
        if path.is_relative() && !path.as_os_str().is_empty() {
            std::env::current_dir()
                .map(|cwd| cwd.join(&path).into_os_string())
                .unwrap_or(value)
        } else {
            value
        }
    });
    let configured_home = crate::user_config::configured_home();
    let user_home = std::env::var_os("HOME");
    let run_bundle = std::env::var_os(RUN_BUNDLE_ENV);
    let run_bundle_choice = std::env::var_os(RUN_BUNDLE_CHOICE_ENV);
    // The work root is needed only to check a marker; a run that cannot
    // resolve one ignores the marker and says why.
    let mut work_root_note = None;
    let work_root = if run_bundle.as_deref().is_some_and(|value| !value.is_empty()) {
        match crate::commands::cleanup::work_root::resolve_from_env() {
            Ok(root) => Some(root.path),
            Err(error) => {
                work_root_note = Some(format!(
                    "recipe bundle: cannot resolve the work root to check \
                     {RUN_BUNDLE_ENV}: {error:#}"
                ));
                None
            }
        }
    } else {
        None
    };
    let mut selection = select_from(SelectInputs {
        choice,
        working_dir,
        amplihack_home: amplihack_home.as_deref(),
        configured_home: configured_home.as_deref(),
        user_home: user_home.as_deref(),
        run_bundle: run_bundle.as_deref(),
        run_bundle_choice: run_bundle_choice.as_deref(),
        work_root: work_root.as_deref(),
    })?;
    if let Some(note) = work_root_note {
        selection.notes.insert(0, note);
    }
    Ok(selection)
}

/// Pure selection over explicit inputs (D1, D4).
pub(crate) fn select_from(inputs: SelectInputs<'_>) -> Result<Selection> {
    let mut notes = Vec::new();
    let mut inputs = inputs;
    if let Some(marker) = inputs.run_bundle.filter(|value| !value.is_empty()) {
        match check_marker(
            marker,
            inputs.amplihack_home,
            inputs.run_bundle_choice,
            inputs.work_root,
        ) {
            Ok((home, marker_choice)) => {
                if let Some(requested) = inputs.choice
                    && requested != marker_choice
                {
                    bail!(
                        "this run is nested in a run pinned to the {} bundle at {}; \
                         --bundle {} cannot change it",
                        marker_choice.as_str(),
                        home.display(),
                        requested.as_str()
                    );
                }
                return Ok(inherited_selection(home, marker_choice));
            }
            Err(reason) => {
                notes.push(format!(
                    "recipe bundle: ignoring {RUN_BUNDLE_ENV}={}: {reason}; \
                     selecting a bundle for this run",
                    Path::new(marker).display()
                ));
                // An AMPLIHACK_HOME that names the refused marker is another
                // run's snapshot, not an install: it is not a candidate. Two
                // paths that cannot be resolved do not name the same folder.
                let same = |value: &OsStr| {
                    matches!(
                        (fs::canonicalize(value), fs::canonicalize(marker)),
                        (Ok(a), Ok(b)) if a == b
                    )
                };
                if inputs.amplihack_home.is_some_and(same) {
                    notes.push(format!(
                        "recipe bundle: skipping AMPLIHACK_HOME={}: it names the ignored \
                         {RUN_BUNDLE_ENV}",
                        Path::new(marker).display()
                    ));
                    inputs.amplihack_home = None;
                }
            }
        }
    }
    let choice = inputs.choice.unwrap_or(BundleChoice::Installed);
    let home = match choice {
        BundleChoice::Installed => installed_home(&inputs, &mut notes)?,
        BundleChoice::Checkout => checkout_home(inputs.working_dir)?,
    };
    let source = pin_bundle(&home)?;
    let version = installed_by_version(&source)
        .or_else(|| {
            (choice == BundleChoice::Checkout)
                .then(|| checkout_version(&home))
                .flatten()
        })
        .unwrap_or_else(|| "unknown".to_string());
    Ok(Selection {
        recipes: source.join("recipes"),
        source,
        choice,
        version,
        inherited: None,
        notes,
    })
}

/// The marker checks of a nested run (S7). Returns the canonical marker and
/// the outer run's choice, or the reason the marker is ignored.
fn check_marker(
    marker: &OsStr,
    amplihack_home: Option<&OsStr>,
    marker_choice: Option<&OsStr>,
    work_root: Option<&Path>,
) -> std::result::Result<(PathBuf, BundleChoice), String> {
    let canonical = fs::canonicalize(marker).map_err(|_| "it is not an existing directory")?;
    if !canonical.is_dir() {
        return Err("it is not an existing directory".into());
    }
    let home = amplihack_home
        .filter(|value| !value.is_empty())
        .and_then(|value| fs::canonicalize(value).ok());
    let same_home = home.as_deref() == Some(canonical.as_path());
    if !same_home && !home.is_some_and(|home| is_launch_copy_of(&home, &canonical)) {
        return Err(
            "it is neither AMPLIHACK_HOME nor the bundle AMPLIHACK_HOME's auto-drive copy \
             was made from"
                .into(),
        );
    }
    let Some(work_root) = work_root else {
        return Err("this run has no work root to hold bundle snapshots".into());
    };
    if !is_snapshot_path(&canonical, work_root) {
        return Err(format!(
            "it is not a bundle snapshot in {}/{PINS_DIR}/<24 hex digits>",
            work_root.display()
        ));
    }
    let pins = canonical
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    for dir in [pins, canonical.clone(), canonical.join(BUNDLE_DIR)] {
        trusted_directory(&dir).map_err(|error| format!("{error:#}"))?;
    }
    let choice = marker_choice
        .and_then(OsStr::to_str)
        .and_then(|value| BundleChoice::parse(value).ok())
        .ok_or_else(|| format!("{RUN_BUNDLE_CHOICE_ENV} is not installed or checkout"))?;
    Ok((canonical, choice))
}

/// True when `home` is an auto-drive invocation copy (`LaunchBundle`) made
/// from the snapshot home `marker`: a trusted folder whose source record
/// names `marker`. Auto-drive steps keep `AMPLIHACK_HOME` at that copy (D7),
/// so their nested runs still reuse the outer run's snapshot.
fn is_launch_copy_of(home: &Path, marker: &Path) -> bool {
    if trusted_directory(home).is_err() {
        return false;
    }
    read_capped_nofollow(&home.join(LAUNCH_SOURCE_FILE), MAX_READ)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .and_then(|text| fs::canonicalize(text.trim_end_matches('\n')).ok())
        .is_some_and(|source| source == marker)
}

/// True only for `<work_root>/bundle-pins/<24 lower-case hex digits>`. The
/// parent is compared as given and, when both exist, after resolving
/// symlinks.
pub(crate) fn is_snapshot_path(path: &Path, work_root: &Path) -> bool {
    let named_as_key = path
        .file_name()
        .and_then(OsStr::to_str)
        .is_some_and(store::is_key);
    let Some(parent) = path.parent().filter(|_| named_as_key) else {
        return false;
    };
    let pins = work_root.join(PINS_DIR);
    parent == pins
        || matches!(
            (fs::canonicalize(parent), fs::canonicalize(&pins)),
            (Ok(a), Ok(b)) if a == b
        )
}

/// The outer run's snapshot as this run's selection. A pin record that
/// cannot be read leaves the source as the snapshot and the version
/// `unknown`, with a note saying why.
fn inherited_selection(home: PathBuf, choice: BundleChoice) -> Selection {
    let bundle = home.join(BUNDLE_DIR);
    let path = home.join(PIN_RECORD);
    let mut notes = Vec::new();
    let record = match read_capped_nofollow(&path, MAX_READ)
        .map_err(|error| format!("{error:#}"))
        .and_then(|bytes| {
            serde_json::from_slice::<BundleRecord>(&bytes).map_err(|error| error.to_string())
        }) {
        Ok(record) => Some(record),
        Err(reason) => {
            notes.push(format!(
                "recipe bundle: cannot read the outer run's pin record {}: {reason}; \
                 recording the snapshot as the source and the version as unknown",
                path.display()
            ));
            None
        }
    };
    let source = record
        .as_ref()
        .map(|r| PathBuf::from(&r.source))
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| bundle.clone());
    let version = record
        .and_then(|r| sanitize_version(&r.version))
        .unwrap_or_else(|| "unknown".to_string());
    Selection {
        recipes: bundle.join("recipes"),
        source: bundle,
        choice,
        version,
        inherited: Some(Inherited { home, source }),
        notes,
    }
}

/// The first installed candidate that holds a bundle and is not a checkout.
fn installed_home(inputs: &SelectInputs<'_>, notes: &mut Vec<String>) -> Result<PathBuf> {
    let candidates: [(&str, Option<PathBuf>); 3] = [
        (
            "AMPLIHACK_HOME",
            inputs
                .amplihack_home
                .filter(|value| !value.is_empty())
                .map(PathBuf::from),
        ),
        ("paths.home", inputs.configured_home.map(Path::to_path_buf)),
        (
            "$HOME/.amplihack",
            inputs
                .user_home
                .filter(|value| !value.is_empty())
                .map(|home| Path::new(home).join(".amplihack")),
        ),
    ];
    let mut searched = Vec::new();
    for (label, candidate) in candidates {
        let Some(candidate) = candidate else {
            continue;
        };
        let Ok(canonical) = fs::canonicalize(&candidate) else {
            searched.push(candidate.display().to_string());
            continue;
        };
        if !canonical.is_dir() {
            searched.push(candidate.display().to_string());
            continue;
        }
        if fs::symlink_metadata(canonical.join(".git")).is_ok() {
            notes.push(format!(
                "recipe bundle: skipping {label}={}: it is a git checkout; \
                 pass --bundle checkout to use its bundle",
                candidate.display()
            ));
            searched.push(format!("{} (git checkout, skipped)", candidate.display()));
            continue;
        }
        if fs::symlink_metadata(canonical.join(BUNDLE_DIR)).is_ok() {
            return Ok(canonical);
        }
        searched.push(candidate.display().to_string());
    }
    let searched = if searched.is_empty() {
        "(no candidate set)".to_string()
    } else {
        searched.join(", ")
    };
    bail!(
        "no installed amplifier-bundle found; searched: {searched}. \
         To use this checkout's bundle, pass --bundle checkout"
    )
}

/// The nearest ancestor of `working_dir` holding both `.git` and the bundle.
/// A `.git` file is never parsed.
fn checkout_home(working_dir: &Path) -> Result<PathBuf> {
    for dir in working_dir.ancestors() {
        if fs::symlink_metadata(dir.join(".git")).is_ok()
            && fs::symlink_metadata(dir.join(BUNDLE_DIR)).is_ok()
        {
            return fs::canonicalize(dir)
                .with_context(|| format!("cannot resolve checkout {}", dir.display()));
        }
    }
    bail!(
        "--bundle checkout: no directory containing both .git and amplifier-bundle/ above {}",
        working_dir.display()
    )
}

/// `version=` from `.installed-by` in the folder that holds the canonical
/// bundle root (the release root for a release link).
fn installed_by_version(source: &Path) -> Option<String> {
    let bytes = read_capped_nofollow(&source.parent()?.join(".installed-by"), MAX_READ).ok()?;
    let text = String::from_utf8(bytes).ok()?;
    text.lines()
        .find_map(|line| line.strip_prefix("version="))
        .and_then(sanitize_version)
}

/// `[workspace.package] version` of `<checkout>/Cargo.toml`, plus `+checkout`.
fn checkout_version(checkout: &Path) -> Option<String> {
    let bytes = read_capped_nofollow(&checkout.join("Cargo.toml"), MAX_READ).ok()?;
    let manifest: toml::Value = toml::from_str(std::str::from_utf8(&bytes).ok()?).ok()?;
    let version = manifest
        .get("workspace")?
        .get("package")?
        .get("version")?
        .as_str()?;
    sanitize_version(version).map(|v| format!("{v}+checkout"))
}

/// `[0-9A-Za-z.+-]{1,64}`, and not a path component such as `..`.
fn sanitize_version(value: &str) -> Option<String> {
    let valid = (1..=64).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'+' | b'-'))
        && !value.contains("..");
    valid.then(|| value.to_string())
}

/// The bundle a run executes with. Owns the run's lease on the snapshot:
/// dropping it releases the lease and, for the last launch using the
/// snapshot, removes it. Keep it alive until the run workspace is finished.
#[derive(Debug)]
pub(crate) struct PinnedBundle {
    home: PathBuf,
    bundle: PathBuf,
    /// `<work root>/bundle-pins`; `None` on a dry run.
    pins: Option<PathBuf>,
    choice: BundleChoice,
    dry_run: bool,
    record: BundleRecord,
    /// The shared lock on `<key>.use`; `None` on a dry run.
    _lease: Option<PinLease>,
}

impl PinnedBundle {
    /// `<work root>/bundle-pins/<key>` (`AMPLIHACK_HOME` for steps). On a
    /// dry run, the source home.
    pub(crate) fn home(&self) -> &Path {
        &self.home
    }

    /// `<work root>/bundle-pins/<key>/amplifier-bundle`, or the source
    /// bundle on a dry run.
    pub(crate) fn bundle(&self) -> &Path {
        &self.bundle
    }

    /// Canonical source bundle path.
    pub(crate) fn source(&self) -> &Path {
        Path::new(&self.record.source)
    }

    /// Log record for `recipe_bundle` and `tool_bundle`.
    pub(crate) fn record(&self) -> &BundleRecord {
        &self.record
    }

    pub(crate) fn choice(&self) -> BundleChoice {
        self.choice
    }

    /// True for a dry run: no snapshot and no bundle variables.
    pub(crate) fn is_dry_run(&self) -> bool {
        self.dry_run
    }

    /// Records the recipe file when the recipe was given as a path (D6).
    pub(crate) fn set_recipe_file(&mut self, file: &Path) {
        self.record.recipe_file = Some(file.display().to_string());
    }

    /// The one stderr line printed at run start (D9).
    pub(crate) fn stderr_line(&self) -> String {
        let inherited = if self.record.inherited {
            " (inherited)"
        } else {
            ""
        };
        format!(
            "recipe bundle: {} ({}){inherited}; tools: same bundle",
            self.source().display(),
            self.record.version
        )
    }
}

/// Pins the selection under `<work_root>/bundle-pins/<key>`, reusing a
/// snapshot with the same contents, or takes a lease on an inherited
/// snapshot. With `dry_run`, walks and checks every entry without copying;
/// `work_root` is then unused.
pub(crate) fn pin(selection: &Selection, work_root: &Path, dry_run: bool) -> Result<PinnedBundle> {
    let record = |source: &Path, snapshot: Option<&Path>, inherited: bool| BundleRecord {
        source: source.display().to_string(),
        version: selection.version().to_string(),
        snapshot: snapshot.map(|p| p.display().to_string()),
        choice: selection.choice.as_str().to_string(),
        inherited,
        recipe_file: None,
    };
    if let Some(inherited) = &selection.inherited {
        let lease = (!dry_run)
            .then(|| PinLease::join(&inherited.home))
            .transpose()?;
        return Ok(PinnedBundle {
            bundle: inherited.home.join(BUNDLE_DIR),
            pins: (!dry_run)
                .then(|| inherited.home.parent().map(Path::to_path_buf))
                .flatten(),
            record: record(
                &inherited.source,
                (!dry_run).then_some(inherited.home.as_path()),
                true,
            ),
            home: inherited.home.clone(),
            choice: selection.choice,
            dry_run,
            _lease: lease,
        });
    }
    if dry_run {
        check_bundle(&selection.source, LinkPolicy::KeepInternal)?;
        return Ok(PinnedBundle {
            home: selection
                .source
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| selection.source.clone()),
            bundle: selection.source.clone(),
            pins: None,
            choice: selection.choice,
            dry_run,
            record: record(&selection.source, None, false),
            _lease: None,
        });
    }

    let pins = work_root.join(PINS_DIR);
    store::ensure_pins_dir(&pins)?;
    let _lock = StoreLock::acquire(&pins, true)?
        .with_context(|| format!("cannot open {}/.lock", pins.display()))?;
    store::clean_stale_copies(&pins);
    let tmp = pins.join(store::new_tmp_name());
    let pinned = pin_locked(
        selection,
        &record(&selection.source, None, false),
        &pins,
        &tmp,
    );
    // The copy is renamed into place on success; anything left is removed.
    if let Err(error) = store::remove_entry(&tmp) {
        eprintln!(
            "recipe bundle: could not remove the copy {}; a later launch removes it: {error:#}",
            tmp.display()
        );
    }
    let (key, home) = pinned?;
    let lease = PinLease::take(&pins, &key)?;
    store::sweep(&pins, &key);
    let record = record(&selection.source, Some(&home), false);
    Ok(PinnedBundle {
        bundle: home.join(BUNDLE_DIR),
        home,
        pins: Some(pins),
        choice: selection.choice,
        dry_run,
        record,
        _lease: Some(lease),
    })
}

/// Steps 4 to 6 of a launch, with `.lock` held: copies and checks the
/// source into `tmp`, writes `pin_record` beside it, derives the key and
/// puts the snapshot in place. Returns the key and `<pins>/<key>`.
fn pin_locked(
    selection: &Selection,
    pin_record: &BundleRecord,
    pins: &Path,
    tmp: &Path,
) -> Result<(String, PathBuf)> {
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        fs::DirBuilder::new()
            .mode(0o755)
            .create(tmp)
            .with_context(|| format!("cannot create {}", tmp.display()))?;
        fs::set_permissions(tmp, fs::Permissions::from_mode(0o755))
            .with_context(|| format!("cannot set mode 0755 on {}", tmp.display()))?;
    }
    let bundle = tmp.join(BUNDLE_DIR);
    copy_bundle(&selection.source, &bundle, LinkPolicy::KeepInternal)?;
    let digest = store::tree_digest(&bundle)?;
    let key = store::snapshot_key(
        selection.choice.as_str(),
        &selection.source,
        &selection.version,
        &digest,
    );
    write_pin_record(tmp, pin_record)?;
    let home = pins.join(&key);
    match store::place(pins, &key, tmp, &digest, &selection.source)? {
        Placement::Created | Placement::Reused => {}
        Placement::Replaced => eprintln!(
            "recipe bundle: replacing {}: its contents differ from {} ({})",
            home.display(),
            selection.source.display(),
            selection.version
        ),
    }
    Ok((key, home))
}

fn write_pin_record(home: &Path, record: &BundleRecord) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let path = home.join(PIN_RECORD);
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .open(&path)
        .and_then(|mut file| file.write_all(&serde_json::to_vec(record)?))
        .and_then(|()| fs::set_permissions(&path, fs::Permissions::from_mode(0o644)))
        .with_context(|| format!("cannot write {}", path.display()))
}

/// E8: the settled state home must not lie under the run's `runtime/`
/// folder or under `<work root>/bundle-pins/`. Paths are compared after
/// resolving symlinks, so a state home that is a link into either is
/// refused too. A dry run has no snapshot and passes no runtime folder.
pub(crate) fn check_state_home(
    state_home: &Path,
    pinned: &PinnedBundle,
    runtime_dir: Option<&Path>,
) -> Result<()> {
    let state = resolved(state_home);
    if let Some(runtime) = runtime_dir
        && state.starts_with(resolved(runtime))
    {
        bail!(
            "AMPLIHACK_STATE_HOME {} lies under a run's runtime/ folder",
            state_home.display()
        );
    }
    if let Some(pins) = pinned.pins.as_deref()
        && state.starts_with(resolved(pins))
    {
        bail!(
            "AMPLIHACK_STATE_HOME {} lies under the bundle snapshots in {}",
            state_home.display(),
            pins.display()
        );
    }
    Ok(())
}

/// `path` with symlinks resolved. A path that does not exist yet resolves
/// its nearest existing ancestor and keeps the rest as given.
fn resolved(path: &Path) -> PathBuf {
    let mut rest = Vec::new();
    let mut current = path;
    loop {
        if let Ok(base) = fs::canonicalize(current) {
            return rest.iter().rev().fold(base, |acc, part| acc.join(part));
        }
        match (current.parent(), current.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_os_string());
                current = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}
