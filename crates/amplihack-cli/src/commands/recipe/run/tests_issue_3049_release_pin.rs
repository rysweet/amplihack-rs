//! Issue #3049: a run sources the stage-changes helper from the release it
//! started on. The runner sets `WORKFLOW_STAGE_CHANGES_HELPER` once per run, so
//! swapping the `amplifier-bundle` link mid-run does not change it, and a live
//! process that carries the value keeps that release out of clean-up.

use super::*;
use crate::commands::install::release_layout::{InUse, scan_pids};
use serde_json::Value as JsonValue;
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

const HELPER: &str = "tools/workflow_stage_changes.sh";
const PIN_VAR: &str = "WORKFLOW_STAGE_CHANGES_HELPER";

fn set_mode(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

fn write_release(home: &Path, name: &str) -> PathBuf {
    let release = home.join("releases").join(name);
    let tools = release.join("amplifier-bundle/tools");
    fs::create_dir_all(&tools).unwrap();
    fs::write(
        tools.join("workflow_stage_changes.sh"),
        format!("# stand-in release {name}\n"),
    )
    .unwrap();
    fs::write(release.join(".complete"), "").unwrap();
    set_mode(&release, 0o755);
    release
}

fn point_link(home: &Path, release: &Path) {
    let link = home.join("amplifier-bundle");
    let _ = fs::remove_file(&link);
    symlink(release.join("amplifier-bundle"), link).unwrap();
}

struct Fixture {
    // Field order is drop order: variables are restored before the folder goes.
    _env: crate::test_support::EnvGuard,
    _work_root: crate::test_support::IsolatedWorkRoot,
    _tmp: tempfile::TempDir,
    home: PathBuf,
    workdir: PathBuf,
    n: PathBuf,
    n1: PathBuf,
    runner: PathBuf,
}

/// Home with releases `1.0.0` (N) and `1.0.1` (N+1), the link on N, and a stub
/// runner that prints the pin it was given. With `swap`, the first real
/// invocation moves the link to N+1 before printing, as an upgrade would.
fn fixture(swap: bool) -> Fixture {
    let work_root = crate::test_support::IsolatedWorkRoot::new();
    let mut env = crate::test_support::EnvGuard::capture(&[]);
    let tmp = tempfile::tempdir_in(
        std::env::var_os("TMPDIR").map_or_else(std::env::temp_dir, PathBuf::from),
    )
    .unwrap();
    let home = tmp.path().join(".amplihack");
    fs::create_dir_all(home.join("releases")).unwrap();
    let n = write_release(&home, "1.0.0");
    let n1 = write_release(&home, "1.0.1");
    set_mode(&home.join("releases"), 0o700);
    set_mode(&home, 0o700);
    point_link(&home, &n);
    let workdir = tmp.path().join("work");
    fs::create_dir_all(&workdir).unwrap();

    let swap_cmd = if swap {
        format!(
            "ln -sfn {} {}\n",
            crate::test_support::sh_quote(&n1.join("amplifier-bundle").to_string_lossy()),
            crate::test_support::sh_quote(&home.join("amplifier-bundle").to_string_lossy()),
        )
    } else {
        String::new()
    };
    let runner = tmp.path().join("recipe-runner-rs");
    crate::test_support::write_executable(
        &runner,
        &format!(
            "#!/bin/sh\ncase \"$1\" in --capabilities) printf '%b\\n' '\\173\"schema_version\":1,\"version\":\"fixture\",\"capabilities\":[\"codex_exec\",\"durable_recovery_v1\"]\\175'; exit 0 ;; esac\n{swap_cmd}cat <<EOF\n{{\"recipe_name\":\"pin-probe\",\"success\":true,\"step_results\":[],\"context\":{{\"pin\":\"${PIN_VAR}\"}}}}\nEOF\n"
        ),
    );
    let resolver = tmp.path().join("amplihack-asset-resolver");
    crate::test_support::write_executable(
        &resolver,
        "#!/bin/sh\ncase \"$1\" in --capabilities) printf '%b\\n' '\\173\"schema_version\":1,\"version\":\"fixture\",\"capabilities\":[\"codex_exec\",\"durable_recovery_v1\"]\\175'; exit 0 ;; esac\nexit 0\n",
    );
    fs::write(
        tmp.path().join("pin-probe.yaml"),
        "name: pin-probe\nsteps: []\n",
    )
    .unwrap();

    env.assign("AMPLIHACK_AGENT_BINARY", "claude");
    for key in crate::test_support::OUTER_RUN_IDENTITY_VARS {
        env.clear(key);
    }
    env.assign("HOME", tmp.path());
    env.assign("RECIPE_RUNNER_RS_PATH", &runner);
    env.assign("AMPLIHACK_HOME", &home);
    env.clear(PIN_VAR);
    let path = format!(
        "{}:{}",
        tmp.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    env.assign("PATH", path);
    Fixture {
        _env: env,
        _work_root: work_root,
        _tmp: tmp,
        home,
        workdir,
        n,
        n1,
        runner,
    }
}

/// One run; returns the pin the stub runner saw (empty when unset).
fn run_once(f: &Fixture) -> String {
    let _ = &f.runner;
    let recipe = f.home.parent().unwrap().join("pin-probe.yaml");
    let result = execute_recipe_via_rust_for_test(
        &recipe,
        &BTreeMap::new(),
        true,
        false,
        &f.workdir,
        &[],
        None,
    )
    .expect("stub run must succeed");
    match result.context.get("pin") {
        Some(JsonValue::String(s)) => s.clone(),
        other => panic!("stub did not report a pin: {other:?}"),
    }
}

fn helper_of(release: &Path) -> String {
    release
        .join("amplifier-bundle")
        .join(HELPER)
        .to_string_lossy()
        .into_owned()
}

#[test]
fn pin_names_the_start_release_after_a_swap() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let f = fixture(true);
    let pin = run_once(&f);
    assert_eq!(
        pin,
        helper_of(&f.n),
        "the runner must pin the release the run started on"
    );
    // The link now names N+1, yet the pinned file is still there and is N's.
    assert!(fs::read_to_string(&pin).unwrap().contains("release 1.0.0"));
    assert!(
        fs::canonicalize(f.home.join("amplifier-bundle"))
            .unwrap()
            .starts_with(fs::canonicalize(&f.n1).unwrap())
    );
}

#[test]
fn run_started_after_the_swap_gets_the_new_release() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let f = fixture(false);
    point_link(&f.home, &f.n1);
    assert_eq!(run_once(&f), helper_of(&f.n1));
}

#[test]
fn development_checkout_gets_no_pin() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let f = fixture(false);
    // A real bundle directory, not a link into releases/.
    fs::remove_file(f.home.join("amplifier-bundle")).unwrap();
    let bundle = f.home.join("amplifier-bundle");
    fs::create_dir_all(bundle.join("tools")).unwrap();
    fs::write(bundle.join(HELPER), "# checkout\n").unwrap();
    assert_eq!(run_once(&f), "");
}

#[test]
fn inherited_pin_is_kept() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let mut f = fixture(false);
    let inherited = helper_of(&f.n1);
    f._env.assign(PIN_VAR, &inherited);
    assert_eq!(
        run_once(&f),
        inherited,
        "a nested run stays on its parent's release"
    );
}

#[test]
fn missing_helper_gives_no_pin() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let f = fixture(false);
    fs::remove_file(f.n.join("amplifier-bundle").join(HELPER)).unwrap();
    assert_eq!(run_once(&f), "");
}

#[cfg(target_os = "linux")]
#[test]
fn live_pinned_process_keeps_its_release() {
    use std::process::{Command, Stdio};
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let f = fixture(false);
    let pin = run_once(&f);
    assert_eq!(pin, helper_of(&f.n));

    let mut pinned = Command::new("cat")
        .env_clear()
        .env(PIN_VAR, &pin)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let mut control = Command::new("cat")
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let candidates = vec![f.n.clone(), f.n1.clone()];
    let seen = scan_pids(&candidates, &[pinned.id(), control.id()]);

    drop(pinned.stdin.take());
    drop(control.stdin.take());
    let _ = pinned.wait();
    let _ = control.wait();

    match seen {
        InUse::Known(map) => {
            let users = map.get(&f.n).expect("release N must be reported as in use");
            assert!(users.contains(&pinned.id()));
            assert!(!users.contains(&control.id()), "the control holds no pin");
            assert!(!map.contains_key(&f.n1), "N+1 is not in use");
        }
        other => panic!("scan must complete: {other:?}"),
    }
}
