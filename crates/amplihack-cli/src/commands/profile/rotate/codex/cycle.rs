//! One Codex cycle, run once after each Claude cycle: list, read live once
//! with no lock held, write the `codex cycle` line, then swap, refuse or
//! recover. It never changes the Claude cycle, its outcome, the rotate
//! ledger or the exit status.

use super::super::ledger::RotateState;
use super::super::log::Sink;
use super::super::settings::Settings;
use super::decide::{DecideInput, Decision, NoneReason, decide};
use super::link::{LinkState, inspect};
use super::state::load;
use super::swap::link_refusal;
use super::{Hooks, find_codex_bin, lines, recover, swap};
use crate::commands::profile::ProfileError;
use amplihack_utils::codex_capacity::{HOME_LABEL, list_accounts, profiles_root, timeout_from_env};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// The deadline warning is written once per process.
static TIMEOUT_WARNED: AtomicBool = AtomicBool::new(false);

/// What a Codex cycle works with.
pub(in crate::commands::profile::rotate) struct CodexCtx<'a> {
    /// The Claude rotate state: its directory holds `codex-state.json` and
    /// the Codex lock, its log the Codex lines.
    pub rs: &'a RotateState,
    pub home: &'a Path,
    pub settings: Settings,
    /// The default Codex home link.
    pub link: &'a Path,
    /// `--dry-run`: no lock, no state written, no link changed.
    pub read_only: bool,
    pub hooks: &'a Hooks,
}

/// What the swap and recovery share with the cycle.
pub(super) struct Env {
    /// The canonical profiles root.
    pub root_c: PathBuf,
    /// The Codex program found.
    pub bin: PathBuf,
}

/// Runs one Codex cycle; an error is written as one `skip:` line.
pub(in crate::commands::profile::rotate) fn run(ctx: &CodexCtx<'_>, out: &mut Sink) {
    if let Err(e) = cycle(ctx, out) {
        out.emit((ctx.hooks.now)(), &format!("skip: codex rotation: {e}"));
    }
}

fn cycle(ctx: &CodexCtx<'_>, out: &mut Sink) -> Result<(), ProfileError> {
    let now = || (ctx.hooks.now)();
    let Some(root) = profiles_root(Some(ctx.home)) else {
        out.emit(now(), &lines::skip("no-accounts"));
        return Ok(());
    };
    let mut accounts = list_accounts(&root, Some(ctx.home)).map_err(|e| {
        ProfileError::Io(std::io::Error::new(
            e.kind(),
            format!(
                "cannot list Codex accounts under {}: {e}",
                lines::clean_path(&root)
            ),
        ))
    })?;
    if accounts.is_empty() {
        out.emit(now(), &lines::skip("no-accounts"));
        return Ok(());
    }
    let Some(bin) = find_codex_bin() else {
        out.emit(now(), &lines::skip("no-codex-bin"));
        return Ok(());
    };
    // The root was just listed, so this resolves; if it has since gone, the
    // raw path matches no link target and every link rule refuses.
    let root_c = std::fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
    let link_state = inspect(ctx.link, &root_c);
    let live = match &link_state {
        LinkState::Link { label, .. } => {
            // The live account is read once, under its own label. With a
            // link in place `~/.codex` is either that link (the live
            // account) or, when another link is configured, a directory
            // outside the root that a swap must refuse; it is never a swap
            // target, so it is not read or ranked. Left in, it could rank
            // first and block every swap as `link-outside-root`.
            accounts.retain(|a| a.label != HOME_LABEL);
            Some(label.clone())
        }
        _ => None,
    };
    let (secs, warning) = timeout_from_env();
    if let Some(w) = warning
        && !TIMEOUT_WARNED.swap(true, Ordering::Relaxed)
    {
        eprintln!("{w}");
    }
    let rows = (ctx.hooks.read)(bin.as_os_str(), &accounts, Duration::from_secs(secs));
    let state = load(&ctx.rs.dir);
    let st = state.as_ref().ok();
    let floor = f64::from(ctx.settings.codex_floor);
    let decision = decide(&DecideInput {
        rows: &rows,
        live: live.as_deref(),
        floor,
        journal_open: st.is_some_and(|s| s.journal.is_some()),
        last_swap_at: st.and_then(|s| s.last_swap_at),
        now: now(),
        hold: ctx.settings.hold,
        margin: ctx.settings.margin,
    });
    out.emit(
        now(),
        &lines::cycle(live.as_deref(), &rows, floor, now(), &decision.text()),
    );
    // An unreadable state file is reported every cycle; no swap, recovery
    // or other refusal runs until it is fixed.
    if let Err(e) = &state {
        swap::state_unreadable(e).emit(out, now());
        return Ok(());
    }
    let journal = st.and_then(|s| s.journal.clone());
    let env = Env { root_c, bin };
    match decision {
        Decision::Swap { from, to } => swap::run(ctx, out, &env, &from, &to)?,
        Decision::None(NoneReason::NoLive) => {
            if let Some(r) = link_refusal(&link_state) {
                r.emit(out, now());
            }
        }
        Decision::None(NoneReason::Recovering) => {
            if let Some(j) = journal {
                recover::run(ctx, out, &env, &j)?;
            }
        }
        Decision::Keep | Decision::None(_) => {}
    }
    Ok(())
}
