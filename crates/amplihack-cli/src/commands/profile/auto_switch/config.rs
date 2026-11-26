//! `<root>/.auto-switch`: the on/off switch, threshold, exclusions and
//! overflow profiles with their tiers, and the `AMPLIHACK_CLAUDE_AUTO_SWITCH`
//! override. Read again on every check; never cached. The grammar and the
//! file read are `amplihack_utils::rotator_status`'s, which the hooks use too.

use super::fsx;
use crate::commands::profile::ProfileError;
use crate::commands::profile::kind::Kind;
use crate::commands::profile::name::ProfileName;
pub use crate::commands::profile::overflow::OverflowEntry;
use amplihack_utils::rotator_status::{self as shared, DEFAULT_THRESHOLD, Toggle, ToggleFile};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use amplihack_utils::rotator_status::{DEFAULT_TIER, MAX_TOGGLE_BYTES as MAX_BYTES};
pub use amplihack_utils::rotator_status::{EnvToggle, TOGGLE_ENV, TOGGLE_FILE as FILE, env_toggle};

/// How long a writer waits for the auto-switch state lock, which a rotator
/// cycle or a swap may hold, before giving up with a retry message.
pub const CONFIG_LOCK_WAIT: Duration = Duration::from_secs(10);

/// What a writer starts from when the file is missing or corrupt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadMode {
    /// The defaults in both cases (`auto-switch on` and `off`, which exist
    /// to recover from a damaged file), except that a corrupt or unsafe
    /// file's `exclude` list is kept when it still parses (issue #2218).
    DefaultOnInvalid,
    /// The defaults when missing; a corrupt file is refused and left alone
    /// (threshold, exclude, overflow add and remove).
    Editable,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub enabled: bool,
    pub threshold: u8,
    pub exclude: Vec<ProfileName>,
    /// Overflow profiles (issue #1606) in file order, each with its tier.
    /// They are used only when every plan profile is exhausted; see
    /// [`Config::overflow_order`] for the order they are tried in.
    pub overflow: Vec<OverflowEntry>,
    /// Kind overrides (issue #1700), the `kind=` line.
    pub kind: BTreeMap<ProfileName, Kind>,
}

impl Config {
    /// Whether `name` is an overflow profile.
    pub fn is_overflow(&self, name: &ProfileName) -> bool {
        self.overflow.iter().any(|e| &e.name == name)
    }

    /// The overflow entries in the order they are tried: by tier, then by
    /// file order within a tier.
    pub fn overflow_order(&self) -> Vec<OverflowEntry> {
        let mut out = self.overflow.clone();
        out.sort_by_key(|e| e.tier);
        out
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: false,
            threshold: DEFAULT_THRESHOLD,
            exclude: Vec::new(),
            overflow: Vec::new(),
            kind: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum FileState {
    Missing,
    Valid(Config),
    Corrupt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Env,
    File,
    Default,
    Corrupt,
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Env => "env",
            Self::File => "file",
            Self::Default => "default",
            Self::Corrupt => "corrupt",
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Effective {
    pub on: bool,
    pub source: Source,
    /// Threshold, exclusions and overflow profiles: always the file's, or the
    /// defaults (so a corrupt file gives no overflow profiles).
    pub config: Config,
}

/// Parses a toggle file into this crate's [`Config`]; [`read`] does the
/// same for the file on disk.
#[cfg(test)]
pub fn parse(raw: &[u8]) -> Result<Config, String> {
    shared::parse_toggle(raw)
        .and_then(Config::from_toggle)
        .map_err(String::from)
}

/// `1`..=`100`, digits only.
pub fn parse_threshold(value: &str) -> Result<u8, String> {
    shared::parse_threshold(value).map_err(String::from)
}

impl Config {
    fn from_toggle(t: Toggle) -> Result<Self, &'static str> {
        let exclude = t
            .exclude
            .iter()
            .map(|n| ProfileName::parse(n).map_err(|_| "invalid profile name in exclude"))
            .collect::<Result<_, _>>()?;
        let overflow = t
            .overflow
            .iter()
            .map(|(n, tier)| {
                ProfileName::parse(n)
                    .map(|name| OverflowEntry { name, tier: *tier })
                    .map_err(|_| "invalid profile name in overflow")
            })
            .collect::<Result<_, _>>()?;
        // The shared parser skips an unknown kind; here it fails closed.
        if t.unknown_kind {
            return Err("unknown kind");
        }
        let mut kind = BTreeMap::new();
        for (n, k) in &t.kind {
            let name = ProfileName::parse(n).map_err(|_| "invalid profile name in kind")?;
            // The shared parser keeps only the two spellings it stores.
            let k = Kind::parse(k).ok_or("unknown kind")?;
            kind.insert(name, k);
        }
        Ok(Self {
            enabled: t.enabled,
            threshold: t.threshold,
            exclude,
            overflow,
            kind,
        })
    }
}

/// The file's text. `overflow=` and then `kind=` come last, each only when
/// it is not empty, so a file without them is unchanged by these keys. An
/// overflow entry in the default tier is written as its bare name; a kind
/// is written as `platform` or `subscription`.
pub fn render(c: &Config) -> String {
    let exclude: Vec<&str> = c.exclude.iter().map(ProfileName::as_str).collect();
    let mut out = format!(
        "enabled={}\nthreshold={}\nexclude={}\n",
        if c.enabled { "on" } else { "off" },
        c.threshold,
        exclude.join(",")
    );
    if !c.overflow.is_empty() {
        let entries: Vec<String> = c
            .overflow
            .iter()
            .map(|e| {
                if e.tier == DEFAULT_TIER {
                    e.name.to_string()
                } else {
                    format!("{}:{}", e.name, e.tier)
                }
            })
            .collect();
        out.push_str(&format!("overflow={}\n", entries.join(",")));
    }
    if !c.kind.is_empty() {
        let entries: Vec<String> = c
            .kind
            .iter()
            .map(|(n, k)| format!("{n}:{}", k.stored()))
            .collect();
        out.push_str(&format!("kind={}\n", entries.join(",")));
    }
    out
}

pub fn effective_from(env: EnvToggle, file: &FileState) -> Effective {
    let config = match file {
        FileState::Valid(c) => c.clone(),
        _ => Config::default(),
    };
    let (on, source) = match (env, file) {
        (EnvToggle::On, _) => (true, Source::Env),
        (EnvToggle::Off, _) => (false, Source::Env),
        (_, FileState::Valid(c)) => (c.enabled, Source::File),
        (_, FileState::Missing) => (false, Source::Default),
        (_, FileState::Corrupt) => (false, Source::Corrupt),
    };
    Effective { on, source, config }
}

/// The effective setting for this process: the environment first, then the
/// file. An unrecognised environment value warns once per process.
///
/// Issue #2218: for a corrupt or unsafe file every other setting is the
/// default, but `config.exclude` is [`excluded_profiles`], the same list the
/// rotator, `profile usage` and `profile verify` apply, so no reader of the
/// exclude list fails open. When even that cannot be worked out, `on` is
/// false: nothing may be swapped in if nothing is known to be excluded.
pub fn effective(root: &Path) -> Effective {
    let env = env_toggle(std::env::var_os(TOGGLE_ENV).as_deref());
    if env == EnvToggle::Invalid {
        static WARNED: std::sync::Once = std::sync::Once::new();
        WARNED.call_once(|| {
            eprintln!("warning: {TOGGLE_ENV} must be 1 or 0; using <root>/{FILE}");
        });
    }
    let file = read(root);
    let mut eff = effective_from(env, &file);
    if file == FileState::Corrupt {
        match excluded_profiles(root) {
            Ok(names) => eff.config.exclude = names,
            Err(_) => eff.on = false,
        }
    }
    eff
}

/// The file as it is. One not owned by the current user, or writable by
/// group or others, reads as `Corrupt`, like the rotate ledger: anyone who
/// can write it could otherwise mark a profile as overflow and spend credits.
pub fn read(root: &Path) -> FileState {
    match shared::read_toggle(root) {
        ToggleFile::Missing => FileState::Missing,
        ToggleFile::Valid(t) => Config::from_toggle(t).map_or(FileState::Corrupt, FileState::Valid),
        ToggleFile::Corrupt => FileState::Corrupt,
    }
}

/// The `exclude` list as [`read_exclude`] finds it (issue #2218).
#[derive(Debug, Clone, PartialEq)]
pub enum ExcludeFile {
    /// The file is missing (no exclusions), or is a private regular file
    /// of yours that parses. `trusted` is false when [`read`] would still
    /// call it corrupt for its other settings, such as an unknown kind.
    Read {
        names: Vec<ProfileName>,
        trusted: bool,
    },
    /// The file exists but cannot be trusted for its list: it cannot be
    /// read, does not parse, or is owned by another user or writable by
    /// group or others.
    Unreadable,
}

/// Issue #2218: the `exclude` list, or [`ExcludeFile::Unreadable`] when
/// the list cannot be trusted. A list is only as trustworthy as whoever can
/// write it, and a writer can remove an entry as easily as add one:
/// rewriting `exclude=shared` to `exclude=` would get `shared` probed and
/// refreshed, signing out the other hosts that share it. So a file owned by
/// another user, or writable by group or others, is `Unreadable` like a
/// symlink, a file that is too large or one that does not parse, and every
/// profile then counts as excluded. The worst such a writer can do is stop
/// rotation (a denial of service); it can never un-exclude a login. Only a
/// private file of yours that is invalid for its other settings keeps its
/// list, with `trusted: false`: [`read`] still refuses it for those.
pub fn read_exclude(root: &Path) -> ExcludeFile {
    use amplihack_utils::secure_files::{owned_by_euid, read_bounded_nofollow, writable_by_others};
    let (raw, meta) = match read_bounded_nofollow(&root.join(FILE), MAX_BYTES) {
        Ok(None) => {
            return ExcludeFile::Read {
                names: Vec::new(),
                trusted: true,
            };
        }
        Ok(Some(found)) => found,
        Err(_) => return ExcludeFile::Unreadable,
    };
    if !owned_by_euid(&meta) || writable_by_others(&meta) {
        return ExcludeFile::Unreadable;
    }
    let Ok(toggle) = shared::parse_toggle(&raw) else {
        return ExcludeFile::Unreadable;
    };
    let Ok(names) = toggle
        .exclude
        .iter()
        .map(|n| ProfileName::parse(n))
        .collect::<Result<Vec<_>, _>>()
    else {
        return ExcludeFile::Unreadable;
    };
    let trusted = Config::from_toggle(toggle).is_ok();
    ExcludeFile::Read { names, trusted }
}

/// Issue #2218: the one rule for which profiles are excluded, followed by
/// every reader of the list: the file's `exclude` list, or every profile in
/// the store when the file cannot be read, does not parse or is not a
/// private file of yours. A
/// store that cannot be listed is an error, never an empty list, so the
/// fallback cannot fail open.
pub fn excluded_profiles(root: &Path) -> Result<Vec<ProfileName>, ProfileError> {
    excluded_from(read_exclude(root), root)
}

/// [`excluded_profiles`] for a file already read with [`read_exclude`], so
/// a caller that warns about the file applies this same rule to the same
/// read instead of repeating it.
pub fn excluded_from(file: ExcludeFile, root: &Path) -> Result<Vec<ProfileName>, ProfileError> {
    match file {
        ExcludeFile::Read { names, .. } => Ok(names),
        ExcludeFile::Unreadable => every_profile(root),
    }
}

/// Every profile in the store under `root`, or the listing error.
pub fn every_profile(root: &Path) -> Result<Vec<ProfileName>, ProfileError> {
    let names = crate::commands::profile::ProfileStore::new(root.to_path_buf()).list()?;
    Ok(names
        .iter()
        .filter_map(|n| ProfileName::parse(n).ok())
        .collect())
}

/// The settings an edit starts from: the file's, or the defaults when it
/// is missing. A corrupt file is an error and is left alone. Reads only.
pub fn read_editable(root: &Path) -> Result<Config, ProfileError> {
    match read(root) {
        FileState::Valid(c) => Ok(c),
        FileState::Missing => Ok(Config::default()),
        FileState::Corrupt => Err(ProfileError::AutoSwitchConfig(format!(
            "{FILE} is unreadable or invalid"
        ))),
    }
}

/// The only writer of `.auto-switch` outside tests: a read-modify-write
/// under the auto-switch state lock, so two writers (or a writer and a
/// rotator cycle) cannot lose each other's change. Waits at most
/// `CONFIG_LOCK_WAIT`. Returns the resulting settings; nothing is written
/// when `f` leaves them unchanged or fails. A write that changes `enabled`,
/// or replaces a missing or corrupt file through `on` or `off`, appends a
/// `change:` line to the rotate log (issue #1803).
pub fn update(
    root: &Path,
    mode: ReadMode,
    f: impl FnOnce(&mut Config) -> Result<(), ProfileError>,
) -> Result<Config, ProfileError> {
    update_within(root, mode, CONFIG_LOCK_WAIT, f)
}

/// `update` with the lock wait given.
pub fn update_within(
    root: &Path,
    mode: ReadMode,
    wait: Duration,
    f: impl FnOnce(&mut Config) -> Result<(), ProfileError>,
) -> Result<Config, ProfileError> {
    let fail = |what: String| ProfileError::AutoSwitchConfig(what);
    let state = super::state::State::new(root)?;
    let Some(_lock) = state.try_lock(wait)? else {
        return Err(fail(format!(
            "{FILE} is being changed by another process (the rotator or auto-switch); retry"
        )));
    };
    // `before` is what an unchanged result needs no write for. `on` and
    // `off` always write over a missing or corrupt file, so the file then
    // says what they set; `was` names that file for the change log.
    let mut was = None;
    if let Some(refused) = unreplaceable_refusal(root) {
        return Err(refused);
    }
    let (before, mut c) = match mode {
        ReadMode::Editable => {
            let c = read_editable(root).map_err(|_| corrupt_refusal(root))?;
            (Some(c.clone()), c)
        }
        ReadMode::DefaultOnInvalid => match read(root) {
            FileState::Valid(c) => (Some(c.clone()), c),
            FileState::Missing => {
                was = Some("unset");
                (None, Config::default())
            }
            FileState::Corrupt => {
                was = Some("corrupt");
                (None, recovered(root))
            }
        },
    };
    f(&mut c)?;
    if before.as_ref() == Some(&c) {
        return Ok(c);
    }
    if render(&c).len() as u64 > MAX_BYTES {
        return Err(fail(format!(
            "{FILE} would be larger than {MAX_BYTES} bytes; not changed"
        )));
    }
    write(root, &c)?;
    #[cfg(unix)]
    {
        use crate::commands::profile::rotate::change_log::{on_off, record};
        let old = match (&before, was) {
            (Some(b), _) if b.enabled != c.enabled => Some(on_off(b.enabled)),
            (None, Some(w)) => Some(w),
            _ => None,
        };
        if let Some(old) = old {
            record(root, "auto-switch", old, on_off(c.enabled));
        }
    }
    #[cfg(not(unix))]
    let _ = was;
    Ok(c)
}

/// Issue #2218: what `on` and `off` start from over a corrupt or unsafe
/// file: the defaults, but with the `exclude` list kept when it is a
/// private file of yours that still parses, so recovering the file does not
/// drop its exclusions. A file that does not parse, or that others could
/// have edited, has no list to keep; `auto-switch on` and `off` refuse it
/// unless `--exclude` names the list ([`exclude_recoverable`]).
fn recovered(root: &Path) -> Config {
    let exclude = match read_exclude(root) {
        ExcludeFile::Read { names, .. } => names,
        ExcludeFile::Unreadable => Vec::new(),
    };
    Config {
        exclude,
        ..Config::default()
    }
}

/// The refusal of an edit over a `.auto-switch` whose list cannot be
/// trusted (issue #2218): one that does not parse, or one writable by group
/// or others. Given to `on` and `off` without `--exclude`, and to every
/// edit that keeps the file's other keys.
pub(crate) const EXCLUDE_UNKNOWN_REFUSAL: &str = "auto-switch config does not parse or \
     others can write it, so its exclude list is unknown; not changed. Run `amplihack profile auto-switch on --exclude \
     NAME,...` or `off --exclude NAME,...` naming the logins that must stay excluded \
     (`--exclude ''` for none)";

/// Why an edit that keeps the file's other keys refuses a corrupt file of
/// yours (issue #2218): `on` and `off` are the recovery that keeps its
/// exclude list, so the refusal names them and never carries the generic
/// `AutoSwitchConfig` hint, which says to remove the file. A file that is
/// not a regular file of yours never gets here: [`update_within`] refuses
/// it first with [`unreplaceable_refusal`].
fn corrupt_refusal(root: &Path) -> ProfileError {
    ProfileError::AutoSwitchExcludeAtRisk(if exclude_recoverable(root) {
        format!(
            "{FILE} is invalid; not changed. Run `amplihack profile auto-switch on` \
             or `off` first: they rewrite it and keep its exclude list"
        )
    } else {
        EXCLUDE_UNKNOWN_REFUSAL.into()
    })
}

/// Issue #2218: the refusal of every write over a `.auto-switch` that no
/// command may replace: a symlink, something that is not a regular file, or
/// a file owned by another user (for example one left by `sudo amplihack
/// profile auto-switch ...`). While it is there [`read_exclude`] cannot
/// trust it, so every profile counts as excluded and rotation is paused.
/// Removing it would leave no exclusions, so the logins it was meant to
/// exclude would be probed and refreshed; the refusal says
/// how to make it writable instead, and its hint never says remove. `None`
/// when the file is missing or is a regular file of yours.
pub(crate) fn unreplaceable_refusal(root: &Path) -> Option<ProfileError> {
    let meta = std::fs::symlink_metadata(root.join(FILE)).ok()?;
    let fix = if meta.file_type().is_symlink() {
        "is a symlink"
    } else if !meta.is_file() {
        "is not a regular file"
    } else if !amplihack_utils::secure_files::owned_by_euid(&meta) {
        "is owned by another user"
    } else {
        return None;
    };
    Some(ProfileError::AutoSwitchExcludeAtRisk(format!(
        "{FILE} {fix}, so auto-switch cannot replace it; not changed. Until it is fixed \
         every profile counts as excluded and rotation is paused. Make it a regular \
         file of yours holding the `exclude=` line you intend (check it: another user \
         could have edited it): fix the owner with `chown`, or \
         replace the symlink with a copy of its target. Do not delete it"
    )))
}

/// Whether `.auto-switch` is a regular file (not a symlink) owned by the
/// effective user: one `on` and `off` can replace.
pub(crate) fn own_regular_file(root: &Path) -> bool {
    std::fs::symlink_metadata(root.join(FILE))
        .is_ok_and(|m| m.file_type().is_file() && amplihack_utils::secure_files::owned_by_euid(&m))
}

/// Issue #2218: whether `on` or `off` can rewrite the file without being
/// told the exclude list: it is missing, or it is a private file of yours
/// that parses.
pub fn exclude_recoverable(root: &Path) -> bool {
    !matches!(read_exclude(root), ExcludeFile::Unreadable)
}

/// Writes the file unlocked; callers go through `update`.
pub(super) fn write(root: &Path, c: &Config) -> Result<(), ProfileError> {
    let fail = |what: String| ProfileError::AutoSwitchConfig(what);
    fsx::check_root_private(root).map_err(|e| fail(format!("profile root {e}")))?;
    fsx::write_private_atomic(root, FILE, render(c).as_bytes())
        .map_err(|e| fail(format!("{FILE}: {e}")))
}
