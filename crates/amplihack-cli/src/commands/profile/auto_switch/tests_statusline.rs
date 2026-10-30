//! The wrapper command `install` writes (issue #1569), and how the wrapper
//! treats the no-owner command from `~/.claude` (issue #1745).

use super::super::state::State;
use super::{
    WrapperState, home_command, install_one, is_home_command, wrapper_command, wrapper_state,
};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// The arguments `sh` sees when it runs `cmd`, one per line.
fn sh_args(cmd: &str) -> Vec<String> {
    let out = std::process::Command::new("/bin/sh")
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .arg("-c")
        .arg(format!("printf '%s\\n' {}", cmd))
        .output()
        .unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn wrapper_command_quotes_hostile_paths() {
    let owner = ProfileName::parse("work").unwrap();
    for (exe, root) in [
        ("/opt/my tools/amplihack", "/home/u/profiles"),
        ("/tmp/it's/amplihack", "/home/u/a'b"),
        ("/tmp/$(touch pwned)/amplihack", "/tmp/`id`;rm -rf ~"),
        ("/tmp/*/amplihack", "/tmp/$HOME/\\x"),
    ] {
        let cmd = wrapper_command(Path::new(exe), Path::new(root), &owner).unwrap();
        assert_eq!(
            sh_args(&cmd),
            vec![
                exe,
                "profile",
                "statusline",
                "--root",
                root,
                "--owner",
                "work"
            ],
            "{cmd}"
        );
    }
}

#[test]
fn wrapper_command_refuses_unquotable_paths() {
    let owner = ProfileName::parse("work").unwrap();
    for exe in ["/tmp/a\nb/amplihack", "/tmp/a\rb/amplihack"] {
        assert!(wrapper_command(Path::new(exe), Path::new("/r"), &owner).is_err());
    }
}

// ---------------------------------------------------------------------------
// Issue #1745: the no-owner command in a profile's settings
// ---------------------------------------------------------------------------

#[test]
fn home_command_quotes_hostile_paths() {
    for exe in [
        "/opt/my tools/amplihack",
        "/tmp/it's/amplihack",
        "/tmp/$(touch pwned)/amplihack",
    ] {
        let cmd = home_command(Path::new(exe)).unwrap();
        assert_eq!(sh_args(&cmd), vec![exe, "profile", "statusline"], "{cmd}");
    }
}

#[test]
fn home_command_is_exact() {
    assert_eq!(
        home_command(Path::new("/usr/local/bin/amplihack")).unwrap(),
        "'/usr/local/bin/amplihack' profile statusline"
    );
    assert!(home_command(Path::new("/tmp/a\nb/amplihack")).is_err());
}

#[test]
fn is_home_command_means_profile_statusline_without_an_owner() {
    assert!(is_home_command(
        "'/usr/local/bin/amplihack' profile statusline"
    ));
    assert!(is_home_command(
        &home_command(Path::new("/x/amplihack")).unwrap()
    ));
    assert!(is_home_command(
        "'/x/amplihack' profile statusline --root '/r'"
    ));
    let owner = ProfileName::parse("work").unwrap();
    let wrapper = wrapper_command(Path::new("/x/amplihack"), Path::new("/r"), &owner).unwrap();
    assert!(!is_home_command(&wrapper));
    for other in [
        "",
        "bash ~/.amplihack/.claude/tools/statusline.sh",
        "$CLAUDE_PROJECT_DIR/.claude/tools/statusline.sh",
        "printf 'abc'",
    ] {
        assert!(!is_home_command(other), "{other}");
    }
}

struct Profiles {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    store: ProfileStore,
    state: State,
    name: ProfileName,
}

/// A private root with profile `work`, whose `settings.json` holds
/// `status_line` (if any), and the state directory set up as `install` does.
fn profiles(status_line: Option<serde_json::Value>) -> Profiles {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("profiles");
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let root = root.canonicalize().unwrap();
    let store = ProfileStore::new(root.clone());
    let name = ProfileName::parse("work").unwrap();
    store.add(&name).unwrap();
    if let Some(sl) = status_line {
        let p = root.join("work/settings.json");
        fs::write(
            &p,
            serde_json::json!({"statusLine": sl, "theme": "dark"}).to_string(),
        )
        .unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let state = State::new(&root).unwrap();
    state.ensure().unwrap();
    Profiles {
        _tmp: tmp,
        root,
        store,
        state,
        name,
    }
}

impl Profiles {
    fn settings(&self) -> serde_json::Value {
        serde_json::from_slice(&fs::read(self.root.join("work/settings.json")).unwrap()).unwrap()
    }
}

const EXE: &str = "/usr/local/bin/amplihack";

#[test]
fn install_wraps_the_home_command_as_an_original() {
    let home = serde_json::json!({
        "type": "command",
        "command": "'/usr/local/bin/amplihack' profile statusline",
        "padding": 1,
    });
    for repoint in [false, true] {
        let p = profiles(Some(home.clone()));
        let (outcome, detail) =
            install_one(&p.store, &p.state, Path::new(EXE), &p.name, repoint).unwrap();
        assert_eq!(
            (outcome, detail.as_str()),
            ("installed", "(wrapped existing status line)"),
            "repoint {repoint}: never stale, never repaired"
        );
        assert_eq!(
            p.state.read_sub_json("statusline", "work").unwrap(),
            Some(home.clone()),
            "the home command is saved as the original"
        );
        let settings = p.settings();
        let want = wrapper_command(Path::new(EXE), &p.root, &p.name).unwrap();
        assert_eq!(settings["statusLine"]["command"], want.as_str());
        assert_eq!(settings["statusLine"]["padding"], 1);
        assert_eq!(settings["theme"], "dark");
    }
}

#[test]
fn install_still_reports_an_owner_wrapper_for_another_binary_as_stale() {
    let owner = ProfileName::parse("work").unwrap();
    let other = wrapper_command(Path::new("/old/amplihack"), Path::new("/r"), &owner).unwrap();
    let p = profiles(Some(
        serde_json::json!({"type": "command", "command": other}),
    ));
    let (outcome, _) = install_one(&p.store, &p.state, Path::new(EXE), &p.name, false).unwrap();
    assert_eq!(outcome, "stale");
}

#[test]
fn wrapper_state_of_the_home_command_is_missing() {
    let p = profiles(Some(serde_json::json!({
        "type": "command",
        "command": "'/usr/local/bin/amplihack' profile statusline",
    })));
    assert_eq!(
        wrapper_state(&p.store, &p.root, &p.name),
        WrapperState::Missing
    );
}
