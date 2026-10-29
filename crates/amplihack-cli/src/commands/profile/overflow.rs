//! `amplihack profile overflow` (issue #1606): marks profiles as overflow,
//! the one reader of those marks, and the [`Gate`] that decides when work
//! may move onto one. The marks are the `overflow=` key of
//! `<root>/.auto-switch`, each with a tier; spend blocks live in the rotate
//! ledger. See "Overflow commands" in docs/reference/profile-command.md.
//!
//! An overflow profile is a login that costs money per use (today: a Claude
//! account billed to usage credits). The rotator and auto-switch move onto
//! one only when [`Gate::allows`] says so, and both leave it as soon as a
//! plan profile has capacity again.
//!
//! `add` and `remove` change the file through `config::update`, under the
//! auto-switch state lock, so a concurrent `auto-switch exclude` or `on`, or
//! a rotator cycle, cannot lose either change. When the lock is busy for
//! longer than `config::CONFIG_LOCK_WAIT` they fail with a retry message and
//! change nothing. Output is printed after the lock is released.

use super::ProfileError;
use super::name::ProfileName;
use super::store::ProfileStore;
use std::path::Path;

/// One `overflow=` entry. The tier ranks kinds of overflow: tier 1 is
/// tried before tier 2, and so on, whatever the file order. Within a tier
/// the file order decides. Tiers exist so that a cheaper kind of overflow
/// (for example usage credits) can be preferred over a dearer one (for
/// example an API key) without relying on the order of names in the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverflowEntry {
    pub name: ProfileName,
    pub tier: u8,
}

/// The API-key tier (issue #1700): after every `overflow=` tier (1 to 9).
pub const PLATFORM_TIER: u8 = 10;

impl OverflowEntry {
    pub fn new(name: ProfileName, tier: u8) -> Self {
        Self { name, tier }
    }

    /// The API-key profile's entry, at [`PLATFORM_TIER`].
    pub fn platform(name: ProfileName) -> Self {
        Self {
            name,
            tier: PLATFORM_TIER,
        }
    }
}

/// When work may move onto an overflow profile, and what counts as plan
/// capacity. The rotator (`rotate::overflow`) and auto-switch
/// (`auto_switch::candidate`) both decide with one `Gate`, and auto-switch
/// takes its `Gate` from the running rotator (`rotate::gate_record`), so on
/// the same readings the two agree about when money is spent. Only when no
/// rotator runs does auto-switch build a `Gate` from the
/// `AMPLIHACK_ROTATE_*` variables, and then there is no rotator to disagree
/// with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gate {
    /// Seconds: the earliest plan reset must be further away than this
    /// (`--overflow-wait`, default 1800).
    pub wait: u64,
    /// Whether a plan reset that no reading reports counts as further away
    /// than `wait` (`--overflow-unknown-reset`, default off). Off, an
    /// unknown reset never moves work onto an overflow profile.
    pub unknown_reset: bool,
    /// A plan login whose session (five-hour) use is at or above this has no
    /// capacity (`--session-at`, default 85).
    pub session_at: u8,
    /// A plan login whose weekly (seven-day) use is at or above this has no
    /// capacity (`--week-at`, default 90).
    pub week_at: u8,
}

impl Gate {
    /// Whether a plan login using `session`% and `week`% has capacity: both
    /// below their thresholds.
    pub fn plan_capacity(&self, session: f64, week: f64) -> bool {
        session < f64::from(self.session_at) && week < f64::from(self.week_at)
    }

    /// Which of a plan login's windows are at their thresholds:
    /// `(session, week)`.
    pub fn at_threshold(&self, session: f64, week: f64) -> (bool, bool) {
        (
            session >= f64::from(self.session_at),
            week >= f64::from(self.week_at),
        )
    }

    /// Whether the earliest plan reset is far enough away to pay for
    /// overflow: strictly more than `wait` seconds after `now`. `None`
    /// (no reading reports a reset time) is far only with `unknown_reset`.
    pub fn far(&self, earliest_plan_reset: Option<i64>, now: i64) -> bool {
        match earliest_plan_reset {
            None => self.unknown_reset,
            Some(t) => t.saturating_sub(now) > i64::try_from(self.wait).unwrap_or(i64::MAX),
        }
    }

    /// Whether work may move onto an overflow profile: no plan profile has
    /// capacity and [`Self::far`] holds.
    pub fn allows(
        &self,
        plan_has_capacity: bool,
        earliest_plan_reset: Option<i64>,
        now: i64,
    ) -> bool {
        !plan_has_capacity && self.far(earliest_plan_reset, now)
    }
}

#[derive(Debug, clap::Subcommand)]
pub enum OverflowCommand {
    /// Mark a profile as overflow (billed per use), used only when every plan profile is exhausted
    Add {
        name: String,
        /// Lower tiers are tried first; within a tier, the order profiles were added (default 1)
        #[arg(long, value_name = "1-9", value_parser = clap::value_parser!(u8).range(1..=9))]
        tier: Option<u8>,
    },
    /// Remove the overflow mark from a profile
    Remove { name: String },
    /// List the overflow profiles in the order they are tried, with tier and any spend block
    List,
    /// Lift the rotator's spend-limit block on a profile, or the API-key block with --platform
    #[command(group(clap::ArgGroup::new("target").required(true).args(["name", "platform"])))]
    Clear {
        name: Option<String>,
        /// Lift the API-key block instead
        #[arg(long)]
        platform: bool,
    },
}

/// The overflow entries under `root`, in the order they are tried (tier,
/// then file order). Empty when `.auto-switch` is missing or corrupt, so a
/// damaged file never spends.
#[cfg(unix)]
pub fn overflow_entries(root: &Path) -> Vec<OverflowEntry> {
    use super::auto_switch::config::{self, FileState};
    match config::read(root) {
        FileState::Valid(c) => c.overflow_order(),
        FileState::Missing | FileState::Corrupt => Vec::new(),
    }
}

/// No auto-switch settings off Unix: no overflow profiles.
#[cfg(not(unix))]
pub fn overflow_entries(_root: &Path) -> Vec<OverflowEntry> {
    Vec::new()
}

/// The names of [`overflow_entries`], in the same order.
pub fn overflow_names(root: &Path) -> Vec<ProfileName> {
    overflow_entries(root).into_iter().map(|e| e.name).collect()
}

/// Every tier the rotator and auto-switch try, in order: the `overflow=`
/// entries without API-key profiles, then the first API-key profile by name
/// at [`PLATFORM_TIER`] (issue #1700). Empty when `.auto-switch` is
/// corrupt, so a damaged file never spends, on credits or on the API key.
/// `home` is the user's home directory.
#[cfg(all(test, unix))]
pub fn tiers(store: &ProfileStore, home: &Path) -> Vec<OverflowEntry> {
    tiers_detail(store, Some(home)).0
}

/// [`tiers`], and the API-key profiles that `overflow=` lists, which are
/// left out of the credit tiers.
#[cfg(unix)]
pub fn tiers_detail(
    store: &ProfileStore,
    home: Option<&Path>,
) -> (Vec<OverflowEntry>, Vec<ProfileName>) {
    use super::auto_switch::config::{self, FileState};
    let marked = match config::read(store.root()) {
        FileState::Valid(c) => c.overflow_order(),
        FileState::Missing => Vec::new(),
        FileState::Corrupt => return (Vec::new(), Vec::new()),
    };
    let platforms = super::kind::Ctx::load(store, home).platform_profiles(store);
    let (ignored, mut out): (Vec<OverflowEntry>, Vec<OverflowEntry>) = marked
        .into_iter()
        .partition(|e| platforms.contains(&e.name));
    if let Some(first) = platforms.first() {
        out.push(OverflowEntry::platform(first.clone()));
    }
    (out, ignored.into_iter().map(|e| e.name).collect())
}

#[cfg(unix)]
pub fn run(store: &ProfileStore, cmd: OverflowCommand) -> Result<(), ProfileError> {
    use super::auto_switch::config::{self, ReadMode};
    let root = store.root();
    match cmd {
        OverflowCommand::Add { name, tier } => {
            let name = ProfileName::parse(&name)?;
            // An existing profile directory, not a symlink.
            store.resolve(&name)?;
            let home = super::usage_cmd::home_dir();
            if super::kind::Ctx::load(store, home.as_deref()).kind(store, &name)
                == super::kind::Kind::Platform
            {
                return Err(ProfileError::PlatformNotMarkable(name.to_string()));
            }
            // `before`: `None` when newly added, else the tier it had. A
            // repeated `add` without `--tier` keeps the tier.
            let mut before = None;
            let mut now_tier = amplihack_utils::rotator_status::DEFAULT_TIER;
            let c = config::update(root, ReadMode::Editable, |c| {
                match c.overflow.iter_mut().find(|e| e.name == name) {
                    Some(e) => {
                        before = Some(e.tier);
                        e.tier = tier.unwrap_or(e.tier);
                        now_tier = e.tier;
                    }
                    None => {
                        now_tier = tier.unwrap_or(now_tier);
                        c.overflow.push(OverflowEntry::new(name.clone(), now_tier));
                    }
                }
                Ok(())
            })?;
            match before {
                None => {
                    println!("overflow: added {name} (tier {now_tier})");
                    println!(
                        "note: {name} is used only when every plan profile is exhausted and the earliest plan reset is more than --overflow-wait away; work on it is billed to that account."
                    );
                }
                Some(old) if old == now_tier => {
                    println!("overflow: {name} is already marked (tier {now_tier})")
                }
                Some(old) => println!("overflow: {name} moved from tier {old} to tier {now_tier}"),
            }
            if before.is_none() {
                println!(
                    "note: set a spend limit for the organization at https://claude.ai/settings/usage; the rotator is not a spending cap."
                );
            }
            if c.exclude.contains(&name) {
                eprintln!(
                    "warning: {name} is in the auto-switch exclude list; excluded profiles are never used, even as overflow"
                );
            }
            if super::pointer::read(root).ok().flatten().as_ref() == Some(&name) {
                eprintln!(
                    "warning: the default profile is {name}; launches use it directly and spend credits whatever the rotator does (run: amplihack profile switch --none)"
                );
            }
        }
        OverflowCommand::Remove { name } => {
            let name = ProfileName::parse(&name)?;
            let mut removed = false;
            config::update(root, ReadMode::Editable, |c| {
                if c.is_overflow(&name) {
                    c.overflow.retain(|e| e.name != name);
                    removed = true;
                }
                Ok(())
            })?;
            if removed {
                println!("overflow: removed {name}");
            } else {
                println!("overflow: {name} is not marked");
            }
        }
        OverflowCommand::List => {
            let c = config::read_editable(root)?;
            let home = super::usage_cmd::home_dir();
            let platforms = super::kind::Ctx::load(store, home.as_deref()).platform_profiles(store);
            if c.overflow.is_empty() && platforms.is_empty() {
                println!("overflow: none");
                return Ok(());
            }
            let ledger = super::rotate::ledger::RotateState::new(root).load_ledger();
            let blocks = match super::rotate::overflow_blocks(root, now()) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("warning: spend blocks not shown: {e}");
                    Default::default()
                }
            };
            let key_block = ledger
                .ok()
                .flatten()
                .and_then(|l| super::rotate::platform_block::active(&l, now()).cloned());
            for e in c.overflow_order() {
                let n = &e.name;
                let mut line = format!("{n}  tier {}", e.tier);
                if store.resolve(n).is_err() {
                    line.push_str("  missing (no profile directory)");
                }
                if let Some(until) = blocks.get(n.as_str()) {
                    line.push_str(&format!(
                        "  blocked until {} (spend_limit)",
                        super::rotate::timestamp(*until)
                    ));
                }
                println!("{line}");
            }
            for (i, p) in platforms.iter().enumerate() {
                let mut line = format!("{p}  api-key tier");
                match (&key_block, platforms.first()) {
                    (_, Some(first)) if i > 0 => {
                        line.push_str(&format!("  not used ({first} comes first)"));
                    }
                    (Some(b), _) => line.push_str(&format!(
                        "  blocked until {} ({})",
                        super::rotate::timestamp(b.until),
                        b.reason
                    )),
                    _ => {}
                }
                println!("{line}");
            }
        }
        OverflowCommand::Clear { platform: true, .. } => {
            match super::rotate::platform_block::clear_reason(root)? {
                Some(reason) => println!("overflow: cleared the API-key block ({reason})"),
                None => println!("overflow: the API key is not blocked"),
            }
        }
        OverflowCommand::Clear { name, .. } => {
            let name = ProfileName::parse(name.as_deref().unwrap_or_default())?;
            if super::rotate::clear_overflow_block(root, &name)? {
                println!("overflow: cleared the spend block on {name}");
            } else {
                println!("overflow: {name} has no spend block");
            }
        }
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn run(_store: &ProfileStore, _cmd: OverflowCommand) -> Result<(), ProfileError> {
    Err(ProfileError::RotateUnsupported)
}

#[cfg(unix)]
fn now() -> i64 {
    super::auto_switch::kit::now()
}
