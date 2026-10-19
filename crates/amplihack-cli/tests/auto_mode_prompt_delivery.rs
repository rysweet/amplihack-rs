//! Auto-mode prompt-delivery tests (issues #652, #709 and #1570).
//!
//! Auto mode builds each turn through the shared launcher prompt-delivery
//! layer. Since #1570 (docs/reference/large-payload-delivery.md, "Per-tool
//! channels" and "Auto mode"):
//!
//! | Tool      | Turn prompt above 4096 bytes                  |
//! | --------- | --------------------------------------------- |
//! | Claude    | stdin, with `AMPLIHACK_NONINTERACTIVE=1` set  |
//! | Copilot   | one short file-reference argument             |
//! | Amplifier | one short file-reference argument             |
//!
//! No prompt ever becomes an argv element larger than `MAX_ARG_STRLEN`
//! (131,072 bytes), and the end-to-end tests below drive the real binary with
//! a 200 KiB `--prompt-file` against stub agents on `PATH`.

use amplihack_cli::commands::auto_mode::{
    AutoModePromptDeliveryOptions, AutoModeTool, build_auto_command_with_prompt_delivery,
};
use amplihack_utils::prompt_delivery::{DeliveryMode, FILE_REFERENCE_PREFIX, PromptDelivery};

const PAYLOAD_SIZE: usize = 64 * 1024;
const MAX_ARG_STRLEN: usize = 131_072;
const FILE_REFERENCE_MAX_BYTES: usize = 512;

fn synthetic_prompt() -> String {
    let pattern = "auto-mode issue 652: don't truncate 'quotes' or $(shell)\n";
    let mut prompt = String::with_capacity(PAYLOAD_SIZE);
    while prompt.len() < PAYLOAD_SIZE {
        prompt.push_str(pattern);
    }
    prompt.truncate(PAYLOAD_SIZE);
    prompt
}

/// A 200 KiB prompt with multi-byte UTF-8, shell metacharacters and newlines.
/// It starts and ends with non-whitespace so trimming cannot change it.
fn large_prompt() -> String {
    let pattern = "issue 1570 payload: 'sq' \"dq\" $(id) `tick` é € 𝄞 中文 \\n\n";
    let mut prompt = String::from("TASK-1570-BEGIN\n");
    while prompt.len() < 200 * 1024 {
        prompt.push_str(pattern);
    }
    prompt.push_str("TASK-1570-END");
    prompt
}

fn argv(command: &std::process::Command) -> Vec<String> {
    command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}

fn env_value(command: &std::process::Command, key: &str) -> Option<String> {
    command
        .get_envs()
        .find(|(k, _)| *k == key)
        .and_then(|(_, v)| v.map(|v| v.to_string_lossy().into_owned()))
}

fn options(
    tool: AutoModeTool,
    dir: &std::path::Path,
    prompt: &str,
    requested_delivery: PromptDelivery,
) -> AutoModePromptDeliveryOptions {
    AutoModePromptDeliveryOptions {
        tool,
        execution_dir: dir.to_path_buf(),
        project_dir: dir.to_path_buf(),
        node_options: None,
        passthrough_args: vec![],
        prompt: prompt.to_string(),
        requested_delivery,
    }
}

/// Asserts the delivered command carries the prompt as exactly one short
/// file-reference argument whose file holds the prompt byte-for-byte.
fn assert_file_reference(
    delivered: &amplihack_launcher::prompt_delivery::DeliveredCommand,
    prompt: &str,
) {
    assert_eq!(delivered.selected_mode, DeliveryMode::FileReference);
    assert!(
        delivered.stdin_payload.is_none(),
        "stdin_payload must be Some only in Stdin mode"
    );
    let args = argv(&delivered.command);
    assert!(
        args.iter().all(|arg| !arg.contains(prompt)),
        "the prompt must not appear in argv in file-reference mode"
    );
    let refs: Vec<&String> = args
        .iter()
        .filter(|arg| arg.starts_with(FILE_REFERENCE_PREFIX))
        .collect();
    assert_eq!(
        refs.len(),
        1,
        "exactly one file-reference argument expected; args: {:?}",
        args.iter().map(|a| a.len()).collect::<Vec<_>>()
    );
    assert!(refs[0].len() < FILE_REFERENCE_MAX_BYTES);
    assert!(!refs[0].contains('\n'));
    let path = delivered
        .delivery_handle
        .tempfile_path()
        .expect("file-reference mode owns a temporary file");
    assert!(
        refs[0].contains(path.to_str().unwrap()),
        "the file-reference argument must name the handle's temporary file"
    );
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        prompt,
        "the referenced file must hold the prompt byte-for-byte"
    );
}

#[test]
fn auto_mode_command_records_prompt_delivery_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let prompt = synthetic_prompt();

    let delivered = build_auto_command_with_prompt_delivery(AutoModePromptDeliveryOptions {
        tool: AutoModeTool::Copilot,
        execution_dir: dir.path().to_path_buf(),
        project_dir: dir.path().to_path_buf(),
        node_options: Some("--max-old-space-size=32768".to_string()),
        passthrough_args: vec!["--model".to_string(), "gpt-5.5".to_string()],
        prompt: prompt.clone(),
        requested_delivery: PromptDelivery::Tempfile,
    })
    .expect("auto-mode delivery-aware command should build");

    // Copilot has no tempfile or stdin contract, so an explicit tempfile
    // request degrades through stdin to file-reference (#1570).
    assert!(
        !delivered.warnings.is_empty(),
        "an unsupported explicit request must produce a deterministic warning"
    );
    assert_file_reference(&delivered, &prompt);
}

#[test]
fn auto_mode_does_not_duplicate_prompt_between_wrapper_and_child_args() {
    let dir = tempfile::tempdir().unwrap();
    let prompt = synthetic_prompt();

    let delivered = build_auto_command_with_prompt_delivery(options(
        AutoModeTool::Amplifier,
        dir.path(),
        &prompt,
        PromptDelivery::Auto,
    ))
    .expect("amplifier auto-mode delivery-aware command should build");

    // Above 4096 bytes Amplifier receives a file reference (#1570), so the
    // prompt is in argv neither once nor twice.
    assert_file_reference(&delivered, &prompt);
}

#[test]
fn amplifier_auto_mode_uses_documented_run_positional_prompt_contract() {
    let dir = tempfile::tempdir().unwrap();
    let prompt = "issue #709: keep quotes, $PATH, and\nnewlines".to_string();

    let delivered = build_auto_command_with_prompt_delivery(AutoModePromptDeliveryOptions {
        tool: AutoModeTool::Amplifier,
        execution_dir: dir.path().to_path_buf(),
        project_dir: dir.path().to_path_buf(),
        node_options: None,
        passthrough_args: vec![
            "--provider".to_string(),
            "openai".to_string(),
            "--model".to_string(),
            "amp-test".to_string(),
        ],
        prompt: prompt.clone(),
        requested_delivery: PromptDelivery::Auto,
    })
    .expect("amplifier auto-mode delivery-aware command should build");

    let args = argv(&delivered.command);
    let passthrough_start = args
        .iter()
        .position(|arg| arg == "--")
        .expect("nested amplihack command must separate wrapper and Amplifier args")
        + 1;
    assert_eq!(
        &args[passthrough_start..],
        &[
            "run".to_string(),
            "--provider".to_string(),
            "openai".to_string(),
            "--model".to_string(),
            "amp-test".to_string(),
            prompt,
        ],
        "Amplifier auto-mode must use `amplifier run [OPTIONS] [PROMPT]` without a synthetic prompt flag"
    );
}

#[test]
fn amplifier_auto_mode_rejects_explicit_unsupported_prompt_delivery_modes() {
    let dir = tempfile::tempdir().unwrap();
    let prompt = synthetic_prompt();

    for requested_delivery in [PromptDelivery::Tempfile, PromptDelivery::Stdin] {
        let err = build_auto_command_with_prompt_delivery(AutoModePromptDeliveryOptions {
            tool: AutoModeTool::Amplifier,
            execution_dir: dir.path().to_path_buf(),
            project_dir: dir.path().to_path_buf(),
            node_options: None,
            passthrough_args: vec![],
            prompt: prompt.clone(),
            requested_delivery,
        })
        .expect_err("Amplifier auto-mode must reject unsupported explicit delivery modes");
        let message = format!("{err:#}");
        assert!(
            message.contains("Amplifier prompt delivery mode"),
            "error should identify Amplifier prompt-delivery policy; got: {message}"
        );
        assert!(
            message.contains(match requested_delivery {
                PromptDelivery::Tempfile => "tempfile",
                PromptDelivery::Stdin => "stdin",
                _ => unreachable!(),
            }),
            "error should name rejected mode; got: {message}"
        );
    }
}

// ---------------------------------------------------------------------------
// #1570: per-tool channels at the command-builder level
// ---------------------------------------------------------------------------

#[test]
fn claude_auto_mode_large_prompt_goes_to_stdin_with_noninteractive() {
    let dir = tempfile::tempdir().unwrap();
    let prompt = large_prompt();

    let delivered = build_auto_command_with_prompt_delivery(options(
        AutoModeTool::Claude,
        dir.path(),
        &prompt,
        PromptDelivery::Auto,
    ))
    .expect("claude auto-mode command should build");

    assert_eq!(
        delivered.selected_mode,
        DeliveryMode::Stdin,
        "Claude's large-prompt channel is stdin"
    );
    assert_eq!(
        delivered.stdin_payload.as_deref(),
        Some(prompt.as_bytes()),
        "stdin_payload must hold the prompt byte-for-byte"
    );
    let args = argv(&delivered.command);
    assert!(
        args.iter().all(|arg| arg.len() <= 4096),
        "no argv element may carry the prompt; lengths: {:?}",
        args.iter().map(String::len).collect::<Vec<_>>()
    );
    assert_eq!(
        args.last().map(String::as_str),
        Some("-p"),
        "the nested claude reads the prompt from stdin after a bare -p"
    );
    assert_eq!(
        env_value(&delivered.command, "AMPLIHACK_NONINTERACTIVE").as_deref(),
        Some("1"),
        "AMPLIHACK_NONINTERACTIVE=1 must be set whenever a stdin payload is present"
    );
}

#[test]
fn claude_auto_mode_small_prompt_stays_on_argv_without_stdin_payload() {
    let dir = tempfile::tempdir().unwrap();
    let prompt = "short prompt for issue 1570";

    let delivered = build_auto_command_with_prompt_delivery(options(
        AutoModeTool::Claude,
        dir.path(),
        prompt,
        PromptDelivery::Auto,
    ))
    .expect("claude auto-mode command should build");

    assert_eq!(delivered.selected_mode, DeliveryMode::Argv);
    assert!(delivered.stdin_payload.is_none());
    assert_eq!(
        argv(&delivered.command).last().map(String::as_str),
        Some(prompt)
    );
}

#[test]
fn copilot_auto_mode_large_prompt_uses_file_reference() {
    let dir = tempfile::tempdir().unwrap();
    let prompt = large_prompt();

    let delivered = build_auto_command_with_prompt_delivery(options(
        AutoModeTool::Copilot,
        dir.path(),
        &prompt,
        PromptDelivery::Auto,
    ))
    .expect("copilot auto-mode command should build");

    assert_file_reference(&delivered, &prompt);
}

// ---------------------------------------------------------------------------
// #1570: end to end through the real binary with stub agents
// ---------------------------------------------------------------------------

#[cfg(unix)]
mod end_to_end {
    use super::{FILE_REFERENCE_MAX_BYTES, MAX_ARG_STRLEN, large_prompt};
    use crate::support::amplihack_bin::amplihack_bin;
    use amplihack_utils::prompt_delivery::FILE_REFERENCE_PREFIX;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output, Stdio};

    /// Records, per run `n`: its argv (NUL-separated), its parent's argv, its
    /// environment and its stdin.
    const FAKE_CLAUDE: &str = r#"#!/bin/sh
case "$1" in
  --version) echo '2.1.0 (Claude Code)'; exit 0 ;;
esac
n=$(ls "$RECORD_DIR" | grep -c '^argv\.')
n=$((n+1))
for a in "$@"; do printf '%s\0' "$a"; done > "$RECORD_DIR/argv.$n"
cat "/proc/$PPID/cmdline" > "$RECORD_DIR/parent_argv.$n" 2>/dev/null || :
env > "$RECORD_DIR/env.$n"
cat > "$RECORD_DIR/stdin.$n"
echo "fake claude turn $n"
"#;

    /// Also copies the file named by a file-reference argument, since the
    /// file is deleted once the turn ends.
    const FAKE_COPILOT: &str = r#"#!/bin/sh
case "$1" in
  --version) echo '1.0.0'; exit 0 ;;
esac
n=$(ls "$RECORD_DIR" | grep -c '^argv\.')
n=$((n+1))
: > "$RECORD_DIR/argv.$n"
for a in "$@"; do
  printf '%s\0' "$a" >> "$RECORD_DIR/argv.$n"
  case "$a" in
    "$FILE_REFERENCE_PREFIX"*)
      rest=${a#"$FILE_REFERENCE_PREFIX"}
      path=${rest%%". Its contents are the task prompt."}
      printf '%s' "$path" > "$RECORD_DIR/ref_path.$n"
      cp "$path" "$RECORD_DIR/ref.$n" ;;
  esac
done
cat "/proc/$PPID/cmdline" > "$RECORD_DIR/parent_argv.$n" 2>/dev/null || :
env > "$RECORD_DIR/env.$n"
echo "fake copilot turn $n"
"#;

    struct Fixture {
        tmp: tempfile::TempDir,
        home: PathBuf,
        work: PathBuf,
        tools: PathBuf,
        records: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let home = tmp.path().join("home");
            let work = tmp.path().join("work");
            let tools = tmp.path().join("tools");
            let records = tmp.path().join("records");
            for d in [&home, &work, &tools, &records] {
                fs::create_dir_all(d).unwrap();
            }
            for (name, body) in [("claude", FAKE_CLAUDE), ("copilot", FAKE_COPILOT)] {
                let path = tools.join(name);
                crate::support::exec_stub::write_executable(&path, body, 0o755);
            }
            Self {
                tmp,
                home,
                work,
                tools,
                records,
            }
        }

        /// No inherited environment, and in particular no
        /// `AMPLIHACK_NONINTERACTIVE`, so the test sees what auto mode sets.
        fn run(&self, args: &[&str]) -> Output {
            Command::new(amplihack_bin())
                .args(args)
                .current_dir(&self.work)
                .env_clear()
                .env(
                    "PATH",
                    format!("{}:/usr/local/bin:/usr/bin:/bin", self.tools.display()),
                )
                .env("HOME", &self.home)
                .env("RECORD_DIR", &self.records)
                .env("FILE_REFERENCE_PREFIX", FILE_REFERENCE_PREFIX)
                .env("AMPLIHACK_CLAUDE_BINARY_PATH", self.tools.join("claude"))
                .env("AMPLIHACK_COPILOT_BINARY_PATH", self.tools.join("copilot"))
                .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
                .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
                .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
                .env("AMPLIHACK_SKIP_REFLECTION", "1")
                .stdin(Stdio::null())
                .output()
                .expect("failed to run amplihack")
        }

        fn record(&self, name: &str) -> Option<Vec<u8>> {
            fs::read(self.records.join(name)).ok()
        }

        fn turns(&self) -> usize {
            (1..)
                .take_while(|n| self.records.join(format!("argv.{n}")).is_file())
                .count()
        }

        fn argv(&self, name: &str) -> Vec<Vec<u8>> {
            self.record(name)
                .unwrap_or_default()
                .split(|b| *b == 0)
                .filter(|s| !s.is_empty())
                .map(<[u8]>::to_vec)
                .collect()
        }
    }

    fn describe(out: &Output) -> String {
        format!(
            "status: {}\nstdout (tail):\n{}\nstderr (tail):\n{}",
            out.status,
            tail(&out.stdout),
            tail(&out.stderr)
        )
    }

    fn tail(bytes: &[u8]) -> String {
        let s = String::from_utf8_lossy(bytes);
        let start = s.len().saturating_sub(4000);
        let start = (start..s.len())
            .find(|i| s.is_char_boundary(*i))
            .unwrap_or(0);
        s[start..].to_string()
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    fn assert_argv_bounded(f: &Fixture, turn: usize, payload: &[u8]) {
        for name in [format!("argv.{turn}"), format!("parent_argv.{turn}")] {
            for arg in f.argv(&name) {
                assert!(
                    arg.len() <= MAX_ARG_STRLEN,
                    "{name}: argv element of {} bytes exceeds MAX_ARG_STRLEN",
                    arg.len()
                );
                assert!(
                    arg.len() <= 4096,
                    "{name}: argv element of {} bytes; a large prompt must not ride on argv",
                    arg.len()
                );
                assert!(
                    !contains(&arg, &payload[..64]),
                    "{name}: the prompt payload must not appear in argv"
                );
            }
        }
    }

    #[test]
    fn claude_auto_mode_delivers_200k_prompt_file_on_stdin() {
        let f = Fixture::new();
        let payload = large_prompt();
        let task = f.tmp.path().join("task-1570.md");
        fs::write(&task, &payload).unwrap();

        let out = f.run(&[
            "claude",
            "--auto",
            "--max-turns",
            "1",
            "--prompt-file",
            task.to_str().unwrap(),
        ]);

        assert!(
            out.status.success(),
            "auto mode must spawn and finish without E2BIG\n{}",
            describe(&out)
        );
        assert!(
            f.turns() >= 1,
            "the stub claude must have been launched\n{}",
            describe(&out)
        );
        for turn in 1..=f.turns() {
            assert_argv_bounded(&f, turn, payload.as_bytes());
        }

        let stdin = f.record("stdin.1").unwrap_or_default();
        assert!(
            contains(&stdin, payload.as_bytes()),
            "turn 1 must reach claude on stdin with the full prompt byte-for-byte \
             (stdin was {} bytes, payload {} bytes)\n{}",
            stdin.len(),
            payload.len(),
            describe(&out)
        );

        let env = String::from_utf8_lossy(&f.record("env.1").unwrap_or_default()).into_owned();
        assert!(
            env.lines().any(|l| l == "AMPLIHACK_NONINTERACTIVE=1"),
            "AMPLIHACK_NONINTERACTIVE=1 must reach the child when the prompt is on stdin"
        );
    }

    #[test]
    fn copilot_auto_mode_delivers_200k_prompt_file_by_file_reference() {
        let f = Fixture::new();
        let payload = large_prompt();
        let task = f.tmp.path().join("task-1570.md");
        fs::write(&task, &payload).unwrap();

        let out = f.run(&[
            "copilot",
            "--auto",
            "--max-turns",
            "1",
            &format!("--prompt-file={}", task.display()),
        ]);

        assert!(out.status.success(), "{}", describe(&out));
        assert!(
            f.turns() >= 1,
            "stub copilot not launched\n{}",
            describe(&out)
        );
        for turn in 1..=f.turns() {
            assert_argv_bounded(&f, turn, payload.as_bytes());
        }

        let argv = f.argv("argv.1");
        let refs: Vec<&Vec<u8>> = argv
            .iter()
            .filter(|a| a.starts_with(FILE_REFERENCE_PREFIX.as_bytes()))
            .collect();
        assert_eq!(
            refs.len(),
            1,
            "copilot must receive exactly one file-reference argument\n{}",
            describe(&out)
        );
        assert!(refs[0].len() < FILE_REFERENCE_MAX_BYTES);
        assert!(
            refs[0].starts_with(b"Read and follow the complete instructions in the file "),
            "file-reference argument must use the fixed template"
        );

        let referenced = f
            .record("ref.1")
            .expect("the referenced file must exist while copilot runs");
        assert!(
            contains(&referenced, payload.as_bytes()),
            "the referenced file must hold the full prompt byte-for-byte"
        );

        let ref_path = String::from_utf8(f.record("ref_path.1").unwrap()).unwrap();
        assert!(
            !Path::new(&ref_path).exists(),
            "the temporary prompt file {ref_path} must be removed after the turn"
        );
    }
}
