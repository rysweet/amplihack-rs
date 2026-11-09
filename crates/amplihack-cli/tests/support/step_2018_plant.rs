//! Issue #2018: records planted by hand for the `recipe steps` tests.
//!
//! Live processes in planted records are the test process itself and
//! `cat` children the test owns; dead ones use pids above any `pid_max`,
//! so no planted pid can name someone else's process.

use crate::support::proc_watch::start_ticks;
use crate::support::step_2018::Fx;
use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

/// Pids above any `pid_max` the kernel allows (2^22): never alive.
pub const DEAD_A: i64 = 2_000_000_001;
pub const DEAD_B: i64 = 2_000_000_002;
pub const DEAD_C: i64 = 2_000_000_003;

/// A `cat` child this test owns, killed and reaped on drop. It blocks
/// reading a pipe whose write end this struct holds, so it runs until killed.
pub struct Sleeper(pub Child);

impl Sleeper {
    pub fn start() -> Self {
        let child = Command::new("/bin/cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn();
        Self(child.expect("start cat"))
    }
}

impl Drop for Sleeper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub fn me() -> i64 {
    std::process::id() as i64
}

pub fn proc_json(pid: i64) -> Value {
    let ticks = if pid < DEAD_A {
        json!(start_ticks(pid as i32).expect("a live pid"))
    } else {
        json!(1)
    };
    json!({ "pid": pid, "start_ticks": ticks })
}

pub fn planted(run_id: &str, tool: &str, launcher: i64, agent: i64, parent: i64) -> Value {
    json!({
        "format": 1, "run_id": run_id, "workspace_run_id": null, "run_dir": null,
        "tool": tool, "cwd": "/tmp/project", "created_at": "2026-11-07T10:15:03Z",
        "launcher": proc_json(launcher), "agent": proc_json(agent), "parent": proc_json(parent),
    })
}

pub fn plant(fx: &Fx, run_id: &str, launcher: i64, body: &Value) -> PathBuf {
    let dir = fx.run_dir(run_id);
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{launcher}.json"));
    fs::write(&path, serde_json::to_vec_pretty(body).unwrap()).unwrap();
    path
}

/// One record in each state, in run-id order:
/// - `run-a`: live (launcher, agent and parent are this process);
/// - `run-b`: orphan (launcher alive, parent gone);
/// - `run-c`: orphan (launcher gone, agent a live `cat`);
/// - `run-d`: stale (nothing alive), then an unreadable format-2 file.
pub struct Planted {
    pub paths: Vec<PathBuf>,
    pub sleeper: Sleeper,
}

pub fn plant_every_state(fx: &Fx) -> Planted {
    let sleeper = Sleeper::start();
    let agent = sleeper.0.id() as i64;
    let mut format_2 = planted("run-d", "claude", DEAD_C, DEAD_C, DEAD_C);
    format_2["format"] = json!(2);
    let paths = vec![
        plant(
            fx,
            "run-a",
            me(),
            &planted("run-a", "claude", me(), me(), me()),
        ),
        plant(
            fx,
            "run-b",
            me(),
            &planted("run-b", "copilot", me(), me(), DEAD_A),
        ),
        plant(
            fx,
            "run-c",
            DEAD_A,
            &planted("run-c", "codex", DEAD_A, agent, DEAD_B),
        ),
        plant(
            fx,
            "run-d",
            DEAD_B,
            &planted("run-d", "claude", DEAD_B, DEAD_B, DEAD_A),
        ),
        plant(fx, "run-d", DEAD_C, &format_2),
    ];
    Planted { paths, sleeper }
}
