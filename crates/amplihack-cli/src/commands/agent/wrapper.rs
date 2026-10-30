//! `amplihack claude --supervise NAME` (design 5.2, D1, D3, D4): records the
//! session before the unchanged launch, finds its Claude process and
//! conversation in the background, and marks it `ended` only after a clean
//! return with no SIGHUP or SIGTERM. A non-zero Claude exit never returns
//! here (`run_launch_core` exits the process), so the entry stays `active`
//! and the supervisor resumes it. Nothing is written to the terminal while
//! Claude runs (B4).

use super::WrapRequest;
use super::fields::{Launch, check_profile, check_socket, subject};
use super::registry::{self, AmplihackFlags, EndedBy, State};
use super::store::{self, AgentsDir};
use super::tmux::{self, Tmux};
use super::{cmdline, env_record, procfs, report, sessions, validate};
use anyhow::{anyhow, bail};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const NO_TMUX: &str = "amplihack claude --supervise needs tmux: $TMUX and $TMUX_PANE are not set";
const PRINT_MODE: &str = "amplihack claude --supervise needs an interactive session; \
                          -p and --print are not supported";
/// How long the id thread looks for the Claude process (D4).
const ID_WATCH_SECS: u32 = 120;

pub fn run_wrapped(
    req: &WrapRequest,
    launch: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    if !cfg!(target_os = "linux") {
        bail!("amplihack claude --supervise needs Linux; nothing was started");
    }
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let (Some(tmux_var), Some(pane)) = (var("TMUX"), var("TMUX_PANE")) else {
        bail!(NO_TMUX);
    };
    subject(validate::agent_name(req.name), "NAME")?;
    let mut argv = vec!["claude".to_string()];
    argv.extend(req.claude_args.iter().cloned());
    let (opts, warnings) = cmdline::parse_claude(&argv);
    if opts.print {
        bail!(PRINT_MODE);
    }
    let socket = tmux::socket_from_env(&tmux_var);
    if let Some(s) = &socket {
        check_socket(s)?;
    }
    let (session, window) = Tmux::new(socket.as_deref())
        .names(&pane)
        .map_err(|e| anyhow!(e))?;
    let cwd = std::env::current_dir()
        .map_err(|e| anyhow!("cannot read the current directory: {}", e.kind()))?;
    subject(validate::launch_dir(&cwd), "current directory")?;
    let add_dirs = opts
        .add_dirs
        .iter()
        .map(|d| subject(validate::add_dir(Path::new(d), &cwd), "--add-dir"))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let home = crate::user_config::home_dir()
        .ok_or_else(|| anyhow!("cannot find the home directory: HOME is not set"))?;
    let profile = req.profile.map(|p| check_profile(&home, p)).transpose()?;
    let (env, refused) = env_record::from_current_env();
    for w in warnings {
        eprintln!("warning: {w}");
    }
    for (name, rule) in refused {
        let pid = std::process::id();
        eprintln!("warning: ignored {name} from the environment of process {pid}: {rule}");
    }
    let fields = Launch {
        socket,
        session,
        window,
        launch_dir: cwd,
        conversation: cmdline::resume_as_conversation(&opts).map(String::from),
        model: opts.model,
        effort: opts.effort,
        add_dirs,
        permission_mode: opts.permission_mode,
        skip_permissions: opts.skip_permissions,
        flags: AmplihackFlags {
            subprocess_safe: req.subprocess_safe,
            no_reflection: req.no_reflection,
            profile,
        },
        env,
    };
    // Watched before the registry write, so a failure here writes nothing.
    let stopped = Arc::new(AtomicBool::new(false));
    for sig in [signal_hook::consts::SIGHUP, signal_hook::consts::SIGTERM] {
        signal_hook::flag::register(sig, Arc::clone(&stopped))
            .map_err(|e| anyhow!("cannot watch for SIGHUP and SIGTERM: {e}"))?;
    }
    let dir = AgentsDir::open(&home)?;
    let launch_id = registry::new_launch_id();
    start(&dir, req.name, &launch_id, fields)?;
    watch_for_claude(dir.clone(), home, req.name.to_string(), launch_id.clone());
    let result = launch();
    if result.is_ok()
        && !stopped.load(Ordering::SeqCst)
        && let Err(e) = mark_ended(&dir, req.name, &launch_id)
    {
        eprintln!(
            "warning: could not mark agent {} as ended: {e:#}; the supervisor may resume it",
            req.name
        );
    }
    result
}

/// Creates the entry, or updates it in place keeping its restart history,
/// `registered_at` and `last_recovery` (D3).
fn start(dir: &AgentsDir, name: &str, launch_id: &str, fields: Launch) -> anyhow::Result<()> {
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
fn mark_ended(dir: &AgentsDir, name: &str, launch_id: &str) -> anyhow::Result<()> {
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

/// A detached thread: every second for up to two minutes it looks for the
/// Claude process below this one and its sessions file, and records both
/// once found (D4). Giving up, or failing to record them, appends one
/// `WARNING:` line to the supervisor log with the last reason; nothing is
/// written to the terminal (B4).
fn watch_for_claude(dir: AgentsDir, home: PathBuf, name: String, launch_id: String) {
    let spawned = std::thread::Builder::new()
        .name("agent-id-watch".into())
        .spawn(move || {
            let root = procfs::proc_root();
            let dirs = || sessions::config_dirs_from(&home, |k| std::env::var_os(k));
            let mut last_error = None;
            for _ in 0..ID_WATCH_SECS {
                std::thread::sleep(Duration::from_secs(1));
                match find_claude(&root, dirs) {
                    Ok(Some(found)) => {
                        if let Err(e) = record_claude(&dir, &home, &name, &launch_id, found) {
                            not_recorded(&dir, &name, &format!("{e:#}"));
                        }
                        return;
                    }
                    Ok(None) => {}
                    Err(e) => last_error = Some(e),
                }
            }
            let reason = last_error.unwrap_or_else(|| {
                format!(
                    "no claude process with a session file found within {ID_WATCH_SECS} seconds"
                )
            });
            not_recorded(&dir, &name, &reason);
        });
    if let Err(e) = spawned {
        tracing::debug!("agent: could not start the id thread: {e}");
    }
}

/// Appends the id thread's warning to the supervisor log.
fn not_recorded(dir: &AgentsDir, name: &str, reason: &str) {
    let line = report::not_recorded_line(registry::now(), name, reason);
    if let Err(e) = report::append(&dir.log(), &line) {
        tracing::debug!("agent {name}: could not log that its conversation was not recorded: {e}");
    }
}

/// The nearest Claude process below this one, its start time, and the
/// conversation in its sessions file; `dirs` lists the config directories
/// once a process is found. `Ok(None)` until both exist; an error is why
/// they cannot be found.
pub(super) fn find_claude(
    root: &Path,
    dirs: impl FnOnce() -> Result<Vec<PathBuf>, String>,
) -> Result<Option<Found>, String> {
    let snap = procfs::snapshot(root, amplihack_utils::secure_files::euid())?;
    let found = snap
        .descendants(snap.self_pid)
        .into_iter()
        .find(|&p| snap.readable_claude(p))
        .and_then(|pid| Some((pid, snap.procs.get(&pid)?.starttime)));
    let Some((pid, start)) = found else {
        return Ok(None);
    };
    let file = sessions::for_process_in(&dirs()?, pid, start)?;
    Ok(file.map(|(f, config_dir)| Found {
        pid,
        start,
        conversation: f.session_id,
        config_dir,
    }))
}

/// What the id thread found: the Claude process, its start time, its
/// conversation and the config directory whose `sessions/` named it.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Found {
    pub pid: u32,
    pub start: u64,
    pub conversation: String,
    pub config_dir: PathBuf,
}

/// Records what the id thread found, unless the entry is no longer this
/// launch's or the conversation belongs to another active agent (C3).
/// Without `--profile`, the launch may still have run under a profile (an
/// inherited `AMPLIHACK_CLAUDE_PROFILE`, possibly redirected by auto-switch,
/// or the default pointer); the profile is then taken from the directory
/// the sessions file was in, so the resume uses the directory that holds
/// the conversation. An explicit `--profile` is kept as given.
fn record_claude(
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
            if e.amplihack_flags.profile.is_none() {
                e.amplihack_flags.profile = profile.clone();
            }
        }
        Ok(())
    })
}
