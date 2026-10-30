//! The launch fields that `agent register` and `amplihack claude
//! --supervise` record, and the checks both make before a write: the
//! conversation and tmux session conflicts (C3, D9), the socket directory
//! (S12) and the profile (S8).

use super::registry::{AmplihackFlags, Entry, Registry};
use super::socket_check::{self, SocketCheck};
use super::{sessions, tmux, validate};
use anyhow::{anyhow, bail};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const SOCKET_NOT_PRIVATE: &str =
    "tmux socket: its directory must be owned by you and closed to other users";

/// The launch fields that `register` and the wrapper record, validated.
pub(super) struct Launch {
    pub socket: Option<PathBuf>,
    pub session: String,
    pub window: String,
    pub launch_dir: PathBuf,
    pub conversation: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub add_dirs: Vec<PathBuf>,
    pub permission_mode: Option<String>,
    pub skip_permissions: bool,
    pub flags: AmplihackFlags,
    pub env: BTreeMap<String, String>,
}

impl Launch {
    /// Refuses a conversation or a tmux session that another `active` agent
    /// already has (C3, D9). An unset socket is the default socket.
    pub(super) fn check_conflicts(&self, reg: &Registry, name: &str) -> anyhow::Result<()> {
        if let Some(id) = &self.conversation
            && let Some(other) = reg.active_with_conversation(id, name)
        {
            bail!("this conversation is already used by registered agent {other}");
        }
        let default = tmux::default_socket();
        if let Some(other) =
            reg.session_conflict(self.socket.as_deref(), &self.session, name, &default)
        {
            bail!(
                "tmux session {} is already used by registered agent {other}",
                self.session
            );
        }
        Ok(())
    }

    /// Copies the launch fields into `e`. A permission mode wins over
    /// `--dangerously-skip-permissions` (I4).
    pub(super) fn apply(self, e: &mut Entry) {
        e.tmux_socket = self.socket.map(path_string);
        e.tmux_session = self.session;
        e.tmux_window = self.window;
        e.launch_dir = path_string(self.launch_dir);
        e.conversation_id = self.conversation;
        e.model = self.model;
        e.effort = self.effort;
        e.add_dirs = self.add_dirs.into_iter().map(path_string).collect();
        e.skip_permissions = self.skip_permissions && self.permission_mode.is_none();
        e.permission_mode = self.permission_mode;
        e.amplihack_flags = self.flags;
        e.env = self.env;
    }

    /// A new `active` entry registered and launched at `registered_at`.
    pub(super) fn into_entry(self, registered_at: String) -> Entry {
        let mut e = Entry::new(
            self.session.clone(),
            self.window.clone(),
            path_string(self.launch_dir.clone()),
            registered_at,
        );
        self.apply(&mut e);
        e
    }
}

/// Paths reach the registry only after validation, which requires UTF-8.
fn path_string(p: PathBuf) -> String {
    p.to_string_lossy().into_owned()
}

/// S12 before any use of a socket: its directory must be private.
pub(super) fn check_socket(socket: &Path) -> anyhow::Result<()> {
    match socket_check::check(socket) {
        SocketCheck::NotPrivate => bail!(SOCKET_NOT_PRIVATE),
        SocketCheck::Ok | SocketCheck::MissingParent => Ok(()),
    }
}

/// A profile name that passes the profile rule (S8) and whose directory
/// exists under the profile root.
pub(super) fn check_profile(home: &Path, profile: &str) -> anyhow::Result<String> {
    subject(validate::profile_name(profile), "--profile")?;
    // The error is a whole `--profile: ...` or profile-root message.
    sessions::profile_config_dir(home, Some(profile)).map_err(|e| anyhow!(e))?;
    Ok(profile.to_string())
}

/// `SUBJECT: RULE`, the shape of every refusal of a value (step 5b 9.1).
pub(super) fn subject<T>(r: Result<T, String>, subject: &str) -> anyhow::Result<T> {
    r.map_err(|rule| anyhow!("{subject}: {rule}"))
}
