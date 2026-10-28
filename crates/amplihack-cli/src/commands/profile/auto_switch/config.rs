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
    /// to recover from a damaged file).
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
pub fn effective(root: &Path) -> Effective {
    let env = env_toggle(std::env::var_os(TOGGLE_ENV).as_deref());
    if env == EnvToggle::Invalid {
        static WARNED: std::sync::Once = std::sync::Once::new();
        WARNED.call_once(|| {
            eprintln!("warning: {TOGGLE_ENV} must be 1 or 0; using <root>/{FILE}");
        });
    }
    effective_from(env, &read(root))
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
/// when `f` leaves them unchanged or fails.
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
    // says what they set.
    let (before, mut c) = match mode {
        ReadMode::Editable => {
            let c = read_editable(root)
                .map_err(|_| fail(format!("{FILE} is unreadable or invalid; not changed")))?;
            (Some(c.clone()), c)
        }
        ReadMode::DefaultOnInvalid => match read(root) {
            FileState::Valid(c) => (Some(c.clone()), c),
            FileState::Missing | FileState::Corrupt => (None, Config::default()),
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
    Ok(c)
}

/// Writes the file unlocked; callers go through `update`.
pub(super) fn write(root: &Path, c: &Config) -> Result<(), ProfileError> {
    let fail = |what: String| ProfileError::AutoSwitchConfig(what);
    fsx::check_root_private(root).map_err(|e| fail(format!("profile root {e}")))?;
    fsx::write_private_atomic(root, FILE, render(c).as_bytes())
        .map_err(|e| fail(format!("{FILE}: {e}")))
}
