//! `amplihack profile auto-switch …`: the toggle, threshold, exclusions,
//! `status`, `install` and `repair`.

use super::config::{self, ReadMode};
use super::fsx;
use super::liveness::{self, Liveness};
use super::state::{CREDS, Ledger, PHASE_INCONSISTENT, State};
use super::statusline;
use super::swap;
use super::usage::{Reading, Window};
use crate::commands::profile::ProfileError;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use crate::commands::profile::{default_live, duplicates, usage_cmd};
use anyhow::Result;
use clap::Subcommand;
use std::fmt::Write as _;

#[derive(Debug, Subcommand)]
pub enum AutoSwitchCommand {
    /// Turn auto-switch on, optionally setting the threshold and exclusions
    On {
        /// Switch when either usage window reaches this percentage (1-100)
        #[arg(long)]
        threshold: Option<String>,
        /// Profiles never switched to (comma-separated)
        #[arg(long, value_delimiter = ',')]
        exclude: Option<Vec<String>>,
    },
    /// Turn auto-switch off (keeps the threshold and exclusions)
    Off,
    /// Show the setting, readings, where each login is, and the wrapper state
    /// (read-only)
    Status,
    /// Set the threshold percentage (1-100)
    Threshold {
        #[arg(allow_hyphen_values = true)]
        value: String,
    },
    /// Replace the exclusion list; no names clears it
    Exclude { names: Vec<String> },
    /// Install the status-line wrapper in every profile that lacks it
    Install,
    /// Finish interrupted swaps, return logins whose session ended, and
    /// rewrite stale wrappers
    Repair,
}

fn threshold_arg(raw: &str) -> Result<u8> {
    config::parse_threshold(raw).map_err(|e| {
        anyhow::anyhow!(
            "invalid threshold {}: {e}",
            crate::commands::profile::escape_for_error(raw)
        )
    })
}

fn exclude_arg(names: &[String]) -> Result<Vec<ProfileName>, ProfileError> {
    names
        .iter()
        .filter(|n| !n.is_empty())
        .map(|n| ProfileName::parse(n))
        .collect()
}

pub fn handle(store: &ProfileStore, cmd: AutoSwitchCommand) -> Result<()> {
    let root = store.root();
    match cmd {
        AutoSwitchCommand::On { threshold, exclude } => {
            let threshold = threshold.as_deref().map(threshold_arg).transpose()?;
            let exclude = exclude.as_deref().map(exclude_arg).transpose()?;
            let c = config::update(root, ReadMode::DefaultOnInvalid, |c| {
                c.enabled = true;
                if let Some(t) = threshold {
                    c.threshold = t;
                }
                if let Some(e) = exclude {
                    c.exclude = e;
                }
                Ok(())
            })?;
            println!("auto-switch: on (threshold {}%)", c.threshold);
            warn_env_override(true);
            if let Ok(state) = State::new(root) {
                let missing =
                    statusline::without_wrapper(store, &state.root, &profile_names(store)?);
                if !missing.is_empty() {
                    eprintln!("note: {}", without_wrapper_note(&missing));
                }
            }
        }
        AutoSwitchCommand::Off => {
            config::update(root, ReadMode::DefaultOnInvalid, |c| {
                c.enabled = false;
                Ok(())
            })?;
            println!("auto-switch: off");
            warn_env_override(false);
        }
        AutoSwitchCommand::Threshold { value } => {
            let t = threshold_arg(&value)?;
            config::update(root, ReadMode::Editable, |c| {
                c.threshold = t;
                Ok(())
            })?;
            println!("auto-switch threshold: {t}%");
        }
        AutoSwitchCommand::Exclude { names } => {
            let exclude = exclude_arg(&names)?;
            let c = config::update(root, ReadMode::Editable, |c| {
                c.exclude = exclude;
                Ok(())
            })?;
            println!("auto-switch exclusions: {}", list(&c.exclude));
        }
        AutoSwitchCommand::Status => print!("{}", status(store)?),
        AutoSwitchCommand::Install => statusline::install(store, false)?,
        AutoSwitchCommand::Repair => repair(store)?,
    }
    Ok(())
}

/// Recovery under the global lock, then `install` with stale wrappers
/// rewritten. The install lines print first, then one `recovery:` line per
/// thing recovery did.
fn repair(store: &ProfileStore) -> Result<()> {
    let state = State::new(store.root())?;
    let done = if state.exists() {
        let _lock = state.lock()?;
        swap::recover_reporting(&state)?.1
    } else {
        Vec::new()
    };
    statusline::install(store, true)?;
    if done.is_empty() {
        println!("recovery: nothing to do");
    }
    for line in done {
        println!("recovery: {line}");
    }
    Ok(())
}

fn profile_names(store: &ProfileStore) -> Result<Vec<ProfileName>, ProfileError> {
    Ok(store
        .list()?
        .iter()
        .filter_map(|n| ProfileName::parse(n).ok())
        .collect())
}

fn without_wrapper_note(names: &[String]) -> String {
    format!(
        "profiles without the wrapper: {} — run: amplihack profile auto-switch install",
        names.join(", ")
    )
}

fn warn_env_override(file_on: bool) {
    match config::env_toggle(std::env::var_os(config::TOGGLE_ENV).as_deref()) {
        config::EnvToggle::On if !file_on => {
            eprintln!(
                "note: {}=1 in this shell keeps auto-switch on here",
                config::TOGGLE_ENV
            );
        }
        config::EnvToggle::Off if file_on => {
            eprintln!(
                "note: {}=0 in this shell keeps auto-switch off here",
                config::TOGGLE_ENV
            );
        }
        _ => {}
    }
}

fn list(names: &[ProfileName]) -> String {
    if names.is_empty() {
        "none".into()
    } else {
        names
            .iter()
            .map(ProfileName::as_str)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// A window cell: its usage, `0%` once it has reset, `-` with no reading.
fn cell(w: Option<&Window>, now: i64) -> String {
    match w {
        None => "-".into(),
        Some(w) if w.resets_at <= now => "0%".into(),
        Some(w) => format!("{:.0}%", w.used_percentage),
    }
}

/// The `5h` cell (issue #1818): `no session limit` for a reading with a
/// week window and no session window, otherwise as [`cell`].
pub(super) fn five_cell(r: Option<&Reading>, now: i64) -> String {
    match r {
        Some(r) if r.no_session_limit() => "no session limit".into(),
        _ => cell(r.and_then(|r| r.five_hour.as_ref()), now),
    }
}

fn ago(observed_at: Option<i64>, now: i64) -> String {
    let Some(t) = observed_at else {
        return "never".into();
    };
    let secs = (now - t).max(0);
    match secs {
        0..60 => format!("{secs}s ago"),
        60..3600 => format!("{}m ago", secs / 60),
        3600..86_400 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86_400),
    }
}

struct Row {
    name: String,
    five: String,
    seven: String,
    observed: String,
    login: String,
    wrapper: String,
}

fn table(rows: &[Row]) -> String {
    let header = Row {
        name: "profile".into(),
        five: "5h".into(),
        seven: "7d".into(),
        observed: "observed".into(),
        login: "login".into(),
        wrapper: "wrapper".into(),
    };
    let all: Vec<&Row> = std::iter::once(&header).chain(rows).collect();
    let w = |f: fn(&Row) -> &str| all.iter().map(|r| f(r).len()).max().unwrap_or(0);
    let (a, b, c, d, e) = (
        w(|r| &r.name),
        w(|r| &r.five),
        w(|r| &r.seven),
        w(|r| &r.observed),
        w(|r| &r.login),
    );
    let mut out = String::new();
    for r in all {
        let line = format!(
            "{:<a$}   {:<b$}   {:<c$}   {:<d$}   {:<e$}   {}",
            r.name, r.five, r.seven, r.observed, r.login, r.wrapper
        );
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

/// Read-only: takes no lock, runs no recovery and writes nothing, so looking
/// at the state cannot change it. What recovery would do is shown as notes.
fn status(store: &ProfileStore) -> Result<String> {
    let root = store.root();
    let eff = config::effective(root);
    let mut out = String::new();
    writeln!(out, "root: {}", root.display())?;
    writeln!(
        out,
        "switch: {} ({})",
        if eff.on { "on" } else { "off" },
        eff.source
    )?;
    #[cfg(unix)]
    {
        let inactive = if eff.on {
            ""
        } else {
            " — inactive (auto-switch off)"
        };
        let line = crate::commands::profile::rotate::autostart_line(root);
        writeln!(out, "{line}{inactive}")?;
        if !eff.config.overflow.is_empty() {
            let line = crate::commands::profile::rotate::overflow_gate_line(root);
            writeln!(out, "{line}")?;
        }
    }
    writeln!(out, "threshold: {}%", eff.config.threshold)?;
    writeln!(out, "exclude: {}", list(&eff.config.exclude))?;
    let names = profile_names(store)?;
    let Ok(state) = State::new(root) else {
        writeln!(out, "profiles: none")?;
        return Ok(out);
    };
    let now = fsx::now();
    let live = liveness::scan_system(&state.root);
    writeln!(out, "in-session swap: {}", swaps_label(&live))?;
    // Missing files read as empty, so this works before the first `on`.
    let ledger = state.load_ledger();
    let usage = state.load_usage_or_warn();
    let wrapper = |n: &ProfileName| statusline::wrapper_state(store, &state.root, n).to_string();
    let rows: Vec<Row> = names
        .iter()
        .map(|n| {
            let r = usage.accounts.get(n.as_str());
            let mut login = match &ledger {
                Ok(l) => login_place(&state, l, n),
                Err(_) => "unknown".to_string(),
            };
            if swap::swaps_disabled(&state, n.as_str()) {
                login.push_str(" (swaps disabled)");
            }
            Row {
                name: n.to_string(),
                five: five_cell(r, now),
                seven: cell(r.and_then(|r| r.seven_day.as_ref()), now),
                observed: ago(r.map(|r| r.observed_at), now),
                login,
                wrapper: wrapper(n),
            }
        })
        .collect();
    out.push_str(&table(&rows));
    match &ledger {
        Ok(l) if l.hosts.is_empty() => writeln!(out, "hosted directories: none")?,
        Ok(l) => {
            for (dir, h) in &l.hosts {
                writeln!(
                    out,
                    "hosted: {dir} holds the login of {} (swapped {})",
                    h.live,
                    ago(Some(h.swapped_at), now)
                )?;
            }
        }
        Err(_) => writeln!(out, "hosted directories: unknown (ledger unreadable)")?,
    }
    let pending = swap::pending_journal(&state);
    let problem = match &ledger {
        Err(e) => Some(e.to_string()),
        Ok(_) if pending => match state.load_journal() {
            Err(e) => Some(e.to_string()),
            Ok(Some(j)) if j.phase == PHASE_INCONSISTENT => Some(format!(
                "an earlier move of profile {:?}'s login into {:?} stopped half way",
                j.to, j.dir
            )),
            Ok(_) => None,
        },
        Ok(l) => invariant_problem(&state, l, &names),
    };
    match problem {
        Some(p) => writeln!(out, "invariant: PROBLEM: {p}")?,
        None if pending => writeln!(out, "invariant: pending recovery")?,
        None => writeln!(out, "invariant: ok (every login is in exactly one place)")?,
    }
    if let Some(line) = crate::commands::profile::rotate::platform_live_line(store.root()) {
        writeln!(out, "{line}")?;
    }
    for note in notes(
        store,
        &state,
        &eff,
        &names,
        ledger.as_ref().ok(),
        &live,
        pending,
    ) {
        writeln!(out, "note: {note}")?;
    }
    Ok(out)
}

/// The status notes, in the documented order. Every input is already read;
/// only the saved and current `oauthAccount` blocks, `$HOME/.claude.json`
/// and the rotator ledgers (for the live profile) are read here.
fn notes(
    store: &ProfileStore,
    state: &State,
    eff: &config::Effective,
    names: &[ProfileName],
    ledger: Option<&Ledger>,
    live: &Liveness,
    pending: bool,
) -> Vec<String> {
    let repair = "run: amplihack profile auto-switch repair";
    let mut notes = Vec::new();
    if pending {
        notes.push(format!("interrupted swap pending — {repair}"));
    }
    if let Some(l) = ledger {
        for dir in l.hosts.keys() {
            let ended = ProfileName::parse(dir).is_ok_and(|d| !live.is_live(&d));
            if ended {
                notes.push(format!("stale lease: {dir} (session ended) — {repair}"));
            }
        }
    }
    if config::env_toggle(std::env::var_os(config::TOGGLE_ENV).as_deref())
        == config::EnvToggle::Invalid
    {
        notes.push(format!(
            "ignored {} value (use 1 or 0); the file decides",
            config::TOGGLE_ENV
        ));
    }
    let missing = statusline::without_wrapper(store, &state.root, names);
    if !missing.is_empty() {
        notes.push(without_wrapper_note(&missing));
    }
    let gone: Vec<&ProfileName> = eff
        .config
        .exclude
        .iter()
        .filter(|e| !names.contains(e))
        .collect();
    if !gone.is_empty() {
        let gone: Vec<&str> = gone.iter().map(|n| n.as_str()).collect();
        notes.push(format!("excluded but missing: {}", gone.join(", ")));
    }
    let home = usage_cmd::home_dir();
    let live_profile = default_live::live_name(store, home.as_deref());
    // Issue #1700: API-key profiles share one credential; never duplicates.
    let kinds = crate::commands::profile::kind::Ctx::load(store, home.as_deref());
    let names: Vec<ProfileName> = names
        .iter()
        .filter(|n| kinds.kind(store, n) != crate::commands::profile::kind::Kind::Platform)
        .cloned()
        .collect();
    let accounts = duplicates::accounts_with(state, ledger, &names, home.as_deref(), live_profile);
    for (a, b) in duplicates::pairs(&accounts) {
        notes.push(duplicates::warning(&a, &b));
    }
    notes
}

fn swaps_label(live: &Liveness) -> &'static str {
    if live.supported() {
        "available (Linux)"
    } else if cfg!(target_os = "linux") {
        "unavailable (/proc cannot be read here; nothing is moved)"
    } else {
        "unavailable (needs Linux /proc; nothing is moved)"
    }
}

fn login_place(state: &State, ledger: &Ledger, n: &ProfileName) -> String {
    let n = n.as_str();
    if let Some(dir) = ledger.leased_to(n) {
        return format!("live in {dir}");
    }
    if ledger.hosts.contains_key(n) {
        return "parked".into();
    }
    if fsx::exists(&state.live_creds(n)) {
        "home".into()
    } else {
        "none".into()
    }
}

fn invariant_problem(state: &State, ledger: &Ledger, names: &[ProfileName]) -> Option<String> {
    let mut seen = std::collections::BTreeSet::new();
    for (dir, h) in &ledger.hosts {
        if !seen.insert(h.live.as_str()) {
            return Some(format!("{} is recorded as live in two directories", h.live));
        }
        if fsx::exists(&state.home_slot(ledger, &h.live)) {
            return Some(format!(
                "{}'s login is both in {dir} and in its home slot",
                h.live
            ));
        }
        if !fsx::exists(&state.parked_dir(dir).join(CREDS)) && ledger.leased_to(dir).is_none() {
            return Some(format!("{dir}'s own login is not parked"));
        }
    }
    for n in names {
        let n = n.as_str();
        if !ledger.hosts.contains_key(n) && fsx::exists(&state.parked_dir(n).join(CREDS)) {
            return Some(format!("{n} has a parked login but hosts nothing"));
        }
    }
    None
}
