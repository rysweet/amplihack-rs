//! The version 1 registry schema (`~/.amplihack/agents/registry.json`), the
//! transitions that keep its invariants (data model 3.2, C6), the conflict
//! queries (U2, U3) and the UTC time helpers. Files are read and written by
//! `store.rs`.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

pub const VERSION: u64 = 1;
/// Restarts are kept for a day; a few minutes ahead are tolerated (S25).
const DAY: i64 = 24 * 60 * 60;
pub(crate) const FUTURE_SLACK: i64 = 300;
const TS_FORMAT: &str = "%Y-%m-%dT%H:%M:%SZ";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Registry {
    pub version: u64,
    #[serde(default)]
    pub agents: BTreeMap<String, Entry>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Active,
    Ended,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EndedBy {
    Wrapper,
    SessionEnd,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AmplihackFlags {
    #[serde(default)]
    pub subprocess_safe: bool,
    #[serde(default)]
    pub no_reflection: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LastRecovery {
    pub logged_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lost: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resumed: Option<String>,
    pub result: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub state: State,
    pub tmux_session: String,
    pub tmux_window: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tmux_socket: Option<String>,
    pub launch_dir: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proc_start: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default)]
    pub add_dirs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    #[serde(default)]
    pub skip_permissions: bool,
    #[serde(default)]
    pub amplihack_flags: AmplihackFlags,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    pub registered_at: String,
    pub launched_at: String,
    pub launch_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lost_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_by: Option<EndedBy>,
    #[serde(default)]
    pub restarts: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_recovery: Option<LastRecovery>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alert: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Registry {
    /// Version 1 with no agents. There is no `Default`, so a missing version
    /// can never be written back as 0.
    pub fn new() -> Self {
        Self {
            version: VERSION,
            agents: BTreeMap::new(),
            extra: BTreeMap::new(),
        }
    }

    /// The other `active` agent whose conversation is `id` (U3, C3).
    pub fn active_with_conversation(&self, id: &str, except: &str) -> Option<&str> {
        self.agents
            .iter()
            .find(|(n, e)| {
                n.as_str() != except
                    && e.state == State::Active
                    && e.conversation_id.as_deref() == Some(id)
            })
            .map(|(n, _)| n.as_str())
    }

    /// The other `active` agent on the same tmux server and session (U2, D9).
    /// An unset socket is the default socket; paths compare by component.
    pub fn session_conflict(
        &self,
        socket: Option<&Path>,
        session: &str,
        except: &str,
        default_socket: &Path,
    ) -> Option<&str> {
        let wanted = socket.unwrap_or(default_socket);
        self.agents
            .iter()
            .find(|(n, e)| {
                let theirs = e.tmux_socket.as_deref().map_or(default_socket, Path::new);
                n.as_str() != except
                    && e.state == State::Active
                    && e.tmux_session == session
                    && theirs == wanted
            })
            .map(|(n, _)| n.as_str())
    }
}

impl Entry {
    /// A new `active` entry with every optional field absent, registered and
    /// launched at `registered_at`.
    pub fn new(session: String, window: String, launch_dir: String, registered_at: String) -> Self {
        Self {
            state: State::Active,
            tmux_session: session,
            tmux_window: window,
            tmux_socket: None,
            launch_dir,
            conversation_id: None,
            claude_pid: None,
            proc_start: None,
            model: None,
            effort: None,
            add_dirs: Vec::new(),
            permission_mode: None,
            skip_permissions: false,
            amplihack_flags: AmplihackFlags::default(),
            env: BTreeMap::new(),
            launched_at: registered_at.clone(),
            registered_at,
            launch_id: new_launch_id(),
            lost_at: None,
            ended_at: None,
            ended_by: None,
            restarts: Vec::new(),
            last_recovery: None,
            alert: None,
            extra: BTreeMap::new(),
        }
    }

    /// The recorded process, only when both halves are present (C5, I3).
    pub fn recorded(&self) -> Option<(u32, u64)> {
        self.claude_pid.zip(self.proc_start)
    }

    /// Wrapper start: active again with a new launch; history is kept (D3).
    pub fn activate(&mut self, now: i64, launch_id: &str) {
        self.state = State::Active;
        self.launched_at = fmt_ts(now);
        self.launch_id = launch_id.to_string();
        self.lost_at = None;
        self.ended_at = None;
        self.ended_by = None;
        self.alert = None;
        self.set_process(None);
    }

    pub fn end(&mut self, now: i64, by: EndedBy) {
        self.state = State::Ended;
        self.ended_at = Some(fmt_ts(now));
        self.ended_by = Some(by);
        self.lost_at = None;
        self.alert = None;
    }

    pub fn stop(&mut self, alert: &str) {
        self.state = State::Stopped;
        self.alert = Some(alert.to_string());
        self.lost_at = None;
        self.ended_at = None;
        self.ended_by = None;
    }

    pub fn set_process(&mut self, p: Option<(u32, u64)>) {
        self.claude_pid = p.map(|(pid, _)| pid);
        self.proc_start = p.map(|(_, start)| start);
    }

    /// A resume the supervisor started. `launch_id` and `launched_at` belong
    /// to the resumed wrapper (A1).
    pub fn record_resume(&mut self, now: i64, recovery: LastRecovery) {
        self.restarts.push(fmt_ts(now));
        self.last_recovery = Some(recovery);
        self.lost_at = None;
    }
}

/// A new launch id: 32 lowercase hexadecimal characters (A14).
pub fn new_launch_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// `%Y-%m-%dT%H:%M:%SZ` in UTC.
pub fn fmt_ts(t: i64) -> String {
    chrono::DateTime::from_timestamp(t, 0)
        .unwrap_or(chrono::DateTime::UNIX_EPOCH)
        .format(TS_FORMAT)
        .to_string()
}

/// The inverse of [`fmt_ts`]; only the exact canonical form is accepted.
pub fn parse_ts(s: &str) -> Option<i64> {
    let t = chrono::NaiveDateTime::parse_from_str(s, TS_FORMAT)
        .ok()?
        .and_utc()
        .timestamp();
    (fmt_ts(t) == s).then_some(t)
}

/// Restarts within the last 24 h and at most 300 s ahead, sorted (S25).
pub fn prune_restarts(r: &[String], now: i64) -> Vec<String> {
    let mut kept: Vec<i64> = r
        .iter()
        .filter_map(|s| parse_ts(s))
        .filter(|&t| t >= now - DAY && t <= now + FUTURE_SLACK)
        .collect();
    kept.sort_unstable();
    kept.into_iter().map(fmt_ts).collect()
}

/// Restarts in `[now - secs, now + 300]`, in any order.
pub fn restarts_within(r: &[String], now: i64, secs: i64) -> usize {
    r.iter()
        .filter_map(|s| parse_ts(s))
        .filter(|&t| t >= now - secs && t <= now + FUTURE_SLACK)
        .count()
}

/// `registered_at` for a registration at `now`: never earlier than one
/// second after the previous value, so it works as a generation (C4).
pub fn next_registered_at(previous: Option<&str>, now: i64) -> String {
    let prev = previous.and_then(parse_ts).map_or(now, |p| p + 1);
    fmt_ts(now.max(prev))
}

/// The current time in Unix seconds.
pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}
