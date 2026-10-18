//! The wrapper command `install` writes (issue #1569).

use super::wrapper_command;
use crate::commands::profile::name::ProfileName;
use std::path::Path;

/// The arguments `sh` sees when it runs `cmd`, one per line.
fn sh_args(cmd: &str) -> Vec<String> {
    let out = std::process::Command::new("sh")
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
