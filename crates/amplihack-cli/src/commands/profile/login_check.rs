//! `amplihack profile login <name>` with the duplicate-login check (bug 2).
//!
//! A login to the rotator's live profile is refused first: its login is
//! `~/.claude`. Otherwise, before Claude starts, the profile's login file is
//! moved into a new `<root>/.rejected-logins/.pending-<name>-<ts>/` with a
//! copy of its `oauthAccount` block. After Claude exits, the new login is
//! compared with every other profile and with `~/.claude` by
//! [`Account::same`]; when they cannot be read, the login is refused (fail
//! closed). A duplicate is moved to `<name>-<ts>/` and the previous login is
//! put back; an accepted login sends the previous one to
//! `<name>-<ts>-replaced/`.
//!
//! Login files are only checked by metadata and renamed (never opened, read,
//! copied, chmod-ed or deleted), so every login is in exactly one place, and
//! each path is printed before a login is moved into it. Only the
//! `oauthAccount` key of the profile's `.claude.json` is written. Messages
//! name profiles and paths, never account values.

use super::auto_switch::Action;
use super::auto_switch::kit::{
    CLAUDE_JSON, CREDS, MoveError, State, ensure_private_dir, exists, fsync_dir, guard_held,
    movable_login, move_login, read_claude_json, write_atomic_keep_mode, write_private_atomic,
};
use super::default_live;
use super::duplicates::{self, Holder};
use super::name::ProfileName;
use super::rotate::identify::Account;
use super::store::{ProfileStore, check_root_not_symlink, private_dir_builder};
use super::usage_cmd::sanitize_display;
use anyhow::{Result, anyhow};
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

pub(super) const REJECTED_DIR: &str = ".rejected-logins";
/// The `oauthAccount` block saved next to a held or set-aside login.
const SAVED_BLOCK: &str = "oauthAccount.json";

/// The refusal, without the `error: ` prefix the top-level printer adds.
pub(super) fn refusal_message(name: &str, holder: &str, path: &str) -> String {
    format!(
        "profile '{name}' was signed in to the same account and organization as {holder}. \
         The new login was moved to {path}; '{name}' is back to its previous login."
    )
}

pub(super) fn refusal_hint(name: &str) -> String {
    format!(
        "sign out of claude.ai in your browser (or use a private/incognito window), \
         then run 'amplihack profile login {name}' again."
    )
}

fn show(p: &Path) -> String {
    sanitize_display(&p.display().to_string(), false)
}

fn fail(msg: impl std::fmt::Display, hint: impl std::fmt::Display) -> anyhow::Error {
    anyhow!("{msg}\nhint: {hint}")
}

fn move_error(e: MoveError) -> String {
    match e {
        MoveError::Exists => "the destination already exists".into(),
        MoveError::Unsupported => "this filesystem cannot rename between these paths".into(),
        MoveError::Other(m) => m,
    }
}

/// The previous login while Claude runs.
struct Held {
    /// The profile directory.
    dir: PathBuf,
    rejected: PathBuf,
    pending: PathBuf,
    ts: String,
    /// Whether a previous login was moved into `pending`.
    held: bool,
    /// The previous `oauthAccount` block; `None` when there was none.
    block: Option<serde_json::Value>,
}

impl Held {
    fn slot(&self) -> PathBuf {
        self.dir.join(CREDS)
    }

    fn held_login(&self) -> PathBuf {
        self.pending.join(CREDS)
    }

    /// Where every login is, for a message after a partial failure.
    /// `aside` is where the new login was moved, once it was.
    fn whereabouts(&self, aside: Option<&Path>) -> String {
        let at = aside.map_or_else(|| self.slot(), |a| a.join(CREDS));
        let new = format!("the new login, if any, is at {}", show(&at));
        if self.held {
            format!(
                "{new}; the previous login is held in {}",
                show(&self.pending)
            )
        } else {
            new
        }
    }
}

/// The refusal of a login to the rotator's live profile, without the
/// `error: ` prefix.
pub(super) fn live_refusal_message(name: &str) -> String {
    format!(
        "profile '{name}' is the rotator's live profile; its login is ~/.claude. \
         Nothing was moved."
    )
}

pub(super) const LIVE_REFUSAL_HINT: &str = "log in with plain 'claude', or stop the rotator first.";

/// Runs `run` (the interactive `claude` login) for `name` and refuses a
/// login whose account another profile or `~/.claude` already holds.
/// `home` is `$HOME`; without it only profiles are compared.
pub(super) fn login_checked(
    store: &ProfileStore,
    name: &ProfileName,
    home: Option<&Path>,
    run: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let root = store.root();
    check_root_not_symlink(root).map_err(super::with_hint)?;
    let dir = store.verify_profile_dir(name).map_err(super::with_hint)?;
    let state = State::new(root).map_err(super::with_hint)?;
    // The live profile's login is `~/.claude`, and the rotator refuses to
    // swap back into a profile that has a login of its own: refused before
    // Claude runs, so nothing is held or moved.
    if default_live::live_name(store, home).as_ref() == Some(name) {
        return Err(fail(live_refusal_message(name.as_str()), LIVE_REFUSAL_HINT));
    }
    let ts = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let held = {
        let _lock = guard_held(&state, name, Action::Login).map_err(super::with_hint)?;
        hold(root, dir, name.as_str(), ts)?
    };
    let outcome = {
        let _sigint = SigintCaught::install().map_err(|e| {
            fail(
                format!("could not set up the Ctrl-C handler: {e}"),
                held.whereabouts(None),
            )
        })?;
        run()
    };
    // The lease check runs again: a swap may have started while Claude ran.
    let _lock = guard_held(&state, name, Action::Login).map_err(|e| {
        after_claude(
            fail(
                format!(
                    "profile '{name}' changed hands while you logged in ({e}); nothing was moved"
                ),
                held.whereabouts(None),
            ),
            &outcome,
        )
    })?;
    // The rotator does not take leases: it may have swapped this profile in
    // while Claude ran, leaving its directory without a login.
    if default_live::live_name(store, home).as_ref() == Some(name) {
        return Err(after_claude(
            fail(
                format!(
                    "profile '{name}' was swapped in as the rotator's live profile while you \
                     logged in; nothing was moved"
                ),
                held.whereabouts(None),
            ),
            &outcome,
        ));
    }
    match outcome {
        Err(e) => match failed(&held, name.as_str()) {
            Ok(()) => Err(e),
            Err(cleanup) => Err(after_claude(cleanup, &Err(e))),
        },
        Ok(()) => finish(store, &held, name, home),
    }
}

/// Keeps Claude's own error when the cleanup after it fails too.
fn after_claude(cleanup: anyhow::Error, outcome: &Result<()>) -> anyhow::Error {
    match outcome {
        Err(e) => anyhow!("claude exited with an error first: {e:#}\n{cleanup:#}"),
        Ok(()) => cleanup,
    }
}

/// Phase A: moves the previous login into a new pending directory, after
/// saving its `oauthAccount` block there.
fn hold(root: &Path, dir: PathBuf, name: &str, ts: String) -> Result<Held> {
    let slot = dir.join(CREDS);
    let present = exists(&slot);
    if present && !movable_login(&slot) {
        return Err(fail(
            format!(
                "the login of profile '{name}' at {} is not a private regular file with one \
                 link, so it cannot be held while you log in; nothing was moved",
                show(&slot)
            ),
            format!("fix that file, then run 'amplihack profile login {name}' again"),
        ));
    }
    let rejected = root.join(REJECTED_DIR);
    ensure_private_dir(&rejected).map_err(|e| {
        fail(
            format!("{}: {e}; nothing was moved", show(&rejected)),
            "it must be a real directory owned by you with mode 0700",
        )
    })?;
    let pending = rejected.join(format!(".pending-{name}-{ts}"));
    create_fresh(&pending).map_err(|e| fail(e, "nothing was moved; retry in a second"))?;
    let block = read_claude_json(&dir).and_then(|(v, _)| v.get("oauthAccount").cloned());
    if let Some(b) = &block
        && let Err(e) = save_block(&pending, b)
    {
        remove_pending(&pending);
        return Err(fail(
            format!("{}: {e}; nothing was moved", show(&pending)),
            "check permissions on the profile root",
        ));
    }
    if present {
        eprintln!(
            "note: holding the previous login of profile '{name}' in {} while you log in",
            show(&pending)
        );
        if let Err(e) = move_login(&slot, &pending.join(CREDS), &show) {
            remove_pending(&pending);
            return Err(fail(
                format!(
                    "could not hold the login of profile '{name}': {}; nothing was moved",
                    move_error(e)
                ),
                "check permissions on the profile directory",
            ));
        }
    }
    Ok(Held {
        dir,
        rejected,
        pending,
        ts,
        held: present,
        block,
    })
}

/// Phase C after Claude failed: the new login, if any, is set aside and the
/// previous login and `oauthAccount` are restored. No duplicate check.
fn failed(held: &Held, name: &str) -> Result<()> {
    let aside = if exists(&held.slot()) {
        Some(set_aside(held, name)?)
    } else {
        None
    };
    restore(held, name, aside.as_deref())
}

/// Phase C after Claude succeeded.
fn finish(
    store: &ProfileStore,
    held: &Held,
    name: &ProfileName,
    home: Option<&Path>,
) -> Result<()> {
    let n = name.as_str();
    let slot = held.slot();
    if !exists(&slot) {
        return no_login_file(store, held, name, home);
    }
    if !movable_login(&slot) {
        return Err(exposed(held, n));
    }
    match duplicate_of(store, held, name, home) {
        Ok(None) => {
            replace_previous(held, n)?;
            record_login(store, held, name);
            Ok(())
        }
        Ok(Some(h)) => {
            let aside = set_aside(held, n)?;
            restore(held, n, Some(&aside))?;
            Err(fail(
                refusal_message(n, &duplicates::holder_label(&h), &show(&aside)),
                refusal_hint(n),
            ))
        }
        Err(e) => {
            let aside = set_aside(held, n)?;
            restore(held, n, Some(&aside))?;
            Err(fail(
                format!(
                    "could not check profile '{n}' for duplicate logins: {e:#}. The new login \
                     was moved to {}; '{n}' is back to its previous login.",
                    show(&aside)
                ),
                format!("fix that, then run 'amplihack profile login {n}' again"),
            ))
        }
    }
}

/// Claude exited 0 without writing a login file. With a previous login,
/// everything is put back. Without one (the macOS Keychain) `oauthAccount`
/// stays as Claude wrote it, matching the Keychain, and a duplicate is
/// still reported.
fn no_login_file(
    store: &ProfileStore,
    held: &Held,
    name: &ProfileName,
    home: Option<&Path>,
) -> Result<()> {
    let n = name.as_str();
    if held.held {
        restore(held, n, None)?;
        return Err(fail(
            format!(
                "Claude exited without writing a login file; the previous login of profile \
                 '{n}' was restored"
            ),
            format!("run 'amplihack profile login {n}' again and finish the login in Claude"),
        ));
    }
    match duplicate_of(store, held, name, home) {
        Ok(None) => {
            remove_pending(&held.pending);
            record_login(store, held, name);
            Ok(())
        }
        Ok(Some(h)) => {
            remove_pending(&held.pending);
            Err(fail(
                format!(
                    "profile '{n}' is signed in to the same account and organization as {}, \
                     but Claude wrote no login file, so there was nothing to move aside",
                    duplicates::holder_label(&h)
                ),
                refusal_hint(n),
            ))
        }
        Err(e) => {
            restore(held, n, None)?;
            Err(fail(
                format!(
                    "could not check profile '{n}' for duplicate logins: {e:#}; its \
                     oauthAccount was restored"
                ),
                format!("fix that, then run 'amplihack profile login {n}' again"),
            ))
        }
    }
}

/// Issue #1845 (D2): an accepted login records the account in the
/// profile's new `.claude.json` as the one it is meant to hold, replacing
/// any earlier record. Nothing is recorded without an account; a record
/// that cannot be saved is a warning and the login stands.
fn record_login(store: &ProfileStore, held: &Held, name: &ProfileName) {
    if let Some(account) = Account::of_dir(&held.dir) {
        super::expected::save(store.root(), name, &account, false);
    }
}

/// The login is accepted: the previous one, if any, moves to a new
/// `<name>-<ts>-replaced/` with its saved block.
fn replace_previous(held: &Held, n: &str) -> Result<()> {
    if held.held {
        let replaced = held.rejected.join(format!("{n}-{}-replaced", held.ts));
        create_fresh(&replaced).map_err(|e| fail(e, held.whereabouts(None)))?;
        eprintln!(
            "note: the previous login of profile '{n}' was moved to {}",
            show(&replaced)
        );
        move_login(&held.held_login(), &replaced.join(CREDS), &show).map_err(|e| {
            fail(
                format!("could not move the previous login: {}", move_error(e)),
                held.whereabouts(None),
            )
        })?;
        let saved = held.pending.join(SAVED_BLOCK);
        if exists(&saved)
            && let Err(e) = fs::rename(&saved, replaced.join(SAVED_BLOCK))
        {
            eprintln!("warning: {}: {e}", show(&saved));
        }
    }
    remove_pending(&held.pending);
    Ok(())
}

/// The holder of the account now in `name`'s `.claude.json`, if any other
/// does. An error when the profiles cannot be read: the check fails closed.
fn duplicate_of(
    store: &ProfileStore,
    held: &Held,
    name: &ProfileName,
    home: Option<&Path>,
) -> Result<Option<Holder>> {
    let Some(mine) = Account::of_dir(&held.dir) else {
        return Ok(None);
    };
    // Issue #1700: an API-key profile is never a duplicate login.
    if super::kind::Ctx::load(store, home).kind(store, name) == super::kind::Kind::Platform {
        return Ok(None);
    }
    Ok(duplicates::try_accounts(store, home)?
        .into_iter()
        .find(|(h, a)| !matches!(h, Holder::Profile(p) if p == name) && a.same(&mine))
        .map(|(h, _)| h))
}

/// S-6: a new login that is not a private regular file with one link is
/// left where it is, mode unchanged. The previous login stays held.
fn exposed(held: &Held, name: &str) -> anyhow::Error {
    let slot = held.slot();
    let mode = fs::symlink_metadata(&slot)
        .map(|m| format!("{:04o}", m.mode() & 0o7777))
        .unwrap_or_else(|e| format!("unknown ({e})"));
    fail(
        format!(
            "the new login of profile '{name}' at {} has mode {mode} and is not a private \
             regular file with one link, so it was not moved and its mode was not changed; \
             treat that login as exposed and sign it out on claude.ai",
            show(&slot)
        ),
        held.whereabouts(None),
    )
}

/// Moves the new login into a new `<name>-<ts>/` and saves its
/// `oauthAccount` block there. Returns that directory.
fn set_aside(held: &Held, name: &str) -> Result<PathBuf> {
    let slot = held.slot();
    if !movable_login(&slot) {
        return Err(exposed(held, name));
    }
    let aside = held.rejected.join(format!("{name}-{}", held.ts));
    create_fresh(&aside).map_err(|e| fail(e, held.whereabouts(None)))?;
    eprintln!(
        "note: moving the new login of profile '{name}' to {}",
        show(&aside)
    );
    move_login(&slot, &aside.join(CREDS), &show).map_err(|e| {
        fail(
            format!("could not move the new login: {}", move_error(e)),
            held.whereabouts(None),
        )
    })?;
    let block = read_claude_json(&held.dir).and_then(|(v, _)| v.get("oauthAccount").cloned());
    if let Some(b) = block
        && let Err(e) = save_block(&aside, &b)
    {
        eprintln!("warning: {}: {e}", show(&aside.join(SAVED_BLOCK)));
    }
    Ok(aside)
}

/// Puts the previous login and `oauthAccount` back and removes the pending
/// directory. `aside` is where the new login was moved, if it was. When the
/// block cannot be written back, the pending directory keeps its only copy.
fn restore(held: &Held, name: &str, aside: Option<&Path>) -> Result<()> {
    put_back(held, name, aside)?;
    put_back_block(&held.dir, held.block.as_ref()).map_err(|e| {
        fail(
            format!(
                "profile '{name}': its login is back, but {} could not be updated: {e}",
                show(&held.dir.join(CLAUDE_JSON))
            ),
            format!(
                "its previous oauthAccount block is in {}; {}",
                show(&held.pending.join(SAVED_BLOCK)),
                held.whereabouts(aside)
            ),
        )
    })?;
    remove_pending(&held.pending);
    Ok(())
}

/// Moves the held login back into the profile directory.
fn put_back(held: &Held, name: &str, aside: Option<&Path>) -> Result<()> {
    if !held.held {
        return Ok(());
    }
    move_login(&held.held_login(), &held.slot(), &show).map_err(|e| {
        fail(
            format!(
                "could not put the previous login of profile '{name}' back: {}",
                move_error(e)
            ),
            held.whereabouts(aside),
        )
    })
}

/// Sets only the `oauthAccount` key of `dir/.claude.json` to `block`, or
/// removes it, keeping every other key and the file's mode. The file is
/// never created: a block to put back into a missing, unparsable or
/// non-object file is an error; with no block there is nothing to do.
fn put_back_block(dir: &Path, block: Option<&serde_json::Value>) -> io::Result<()> {
    let parsed = read_claude_json(dir).filter(|(v, _)| v.is_object());
    let Some((mut v, _)) = parsed else {
        return match block {
            Some(_) => Err(io::Error::other("it is missing or not a JSON object")),
            None => Ok(()),
        };
    };
    let Some(obj) = v.as_object_mut() else {
        return Ok(());
    };
    let changed = match block {
        Some(b) if obj.get("oauthAccount") == Some(b) => false,
        Some(b) => {
            obj.insert("oauthAccount".into(), b.clone());
            true
        }
        None => obj.remove("oauthAccount").is_some(),
    };
    if !changed {
        return Ok(());
    }
    let bytes = serde_json::to_vec_pretty(&v).map_err(io::Error::other)?;
    write_atomic_keep_mode(dir, CLAUDE_JSON, &bytes)
}

fn save_block(dir: &Path, block: &serde_json::Value) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(block).map_err(io::Error::other)?;
    write_private_atomic(dir, SAVED_BLOCK, &bytes)
}

/// Creates `path` as a new private directory, never through a symlink, and
/// syncs its parent; an existing one is `AlreadyExists`. Shared with rotate's
/// hand-change recovery (issue #1693).
pub(super) fn create_fresh_dir(path: &Path) -> io::Result<()> {
    private_dir_builder().create(path)?;
    match path.parent().map(fsync_dir) {
        Some(Err(e)) => Err(e),
        _ => Ok(()),
    }
}

/// Creates `path` as a new private directory; an existing one is an error.
fn create_fresh(path: &Path) -> std::result::Result<(), String> {
    match create_fresh_dir(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(format!(
            "{} already exists; nothing was moved into it",
            show(path)
        )),
        Err(e) => Err(format!("{}: {e}", show(path))),
    }
}

/// Removes amplihack's own saved block and the then-empty pending
/// directory. Anything else left in it keeps the directory.
fn remove_pending(pending: &Path) {
    let saved = pending.join(SAVED_BLOCK);
    if exists(&saved)
        && let Err(e) = fs::remove_file(&saved)
    {
        eprintln!("warning: {}: {e}", show(&saved));
    }
    if let Err(e) = fs::remove_dir(pending) {
        eprintln!("warning: {} was not removed: {e}", show(pending));
    }
}

/// Catches SIGINT for as long as it lives, so Ctrl-C ends only Claude and
/// the restore still runs. A caught signal (unlike an ignored one) is reset
/// to the default in the child at exec, so Claude still sees Ctrl-C.
struct SigintCaught {
    old: libc::sigaction,
}

extern "C" fn on_sigint(_: libc::c_int) {}

impl SigintCaught {
    fn install() -> io::Result<Self> {
        // SAFETY: both structs are fully initialised (zeroed, then the
        // handler and mask set); the handler is async-signal-safe (it does
        // nothing).
        unsafe {
            let mut new: libc::sigaction = std::mem::zeroed();
            new.sa_sigaction = on_sigint as extern "C" fn(libc::c_int) as libc::sighandler_t;
            new.sa_flags = libc::SA_RESTART;
            libc::sigemptyset(&mut new.sa_mask);
            let mut old: libc::sigaction = std::mem::zeroed();
            if libc::sigaction(libc::SIGINT, &new, &mut old) != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self { old })
        }
    }
}

impl Drop for SigintCaught {
    fn drop(&mut self) {
        // SAFETY: `old` is the disposition `sigaction` returned.
        unsafe {
            libc::sigaction(libc::SIGINT, &self.old, std::ptr::null_mut());
        }
    }
}
