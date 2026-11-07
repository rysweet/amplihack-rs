//! Native outside-in regression: ordinary process launches, no tmux server.
#![cfg(target_os = "macos")]

use crate::support::amplihack_bin::amplihack_bin;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

const RUNNER: &str = r#"#!/usr/bin/python3
import json, os, signal, subprocess, sys, time
if os.environ.get('STUB_FAST'):
    print(json.dumps({'recipe_name':'issue-1928', 'success':True, 'step_results':[], 'context':{}}))
    sys.exit(0)
signal.signal(signal.SIGTERM, signal.SIG_IGN)
child = subprocess.Popen([sys.executable, '-c', 'import signal,time; signal.signal(signal.SIGTERM, signal.SIG_IGN); time.sleep(300)'], start_new_session=True)
with open(os.environ['RECORD_DIR']+'/pids.tmp','w') as f:
    json.dump([os.getpid(),child.pid],f)
os.replace(os.environ['RECORD_DIR']+'/pids.tmp',os.environ['RECORD_DIR']+'/pids.json')
while True: time.sleep(1)
"#;

struct Fx {
    tmp: tempfile::TempDir,
    home: PathBuf,
    tools: PathBuf,
    project: PathBuf,
    log: PathBuf,
    recipe: PathBuf,
    amp: Option<Child>,
    owned: Vec<(i32, u64)>,
}

impl Fx {
    fn new() -> Self {
        let tmp = tempfile::Builder::new().prefix("i1928-").tempdir().unwrap();
        let home = tmp
            .path()
            .join("a-long-home-path-that-does-not-hold-the-unix-socket");
        let tools = tmp.path().join("tools");
        let project = tmp.path().join("project");
        for dir in [&home, &tools, &project] {
            fs::create_dir_all(dir).unwrap();
        }
        fs::create_dir(project.join(".git")).unwrap();
        fs::write(project.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(project.join("sentinel"), "source remains unchanged\n").unwrap();
        crate::support::exec_stub::write_executable(&tools.join("recipe-runner-rs"), RUNNER, 0o755);
        // The CLI's session lookup cannot contact a developer's tmux server.
        crate::support::exec_stub::write_executable(
            &tools.join("tmux"),
            "#!/bin/sh\nset -- -S \"$RECORD_DIR/tmux.sock\" \"$@\"\nprintf '%s\\n' \"$@\" > \"$RECORD_DIR/tmux-args\"\nexit 1\n",
            0o755,
        );
        let recipe = tmp.path().join("r.yaml");
        fs::write(&recipe, "name: issue-1928\nversion: 1.0.0\nsteps:\n  - id: wait\n    type: bash\n    command: 'true'\n").unwrap();
        let log = tmp.path().join("stderr.log");
        Self {
            tmp,
            home,
            tools,
            project,
            log,
            recipe,
            amp: None,
            owned: Vec::new(),
        }
    }

    fn command(&self) -> Command {
        let mut cmd = crate::isolated_amplihack::isolated(amplihack_bin());
        cmd.env_clear()
            .env(crate::isolated_amplihack::HOME_GUARD_ENV, "1")
            .env("HOME", &self.home)
            // Keep the socket in the short outer TMPDIR even with a long HOME.
            .env("TMPDIR", std::env::temp_dir())
            .env("PATH", format!("{}:/usr/bin:/bin", self.tools.display()))
            .env("RECIPE_RUNNER_RS_PATH", self.tools.join("recipe-runner-rs"))
            .env("RECORD_DIR", self.tmp.path())
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_AGENT_BINARY", "claude")
            .env("AMPLIHACK_TEARDOWN_GRACE_SECS", "0")
            .current_dir(&self.project)
            .stdin(Stdio::null());
        cmd
    }

    fn start(&mut self) -> String {
        self.amp = Some(
            self.command()
                .args(["recipe", "run"])
                .arg(&self.recipe)
                .arg("-w")
                .arg(&self.project)
                .process_group(0)
                .stdout(Stdio::null())
                .stderr(fs::File::create(&self.log).unwrap())
                .spawn()
                .unwrap(),
        );
        wait(|| self.tmp.path().join("pids.json").is_file());
        let pids: Vec<i32> =
            serde_json::from_slice(&fs::read(self.tmp.path().join("pids.json")).unwrap()).unwrap();
        self.owned = pids
            .into_iter()
            .map(|pid| (pid, native_start(pid).unwrap()))
            .collect();
        let body = fs::read_to_string(&self.log).unwrap();
        let run_id = body
            .lines()
            .filter_map(|line| line.strip_prefix("amplihack.recipe.log_pointer "))
            .map(|json| serde_json::from_str::<serde_json::Value>(json).unwrap())
            .find(|value| value["event"] == "early")
            .unwrap()["run_id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(
            fs::metadata(self.path(&run_id))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        run_id
    }

    fn stop(&self, run_id: &str) -> Output {
        self.command()
            .args(["recipe", "stop", run_id])
            .output()
            .unwrap()
    }

    fn path(&self, run_id: &str) -> PathBuf {
        self.home
            .join(".amplihack/recipe-runs")
            .join(format!("{run_id}.json"))
    }

    fn edit_record(&self, run_id: &str, edit: impl FnOnce(&mut serde_json::Value)) -> Vec<u8> {
        let path = self.path(run_id);
        let old = fs::read(&path).unwrap();
        let mut value = serde_json::from_slice(&old).unwrap();
        edit(&mut value);
        fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
        old
    }
}

impl Drop for Fx {
    fn drop(&mut self) {
        // Only exact recorded test-owned identities, never process groups.
        for (pid, start) in &self.owned {
            if native_start(*pid) == Some(*start) {
                unsafe {
                    libc::kill(*pid, libc::SIGKILL);
                }
            }
        }
        if let Some(mut amp) = self.amp.take() {
            let _ = amp.kill();
            let _ = amp.wait();
        }
    }
}

fn wait(mut condition: impl FnMut() -> bool) {
    let start = Instant::now();
    while !condition() {
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "native process fixture timed out"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

// Native independent observation of start identity for fixture cleanup and
// negative controls. The full SDK layout avoids parsing ps or process names.
#[repr(C)]
#[derive(Default)]
struct Bsd {
    words: [u32; 12],
    comm: [u8; 16],
    name: [u8; 32],
    tail: [u32; 6],
    sec: u64,
    usec: u64,
}
unsafe extern "C" {
    fn proc_pidinfo(pid: i32, flavor: i32, arg: u64, buffer: *mut libc::c_void, size: i32) -> i32;
}
fn native_start(pid: i32) -> Option<u64> {
    let mut info = Bsd::default();
    let size = std::mem::size_of::<Bsd>() as i32;
    let read = unsafe { proc_pidinfo(pid, 3, 0, (&mut info as *mut Bsd).cast(), size) };
    (read == size && info.words[1] != 5).then(|| info.sec * 1_000_000 + info.usec)
}

#[test]
fn ordinary_run_id_stop_kills_escaped_steps_and_preserves_shared_group() {
    let mut fx = Fx::new();
    let run_id = fx.start();
    let pgid = fx.amp.as_ref().unwrap().id() as i32;
    let mut foreign = OwnedChild(
        Command::new("/bin/sleep")
            .arg("300")
            .env("AMPLIHACK_RECIPE_RUN_ID", &run_id)
            .process_group(pgid)
            .spawn()
            .unwrap(),
    );
    let foreign_start = native_start(foreign.0.id() as i32).unwrap();
    let out = fx.stop(&run_id);
    let still_live = foreign.0.try_wait().unwrap().is_none();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("stopped run"));
    assert!(
        still_live,
        "same-env process in launcher's shared group was signalled ({foreign_start})"
    );
    wait(|| {
        fx.owned
            .iter()
            .all(|(pid, start)| native_start(*pid) != Some(*start))
    });
    let amp = fx.amp.as_mut().unwrap();
    wait(|| amp.try_wait().unwrap().is_some());
    assert!(!amp.wait().unwrap().success());
    assert_eq!(
        fs::read_to_string(fx.project.join("sentinel")).unwrap(),
        "source remains unchanged\n"
    );
    assert!(
        !fx.path(&run_id).exists(),
        "terminal launch record was not removed"
    );
    assert!(
        !fx.stop(&run_id).status.success(),
        "dead run must be refused"
    );
}

#[test]
fn stale_reused_foreign_and_ancestor_identities_are_refused() {
    let mut fx = Fx::new();
    let run_id = fx.start();
    let mut foreign = OwnedChild(Command::new("/bin/sleep").arg("300").spawn().unwrap());
    let pid = foreign.0.id() as i32;
    let start = native_start(pid).unwrap();
    for replacement in [
        serde_json::json!({"pid":pid,"start":start + 1}), // Reused/stale start.
        serde_json::json!({"pid":pid,"start":start}), // Live foreign identity, wrong socket peer.
        serde_json::json!({"pid":std::process::id(),"start":native_start(std::process::id() as i32).unwrap()}),
    ] {
        let old = fx.edit_record(&run_id, |record| {
            record["pid"] = replacement["pid"].clone();
            record["start"] = replacement["start"].clone();
        });
        let out = fx.stop(&run_id);
        fs::write(fx.path(&run_id), old).unwrap();
        assert!(!out.status.success(), "forged launch record was accepted");
        assert!(foreign.0.try_wait().unwrap().is_none());
        assert!(
            fx.owned
                .iter()
                .all(|(pid, start)| native_start(*pid) == Some(*start))
        );
    }
    let path = fx.path(&run_id);
    let original = fs::read(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(
        !fx.stop(&run_id).status.success(),
        "public launch record was accepted"
    );
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let mut oversized = original.clone();
    oversized.resize(1024 * 1024 + 1, b' ');
    fs::write(&path, oversized).unwrap();
    assert!(
        !fx.stop(&run_id).status.success(),
        "oversized launch record was accepted"
    );
    fs::write(&path, &original).unwrap();
    let backup = path.with_extension("backup");
    fs::rename(&path, &backup).unwrap();
    std::os::unix::fs::symlink(&backup, &path).unwrap();
    assert!(
        !fx.stop(&run_id).status.success(),
        "symlink launch record was accepted"
    );
    fs::remove_file(&path).unwrap();
    fs::rename(backup, &path).unwrap();
    assert!(foreign.0.try_wait().unwrap().is_none());
    assert!(fx.stop(&run_id).status.success());
    fs::write(&path, original).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(
        !fx.stop(&run_id).status.success(),
        "stale dead-owner record was accepted"
    );
}

#[test]
fn quick_recipe_completion_and_private_registry_permissions() {
    let fx = Fx::new();
    let out = fx
        .command()
        .args(["recipe", "run"])
        .arg(&fx.recipe)
        .arg("-w")
        .arg(&fx.project)
        .env("STUB_FAST", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let registry = fx.home.join(".amplihack/recipe-runs");
    assert_eq!(
        fs::metadata(&registry).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(fs::read_dir(registry).unwrap().count(), 0);
}

#[test]
fn autoreaping_sigchld_is_refused_before_runner_workload() {
    let fx = Fx::new();
    let mut command = fx.command();
    command
        .args(["recipe", "run"])
        .arg(&fx.recipe)
        .arg("-w")
        .arg(&fx.project);
    // Only this exec child receives the ignored disposition; the test runner
    // and every other process keep their normal signal and wait ownership.
    unsafe {
        command.pre_exec(|| {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = libc::SIG_IGN;
            if libc::sigaction(libc::SIGCHLD, &action, std::ptr::null_mut()) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let out = command.output().unwrap();
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("SIGCHLD"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !fx.tmp.path().join("pids.json").exists(),
        "unverifiable runner was launched"
    );
}
