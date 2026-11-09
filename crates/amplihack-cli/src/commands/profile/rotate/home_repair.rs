//! Issue #2013 (D8, D9, D14): the per-cycle repair of the `oauthAccount`
//! block in `~/.claude.json`.
//!
//! Claude Code shows the account that block names, and a running session
//! writes the whole file back from memory, block included. The block never
//! decides whose login is in `~/.claude`; this step only keeps it in line
//! with the verified identity, and only when the evidence agrees:
//!
//! - the ledger names the live profile `<p>` and the cycle has no identity
//!   refusal;
//! - the server-verified identity of the login in `~/.claude` is known, the
//!   ledger owns it for `<p>` (`swap_authorize::ledger_owns`), and the
//!   record for `<p>` names it;
//! - `~/.claude.json` is absent or parses as a JSON object.
//!
//! Then a missing block, or one naming another account, is rewritten from
//! the verified account (`swap_finish::write_home_account`) and the cycle
//! goes on. Anything doubtful is [`Repair::NotApplicable`] and writes
//! nothing; a write that is refused or fails is [`Repair::Refused`]. The step makes no identity request of its own: it uses the
//! snapshot's reading. See "Rotate home account repair" in
//! docs/reference/profile-command.md.

use super::super::alert_limit::{Note, note};
use super::super::identify::{Account, LiveId};
use super::super::identity_claims;
use super::super::log::Sink;
use super::super::observe::Snap;
use super::super::swap_authorize::ledger_owns;
use super::super::swap_finish::write_home_account;
use super::Ctx;
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{CLAUDE_JSON, State, exists, read_claude_json};

/// What [`step`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Repair {
    /// The block already names the verified account; nothing was written.
    InPlace,
    /// The block named `old` (an e-mail address, or `none`) and was
    /// rewritten to `new`, or, read-only, would have been.
    Rewritten { old: String, new: String },
    /// The evidence does not agree, or the file cannot be used: nothing was
    /// written.
    NotApplicable,
    /// The evidence agreed but the write was refused or failed; nothing was
    /// written.
    Refused,
}

/// The repair line, `repair: ...; rewritten`.
pub fn line(old: &str, new: &str) -> String {
    format!("repair: home account metadata named {old}, verified identity is {new}; rewritten")
}

/// The line for a refused or failed write, `repair refused: ...`.
pub fn refused_line(e: &dyn std::fmt::Display) -> String {
    format!("repair refused: ~/.claude.json was not rewritten: {e}")
}

/// The `--dry-run` line.
pub fn would(old: &str, new: &str) -> String {
    format!("would repair: home account metadata named {old}, verified identity is {new}")
}

/// What the repair would change, without writing: `None` when it does not
/// apply, `Some(None)` when the block is right, `Some(Some((old, new)))`
/// when it would be rewritten.
pub fn plan(
    ctx: &Ctx,
    state: &State,
    snap: &Snap,
) -> Result<Option<Option<(String, String)>>, ProfileError> {
    let (LiveId::Known(p), None, Some(verified)) =
        (&snap.live, &snap.identity_refusal, &snap.trusted_live)
    else {
        return Ok(None);
    };
    if snap.platform_live() {
        return Ok(None);
    }
    let Some(ledger) = ctx.rs.load_ledger()? else {
        return Ok(None);
    };
    if !ledger_owns(&ledger, p, verified) {
        return Ok(None);
    }
    match identity_claims::record(state, p) {
        Ok(Some(rec)) if rec.names(verified) => {}
        _ => return Ok(None),
    }
    let block = match read_claude_json(&ctx.home) {
        Some((v, _)) => v.get("oauthAccount").cloned(),
        None if !exists(&ctx.home.join(CLAUDE_JSON)) => None,
        // A file that does not parse is never rewritten.
        None => return Ok(None),
    };
    let current = block
        .as_ref()
        .filter(|b| !b.is_null())
        .map(|b| Account::from_block(b).ok_or(()));
    let old = match &current {
        // The same account: Claude's other fields in the block are kept.
        Some(Ok(a)) if a.same(verified) => return Ok(Some(None)),
        Some(Ok(a)) => a.email_or_placeholder(),
        Some(Err(())) => "unreadable".to_string(),
        None => "none".to_string(),
    };
    Ok(Some(Some((old, verified.email_or_placeholder()))))
}

/// Runs the repair for this cycle. The `repair:` line goes through the
/// alert limit under `repair:<p>`, so a session that writes the old block
/// back on every cycle is repaired every cycle and logged once an hour.
pub fn step(ctx: &Ctx, state: &State, snap: &Snap, out: &mut Sink) -> Result<Repair, ProfileError> {
    // Issue #2013 (D14): what left the block stale on candide on 6 November
    // is drawn from the code, not from the host's evidence files: in
    // 1.2.97 `s3` copied the block from the target profile's own
    // `.claude.json`, which could still name the previous account, and a
    // running Claude Code session can write its cached block back at any
    // time. `s3` now writes from the verified identity; this step is the
    // backstop for the second cause.
    let Some(planned) = plan(ctx, state, snap)? else {
        return Ok(Repair::NotApplicable);
    };
    let Some((old, new)) = planned else {
        return Ok(Repair::InPlace);
    };
    let (LiveId::Known(p), Some(verified)) = (&snap.live, &snap.trusted_live) else {
        return Ok(Repair::NotApplicable);
    };
    // The ledger is loaded before anything is written, so a ledger gone
    // since `plan` writes nothing rather than saving a default one.
    let Some(mut l) = ctx.rs.load_ledger()? else {
        return Ok(Repair::NotApplicable);
    };
    let t = ctx.now();
    if ctx.read_only {
        out.emit(t, &would(&old, &new));
        return Ok(Repair::Rewritten { old, new });
    }
    if let Err(e) = write_home_account(&ctx.home, verified) {
        // Refused (a symlink, a file of another user) or failed: nothing
        // was written, and the swap checks still apply this cycle. The
        // line is limited under its own key, so a symlinked file is
        // reported once an hour, not every cycle.
        let text = refused_line(&e);
        match note(&mut l.alert_log, &format!("repair-refused:{p}"), t, &text) {
            Note::Log(text) => out.emit(t, &text),
            Note::Suppress => out.print(t, &text),
        }
        ctx.rs.save_ledger(&l)?;
        return Ok(Repair::Refused);
    }
    let text = line(&old, &new);
    match note(&mut l.alert_log, &format!("repair:{p}"), t, &text) {
        Note::Log(text) => out.emit(t, &text),
        Note::Suppress => out.print(t, &text),
    }
    ctx.rs.save_ledger(&l)?;
    Ok(Repair::Rewritten { old, new })
}
