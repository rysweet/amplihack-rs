//! Issue #1634: every Claude profile shares one session store with
//! `~/.claude` (`projects/`, `file-history/`, `session-env/` are links to it),
//! while logins stay per profile.
//!
//! Runs the real `amplihack` binary with a temporary `HOME` and profile root,
//! asserted before every command, and a fake `claude` on `PATH` that records
//! what it sees through `$CLAUDE_CONFIG_DIR` and can write a session file.
//! Nothing touches the real `~/.claude`. Linux-only where a launch must decide
//! liveness from `/proc`.

#![cfg(unix)]

use crate::support::amplihack_bin::amplihack_bin;
use crate::support::profile_rotate as rotate;
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SHARED: [&str; 3] = ["projects", "file-history", "session-env"];

/// The fake `claude`:
/// - `--version` answers like Claude Code;
/// - records in `$RECORD_DIR/seen.<profile>` whether `projects` is a link and
///   every `projects/*/*.jsonl` it can see, `<profile>` being the basename of
///   `$CLAUDE_CONFIG_DIR`;
/// - with `$WRITE_SESSION`, writes `projects/-work/$WRITE_SESSION.jsonl`, a
///   `file-history/$WRITE_SESSION/` entry and a `session-env/$WRITE_SESSION/`
///   entry through its config dir, as Claude Code does;
/// - prints `done as <profile>`.
const FAKE_CLAUDE: &str = r#"#!/bin/sh
case "$1" in
  --version) echo '2.1.301 (Claude Code)'; exit 0 ;;
esac
d="${CLAUDE_CONFIG_DIR-}"
p=$(basename "${d:-unset}")
{
  if [ -L "$d/projects" ]; then echo 'PROJECTS=link'; else echo 'PROJECTS=real'; fi
  for f in "$d"/projects/*/*.jsonl; do
    [ -e "$f" ] && printf 'SEEN=%s\n' "${f##*/}"
  done
} > "$RECORD_DIR/seen.$p"
if [ -n "$WRITE_SESSION" ]; then
  mkdir -p "$d/projects/-work" "$d/file-history/$WRITE_SESSION" "$d/session-env/$WRITE_SESSION"
  printf '{"sessionId":"%s"}\n' "$WRITE_SESSION" > "$d/projects/-work/$WRITE_SESSION.jsonl"
  printf 'snapshot\n' > "$d/file-history/$WRITE_SESSION/v1"
fi
echo "done as $p"
exit 0
"#;

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn chmod(p: &Path, mode: u32) {
    fs::set_permissions(p, fs::Permissions::from_mode(mode)).unwrap();
}

fn is_link(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink())
}

fn is_real_dir(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok_and(|m| m.is_dir())
}

/// No credential-named file anywhere below the three shared dirs of `canon`.
fn assert_no_credentials_under(canon: &Path) {
    fn walk(p: &Path) {
        let Ok(rd) = fs::read_dir(p) else { return };
        for e in rd {
            let e = e.unwrap();
            let name = e.file_name().to_string_lossy().into_owned();
            assert!(
                !name.starts_with(".credentials") && name != ".claude.json",
                "credential file in the shared store: {}",
                e.path().display()
            );
            if e.file_type().unwrap().is_dir() {
                walk(&e.path());
            }
        }
    }
    for name in SHARED {
        walk(&canon.join(name));
    }
}

/// Path -> (type, mode, size, inode, mtime, link target, contents), links not
/// followed. Equal snapshots mean nothing on disk changed.
fn snapshot(base: &Path) -> BTreeMap<PathBuf, String> {
    fn walk(base: &Path, p: &Path, out: &mut BTreeMap<PathBuf, String>) {
        let m = fs::symlink_metadata(p).unwrap();
        let ft = m.file_type();
        let detail = if ft.is_symlink() {
            format!("link {:?}", fs::read_link(p).unwrap())
        } else if ft.is_file() {
            format!("file {:?}", fs::read(p).unwrap_or_default())
        } else {
            "dir".to_string()
        };
        out.insert(
            p.strip_prefix(base).unwrap().to_path_buf(),
            format!(
                "{detail} {:o} {} {} {:?}",
                m.mode(),
                m.len(),
                m.ino(),
                m.modified().unwrap()
            ),
        );
        if ft.is_dir() {
            for e in fs::read_dir(p).unwrap() {
                walk(base, &e.unwrap().path(), out);
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(base, base, &mut out);
    out
}

struct Fixture {
    tmp: tempfile::TempDir,
    home: PathBuf,
    canon: PathBuf,
    root: PathBuf,
    records: PathBuf,
    work: PathBuf,
    tools: PathBuf,
}

impl Fixture {
    /// `~/.claude` exists (0700) before any profile is added, so `add` links.
    fn new(profiles: &[&str]) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        chmod(tmp.path(), 0o700);
        let base = tmp.path().canonicalize().unwrap();
        let home = base.join("home");
        let canon = home.join(".claude");
        let records = base.join("records");
        let work = base.join("work");
        let tools = base.join("tools");
        for d in [&home, &canon, &records, &work, &tools] {
            fs::create_dir_all(d).unwrap();
        }
        chmod(&home, 0o700);
        chmod(&canon, 0o700);
        crate::support::exec_stub::write_executable(&tools.join("claude"), FAKE_CLAUDE, 0o755);
        let f = Self {
            root: home.join(".claude-profiles"),
            tmp,
            home,
            canon,
            records,
            work,
            tools,
        };
        for p in profiles {
            f.ok(&["profile", "add", p]);
        }
        f
    }

    fn dir(&self, p: &str) -> PathBuf {
        self.root.join(p)
    }

    /// Turns `<profile>/<name>` back into a pre-#1634 real directory holding
    /// `rel` with `body`, as a profile created before this change has.
    fn legacy(&self, p: &str, rel: &str, body: &str) -> PathBuf {
        let name = rel.split('/').next().unwrap();
        let top = self.dir(p).join(name);
        if is_link(&top) {
            fs::remove_file(&top).unwrap();
        }
        let path = self.dir(p).join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, body).unwrap();
        path
    }

    fn assert_isolated(&self) {
        assert!(
            self.home
                .starts_with(self.tmp.path().canonicalize().unwrap())
        );
        if let Some(real) = std::env::var_os("HOME") {
            assert_ne!(self.home, PathBuf::from(real), "never the real HOME");
        }
    }

    fn cmd(&self, args: &[&str]) -> Command {
        self.assert_isolated();
        let mut c = crate::isolated_amplihack::isolated(amplihack_bin());
        c.args(args)
            .current_dir(&self.work)
            .env(
                "PATH",
                format!("{}:/usr/local/bin:/usr/bin:/bin", self.tools.display()),
            )
            .env("HOME", &self.home)
            .env("AMPLIHACK_CLAUDE_BINARY_PATH", self.tools.join("claude"))
            .env("RECORD_DIR", &self.records)
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
            .env("AMPLIHACK_SKIP_REFLECTION", "1")
            .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", &self.root);
        c
    }

    fn run(&self, c: &mut Command) -> Output {
        // A just-written stub can briefly fail with ETXTBSY; see #1569 tests.
        let mut attempts = 0;
        loop {
            match c.output() {
                Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy && attempts < 50 => {
                    attempts += 1;
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                r => return r.unwrap(),
            }
        }
    }

    fn ok_cmd(&self, c: &mut Command) -> Output {
        let out = self.run(c);
        assert!(
            out.status.success(),
            "{c:?} failed: {}\nstdout: {}\nstderr: {}",
            out.status,
            text(&out.stdout),
            text(&out.stderr),
        );
        out
    }

    fn ok(&self, args: &[&str]) -> Output {
        self.ok_cmd(&mut self.cmd(args))
    }

    /// Gives `p` an OAuth login unless it has a login file: the launch
    /// guard (issue #1700) refuses a profile without one.
    fn login(&self, p: &str) {
        let creds = self.dir(p).join(".credentials.json");
        if fs::symlink_metadata(&creds).is_err() {
            rotate::private_file(&creds, r#"{"claudeAiOauth":{"accessToken":"SENTINEL"}}"#);
        }
    }

    fn launch(&self, p: &str) -> Command {
        self.login(p);
        self.cmd(&[
            "claude",
            "--profile",
            p,
            "--subprocess-safe",
            "--",
            "-p",
            "x",
        ])
    }

    fn seen(&self, p: &str) -> String {
        fs::read_to_string(self.records.join(format!("seen.{p}"))).unwrap_or_default()
    }

    fn assert_linked(&self, p: &str) {
        for name in SHARED {
            let link = self.dir(p).join(name);
            assert!(is_link(&link), "{} is not a link", link.display());
            assert_eq!(fs::read_link(&link).unwrap(), self.canon.join(name));
        }
    }

    fn backups(&self, p: &str, name: &str) -> Vec<PathBuf> {
        let prefix = format!("{name}.pre-share-");
        fs::read_dir(self.dir(p))
            .unwrap()
            .map(|e| e.unwrap())
            .filter(|e| e.file_name().to_string_lossy().starts_with(&prefix))
            .map(|e| e.path())
            .collect()
    }
}

// ------------------------------------------------------------------ add

#[test]
fn add_links_every_profile_and_prints_nothing_new() {
    let f = Fixture::new(&[]);
    let out = f.ok(&["profile", "add", "A"]);
    f.assert_linked("A");
    let stdout = text(&out.stdout);
    assert!(stdout.starts_with("created profile A at "), "{stdout}");
    assert!(!stdout.contains("linked"), "{stdout}");
    let stderr = text(&out.stderr);
    assert!(
        !stderr.contains("amplihack: profile A:"),
        "add printed: {stderr}"
    );
    for name in SHARED {
        assert!(
            is_real_dir(&f.canon.join(name)),
            "{name} not created in ~/.claude"
        );
    }
}

#[test]
fn add_without_a_home_claude_dir_creates_no_links_and_no_home_claude() {
    let f = Fixture::new(&[]);
    fs::remove_dir(&f.canon).unwrap();
    f.ok(&["profile", "add", "A"]);
    assert!(!f.canon.exists(), "~/.claude created");
    for name in SHARED {
        assert!(
            fs::symlink_metadata(f.dir("A").join(name)).is_err(),
            "{name}"
        );
    }
}

// --------------------------------------------------------------- launch

#[cfg(target_os = "linux")]
#[test]
fn a_launch_migrates_a_legacy_profile_and_claude_sees_the_shared_store() {
    let f = Fixture::new(&["A"]);
    f.legacy("A", "projects/-work/s1.jsonl", "{\"sessionId\":\"s1\"}\n");

    let out = f.ok_cmd(&mut f.launch("A"));

    f.assert_linked("A");
    assert_eq!(
        fs::read_to_string(f.canon.join("projects/-work/s1.jsonl")).unwrap(),
        "{\"sessionId\":\"s1\"}\n"
    );
    assert_eq!(f.backups("A", "projects").len(), 1);
    let seen = f.seen("A");
    assert!(
        seen.contains("PROJECTS=link"),
        "check ran after spawn: {seen}"
    );
    assert!(seen.contains("SEEN=s1.jsonl"), "{seen}");
    let stderr = text(&out.stderr);
    assert!(
        stderr.contains(
            "amplihack: profile A: projects: moved 1 files into ~/.claude/projects, 0 identical, 0 conflict"
        ),
        "{stderr}"
    );
    assert!(
        !text(&out.stdout).contains("amplihack: profile"),
        "report on stdout"
    );
}

/// AC4: a session Claude writes under profile A is listed under profile B.
#[test]
fn a_session_created_under_a_is_visible_for_resume_under_b() {
    let f = Fixture::new(&["A", "B"]);
    let mut c = f.launch("A");
    c.env("WRITE_SESSION", "0b9f-session-a");
    f.ok_cmd(&mut c);

    f.ok_cmd(&mut f.launch("B"));

    let seen = f.seen("B");
    assert!(
        seen.contains("SEEN=0b9f-session-a.jsonl"),
        "B cannot see A's session: {seen}"
    );
    assert!(f.dir("B").join("file-history/0b9f-session-a/v1").is_file());
    assert!(f.dir("B").join("session-env/0b9f-session-a").is_dir());
    assert!(
        f.canon
            .join("projects/-work/0b9f-session-a.jsonl")
            .is_file()
    );
}

#[test]
fn a_session_written_in_home_claude_is_visible_under_a_profile() {
    let f = Fixture::new(&["A"]);
    fs::create_dir_all(f.canon.join("projects/-work")).unwrap();
    fs::write(f.canon.join("projects/-work/plain.jsonl"), "{}\n").unwrap();
    f.ok_cmd(&mut f.launch("A"));
    assert!(f.seen("A").contains("SEEN=plain.jsonl"), "{}", f.seen("A"));
}

#[test]
fn repeated_launches_change_nothing_on_disk_and_print_nothing() {
    let f = Fixture::new(&["A"]);
    f.ok_cmd(&mut f.launch("A"));
    // The launch itself may write amplihack's own state; compare only the
    // profile's session links and the shared store.
    let links = |f: &Fixture| -> Vec<String> {
        SHARED
            .iter()
            .map(|n| {
                let p = f.dir("A").join(n);
                let m = fs::symlink_metadata(&p).unwrap();
                format!(
                    "{n} {} {:?} {:?}",
                    m.ino(),
                    m.modified().unwrap(),
                    fs::read_link(&p).ok()
                )
            })
            .collect()
    };
    let before = (links(&f), snapshot(&f.canon));

    let out = f.ok_cmd(&mut f.launch("A"));

    assert_eq!(
        (links(&f), snapshot(&f.canon)),
        before,
        "second launch changed the store"
    );
    assert!(
        !text(&out.stderr).contains("amplihack: profile A:"),
        "a no-op printed: {}",
        text(&out.stderr)
    );
}

#[cfg(target_os = "linux")]
#[test]
fn profile_run_and_login_both_establish_the_links() {
    let f = Fixture::new(&["R", "L"]);
    f.legacy("R", "projects/-work/r.jsonl", "r");
    f.legacy("L", "projects/-work/l.jsonl", "l");

    f.login("R");
    f.ok(&["profile", "run", "R", "--", "-p", "x"]);
    f.ok(&["profile", "login", "L"]);

    f.assert_linked("R");
    f.assert_linked("L");
    assert!(f.canon.join("projects/-work/r.jsonl").is_file());
    assert!(f.canon.join("projects/-work/l.jsonl").is_file());
    assert!(
        f.seen("L").contains("PROJECTS=link"),
        "login checked after spawn"
    );
    // `profile run` checks once: exactly one backup, not one per hook.
    assert_eq!(f.backups("R", "projects").len(), 1);
}

#[cfg(target_os = "linux")]
#[test]
fn switch_links_on_stderr_and_keeps_its_stdout() {
    let f = Fixture::new(&["A"]);
    f.legacy("A", "projects/-work/s.jsonl", "s");
    f.login("A");
    let out = f.ok(&["profile", "switch", "A"]);
    assert_eq!(text(&out.stdout), "default profile: A\n");
    assert!(text(&out.stderr).contains("amplihack: profile A: projects: moved 1 files"));
    f.assert_linked("A");
}

#[test]
fn a_launch_never_fails_because_of_the_shared_store() {
    let f = Fixture::new(&["A"]);
    // A foreign link at a shared name: warned about, left alone, launch runs.
    fs::remove_file(f.dir("A").join("projects")).unwrap();
    std::os::unix::fs::symlink(&f.work, f.dir("A").join("projects")).unwrap();
    let out = f.ok_cmd(&mut f.launch("A"));
    assert!(text(&out.stdout).contains("done as A"));
    assert_eq!(fs::read_link(f.dir("A").join("projects")).unwrap(), f.work);
    assert!(text(&out.stderr).contains("amplihack: profile A: projects is a symlink to "));
}

#[test]
fn the_opt_out_marker_keeps_a_profile_separate() {
    let f = Fixture::new(&[]);
    f.ok(&["profile", "add", "A"]);
    fs::write(f.dir("A").join(".no-shared-sessions"), "").unwrap();
    f.legacy("A", "projects/-work/private.jsonl", "p");
    f.ok_cmd(&mut f.launch("A"));
    assert!(is_real_dir(&f.dir("A").join("projects")));
    assert!(!f.canon.join("projects/-work/private.jsonl").exists());
}

// --------------------------------------------------------------- remove

/// AC10: removing a profile unlinks its session links and keeps the sessions.
#[test]
fn remove_keeps_the_shared_sessions() {
    let f = Fixture::new(&["A", "B"]);
    let mut c = f.launch("A");
    c.env("WRITE_SESSION", "keep-me");
    f.ok_cmd(&mut c);

    f.ok(&["profile", "remove", "A", "--yes"]);

    assert!(!f.dir("A").exists());
    assert!(f.canon.join("projects/-work/keep-me.jsonl").is_file());
    assert!(f.canon.join("file-history/keep-me/v1").is_file());
    assert!(f.dir("B").join("projects/-work/keep-me.jsonl").is_file());
}

// ---------------------------------------------------- logins stay apart

#[cfg(target_os = "linux")]
#[test]
fn logins_are_never_moved_into_the_shared_store_by_a_migration() {
    let f = Fixture::new(&["A"]);
    rotate::private_file(
        &f.dir("A").join(".credentials.json"),
        r#"{"claudeAiOauth":{"accessToken":"SENTINEL-A"}}"#,
    );
    rotate::private_file(&f.dir("A").join(".claude.json"), "{}");
    f.legacy("A", "projects/-work/s1.jsonl", "s1");
    f.legacy("A", "projects/-work/.credentials.json", "SENTINEL-NESTED");
    let ino = fs::metadata(f.dir("A").join(".credentials.json"))
        .unwrap()
        .ino();

    let out = f.ok_cmd(&mut f.launch("A"));

    assert_no_credentials_under(&f.canon);
    assert_eq!(
        fs::metadata(f.dir("A").join(".credentials.json"))
            .unwrap()
            .ino(),
        ino
    );
    let backup = &f.backups("A", "projects")[0];
    assert!(backup.join("-work/.credentials.json").is_file());
    for s in [text(&out.stdout), text(&out.stderr)] {
        assert!(!s.contains("SENTINEL"), "secret printed: {s}");
    }
}

#[cfg(target_os = "linux")]
#[test]
fn rotate_links_both_swapped_profiles_and_keeps_logins_out_of_the_store() {
    let f = rotate::Fixture::new();
    let canon = f.home.join(".claude");
    // The rotate fixture creates ~/.claude after `add`: no links yet.
    for p in rotate::PROFILES {
        assert!(fs::symlink_metadata(f.root.join(p).join("projects")).is_err());
    }
    fs::create_dir_all(f.root.join("spare/projects/-work")).unwrap();
    fs::write(f.root.join("spare/projects/-work/old.jsonl"), "old").unwrap();

    f.ok(&["profile", "rotate"]);

    for p in ["work", "spare"] {
        for name in SHARED {
            let link = f.root.join(p).join(name);
            assert!(is_link(&link), "{} not linked after rotate", link.display());
            assert_eq!(
                fs::read_link(&link).unwrap(),
                canon.canonicalize().unwrap().join(name)
            );
        }
    }
    assert!(
        fs::symlink_metadata(f.root.join("extra/projects")).is_err(),
        "a profile the swap did not touch was linked"
    );
    assert!(canon.join("projects/-work/old.jsonl").is_file());
    assert_no_credentials_under(&canon);
    f.assert_each_login_once();
}

#[test]
fn a_rotate_dry_run_creates_no_links() {
    let f = rotate::Fixture::new();
    f.ok(&["profile", "rotate", "--dry-run"]);
    for p in rotate::PROFILES {
        assert!(
            fs::symlink_metadata(f.root.join(p).join("projects")).is_err(),
            "{p}"
        );
    }
}
