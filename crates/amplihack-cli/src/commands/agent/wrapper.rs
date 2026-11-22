//! `amplihack claude --supervise NAME` (design 5.2, D1, D3, D4): records the
//! session before the unchanged launch, finds its Claude process and
//! conversation in the background, and marks it `ended` only after a clean
//! return with no SIGHUP or SIGTERM. A non-zero Claude exit comes back here
//! as a `CliExitError`, so the entry stays `active` and the supervisor
//! resumes it. Nothing is written to the terminal while Claude runs (B4).
//!
//! Issue #2912: a session marked standing is launched again in the same
//! pane when its process exits other than by its owner. The decision, the
//! wait for a login with room and the logging are all in
//! [`super::standing`]; this file owns the loop that acts on it.

use super::WrapRequest;
use super::entry::{mark_ended, record_claude, start};
use super::fields::{Launch, check_profile, check_socket, subject};
use super::launch::continue_args;
use super::registry::{self, AmplihackFlags};
use super::standing::{Exit, Standing};
use super::store::AgentsDir;
use super::tmux::{self, Tmux};
use super::{cmdline, env_record, procfs, report, sessions, validate};
use anyhow::{anyhow, bail};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

const NO_TMUX: &str = "amplihack claude --supervise needs tmux: $TMUX and $TMUX_PANE are not set";
const PRINT_MODE: &str = "amplihack claude --supervise needs an interactive session; \
                          -p and --print are not supported";
/// How long the id thread looks for the Claude process (D4).
const ID_WATCH_SECS: u32 = 120;

pub fn run_wrapped(
    req: &WrapRequest,
    launch: impl Fn(&[String]) -> anyhow::Result<()>,
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
        launch_dir: cwd.clone(),
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
    let mut launch_id = registry::new_launch_id();
    let first_conversation = fields.conversation.clone();
    let mut since = registry::now();
    start(&dir, req.name, &launch_id, fields)?;

    // Issue #2912: whether this session keeps running, settled once.
    let standing = Standing::resolve(req.standing, req.name, &home);

    let mut args: Vec<String> = req.claude_args.to_vec();
    // Issue #2912: the relaunches since the model last replied, carried from
    // one turn of the loop to the next so the session's record can say how
    // long the chain has run. Evidence for the supervisor agent; nothing
    // here decides on it.
    let mut quiet = None;
    loop {
        // One per run, so the id thread of an earlier run can never say
        // that this one started.
        let run = Arc::new(RunWatch::default());
        watch_for_claude(
            dir.clone(),
            home.clone(),
            req.name.to_string(),
            launch_id.clone(),
            Arc::clone(&run),
        );
        // Issue #2912: when this run began. Whether the model wrote a reply
        // to the session's conversation from then on is read once, after
        // the exit, for the record of the relaunch; see `standing::Quiet`.
        let started = SystemTime::now();
        let result = launch(&args);
        run.over.store(true, Ordering::SeqCst);
        let replied = standing.replied(&dir, &home, &cwd, req.name, started);
        let signalled = stopped.load(Ordering::SeqCst);
        if result.is_ok()
            && !signalled
            && let Err(e) = mark_ended(&dir, req.name, &launch_id)
        {
            eprintln!(
                "warning: could not mark agent {} as ended: {e:#}; the supervisor may resume it",
                req.name
            );
        }
        // Issue #2912: either the next launch of a standing session, or
        // nothing, in which case this wrapper is done and returns what
        // Claude Code gave it.
        let exit = Exit {
            launch_id: &launch_id,
            since,
            first_conversation: first_conversation.as_deref(),
            result: &result,
            signalled,
            replied,
            quiet,
        };
        let Some(next) = standing.next(&dir, req.name, &exit, &stopped) else {
            return result;
        };
        launch_id = next.launch_id;
        quiet = next.quiet;
        since = registry::now();
        // Through the same launch path as the first run, so the relaunch
        // gets what the first launch got from the launcher and nothing
        // more; only the Claude arguments are rebuilt from the record.
        args = continue_args(&next.entry);
    }
}

/// A detached thread: every second for up to two minutes it looks for the
/// Claude process below this one and its sessions file, and records both
/// once found (D4). Giving up, or failing to record them, appends one
/// `WARNING:` line to the supervisor log with the last reason; nothing is
/// written to the terminal (B4).
///
/// Issue #2912: the thread stops at its next tick once `run.over` is set,
/// silently, as it did when the wrapper exited with its one run. A standing
/// session's wrapper outlives each run, and without this every relaunch
/// would leave one more thread scanning the process table each second for
/// the rest of its two minutes, and writing a warning about a run that had
/// already ended.
///
/// Nothing in the standing-session decision rests on what this thread saw.
/// It records the conversation, and the record of a relaunch reads that and
/// the conversation's transcript off the disk after the exit, so a run that
/// ended between two of these ticks is described by what it wrote rather
/// than by whether a scan happened to catch it.
fn watch_for_claude(
    dir: AgentsDir,
    home: PathBuf,
    name: String,
    launch_id: String,
    run: Arc<RunWatch>,
) {
    let spawned = std::thread::Builder::new()
        .name("agent-id-watch".into())
        .spawn(move || {
            let root = procfs::proc_root();
            let dirs = || sessions::config_dirs_from(&home, |k| std::env::var_os(k));
            let mut last_error = None;
            for _ in 0..ID_WATCH_SECS {
                std::thread::sleep(Duration::from_secs(1));
                if run.over.load(Ordering::SeqCst) {
                    return;
                }
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

/// What one run's id thread and the wrapper share (issue #2912).
#[derive(Default)]
struct RunWatch {
    /// The run has returned, so its id thread has nothing left to find.
    over: AtomicBool,
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
