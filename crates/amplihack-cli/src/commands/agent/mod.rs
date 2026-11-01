//! `amplihack agent` (issue #1760): a registry of long-lived Claude sessions
//! in tmux, a supervisor that resumes each conversation once when its tmux
//! session is lost, and `amplihack claude --supervise NAME`, which records a
//! session at launch and its clean exit at the end.
//!
//! The clap types build on every platform; the implementation is Unix only
//! and the parts that read `/proc` check for Linux at run time (A13).

use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Subcommand)]
pub enum AgentCommands {
    /// Record a long-lived Claude session so `agent supervise` can resume it.
    Register(Box<RegisterArgs>),
    /// Remove an agent from the registry.
    Unregister {
        #[arg(value_name = "NAME")]
        name: String,
    },
    /// Resume registered agents whose terminal session was lost.
    Supervise(SuperviseArgs),
    /// Show registered agents and their last recovery.
    Status {
        /// Print one JSON document instead of a table.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Args)]
pub struct RegisterArgs {
    /// Agent name: letters, digits, '.', '_' or '-', up to 64 characters.
    #[arg(value_name = "NAME")]
    pub name: String,
    /// The claude process to record (default: the nearest claude ancestor).
    #[arg(long, value_name = "PID", conflicts_with = "pane")]
    pub pid: Option<u32>,
    /// The tmux pane whose claude process to record.
    #[arg(long, value_name = "TARGET")]
    pub pane: Option<String>,
    /// tmux session name.
    #[arg(long)]
    pub session: Option<String>,
    /// tmux window name.
    #[arg(long)]
    pub window: Option<String>,
    /// tmux server socket.
    #[arg(long = "tmux-socket", value_name = "PATH")]
    pub tmux_socket: Option<PathBuf>,
    /// Directory the session is resumed in.
    #[arg(long, value_name = "PATH")]
    pub dir: Option<PathBuf>,
    /// Conversation id to resume.
    #[arg(long, value_name = "ID")]
    pub conversation: Option<String>,
    /// Claude model to resume with.
    #[arg(long)]
    pub model: Option<String>,
    /// Claude effort level to resume with.
    #[arg(long)]
    pub effort: Option<String>,
    /// Extra directory for Claude (repeatable).
    #[arg(long = "add-dir", value_name = "DIR")]
    pub add_dir: Vec<PathBuf>,
    /// Claude permission mode to resume with.
    #[arg(
        long = "permission-mode",
        conflicts_with = "dangerously_skip_permissions"
    )]
    pub permission_mode: Option<String>,
    /// Resume with --dangerously-skip-permissions.
    #[arg(long = "dangerously-skip-permissions")]
    pub dangerously_skip_permissions: bool,
    /// amplihack login profile to resume with.
    #[arg(long)]
    pub profile: Option<String>,
    /// Resume with amplihack's --subprocess-safe.
    #[arg(long = "subprocess-safe")]
    pub subprocess_safe: bool,
    /// Resume with amplihack's --no-reflection.
    #[arg(long = "no-reflection")]
    pub no_reflection: bool,
    /// Allowlisted variable to set when resuming (repeatable).
    #[arg(long = "env", value_name = "KEY=VALUE")]
    pub env: Vec<String>,
}

#[derive(Debug, Args)]
pub struct SuperviseArgs {
    /// Run one pass and exit.
    #[arg(long)]
    pub once: bool,
    /// Seconds from the end of one pass to the start of the next.
    #[arg(long, default_value_t = 60, value_parser = clap::value_parser!(u64).range(1..))]
    pub interval: u64,
    /// Agents resumed together before a gap.
    #[arg(long, default_value_t = 6, value_parser = clap::value_parser!(u64).range(1..))]
    pub batch: u64,
    /// Seconds between batches.
    #[arg(long, default_value_t = 60, value_parser = clap::value_parser!(u64).range(0..))]
    pub gap: u64,
    /// Launch attempts allowed in the trailing hour before an agent is stopped.
    #[arg(long = "max-restarts", default_value_t = 3, value_parser = clap::value_parser!(u64).range(1..))]
    pub max_restarts: u64,
}

/// What the `Claude` arm hands the wrapper.
pub struct WrapRequest<'a> {
    pub name: &'a str,
    pub profile: Option<&'a str>,
    pub subprocess_safe: bool,
    pub no_reflection: bool,
    pub claude_args: &'a [String],
}

#[cfg(unix)]
mod claude_names;
#[cfg(unix)]
mod cmdline;
#[cfg(unix)]
mod dirty;
#[cfg(unix)]
mod discover;
#[cfg(unix)]
mod env_record;
#[cfg(unix)]
mod fields;
#[cfg(unix)]
mod launch;
#[cfg(unix)]
mod live_conversations;
#[cfg(unix)]
mod markers;
#[cfg(unix)]
mod pass;
#[cfg(unix)]
mod procfs;
#[cfg(unix)]
mod profile_dir;
#[cfg(unix)]
mod register;
#[cfg(unix)]
mod registry;
#[cfg(unix)]
mod report;
#[cfg(unix)]
mod resume;
#[cfg(unix)]
mod scan;
#[cfg(unix)]
mod server;
#[cfg(unix)]
mod sessions;
#[cfg(unix)]
mod socket_check;
#[cfg(unix)]
mod store;
#[cfg(unix)]
mod supervise;
#[cfg(unix)]
mod tmux;
#[cfg(unix)]
mod tmux_parse;
#[cfg(unix)]
mod validate;
#[cfg(unix)]
mod wrapper;

#[cfg(unix)]
pub(crate) use live_conversations::unended_conversations;
#[cfg(unix)]
pub use wrapper::run_wrapped;

/// Off Unix there is no tmux to recreate and no `/proc` to read.
#[cfg(not(unix))]
pub fn run_wrapped(
    _req: &WrapRequest,
    _launch: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    anyhow::bail!("amplihack claude --supervise needs Linux; nothing was started")
}

pub fn run_agent(cmd: AgentCommands) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        let home = crate::user_config::home_dir()
            .ok_or_else(|| anyhow::anyhow!("cannot find the home directory: HOME is not set"))?;
        match cmd {
            AgentCommands::Register(args) => register::run_register(*args, &home),
            AgentCommands::Unregister { name } => register::run_unregister(&name, &home),
            AgentCommands::Supervise(args) => supervise::run_supervise(args, &home),
            AgentCommands::Status { json } => run_status(json, &home),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = cmd;
        anyhow::bail!("amplihack agent needs Linux; nothing was done")
    }
}

/// `agent status`: reads without the lock and never creates a directory. A
/// missing directory or file is an empty registry.
#[cfg(unix)]
fn run_status(json: bool, home: &std::path::Path) -> anyhow::Result<()> {
    let reg = match store::AgentsDir::existing(home)? {
        Some(dir) => store::load(&dir).map_err(|e| e.into_error(&dir))?,
        None => registry::Registry::new(),
    };
    if json {
        println!("{}", report::status_json(&reg, registry::now()));
    } else {
        print!("{}", report::status_text(&reg));
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests_cmdline.rs"]
mod tests_cmdline;
#[cfg(test)]
#[path = "tests_env_drift.rs"]
mod tests_env_drift;
#[cfg(test)]
#[path = "tests_env_record.rs"]
mod tests_env_record;
#[cfg(test)]
#[path = "tests_launch.rs"]
mod tests_launch;
#[cfg(test)]
#[path = "tests_markers.rs"]
mod tests_markers;
#[cfg(test)]
#[path = "tests_pass.rs"]
mod tests_pass;
#[cfg(test)]
#[path = "tests_pass_io.rs"]
mod tests_pass_io;
#[cfg(test)]
#[path = "tests_proc_tree.rs"]
mod tests_proc_tree;
#[cfg(test)]
#[path = "tests_procfs.rs"]
mod tests_procfs;
#[cfg(test)]
#[path = "tests_registry.rs"]
mod tests_registry;
#[cfg(test)]
#[path = "tests_report.rs"]
mod tests_report;
#[cfg(test)]
#[path = "tests_report_status.rs"]
mod tests_report_status;
#[cfg(test)]
#[path = "tests_scan.rs"]
mod tests_scan;
#[cfg(test)]
#[path = "tests_scan_observe.rs"]
mod tests_scan_observe;
#[cfg(test)]
#[path = "tests_server.rs"]
mod tests_server;
#[cfg(test)]
#[path = "tests_sessions.rs"]
mod tests_sessions;
#[cfg(test)]
#[path = "tests_sessions_for_process.rs"]
mod tests_sessions_for_process;
#[cfg(test)]
#[path = "tests_socket_check.rs"]
mod tests_socket_check;
#[cfg(test)]
#[path = "tests_store.rs"]
mod tests_store;
#[cfg(test)]
#[path = "tests_supervise.rs"]
mod tests_supervise;
#[cfg(test)]
#[path = "tests_tmux.rs"]
mod tests_tmux;
#[cfg(test)]
#[path = "tests_validate.rs"]
mod tests_validate;
#[cfg(test)]
#[path = "tests_validate_hardening.rs"]
mod tests_validate_hardening;
#[cfg(test)]
#[path = "tests_wrapper.rs"]
mod tests_wrapper;
