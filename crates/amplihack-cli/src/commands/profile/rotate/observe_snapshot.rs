//! Trusted snapshot acquisition, separated from measurement and decisions.
use super::{CandidateInput, Ctx, LiveId, LoginKind, Skip, Snap, identify, kind, pin};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{CREDS, Liveness, State};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::overflow::{PLATFORM_TIER, tiers_detail};

pub fn snapshot(ctx: &Ctx, state: &State) -> Result<Snap, ProfileError> {
    let root = ctx.store.root();
    let live_read = ctx.identity_reader().oauth(&ctx.home.join(".claude"));
    let identity_cause = live_read.as_ref().err().copied();
    let trusted_live = live_read.ok();
    let mut identity_refusal = None;
    if let Some(l) = ctx.rs.load_ledger()?
        && let Some(raw) = l.live.as_deref()
    {
        let n = ProfileName::parse(raw)?;
        identity_refusal = match super::super::identity_claims::record(state, &n) {
            Ok(Some(rec)) if trusted_live.as_ref().is_some_and(|a| !rec.names(a)) => Some(format!(
                "{} {n}",
                super::super::swap_expect::ACCOUNT_MISMATCH
            )),
            Err(_) => Some(
                "trusted login identity: expected record is unreadable; repair it before retrying"
                    .into(),
            ),
            _ => None,
        };
    }
    let (live, proto) = identify::live_with(&ctx.store, &ctx.rs, &ctx.home, trusted_live.as_ref())?;
    let liveness = (ctx.scan)(root);
    let sees_sessions = matches!(liveness, Liveness::Known(_));
    let live_name = match &live {
        LiveId::Known(n) => Some(n.clone()),
        _ => None,
    };
    // Issue #2218: the shared reader, so a private file that is invalid for
    // other settings still excludes, and an unreadable or unsafe one
    // excludes every profile.
    let exclude = crate::commands::profile::usage_cmd::exclude_list(root, |_| {})?;
    let live_excluded = live_name.as_ref().is_some_and(|n| exclude.contains(n));
    let (overflow, ignored) = tiers_detail(&ctx.store, Some(&ctx.home));
    let input = CandidateInput {
        live: live_name.as_ref(),
        exclude,
        liveness,
        auto_switch: state.load_ledger()?,
    };
    let (mut candidates, mut skips) = identify::candidates(&ctx.store, &input)?;
    // Only the marked API-key profile the tier uses is ever a target: the
    // others share its credential, and an unmarked one has no consent to
    // per-token billing (issue #1804). None of them is probed or treated as
    // a plan login, so a synthetic 0/0 never counts as plan room.
    let kinds = kind::Ctx::load(&ctx.store, Some(&ctx.home));
    let platforms = kinds.platform_profiles(&ctx.store);
    let marked = overflow
        .iter()
        .find(|e| e.tier == PLATFORM_TIER)
        .map(|e| &e.name);
    let home_no_oauth =
        identify::login_kind(&ctx.home.join(".claude").join(CREDS)) == LoginKind::NoOAuth;
    // On an API-key profile, no other one is a target: they share one
    // credential, so such a move would change nothing but the name.
    let on_key = home_no_oauth && live_name.as_ref().is_some_and(|n| platforms.contains(n));
    candidates.retain(|c| !platforms.contains(c) || (marked == Some(c) && !on_key));
    let unmarked: Vec<ProfileName> = platforms
        .into_iter()
        .filter(|p| marked != Some(p) && !ignored.contains(p))
        .collect();
    let detected_mark = marked
        .filter(|n| kinds.resolve_kind(&ctx.store, n).source != kind::KindSource::Override)
        .cloned();
    let (candidate_accounts, mut candidate_refusals) =
        super::super::revalidate::accounts_with(ctx, &candidates);
    candidates.retain(|n| {
        let readable = marked == Some(n)
            || candidate_accounts
                .iter()
                .any(|(c, a)| c == n && a.is_some());
        if !readable {
            skips.push((n.clone(), Skip::NoAccount));
        }
        readable
    });
    candidate_refusals.retain(|(n, _)| !candidates.contains(n));
    Ok(Snap {
        account: trusted_live.as_ref().map(|a| a.key().to_string()),
        trusted_live,
        identity_refusal,
        identity_cause,
        candidate_refusals,
        candidate_accounts,
        live,
        proto,
        candidates,
        skips,
        sees_sessions,
        overflow,
        pin: pin::read(&ctx.rs),
        ignored,
        unmarked,
        detected_mark,
        home_no_oauth,
        live_excluded,
    })
}

impl Snap {
    /// Issue #2218: the live login when a profile owns it and it is not in
    /// the auto-switch `exclude` list (`live_excluded`, the
    /// `config::excluded_from` rule this snapshot applied). Every reader
    /// that decides on, records or re-reads the live login's usage takes its
    /// name from here, so an excluded live login is never read again.
    pub fn measured_live(&self) -> Option<&ProfileName> {
        match &self.live {
            LiveId::Known(n) if !self.live_excluded => Some(n),
            _ => None,
        }
    }
}
