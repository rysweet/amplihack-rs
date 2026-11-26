//! Issue #2030 (red phase): delegation spawn configs never put the prompt in
//! argv. Claude gets `claude -p` with the prompt on stdin; `amplifier run` gets
//! the file-reference pointer to a 0600 file in a 0700 directory held by
//! `prompt_guard` until the child is waited on. `gh copilot suggest` cannot read
//! a file or stdin, so building its spawn config fails.
//!
//! API pinned: `SpawnConfig { .., stdin: Option<String>,
//! prompt_guard: Option<Arc<TempDir>> }`, `impl Default`, and a `Debug` that
//! redacts `stdin` as `Some(<n bytes>)`.

#![cfg(unix)]

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;

use amplihack_delegation::platform_cli::{
    AmplifierCli, ClaudeCodeCli, CopilotCli, PlatformCli, SpawnConfig,
};
/// `amplihack_utils::prompt_delivery::FILE_REFERENCE_PREFIX`, unchanged, so
/// the `/proc` parsers keep matching.
const FILE_REFERENCE_PREFIX: &str = "Read and follow the complete instructions in the file ";

const SENTINEL: &str = "SENTINEL-2030-delegation-goal";

fn spawn(cli: &dyn PlatformCli) -> SpawnConfig {
    cli.build_spawn_config(
        SENTINEL,
        "guide",
        env!("CARGO_MANIFEST_DIR"),
        &HashMap::new(),
        &[],
        "",
    )
    .expect("spawn config builds")
}

fn assert_no_prompt(cfg: &SpawnConfig) {
    for a in &cfg.command {
        assert!(
            !a.contains(SENTINEL),
            "prompt text in argv: {:?}",
            cfg.command
        );
    }
}

#[test]
fn spawn_config_default_has_no_stdin_or_guard() {
    let cfg = SpawnConfig::default();
    assert!(cfg.command.is_empty());
    assert!(cfg.stdin.is_none());
    assert!(cfg.prompt_guard.is_none());
}

#[test]
fn claude_delegate_uses_bare_print_and_stdin() {
    let cfg = spawn(&ClaudeCodeCli);
    assert_no_prompt(&cfg);
    assert_eq!(cfg.command.first().map(String::as_str), Some("claude"));
    assert!(cfg.command.iter().any(|a| a == "-p"));
    assert!(cfg.stdin.as_deref().is_some_and(|s| s.contains(SENTINEL)));
    assert!(cfg.prompt_guard.is_none(), "stdin needs no file");
}

#[test]
fn copilot_delegate_refuses_without_a_safe_channel() {
    let err = CopilotCli
        .build_spawn_config(
            SENTINEL,
            "guide",
            env!("CARGO_MANIFEST_DIR"),
            &HashMap::new(),
            &[],
            "",
        )
        .expect_err("gh copilot suggest has no safe prompt channel");
    let msg = err.to_string();
    assert!(msg.contains("no safe channel"), "{msg}");
    assert!(
        !msg.contains(SENTINEL),
        "the error must not echo the prompt: {msg}"
    );
}

#[test]
fn amplifier_delegate_uses_the_pointer() {
    let clis: [&dyn PlatformCli; 1] = [&AmplifierCli];
    for cli in clis {
        let cfg = spawn(cli);
        assert_no_prompt(&cfg);
        assert!(cfg.stdin.is_none(), "{}", cli.platform_name());
        let pointer = cfg
            .command
            .iter()
            .find(|a| a.starts_with(FILE_REFERENCE_PREFIX))
            .unwrap_or_else(|| panic!("{}: {:?}", cli.platform_name(), cfg.command));
        let guard = cfg
            .prompt_guard
            .clone()
            .expect("the guard owns the prompt dir");
        let dir = guard.path().to_path_buf();
        assert_eq!(
            std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let file = std::fs::read_dir(&dir)
            .unwrap()
            .next()
            .expect("one prompt file")
            .unwrap()
            .path();
        assert!(pointer.contains(file.to_str().unwrap()));
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(std::fs::read_to_string(&file).unwrap().contains(SENTINEL));
        drop(guard);
        drop(cfg);
        assert!(!dir.exists(), "dropping the last guard removes the prompt");
    }
}

#[test]
fn debug_redacts_stdin() {
    let cfg = spawn(&ClaudeCodeCli);
    let dbg = format!("{cfg:?}");
    assert!(!dbg.contains(SENTINEL), "{dbg}");
    assert!(dbg.contains("bytes"), "{dbg}");
}
