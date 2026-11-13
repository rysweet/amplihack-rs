//! Issue #2528: one `amplifier-bundle` per recipe run.
//!
//! A run selects one bundle (the installed one by default, a checkout's only
//! when asked for), checks it with the auto-drive trust checks (#1551,
//! #2340), and copies it into `<run dir>/runtime/bundle/amplifier-bundle`.
//! Every step then reads recipes, agents and tools from that snapshot, so an
//! upgrade of the installed bundle during the run does not reach it. A
//! nested run reuses its parent's snapshot.
//!
//! The snapshot lives under `runtime/`, which the run's `RuntimeGuard`
//! removes on every exit path. This module starts no process, thread or
//! timer.
//!
//! Reference: `docs/reference/recipe-run-bundle.md`.

use super::correlation::BundleRecord;
use crate::commands::cleanup::run_dir::read_capped_nofollow;
use crate::commands::orch::autodrive::trusted::{
    LAUNCH_SOURCE_FILE, LinkPolicy, check_bundle, copy_bundle, pin_bundle, trusted_directory,
};
use anyhow::{Context, Result, bail};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

/// Names the outer run's snapshot home for nested runs.
pub(crate) const RUN_BUNDLE_ENV: &str = "AMPLIHACK_RUN_BUNDLE";
/// The choice the outermost run made.
pub(crate) const RUN_BUNDLE_CHOICE_ENV: &str = "AMPLIHACK_RUN_BUNDLE_CHOICE";
/// The snapshot folder inside a run's `runtime/` folder.
const SNAPSHOT_DIR: &str = "bundle";
/// The bundle folder inside a home.
const BUNDLE_DIR: &str = "amplifier-bundle";
/// Written by the outermost run next to its snapshot so a nested run can
/// record the same source and version.
const PIN_RECORD: &str = ".bundle-record.json";
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
}

/// An outer run's snapshot that a nested run reuses.
#[derive(Debug, Clone)]
struct Inherited {
    /// Canonical `<outer run>/runtime/bundle`.
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
    select_from(SelectInputs {
        choice,
        working_dir,
        amplihack_home: amplihack_home.as_deref(),
        configured_home: configured_home.as_deref(),
        user_home: user_home.as_deref(),
        run_bundle: run_bundle.as_deref(),
        run_bundle_choice: run_bundle_choice.as_deref(),
    })
}

/// Pure selection over explicit inputs (D1, D4).
pub(crate) fn select_from(inputs: SelectInputs<'_>) -> Result<Selection> {
    let mut notes = Vec::new();
    let mut inputs = inputs;
    if let Some(marker) = inputs.run_bundle.filter(|value| !value.is_empty()) {
        match check_marker(marker, inputs.amplihack_home, inputs.run_bundle_choice) {
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
                // run's snapshot, not an install: it is not a candidate.
                let same =
                    |value: &OsStr| fs::canonicalize(value).ok() == fs::canonicalize(marker).ok();
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
    if !is_snapshot_path(&canonical) {
        return Err("it is not a bundle folder inside a run's runtime folder".into());
    }
    for dir in [canonical.clone(), canonical.join(BUNDLE_DIR)] {
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

/// True for `<runtime>/bundle`, where `<runtime>` has one of the two
/// layouts of `run_dir::runtime_dir_for`: `.../runtime` or
/// `.../r/<12 hex digits>`.
fn is_snapshot_path(path: &Path) -> bool {
    fn name(path: Option<&Path>) -> Option<&str> {
        path.and_then(Path::file_name).and_then(OsStr::to_str)
    }
    if name(Some(path)) != Some(SNAPSHOT_DIR) {
        return false;
    }
    let runtime = path.parent();
    match name(runtime) {
        Some("runtime") => true,
        Some(short) => {
            short.len() == 12
                && short.bytes().all(|b| b.is_ascii_hexdigit())
                && name(runtime.and_then(Path::parent)) == Some("r")
        }
        None => false,
    }
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

/// The bundle a run executes with.
#[derive(Debug)]
pub(crate) struct PinnedBundle {
    home: PathBuf,
    bundle: PathBuf,
    /// The `runtime/` folder the snapshot lives in; `None` on a dry run.
    runtime_dir: Option<PathBuf>,
    choice: BundleChoice,
    dry_run: bool,
    record: BundleRecord,
}

impl PinnedBundle {
    /// `<run dir>/runtime/bundle` (`AMPLIHACK_HOME` for steps). On a dry
    /// run, the source home.
    pub(crate) fn home(&self) -> &Path {
        &self.home
    }

    /// `<run dir>/runtime/bundle/amplifier-bundle`, or the source bundle on
    /// a dry run.
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

/// Copies the selection into `<runtime_dir>/bundle`, or reuses an inherited
/// snapshot. With `dry_run`, walks and checks every entry without copying.
pub(crate) fn pin(
    selection: &Selection,
    runtime_dir: &Path,
    dry_run: bool,
) -> Result<PinnedBundle> {
    let record = |source: &Path, snapshot: Option<&Path>, inherited: bool| BundleRecord {
        source: source.display().to_string(),
        version: selection.version().to_string(),
        snapshot: snapshot.map(|p| p.display().to_string()),
        choice: selection.choice.as_str().to_string(),
        inherited,
        recipe_file: None,
    };
    if let Some(inherited) = &selection.inherited {
        return Ok(PinnedBundle {
            bundle: inherited.home.join(BUNDLE_DIR),
            runtime_dir: inherited.home.parent().map(Path::to_path_buf),
            record: record(
                &inherited.source,
                (!dry_run).then_some(inherited.home.as_path()),
                true,
            ),
            home: inherited.home.clone(),
            choice: selection.choice,
            dry_run,
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
            runtime_dir: None,
            choice: selection.choice,
            dry_run,
            record: record(&selection.source, None, false),
        });
    }
    let home = runtime_dir.join(SNAPSHOT_DIR);
    fs::create_dir(&home).with_context(|| format!("cannot create {}", home.display()))?;
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&home, fs::Permissions::from_mode(0o755))
            .with_context(|| format!("cannot set mode 0755 on {}", home.display()))?;
    }
    let bundle = home.join(BUNDLE_DIR);
    copy_bundle(&selection.source, &bundle, LinkPolicy::KeepInternal)?;
    let record = record(&selection.source, Some(&home), false);
    write_pin_record(&home, &record)?;
    Ok(PinnedBundle {
        home,
        bundle,
        runtime_dir: Some(runtime_dir.to_path_buf()),
        choice: selection.choice,
        dry_run,
        record,
    })
}

fn write_pin_record(home: &Path, record: &BundleRecord) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let path = home.join(PIN_RECORD);
    let shared = BundleRecord {
        snapshot: None,
        recipe_file: None,
        ..record.clone()
    };
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .open(&path)
        .and_then(|mut file| file.write_all(&serde_json::to_vec(&shared)?))
        .with_context(|| format!("cannot write {}", path.display()))
}

/// E8: the settled state home must not lie under the run's `runtime/` folder.
/// Both paths are compared after resolving symlinks, so a state home that is
/// a link into `runtime/` is refused too.
pub(crate) fn check_state_home(state_home: &Path, pinned: &PinnedBundle) -> Result<()> {
    // A dry run sets no bundle variables, so there is nothing to check.
    if let Some(runtime) = pinned.runtime_dir.as_deref()
        && resolved(state_home).starts_with(resolved(runtime))
    {
        bail!(
            "AMPLIHACK_STATE_HOME {} lies under a run's runtime/ folder",
            state_home.display()
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
