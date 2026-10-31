//! Step 6 of a rotate cycle (issue #1721): under the lock, compare a fresh
//! snapshot with the one the cycle measured from, on only what the decision
//! depends on. Read the state once more when `~/.claude.json` held no
//! account, and reconsider a swap once when only its target changed. See
//! "Rotate revalidation" in docs/reference/profile-command.md.
//!
//! Nothing here writes: no file, ledger or log line. The cycle acts on the
//! [`Checked`] result.

use super::cycle::Ctx;
use super::decide::Decision;
use super::identify::{Account, Skip};
use super::observe::{Measurement, Snap, decide_measured, snapshot};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{State, exists};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use std::time::Duration;

/// How long step 6 waits, holding the lock, before it reads the state again
/// when `~/.claude.json` held no account. Running sessions rewrite that
/// file, and a read during a rewrite can find no account.
pub const REREAD_PAUSE: Duration = Duration::from_millis(500);

/// Logged when step 6 read the state again after [`REREAD_PAUSE`].
pub const REREAD: &str = "revalidate: ~/.claude.json held no account; read again";

/// What changed during measurement that the decision depends on. Holds no
/// profile name and no account, so its log label cannot leak either.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Changed {
    /// A rotate journal exists.
    Journal,
    /// The live login: `live`, `account`, `proto` or `home_no_oauth`.
    Live,
    Pin,
    /// The overflow entries or the API-key profiles ignored there.
    Overflow,
    /// A swap's target is no longer a candidate: why it is skipped, or
    /// `None` when it is in neither list.
    Target(Option<Skip>),
    /// A swap's target is still a candidate, but the account in its
    /// `.claude.json` is not the one it had when it was measured.
    TargetIdentity,
}

impl Changed {
    /// Fixed text, or a [`Skip::as_str`] reason; never free text.
    pub fn label(&self) -> String {
        match self {
            Self::Journal => "journal".into(),
            Self::Live => "live".into(),
            Self::Pin => "pin".into(),
            Self::Overflow => "overflow".into(),
            Self::Target(why) => format!("target eligibility: {}", target_reason(*why)),
            Self::TargetIdentity => "target identity".into(),
        }
    }

    /// The reason in the [`reconsidered`] line, for a target change only.
    fn retry_reason(&self) -> Option<&'static str> {
        match self {
            Self::Target(why) => Some(target_reason(*why)),
            Self::TargetIdentity => Some("account changed"),
            _ => None,
        }
    }
}

/// A target's reason without the `target eligibility: ` prefix.
fn target_reason(why: Option<Skip>) -> &'static str {
    why.map_or("gone", Skip::as_str)
}

/// The account in each candidate's `.claude.json`, for
/// [`Snap::candidate_accounts`]. Compared, never logged.
pub fn accounts(
    store: &ProfileStore,
    candidates: &[ProfileName],
) -> Vec<(ProfileName, Option<Account>)> {
    let of = |c: &ProfileName| {
        store
            .verify_profile_dir(c)
            .ok()
            .as_deref()
            .and_then(Account::of_dir)
    };
    candidates.iter().map(|c| (c.clone(), of(c))).collect()
}

/// Whether `name` is a candidate in `fresh` and holds the account it held
/// in `first`, by [`Account::same`]. A profile with no account in both
/// counts as unchanged, as an API-key profile may have none.
fn still_eligible(first: &Snap, fresh: &Snap, name: &ProfileName) -> bool {
    let of = |s: &Snap| {
        let found = s.candidate_accounts.iter().find(|(n, _)| n == name);
        found.and_then(|(_, a)| a.clone())
    };
    let same = match (of(first), of(fresh)) {
        (Some(a), Some(b)) => a.same(&b),
        (None, None) => true,
        _ => false,
    };
    fresh.candidates.contains(name) && same
}

/// What changed between `first` and `fresh` that `decision` depends on.
/// Other profiles joining or leaving the candidates, their accounts, and
/// whether sessions can be seen, are not compared: losing sight of sessions
/// makes every profile `session running`, which a swap's target rule
/// already catches.
pub fn changed(first: &Snap, fresh: &Snap, decision: &Decision, journal: bool) -> Vec<Changed> {
    // Every field is named, so a field added to `Snap` does not compile
    // until it is classified here.
    let Snap {
        live,
        proto,
        account,
        candidates: _,
        skips: _,
        sees_sessions: _,
        overflow,
        pin,
        ignored,
        unmarked,
        home_no_oauth,
        // A swap's target only, through `still_eligible` below.
        candidate_accounts: _,
    } = first;
    let mut why = Vec::new();
    if journal {
        why.push(Changed::Journal);
    }
    if *live != fresh.live
        || *proto != fresh.proto
        || *account != fresh.account
        || *home_no_oauth != fresh.home_no_oauth
    {
        why.push(Changed::Live);
    }
    if *pin != fresh.pin {
        why.push(Changed::Pin);
    }
    if *overflow != fresh.overflow || *ignored != fresh.ignored || *unmarked != fresh.unmarked {
        why.push(Changed::Overflow);
    }
    if let Decision::Swap { to, .. } = decision {
        if !fresh.candidates.contains(to) {
            let skip = fresh.skips.iter().find(|(n, _)| n == to).map(|(_, s)| *s);
            why.push(Changed::Target(skip));
        } else if !still_eligible(first, fresh, to) {
            why.push(Changed::TargetIdentity);
        }
    }
    why
}

/// What step 6 does with the measurement.
pub enum Verdict {
    /// Nothing the decision depends on changed.
    Proceed,
    /// The target changed, and a re-decision on the same probes chose
    /// another fresh candidate. `m` replaces the measurement, `fresh` the
    /// snapshot, and `line` is the [`reconsidered`] log line.
    Reconsidered {
        m: Box<Measurement>,
        fresh: Box<Snap>,
        line: String,
    },
    /// Skip the cycle; the log line names what changed.
    Skip(Vec<Changed>),
}

/// Step 6's result: whether the state was read twice, and the verdict on
/// the last read.
pub struct Checked {
    /// The cycle logs [`REREAD`] before acting on `verdict`.
    pub reread: bool,
    pub verdict: Verdict,
}

/// A fresh snapshot and what changed since `first`.
fn compare(
    ctx: &Ctx,
    state: &State,
    first: &Snap,
    decision: &Decision,
) -> Result<(Snap, Vec<Changed>), ProfileError> {
    let journal = exists(&ctx.rs.journal);
    let fresh = snapshot(ctx, state)?;
    let why = changed(first, &fresh, decision, journal);
    Ok((fresh, why))
}

/// `~/.claude.json` held an account in step 3 and none now, and nothing
/// else changed that a second read could not clear. A different account,
/// a journal, a pin or an overflow change is never read again.
fn home_blinked(first: &Snap, fresh: &Snap, why: &[Changed]) -> bool {
    first.account.is_some()
        && fresh.account.is_none()
        && why
            .iter()
            .all(|c| *c == Changed::Live || c.retry_reason().is_some())
}

/// Revalidates `m`, decided on `first`, under the lock the caller holds.
/// `failed_before` is the count of failed live probes before this cycle's,
/// so a re-decision does not count the live probe twice. Probes nothing.
/// Each retry is made at most once, in line, with no loop: the state is
/// read again after [`REREAD_PAUSE`] when `~/.claude.json` held no account,
/// and a swap is reconsidered on the probes already taken, limited to fresh
/// candidates with unchanged accounts, when only its target changed.
pub fn check(
    ctx: &mut Ctx,
    state: &State,
    first: &Snap,
    m: &Measurement,
    failed_before: u32,
) -> Result<Checked, ProfileError> {
    let (mut fresh, mut why) = compare(ctx, state, first, &m.decision)?;
    let reread = home_blinked(first, &fresh, &why);
    if reread {
        (ctx.pause)(ctx);
        (fresh, why) = compare(ctx, state, first, &m.decision)?;
    }
    let verdict = reconsider(ctx, first, fresh, why, m, failed_before)?;
    Ok(Checked { reread, verdict })
}

/// The verdict on `fresh`: proceed, skip, or one re-decision of a swap
/// whose target is the only change.
fn reconsider(
    ctx: &mut Ctx,
    first: &Snap,
    fresh: Snap,
    why: Vec<Changed>,
    m: &Measurement,
    failed_before: u32,
) -> Result<Verdict, ProfileError> {
    let (old, reason) = match (&m.decision, why.as_slice()) {
        (_, []) => return Ok(Verdict::Proceed),
        (Decision::Swap { to, .. }, [only]) => match only.retry_reason() {
            Some(reason) => (to.clone(), reason),
            None => return Ok(Verdict::Skip(why)),
        },
        _ => return Ok(Verdict::Skip(why)),
    };
    // A stub that built the measurement by hand kept no probes: no retry.
    let Some((probe, probes)) = &m.probed else {
        return Ok(Verdict::Skip(why));
    };
    let probes = probes
        .iter()
        .filter(|(n, _)| still_eligible(first, &fresh, n))
        .cloned()
        .collect();
    // Only the target changed, so the retry's `failed` equals the count
    // already stored; `cycle.rs` relies on this (see the `Reconsidered` arm).
    let counted = std::mem::replace(&mut ctx.failed_live, failed_before);
    let retry = decide_measured(ctx, &fresh, probe.clone(), probes, m.measured_at);
    ctx.failed_live = counted;
    let retry = retry?;
    debug_assert_eq!(
        retry.failed, m.failed,
        "a target-only retry must count failed live probes as the first decision did"
    );
    match &retry.decision {
        Decision::Swap { to, .. } if still_eligible(first, &fresh, to) => {
            Ok(Verdict::Reconsidered {
                m: Box::new(retry),
                fresh: Box::new(fresh),
                line: reconsidered(&old, reason),
            })
        }
        _ => Ok(Verdict::Skip(why)),
    }
}

/// `skip: state changed during measurement (<labels>)`; the bare
/// [`super::log::msg::CHANGED`] for no labels, which older readers match.
pub fn changed_line(why: &[Changed]) -> String {
    let text = super::log::msg::CHANGED;
    if why.is_empty() {
        return text.to_string();
    }
    let labels: Vec<String> = why.iter().map(Changed::label).collect();
    format!("{text} ({})", labels.join(", "))
}

/// A swap's target changed and the re-decision chose another. `reason` is
/// a bare [`Skip::as_str`], `gone`, or `account changed`.
pub fn reconsidered(old: &ProfileName, reason: &str) -> String {
    format!("revalidate: target {old} no longer eligible ({reason}); reconsidered")
}
