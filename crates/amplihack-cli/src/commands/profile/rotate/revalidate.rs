//! Step 6 of a rotate cycle (issue #1721): under the lock, compare a fresh
//! snapshot with the one the cycle measured from, on only what the decision
//! depends on, and reconsider a swap once when only its target changed. See
//! "Rotate revalidation" in docs/reference/profile-command.md.
//!
//! Nothing here writes: no file, ledger or log line. The cycle acts on the
//! [`Verdict`].

use super::cycle::Ctx;
use super::decide::Decision;
use super::identify::Skip;
use super::observe::{Measurement, Snap, decide_measured, snapshot};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{State, exists};
use crate::commands::profile::name::ProfileName;

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
        }
    }
}

/// A target's reason without the `target eligibility: ` prefix.
fn target_reason(why: Option<Skip>) -> &'static str {
    why.map_or("gone", Skip::as_str)
}

/// What changed between `first` and `fresh` that `decision` depends on.
/// Other profiles joining or leaving the candidates, and whether sessions
/// can be seen, are not compared: losing sight of sessions makes every
/// profile `session running`, which a swap's target rule already catches.
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
        home_no_oauth,
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
    if *overflow != fresh.overflow || *ignored != fresh.ignored {
        why.push(Changed::Overflow);
    }
    if let Decision::Swap { to, .. } = decision
        && !fresh.candidates.contains(to)
    {
        let skip = fresh.skips.iter().find(|(n, _)| n == to).map(|(_, s)| *s);
        why.push(Changed::Target(skip));
    }
    why
}

/// What step 6 does with the measurement.
pub enum Verdict {
    /// Nothing the decision depends on changed.
    Proceed,
    /// The target left the candidates, and a re-decision on the same probes
    /// chose another fresh candidate. `m` replaces the measurement, `fresh`
    /// the snapshot, and `line` is the [`reconsidered`] log line.
    Reconsidered {
        m: Box<Measurement>,
        fresh: Box<Snap>,
        line: String,
    },
    /// Skip the cycle; the log line names what changed.
    Skip(Vec<Changed>),
}

/// Revalidates `m`, decided on `first`, under the lock the caller holds.
/// `failed_before` is the count of failed live probes before this cycle's,
/// so a re-decision does not count the live probe twice. Probes nothing:
/// a swap is reconsidered once, on the probes already taken, limited to the
/// fresh candidates, and only when its target is all that changed. The one
/// attempt is made in line, with no loop, so it cannot repeat.
pub fn check(
    ctx: &mut Ctx,
    state: &State,
    first: &Snap,
    m: &Measurement,
    failed_before: u32,
) -> Result<Verdict, ProfileError> {
    let journal = exists(&ctx.rs.journal);
    let fresh = snapshot(ctx, state)?;
    let why = changed(first, &fresh, &m.decision, journal);
    let (old, target) = match (&m.decision, why.as_slice()) {
        (_, []) => return Ok(Verdict::Proceed),
        (Decision::Swap { to, .. }, [Changed::Target(t)]) => (to.clone(), *t),
        _ => return Ok(Verdict::Skip(why)),
    };
    // A stub that built the measurement by hand kept no probes: no retry.
    let Some((probe, probes)) = &m.probed else {
        return Ok(Verdict::Skip(why));
    };
    let probes = probes
        .iter()
        .filter(|(n, _)| fresh.candidates.contains(n))
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
        Decision::Swap { to, .. } if fresh.candidates.contains(to) => Ok(Verdict::Reconsidered {
            m: Box::new(retry),
            fresh: Box::new(fresh),
            line: reconsidered(&old, target_reason(target)),
        }),
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

/// A swap's target left the candidates and the re-decision chose another.
/// `reason` is a bare [`Skip::as_str`] or `gone`.
pub fn reconsidered(old: &ProfileName, reason: &str) -> String {
    format!("revalidate: target {old} no longer eligible ({reason}); reconsidered")
}
