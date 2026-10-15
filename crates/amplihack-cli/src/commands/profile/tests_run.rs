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
