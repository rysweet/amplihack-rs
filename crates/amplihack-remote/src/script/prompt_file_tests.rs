//! Issue #1570 (finding 8): the remote prompt travels on stdin and is saved to
//! a 0600 `mktemp` file that `amplihack claude --auto --prompt-file` reads.
//!
//! The first half checks the generated script text against
//! `docs/reference/large-payload-delivery.md` ("Remote and admiral prompt
//! hand-off"). The second half runs the generated scripts with a local `bash`
//! against stub `amplihack`, `tmux`, `tar` and `git` binaries on `PATH`, and
//! proves that the prompt file holds the prompt byte for byte, excludes the
//! key, is mode 0600 and is removed afterwards.

use super::*;
use std::fs;
use std::io::Write as _;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};

const MARKER: &str = "ISSUE-1570-REMOTE-PROMPT-MARKER";
const KEY: &str = "sk-ant-issue-1570-test-key";

/// Local copy of standard base64 so this contract does not depend on the
/// crate keeping its own encoder.
fn base64(data: &[u8]) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let n = (chunk[0] as u32) << 16
            | (*chunk.get(1).unwrap_or(&0) as u32) << 8
            | *chunk.get(2).unwrap_or(&0) as u32;
        out.push(A[(n >> 18 & 63) as usize] as char);
        out.push(A[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            A[(n >> 6 & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            A[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

fn large_hostile_prompt() -> String {
    let mut prompt = format!("{MARKER}\n'; touch \"$HOME/pwned\"; '\n$(touch \"$HOME/pwned\")\n");
    prompt.push_str("`touch \"$HOME/pwned\"` \\ \" é 日本 🚀\n");
    while prompt.len() < 200 * 1024 {
        prompt.push_str("line of remote prompt text with 'quotes' and $VARS\n");
    }
    prompt.push_str("final line without trailing newline");
    prompt
}

fn command_lines(script: &str) -> Vec<&str> {
    script
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect()
}

fn assert_only_auto_message(err: RemoteError, command: &str) {
    assert!(matches!(err, RemoteError::Validation(_)), "{err:?}");
    let expected =
        format!("remote command '{command}' is not supported: only 'auto' accepts --prompt-file");
    assert!(err.to_string().contains(&expected), "got: {err}");
}

#[test]
fn scripts_never_contain_prompt_bytes_or_base64() {
    let prompt = large_hostile_prompt();
    let direct = build_remote_script("~/workspace", "auto", 10).unwrap();
    let tmux = build_tmux_script("sess-1", "auto", 10).unwrap();
    for script in [&direct, &tmux] {
        assert!(!script.contains(MARKER), "prompt bytes embedded in script");
        assert!(!script.contains(&base64(prompt.as_bytes())[..64]));
        assert!(!script.contains("base64"), "no base64 transport: {script}");
        assert!(!script.contains("$PROMPT\""), "no $PROMPT argv fallback");
        assert!(
            script
                .lines()
                .filter(|line| line.contains("amplihack claude"))
                .all(|line| !line.contains(" -p ")),
            "prompt must not be passed with -p: {script}"
        );
        assert!(
            script.len() < 4096,
            "script must stay small: {}",
            script.len()
        );
    }
}

#[test]
fn only_auto_is_accepted_with_exact_message() {
    for command in ["ultrathink", "analyze", "fix", "smart-orchestrator"] {
        assert_only_auto_message(
            build_remote_script("~/workspace", command, 1).unwrap_err(),
            command,
        );
        assert_only_auto_message(
            build_tmux_script("sess-1", command, 1).unwrap_err(),
            command,
        );
    }
}

#[test]
fn direct_script_reads_key_then_saves_prompt_before_other_commands() {
    let script = build_remote_script("~/workspace", "auto", 10).unwrap();
    let lines = command_lines(&script);
    assert_eq!(
        &lines[..6],
        &[
            "set -e",
            "IFS= read -r ANTHROPIC_API_KEY",
            "export ANTHROPIC_API_KEY",
            "PROMPT_FILE=$(umask 077; mktemp)",
            "trap 'rm -f -- \"$PROMPT_FILE\"' EXIT",
            "cat > \"$PROMPT_FILE\"",
        ],
        "{script}"
    );
    assert!(
        script.contains("amplihack claude --auto --max-turns 10 --prompt-file \"$PROMPT_FILE\""),
        "{script}"
    );
}

#[test]
fn tmux_script_removes_file_inside_session_and_on_launch_failure() {
    let script = build_tmux_script("sess-1", "auto", 10).unwrap();
    let lines = command_lines(&script);
    assert_eq!(
        &lines[..6],
        &[
            "set -e",
            "IFS= read -r ANTHROPIC_API_KEY",
            "export ANTHROPIC_API_KEY",
            "PROMPT_FILE=$(umask 077; mktemp)",
            "case \"$PROMPT_FILE\" in /*[!A-Za-z0-9._/-]*|*..*|[!/]*|'') rm -f -- \"$PROMPT_FILE\"; exit 1;; esac",
            "cat > \"$PROMPT_FILE\"",
        ],
        "{script}"
    );
    assert!(
        !script.contains("trap"),
        "an EXIT trap would delete the file early"
    );
    assert!(script.contains("tmux new-session -d -s sess-1"), "{script}");
    assert!(
        script.contains(
            "\"cd ~/workspace && amplihack claude --auto --max-turns 10 \
             --prompt-file '$PROMPT_FILE'; rm -f -- '$PROMPT_FILE'\""
        ),
        "{script}"
    );
    assert!(
        script.contains("|| { rm -f -- \"$PROMPT_FILE\"; exit 1; }"),
        "{script}"
    );
}

// ---- Executing the generated scripts locally ----

fn write_executable(path: &Path, body: &str) {
    let mut file = fs::File::create(path).unwrap();
    file.write_all(body.as_bytes()).unwrap();
    drop(file);
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

const AMPLIHACK_STUB: &str = r#"#!/bin/sh
out="$STUB_OUT"
printf '%s\0' "$@" > "$out/amplihack.argv"
pf=""; prev=""
for a in "$@"; do
  if [ "$prev" = "--prompt-file" ]; then pf=$a; fi
  prev=$a
done
[ -n "$pf" ] || { echo "stub amplihack: no --prompt-file in argv" >&2; exit 64; }
printf '%s' "$pf" > "$out/prompt_path"
cp "$pf" "$out/prompt_copy"
ls -ln "$pf" | cut -c1-10 > "$out/prompt_mode"
printf '%s' "$ANTHROPIC_API_KEY" > "$out/key_seen"
"#;

const TMUX_STUB: &str = r#"#!/bin/sh
[ -f "$STUB_OUT/tmux_fail" ] && exit 1
last=""
for a in "$@"; do last=$a; done
exec sh -c "$last"
"#;

struct Sandbox {
    _temp: tempfile::TempDir,
    home: std::path::PathBuf,
    out: std::path::PathBuf,
    tmpdir: std::path::PathBuf,
    path: String,
}

fn sandbox() -> Sandbox {
    let temp = tempfile::tempdir().unwrap();
    let (home, out, tmpdir, bin) = (
        temp.path().join("home"),
        temp.path().join("out"),
        temp.path().join("tmp"),
        temp.path().join("bin"),
    );
    for dir in [
        &home,
        &out,
        &tmpdir,
        &bin,
        &home.join(".claude"),
        &home.join("workspace"),
    ] {
        fs::create_dir_all(dir).unwrap();
    }
    write_executable(&bin.join("amplihack"), AMPLIHACK_STUB);
    write_executable(&bin.join("tmux"), TMUX_STUB);
    write_executable(&bin.join("tar"), "#!/bin/sh\nexit 0\n");
    write_executable(&bin.join("git"), "#!/bin/sh\nexit 0\n");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    Sandbox {
        _temp: temp,
        home,
        out,
        tmpdir,
        path,
    }
}

fn run_script(sb: &Sandbox, script: &str, prompt: &str) -> std::process::Output {
    let mut child = Command::new("bash")
        .arg("-c")
        .arg(script)
        .env("HOME", &sb.home)
        .env("PATH", &sb.path)
        .env("TMPDIR", &sb.tmpdir)
        .env("STUB_OUT", &sb.out)
        .env_remove("ANTHROPIC_API_KEY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn bash; E2BIG means the prompt is still embedded in the script");
    let mut stdin = child.stdin.take().unwrap();
    let payload = format!("{KEY}\n{prompt}");
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(payload.as_bytes());
    });
    let output = child.wait_with_output().unwrap();
    writer.join().unwrap();
    output
}

fn assert_delivered_and_removed(sb: &Sandbox, output: &std::process::Output, prompt: &str) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "script failed: {stderr}");
    let copy = fs::read(sb.out.join("prompt_copy")).expect("stub amplihack saw no prompt file");
    assert_eq!(
        copy,
        prompt.as_bytes(),
        "prompt file must hold the prompt byte for byte"
    );
    assert!(
        !String::from_utf8_lossy(&copy).contains(KEY),
        "prompt file must exclude the key"
    );
    assert_eq!(fs::read_to_string(sb.out.join("key_seen")).unwrap(), KEY);
    let mode = fs::read_to_string(sb.out.join("prompt_mode")).unwrap();
    assert_eq!(mode.trim(), "-rw-------", "prompt file must be 0600");
    let path = fs::read_to_string(sb.out.join("prompt_path")).unwrap();
    assert!(
        !Path::new(&path).exists(),
        "prompt file {path} must be removed"
    );
    assert_eq!(
        fs::read_dir(&sb.tmpdir).unwrap().count(),
        0,
        "TMPDIR must be empty"
    );
    assert!(
        !sb.home.join("pwned").exists(),
        "prompt text was interpreted by a shell"
    );
}

#[test]
fn executed_direct_script_delivers_prompt_file_and_removes_it() {
    let small = format!("{MARKER} '; touch \"$HOME/pwned\"; ' small");
    for prompt in [small, large_hostile_prompt()] {
        let sb = sandbox();
        let workspace = sb.home.join("ws").display().to_string();
        let script = build_remote_script(&workspace, "auto", 10).unwrap();
        let output = run_script(&sb, &script, &prompt);
        assert_delivered_and_removed(&sb, &output, &prompt);
    }
}

#[test]
fn executed_tmux_script_delivers_prompt_file_and_removes_it() {
    let sb = sandbox();
    let prompt = large_hostile_prompt();
    let script = build_tmux_script("sess-1", "auto", 10).unwrap();
    let output = run_script(&sb, &script, &prompt);
    assert_delivered_and_removed(&sb, &output, &prompt);
}

#[test]
fn executed_tmux_script_removes_file_when_new_session_fails() {
    let sb = sandbox();
    fs::write(sb.out.join("tmux_fail"), "").unwrap();
    let script = build_tmux_script("sess-1", "auto", 10).unwrap();
    let output = run_script(&sb, &script, "small prompt");
    assert!(
        !output.status.success(),
        "a failed tmux launch must fail the script"
    );
    assert_eq!(
        fs::read_dir(&sb.tmpdir).unwrap().count(),
        0,
        "prompt file left behind"
    );
}

#[test]
fn executed_tmux_script_refuses_a_prompt_path_with_a_quote() {
    let mut sb = sandbox();
    let quoted = sb.tmpdir.join("it's");
    fs::create_dir_all(&quoted).unwrap();
    sb.tmpdir = quoted.clone();
    let script = build_tmux_script("sess-1", "auto", 10).unwrap();
    let output = run_script(&sb, &script, "small prompt");
    assert!(
        !output.status.success(),
        "a mktemp path with a quote must stop the script"
    );
    assert_eq!(
        fs::read_dir(&quoted).unwrap().count(),
        0,
        "prompt file left behind"
    );
}
