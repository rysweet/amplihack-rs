//! Real launcher, Git-checking Codex-compatible stub. This is not live Codex evidence.
use super::fixture::{Fixture, succeeds};
use crate::support::{amplihack_bin::amplihack_bin, exec_stub::write_executable};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::Stdio;

const CODEX: &str = r#"#!/bin/bash
set -eu
case "${1:-}" in
  --version) echo 'codex-cli 0.166.0'; exit 0 ;;
  plugin) echo '{"installed":[]}'; exit 0 ;;
esac
printf '%s\0' "$@" > "$HOME/argv"
final= skip=0
while [ $# -gt 0 ]; do
  case "$1" in
    --cd|-C) cd -- "$2"; shift 2 ;;
    --output-last-message|-o) final="$2"; shift 2 ;;
    --skip-git-repo-check) skip=$((skip+1)); shift ;;
    --model|-m|--config|-c|--sandbox|-s|--profile|-p|--add-dir|--enable|--disable) shift 2 ;;
    *) shift ;;
  esac
done
cat > "$HOME/prompt"
pwd -P > "$HOME/effective-cwd"
printf '%s' "$skip" > "$HOME/skip-count"
git rev-parse --is-inside-work-tree > "$HOME/git-discovery" || { [ "$skip" = 1 ] || exit 42; }
git rev-parse HEAD > "$HOME/reviewed-head"
[ -z "$final" ] || printf 'CLEAN\n' > "$final"
printf 'CLEAN\n'
"#;

#[test]
fn codex_reviews_real_linked_worktree_without_automatic_git_check_bypass() {
    let f = Fixture::new();
    write_executable(&f.bin.join("codex"), CODEX, 0o700);
    write_executable(
        &f.bin.join("codex-code-mode-host"),
        "#!/bin/sh\nexit 0\n",
        0o700,
    );
    write_executable(&f.bin.join("node"), "#!/bin/sh\necho v22.14.0\n", 0o700);
    let config = b"# caller settings\nsandbox_mode = \"read-only\"\n";
    fs::write(f.home.join(".codex/config.toml"), config).unwrap();
    let index = f.git(&f.pr, &["ls-files", "--stage", "-z"]);
    let final_path = f.temp.path().join("verdict");
    for explicit_cd in [false, true] {
        let mut cmd = f.command(amplihack_bin());
        cmd.current_dir(if explicit_cd { f.temp.path() } else { &f.pr })
            .env("AMPLIHACK_AGENT_BINARY", "codex")
            .env("CODEX_BINARY_PATH", f.bin.join("codex"))
            .env("AMPLIHACK_CODEX_BINARY_PATH", f.bin.join("codex"))
            .args(["codex", "--", "exec"]);
        if explicit_cd {
            cmd.arg("--cd").arg(&f.pr);
        }
        let mut child = cmd
            .arg("--output-last-message")
            .arg(&final_path)
            .arg("-")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"Review the current commit. Leave the checkout unchanged.\n")
            .unwrap();
        succeeds(&child.wait_with_output().unwrap());
        assert_eq!(fs::read(f.home.join("skip-count")).unwrap(), b"0");
        let argv = fs::read(f.home.join("argv")).unwrap();
        assert!(
            !argv
                .split(|b| *b == 0)
                .any(|arg| arg == b"--skip-git-repo-check")
        );
        assert_eq!(
            Path::new(
                fs::read_to_string(f.home.join("effective-cwd"))
                    .unwrap()
                    .trim()
            ),
            f.pr
        );
        assert_eq!(fs::read(f.home.join("git-discovery")).unwrap(), b"true\n");
        assert_eq!(
            fs::read_to_string(f.home.join("reviewed-head"))
                .unwrap()
                .trim(),
            f.observed_head
        );
        assert_eq!(fs::read(&final_path).unwrap(), b"CLEAN\n");
    }
    assert_eq!(fs::read(f.home.join(".codex/config.toml")).unwrap(), config);
    assert_eq!(f.git(&f.pr, &["ls-files", "--stage", "-z"]), index);
    f.assert_no_private_repositories();
}
