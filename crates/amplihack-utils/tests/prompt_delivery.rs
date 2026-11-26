//! TDD red-phase tests for `amplihack_utils::prompt_delivery`.
//!
//! Tracks Simard issue #1897 and the linked amplihack-rs follow-up. These
//! tests are written against the public API surface only; they DELIBERATELY
//! fail until `prompt_delivery::{from_env, try_select_mode, deliver}` are
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
    DeliveryCaps, DeliveryMode, ENV_VAR_NAME, FILE_REFERENCE_PREFIX, FILE_REFERENCE_TEMPLATE,
    PromptDelivery, deliver, from_env, try_select_mode,
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
fn auto_never_selects_argv_for_small_prompt() {
    // Issue #2030: a short prompt no longer stays on argv.
    let caps = claude_like_caps();
    let mode = try_select_mode(PromptDelivery::Auto, &caps).unwrap();
    assert_eq!(
        mode,
        DeliveryMode::Stdin,
        "100-byte prompt + full-cap binary should auto-select Stdin"
    );
}

// ---------------------------------------------------------------------------
// 2. auto_selects_tempfile_for_large_prompt
// ---------------------------------------------------------------------------

#[test]
fn auto_selects_stdin_before_tempfile_for_large_prompt() {
    let caps = claude_like_caps();
    let mode = try_select_mode(PromptDelivery::Auto, &caps).unwrap();
    assert_eq!(
        mode,
        DeliveryMode::Stdin,
        "8 KiB prompt + stdin-capable binary should auto-select Stdin (#2030 order)"
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
        supports_file_reference: false,
        tempfile_flag: None,
    };
    let mode = try_select_mode(PromptDelivery::Auto, &caps).unwrap();
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
fn auto_refuses_when_no_long_form_supported() {
    // Issue #2030: an argv-only binary has no safe channel, so the selector
    // errors; nothing ever answers Argv.
    let caps = codex_like_caps();
    assert!(try_select_mode(PromptDelivery::Auto, &caps).is_err());
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
        let mode = try_select_mode(parsed, &claude_like_caps()).unwrap();
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
    assert!(
        try_select_mode(PromptDelivery::Stdin, &caps).is_err(),
        "explicit Stdin on argv-only binary must not degrade to Argv (#2030)"
    );

    // tempfile request on a stdin-only binary should land on stdin per the
    // documented degradation chain (Tempfile → Stdin → FileReference).
    let caps = DeliveryCaps {
        supports_argv: true,
        supports_tempfile: false,
        supports_stdin: true,
        supports_file_reference: false,
        tempfile_flag: None,
    };
    let mode = try_select_mode(PromptDelivery::Tempfile, &caps).unwrap();
    assert_eq!(
        mode,
        DeliveryMode::Stdin,
        "explicit Tempfile on stdin-capable binary must degrade to Stdin"
    );
}

#[test]
fn explicit_stdin_request_never_degrades_to_tempfile_or_argv() {
    let caps = DeliveryCaps {
        supports_argv: true,
        supports_tempfile: true,
        supports_stdin: false,
        supports_file_reference: false,
        tempfile_flag: Some("--prompt-file"),
    };

    assert!(
        try_select_mode(PromptDelivery::Stdin, &caps).is_err(),
        "explicit Stdin on a binary without a verified stdin contract must fail; it must not switch prompt roles by falling back to Tempfile, nor use argv"
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
    // Issue #2030 R1.2: `argv` is no longer accepted; any case maps to auto.
    for value in ["argv", "ARGV", "ArGv"] {
        with_env(Some(value), || {
            assert_eq!(
                from_env(),
                PromptDelivery::Auto,
                "env={value} must fall back to Auto"
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
// 11. deliver_never_appends_prompt_for_argv_request (#2030 R1.2)
// ---------------------------------------------------------------------------

#[test]
fn deliver_never_appends_prompt_for_argv_request() {
    let caps = codex_like_caps();
    let prompt = "hello prompt";
    let mut cmd = Command::new("/bin/true");
    cmd.arg("--existing");
    deliver(&mut cmd, prompt, PromptDelivery::Argv, &caps)
        .expect_err("argv-only caps have no safe channel, even for an Argv request");
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert_eq!(args, vec!["--existing".to_string()]);
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
// 14. copilot_like_caps_auto_long_prompt_uses_tempfile
// ---------------------------------------------------------------------------

#[test]
fn copilot_like_caps_auto_long_prompt_uses_tempfile() {
    let caps = copilot_like_caps();
    let mode = try_select_mode(PromptDelivery::Auto, &caps).unwrap();
    assert_eq!(
        mode,
        DeliveryMode::Tempfile,
        "Copilot-like caps (no stdin, has tempfile) should pick Tempfile for 64 KiB"
    );
}

// ---------------------------------------------------------------------------
// Issue #1570: file-reference delivery, capability builders and the extended
// selection table. See docs/reference/large-payload-delivery.md.
// ---------------------------------------------------------------------------

/// Linux `MAX_ARG_STRLEN`: the largest single argv or environment string.
const MAX_ARG_STRLEN: usize = 131_072;

/// A caps value with every channel disabled, built without the non-const
/// constructors so it can seed `const` builder chains.
const NO_CAPS: DeliveryCaps = DeliveryCaps {
    supports_argv: false,
    supports_tempfile: false,
    supports_stdin: false,
    supports_file_reference: false,
    tempfile_flag: None,
};

/// The builders are `const fn`: this only compiles if they are.
const CONST_BUILT_CAPS: DeliveryCaps = NO_CAPS.with_stdin().with_file_reference();

fn caps(argv: bool, tempfile: bool, stdin: bool, file_reference: bool) -> DeliveryCaps {
    DeliveryCaps {
        supports_argv: argv,
        supports_tempfile: tempfile,
        supports_stdin: stdin,
        supports_file_reference: file_reference,
        tempfile_flag: tempfile.then_some("--prompt-file"),
    }
}

/// A prompt of at least `min_bytes` bytes that mixes 1-, 2-, 3- and 4-byte
/// UTF-8 characters with shell metacharacters.
fn multibyte_prompt(min_bytes: usize) -> String {
    let pattern = "ascii 'quoted' \"double\" $HOME `tick` \\ é ü € 中 𝄞 🎉\n";
    let mut out = String::with_capacity(min_bytes + pattern.len());
    while out.len() < min_bytes {
        out.push_str(pattern);
    }
    out
}

fn args_of(cmd: &Command) -> Vec<String> {
    cmd.get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn delivery_mode_has_file_reference_variant() {
    let mode = DeliveryMode::FileReference;
    assert_ne!(mode, DeliveryMode::Argv);
    assert_ne!(mode, DeliveryMode::Tempfile);
    assert_ne!(mode, DeliveryMode::Stdin);
}

#[test]
fn existing_constructors_leave_file_reference_disabled() {
    assert!(!DeliveryCaps::argv_only().supports_file_reference);
    assert!(!DeliveryCaps::argv_and_tempfile("--prompt-file").supports_file_reference);
    assert!(!DeliveryCaps::all_modes("--prompt-file").supports_file_reference);
}

#[test]
fn with_stdin_sets_only_supports_stdin() {
    let base = DeliveryCaps::argv_only();
    let built = base.with_stdin();
    assert_eq!(
        built,
        DeliveryCaps {
            supports_stdin: true,
            ..base
        },
        "with_stdin() must set supports_stdin and leave every other field unchanged"
    );
}

#[test]
fn with_file_reference_sets_only_supports_file_reference() {
    let base = DeliveryCaps::argv_and_tempfile("--prompt-file");
    let built = base.with_file_reference();
    assert_eq!(
        built,
        DeliveryCaps {
            supports_file_reference: true,
            ..base
        },
        "with_file_reference() must set supports_file_reference and leave every other field unchanged"
    );
}

#[test]
fn builders_are_const_and_compose() {
    assert_eq!(
        CONST_BUILT_CAPS,
        DeliveryCaps {
            supports_argv: false,
            supports_tempfile: false,
            supports_stdin: true,
            supports_file_reference: true,
            tempfile_flag: None,
        }
    );
}

#[test]
fn file_reference_template_is_the_documented_fixed_sentence() {
    assert_eq!(
        FILE_REFERENCE_TEMPLATE,
        "Read and follow the complete instructions in the file {path}. Its contents are the task prompt."
    );
    assert_eq!(
        FILE_REFERENCE_TEMPLATE.matches("{path}").count(),
        1,
        "{{path}} must be the only substitution in the template"
    );
    assert!(!FILE_REFERENCE_TEMPLATE.contains('\n'));
}

#[test]
fn file_reference_prefix_is_the_template_text_before_path() {
    let (before, _) = FILE_REFERENCE_TEMPLATE
        .split_once("{path}")
        .expect("template must contain {path}");
    assert_eq!(FILE_REFERENCE_PREFIX, before);
    assert!(!FILE_REFERENCE_PREFIX.is_empty());
}

#[test]
fn auto_selection_order_is_stdin_file_reference_tempfile() {
    // (argv, tempfile, stdin, file_reference) -> expected mode; `None` where
    // no safe channel exists.
    let table = [
        ((true, true, true, true), Some(DeliveryMode::Stdin)),
        ((true, false, true, true), Some(DeliveryMode::Stdin)),
        (
            (true, false, false, true),
            Some(DeliveryMode::FileReference),
        ),
        ((true, false, false, false), None),
        ((false, false, true, true), Some(DeliveryMode::Stdin)),
        (
            (false, false, false, true),
            Some(DeliveryMode::FileReference),
        ),
        ((true, true, false, true), Some(DeliveryMode::FileReference)),
        ((true, true, false, false), Some(DeliveryMode::Tempfile)),
    ];
    for ((argv, tempfile, stdin, file_reference), expected) in table {
        let c = caps(argv, tempfile, stdin, file_reference);
        assert_eq!(
            try_select_mode(PromptDelivery::Auto, &c).ok(),
            expected,
            "auto, caps {c:?}"
        );
    }
}

#[test]
fn auto_moves_small_prompts_off_argv_with_file_reference() {
    let c = DeliveryCaps::argv_only().with_file_reference();
    assert_eq!(
        try_select_mode(PromptDelivery::Auto, &c).unwrap(),
        DeliveryMode::FileReference
    );
}

#[test]
fn auto_small_prompt_without_argv_uses_the_long_form_order() {
    let c = caps(false, false, false, true);
    assert_eq!(
        try_select_mode(PromptDelivery::Auto, &c).unwrap(),
        DeliveryMode::FileReference
    );
}

#[test]
fn explicit_requests_degrade_through_file_reference_before_argv() {
    // (request, caps, expected)
    let table = [
        // argv unsupported: stdin, file-reference, tempfile
        (
            PromptDelivery::Argv,
            caps(false, false, false, true),
            DeliveryMode::FileReference,
        ),
        (
            PromptDelivery::Argv,
            caps(false, false, true, true),
            DeliveryMode::Stdin,
        ),
        (
            PromptDelivery::Argv,
            caps(false, true, true, true),
            DeliveryMode::Stdin,
        ),
        (
            PromptDelivery::Argv,
            caps(false, true, false, false),
            DeliveryMode::Tempfile,
        ),
        // tempfile: stdin, file-reference, then no safe channel
        (
            PromptDelivery::Tempfile,
            caps(true, false, true, true),
            DeliveryMode::Stdin,
        ),
        (
            PromptDelivery::Tempfile,
            caps(true, false, false, true),
            DeliveryMode::FileReference,
        ),
        // stdin: file-reference, then no safe channel
        (
            PromptDelivery::Stdin,
            caps(true, false, false, true),
            DeliveryMode::FileReference,
        ),
        (
            PromptDelivery::Stdin,
            caps(true, true, false, true),
            DeliveryMode::FileReference,
        ),
    ];
    for (requested, c, expected) in table {
        assert_eq!(
            try_select_mode(requested, &c).unwrap(),
            expected,
            "request {requested:?}, caps {c:?}"
        );
    }
    for (requested, c) in [
        (PromptDelivery::Tempfile, caps(true, false, false, false)),
        (PromptDelivery::Stdin, caps(true, true, false, false)),
    ] {
        assert!(
            try_select_mode(requested, &c).is_err(),
            "request {requested:?}, caps {c:?} has no safe channel"
        );
    }
}

#[test]
fn explicit_supported_requests_still_win_over_file_reference() {
    let c = caps(true, true, true, true);
    // #2030 R1.2: an Argv request is Auto, which prefers stdin.
    assert_eq!(
        try_select_mode(PromptDelivery::Argv, &c).unwrap(),
        DeliveryMode::Stdin
    );
    assert_eq!(
        try_select_mode(PromptDelivery::Tempfile, &c).unwrap(),
        DeliveryMode::Tempfile
    );
    assert_eq!(
        try_select_mode(PromptDelivery::Stdin, &c).unwrap(),
        DeliveryMode::Stdin
    );
}

#[cfg(unix)]
#[test]
fn file_reference_delivery_writes_exact_0600_file_and_one_short_argument() {
    let c = DeliveryCaps::argv_only().with_file_reference();
    let prompt = multibyte_prompt(128 * 1024 + 17);
    assert!(prompt.len() > 128 * 1024);
    assert!(prompt.len() > MAX_ARG_STRLEN);

    let mut cmd = Command::new("/bin/true");
    cmd.arg("-p");
    let handle = deliver(&mut cmd, &prompt, PromptDelivery::Auto, &c)
        .expect("file-reference delivery should succeed");

    assert_eq!(handle.mode(), DeliveryMode::FileReference);
    let path = handle
        .tempfile_path()
        .expect("file-reference mode must expose the temporary file path")
        .to_path_buf();
    let path_str = path.to_str().expect("temporary path should be UTF-8");
    assert!(!path_str.contains('\n'), "path must contain no newline");

    let meta = std::fs::metadata(&path).expect("prompt file must exist while the handle lives");
    assert_eq!(
        meta.permissions().mode() & 0o7777,
        0o600,
        "file-reference prompt file must be 0600"
    );
    let written = std::fs::read(&path).expect("prompt file should be readable");
    assert!(
        written == prompt.as_bytes(),
        "prompt file contents must be byte-exact ({} bytes written, {} expected)",
        written.len(),
        prompt.len()
    );

    let args = args_of(&cmd);
    assert_eq!(
        args.len(),
        2,
        "file-reference must append exactly one argument; got {} args",
        args.len()
    );
    assert_eq!(args[0], "-p");
    let reference = &args[1];
    assert!(
        reference.len() < 512,
        "file-reference argument must be under 512 bytes; got {}",
        reference.len()
    );
    assert!(reference.starts_with(FILE_REFERENCE_PREFIX));
    assert!(reference.contains(path_str));
    assert_eq!(
        reference,
        &FILE_REFERENCE_TEMPLATE.replace("{path}", path_str),
        "the argument must be the fixed template with only the path substituted"
    );
    assert!(
        args.iter().all(|a| a.len() < MAX_ARG_STRLEN),
        "no argv element may approach MAX_ARG_STRLEN"
    );
    assert!(
        !args.iter().any(|a| a.contains("ascii 'quoted'")),
        "prompt text must not leak into argv"
    );

    // Security review (#1570): the file sits alone in a private 0700
    // directory, which is removed with the handle.
    let parent = path
        .parent()
        .expect("prompt file has a parent")
        .to_path_buf();
    assert_ne!(
        parent,
        std::env::temp_dir(),
        "must not live in the shared temp root"
    );
    assert_eq!(
        std::fs::metadata(&parent).unwrap().permissions().mode() & 0o7777,
        0o700,
        "file-reference prompt directory must be 0700"
    );
    assert_eq!(std::fs::read_dir(&parent).unwrap().count(), 1);

    drop(handle);
    assert!(
        !path.exists(),
        "file-reference prompt file must be removed when the handle is dropped: {path:?}"
    );
    assert!(
        !parent.exists(),
        "private prompt directory must be removed: {parent:?}"
    );
}

#[test]
fn file_reference_selected_by_degrade_chain_also_writes_file() {
    let c = DeliveryCaps::argv_only().with_file_reference();
    let prompt = multibyte_prompt(8 * 1024);
    let mut cmd = Command::new("/bin/true");
    let handle = deliver(&mut cmd, &prompt, PromptDelivery::Stdin, &c)
        .expect("stdin request on a file-reference binary should degrade, not fail");
    assert_eq!(handle.mode(), DeliveryMode::FileReference);
    let path = handle
        .tempfile_path()
        .expect("file-reference handle must expose a path")
        .to_path_buf();
    assert_eq!(
        std::fs::read(&path).expect("prompt file readable"),
        prompt.as_bytes()
    );
    let args = args_of(&cmd);
    assert_eq!(args.len(), 1);
    assert!(args[0].starts_with(FILE_REFERENCE_PREFIX));
    drop(handle);
    assert!(!path.exists());
}

#[test]
fn tempfile_path_is_none_only_for_stdin_mode() {
    // Issue #2030 R1.2: an `Argv` request is treated as `Auto`, so on a
    // pointer-capable binary it yields a file reference with a real path and
    // the prompt stays out of argv.
    let mut cmd = Command::new("/bin/true");
    let handle = deliver(
        &mut cmd,
        "short",
        PromptDelivery::Argv,
        &DeliveryCaps::argv_only().with_file_reference(),
    )
    .expect("argv request falls back to auto");
    assert_eq!(handle.mode(), DeliveryMode::FileReference);
    assert!(handle.tempfile_path().is_some());
    assert!(args_of(&cmd).iter().all(|a| a != "short"));

    let mut cmd = Command::new("/bin/true");
    let handle = deliver(
        &mut cmd,
        &multibyte_prompt(8 * 1024),
        PromptDelivery::Auto,
        &DeliveryCaps::argv_only().with_stdin().with_file_reference(),
    )
    .expect("stdin delivery");
    assert_eq!(handle.mode(), DeliveryMode::Stdin);
    assert!(handle.tempfile_path().is_none());
}
