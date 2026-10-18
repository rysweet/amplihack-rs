//! Measuring: `claude -p /usage` for the live login and every candidate, in
//! parallel, with the `profile usage` probe. The live login is probed with
//! `CLAUDE_CONFIG_DIR` removed, so `claude` uses `~/.claude`. Claude's text
//! is parsed and dropped; only percentages and states leave this module.

use super::decide::Reading;
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

fn reading(p: &Probe) -> Reading {
    if p.error.is_some() || p.report.state == UsageState::Error {
        return Reading::Failed;
    }
    Reading::from_report(&p.report)
}

/// The probe deadline, as `profile usage` resolves it without a flag.
fn timeout() -> Duration {
    let (secs, _) = usage_cmd::timeout_secs(None, std::env::var_os(TIMEOUT_ENV).as_deref());
    Duration::from_secs(secs)
}

/// The live reading and each candidate's, in the order of `candidates`.
pub fn measure(
    store: &ProfileStore,
    binary: &BinaryInfo,
    live: Live<'_>,
    candidates: &[ProfileName],
    now: i64,
) -> (Reading, Vec<(ProfileName, Reading)>) {
    let timeout = timeout();
    let args = ["-p".to_string(), "/usage".to_string()];
    std::thread::scope(|s| {
        let live = s.spawn(|| {
            let cmd = match live {
                Live::Profile(name) => run::build_default_dir_command(binary, store, name, &args),
                Live::Unowned => Ok(run::build_home_command(binary, &args)),
            };
            match cmd {
                Ok(cmd) => reading(&usage_cmd::probe(cmd, timeout, now, false)),
                Err(_) => Reading::Failed,
            }
        });
        let probes = usage_cmd::probe_all(store, binary, candidates, None, timeout, now, false);
        let others = candidates
            .iter()
            .cloned()
            .zip(probes.iter().map(reading))
            .collect();
        // A panic in the probe thread is a bug; report the live login failed.
        (live.join().unwrap_or(Reading::Failed), others)
    })
}
