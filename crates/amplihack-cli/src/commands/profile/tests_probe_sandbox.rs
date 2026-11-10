//! `probe_sandbox` (issue #1974): the usage probe's `--no-session-persistence`
//! flag and the `bwrap` wrapper that hides the shared `projects` folder.
//!
//! Contract:
//! - `NO_PERSIST == "--no-session-persistence"`.
//! - `ProbeSandbox` is `Bwrap(PathBuf)` (an absolute, usable `bwrap`) or
//!   `Unavailable(Unavailable)`; `Unavailable` is `NotFound`, `Refused` or
//!   `Test` (what `detected()` gives under `cfg(test)`).
//! - `wrap(cmd, &sandbox, home) -> (Command, Option<Fallback>)` is pure:
//!   - The config dir comes from `cmd.get_envs()`: `CLAUDE_CONFIG_DIR` set
//!     on `cmd` gives that value; removed gives `<home>/.claude`; not
//!     mentioned gives amplihack's own `CLAUDE_CONFIG_DIR`, else
//!     `<home>/.claude`.
//!   - The hidden folder is `canonicalize(<config>/projects)`. It must
//!     resolve and must not be `/`, `home` or the config dir itself.
//!   - With `Bwrap(b)` and a hidden folder, the command is
//!     `b --dev-bind / / --tmpfs <canon> -- <program> <args...>`, with the
//!     env changes of `cmd` (set and removed) and its current dir.
//!   - Otherwise `cmd` comes back unchanged with a `Fallback`:
//!     `Unavailable(NotFound)` → `Fallback::NotFound`,
//!     `Unavailable(Refused)` → `Fallback::Refused`, a folder that does
//!     not resolve → `Fallback::ProjectsUnresolved`. `Unavailable(Test)`
//!     gives no `Fallback` (and so no note).
//!   - It creates nothing and never changes amplihack's environment.
//! - `Fallback::reason()` is `bwrap not found`, `bwrap refused` or
//!   `projects folder not resolved`.
//! - `Notice::new()` writes, through `note_to(fallback, out)`, the line
//!   `note: usage probes run claude with --no-session-persistence only:
//!   <reason>` the first time only; `emitted()` tells whether it has.
//! - `usage_cmd::probe_with_sandbox(build, &sandbox, home, &notice, mode,
//!   timeout, now, keep_raw)` runs every attempt through `wrap`.
//!
//! Every fake lives in its own `0700` temp dir and is passed in as
//! `Bwrap(path)`; no test changes `PATH` or the process environment, and
//! nothing outside the temp dirs is read or written.

use super::probe_limits::ProbeMode;
use super::probe_sandbox::{Fallback, NO_PERSIST, Notice, ProbeSandbox, Unavailable, wrap};
use super::usage_cmd::probe_with_sandbox;
use super::usage_parse::UsageState;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const NOW: i64 = 1_760_000_000;
const NOTE: &str = "note: usage probes run claude with --no-session-persistence only: ";
const SAFE_PATH: &str = "/usr/local/bin:/usr/bin:/bin";
const USAGE: &str = "printf 'Current session: 37%% used · resets 4pm (UTC)\\nCurrent week (all models): 61%% used · resets Oct 22, 2am (UTC)\\n'";

/// A synthetic host: `home/.claude/projects` (the shared folder, holding
/// one log) and a profile `root/p` whose `projects` links to it, with a
/// `.credentials.json`.
struct Host {
    tmp: tempfile::TempDir,
    home: PathBuf,
    profile: PathBuf,
    shared: PathBuf,
}

impl Host {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let home = base.join("home");
        let shared = home.join(".claude").join("projects");
        fs::create_dir_all(shared.join("-home-someone")).unwrap();
        fs::write(shared.join("-home-someone").join("old.jsonl"), "{}\n").unwrap();
        let profile = base.join("root").join("p");
        fs::create_dir_all(&profile).unwrap();
        fs::set_permissions(&profile, fs::Permissions::from_mode(0o700)).unwrap();
        std::os::unix::fs::symlink(&shared, profile.join("projects")).unwrap();
        let creds = profile.join(".credentials.json");
        fs::write(&creds, r#"{"claudeAiOauth":{"accessToken":"FAKE-1974"}}"#).unwrap();
        fs::set_permissions(&creds, fs::Permissions::from_mode(0o600)).unwrap();
        Self {
            tmp,
            home,
            profile,
            shared,
        }
    }

    fn base(&self) -> PathBuf {
        self.tmp.path().canonicalize().unwrap()
    }

    /// A profile command for `program`, as `run::build_profile_command`
    /// makes one: `CLAUDE_CONFIG_DIR` set on the command only.
    fn profile_command(&self, program: &Path) -> Command {
        let mut c = Command::new(program);
        c.args(["-p", "/usage", NO_PERSIST])
            .env("CLAUDE_CONFIG_DIR", &self.profile)
            .env("PATH", SAFE_PATH);
        c
    }
}

/// Writes an executable script into a new `0700` dir under `base`.
fn script(base: &Path, name: &str, body: &str) -> PathBuf {
    let dir = base.join(format!("bin-{name}"));
    fs::create_dir_all(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    let path = dir.join(name);
    fs::write(&path, body).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    settle(&path);
    path
}

/// Another test thread's fork can hold the script's write descriptor until
/// its exec (ETXTBSY); run it with no arguments until it starts. A fake
/// with no arguments exits at once.
fn settle(path: &Path) {
    let start = Instant::now();
    while Command::new(path).status().is_err() {
        assert!(start.elapsed() < Duration::from_secs(5), "fake never ran");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A fake `bwrap`: records its argv (one per line) in `<dir>/bwrap-argv`,
/// then runs everything after the first `--` as a child (not `exec`), so
/// it stays the group leader as the real one does, and exits with its
/// status.
fn fake_bwrap(base: &Path) -> PathBuf {
    let rec = base.join("bwrap-argv");
    script(
        base,
        "bwrap",
        &format!(
            "#!/bin/sh\n\
             [ $# -gt 0 ] || exit 0\n\
             printf '%s\\n' \"$@\" > '{rec}'\n\
             while [ $# -gt 0 ] && [ \"$1\" != -- ]; do shift; done\n\
             shift\n\
             \"$@\"\n\
             exit $?\n",
            rec = rec.display()
        ),
    )
}

/// A fake `claude`: records `$*`, its `CLAUDE_CONFIG_DIR`, a marker
/// variable and its working dir, then prints `/usage` text.
fn fake_claude(base: &Path) -> PathBuf {
    let d = base.display();
    script(
        base,
        "claude",
        &format!(
            "#!/bin/sh\n\
             [ \"$1\" = -p ] || exit 0\n\
             echo \"$*\" >> '{d}/claude-starts'\n\
             echo \"${{CLAUDE_CONFIG_DIR-unset}}\" > '{d}/claude-config'\n\
             echo \"${{AMPLIHACK_1974_MARK-unset}}\" > '{d}/claude-mark'\n\
             pwd > '{d}/claude-cwd'\n\
             {USAGE}\n"
        ),
    )
}

fn lines(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn argv(cmd: &Command) -> Vec<OsString> {
    std::iter::once(cmd.get_program().to_owned())
        .chain(cmd.get_args().map(OsStr::to_owned))
        .collect()
}

fn os(v: &[&str]) -> Vec<OsString> {
    v.iter().map(OsString::from).collect()
}

fn envs(cmd: &Command) -> Vec<(OsString, Option<OsString>)> {
    let mut v: Vec<_> = cmd
        .get_envs()
        .map(|(k, v)| (k.to_owned(), v.map(OsStr::to_owned)))
        .collect();
    v.sort();
    v
}

// ---------------------------------------------------------------------------
// The flag
// ---------------------------------------------------------------------------

#[test]
fn the_flag_is_no_session_persistence() {
    assert_eq!(NO_PERSIST, "--no-session-persistence");
}

#[test]
fn detection_under_test_never_runs_the_host_bwrap() {
    assert_eq!(
        *super::probe_sandbox::detected(),
        ProbeSandbox::Unavailable(Unavailable::Test)
    );
}

// ---------------------------------------------------------------------------
// AC2: the wrapped command line
// ---------------------------------------------------------------------------

#[test]
fn a_profile_probe_is_wrapped_with_the_resolved_shared_folder() {
    let h = Host::new();
    let bwrap = PathBuf::from("/opt/fake/bwrap");
    let claude = PathBuf::from("/opt/fake/claude");
    let (cmd, fallback) = wrap(
        h.profile_command(&claude),
        &ProbeSandbox::Bwrap(bwrap.clone()),
        &h.home,
    );
    assert_eq!(fallback, None);
    // The profile's `projects` is a link: the target is hidden, not the link.
    let canon = h.shared.canonicalize().unwrap();
    let mut want = vec![
        bwrap.into_os_string(),
        "--dev-bind".into(),
        "/".into(),
        "/".into(),
        "--tmpfs".into(),
        canon.into_os_string(),
        "--".into(),
        claude.into_os_string(),
    ];
    want.extend(os(&["-p", "/usage", NO_PERSIST]));
    assert_eq!(argv(&cmd), want);
}

#[test]
fn no_extra_bwrap_flags_are_passed() {
    let h = Host::new();
    let (cmd, _) = wrap(
        h.profile_command(Path::new("/opt/fake/claude")),
        &ProbeSandbox::Bwrap("/opt/fake/bwrap".into()),
        &h.home,
    );
    for flag in [
        "--die-with-parent",
        "--new-session",
        "--unshare-all",
        "--ro-bind",
    ] {
        assert!(
            !argv(&cmd).iter().any(|a| a == flag),
            "{flag} must not be passed: {:?}",
            argv(&cmd)
        );
    }
}

#[test]
fn the_request_arguments_follow_the_separator_unchanged() {
    let h = Host::new();
    let mut c = Command::new("/opt/fake/claude");
    let args: Vec<String> = super::usage_request::ARGS
        .iter()
        .map(|s| s.to_string())
        .chain([NO_PERSIST.to_string()])
        .collect();
    c.args(&args).env("CLAUDE_CONFIG_DIR", &h.profile);
    let (cmd, _) = wrap(c, &ProbeSandbox::Bwrap("/opt/fake/bwrap".into()), &h.home);
    let all = argv(&cmd);
    let sep = all.iter().position(|a| a == "--").expect("a -- separator");
    assert_eq!(all[sep + 1], "/opt/fake/claude");
    let tail: Vec<OsString> = all[sep + 2..].to_vec();
    assert_eq!(tail, args.iter().map(OsString::from).collect::<Vec<_>>());
}

#[test]
fn a_removed_config_dir_hides_home_dot_claude_projects() {
    let h = Host::new();
    let mut c = Command::new("/opt/fake/claude");
    c.args(["-p", "/usage", NO_PERSIST])
        .env_remove("CLAUDE_CONFIG_DIR")
        .env("AMPLIHACK_CLAUDE_PROFILE", "p");
    let (cmd, fallback) = wrap(c, &ProbeSandbox::Bwrap("/opt/fake/bwrap".into()), &h.home);
    assert_eq!(fallback, None);
    let all = argv(&cmd);
    let at = all.iter().position(|a| a == "--tmpfs").unwrap();
    assert_eq!(
        PathBuf::from(&all[at + 1]),
        h.shared.canonicalize().unwrap()
    );
}

#[test]
fn env_changes_and_working_dir_are_kept_on_the_wrapper() {
    let h = Host::new();
    let cwd = h.base().join("work");
    fs::create_dir_all(&cwd).unwrap();
    let mut c = h.profile_command(Path::new("/opt/fake/claude"));
    c.env("AMPLIHACK_1974_MARK", "kept")
        .env_remove("AMPLIHACK_1974_GONE")
        .current_dir(&cwd);
    let before = envs(&c);
    let parent_before: Vec<_> = std::env::vars_os().collect();
    let (cmd, _) = wrap(c, &ProbeSandbox::Bwrap("/opt/fake/bwrap".into()), &h.home);
    assert_eq!(envs(&cmd), before, "set and removed values both carried");
    assert_eq!(cmd.get_current_dir(), Some(cwd.as_path()));
    let parent_after: Vec<_> = std::env::vars_os().collect();
    assert_eq!(parent_before, parent_after, "amplihack's env is untouched");
}

#[test]
fn the_wrapped_probe_runs_claude_with_its_env_and_cwd() {
    let h = Host::new();
    let base = h.base();
    let bwrap = fake_bwrap(&base);
    let claude = fake_claude(&base);
    let cwd = base.join("work");
    fs::create_dir_all(&cwd).unwrap();
    let mut c = h.profile_command(&claude);
    c.env("AMPLIHACK_1974_MARK", "kept").current_dir(&cwd);
    let (mut cmd, fallback) = wrap(c, &ProbeSandbox::Bwrap(bwrap.clone()), &h.home);
    assert_eq!(fallback, None);
    let out = cmd.output().unwrap();
    assert!(out.status.success(), "{out:?}");
    let recorded = lines(&base.join("bwrap-argv"));
    let canon = h.shared.canonicalize().unwrap().display().to_string();
    let want: Vec<String> = [
        "--dev-bind",
        "/",
        "/",
        "--tmpfs",
        &canon,
        "--",
        &claude.display().to_string(),
        "-p",
        "/usage",
        NO_PERSIST,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(recorded, want);
    assert_eq!(
        lines(&base.join("claude-starts")),
        ["-p /usage --no-session-persistence"]
    );
    assert_eq!(
        lines(&base.join("claude-config")),
        [h.profile.display().to_string()]
    );
    assert_eq!(lines(&base.join("claude-mark")), ["kept"]);
    assert_eq!(lines(&base.join("claude-cwd")), [cwd.display().to_string()]);
}

// ---------------------------------------------------------------------------
// AC3: the fallback
// ---------------------------------------------------------------------------

#[test]
fn an_unavailable_bwrap_leaves_the_command_unchanged() {
    let h = Host::new();
    for (why, want) in [
        (Unavailable::NotFound, Fallback::NotFound),
        (Unavailable::Refused, Fallback::Refused),
    ] {
        let c = h.profile_command(Path::new("/opt/fake/claude"));
        let (before_argv, before_env) = (argv(&c), envs(&c));
        let (cmd, fallback) = wrap(c, &ProbeSandbox::Unavailable(why), &h.home);
        assert_eq!(fallback, Some(want));
        assert_eq!(argv(&cmd), before_argv);
        assert_eq!(envs(&cmd), before_env);
    }
}

#[test]
fn the_test_sandbox_gives_no_fallback_and_no_wrapper() {
    let h = Host::new();
    let c = h.profile_command(Path::new("/opt/fake/claude"));
    let before = argv(&c);
    let (cmd, fallback) = wrap(c, &ProbeSandbox::Unavailable(Unavailable::Test), &h.home);
    assert_eq!(fallback, None);
    assert_eq!(argv(&cmd), before);
}

#[test]
fn a_projects_folder_that_does_not_resolve_is_not_wrapped() {
    let h = Host::new();
    // A broken link.
    fs::remove_file(h.profile.join("projects")).unwrap();
    std::os::unix::fs::symlink(h.base().join("missing"), h.profile.join("projects")).unwrap();
    let c = h.profile_command(Path::new("/opt/fake/claude"));
    let before = argv(&c);
    let (cmd, fallback) = wrap(c, &ProbeSandbox::Bwrap("/opt/fake/bwrap".into()), &h.home);
    assert_eq!(fallback, Some(Fallback::ProjectsUnresolved));
    assert_eq!(argv(&cmd), before);
    // No folder was created to make it resolve.
    assert!(!h.base().join("missing").exists());

    // No `projects` at all.
    fs::remove_file(h.profile.join("projects")).unwrap();
    let c = h.profile_command(Path::new("/opt/fake/claude"));
    let (_, fallback) = wrap(c, &ProbeSandbox::Bwrap("/opt/fake/bwrap".into()), &h.home);
    assert_eq!(fallback, Some(Fallback::ProjectsUnresolved));
    assert!(!h.profile.join("projects").exists(), "nothing created");
}

#[test]
fn a_projects_folder_resolving_to_root_home_or_the_config_dir_is_not_wrapped() {
    for target in ["root", "home", "self"] {
        let h = Host::new();
        fs::remove_file(h.profile.join("projects")).unwrap();
        let to = match target {
            "root" => PathBuf::from("/"),
            "home" => h.home.clone(),
            _ => h.profile.clone(),
        };
        std::os::unix::fs::symlink(&to, h.profile.join("projects")).unwrap();
        let c = h.profile_command(Path::new("/opt/fake/claude"));
        let before = argv(&c);
        let (cmd, fallback) = wrap(c, &ProbeSandbox::Bwrap("/opt/fake/bwrap".into()), &h.home);
        assert_eq!(fallback, Some(Fallback::ProjectsUnresolved), "{target}");
        assert_eq!(argv(&cmd), before, "{target}");
    }
}

#[test]
fn fallback_reasons_name_no_path() {
    assert_eq!(Fallback::NotFound.reason(), "bwrap not found");
    assert_eq!(Fallback::Refused.reason(), "bwrap refused");
    assert_eq!(
        Fallback::ProjectsUnresolved.reason(),
        "projects folder not resolved"
    );
}

#[test]
fn the_note_is_written_once_per_notice() {
    let notice = Notice::new();
    assert!(!notice.emitted());
    let mut out = Vec::new();
    notice.note_to(Fallback::NotFound, &mut out);
    notice.note_to(Fallback::ProjectsUnresolved, &mut out);
    notice.note_to(Fallback::Refused, &mut out);
    let text = String::from_utf8(out).unwrap();
    assert_eq!(text, format!("{NOTE}bwrap not found\n"));
    assert!(notice.emitted());
}

#[test]
fn each_reason_gives_its_own_note_line() {
    for f in [
        Fallback::NotFound,
        Fallback::Refused,
        Fallback::ProjectsUnresolved,
    ] {
        let mut out = Vec::new();
        Notice::new().note_to(f, &mut out);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            format!("{NOTE}{}\n", f.reason())
        );
    }
}

#[test]
fn probes_without_bwrap_run_claude_directly_with_the_flag_and_note_once() {
    let h = Host::new();
    let base = h.base();
    let claude = fake_claude(&base);
    let notice = Notice::new();
    let sandbox = ProbeSandbox::Unavailable(Unavailable::NotFound);
    for _ in 0..3 {
        let build = |a: &[String]| {
            let mut c = Command::new(&claude);
            c.args(a)
                .env("CLAUDE_CONFIG_DIR", &h.profile)
                .env("PATH", SAFE_PATH);
            Ok(c)
        };
        let p = probe_with_sandbox(
            build,
            &sandbox,
            &h.home,
            &notice,
            ProbeMode::Text,
            Duration::from_secs(20),
            NOW,
            false,
        )
        .unwrap();
        assert_eq!(p.report.state, UsageState::Ok);
    }
    assert!(notice.emitted());
    assert_eq!(
        lines(&base.join("claude-starts")),
        ["-p /usage --no-session-persistence"; 3]
    );
    assert!(!base.join("bwrap-argv").exists());
}

#[test]
fn the_test_sandbox_writes_no_note() {
    let h = Host::new();
    let base = h.base();
    let claude = fake_claude(&base);
    let notice = Notice::new();
    let build = |a: &[String]| {
        let mut c = Command::new(&claude);
        c.args(a).env("CLAUDE_CONFIG_DIR", &h.profile);
        Ok(c)
    };
    probe_with_sandbox(
        build,
        &ProbeSandbox::Unavailable(Unavailable::Test),
        &h.home,
        &notice,
        ProbeMode::Text,
        Duration::from_secs(20),
        NOW,
        false,
    )
    .unwrap();
    assert!(!notice.emitted());
}

// ---------------------------------------------------------------------------
// End to end through probe_with_sandbox, and AC4: nothing on the host moves
// ---------------------------------------------------------------------------

fn listing(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

fn tree(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p.clone());
            }
            out.push(p);
        }
    }
    out.sort();
    out
}

#[test]
fn a_sandboxed_probe_changes_nothing_in_the_profile_or_shared_folder() {
    let h = Host::new();
    let base = h.base();
    let bwrap = fake_bwrap(&base);
    let claude = fake_claude(&base);
    let creds = fs::read(h.profile.join(".credentials.json")).unwrap();
    let link = fs::read_link(h.profile.join("projects")).unwrap();
    let profile_list = listing(&h.profile);
    let shared_tree = tree(&h.shared);
    let notice = Notice::new();
    for mode in [ProbeMode::Auto, ProbeMode::Text] {
        let build = |a: &[String]| {
            let mut c = Command::new(&claude);
            c.args(a)
                .env("CLAUDE_CONFIG_DIR", &h.profile)
                .env("PATH", SAFE_PATH);
            Ok(c)
        };
        let p = probe_with_sandbox(
            build,
            &ProbeSandbox::Bwrap(bwrap.clone()),
            &h.home,
            &notice,
            mode,
            Duration::from_secs(20),
            NOW,
            false,
        )
        .unwrap();
        assert_eq!(p.report.state, UsageState::Ok, "{mode:?}");
    }
    assert!(!notice.emitted(), "no fallback, no note");
    // Text runs `/usage` directly; Auto's request gets no answer from this
    // fake, so it falls back to `/usage`: every start went through bwrap.
    let starts = lines(&base.join("claude-starts"));
    assert!(!starts.is_empty());
    assert!(starts.iter().all(|s| s.ends_with(NO_PERSIST)), "{starts:?}");
    assert_eq!(
        fs::read(h.profile.join(".credentials.json")).unwrap(),
        creds
    );
    assert_eq!(fs::read_link(h.profile.join("projects")).unwrap(), link);
    assert_eq!(listing(&h.profile), profile_list);
    assert_eq!(
        tree(&h.shared),
        shared_tree,
        "no file added to the shared folder"
    );
}

#[test]
fn the_group_kill_ends_what_claude_started_under_bwrap() {
    let h = Host::new();
    let base = h.base();
    let bwrap = fake_bwrap(&base);
    let fifo = base.join("held");
    let c = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: `c` is a valid NUL-terminated path.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    // `claude` opens the FIFO for writing, leaves a child holding it, and
    // exits. The reader sees end-of-file only when that child is gone, so
    // the test waits on the event, not on a clock.
    let claude = script(
        &base,
        "claude",
        &format!(
            "#!/bin/sh\n\
             [ \"$1\" = -p ] || exit 0\n\
             exec 3>'{f}'\n\
             sleep 1000 </dev/null >/dev/null 2>&1 &\n\
             exec 3>&-\n\
             {USAGE}\n",
            f = fifo.display()
        ),
    );
    let reader = {
        let fifo = fifo.clone();
        std::thread::spawn(move || {
            let mut f = fs::File::open(&fifo).unwrap();
            let mut buf = Vec::new();
            f.read_to_end(&mut buf).unwrap();
        })
    };
    let build = |a: &[String]| {
        let mut c = Command::new(&claude);
        c.args(a)
            .env("CLAUDE_CONFIG_DIR", &h.profile)
            .env("PATH", SAFE_PATH);
        Ok(c)
    };
    let p = probe_with_sandbox(
        build,
        &ProbeSandbox::Bwrap(bwrap),
        &h.home,
        &Notice::new(),
        ProbeMode::Text,
        Duration::from_secs(20),
        NOW,
        false,
    )
    .unwrap();
    assert_eq!(p.report.state, UsageState::Ok);
    reader
        .join()
        .expect("the leftover child was killed with the group");
}
