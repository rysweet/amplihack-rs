//! `amplihack account pick` (issue #2006): the Claude or Codex account with
//! the most live headroom after the expected use of jobs already running on
//! each account and of picks made moments ago.
//!
//! See docs/reference/account-pick.md for the command, ranking, job counting,
//! lease and pin contracts. The parts:
//!
//! - [`config`]: settings from the environment.
//! - [`reading`]: live readings of every candidate, taken without the lock.
//! - [`procscan`]: running jobs per account from `/proc`.
//! - [`lease`]: lease files and the lock that serialises picks.
//! - [`rank`]: the ranking rule.
//! - [`pick`]: one pick end to end.
//! - [`launch`]: the `amplihack codex`, `claude` and `recipe run` hooks.

pub mod config;
pub mod launch;
pub mod lease;
pub mod pick;
pub mod procscan;
pub mod rank;
pub mod reading;

use amplihack_utils::codex_capacity::DEFAULT_FLOOR;
use anyhow::Result;
use clap::{Args, Subcommand, ValueEnum};
use pick::{Pick, PickError, PickOpts, Ranked};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::Path;

/// Set by `scripts/codex-pick`, so a failure keeps codex-pick's wording.
pub const CALLER_ENV: &str = "AMPLIHACK_ACCOUNT_PICK_CALLER";
const CODEX_PICK_NO_ACCOUNT: &str =
    "codex-pick: no signed-in profile has headroom above the credit floor";
const MAX_FLOOR: f64 = 1e9;

/// Which CLI's accounts to pick from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Cli {
    Claude,
    Codex,
}

impl Cli {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    /// The variable that names an account home for this CLI.
    pub fn env_key(self) -> &'static str {
        match self {
            Self::Claude => "CLAUDE_CONFIG_DIR",
            Self::Codex => "CODEX_HOME",
        }
    }
}

#[derive(Subcommand, Debug)]
pub enum AccountCommands {
    /// Print the account with the most live headroom after running jobs and
    /// recent picks, and record a short-lived lease for it
    Pick(PickArgs),
}

#[derive(Args, Debug)]
pub struct PickArgs {
    /// Which CLI's accounts to pick from
    #[arg(long, value_enum)]
    pub cli: Cli,
    /// Print one JSON object with the choice and every candidate
    #[arg(long, conflicts_with = "list")]
    pub json: bool,
    /// Print every candidate, best first, and write no lease
    #[arg(long)]
    pub list: bool,
    /// Codex only: the credit balance above which a full account is still
    /// usable (default 3000)
    #[arg(long, value_name = "N", value_parser = parse_floor, allow_negative_numbers = true)]
    pub floor: Option<f64>,
}

fn parse_floor(raw: &str) -> std::result::Result<f64, String> {
    let v: f64 = raw
        .parse()
        .map_err(|_| format!("{raw:?} is not a number"))?;
    if v.is_finite() && (0.0..=MAX_FLOOR).contains(&v) {
        Ok(v)
    } else {
        Err(format!("{raw:?} is not a number from 0 to {MAX_FLOOR}"))
    }
}

pub fn run(cmd: AccountCommands) -> Result<()> {
    match cmd {
        AccountCommands::Pick(args) => run_pick(args),
    }
}

/// A home as printed: as it is, or quoted when it holds a control byte.
fn shown(home: &Path) -> String {
    let lossy = home.to_string_lossy();
    if lossy.chars().any(char::is_control) {
        format!("{lossy:?}")
    } else {
        lossy.into_owned()
    }
}

fn candidate_json(r: &Ranked) -> Value {
    json!({
        "account": r.candidate.account,
        "home": r.candidate.home.to_string_lossy(),
        "live_headroom": r.candidate.live_headroom,
        "running_jobs": r.running_jobs,
        "active_leases": r.active_leases,
        "effective_headroom": r.effective_headroom,
        "usable": r.candidate.usable,
        "note": r.candidate.note,
    })
}

/// The `--json` object: the chosen account's fields (all null when none was
/// chosen), the ranked candidates and the error, if any.
fn report_json(
    cli: Cli,
    cost: f64,
    chosen: Option<&Ranked>,
    ranked: &[Ranked],
    read_at: Option<i64>,
    error: Option<String>,
) -> Value {
    json!({
        "cli": cli.as_str(),
        "account": chosen.map(|r| r.candidate.account.clone()),
        "home": chosen.map(|r| r.candidate.home.to_string_lossy().into_owned()),
        "live_headroom": chosen.and_then(|r| r.candidate.live_headroom),
        "running_jobs": chosen.map(|r| r.running_jobs),
        "active_leases": chosen.map(|r| r.active_leases),
        "effective_headroom": chosen.and_then(|r| r.effective_headroom),
        "job_cost_pct": cost,
        "read_at": read_at,
        "error": error,
        "candidates": ranked.iter().map(candidate_json).collect::<Vec<_>>(),
    })
}

fn pick_json(cli: Cli, cost: f64, pick: &Pick) -> Value {
    let chosen = pick.chosen.map(|i| &pick.ranked[i]);
    report_json(cli, cost, chosen, &pick.ranked, Some(pick.read_at), None)
}

fn error_json(cli: Cli, cost: f64, err: &PickError) -> Value {
    let (ranked, read_at): (&[Ranked], _) = match err {
        PickError::NoUsable {
            ranked, read_at, ..
        } => (ranked, Some(*read_at)),
        PickError::Io { .. } => (&[], None),
    };
    report_json(cli, cost, None, ranked, read_at, Some(err.to_string()))
}

fn run_pick(args: PickArgs) -> Result<()> {
    let cfg = launch::config();
    let floor = match (args.cli, args.floor) {
        (Cli::Claude, Some(_)) => {
            eprintln!("warning: --floor applies to --cli codex only; ignored");
            DEFAULT_FLOOR
        }
        (_, f) => f.unwrap_or(DEFAULT_FLOOR),
    };
    let result = launch::pick_live(
        &cfg,
        args.cli,
        floor,
        None,
        PickOpts {
            write_lease: !args.list,
        },
    );
    match result {
        Ok(pick) => {
            if args.list {
                for r in &pick.ranked {
                    println!(
                        "{:6.1}  {}  ({})",
                        r.effective_headroom.unwrap_or(-1.0),
                        shown(&r.candidate.home),
                        r.candidate.note
                    );
                }
            } else if args.json {
                println!("{}", pick_json(args.cli, cfg.job_cost_pct, &pick));
            } else if let Some(i) = pick.chosen {
                println!("{}", shown(&pick.ranked[i].candidate.home));
            }
            Ok(())
        }
        Err(err) => {
            if args.json {
                println!("{}", error_json(args.cli, cfg.job_cost_pct, &err));
            }
            let from_codex_pick = std::env::var_os(CALLER_ENV).is_some_and(|v| v == "codex-pick");
            match &err {
                PickError::NoUsable { .. } if from_codex_pick => {
                    eprintln!("{CODEX_PICK_NO_ACCOUNT}")
                }
                _ => eprintln!("amplihack account pick: {err}"),
            }
            Err(crate::command_error::exit_error(1))
        }
    }
}

#[cfg(test)]
mod test_fixtures;
#[cfg(test)]
mod tests_config;
#[cfg(test)]
mod tests_credentials;
#[cfg(test)]
mod tests_launch;
#[cfg(test)]
mod tests_lease;
#[cfg(all(test, target_os = "linux"))]
mod tests_pick_concurrency;
#[cfg(all(test, target_os = "linux"))]
mod tests_procscan;
#[cfg(test)]
mod tests_rank;
