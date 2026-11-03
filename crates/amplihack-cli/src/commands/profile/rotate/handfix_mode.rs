//! `--recovery log|off` for hand-change recovery (#1890), and the
//! `--dry-run` preview of the step in each mode. Split from `handfix.rs`,
//! which keeps the plan and the act path, to keep both within the line
//! budget. See "Rotate hand-change recovery" in
//! docs/reference/profile-command.md.
//!
//! Under `log` nothing is moved, created or recorded: a backup path is only
//! looked up with `first_free`, which checks names and creates nothing.

use super::backup::{BackupRefusal, first_free};
use super::guide::Fix;
use super::handfix::{Env, Plan, plan, refused_lines};
use super::handfix_msg as hmsg;
use super::ledger::{MAX_GUIDANCE_LINE, RotateState};
use super::log::{cut, msg};
use super::settings::Recovery;
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::CREDS;
use crate::commands::profile::login_check::REJECTED_DIR;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use std::path::Path;

/// The line `--recovery log` writes in place of the recovery `plan` would
/// make, with the profile the ledger names; `None` when the plan is
/// guidance, which `log` writes as `act` does.
pub(super) fn logged<'p>(
    root: &Path,
    plan: &'p Plan<'_>,
    t: i64,
) -> Option<(String, &'p ProfileName)> {
    match plan {
        Plan::Record { case, m, .. } if *m == case.l => Some((hmsg::log_relogin(m), &case.l)),
        Plan::Record { case, m, .. } => Some((hmsg::log_record(m, &case.l), &case.l)),
        Plan::Move { case, m, .. } => {
            let to = first_free(&root.join(REJECTED_DIR), m, t).map(|d| d.join(CREDS));
            Some((hmsg::log_moved(m, to.as_deref()), &case.l))
        }
        Plan::Guide(..) => None,
    }
}

/// The `--dry-run` lines under `act`: `would recover: ...`, or
/// `would alert: ...` and the `fix` lines; empty without a hand change.
/// Takes no lock and writes nothing. `env_for` is called only after a hand
/// change. Dry run calls [`preview_mode`]; this is the `act` form.
#[cfg(test)]
pub fn preview<'a>(
    store: &ProfileStore,
    rs: &RotateState,
    home: &Path,
    env_for: impl FnOnce() -> Env<'a>,
    t: i64,
) -> Result<Vec<String>, ProfileError> {
    preview_mode(store, rs, home, Recovery::Act, env_for, t)
}

/// [`preview`] under `mode`. `off` gives nothing and calls nothing; `log`
/// gives the log-only line and the `would alert:` line the cycle then
/// writes, or the `act` lines for guidance.
pub fn preview_mode<'a>(
    store: &ProfileStore,
    rs: &RotateState,
    home: &Path,
    mode: Recovery,
    env_for: impl FnOnce() -> Env<'a>,
    t: i64,
) -> Result<Vec<String>, ProfileError> {
    if mode == Recovery::Off {
        return Ok(Vec::new());
    }
    let would = |line: String| {
        let text = line.strip_prefix("recovered: ").unwrap_or(&line);
        vec![format!("would recover: {text}")]
    };
    let since = rs.load_ledger()?.and_then(|l| l.home_unreadable_since);
    let (Some(plan), _) = plan(store, rs, home, since, env_for, t)? else {
        return Ok(Vec::new());
    };
    if mode == Recovery::Log
        && let Some((line, l)) = logged(store.root(), &plan, t)
    {
        let alert = msg::hand_change(l);
        let text = alert.strip_prefix("ALERT: ").unwrap_or(&alert);
        let alert = format!("would alert: {text}");
        return Ok(vec![
            cut(&line, MAX_GUIDANCE_LINE),
            cut(&alert, MAX_GUIDANCE_LINE),
        ]);
    }
    let lines = match plan {
        Plan::Guide(lines, _) => lines,
        Plan::Record { line, .. } => return Ok(would(line)),
        Plan::Move { case, m, .. } => match first_free(&store.root().join(REJECTED_DIR), &m, t) {
            Some(dir) => return Ok(would(hmsg::recovered_moved(&m, &dir.join(CREDS)))),
            None => {
                let why = (BackupRefusal::NoFreeName.reason(), Fix::ByHand);
                refused_lines(&case, &m, true, why)
            }
        },
    };
    Ok(lines
        .into_iter()
        .enumerate()
        .map(|(i, l)| match (i, l.strip_prefix("ALERT: ")) {
            (0, Some(text)) => format!("would alert: {text}"),
            _ => l,
        })
        .collect())
}
