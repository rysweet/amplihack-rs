//! Issue #1570: `--prompt-file <PATH>` for auto mode.
//!
//! Contract (docs/reference/large-payload-delivery.md, "Auto mode" ->
//! "`--prompt-file`"): an option of the `claude`, `copilot`, `codex` and
//! `amplifier` launch subcommands, accepted only with `--auto`, placed before
//! `--`.
//!
//! | Case                                   | Result                                   |
//! | -------------------------------------- | ---------------------------------------- |
//! | `--prompt-file` without `--auto`       | Usage error from the parser, exit 2      |
//! | Option given without a path            | Usage error from the parser, exit 2      |
//! | `--prompt-file` and a `-p` after `--`  | File used; a warning names the ignored -p |
//! | File missing or unreadable             | Exit 1, message names the path           |
//! | File empty after trimming              | Exit 1, message names the path           |

#![cfg(unix)]

use crate::support::amplihack_bin::amplihack_bin;
use amplihack_cli::{Cli, Commands};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

const LAUNCH_SUBCOMMANDS: [&str; 4] = ["claude", "copilot", "codex", "amplifier"];

/// Passthrough args for the four launch subcommands, so a test can check that
/// `--prompt-file` was consumed by the parser and not passed to the agent.
fn passthrough(command: Commands) -> (bool, Vec<String>) {
    match command {
        Commands::Claude {
            auto, claude_args, ..
        } => (auto, claude_args),
        Commands::Copilot { auto, args, .. }
        | Commands::Codex { auto, args, .. }
        | Commands::Amplifier { auto, args, .. } => (auto, args),
        other => panic!("expected a launch subcommand, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Parser level
// ---------------------------------------------------------------------------

#[test]
fn prompt_file_is_a_subcommand_option_not_a_passthrough_arg() {
    for sub in LAUNCH_SUBCOMMANDS {
        for form in [
            vec!["--prompt-file", "/var/tmp/task.md"],
            vec!["--prompt-file=/var/tmp/task.md"],
        ] {
            let mut argv = vec!["amplihack", sub, "--auto", "--max-turns", "3"];
            argv.extend(form.iter().copied());
            let cli = Cli::try_parse_from(&argv)
                .unwrap_or_else(|e| panic!("`{}` must parse: {e}", argv.join(" ")));
            let (auto, args) = passthrough(cli.command);
            assert!(auto);
            assert!(
                args.iter().all(|a| !a.contains("--prompt-file")),
                "`{}`: --prompt-file must be consumed by the parser, not forwarded to the agent; \
                 passthrough was {args:?}",
                argv.join(" ")
            );
            assert!(
                args.iter().all(|a| !a.contains("/var/tmp/task.md")),
                "`{}`: the prompt-file path must not be forwarded as an agent arg or prompt; \
                 passthrough was {args:?}",
                argv.join(" ")
            );
        }
    }
}

#[test]
fn prompt_file_keeps_passthrough_after_double_dash() {
    let cli = Cli::try_parse_from([
        "amplihack",
        "claude",
        "--auto",
        "--prompt-file",
        "/var/tmp/task.md",
        "--",
        "--model",
        "sonnet",
    ])
    .expect("--prompt-file before -- must parse");
    let (_, args) = passthrough(cli.command);
    assert_eq!(args, vec!["--model", "sonnet"]);
}

#[test]
fn prompt_file_without_auto_is_a_usage_error() {
    for sub in LAUNCH_SUBCOMMANDS {
        let err = Cli::try_parse_from(["amplihack", sub, "--prompt-file", "/var/tmp/task.md"])
            .expect_err(&format!(
                "`amplihack {sub} --prompt-file` without --auto must be rejected by the parser"
            ));
        assert_eq!(err.exit_code(), 2, "{sub}: usage errors exit 2; got {err}");
        let rendered = err.to_string();
        assert!(
            rendered.contains("--auto"),
            "{sub}: the usage error must name --auto; got {rendered}"
        );
    }
}

#[test]
fn prompt_file_without_a_path_is_a_usage_error() {
    for sub in LAUNCH_SUBCOMMANDS {
        let err = Cli::try_parse_from(["amplihack", sub, "--auto", "--prompt-file"]).expect_err(
            &format!("`amplihack {sub} --auto --prompt-file` with no path must be rejected"),
        );
        assert_eq!(err.exit_code(), 2, "{sub}: got {err}");
        assert!(
            err.to_string().contains("--prompt-file"),
            "{sub}: the usage error must name --prompt-file; got {err}"
        );
    }
}

// ---------------------------------------------------------------------------
// Binary level
// ---------------------------------------------------------------------------

/// A fake agent that records the stdin of each run to `stdin.<n>`, then
/// prints a short answer.
const FAKE_AGENT: &str = r#"#!/bin/sh
case "$1" in
  --version) echo '2.1.0 (Claude Code)'; exit 0 ;;
esac
n=$(ls "$RECORD_DIR" | grep -c '^stdin\.')
n=$((n+1))
for a in "$@"; do printf '%s\0' "$a"; done > "$RECORD_DIR/argv.$n"
cat > "$RECORD_DIR/stdin.$n"
echo "fake turn $n"
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
        for name in ["claude", "copilot", "codex", "amplifier"] {
            let path = tools.join(name);
            crate::support::exec_stub::write_executable(&path, FAKE_AGENT, 0o755);
        }
        Self {
            tmp,
            home,
            work,
            tools,
            records,
        }
    }

    fn write(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.tmp.path().join(name);
        fs::write(&path, contents).unwrap();
        path
    }

    fn run(&self, args: &[&str]) -> Output {
        let mut c = Command::new(amplihack_bin());
        c.args(args)
            .current_dir(&self.work)
            .env_clear()
            .env(
                "PATH",
                format!("{}:/usr/local/bin:/usr/bin:/bin", self.tools.display()),
            )
            .env("HOME", &self.home)
            .env("RECORD_DIR", &self.records)
            .env("AMPLIHACK_CLAUDE_BINARY_PATH", self.tools.join("claude"))
            .env("AMPLIHACK_COPILOT_BINARY_PATH", self.tools.join("copilot"))
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
            .env("AMPLIHACK_SKIP_REFLECTION", "1")
            .stdin(Stdio::null());
        c.output().expect("failed to run amplihack")
    }

    fn stdin_records(&self) -> Vec<String> {
        let mut out = Vec::new();
        for n in 1.. {
            match fs::read(self.records.join(format!("stdin.{n}"))) {
                Ok(bytes) => out.push(String::from_utf8_lossy(&bytes).into_owned()),
                Err(_) => break,
            }
        }
        out
    }

    fn argv_records(&self) -> Vec<Vec<String>> {
        let mut out = Vec::new();
        for n in 1.. {
            match fs::read(self.records.join(format!("argv.{n}"))) {
                Ok(bytes) => out.push(
                    bytes
                        .split(|b| *b == 0)
                        .filter(|s| !s.is_empty())
                        .map(|s| String::from_utf8_lossy(s).into_owned())
                        .collect(),
                ),
                Err(_) => break,
            }
        }
        out
    }
}

fn describe(out: &Output) -> String {
    format!(
        "status: {}\nstdout:\n{}\nstderr:\n{}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn binary_rejects_prompt_file_without_auto_with_exit_2() {
    let f = Fixture::new();
    let task = f.write("task.md", "do the thing");
    for sub in LAUNCH_SUBCOMMANDS {
        let out = f.run(&[sub, "--prompt-file", task.to_str().unwrap()]);
        assert_eq!(
            out.status.code(),
            Some(2),
            "`amplihack {sub} --prompt-file` without --auto must be a usage error\n{}",
            describe(&out)
        );
    }
    assert!(
        f.argv_records().is_empty(),
        "no agent may be launched on a usage error"
    );
}

#[test]
fn binary_rejects_prompt_file_without_path_with_exit_2() {
    let f = Fixture::new();
    let out = f.run(&["claude", "--auto", "--max-turns", "1", "--prompt-file"]);
    assert_eq!(out.status.code(), Some(2), "{}", describe(&out));
    assert!(f.argv_records().is_empty());
}

#[test]
fn missing_prompt_file_exits_1_naming_the_path() {
    let f = Fixture::new();
    let missing = f.tmp.path().join("no-such-task-1570.md");
    let out = f.run(&[
        "claude",
        "--auto",
        "--max-turns",
        "1",
        "--prompt-file",
        missing.to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(missing.to_str().unwrap()),
        "the error must name the missing path\n{}",
        describe(&out)
    );
    assert!(
        f.argv_records().is_empty(),
        "no agent turn may start without a prompt"
    );
}

#[test]
fn unreadable_prompt_file_exits_1_naming_the_path() {
    let f = Fixture::new();
    // A directory cannot be read as a file, whatever the caller's uid.
    let dir = f.tmp.path().join("task-dir-1570");
    fs::create_dir_all(&dir).unwrap();
    let out = f.run(&[
        "copilot",
        "--auto",
        "--max-turns",
        "1",
        &format!("--prompt-file={}", dir.display()),
    ]);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains(dir.to_str().unwrap()),
        "the error must name the unreadable path\n{}",
        describe(&out)
    );
    assert!(f.argv_records().is_empty());
}

#[test]
fn empty_after_trim_prompt_file_exits_1_naming_the_path() {
    let f = Fixture::new();
    let blank = f.write("blank-1570.md", "  \n\t\n   \n");
    let out = f.run(&[
        "claude",
        "--auto",
        "--max-turns",
        "1",
        "--prompt-file",
        blank.to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains(blank.to_str().unwrap()),
        "the error must name the empty prompt file\n{}",
        describe(&out)
    );
    assert!(f.argv_records().is_empty());
}

#[test]
fn prompt_file_wins_over_dash_p_after_double_dash_with_warning() {
    let f = Fixture::new();
    let file_prompt = "PROMPT-FROM-FILE-1570: refactor the payload layer";
    let task = f.write("task.md", file_prompt);
    let out = f.run(&[
        "claude",
        "--auto",
        "--max-turns",
        "1",
        "--prompt-file",
        task.to_str().unwrap(),
        "--",
        "-p",
        "PROMPT-FROM-ARGV-IGNORED",
    ]);
    assert!(out.status.success(), "{}", describe(&out));

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.lines().any(|line| {
            let lower = line.to_ascii_lowercase();
            lower.contains("ignor") && line.contains("-p") && line.contains("--prompt-file")
        }),
        "a warning must say the -p after -- is ignored in favour of --prompt-file\n{}",
        describe(&out)
    );

    let turn1 = f.stdin_records().into_iter().next().unwrap_or_default()
        + &f.argv_records()
            .into_iter()
            .next()
            .unwrap_or_default()
            .join("\n");
    assert!(
        turn1.contains(file_prompt),
        "turn 1 must carry the prompt from the file\n{}",
        describe(&out)
    );
    assert!(
        !turn1.contains("PROMPT-FROM-ARGV-IGNORED"),
        "the ignored -p value must not reach the agent"
    );
}
