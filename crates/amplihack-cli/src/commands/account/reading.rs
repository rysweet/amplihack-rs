//! Live readings of every candidate account (issue #2006), taken before the
//! lease lock and never cached across picks.
//!
//! - Codex: the accounts `codex_capacity::list_accounts` finds under the
//!   profiles root, plus `~/.codex` when it is signed in, read live through
//!   the Codex app-server. Live headroom is `codex_capacity::headroom`.
//! - Claude: the profile store's subscription login profiles, except the one
//!   whose login is live in `~/.claude`, probed as `profile usage` probes
//!   them. Live headroom is 100 minus the fuller of the session and weekly
//!   windows.
//!
//! What leaves this module about an account is derived: numbers, a fixed
//! reason word, the home path and the account name. Raw server or probe text
//! never reaches a note.

use super::Cli;
use super::pick::Readings;
use amplihack_utils::codex_capacity::{self, AccountResult, Outcome};
use std::path::PathBuf;
use std::time::Duration;

/// The longest note, in characters.
const MAX_NOTE: usize = 200;

/// One account as read.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    /// The profile or account name.
    pub account: String,
    pub home: PathBuf,
    /// `None` when the account gave no reading.
    pub live_headroom: Option<f64>,
    pub usable: bool,
    pub note: String,
}

/// `p` as a percentage without a needless `.0`.
fn pct(p: f64) -> String {
    if p.fract() == 0.0 {
        format!("{p:.0}%")
    } else {
        format!("{p:.1}%")
    }
}

/// A Codex account's candidate from its live result. `floor` is the credit
/// balance above which a full account still counts as usable.
pub fn codex_candidate(r: &AccountResult, floor: f64) -> Candidate {
    let (live_headroom, usable, note) = match &r.outcome {
        Outcome::Ok(reading) => {
            let room = codex_capacity::headroom(reading, floor);
            let fullest = reading
                .windows
                .iter()
                .filter_map(|w| w.used_percent)
                .fold(None, |m: Option<f64>, p| Some(m.map_or(p, |m| m.max(p))));
            let mut note = match fullest {
                Some(u) => format!("used {}", pct(u)),
                None => "no usage window".to_string(),
            };
            if room == 0.5 {
                note.push_str("; credits above the floor");
            } else if room < 0.0 {
                note.push_str("; no room");
            }
            (Some(room), room > 0.0, note)
        }
        Outcome::NotSignedIn => (None, false, "not signed in".to_string()),
        Outcome::Timeout => (None, false, "timed out".to_string()),
        // The reader's own text may quote the server, so it is never kept.
        _ => (None, false, "error: read failed".to_string()),
    };
    Candidate {
        account: r.label.clone(),
        home: r.path.clone(),
        live_headroom,
        usable,
        note,
    }
}

/// `c` made safe to print: a home holding a byte below 0x20 (or DEL) is
/// unusable with the note `unsafe path`; every note loses its control
/// characters and is cut to [`MAX_NOTE`] characters.
pub fn checked(mut c: Candidate) -> Candidate {
    if c.home
        .as_os_str()
        .as_encoded_bytes()
        .iter()
        .any(|&b| b < 0x20 || b == 0x7f)
    {
        c.usable = false;
        c.note = "unsafe path".to_string();
        return c;
    }
    c.note = c
        .note
        .chars()
        .filter(|ch| !ch.is_control())
        .take(MAX_NOTE)
        .collect();
    c
}

/// Every candidate of `cli`, read live now, with the time of the earliest
/// read. Warnings go to stderr. `floor` applies to Codex only.
pub fn read_live(cli: Cli, floor: f64) -> Readings {
    let now = chrono::Utc::now().timestamp();
    let home = std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from);
    let candidates = match cli {
        Cli::Codex => read_codex(home.as_deref(), floor),
        Cli::Claude => read_claude(home.as_deref()),
    };
    let read_at = candidates.iter().map(|(_, t)| *t).min().unwrap_or(now);
    Readings {
        candidates: candidates.into_iter().map(|(c, _)| checked(c)).collect(),
        read_at,
    }
}

fn read_codex(home: Option<&std::path::Path>, floor: f64) -> Vec<(Candidate, i64)> {
    let (secs, warning) = codex_capacity::timeout_from_env();
    if let Some(w) = warning {
        eprintln!("{w}");
    }
    let accounts = match codex_capacity::profiles_root(home) {
        Some(root) => match codex_capacity::list_accounts(&root, home) {
            Ok(a) => a,
            Err(e) => {
                eprintln!(
                    "warning: account pick: could not list Codex profiles in {}: {e}",
                    root.display()
                );
                Vec::new()
            }
        },
        None => Vec::new(),
    };
    if accounts.is_empty() {
        return Vec::new();
    }
    let bin = codex_capacity::codex_bin();
    codex_capacity::read_all(&bin, &accounts, Duration::from_secs(secs))
        .iter()
        .map(|r| (codex_candidate(r, floor), r.read_at))
        .collect()
}

fn read_claude(home: Option<&std::path::Path>) -> Vec<(Candidate, i64)> {
    use crate::commands::profile::pick_bridge;
    match pick_bridge::claude_rows(home) {
        Ok(rows) => rows
            .into_iter()
            .map(|r| {
                (
                    Candidate {
                        account: r.name,
                        home: r.dir,
                        live_headroom: r.live_headroom,
                        usable: r.usable,
                        note: r.note,
                    },
                    r.read_at,
                )
            })
            .collect(),
        Err(e) => {
            eprintln!("warning: account pick: could not read Claude profiles: {e}");
            Vec::new()
        }
    }
}
