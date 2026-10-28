//! Measuring: `claude -p /usage` for the live login and every candidate, in
//! parallel, with the `profile usage` probe. The live login is probed with
//! `CLAUDE_CONFIG_DIR` removed, so `claude` uses `~/.claude`. Claude's text
//! is parsed and dropped; only percentages, reset times and states leave
//! this module.

use super::decide::Probed;
use crate::binary_finder::BinaryInfo;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use crate::commands::profile::usage_cmd::{self, Probe, TIMEOUT_ENV};
use crate::commands::profile::{run, usage_parse::UsageState};
use std::time::Duration;

/// Whose login is in `~/.claude`.
pub enum Live<'a> {
    Profile(&'a ProfileName),
    /// No profile owns it yet.
    Unowned,
}

/// The reading for `p`. A failed probe counts as "swap needed", so its
/// cause goes to stderr (the daemon's `rotate.err`); the text is fixed and
/// never Claude's output.
fn reading(who: &str, p: &Probe) -> Probed {
    if p.error.is_some() || p.report.state == UsageState::Error {
        let why = p.error.map_or("unreadable /usage output", |e| e.as_str());
        eprintln!("warning: rotate: probe of {who} failed: {why}");
        return Probed::failed();
    }
    Probed::from_report(&p.report)
}

/// The probe deadline, as `profile usage` resolves it without a flag.
fn timeout() -> Duration {
    let (secs, _) = usage_cmd::timeout_secs(None, std::env::var_os(TIMEOUT_ENV).as_deref());
    Duration::from_secs(secs)
}

/// The live probe and each candidate's, in the order of `candidates`, with
/// reset times kept for exhausted readings too.
pub fn measure(
    store: &ProfileStore,
    binary: &BinaryInfo,
    live: Live<'_>,
    candidates: &[ProfileName],
    now: i64,
) -> (Probed, Vec<(ProfileName, Probed)>) {
    let timeout = timeout();
    // As `profile usage` warns: with one of these set, every probe may
    // report that credential, so the readings say nothing about the logins.
    if let Some(var) = usage_cmd::env_credential_var(|k| std::env::var_os(k)) {
        eprintln!("{}", usage_cmd::env_credential_warning(var));
    }
    let args = ["-p".to_string(), "/usage".to_string()];
    std::thread::scope(|s| {
        let live = s.spawn(|| {
            let cmd = match live {
                Live::Profile(name) => run::build_default_dir_command(binary, store, name, &args),
                Live::Unowned => Ok(run::build_home_command(binary, &args)),
            };
            match cmd {
                Ok(cmd) => reading("~/.claude", &usage_cmd::probe(cmd, timeout, now, false)),
                Err(e) => {
                    eprintln!("warning: rotate: probe of ~/.claude failed: {e}");
                    Probed::failed()
                }
            }
        });
        let probes = usage_cmd::probe_all(store, binary, candidates, None, timeout, now, false);
        let others = candidates
            .iter()
            .zip(probes.iter())
            .map(|(n, p)| (n.clone(), reading(n.as_str(), p)))
            .collect();
        // A panic in the probe thread is a bug; report the live login failed.
        (live.join().unwrap_or_else(|_| Probed::failed()), others)
    })
}
