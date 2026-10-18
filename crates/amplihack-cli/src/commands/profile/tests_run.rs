#![cfg(unix)]
use super::name::ProfileName;
use super::run::build_profile_command;
use super::store::ProfileStore;
use crate::binary_finder::BinaryInfo;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::PermissionsExt;

fn fake_claude(dir: &std::path::Path) -> BinaryInfo {
    let p = dir.join("claude");
    fs::write(
        &p,
        "#!/bin/sh\necho \"CFG=$CLAUDE_CONFIG_DIR\"\nfor a in \"$@\"; do echo \"ARG=$a\"; done\n",
    )
    .unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    BinaryInfo {
        name: "claude".into(),
        path: p,
        version: None,
    }
}

fn setup() -> (tempfile::TempDir, ProfileStore, ProfileName, BinaryInfo) {
    let d = tempfile::tempdir().unwrap();
    let s = ProfileStore::new(d.path().join("root"));
    let name = ProfileName::parse("work").unwrap();
    s.add(&name).unwrap();
    let bin = fake_claude(d.path());
    (d, s, name, bin)
}

#[test]
fn run_sets_config_dir_to_profile_dir_for_child() {
    let (_d, s, name, bin) = setup();
    let cmd = build_profile_command(&bin, &s, &name, &[]).unwrap();
    let want = s.resolve(&name).unwrap();
    let got = cmd
        .get_envs()
        .find(|(k, _)| *k == OsStr::new("CLAUDE_CONFIG_DIR"))
        .and_then(|(_, v)| v)
        .expect("CLAUDE_CONFIG_DIR set on child");
    assert_eq!(std::path::Path::new(got), want);
}

#[test]
fn run_forwards_args_literally_without_shell() {
    let (_d, s, name, bin) = setup();
    let args = vec![
        "--flag".to_string(),
        "; rm -rf ~".to_string(),
        "$(id)".to_string(),
    ];
    let mut cmd = build_profile_command(&bin, &s, &name, &args).unwrap();
    let out = cmd.output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let want = s.resolve(&name).unwrap();
    assert!(text.contains(&format!("CFG={}", want.display())), "{text}");
    for a in &args {
        assert!(text.contains(&format!("ARG={a}")), "{text}");
    }
}

#[test]
fn run_missing_profile_fails_before_spawn() {
    let (_d, s, _n, bin) = setup();
    let missing = ProfileName::parse("nope").unwrap();
    assert!(build_profile_command(&bin, &s, &missing, &[]).is_err());
}

#[cfg(unix)]
#[test]
fn run_refuses_profile_swapped_for_symlink() {
    let (d, s, name, bin) = setup();
    let outside = d.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::remove_dir(s.root().join("work")).unwrap();
    std::os::unix::fs::symlink(&outside, s.root().join("work")).unwrap();
    assert!(build_profile_command(&bin, &s, &name, &[]).is_err());
}

#[test]
fn run_refuses_group_writable_profile_before_spawn() {
    let (_d, s, name, bin) = setup();
    fs::set_permissions(s.root().join("work"), fs::Permissions::from_mode(0o770)).unwrap();
    assert!(build_profile_command(&bin, &s, &name, &[]).is_err());
}

struct DockerEnvGuard(Option<std::ffi::OsString>);
impl DockerEnvGuard {
    fn set(v: &str) -> Self {
        let prev = std::env::var_os("AMPLIHACK_USE_DOCKER");
        unsafe { std::env::set_var("AMPLIHACK_USE_DOCKER", v) };
        Self(prev)
    }
}
impl Drop for DockerEnvGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.0 {
                Some(v) => std::env::set_var("AMPLIHACK_USE_DOCKER", v),
                None => std::env::remove_var("AMPLIHACK_USE_DOCKER"),
            }
        }
    }
}

#[test]
fn reject_docker_env_errors_when_env_active() {
    let _l = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _g = DockerEnvGuard::set("1");
    let err = super::reject_docker_env()
        .unwrap_err()
        .to_string()
        .to_lowercase();
    assert!(err.contains("docker"), "{err}");
}

#[test]
fn profile_run_fails_closed_under_docker_env_before_touching_disk() {
    let _l = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _g = DockerEnvGuard::set("1");
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("root");
    let s = ProfileStore::new(root.clone());
    let missing = ProfileName::parse("ghost").unwrap();
    let err = super::run::run(&s, &missing, vec![])
        .unwrap_err()
        .to_string()
        .to_lowercase();
    assert!(
        err.contains("docker"),
        "docker check must precede resolution: {err}"
    );
    assert!(!root.exists());
}

// ---------------------------------------------------------------------------
// Issue #1567 (D2): `profile login` (build_profile_command) pins the profile
// for any amplihack launch started inside it, like launch/claude and
// `profile run` do.
// ---------------------------------------------------------------------------

const PROFILE_VAR: &str = "AMPLIHACK_CLAUDE_PROFILE";
const ROOT_VAR: &str = "AMPLIHACK_CLAUDE_PROFILE_ROOT";

fn child_env(cmd: &std::process::Command, key: &str) -> Option<Option<std::ffi::OsString>> {
    cmd.get_envs()
        .find(|(k, _)| *k == OsStr::new(key))
        .map(|(_, v)| v.map(|s| s.to_os_string()))
}

/// Runs `f` with `ROOT_VAR` set to `value` (or removed), restoring it after.
fn with_root_env<T>(value: Option<&std::path::Path>, f: impl FnOnce() -> T) -> T {
    crate::test_support::with_env(&[(ROOT_VAR, value.map(|v| v.as_os_str()))], f)
}

#[test]
fn profile_command_pins_the_profile_name_for_descendants() {
    let (_d, s, name, bin) = setup();
    let cmd = with_root_env(None, || {
        build_profile_command(&bin, &s, &name, &[]).unwrap()
    });
    assert_eq!(child_env(&cmd, PROFILE_VAR), Some(Some("work".into())));
    let want = s.resolve(&name).unwrap();
    assert_eq!(
        child_env(&cmd, "CLAUDE_CONFIG_DIR"),
        Some(Some(want.into_os_string()))
    );
}

#[test]
fn profile_command_passes_root_only_when_parent_had_it() {
    let (_d, s, name, bin) = setup();
    let without = with_root_env(None, || {
        build_profile_command(&bin, &s, &name, &[]).unwrap()
    });
    assert_eq!(child_env(&without, ROOT_VAR), None);

    let with = with_root_env(Some(s.root()), || {
        build_profile_command(&bin, &s, &name, &[]).unwrap()
    });
    let want_root = s.resolve(&name).unwrap().parent().unwrap().to_path_buf();
    assert_eq!(
        child_env(&with, ROOT_VAR),
        Some(Some(want_root.into_os_string()))
    );
    assert_eq!(child_env(&with, PROFILE_VAR), Some(Some("work".into())));
}

#[test]
fn profile_command_pin_is_child_only() {
    let (_d, s, name, bin) = setup();
    let _g = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let before = std::env::var_os(PROFILE_VAR);
    let _cmd = build_profile_command(&bin, &s, &name, &[]).unwrap();
    assert_eq!(std::env::var_os(PROFILE_VAR), before);
}

#[test]
fn profile_command_child_sees_the_pinned_name() {
    let (d, s, name, _) = setup();
    let p = d.path().join("claude-env");
    fs::write(&p, "#!/bin/sh\necho \"PIN=$AMPLIHACK_CLAUDE_PROFILE\"\n").unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    let bin = BinaryInfo {
        name: "claude".into(),
        path: p,
        version: None,
    };
    let mut cmd = with_root_env(None, || {
        build_profile_command(&bin, &s, &name, &[]).unwrap()
    });
    let out = cmd.env_remove("NOT_A_VAR").output().unwrap();
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("PIN=work\n"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn profile_command_for_missing_profile_sets_no_pin() {
    let (_d, s, _n, bin) = setup();
    let missing = ProfileName::parse("nope").unwrap();
    // Fails before a Command exists, so nothing can be exported.
    assert!(build_profile_command(&bin, &s, &missing, &[]).is_err());
}

#[test]
fn default_dir_command_removes_config_dir_and_keeps_the_pin() {
    use super::run::build_default_dir_command;
    let (_d, s, name, bin) = setup();
    let cmd = build_default_dir_command(&bin, &s, &name, &[]).unwrap();
    let envs: Vec<_> = cmd.get_envs().collect();
    assert!(envs.contains(&(OsStr::new("CLAUDE_CONFIG_DIR"), None)));
    assert!(envs.contains(&(OsStr::new(super::PROFILE_ENV), Some(OsStr::new("work")))));
}

// Issue #1592: probing the login in `~/.claude` when it belongs to no profile.

#[test]
fn home_command_removes_config_dir_and_pins_nothing() {
    use super::run::build_home_command;
    let (_d, _s, _name, bin) = setup();
    let cmd = build_home_command(&bin, &["-p".to_string(), "/usage".to_string()]);
    assert_eq!(child_env(&cmd, "CLAUDE_CONFIG_DIR"), Some(None), "removed");
    assert!(
        !matches!(child_env(&cmd, PROFILE_VAR), Some(Some(_))),
        "the pin is never set"
    );
    let args: Vec<_> = cmd.get_args().collect();
    assert_eq!(args, [OsStr::new("-p"), OsStr::new("/usage")]);
    assert_eq!(cmd.get_program(), bin.path.as_os_str());
}

#[test]
fn home_command_child_sees_no_config_dir() {
    use super::run::build_home_command;
    let (d, _s, _n, _) = setup();
    let p = d.path().join("claude-env");
    fs::write(&p, "#!/bin/sh\necho \"DIR=${CLAUDE_CONFIG_DIR-<unset>}\"\n").unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    let bin = BinaryInfo {
        name: "claude".into(),
        path: p,
        version: None,
    };
    let mut cmd = build_home_command(&bin, &[]);
    cmd.env("CLAUDE_CONFIG_DIR_UNRELATED", "x");
    let out = cmd.output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "DIR=<unset>\n");
}
