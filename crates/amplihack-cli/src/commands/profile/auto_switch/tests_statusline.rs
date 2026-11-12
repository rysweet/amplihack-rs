//! The wrapper command `install` writes (issue #1569), and how the wrapper
//! treats the no-owner command from `~/.claude` (issue #1745).

use super::super::state::State;
use super::{WrapperState, home_command, install_one, is_wrapper, wrapper_command, wrapper_state};
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
fn only_a_profile_statusline_command_with_an_owner_is_a_wrapper() {
    let owner = ProfileName::parse("work").unwrap();
    let wrapper = wrapper_command(Path::new("/x/amplihack"), Path::new("/r"), &owner).unwrap();
    assert!(is_wrapper(&wrapper));
    assert!(is_wrapper(
        "'/x/amplihack' profile statusline --owner 'work'"
    ));
    for other in [
        "'/usr/local/bin/amplihack' profile statusline",
        "'/x/amplihack' profile statusline --root '/r'",
        "",
        "bash ~/.amplihack/.claude/tools/statusline.sh",
        "printf '--owner'",
        "my-profile statusline-helper.sh --owner me",
    ] {
        assert!(!is_wrapper(other), "{other}");
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
const SCRIPT: &str = "'/home/u/.amplihack/.claude/tools/statusline.sh'";

#[test]
fn install_saves_the_script_in_place_of_the_home_command() {
    let home = serde_json::json!({
        "type": "command",
        "command": "'/usr/local/bin/amplihack' profile statusline",
        "padding": 1,
    });
    for repoint in [false, true] {
        let p = profiles(Some(home.clone()));
        let (outcome, detail) = install_one(
            &p.store,
            &p.state,
            Path::new(EXE),
            &p.name,
            repoint,
            Some(SCRIPT),
        )
        .unwrap();
        assert_eq!(
            (outcome, detail.as_str()),
            (
                "installed",
                "(wrapped statusline.sh in place of the ~/.claude command)"
            ),
            "repoint {repoint}: never stale, never repaired"
        );
        assert_eq!(
            p.state.read_sub_json("statusline", "work").unwrap(),
            Some(serde_json::json!({"type": "command", "command": SCRIPT})),
            "the script is saved, not the home command"
        );
        let settings = p.settings();
        let want = wrapper_command(Path::new(EXE), &p.root, &p.name).unwrap();
        assert_eq!(settings["statusLine"]["command"], want.as_str());
        assert_eq!(settings["statusLine"]["padding"], 1);
        assert_eq!(settings["theme"], "dark");
    }
}

#[test]
fn without_a_usable_script_install_saves_no_original() {
    let p = profiles(Some(serde_json::json!({
        "type": "command",
        "command": "'/usr/local/bin/amplihack' profile statusline",
    })));
    let (outcome, detail) =
        install_one(&p.store, &p.state, Path::new(EXE), &p.name, false, None).unwrap();
    assert_eq!(
        (outcome, detail.as_str()),
        ("installed", "(no previous status line)")
    );
    assert_eq!(p.state.read_sub_json("statusline", "work").unwrap(), None);
}

#[test]
fn the_home_command_and_a_wrapper_are_never_saved_as_an_original() {
    for cmd in [
        "'/usr/local/bin/amplihack' profile statusline",
        "amplihack profile statusline",
        "'/x/amplihack' profile statusline --owner 'work'",
    ] {
        for (script, repoint) in [(None, false), (Some(SCRIPT), false), (Some(SCRIPT), true)] {
            let p = profiles(Some(serde_json::json!({"type": "command", "command": cmd})));
            let _ = install_one(&p.store, &p.state, Path::new(EXE), &p.name, repoint, script);
            let saved = p.state.read_sub_json("statusline", "work").unwrap();
            assert!(
                !saved.is_some_and(|v| v.to_string().contains("profile statusline")),
                "{cmd}"
            );
        }
    }
}

/// Home install keeps these as the user's (`settings_tests.rs`), so shared
/// keys can copy them into a profile; `install` must not lose them there.
#[test]
fn install_saves_a_users_command_that_mentions_profile_statusline() {
    for mine in [
        "my-profile statusline-helper.sh",
        "my-profile statusline-helper.sh --owner me",
        "amplihack profile statusline | my-filter",
        "'/usr/local/bin/amplihack' profile statusline | my-filter",
        "'/x/amplihack' profile statusline --root '/r'",
    ] {
        for repoint in [false, true] {
            let entry = serde_json::json!({"type": "command", "command": mine, "padding": 2});
            let p = profiles(Some(entry.clone()));
            let (outcome, detail) = install_one(
                &p.store,
                &p.state,
                Path::new(EXE),
                &p.name,
                repoint,
                Some(SCRIPT),
            )
            .unwrap();
            assert_eq!(
                (outcome, detail.as_str()),
                ("installed", "(wrapped existing status line)"),
                "{mine}, repoint {repoint}"
            );
            assert_eq!(
                p.state.read_sub_json("statusline", "work").unwrap(),
                Some(entry),
                "{mine}: the user's command is saved whole"
            );
            let want = wrapper_command(Path::new(EXE), &p.root, &p.name).unwrap();
            assert_eq!(p.settings()["statusLine"]["command"], want.as_str());
            assert_eq!(p.settings()["statusLine"]["padding"], 2);
        }
    }
}

#[test]
fn an_original_with_an_owner_is_treated_as_a_wrapper() {
    let p = profiles(Some(serde_json::json!({
        "type": "command",
        "command": "'/x/amplihack' profile statusline --owner 'work'",
    })));
    let (outcome, _) = install_one(
        &p.store,
        &p.state,
        Path::new(EXE),
        &p.name,
        false,
        Some(SCRIPT),
    )
    .unwrap();
    assert_eq!(outcome, "stale");
    assert_eq!(p.state.read_sub_json("statusline", "work").unwrap(), None);
}

#[test]
fn install_still_reports_an_owner_wrapper_for_another_binary_as_stale() {
    let owner = ProfileName::parse("work").unwrap();
    let other = wrapper_command(Path::new("/old/amplihack"), Path::new("/r"), &owner).unwrap();
    let p = profiles(Some(
        serde_json::json!({"type": "command", "command": other}),
    ));
    let (outcome, _) =
        install_one(&p.store, &p.state, Path::new(EXE), &p.name, false, None).unwrap();
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
