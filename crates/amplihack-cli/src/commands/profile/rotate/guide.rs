//! The hand-change guidance (issues #1693, #1886): one ALERT line and numbered
//! `fix` lines, worded as in docs/reference/profile-command.md, "Guidance".
//! The lines hold profile names, fixed text and quoted paths only; never an
//! account. Each line fits the ledger's `guidance` limits.

use super::handfix_msg::only_login;
use super::ledger::{MAX_GUIDANCE_LINE, MAX_GUIDANCE_LINES};
use super::log::{cut, sh_quote};
use crate::commands::profile::auto_switch::kit::CREDS;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::usage_cmd::sanitize_display;
use std::path::PathBuf;

/// The paths a `fix` line may name. `manual` is
/// `<root>/.rejected-logins/manual-<time>`.
pub struct Paths {
    pub home: PathBuf,
    pub root: PathBuf,
    pub ledger: PathBuf,
    pub manual: PathBuf,
}

/// The `fix` line for a refused recovery, by its reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fix {
    /// An earlier swap stopped half way.
    Recovery,
    Home,
    DotClaude,
    Root,
    /// A session runs on `M`.
    Close,
    /// Sessions cannot be seen.
    CloseThenMove,
    /// Claude's refresh lock.
    Wait,
    /// Leased or hosting.
    Lease,
    /// Move `M`'s copy by hand.
    ByHand,
    /// Nothing to do; the next cycle retries.
    Retry,
}

/// What the guidance is about.
pub enum Kind<'a> {
    /// `~/.claude.json` has had no readable account for at least
    /// `HOME_UNREADABLE_GRACE` seconds; `problem` says why, from
    /// `identify::home_problem` (#1702 review).
    HomeStuck { problem: &'a str },
    /// Issue #1886: `~/.claude` holds the same account as `m`'s own login
    /// on two readings, but `m` cannot be recorded without leaving the ledger's
    /// profile with no login anywhere (the only-login rule).
    OnlyLogin { m: &'a ProfileName },
    /// A confirmed recovery that a check refused.
    Refused {
        m: &'a ProfileName,
        reason: &'a str,
        fix: Fix,
    },
}

const HOME_WHAT: &str = "the login in ~/.claude";

/// A `fix` line, and what to move by hand when a path it needs cannot be
/// printed or the line is too long. `what: None`: the line names no path
/// and is cut instead.
struct FixLine {
    text: Option<String>,
    what: Option<String>,
}

fn plain(text: String) -> FixLine {
    FixLine {
        text: Some(text),
        what: None,
    }
}

/// A line built from quoted paths; `None` text when any cannot be quoted.
fn with_paths(what: &str, paths: &[PathBuf], f: impl Fn(&[String]) -> String) -> FixLine {
    let quoted: Option<Vec<String>> = paths.iter().map(|p| sh_quote(p)).collect();
    FixLine {
        text: quoted.map(|q| f(&q)),
        what: Some(what.to_string()),
    }
}

/// The ALERT line and the `fix` lines, at most 16, each at most 1024 bytes
/// with no control or bidirectional-override character.
pub fn lines(l: &ProfileName, kind: &Kind<'_>, p: &Paths) -> Vec<String> {
    let head = match kind {
        Kind::HomeStuck { problem } => home_stuck_head(l, problem),
        Kind::OnlyLogin { m } => only_login(m, l),
        Kind::Refused { m, reason, .. } => format!(
            "ALERT: the account in ~/.claude is not the one the ledger names (live={l}); profile {m} holds the same account and has its own login; recovery refused: {reason}; no swap"
        ),
    };
    let fixes = kind_fixes(l, kind, p);
    let mut out = vec![clean(&head)];
    for (i, f) in fixes.into_iter().enumerate() {
        let prefix = format!("fix {}: ", i + 1);
        // Never cut a line that names a path: a cut path is another path.
        let full = f
            .text
            .map(|t| sanitize_display(&format!("{prefix}{t}"), false));
        let line = match (full, f.what) {
            (Some(line), _) if line.len() <= MAX_GUIDANCE_LINE => line,
            (_, Some(what)) => {
                format!("{prefix}move {what} by hand; its path cannot be printed safely")
            }
            (Some(line), None) => cut(&line, MAX_GUIDANCE_LINE),
            (None, None) => continue,
        };
        out.push(line);
    }
    out.truncate(MAX_GUIDANCE_LINES);
    out
}

/// Sanitized and cut: only for lines that name no path.
fn clean(s: &str) -> String {
    cut(&sanitize_display(s, false), MAX_GUIDANCE_LINE)
}

/// The ALERT for a `~/.claude.json` that has stayed unreadable.
pub fn home_stuck_head(l: &ProfileName, problem: &str) -> String {
    format!(
        "ALERT: ~/.claude.json has had no account that can be read for 5 minutes or more (live={l}): {problem}; no swap until it is fixed; do not /login in ~/.claude before the login there is set aside, it would replace {l}'s login"
    )
}

fn kind_fixes(l: &ProfileName, kind: &Kind<'_>, p: &Paths) -> Vec<FixLine> {
    match kind {
        // The login in ~/.claude is L's as far as anyone knows: neither fix
        // replaces it. The first lets Claude write the file again; the
        // second gives L its login back before any `/login` (#1702 review).
        Kind::HomeStuck { .. } => {
            let json = p.home.join(".claude.json");
            let home_creds = p.home.join(".claude").join(CREDS);
            let l_dir = p.root.join(l.as_str());
            vec![
                with_paths(HOME_WHAT, std::slice::from_ref(&json), |q| {
                    format!(
                        "run claude with no CLAUDE_CONFIG_DIR and without /login, then quit it; if {} then holds an account, the next cycle goes on",
                        q[0]
                    )
                }),
                with_paths(
                    HOME_WHAT,
                    &[home_creds, l_dir.join(CREDS), l_dir, p.ledger.clone()],
                    |q| {
                        format!(
                            "or give {l} its login back and start over: close every claude session that uses ~/.claude, then run: test ! -e {1} && mv -n {0} {2}/ && rm {3}; then run claude with no CLAUDE_CONFIG_DIR and /login",
                            q[0], q[1], q[2], q[3]
                        )
                    },
                ),
            ]
        }
        // The server says `~/.claude` holds M's account (issue #1886). Either
        // a `/login` replaced L's only login, or L and M are two profiles of
        // one account and L has no record saying so (a record that names
        // the account would have made the finding Clear).
        Kind::OnlyLogin { m } => {
            let creds = p.root.join(m.as_str()).join(CREDS);
            vec![
                with_paths(
                    &format!("{m}'s copy"),
                    &[p.manual.clone(), creds, p.ledger.clone()],
                    |q| {
                        format!(
                            "if a /login in ~/.claude replaced {l}'s login: close every claude session that uses ~/.claude, then run: mkdir -m 700 {0} && mv -n {1} {0}/ && rm {2}; then run: amplihack profile login {l}",
                            q[0], q[1], q[2]
                        )
                    },
                ),
                plain(format!(
                    "if {l} and {m} are profiles of one account, run amplihack profile verify to see the account each folder holds, then remove one of the two profiles"
                )),
            ]
        }
        Kind::Refused { m, fix, .. } => vec![refused_fix(m, *fix, p)],
    }
}

fn refused_fix(m: &ProfileName, fix: Fix, p: &Paths) -> FixLine {
    let copy = format!("{m}'s copy");
    let creds = p.root.join(m.as_str()).join(CREDS);
    let mv = |q: &[String]| format!("mkdir -m 700 {1} && mv -n {0} {1}/", q[0], q[1]);
    match fix {
        Fix::Recovery => {
            plain("see the Recovery section of docs/howto/claude-profile-rotation.md".into())
        }
        Fix::Home => with_paths(HOME_WHAT, std::slice::from_ref(&p.home), |q| {
            format!("make {0} owned by you and run: chmod go-w {0}", q[0])
        }),
        Fix::DotClaude => with_paths(HOME_WHAT, &[p.home.join(".claude")], |q| {
            format!("make {0} owned by you and run: chmod 700 {0}", q[0])
        }),
        Fix::Root => with_paths(&copy, std::slice::from_ref(&p.root), |q| {
            format!("make {0} owned by you and run: chmod 700 {0}", q[0])
        }),
        Fix::Close => plain(format!(
            "close the claude sessions that use {m}; the next cycle retries"
        )),
        Fix::CloseThenMove => with_paths(&copy, &[creds, p.manual.clone()], |q| {
            format!(
                "close the claude sessions that use {m}, then run: {}",
                mv(q)
            )
        }),
        Fix::Wait => plain("wait; the next cycle retries".into()),
        Fix::Lease => {
            plain("wait for the auto-switch session to end; the next cycle retries".into())
        }
        Fix::ByHand => with_paths(&copy, &[creds, p.manual.clone()], |q| {
            format!("move {m}'s copy by hand: {}", mv(q))
        }),
        Fix::Retry => plain("none needed; the next cycle retries".into()),
    }
}
