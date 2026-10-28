//! The hand-change guidance (issue #1693): one ALERT line and numbered
//! `fix` lines, worded as in docs/reference/profile-command.md, "Guidance".
//! The lines hold profile names, fixed text and quoted paths only; never an
//! account. Each line fits the ledger's `guidance` limits.

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

/// Who holds the account, as far as the guidance needs it.
pub enum Kind<'a> {
    Many(&'a [ProfileName]),
    None,
    Unreadable(&'a [ProfileName]),
    HomeUnreadable,
    /// `~/.claude.json` has had no readable account for at least
    /// `HOME_UNREADABLE_GRACE` seconds; `problem` says why, from
    /// `identify::home_problem` (#1702 review).
    HomeStuck {
        problem: &'a str,
    },
    HomeNoLogin,
    Refused {
        m: &'a ProfileName,
        own_login: bool,
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

fn join(names: &[ProfileName]) -> String {
    names
        .iter()
        .map(ProfileName::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The ALERT line and the `fix` lines, at most 16, each at most 1024 bytes
/// with no control or bidirectional-override character. `l_lost`: the
/// profile the ledger names has no login file, so `fix 1` logs it in.
pub fn lines(l: &ProfileName, l_lost: bool, kind: &Kind<'_>, p: &Paths) -> Vec<String> {
    let middle = match kind {
        Kind::Many(v) => format!("it matches more than one profile ({})", join(v)),
        Kind::None => "no profile has that account".into(),
        Kind::Unreadable(v) => format!(
            "the account of profile {} cannot be read, so the match is uncertain",
            join(v)
        ),
        Kind::HomeUnreadable => "the account in ~/.claude.json cannot be read".into(),
        Kind::HomeStuck { .. } => String::new(),
        Kind::HomeNoLogin => "~/.claude has no login".into(),
        Kind::Refused {
            m,
            own_login,
            reason,
            ..
        } => {
            let own = if *own_login {
                " and has its own login"
            } else {
                ""
            };
            format!("profile {m} holds the same account{own}; recovery refused: {reason}")
        }
    };
    let head = match kind {
        Kind::HomeStuck { problem } => home_stuck_head(l, problem),
        _ => format!(
            "ALERT: the account in ~/.claude is not the one the ledger names (live={l}); {middle}; no swap"
        ),
    };
    let mut fixes = Vec::new();
    if l_lost {
        fixes.push(plain(format!("amplihack profile login {l}")));
    }
    fixes.extend(kind_fixes(l, kind, p));
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
        Kind::Many(v) => vec![plain(format!(
            "keep this account in one of {}: log the others in to another account or remove them; the next cycle then recovers",
            join(v)
        ))],
        Kind::None => {
            let creds = p.home.join(".claude").join(CREDS);
            vec![
                with_paths(HOME_WHAT, std::slice::from_ref(&p.ledger), |q| {
                    format!(
                        "rm {} (the next swap keeps the login in ~/.claude as profile default-original)",
                        q[0]
                    )
                }),
                with_paths(
                    HOME_WHAT,
                    &[creds, p.manual.clone(), p.ledger.clone()],
                    |q| {
                        format!(
                            "or, to set that login aside instead: close every claude session that uses ~/.claude, then run: mkdir -m 700 {1} && mv -n {0} {1}/ && rm {2}; then run claude with no CLAUDE_CONFIG_DIR and /login",
                            q[0], q[1], q[2]
                        )
                    },
                ),
            ]
        }
        Kind::Unreadable(v) => v
            .iter()
            .map(|n| {
                plain(format!(
                    "amplihack profile login {n}; the next cycle then recovers"
                ))
            })
            .collect(),
        // Never `/login` after one failed read: Claude rewrites
        // ~/.claude.json often, and a `/login` replaces the login there (#1702).
        Kind::HomeUnreadable => vec![plain(
            "none needed; the next cycle reads ~/.claude.json again".into(),
        )],
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
        Kind::HomeNoLogin => vec![plain(
            "run claude with no CLAUDE_CONFIG_DIR and /login".into(),
        )],
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
