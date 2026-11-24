//! Issue #1985: `amplihack codex -- exec …` (the recipe agent-step path) runs
//! Codex with `--sandbox danger-full-access` (#2317: the same trust a Claude
//! step has) and `--dangerously-bypass-hook-trust`, unless the caller chose
//! otherwise.
//! Interactive launches are unchanged, and the approvals bypass is never
//! added.
//!
//! The Codex here is a fake `sh` script in a temporary home. It records the
//! argv of the launch, writes [`FAKE_FINAL_MESSAGE`] to the path after
//! `--output-last-message` (as a real Codex writes its final message there),
//! and discards stdin. The real
//! `~/.codex`, `~/.claude`, `auth.json` and `config.toml` are never read:
//! `HOME`, `CODEX_HOME` and `AMPLIHACK_HOME` all point into the temp dir.
#![cfg(unix)]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const SANDBOX: [&str; 2] = ["--sandbox", "danger-full-access"];
/// The network override added before #2317; it must never appear now.
const OLD_NETWORK: &str = "sandbox_workspace_write.network_access=true";
const HOOK_TRUST: &str = "--dangerously-bypass-hook-trust";
const APPROVALS_BYPASS: &str = "--dangerously-bypass-approvals-and-sandbox";

/// The host-style config a host Codex home carries. The launch must leave
/// it byte for byte as it was (S5).
const HOST_CONFIG: &str = "sandbox_mode = \"danger-full-access\"\n";

/// What [`fake_codex`] writes to the `--output-last-message` path. Keep the two
/// in step: the #2010 test asserts the file holds exactly this text.
pub(crate) const FAKE_FINAL_MESSAGE: &str = "fake codex final message\n";

/// Answers the launcher's probes; records the argv of anything else, and
/// writes [`FAKE_FINAL_MESSAGE`] to the `--output-last-message` path. The
/// shell in `step` then runs in the launch's working directory, as an agent
/// step's own work would (#2317). Empty `step` runs nothing.
fn fake_codex(step: &str) -> String {
    FAKE_CODEX_HEAD.to_string() + step + FAKE_CODEX_TAIL
}

const FAKE_CODEX_HEAD: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$HOME/calls.log"
case "$1" in
  --version) echo 'codex-cli 0.166.0'; exit 0;;
  plugin)
    case "$2" in
      list) echo '{"installed":[]}';;
      *) echo '{"ok":true}';;
    esac
    exit 0;;
esac
: > "$HOME/argv.txt"
prev=
for arg in "$@"; do
  printf '%s\n' "$arg" >> "$HOME/argv.txt"
  if [ "$prev" = "--output-last-message" ]; then
    printf 'fake codex final message\n' > "$arg"
  fi
  prev=$arg
done
"#;

const FAKE_CODEX_TAIL: &str = r#"
cat > /dev/null
exit 0
"#;

pub(crate) struct Launch {
    pub(crate) argv: Vec<String>,
    pub(crate) out: Output,
    pub(crate) home: tempfile::TempDir,
}

fn tools_dir(home: &Path, step: &str) -> PathBuf {
    let tools = home.join("tools");
    fs::create_dir(&tools).unwrap();
    // No host PATH: no accidental real Codex on the search path.
    for tool in [
        "sh", "cat", "mkdir", "rm", "mv", "cp", "chmod", "git", "uname",
    ] {
        let found = Command::new("/bin/sh")
            .args(["-c", &format!("command -v {tool}")])
            .output()
            .unwrap();
        if found.status.success() {
            std::os::unix::fs::symlink(
                String::from_utf8(found.stdout).unwrap().trim(),
                tools.join(tool),
            )
            .unwrap();
        }
    }
    crate::support::exec_stub::write_executable(&tools.join("codex"), fake_codex(step), 0o700);
    crate::support::exec_stub::write_executable(
        &tools.join("codex-code-mode-host"),
        "#!/bin/sh\nexit 0\n",
        0o700,
    );
    crate::support::exec_stub::write_executable(
        &tools.join("node"),
        "#!/bin/sh\necho v22.14.0\n",
        0o700,
    );
    tools
}

/// Runs `amplihack codex [-- tail…]` against the fake. No amplihack option
/// comes before `--`: the recipe runner (`amplihack-recipe-runner`,
/// `src/adapters/cli_subprocess.rs`) sends exactly
/// `amplihack codex -- exec --output-last-message <file> [--model m] -`,
/// with no `--no-reflection`, so this is the command under test.
pub(crate) fn launch(tail: &[&str], stdin: &[u8]) -> Launch {
    launch_in(tail, stdin, None, "")
}

/// [`launch`] with the working directory set to `cwd` (the temp home when
/// `None`) and `step` shell run by the fake Codex after it records its argv.
pub(crate) fn launch_in(tail: &[&str], stdin: &[u8], cwd: Option<&Path>, step: &str) -> Launch {
    let home = tempfile::tempdir().unwrap();
    let tools = tools_dir(home.path(), step);
    let codex_home = home.path().join(".codex");
    fs::create_dir_all(&codex_home).unwrap();
    fs::write(codex_home.join("config.toml"), HOST_CONFIG).unwrap();
    let mut args = vec!["codex"];
    if !tail.is_empty() {
        args.push("--");
        args.extend_from_slice(tail);
    }
    let mut child =
        crate::isolated_amplihack::isolated(crate::support::amplihack_bin::amplihack_bin())
            .args(&args)
            .env("HOME", home.path())
            .env("CODEX_HOME", &codex_home)
            .env("AMPLIHACK_HOME", home.path().join(".amplihack"))
            .env("PATH", &tools)
            .env("CODEX_BINARY_PATH", tools.join("codex"))
            .env("AMPLIHACK_AGENT_BINARY", "codex")
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_NO_UPDATE_CHECK", "1")
            // No startup self-heal install: the launch path is under test, not install.
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_DISABLE_BLARIFY", "1")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("AMPLIHACK_CODEX_BINARY_PATH")
            .env_remove("AMPLIHACK_DEFAULT_MODEL")
            .env_remove("AMPLIHACK_SESSION_ID")
            .env_remove("AMPLIHACK_SESSION_DEPTH")
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("OPENAI_API_KEY")
            .env_remove("CODEX_API_KEY")
            .env_remove("ANTHROPIC_API_KEY")
            .env_remove("GH_TOKEN")
            .env_remove("GITHUB_TOKEN")
            .env_remove("HTTP_PROXY")
            .env_remove("HTTPS_PROXY")
            .env_remove("http_proxy")
            .env_remove("https_proxy")
            .env_remove("ALL_PROXY")
            .current_dir(cwd.unwrap_or(home.path()))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
    {
        let mut input = child.stdin.take().unwrap();
        // The child may not read stdin (interactive case); ignore EPIPE.
        let _ = input.write_all(stdin);
    }
    let out = child.wait_with_output().unwrap();
    let recorded = home.path().join("argv.txt");
    assert!(
        recorded.is_file(),
        "the fake codex was never launched for {tail:?}\nstatus: {}\nstdout: {}\nstderr: {}\ncalls: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
        fs::read_to_string(home.path().join("calls.log")).unwrap_or_default(),
    );
    let argv = fs::read_to_string(&recorded)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(
        fs::read_to_string(codex_home.join("config.toml")).unwrap(),
        HOST_CONFIG,
        "CODEX_HOME/config.toml must be left unchanged"
    );
    Launch { argv, out, home }
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

fn count(argv: &[String], token: &str) -> usize {
    argv.iter().filter(|a| *a == token).count()
}

fn assert_no_approvals_bypass(launch: &Launch) {
    assert!(
        !launch
            .argv
            .iter()
            .any(|a| a == APPROVALS_BYPASS || a == "--yolo"),
        "approvals bypass added: {:?}",
        launch.argv
    );
}

/// AC1: an agent step gets exactly the two flags, right after `exec`, and
/// no workspace-write token (#2317).
#[test]
fn agent_step_gets_full_access_and_hook_trust() {
    let tmp = tempfile::tempdir().unwrap();
    let final_message = tmp.path().join("final-message");
    let final_message = final_message.to_str().unwrap();
    let launch = launch(
        &["exec", "--output-last-message", final_message, "-"],
        b"do the step\n",
    );
    assert!(launch.out.status.success(), "{:?}", launch.out);
    let expected = strings(&[
        "exec",
        SANDBOX[0],
        SANDBOX[1],
        HOOK_TRUST,
        "--output-last-message",
        final_message,
        "-",
    ]);
    assert_eq!(launch.argv, expected);
    assert!(
        !launch.argv.iter().any(|a| a.contains("workspace-write")),
        "{:?}",
        launch.argv
    );
    assert_no_approvals_bypass(&launch);
    drop(launch.home);
}

/// AC2: the caller's explicit sandbox is respected; no second `--sandbox`
/// and no full access over it, but hook trust is still added.
#[test]
fn caller_sandbox_is_respected() {
    for choice in [["--sandbox", "read-only"], ["-s", "danger-full-access"]] {
        let launch = launch(&["exec", choice[0], choice[1], "-"], b"prompt\n");
        assert!(launch.out.status.success(), "{choice:?}: {:?}", launch.out);
        assert_eq!(
            launch.argv,
            strings(&["exec", HOOK_TRUST, choice[0], choice[1], "-"]),
            "caller choice {choice:?}"
        );
        assert_eq!(
            count(&launch.argv, "--sandbox"),
            usize::from(choice[0] == "--sandbox"),
            "{choice:?}"
        );
        assert_eq!(count(&launch.argv, "workspace-write"), 0, "{choice:?}");
        assert_eq!(count(&launch.argv, OLD_NETWORK), 0, "{choice:?}");
        assert_no_approvals_bypass(&launch);
    }
}

/// `--approve-for-me` chooses the workspace-write sandbox, and codex-cli
/// 0.166.0 refuses it alongside `--sandbox`, so no sandbox may be added.
#[test]
fn approve_for_me_is_a_sandbox_choice() {
    let launch = launch(&["exec", "--approve-for-me", "-"], b"prompt\n");
    assert!(launch.out.status.success(), "{:?}", launch.out);
    assert_eq!(
        launch.argv,
        strings(&["exec", HOOK_TRUST, "--approve-for-me", "-"])
    );
    assert_eq!(count(&launch.argv, SANDBOX[0]), 0);
    assert_eq!(count(&launch.argv, SANDBOX[1]), 0);
    assert_eq!(count(&launch.argv, OLD_NETWORK), 0);
    assert_no_approvals_bypass(&launch);
}

/// AC3: a caller that already bypasses hook trust gets it once.
#[test]
fn caller_hook_trust_appears_once() {
    let launch = launch(&["exec", HOOK_TRUST, "-"], b"prompt\n");
    assert!(launch.out.status.success(), "{:?}", launch.out);
    assert_eq!(
        launch.argv,
        strings(&["exec", SANDBOX[0], SANDBOX[1], HOOK_TRUST, "-"])
    );
    assert_eq!(count(&launch.argv, HOOK_TRUST), 1);
    assert_no_approvals_bypass(&launch);
}

/// AC4: interactive launches get none of the flags.
#[test]
fn interactive_launches_get_no_injected_flags() {
    let bare = launch(&[], b"");
    assert!(bare.out.status.success(), "{:?}", bare.out);
    assert!(bare.argv.is_empty(), "bare launch: {:?}", bare.argv);

    let resumed = launch(&["resume", "--last"], b"");
    assert!(resumed.out.status.success(), "{:?}", resumed.out);
    assert_eq!(resumed.argv, strings(&["resume", "--last"]));

    for launch in [&bare, &resumed] {
        for flag in [
            "--sandbox",
            "danger-full-access",
            "workspace-write",
            OLD_NETWORK,
            HOOK_TRUST,
        ] {
            assert!(
                !launch.argv.iter().any(|a| a == flag),
                "{flag} in interactive argv {:?}",
                launch.argv
            );
        }
        assert_no_approvals_bypass(launch);
    }
}
