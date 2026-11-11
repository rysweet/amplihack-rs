//! `agent register` and `agent unregister` (design 4.10, R2.6, R3). Register
//! reads a running `claude` process when there is one, lets every flag
//! replace what was found (R3.4), validates the result, and replaces the
//! entry in one locked write that refuses a conversation or tmux session
//! another active agent already has (C3, D9).

use super::RegisterArgs;
use super::discover::{self, Discovered, Target};
use super::fields::{Launch, check_profile, check_socket, subject};
use super::registry::{self, AmplihackFlags};
use super::store::{self, AgentsDir};
use super::tmux::{self, Tmux};
use super::{env_record, procfs, sessions, validate};
use anyhow::anyhow;
use std::path::{Path, PathBuf};

const NO_PROCESS: &str =
    "no claude process found; pass --session, --window, --dir and --conversation";

/// The lexical rules for every value given as a flag, before anything is
/// read or asked.
fn check_flags(a: &RegisterArgs) -> anyhow::Result<()> {
    subject(validate::agent_name(&a.name), "NAME")?;
    let opt = |v: &Option<String>, rule: fn(&str) -> Result<(), String>, s: &str| {
        v.as_deref().map_or(Ok(()), |v| subject(rule(v), s))
    };
    opt(&a.conversation, validate::conversation_id, "--conversation")?;
    opt(&a.session, validate::session_name, "--session")?;
    opt(&a.window, validate::window_name, "--window")?;
    opt(&a.model, validate::option_value, "--model")?;
    opt(&a.effort, validate::option_value, "--effort")?;
    opt(
        &a.permission_mode,
        validate::option_value,
        "--permission-mode",
    )?;
    opt(&a.profile, validate::profile_name, "--profile")?;
    if let Some(s) = &a.tmux_socket {
        subject(validate::socket_path(s), "--tmux-socket")?;
    }
    if let Some(d) = &a.dir {
        subject(validate::launch_dir(d), "--dir")?;
    }
    Ok(())
}

/// `a, b and c`.
fn and_list(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [one] => one.to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

pub fn run_register(args: RegisterArgs, home: &Path) -> anyhow::Result<()> {
    check_flags(&args)?;
    let flag_env: Vec<(String, String)> = args
        .env
        .iter()
        .map(|kv| env_record::parse_flag(kv).map_err(|e| anyhow!(e)))
        .collect::<anyhow::Result<_>>()?;
    // The detailed form names `~/.claude` or the profile (S10, B6).
    let dirs = sessions::config_dirs_from(home, |k| std::env::var_os(k)).map_err(|e| anyhow!(e))?;
    let target = match (args.pid, &args.pane) {
        (Some(pid), _) => Target::Pid(pid),
        (None, Some(pane)) => Target::Pane(pane.clone()),
        (None, None) => Target::Ancestor {
            conversation: args.conversation.clone(),
        },
    };
    // `--pane` asks the server of `--tmux-socket`, else the caller's own.
    let caller = std::env::var("TMUX")
        .ok()
        .and_then(|v| tmux::socket_from_env(&v));
    let pane_socket = args.tmux_socket.clone().or(caller);
    if let (Target::Pane(_), Some(s)) = (&target, &pane_socket) {
        check_socket(s)?;
    }
    let pane_tmux = Tmux::new(pane_socket.as_deref());
    let found = discover::discover(&target, &procfs::proc_root(), &dirs, &pane_tmux)?;
    for w in found.iter().flat_map(|f| &f.warnings) {
        eprintln!("warning: {w}");
    }
    let socket = match (&target, &found) {
        (Target::Pane(_), _) => Some(pane_socket.unwrap_or_else(tmux::default_socket)),
        (_, Some(f)) => args.tmux_socket.clone().or_else(|| f.tmux_socket.clone()),
        (_, None) => args.tmux_socket.clone(),
    };
    if let Some(s) = &socket {
        check_socket(s)?;
    }
    let (session, window) = names(&args, &target, found.as_ref(), &pane_tmux)?;
    let launch_dir = args.dir.clone().or_else(|| found_dir(found.as_ref()));
    let found_conversation = found.as_ref().and_then(|f| f.conversation.clone());
    let conversation = args.conversation.clone().or(found_conversation);
    let gaps = [
        session.is_none(),
        window.is_none(),
        launch_dir.is_none(),
        conversation.is_none(),
    ];
    let (Some(session), Some(window), Some(launch_dir), Some(conversation)) =
        (session, window, launch_dir, conversation)
    else {
        return Err(missing(found.as_ref(), gaps));
    };
    let name = args.name.clone();
    let required = (session, window, launch_dir, conversation);
    let mut launch = merge(args, found.as_ref(), home, socket, required)?;
    launch.env.extend(flag_env);
    let line = format!(
        "registered {name}: session {}, window {}, conversation {}, directory {}",
        launch.session,
        launch.window,
        launch.conversation.as_deref().unwrap_or("-"),
        launch.launch_dir.display()
    );
    let dir = AgentsDir::open(home)?;
    let now = registry::now();
    let process = found.as_ref().map(|f| (f.claude_pid, f.proc_start));
    store::update(&dir, now, |reg| {
        launch.check_conflicts(reg, &name)?;
        let previous = reg.agents.get(&name).map(|e| e.registered_at.clone());
        let at = registry::next_registered_at(previous.as_deref(), now);
        let mut e = launch.into_entry(at);
        e.set_process(process);
        reg.agents.insert(name.clone(), e);
        Ok(())
    })?;
    println!("{line}");
    Ok(())
}

/// The tmux session and window: the flags, and tmux for any not given,
/// asked about the pane: `--pane` on its server, or the process's
/// `TMUX_PANE` on the server in its own `$TMUX`, whatever `--tmux-socket`
/// says. Without both of those, tmux is not asked.
fn names(
    args: &RegisterArgs,
    target: &Target,
    found: Option<&Discovered>,
    pane_tmux: &Tmux,
) -> anyhow::Result<(Option<String>, Option<String>)> {
    let (s, w) = (args.session.clone(), args.window.clone());
    if (s.is_some() && w.is_some()) || found.is_none() {
        return Ok((s, w));
    }
    let (pane, asked) = match target {
        Target::Pane(t) => (t.clone(), None),
        _ => {
            let own = found.and_then(|f| Some((f.tmux_pane.clone()?, f.tmux_socket.clone()?)));
            let Some((pane, socket)) = own else {
                return Ok((s, w));
            };
            (pane, Some(Tmux::new(Some(&socket))))
        }
    };
    let tmux = asked.as_ref().unwrap_or(pane_tmux);
    let (ds, dw) = tmux.names(&pane).map_err(|e| anyhow!(e))?;
    Ok((s.or(Some(ds)), w.or(Some(dw))))
}

/// The discovered launch directory, when it passes the launch-directory
/// rule; otherwise a warning with its length only (S11).
fn found_dir(found: Option<&Discovered>) -> Option<PathBuf> {
    let f = found?;
    let d = f.launch_dir.clone()?;
    match validate::launch_dir(&d) {
        Ok(()) => Some(d),
        Err(rule) => {
            let n = d.as_os_str().len();
            eprintln!(
                "warning: ignored the directory of process {} ({n} bytes): {rule}",
                f.claude_pid
            );
            None
        }
    }
}

/// The refusal for a session, window, directory or conversation still
/// missing after discovery; `gaps` marks them in that order.
fn missing(found: Option<&Discovered>, gaps: [bool; 4]) -> anyhow::Error {
    let Some(f) = found else {
        return anyhow!(NO_PROCESS);
    };
    let fields = [
        ("the tmux session", "--session"),
        ("the tmux window", "--window"),
        ("the directory", "--dir"),
        ("the conversation", "--conversation"),
    ];
    let (items, flags): (Vec<&str>, Vec<&str>) = fields
        .iter()
        .zip(gaps)
        .filter_map(|(field, gap)| gap.then_some(*field))
        .unzip();
    anyhow!(
        "claude process {} found, but {} could not be discovered; pass {}",
        f.claude_pid,
        and_list(&items),
        and_list(&flags)
    )
}

/// The recorded options: each flag replaces what was found (R3.4), except
/// the profile, which follows the directory holding the sessions file.
fn merge(
    args: RegisterArgs,
    found: Option<&Discovered>,
    home: &Path,
    socket: Option<PathBuf>,
    (session, window, launch_dir, conversation): (String, String, PathBuf, String),
) -> anyhow::Result<Launch> {
    let opts = found.map(|f| f.opts.clone()).unwrap_or_default();
    let amp = found.and_then(|f| f.amplihack.clone()).unwrap_or_default();
    let raw_dirs: Vec<PathBuf> = if args.add_dir.is_empty() {
        opts.add_dirs.iter().map(PathBuf::from).collect()
    } else {
        args.add_dir
    };
    let add_dirs = raw_dirs
        .iter()
        .map(|d| subject(validate::add_dir(d, &launch_dir), "--add-dir"))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let (permission_mode, skip_permissions) =
        match (args.permission_mode, args.dangerously_skip_permissions) {
            (Some(mode), _) => (Some(mode), false),
            (None, true) => (None, true),
            (None, false) => (opts.permission_mode.clone(), opts.skip_permissions),
        };
    // The profile whose directory held the sessions file, as the conversation
    // is there: a `--profile` naming the rotator's live login ran against
    // `~/.claude`. Without one, `--profile`, else the ancestor's `--profile`.
    let given = args.profile.map(|p| check_profile(home, &p)).transpose()?;
    let profile = match found.and_then(|f| f.config_dir.as_deref()) {
        Some(d) => {
            let p = sessions::profile_of_config_dir(home, d);
            if let Some(g) = given.as_deref().filter(|g| p.as_deref() != Some(*g)) {
                eprintln!(
                    "note: --profile {g} not recorded; the conversation is in {}",
                    p.as_deref()
                        .map_or("~/.claude".to_string(), |p| format!("profile {p}"))
                );
            }
            p
        }
        None => given.or_else(|| {
            amp.profile
                .and_then(|p| found.and_then(|f| found_profile(home, f, p)))
        }),
    };
    Ok(Launch {
        socket,
        session,
        window,
        launch_dir,
        conversation: Some(conversation),
        model: args.model.or(opts.model),
        effort: args.effort.or(opts.effort),
        add_dirs,
        permission_mode,
        skip_permissions,
        flags: AmplihackFlags {
            subprocess_safe: args.subprocess_safe || amp.subprocess_safe,
            no_reflection: args.no_reflection || amp.no_reflection,
            profile,
        },
        env: found.map(|f| f.env.clone()).unwrap_or_default(),
    })
}

/// The `amplihack claude` ancestor's profile, when its directory still
/// exists; otherwise a warning with its length only, so that no error names
/// a `--profile` that was not given.
fn found_profile(home: &Path, f: &Discovered, p: String) -> Option<String> {
    match sessions::profile_config_dir(home, Some(&p)) {
        Ok(_) => Some(p),
        Err(e) => {
            let why = e.strip_prefix("--profile: ").unwrap_or(&e);
            let (pid, n) = (f.claude_pid, p.len());
            eprintln!("warning: ignored the profile of process {pid} ({n} bytes): {why}");
            None
        }
    }
}

/// `agent unregister NAME`: removes the entry in one locked write.
pub fn run_unregister(name: &str, home: &Path) -> anyhow::Result<()> {
    subject(validate::agent_name(name), "NAME")?;
    let unknown = || anyhow!("no registered agent named {name}");
    let dir = AgentsDir::existing(home)?.ok_or_else(unknown)?;
    store::update(&dir, registry::now(), |reg| {
        reg.agents.remove(name).map(|_| ()).ok_or_else(unknown)
    })?;
    println!("unregistered {name}");
    Ok(())
}
