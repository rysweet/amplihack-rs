//! TDD red-phase tests for `amplihack_utils::prompt_delivery`.
//!
//! Tracks Simard issue #1897 and the linked amplihack-rs follow-up. These
//! tests are written against the public API surface only; they DELIBERATELY
//! fail until `prompt_delivery::{from_env, select_mode, deliver}` are
//! implemented per the design note.
//!
//! Run with:
//!     cargo test -p amplihack-utils --test prompt_delivery
//!
//! Expected red state right now: every test that exercises real behaviour
//! (everything except the pure type-shape sanity checks) fails. That is the
//! TDD contract — the implementation PR turns them green.

use std::os::unix::fs::PermissionsExt;
use std::process::Command;

use amplihack_utils::prompt_delivery::{
    AUTO_TEMPFILE_THRESHOLD_BYTES, DeliveryCaps, DeliveryMode, ENV_VAR_NAME, PromptDelivery,
    deliver, from_env, select_mode,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn claude_like_caps() -> DeliveryCaps {
    DeliveryCaps::all_modes("--prompt-file")
}

fn copilot_like_caps() -> DeliveryCaps {
    DeliveryCaps::argv_and_tempfile("--prompt-file")
}

fn codex_like_caps() -> DeliveryCaps {
    DeliveryCaps::argv_only()
}

/// Build a prompt of exactly `size` bytes. Content includes apostrophes,
/// double quotes, and backslashes so that any surviving shell-escape bug
/// from #1871 surfaces immediately.
fn synthetic_prompt(size: usize) -> String {
    let pattern = b"a'\\\"b\n";
    let mut out = Vec::with_capacity(size);
    while out.len() < size {
        out.extend_from_slice(pattern);
    }
    out.truncate(size);
    String::from_utf8(out).expect("ascii pattern is valid utf-8")
}

/// Mutex-guard env access — tests within this binary run on a shared
/// process and `set_var` is not thread-safe across threads.
fn with_env<R>(value: Option<&str>, f: impl FnOnce() -> R) -> R {
    use std::sync::{Mutex, OnceLock};
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    // Ignore poisoning: when a TDD-red test panics holding the lock we still
    // want subsequent env tests to run their own assertions.
    let _g = LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let previous = std::env::var(ENV_VAR_NAME).ok();
    // SAFETY: serialized via LOCK above.
    unsafe {
        match value {
            Some(v) => std::env::set_var(ENV_VAR_NAME, v),
            None => std::env::remove_var(ENV_VAR_NAME),
        }
    }
    let out = f();
    unsafe {
        match previous {
            Some(v) => std::env::set_var(ENV_VAR_NAME, v),
            None => std::env::remove_var(ENV_VAR_NAME),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// 1. auto_selects_argv_for_small_prompt
// ---------------------------------------------------------------------------

#[test]
fn auto_selects_argv_for_small_prompt() {
    let caps = claude_like_caps();
    let mode = select_mode(PromptDelivery::Auto, 100, &caps);
    assert_eq!(
        mode,
        DeliveryMode::Argv,
        "100-byte prompt + full-cap binary should auto-select Argv"
    );
}

// ---------------------------------------------------------------------------
// 2. auto_selects_tempfile_for_large_prompt
// ---------------------------------------------------------------------------

#[test]
fn auto_selects_tempfile_for_large_prompt() {
    let caps = claude_like_caps();
    let mode = select_mode(PromptDelivery::Auto, 8 * 1024, &caps);
    assert_eq!(
        mode,
        DeliveryMode::Tempfile,
        "8 KiB prompt + tempfile-capable binary should auto-select Tempfile"
    );
}

// ---------------------------------------------------------------------------
// 3. auto_falls_back_to_stdin_when_tempfile_unsupported
// ---------------------------------------------------------------------------

#[test]
fn auto_falls_back_to_stdin_when_tempfile_unsupported() {
    let caps = DeliveryCaps {
        supports_argv: true,
        supports_tempfile: false,
        supports_stdin: true,
        tempfile_flag: None,
    };
    let mode = select_mode(PromptDelivery::Auto, 8 * 1024, &caps);
    assert_eq!(
        mode,
        DeliveryMode::Stdin,
        "8 KiB prompt + stdin-only binary should auto-select Stdin"
    );
}

// ---------------------------------------------------------------------------
// 4. auto_falls_back_to_argv_when_no_long_form_supported
// ---------------------------------------------------------------------------

#[test]
fn auto_falls_back_to_argv_when_no_long_form_supported() {
    let caps = codex_like_caps();
    let mode = select_mode(PromptDelivery::Auto, 8 * 1024, &caps);
    assert_eq!(
        mode,
        DeliveryMode::Argv,
        "argv-only binary must fall back to Argv even for large prompts"
    );
}

// ---------------------------------------------------------------------------
// 5. explicit_env_overrides_auto
// ---------------------------------------------------------------------------

#[test]
fn explicit_env_overrides_auto() {
    with_env(Some("stdin"), || {
        let parsed = from_env();
        assert_eq!(
            parsed,
            PromptDelivery::Stdin,
            "env=stdin should parse to Stdin"
        );
        let mode = select_mode(parsed, 100, &claude_like_caps());
        assert_eq!(
            mode,
            DeliveryMode::Stdin,
            "explicit Stdin must win over the auto-threshold rule"
        );
    });
}

// ---------------------------------------------------------------------------
// 6. explicit_env_with_unsupported_mode_degrades_deterministically
// ---------------------------------------------------------------------------

#[test]
fn explicit_env_with_unsupported_mode_degrades_deterministically() {
    let caps = codex_like_caps(); // argv-only
    let mode = select_mode(PromptDelivery::Stdin, 100, &caps);
    assert_eq!(
        mode,
        DeliveryMode::Argv,
        "explicit Stdin on argv-only binary must degrade to Argv"
    );

    // tempfile request on a stdin-only binary should land on stdin per the
    // documented degradation chain (Tempfile → Stdin → Argv).
    let caps = DeliveryCaps {
        supports_argv: true,
        supports_tempfile: false,
        supports_stdin: true,
        tempfile_flag: None,
    };
    let mode = select_mode(PromptDelivery::Tempfile, 100, &caps);
    assert_eq!(
        mode,
        DeliveryMode::Stdin,
        "explicit Tempfile on stdin-capable binary must degrade to Stdin"
    );
}

// ---------------------------------------------------------------------------
// 7. invalid_env_value_warns_and_defaults_to_auto
// ---------------------------------------------------------------------------

#[test]
fn invalid_env_value_warns_and_defaults_to_auto() {
    with_env(Some("garbage-value-xyz"), || {
        let parsed = from_env();
        assert_eq!(
            parsed,
            PromptDelivery::Auto,
            "unrecognised env value must fall back to Auto"
        );
    });
}

// ---------------------------------------------------------------------------
// 8. parser_is_case_insensitive
// ---------------------------------------------------------------------------

#[test]
fn parser_is_case_insensitive() {
    for value in ["argv", "ARGV", "ArGv"] {
        with_env(Some(value), || {
            assert_eq!(
                from_env(),
                PromptDelivery::Argv,
                "env={value} should parse to Argv"
            );
        });
    }
    for value in ["tempfile", "TempFile", "TEMPFILE"] {
        with_env(Some(value), || {
            assert_eq!(
                from_env(),
                PromptDelivery::Tempfile,
                "env={value} should parse to Tempfile"
            );
        });
    }
    for value in ["stdin", "Stdin", "STDIN"] {
        with_env(Some(value), || {
            assert_eq!(
                from_env(),
                PromptDelivery::Stdin,
                "env={value} should parse to Stdin"
            );
        });
    }
    for value in ["auto", "Auto", "AUTO"] {
        with_env(Some(value), || {
            assert_eq!(
                from_env(),
                PromptDelivery::Auto,
                "env={value} should parse to Auto"
            );
        });
    }
}

// ---------------------------------------------------------------------------
// 9. tempfile_handle_drops_file_on_drop
// ---------------------------------------------------------------------------

#[test]
fn tempfile_handle_drops_file_on_drop() {
    let caps = claude_like_caps();
    let prompt = synthetic_prompt(8 * 1024);
    let mut cmd = Command::new("/bin/true");
    let handle = deliver(&mut cmd, &prompt, PromptDelivery::Tempfile, &caps)
        .expect("deliver should succeed for tempfile mode");
    assert_eq!(
        handle.mode(),
        DeliveryMode::Tempfile,
        "handle mode should be Tempfile"
    );
    let path = handle
        .tempfile_path()
        .expect("tempfile mode handle must expose a path")
        .to_path_buf();
    assert!(
        path.exists(),
        "tempfile path should exist while handle is alive: {path:?}"
    );
    drop(handle);
    assert!(
        !path.exists(),
        "tempfile must be unlinked when handle is dropped: {path:?}"
    );
}

// ---------------------------------------------------------------------------
// 10. tempfile_handle_perms_are_0600 (Unix-only)
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn tempfile_handle_perms_are_0600() {
    let caps = claude_like_caps();
    let prompt = synthetic_prompt(8 * 1024);
    let mut cmd = Command::new("/bin/true");
    let handle = deliver(&mut cmd, &prompt, PromptDelivery::Tempfile, &caps)
        .expect("deliver should succeed for tempfile mode");
    let path = handle
        .tempfile_path()
        .expect("tempfile mode handle must expose a path");
    let meta = std::fs::metadata(path).expect("temp file should exist");
    let mode_bits = meta.permissions().mode() & 0o7777;
    assert_eq!(
        mode_bits, 0o600,
        "tempfile must be created with 0600 permissions"
    );
}

// ---------------------------------------------------------------------------
// 11. deliver_mutates_command_for_argv
// ---------------------------------------------------------------------------

#[test]
fn deliver_mutates_command_for_argv() {
    let caps = codex_like_caps();
    let prompt = "hello prompt";
    let mut cmd = Command::new("/bin/true");
    cmd.arg("--existing");
    let _handle = deliver(&mut cmd, prompt, PromptDelivery::Argv, &caps)
        .expect("argv delivery should succeed");
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert!(
        args.iter().any(|a| a == prompt),
        "argv mode must append the prompt verbatim. Got args: {args:?}"
    );
}

// ---------------------------------------------------------------------------
// 12. deliver_mutates_command_for_tempfile
// ---------------------------------------------------------------------------

#[test]
fn deliver_mutates_command_for_tempfile() {
    let caps = claude_like_caps();
    let prompt = synthetic_prompt(8 * 1024);
    let mut cmd = Command::new("/bin/true");
    let handle = deliver(&mut cmd, &prompt, PromptDelivery::Tempfile, &caps)
        .expect("tempfile delivery should succeed");
    let path = handle
        .tempfile_path()
        .expect("path must be exposed")
        .to_path_buf();
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert!(
        args.iter().any(|a| a == "--prompt-file"),
        "tempfile mode must append the documented flag. Got args: {args:?}"
    );
    assert!(
        args.iter().any(|a| a.as_str() == path.to_string_lossy()),
        "tempfile mode must append the temp file path. Got args: {args:?}"
    );
    // And the prompt must NEVER appear inline in argv.
    assert!(
        !args.iter().any(|a| a.contains(&prompt[..200])),
        "tempfile mode must not leak the prompt into argv"
    );
}

// ---------------------------------------------------------------------------
// 13. auto_threshold_is_inclusive
// ---------------------------------------------------------------------------

#[test]
fn auto_threshold_is_inclusive() {
    let caps = claude_like_caps();
    let mode = select_mode(PromptDelivery::Auto, AUTO_TEMPFILE_THRESHOLD_BYTES, &caps);
    assert_eq!(
        mode,
        DeliveryMode::Argv,
        "prompts AT the threshold should stay on argv (inclusive boundary)"
    );
    let mode = select_mode(
        PromptDelivery::Auto,
        AUTO_TEMPFILE_THRESHOLD_BYTES + 1,
        &caps,
    );
    assert_eq!(
        mode,
        DeliveryMode::Tempfile,
        "prompts one byte over the threshold should promote to Tempfile"
    );
}

// ---------------------------------------------------------------------------
// 14. copilot_like_caps_auto_long_prompt_uses_tempfile
// ---------------------------------------------------------------------------

#[test]
fn copilot_like_caps_auto_long_prompt_uses_tempfile() {
    let caps = copilot_like_caps();
    let mode = select_mode(PromptDelivery::Auto, 64 * 1024, &caps);
    assert_eq!(
        mode,
        DeliveryMode::Tempfile,
        "Copilot-like caps (no stdin, has tempfile) should pick Tempfile for 64 KiB"
    );
}
