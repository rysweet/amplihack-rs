//! Registry, log, transcript, profile and marker helpers of the issue #1760
//! fixture (`issue_1760_fixture.rs`).

#![cfg(target_os = "linux")]
#![allow(dead_code)]

use crate::issue_1760_fixture::{Fx, LAUNCH_ID, chmod, now, ts};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

impl Fx {
    // ----- registry -----

    pub fn agents_dir(&self) -> PathBuf {
        self.home.join(".amplihack/agents")
    }
    pub fn registry_path(&self) -> PathBuf {
        self.agents_dir().join("registry.json")
    }
    pub fn registry_raw(&self) -> Vec<u8> {
        fs::read(self.registry_path()).expect("registry.json exists")
    }
    pub fn registry(&self) -> Value {
        serde_json::from_slice(&self.registry_raw()).expect("registry.json is JSON")
    }
    pub fn agent(&self, name: &str) -> Value {
        self.registry()["agents"][name].clone()
    }
    /// Creates `~/.amplihack` and `agents/` at 0700.
    pub fn private_agents_dir(&self) {
        let top = self.home.join(".amplihack");
        fs::create_dir_all(self.agents_dir()).unwrap();
        chmod(&top, 0o700);
        chmod(&self.agents_dir(), 0o700);
    }
    pub fn write_registry_raw(&self, raw: &str) {
        self.private_agents_dir();
        let p = self.registry_path();
        fs::write(&p, raw).unwrap();
        chmod(&p, 0o600);
    }
    pub fn write_registry(&self, reg: &Value) {
        self.write_registry_raw(&serde_json::to_string_pretty(reg).unwrap());
    }
    /// A version 1 registry holding `agents`.
    pub fn write_agents(&self, agents: &[(&str, Value)]) {
        let map: serde_json::Map<String, Value> = agents
            .iter()
            .map(|(n, e)| (n.to_string(), e.clone()))
            .collect();
        self.write_registry(&json!({ "version": 1, "agents": map }));
    }
    /// A valid `active` entry, registered and launched two hours ago, with no
    /// socket (tmux's default for the reader), window `main`, and
    /// `--dangerously-skip-permissions`. Change fields on the returned value.
    pub fn entry(&self, session: &str, dir: &Path, conv: Option<&str>) -> Value {
        let mut e = json!({
            "state": "active",
            "tmux_session": session,
            "tmux_window": "main",
            "launch_dir": dir,
            "add_dirs": [],
            "skip_permissions": true,
            "amplihack_flags": { "subprocess_safe": false, "no_reflection": false },
            "env": {},
            "registered_at": ts(now() - 7200),
            "launched_at": ts(now() - 7200),
            "launch_id": LAUNCH_ID,
            "restarts": []
        });
        if let Some(c) = conv {
            e["conversation_id"] = json!(c);
        }
        e
    }

    // ----- log, transcripts, profiles, markers -----

    pub fn log_path(&self) -> PathBuf {
        self.agents_dir().join("supervisor.log")
    }
    pub fn log_lines(&self) -> Vec<String> {
        fs::read_to_string(self.log_path())
            .unwrap_or_default()
            .lines()
            .map(String::from)
            .collect()
    }
    /// `<config>/projects/<dir with '/' and '.' as '-'>/<conv>.jsonl`.
    pub fn transcript_in(&self, config: &Path, dir: &Path, conv: &str) -> PathBuf {
        let enc = dir.display().to_string().replace(['/', '.'], "-");
        let d = config.join("projects").join(enc);
        fs::create_dir_all(&d).unwrap();
        let f = d.join(format!("{conv}.jsonl"));
        fs::write(&f, "{}\n").unwrap();
        f
    }
    /// The transcript in `~/.claude`.
    pub fn transcript(&self, dir: &Path, conv: &str) -> PathBuf {
        self.transcript_in(&self.claude_dir.clone(), dir, conv)
    }
    /// `profiles/<name>`, mode 0700.
    pub fn profile_dir(&self, name: &str) -> PathBuf {
        let d = self.profiles.join(name);
        fs::create_dir_all(&d).unwrap();
        chmod(&d, 0o700);
        d
    }
    pub fn ended_dir(&self) -> PathBuf {
        self.agents_dir().join("ended")
    }
    /// `ended/<conv>` as the hook writes it (0600 in a 0700 directory).
    pub fn marker(&self, conv: &str, reason: &str, at: i64) -> PathBuf {
        let d = self.ended_dir();
        fs::create_dir_all(&d).unwrap();
        chmod(&d, 0o700);
        let p = d.join(conv);
        let body = json!({ "session_id": conv, "reason": reason, "at": at });
        fs::write(&p, body.to_string()).unwrap();
        chmod(&p, 0o600);
        p
    }

    /// The `TMUX` value of a pane on `socket`.
    pub fn tmux_env(&self) -> String {
        format!("{},4242,0", self.socket.display())
    }

    pub(crate) fn make_socket(&self) {
        let _ = fs::remove_file(&self.socket);
        drop(std::os::unix::net::UnixListener::bind(&self.socket).expect("bind the test socket"));
    }
}

// ----- the resume contract (R5.6, R5.7) -----

/// The start of the R5.7 resume message, up to `lost_at`.
pub const MESSAGE_HEAD: &str =
    "amplihack agent supervisor: the terminal session for this agent was lost at ";
const MESSAGE_MIDDLE: &str = ". This conversation was resumed at ";

/// The R5.7 resume message.
pub fn resume_message(lost: &str, resumed: &str) -> String {
    format!(
        "{MESSAGE_HEAD}{lost}{MESSAGE_MIDDLE}{resumed}. Before continuing, check what your \
         interrupted step had already done (git status, open pull requests, running processes) \
         and do not repeat finished steps."
    )
}

/// `(lost, resumed)` from a resume message, which must match the template.
pub fn message_times(msg: &str) -> (String, String) {
    let rest = msg
        .strip_prefix(MESSAGE_HEAD)
        .unwrap_or_else(|| panic!("not a resume message: {msg:?}"));
    let lost = rest.get(..20).unwrap_or_default().to_string();
    let end = 20 + MESSAGE_MIDDLE.len();
    let resumed = rest.get(end..end + 20).unwrap_or_default().to_string();
    assert_eq!(msg, resume_message(&lost, &resumed), "the R5.7 template");
    (lost, resumed)
}

/// The `-u` names of the pane argv: `SESSION_MARKERS` in order, then the
/// design 4.13 list, without repeats (D6, A3).
pub fn pane_unset() -> Vec<String> {
    let extra = "CLAUDE_CODE_SESSION_ID CLAUDE_CODE_ENTRYPOINT CLAUDE_PROJECT_DIR \
                 AMPLIHACK_SESSION_ID AMPLIHACK_DEPTH AMPLIHACK_SESSION_DEPTH AMPLIHACK_TREE_ID \
                 CLAUDE_CONFIG_DIR AMPLIHACK_CLAUDE_PROFILE";
    let markers = amplihack_utils::agent_binary::SESSION_MARKERS
        .iter()
        .map(|(n, _)| *n);
    let mut v: Vec<String> = Vec::new();
    for n in markers.chain(extra.split_whitespace()) {
        if !v.iter().any(|x| x == n) {
            v.push(n.to_string());
        }
    }
    v
}
