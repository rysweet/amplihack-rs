//! Every registry write the in-pane wrapper makes for its own session:
//! starting it, recording the Claude Code process the id thread found,
//! marking it ended, reading whether its owner ended it, and taking it
//! again for a relaunch (issue #2912).
//!
//! Split out of `wrapper.rs`, which now holds only the checks before the
//! launch and the loop around it.
//!
//! Nothing here widens a permission or removes a file: the ended markers
//! are only read, and every write happens under the registry's own lock,
//! and only while the entry is still this launch's.

use super::fields::Launch;
use super::markers;
use super::registry::{self, EndedBy, Entry, State};
use super::sessions;
use super::store::{self, AgentsDir};
use super::wrapper::Found;
use std::path::Path;

/// Creates the entry, or updates it in place keeping its restart history,
/// `registered_at` and `last_recovery` (D3).
pub(super) fn start(
    dir: &AgentsDir,
    name: &str,
    launch_id: &str,
    fields: Launch,
) -> anyhow::Result<()> {
    let now = registry::now();
    store::update(dir, now, |reg| {
        fields.check_conflicts(reg, name)?;
        match reg.agents.get_mut(name) {
            Some(e) => {
                e.activate(now, launch_id);
                fields.apply(e);
            }
            None => {
                let mut e = fields.into_entry(registry::fmt_ts(now));
                e.activate(now, launch_id);
                reg.agents.insert(name.to_string(), e);
            }
        }
        Ok(())
    })
}

/// The clean-exit write, only while the entry is this launch's and active.
pub(super) fn mark_ended(dir: &AgentsDir, name: &str, launch_id: &str) -> anyhow::Result<()> {
    let now = registry::now();
    store::update(dir, now, |reg| {
        if let Some(e) = reg.agents.get_mut(name)
            && e.state == State::Active
            && e.launch_id == launch_id
        {
            e.end(now, EndedBy::Wrapper);
        }
        Ok(())
    })
}

/// Whether the owner ended this session on purpose: its entry is already
/// `ended`, or a SessionEnd marker under `agents/ended/` says so.
///
/// The entry comes first because the supervisor's pass reads the same
/// marker, ends the entry and then deletes the marker. A wrapper that
/// looked only for the marker could arrive after that deletion and start
/// again a session its owner had ended.
///
/// Both conversations this launch has been known by are checked: the one
/// `--resume` named, recorded before Claude Code started, and the one the
/// id thread found in the sessions file. They differ whenever Claude Code
/// opened a conversation of its own, and a marker may name either.
///
/// Only a marker written at or after `since`, the start of this run,
/// counts. An older one records an earlier exit of the same conversation
/// that the supervisor has not yet deleted, the same marker its own pass
/// treats as stale (design 4.14).
pub(super) fn owner_ended(
    dir: &AgentsDir,
    name: &str,
    launch_id: &str,
    first: Option<&str>,
    since: i64,
) -> bool {
    let entry = read_entry(dir, name, launch_id);
    if entry.as_ref().is_some_and(|e| e.state == State::Ended) {
        return true;
    }
    let ended = dir.ended();
    let found = entry.and_then(|e| e.conversation_id);
    [first.map(str::to_string), found]
        .into_iter()
        .flatten()
        .any(|id| markers::ended_since(&ended, &id, since))
}

/// This launch's entry, or `None` when the registry cannot be read or the
/// entry is no longer this launch's.
fn read_entry(dir: &AgentsDir, name: &str, launch_id: &str) -> Option<Entry> {
    let reg = store::load(dir).ok()?;
    reg.agents
        .get(name)
        .filter(|e| e.launch_id == launch_id)
        .cloned()
}

/// Compare and set, under the registry's own lock: the entry becomes active
/// again under a new launch id, but only while it is still `launch_id`'s
/// and still active. Returns the new launch id and the entry the relaunch
/// is rebuilt from, or `None` when the entry has moved on (the supervisor
/// rebuilt the session elsewhere, or ended or stopped it), is gone, or
/// cannot be written.
pub(super) fn reactivate(dir: &AgentsDir, name: &str, launch_id: &str) -> Option<(String, Entry)> {
    let next = registry::new_launch_id();
    let now = registry::now();
    let out = store::update(dir, now, |reg| {
        let Some(e) = reg.agents.get_mut(name) else {
            return Ok(None);
        };
        if e.launch_id != launch_id || e.state != State::Active {
            return Ok(None);
        }
        e.activate(now, &next);
        Ok(Some(e.clone()))
    });
    match out {
        Ok(Some(entry)) => Some((next, entry)),
        Ok(None) => None,
        Err(e) => {
            eprintln!(
                "warning: agent {name}: the relaunch could not take the registry entry: {e:#}"
            );
            None
        }
    }
}

/// Records what the id thread found, unless the entry is no longer this
/// launch's or the conversation belongs to another active agent (C3).
/// The profile recorded is the one whose directory held the sessions file,
/// with or without `--profile`: without it the launch may still have run
/// under a profile (an inherited `AMPLIHACK_CLAUDE_PROFILE`, possibly
/// redirected by auto-switch, or the default pointer), and with
/// `--profile NAME` naming the rotator's live login it ran against
/// `~/.claude`, so no profile is recorded. Either way the resume uses the
/// directory that holds the conversation, not one the rotator may since
/// have given back its own login.
pub(super) fn record_claude(
    dir: &AgentsDir,
    home: &Path,
    name: &str,
    launch_id: &str,
    found: Found,
) -> anyhow::Result<()> {
    let profile = sessions::profile_of_config_dir(home, &found.config_dir);
    store::update(dir, registry::now(), |reg| {
        let held = reg
            .active_with_conversation(&found.conversation, name)
            .is_some();
        if let Some(e) = reg.agents.get_mut(name)
            && e.state == State::Active
            && e.launch_id == launch_id
            && !held
        {
            e.set_process(Some((found.pid, found.start)));
            e.conversation_id = Some(found.conversation.clone());
            e.amplihack_flags.profile = profile.clone();
        }
        Ok(())
    })
}
