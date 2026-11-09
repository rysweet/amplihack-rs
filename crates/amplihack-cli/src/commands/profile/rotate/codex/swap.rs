//! The Codex swap: under the Codex lock, check the link rules again, check
//! that no Codex process uses the default home, journal, relink, record.
//! Every refusal writes one `codex refuse` line and changes nothing.

use super::super::log::Sink;
use super::cycle::{CodexCtx, Env};
use super::inuse::{InUse, default_home_in_use};
use super::link::{Expect, LinkState, inspect, mismatch_detail, relink_checked, swap_tmp};
use super::state::{CodexJournal, CodexPhase, load, save, try_lock};
use super::{FaultPoint, account_of, bin_in_home, fault_stop, lines};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::euid;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

/// Why a swap was refused. The codes are fixed; scripts may match them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RefuseCode {
    HomeNotLink,
    LinkOutsideRoot,
    LinkDangling,
    JournalMismatch,
    DefaultHomeInUse,
    LockBusy,
    CodexBinInHome,
    ProcUnreadable,
}

/// Every refusal code, as documented in the reference's table.
pub(super) const DOCUMENTED: [&str; 8] = [
    "home-not-link",
    "link-outside-root",
    "link-dangling",
    "journal-mismatch",
    "default-home-in-use",
    "lock-busy",
    "codex-bin-in-home",
    "proc-unreadable",
];

impl RefuseCode {
    #[cfg(test)]
    pub(super) const ALL: [RefuseCode; 8] = [
        Self::HomeNotLink,
        Self::LinkOutsideRoot,
        Self::LinkDangling,
        Self::JournalMismatch,
        Self::DefaultHomeInUse,
        Self::LockBusy,
        Self::CodexBinInHome,
        Self::ProcUnreadable,
    ];

    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::HomeNotLink => DOCUMENTED[0],
            Self::LinkOutsideRoot => DOCUMENTED[1],
            Self::LinkDangling => DOCUMENTED[2],
            Self::JournalMismatch => DOCUMENTED[3],
            Self::DefaultHomeInUse => DOCUMENTED[4],
            Self::LockBusy => DOCUMENTED[5],
            Self::CodexBinInHome => DOCUMENTED[6],
            Self::ProcUnreadable => DOCUMENTED[7],
        }
    }
}

/// A refusal with its optional `detail=` word.
pub(super) struct Refusal(pub RefuseCode, pub Option<String>);

impl Refusal {
    pub(super) fn bare(code: RefuseCode) -> Self {
        Self(code, None)
    }

    pub(super) fn with(code: RefuseCode, detail: &str) -> Self {
        Self(code, Some(detail.to_string()))
    }

    pub(super) fn emit(&self, out: &mut Sink, now: i64) {
        out.emit(now, &lines::refuse(self.0.as_str(), self.1.as_deref()));
    }
}

/// The refusal for a `codex-state.json` that cannot be loaded, naming why:
/// `detail=state-unreadable:<why>`.
pub(super) fn state_unreadable(e: &std::io::Error) -> Refusal {
    let text = e.to_string();
    let why = text.strip_prefix("codex-state.json: ").unwrap_or(&text);
    Refusal(
        RefuseCode::JournalMismatch,
        Some(format!("state-unreadable:{why}")),
    )
}

/// The refusal for a link that is not a link to an account.
pub(super) fn link_refusal(state: &LinkState) -> Option<Refusal> {
    match state {
        LinkState::Link { .. } => None,
        LinkState::NotLink => Some(Refusal::bare(RefuseCode::HomeNotLink)),
        LinkState::Missing => Some(Refusal::with(RefuseCode::HomeNotLink, "missing")),
        LinkState::Unreadable => Some(Refusal::with(RefuseCode::HomeNotLink, "unreadable")),
        LinkState::OutsideRoot => Some(Refusal::bare(RefuseCode::LinkOutsideRoot)),
        LinkState::Dangling => Some(Refusal::bare(RefuseCode::LinkDangling)),
    }
}

/// Ours, a directory, and neither group- nor world-writable.
fn private_dir(p: &Path) -> bool {
    std::fs::metadata(p).is_ok_and(|m| m.is_dir() && m.uid() == euid() && m.mode() & 0o022 == 0)
}

/// The link rules beyond the link itself: the parent and the root are
/// private, the temp path is absent or a symlink, and the Codex program is
/// not inside the live account, the profiles root or the link.
fn rule_refusal(ctx: &CodexCtx<'_>, env: &Env, old_target: &Path) -> Option<Refusal> {
    let parent = ctx.link.parent().unwrap_or(Path::new("/"));
    if !private_dir(parent) {
        return Some(Refusal::with(
            RefuseCode::LinkOutsideRoot,
            "parent-not-private",
        ));
    }
    if !private_dir(&env.root_c) {
        return Some(Refusal::with(
            RefuseCode::LinkOutsideRoot,
            "root-not-private",
        ));
    }
    if let Ok(m) = std::fs::symlink_metadata(swap_tmp(ctx.link))
        && !m.file_type().is_symlink()
    {
        return Some(Refusal::with(RefuseCode::HomeNotLink, "swap-path-not-link"));
    }
    if bin_in_home(&env.bin, &env.root_c, ctx.link, Some(old_target)) {
        return Some(Refusal::bare(RefuseCode::CodexBinInHome));
    }
    match default_home_in_use(&ctx.hooks.proc_root, ctx.link, euid()) {
        InUse::Free => None,
        InUse::Pid(pid) => Some(Refusal::with(
            RefuseCode::DefaultHomeInUse,
            &format!("pid-{pid}"),
        )),
        // Fail closed: without a readable /proc no swap can be shown safe.
        InUse::ProcUnreadable => Some(Refusal::bare(RefuseCode::ProcUnreadable)),
    }
}

/// Swaps the link from the live account `from` to `to`, or writes the one
/// refusal; in a dry run, checks without the lock and writes
/// `dry-run would-swap` instead of swapping.
pub(super) fn run(
    ctx: &CodexCtx<'_>,
    out: &mut Sink,
    env: &Env,
    from: &str,
    to: &str,
) -> Result<(), ProfileError> {
    let now = (ctx.hooks.now)();
    let _lock = if ctx.read_only {
        None
    } else {
        ctx.rs.ensure()?;
        match try_lock(&ctx.rs.dir)? {
            Some(l) => Some(l),
            None => {
                Refusal::bare(RefuseCode::LockBusy).emit(out, now);
                return Ok(());
            }
        }
    };
    // Under the lock: the state and the link again.
    let mut state = match load(&ctx.rs.dir) {
        Ok(s) if s.journal.is_none() => s,
        Ok(_) => {
            Refusal::bare(RefuseCode::JournalMismatch).emit(out, now);
            return Ok(());
        }
        Err(e) => {
            state_unreadable(&e).emit(out, now);
            return Ok(());
        }
    };
    let (old_target, from_ino) = match inspect(ctx.link, &env.root_c) {
        LinkState::Link { target, label, ino } if label == from => (target, ino),
        _ => {
            Refusal::bare(RefuseCode::JournalMismatch).emit(out, now);
            return Ok(());
        }
    };
    if let Some(r) = rule_refusal(ctx, env, &old_target) {
        r.emit(out, now);
        return Ok(());
    }
    let Some((to_label, new_target)) = account_of(&env.root_c.join(to), &env.root_c) else {
        Refusal::bare(RefuseCode::LinkOutsideRoot).emit(out, now);
        return Ok(());
    };
    if ctx.read_only {
        out.emit(now, &lines::would_swap(from, &to_label));
        return Ok(());
    }
    let to_ino = std::fs::metadata(&new_target)?.ino();
    let mut journal = CodexJournal {
        from: from.to_string(),
        to: to_label.clone(),
        link: ctx.link.to_path_buf(),
        old_target: old_target.clone(),
        new_target: new_target.clone(),
        from_ino,
        to_ino,
        phase: CodexPhase::Prepared,
    };
    state.journal = Some(journal.clone());
    save(&ctx.rs.dir, &state, now)?;
    fault_stop(ctx.hooks.fault, FaultPoint::AfterJournal);
    let expect = Expect {
        old_target: &old_target,
        old_ino: from_ino,
        new_ino: to_ino,
    };
    if let Err(e) = relink_checked(ctx.link, &expect, &new_target, ctx.hooks.fault) {
        // The link or an account changed after the journal was written.
        // Nothing was renamed; the journal stays open and the next cycle's
        // recovery rolls it back or keeps it for a person to inspect.
        return match mismatch_detail(&e) {
            Some(detail) => {
                Refusal::with(RefuseCode::JournalMismatch, detail).emit(out, now);
                Ok(())
            }
            None => Err(e.into()),
        };
    }
    journal.phase = CodexPhase::Linked;
    state.journal = Some(journal);
    save(&ctx.rs.dir, &state, now)?;
    finish(ctx, &mut state, from, &to_label, true, now)?;
    out.emit(now, &lines::swap(from, &to_label, ctx.link));
    Ok(())
}

/// Closes a swap whose link points at the new account: the `change:` line
/// when `record`, `last_swap_at`, no journal.
pub(super) fn finish(
    ctx: &CodexCtx<'_>,
    state: &mut super::state::CodexState,
    from: &str,
    to: &str,
    record: bool,
    now: i64,
) -> Result<(), ProfileError> {
    if record {
        super::super::change_log::record(
            &ctx.rs.root,
            "codex_home",
            &lines::clean_label(from),
            &lines::clean_label(to),
        );
    }
    state.last_swap_at = Some(now);
    state.journal = None;
    save(&ctx.rs.dir, state, now)?;
    Ok(())
}
