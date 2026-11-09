//! Issue #2030 (red phase): the prompt-delivery policy never puts a prompt in
//! argv unless the user asked for it by name.
//!
//! Spec: `docs/reference/agent-prompt-channels.md`, sections "Delivery policy"
//! and "Errors". The error strings asserted here are the spec; a wording change
//! edits that table and these tests together.
//!
//! Run with:
//!     cargo test -p amplihack-utils --test prompt_delivery_issue_2030

use std::io::ErrorKind;
use std::process::Command;

use amplihack_utils::prompt_delivery::{
    DeliveryCaps, DeliveryMode, FILE_REFERENCE_PREFIX, NoSafeChannel, PromptDelivery, deliver,
    try_select_mode,
};

/// Contract sentinel: a phrase no real prompt contains. If it ever reaches
/// argv, a `pkill -f` on it would match the process — the #2030 incident.
const SENTINEL: &str = "SENTINEL-2030-tail-f-dev-null";

fn argv(cmd: &Command) -> Vec<String> {
    cmd.get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

/// Every combination of the four channel booleans, with and without a
/// tempfile flag. 32 rows.
fn all_caps() -> Vec<DeliveryCaps> {
    let mut out = Vec::new();
    for bits in 0u8..16 {
        for flag in [None, Some("--prompt-file")] {
            out.push(DeliveryCaps {
                supports_argv: bits & 1 != 0,
                supports_tempfile: bits & 2 != 0,
                supports_stdin: bits & 4 != 0,
                supports_file_reference: bits & 8 != 0,
                tempfile_flag: flag,
            });
        }
    }
    out
}

fn has_safe_channel(caps: &DeliveryCaps) -> bool {
    caps.supports_stdin || caps.supports_file_reference || caps.supports_tempfile
}

// ---------------------------------------------------------------------------
// DeliveryCaps::default()
// ---------------------------------------------------------------------------

#[test]
fn default_caps_have_no_channels() {
    let caps = DeliveryCaps::default();
    assert!(!caps.supports_argv);
    assert!(!caps.supports_tempfile);
    assert!(!caps.supports_stdin);
    assert!(!caps.supports_file_reference);
    assert_eq!(caps.tempfile_flag, None);
}

// ---------------------------------------------------------------------------
// try_select_mode: Auto
// ---------------------------------------------------------------------------

#[test]
fn try_select_mode_auto_never_argv() {
    // Exhaustive over caps: Auto must never answer Argv. The selector takes
    // no prompt size, so no size can move a prompt onto the command line.
    for caps in all_caps() {
        match try_select_mode(PromptDelivery::Auto, &caps) {
            Ok(mode) => {
                assert_ne!(
                    mode,
                    DeliveryMode::Argv,
                    "Auto chose argv for caps {caps:?}"
                );
                assert!(has_safe_channel(&caps), "Ok({mode:?}) with no safe channel");
            }
            Err(e) => {
                assert!(
                    !has_safe_channel(&caps),
                    "Auto failed although caps {caps:?} offer a safe channel"
                );
                assert_eq!(e.requested, PromptDelivery::Auto);
                assert_eq!(e.caps, caps);
            }
        }
    }
}

#[test]
fn auto_prefers_stdin_then_file_reference_then_tempfile() {
    let every = DeliveryCaps {
        supports_argv: true,
        supports_tempfile: true,
        supports_stdin: true,
        supports_file_reference: true,
        tempfile_flag: Some("--prompt-file"),
    };
    assert_eq!(
        try_select_mode(PromptDelivery::Auto, &every).unwrap(),
        DeliveryMode::Stdin
    );
    let no_stdin = DeliveryCaps {
        supports_stdin: false,
        ..every
    };
    assert_eq!(
        try_select_mode(PromptDelivery::Auto, &no_stdin).unwrap(),
        DeliveryMode::FileReference
    );
    let tempfile_only = DeliveryCaps {
        supports_stdin: false,
        supports_file_reference: false,
        ..every
    };
    assert_eq!(
        try_select_mode(PromptDelivery::Auto, &tempfile_only).unwrap(),
        DeliveryMode::Tempfile
    );
}

#[test]
fn auto_small_prompt_with_argv_and_stdin_uses_stdin() {
    // Before #2030 a 10-byte prompt went on argv. Claude's caps.
    let caps = DeliveryCaps::argv_only().with_stdin();
    assert_eq!(
        try_select_mode(PromptDelivery::Auto, &caps).unwrap(),
        DeliveryMode::Stdin
    );
}

#[test]
fn auto_argv_only_caps_are_an_error() {
    let err = try_select_mode(PromptDelivery::Auto, &DeliveryCaps::argv_only())
        .expect_err("argv-only caps have no safe channel");
    assert_eq!(err.requested, PromptDelivery::Auto);
    assert_eq!(err.caps, DeliveryCaps::argv_only());
}

// ---------------------------------------------------------------------------
// try_select_mode: explicit requests
// ---------------------------------------------------------------------------

#[test]
fn explicit_argv_request_is_treated_as_auto() {
    // R1.2: `AMPLIHACK_PROMPT_DELIVERY=argv` is inherited by every nested
    // step, so honouring it would put prompts back within reach of
    // `pkill -f`. A `PromptDelivery::Argv` request picks exactly what `Auto`
    // picks, for every capability set.
    for caps in all_caps() {
        let as_argv = try_select_mode(PromptDelivery::Argv, &caps).ok();
        let as_auto = try_select_mode(PromptDelivery::Auto, &caps).ok();
        assert_eq!(as_argv, as_auto, "Argv request with {caps:?}");
    }
}

#[test]
fn explicit_argv_request_on_an_argv_capable_binary_uses_stdin() {
    // Before R1.2 this returned `DeliveryMode::Argv`.
    let caps = DeliveryCaps::argv_only().with_stdin();
    assert_eq!(
        try_select_mode(PromptDelivery::Argv, &caps).unwrap(),
        DeliveryMode::Stdin
    );
}

#[test]
fn explicit_argv_request_on_an_argv_only_binary_is_no_safe_channel() {
    try_select_mode(PromptDelivery::Argv, &DeliveryCaps::argv_only())
        .expect_err("argv is never a delivery channel, even when asked for by name");
}

#[test]
fn explicit_argv_unsupported_degrades_through_safe_channels_only() {
    let caps = DeliveryCaps {
        supports_argv: false,
        ..DeliveryCaps::default().with_file_reference()
    };
    assert_eq!(
        try_select_mode(PromptDelivery::Argv, &caps).unwrap(),
        DeliveryMode::FileReference
    );
    assert!(try_select_mode(PromptDelivery::Argv, &DeliveryCaps::default()).is_err());
}

#[test]
fn explicit_stdin_degrades_to_file_reference_never_argv() {
    let pointer = DeliveryCaps::argv_only().with_file_reference();
    assert_eq!(
        try_select_mode(PromptDelivery::Stdin, &pointer).unwrap(),
        DeliveryMode::FileReference
    );
    // Stdin never degrades to tempfile: the two have different process
    // contracts.
    let err = try_select_mode(
        PromptDelivery::Stdin,
        &DeliveryCaps::argv_and_tempfile("--prompt-file"),
    )
    .expect_err("stdin request must not degrade to tempfile or argv");
    assert_eq!(err.requested, PromptDelivery::Stdin);
}

#[test]
fn explicit_tempfile_degrades_to_stdin_then_pointer_never_argv() {
    assert_eq!(
        try_select_mode(
            PromptDelivery::Tempfile,
            &DeliveryCaps::argv_only().with_stdin()
        )
        .unwrap(),
        DeliveryMode::Stdin
    );
    assert_eq!(
        try_select_mode(
            PromptDelivery::Tempfile,
            &DeliveryCaps::argv_only().with_file_reference()
        )
        .unwrap(),
        DeliveryMode::FileReference
    );
    assert!(try_select_mode(PromptDelivery::Tempfile, &DeliveryCaps::argv_only()).is_err());
}

#[test]
fn no_request_ever_yields_argv() {
    // R1.2: not even an explicit `Argv` request.
    for caps in all_caps() {
        for requested in [
            PromptDelivery::Auto,
            PromptDelivery::Argv,
            PromptDelivery::Stdin,
            PromptDelivery::Tempfile,
        ] {
            if let Ok(mode) = try_select_mode(requested, &caps) {
                assert_ne!(mode, DeliveryMode::Argv, "{requested:?} with {caps:?}");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// NoSafeChannel
// ---------------------------------------------------------------------------

#[test]
fn no_safe_channel_message_is_stable() {
    let err = try_select_mode(PromptDelivery::Auto, &DeliveryCaps::default()).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.starts_with("no channel keeps this prompt out of the command line: "),
        "message changed; update docs/reference/agent-prompt-channels.md#errors with it: {msg:?}"
    );
    // R1.2: argv is no longer an opt-in, so the message must not offer it.
    assert!(
        !msg.contains("AMPLIHACK_PROMPT_DELIVERY=argv") && !msg.contains("=argv"),
        "the message must not suggest the removed argv opt-in: {msg:?}"
    );
    assert_eq!(
        msg,
        "no channel keeps this prompt out of the command line: the agent CLI \
         accepts none of stdin, a prompt file or a file reference",
        "keep in step with docs/reference/agent-prompt-channels.md#errors"
    );
}

#[test]
fn no_safe_channel_converts_to_invalid_input() {
    let err = NoSafeChannel {
        requested: PromptDelivery::Auto,
        caps: DeliveryCaps::default(),
    };
    let io: std::io::Error = err.into();
    assert_eq!(io.kind(), ErrorKind::InvalidInput);
    assert!(
        io.to_string()
            .starts_with("no channel keeps this prompt out of the command line: ")
    );
}

// ---------------------------------------------------------------------------
// deliver
// ---------------------------------------------------------------------------

#[test]
fn deliver_auto_small_prompt_never_lands_in_argv() {
    let mut cmd = Command::new("claude");
    cmd.arg("-p");
    let handle = deliver(
        &mut cmd,
        SENTINEL,
        PromptDelivery::Auto,
        &DeliveryCaps::argv_only().with_stdin(),
    )
    .expect("stdin is available");
    assert_eq!(handle.mode(), DeliveryMode::Stdin);
    assert_eq!(argv(&cmd), vec!["-p".to_string()]);
}

#[test]
fn deliver_with_no_safe_channel_errors_and_leaves_argv_alone() {
    let mut cmd = Command::new("mystery");
    cmd.arg("--print");
    let err = deliver(
        &mut cmd,
        SENTINEL,
        PromptDelivery::Auto,
        &DeliveryCaps::argv_only(),
    )
    .expect_err("no safe channel");
    assert_eq!(err.kind(), ErrorKind::InvalidInput);
    assert!(
        !err.to_string().contains(SENTINEL),
        "errors never echo the prompt"
    );
    assert_eq!(argv(&cmd), vec!["--print".to_string()]);
}

#[cfg(unix)]
#[test]
fn deliver_file_reference_is_0600_in_0700_dir_and_removed_on_drop() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let mut cmd = Command::new("copilot");
    let handle = deliver(
        &mut cmd,
        SENTINEL,
        PromptDelivery::Auto,
        &DeliveryCaps::argv_only().with_file_reference(),
    )
    .unwrap();
    assert_eq!(handle.mode(), DeliveryMode::FileReference);
    let args = argv(&cmd);
    assert_eq!(args.len(), 1);
    assert!(args[0].starts_with(FILE_REFERENCE_PREFIX));
    assert!(!args[0].contains(SENTINEL));

    let path = handle.tempfile_path().unwrap().to_path_buf();
    let meta = std::fs::symlink_metadata(&path).unwrap();
    assert!(meta.file_type().is_file());
    assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    assert_eq!(meta.uid(), unsafe { libc::geteuid() });
    let dir = path.parent().unwrap().to_path_buf();
    assert_eq!(
        std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), SENTINEL);

    drop(handle);
    assert!(!path.exists(), "the prompt file is removed with the handle");
    assert!(
        !dir.exists(),
        "the private directory is removed with the handle"
    );
}

// ---------------------------------------------------------------------------
// R1.2: `AMPLIHACK_PROMPT_DELIVERY=argv` is not an accepted value
// ---------------------------------------------------------------------------

#[test]
fn deliver_argv_request_never_appends_the_prompt() {
    // Claude's caps: argv and stdin. An Argv request goes to stdin.
    let mut cmd = Command::new("claude");
    cmd.arg("-p");
    let handle = deliver(
        &mut cmd,
        SENTINEL,
        PromptDelivery::Argv,
        &DeliveryCaps::argv_only().with_stdin(),
    )
    .expect("stdin is available");
    assert_eq!(handle.mode(), DeliveryMode::Stdin);
    assert_eq!(argv(&cmd), vec!["-p".to_string()]);
}

#[test]
fn deliver_argv_request_on_argv_only_binary_errors_without_touching_argv() {
    let mut cmd = Command::new("mystery");
    cmd.arg("--existing");
    let err = deliver(
        &mut cmd,
        SENTINEL,
        PromptDelivery::Argv,
        &DeliveryCaps::argv_only(),
    )
    .expect_err("R5.3: NoSafeChannel, never argv");
    assert_eq!(err.kind(), ErrorKind::InvalidInput);
    assert!(!err.to_string().contains(SENTINEL));
    assert_eq!(argv(&cmd), vec!["--existing".to_string()]);
}

#[test]
fn deliver_never_builds_argv_delivery_for_any_request_or_caps() {
    for caps in all_caps() {
        for requested in [
            PromptDelivery::Auto,
            PromptDelivery::Argv,
            PromptDelivery::Stdin,
            PromptDelivery::Tempfile,
        ] {
            let mut cmd = Command::new("agent");
            if let Ok(handle) = deliver(&mut cmd, SENTINEL, requested, &caps) {
                assert_ne!(handle.mode(), DeliveryMode::Argv, "{requested:?} {caps:?}");
            }
            for a in argv(&cmd) {
                assert!(
                    !a.contains(SENTINEL),
                    "{requested:?} {caps:?} put the prompt in argv: {a:?}"
                );
            }
        }
    }
}

/// Serialise env mutation within this test binary.
fn with_delivery_env<R>(value: Option<&str>, f: impl FnOnce() -> R) -> R {
    use amplihack_utils::prompt_delivery::ENV_VAR_NAME;
    use std::sync::{Mutex, OnceLock};
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let _g = LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let previous = std::env::var(ENV_VAR_NAME).ok();
    // SAFETY: serialised by LOCK; no other test in this binary reads the env.
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

#[test]
fn from_env_maps_argv_to_auto_in_any_case() {
    use amplihack_utils::prompt_delivery::from_env;
    for value in ["argv", "ARGV", "ArGv", "  argv  "] {
        with_delivery_env(Some(value), || {
            assert_eq!(
                from_env(),
                PromptDelivery::Auto,
                "AMPLIHACK_PROMPT_DELIVERY={value:?} must resolve to auto"
            );
        });
    }
}

#[test]
fn from_env_never_returns_argv() {
    use amplihack_utils::prompt_delivery::from_env;
    for value in [
        "",
        "auto",
        "argv",
        "tempfile",
        "stdin",
        "file-reference",
        "bogus",
    ] {
        with_delivery_env(Some(value), || {
            assert_ne!(from_env(), PromptDelivery::Argv, "value {value:?}");
        });
    }
    with_delivery_env(None, || assert_eq!(from_env(), PromptDelivery::Auto));
}

#[test]
fn env_argv_end_to_end_keeps_the_prompt_out_of_argv() {
    use amplihack_utils::prompt_delivery::from_env;
    with_delivery_env(Some("argv"), || {
        let mut cmd = Command::new("claude");
        cmd.arg("-p");
        let handle = deliver(
            &mut cmd,
            SENTINEL,
            from_env(),
            &DeliveryCaps::argv_only().with_stdin(),
        )
        .expect("stdin is available");
        assert_eq!(handle.mode(), DeliveryMode::Stdin);
        assert!(argv(&cmd).iter().all(|a| !a.contains(SENTINEL)));
    });
}

/// The `tracing::warn!` text for an unrecognised value is the spec in
/// docs/reference/agent-prompt-channels.md ("Automatic prompt delivery never
/// selects argv"). It must not list `argv` as valid.
#[test]
fn unrecognised_value_warning_does_not_list_argv() {
    let src = include_str!("../src/prompt_delivery.rs");
    let from_env = &src[src.find("pub fn from_env(").expect("from_env exists")..];
    let body = &from_env[..from_env.find("\n}\n").unwrap_or(from_env.len())];
    assert!(
        !body.contains("\"argv\" =>"),
        "from_env must not accept \"argv\" (R1.2):\n{body}"
    );
    let flat: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        flat.contains("Valid values: auto, tempfile, stdin (case-insensitive)."),
        "warning text must match the reference page:\n{body}"
    );
    assert!(!flat.contains("auto, argv"), "{body}");
}
