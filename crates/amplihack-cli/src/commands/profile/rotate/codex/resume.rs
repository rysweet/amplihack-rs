//! `amplihack profile codex-resume`: resume a Codex session that stopped on
//! a usage limit with `codex resume <id>`, on another account with room, or
//! after one wait for the earliest reset. No loop and no retry. See "Codex
//! resume command" in docs/reference/profile-command.md.

use super::super::ledger::RotateState;
use super::super::log::{RotateLog, open_or_warn};
use super::clock::Clock;
use super::limit::limit_hit_reading;
use super::session::{ensure_rollout, read_events, valid_session_id};
use super::state::{CodexState, ResumeRecord, load, save, try_lock};
use super::{Hooks, account_of, bin_in_home, find_codex_bin, lines};
use amplihack_utils::codex_capacity::{
    AccountResult, Outcome, list_accounts, next_choice, profiles_root, timeout_from_env,
};
use amplihack_utils::limit_resume_cap::MAX_RESUMES_PER_SESSION;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Exit statuses other than the child's.
const EXIT_FAILED: i32 = 1;
const EXIT_USAGE: i32 = 2;
const EXIT_CAPPED: i32 = 3;
const EXIT_NO_ROOM: i32 = 4;

/// The refusal when the Codex program is inside the profiles root or
/// under the link.
const BIN_IN_HOME: &str = "codex-bin-in-home";

/// The longest the one wait for a reset may last: 7 days, the length of
/// the weekly window. A later reset time is cut to this.
pub(super) const MAX_WAIT_SECS: i64 = 7 * 86_400;

/// What the no-room path does with the earliest reset time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Wait {
    /// No reading gave a reset time: stop with `no-reset-known`.
    Unknown,
    /// The reset is now or past: read again at once, with no sleep.
    Now,
    /// Sleep once until this epoch second, then read again.
    Until(i64),
}

/// The one wait for `earliest_reset`, seen at `now`: never into the past
/// and never longer than [`MAX_WAIT_SECS`].
pub(super) fn wait_plan(earliest_reset: Option<i64>, now: i64) -> Wait {
    match earliest_reset {
        None => Wait::Unknown,
        Some(reset) if reset <= now => Wait::Now,
        Some(reset) => Wait::Until(reset.min(now.saturating_add(MAX_WAIT_SECS))),
    }
}

/// The command's arguments.
pub(in crate::commands::profile::rotate) struct ResumeRequest {
    pub session: Option<String>,
    pub events: Option<PathBuf>,
    pub from_home: Option<PathBuf>,
    /// Passed to `codex resume` after the id; only their number is logged.
    pub extra: Vec<OsString>,
}

/// Why the command stopped before `codex resume` ran.
enum Fail {
    /// A usage or configuration error: status 2.
    Usage(String),
    /// Anything else: status 1.
    Other(String),
}

fn other(what: &str) -> impl FnOnce(std::io::Error) -> Fail + '_ {
    move |e| Fail::Other(format!("{what}: {e}"))
}

/// Resume lines go to `rotate.log` and to stderr.
struct ResumeLog(Option<RotateLog>);

impl ResumeLog {
    fn line(&mut self, now: i64, msg: &str) {
        if let Some(log) = &mut self.0
            && let Err(e) = log.write(now, msg)
        {
            eprintln!("warning: could not write the rotate log: {e}");
        }
        eprintln!("{msg}");
    }
}

/// Runs the command and returns its exit status.
pub(in crate::commands::profile::rotate) fn cli(
    rs: &RotateState,
    home: &Path,
    floor: u32,
    link: &Path,
    req: ResumeRequest,
    hooks: &Hooks,
    clock: &dyn Clock,
) -> i32 {
    match resume(rs, home, f64::from(floor), link, &req, hooks, clock) {
        Ok(code) => code,
        Err(Fail::Usage(m)) => {
            eprintln!("error: {m}");
            EXIT_USAGE
        }
        Err(Fail::Other(m)) => {
            eprintln!("error: {m}");
            EXIT_FAILED
        }
    }
}

/// The session id from `--session`, or from the events file, which must
/// also show a usage limit.
fn session_id(req: &ResumeRequest, refused: &[&Path]) -> Result<String, Fail> {
    let id = match (&req.session, &req.events) {
        (Some(s), _) => s.clone(),
        (None, Some(p)) => {
            let scan = read_events(p, refused).map_err(|e| Fail::Usage(e.to_string()))?;
            if !scan.limit_hit {
                return Err(Fail::Usage(
                    "codex-resume: the events file shows no usage limit".into(),
                ));
            }
            scan.session.ok_or_else(|| {
                Fail::Usage("codex-resume: the events file names no session id".into())
            })?
        }
        (None, None) => {
            return Err(Fail::Usage(
                "codex-resume: give --session <id> or --events <file>".into(),
            ));
        }
    };
    if !valid_session_id(&id) {
        return Err(Fail::Usage(
            "codex-resume: a session id must match ^[A-Za-z0-9][A-Za-z0-9-]{0,63}$".into(),
        ));
    }
    Ok(id)
}

/// The label of the account to resume on: among signed-in rows other than
/// `exclude` whose reading is not limited, the next choice.
fn choose(rows: &[AccountResult], exclude: Option<&str>, floor: f64) -> Option<String> {
    let open: Vec<AccountResult> = rows
        .iter()
        .filter(|r| Some(r.label.as_str()) != exclude)
        .filter(|r| matches!(&r.outcome, Outcome::Ok(reading) if !limit_hit_reading(reading)))
        .cloned()
        .collect();
    next_choice(&open, floor).map(|i| open[i].label.clone())
}

/// The earliest reset over the signed-in rows, each row's being the
/// `resetsAt` of its fullest window.
fn earliest_reset(rows: &[AccountResult]) -> Option<i64> {
    rows.iter()
        .filter_map(|r| match &r.outcome {
            Outcome::Ok(reading) => reading
                .windows
                .iter()
                .max_by(|a, b| {
                    let p = |w: &amplihack_utils::codex_capacity::Window| {
                        w.used_percent.unwrap_or(f64::MIN)
                    };
                    p(a).total_cmp(&p(b))
                })
                .and_then(|w| w.resets_at),
            _ => None,
        })
        .min()
}

/// Appends this resume under the lock, unless the session is capped.
/// `false` when capped.
fn record(state: &mut CodexState, id: &str, from: &str, to: &str, now: i64) -> bool {
    if state.resumes_in_window(id, now) >= MAX_RESUMES_PER_SESSION {
        return false;
    }
    state.resumes.push(ResumeRecord {
        session: id.to_string(),
        at: now,
        from: from.to_string(),
        to: to.to_string(),
    });
    true
}

#[allow(clippy::too_many_arguments)]
fn resume(
    rs: &RotateState,
    home: &Path,
    floor: f64,
    link: &Path,
    req: &ResumeRequest,
    hooks: &Hooks,
    clock: &dyn Clock,
) -> Result<i32, Fail> {
    let root = profiles_root(Some(home))
        .ok_or_else(|| Fail::Usage("codex-resume: no Codex profiles root".into()))?;
    let root_c = std::fs::canonicalize(&root).map_err(|e| {
        Fail::Usage(format!(
            "codex-resume: the Codex profiles root {}: {e}",
            lines::clean_path(&root)
        ))
    })?;
    let link_target = std::fs::canonicalize(link).ok();
    let mut refused: Vec<&Path> = vec![&root_c];
    refused.extend(link_target.as_deref());
    let id = session_id(req, &refused)?;
    let from_path = req
        .from_home
        .clone()
        .or_else(|| {
            std::env::var_os("CODEX_HOME")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        })
        .unwrap_or_else(|| link.to_path_buf());
    let (from, _) = account_of(&from_path, &root_c).ok_or_else(|| {
        Fail::Usage(format!(
            "codex-resume: --from-home {} is not an account directory under {}",
            lines::clean_path(&from_path),
            lines::clean_path(&root_c)
        ))
    })?;
    let bin = find_codex_bin()
        .ok_or_else(|| Fail::Other("codex-resume: the codex program was not found".into()))?;
    rs.ensure()
        .map_err(|e| Fail::Other(format!("codex-resume: {e}")))?;
    let mut log = ResumeLog(open_or_warn(&rs.log, &rs.log1));
    // EXEC-1: a program inside an account or under the link would be
    // chosen by the account, not by the user.
    if bin_in_home(&bin, &root_c, link, link_target.as_deref()) {
        log.line(clock.now(), &lines::resume_none(BIN_IN_HOME, &id));
        return Err(Fail::Usage(format!(
            "codex-resume: refused reason={BIN_IN_HOME}"
        )));
    }
    let (secs, warning) = timeout_from_env();
    if let Some(w) = warning {
        eprintln!("{w}");
    }
    // Only accounts that are themselves a directory under the root: a
    // symlink that points elsewhere, or an alias of another account, is
    // never read, chosen or used as CODEX_HOME.
    let accounts: Vec<_> = list_accounts(&root_c, None)
        .map_err(other("codex-resume: cannot list Codex accounts"))?
        .into_iter()
        .filter(|a| account_of(&root_c.join(&a.label), &root_c).is_some_and(|(l, _)| l == a.label))
        .collect();
    let read = || (hooks.read)(bin.as_os_str(), &accounts, Duration::from_secs(secs));

    // A capped session reads nothing. The cap is checked again under the
    // lock, with the record.
    let state = load(&rs.dir).map_err(other("codex-resume"))?;
    if state.resumes_in_window(&id, clock.now()) >= MAX_RESUMES_PER_SESSION {
        log.line(clock.now(), &lines::resume_none("capped", &id));
        return Ok(EXIT_CAPPED);
    }
    // Read with no lock held; the lock is never held while reading or
    // waiting.
    let rows = read();
    let (to, after_reset) = match choose(&rows, Some(&from), floor) {
        Some(to) => (to, false),
        None => {
            match wait_plan(earliest_reset(&rows), clock.now()) {
                Wait::Unknown => {
                    log.line(clock.now(), &lines::resume_none("no-reset-known", &id));
                    return Ok(EXIT_NO_ROOM);
                }
                Wait::Now => {}
                Wait::Until(deadline) => clock.sleep_until(deadline),
            }
            let Some(to) = choose(&read(), None, floor) else {
                log.line(clock.now(), &lines::resume_none("no-room-after-reset", &id));
                return Ok(EXIT_NO_ROOM);
            };
            (to, true)
        }
    };
    {
        let Some(_lock) = try_lock(&rs.dir).map_err(other("codex-resume: the Codex lock"))? else {
            log.line(clock.now(), &lines::resume_none("lock-busy", &id));
            return Ok(EXIT_FAILED);
        };
        let mut state = load(&rs.dir).map_err(other("codex-resume"))?;
        if !record(&mut state, &id, &from, &to, clock.now()) {
            log.line(clock.now(), &lines::resume_none("capped", &id));
            return Ok(EXIT_CAPPED);
        }
        save(&rs.dir, &state, clock.now()).map_err(other("codex-resume: codex-state.json"))?;
    }
    let canon = |label: &str| {
        account_of(&root_c.join(label), &root_c)
            .filter(|(l, _)| l == label)
            .map(|(_, p)| p)
            .ok_or_else(|| {
                Fail::Other(format!(
                    "codex-resume: account {} is no longer a directory under the root",
                    lines::clean_label(label)
                ))
            })
    };
    let (src, dst) = (canon(&from)?, canon(&to)?);
    ensure_rollout(&src, &dst, &id).map_err(other("codex-resume: the session's rollout"))?;
    log.line(
        clock.now(),
        &lines::resume(&id, &from, &to, after_reset, req.extra.len()),
    );
    let status = std::process::Command::new(&bin)
        .arg("resume")
        .arg(&id)
        .args(&req.extra)
        .env("CODEX_HOME", &dst)
        .env_remove("OPENAI_API_KEY")
        .env_remove("CODEX_API_KEY")
        .status()
        .map_err(other("codex-resume: could not start codex resume"))?;
    Ok(status.code().unwrap_or(EXIT_FAILED))
}
