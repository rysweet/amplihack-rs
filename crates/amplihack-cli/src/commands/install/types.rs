//! Constants, types, and static hook specifications for install/uninstall.

use serde::{Deserialize, Serialize};

pub(super) const REPO_ARCHIVE_URL: &str =
    "https://github.com/rysweet/amplihack-rs/archive/refs/heads/main.tar.gz";
pub(super) const REPO_GIT_URL: &str = "https://github.com/rysweet/amplihack-rs";
pub(super) const ESSENTIAL_DIRS: &[&str] = &[
    "agents/amplihack",
    "commands/amplihack",
    "tools/amplihack",
    "tools/xpia",
    "context",
    "workflow",
    "skills",
    "templates",
    "scenarios",
    "docs",
    "schemas",
    "config",
];
pub(super) const ESSENTIAL_FILES: &[&str] = &["tools/statusline.sh", "AMPLIHACK.md"];
pub(super) const RUNTIME_DIRS: &[&str] = &[
    "runtime",
    "runtime/logs",
    "runtime/metrics",
    "runtime/security",
    "runtime/analysis",
];
pub(super) const XPIA_HOOK_FILES: &[&str] =
    &["session_start.py", "post_tool_use.py", "pre_tool_use.py"];

/// Discriminates between hook command styles.
#[derive(Clone)]
pub(super) enum HookCommandKind {
    /// Invokes the amplihack-hooks binary with a specific subcommand.
    BinarySubcmd { subcmd: &'static str },
}

#[derive(Clone)]
pub(super) struct HookSpec {
    pub event: &'static str,
    pub cmd: HookCommandKind,
    pub timeout: Option<u64>,
    pub matcher: Option<&'static str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CanonicalHookContractEntry {
    pub event: &'static str,
    pub hook_file: &'static str,
    pub native_subcmd: Option<&'static str>,
    pub timeout: Option<u64>,
    pub matcher: Option<&'static str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ObservedNativeHookContractEntry {
    pub subcmd: String,
    pub timeout: Option<u64>,
    pub matcher: Option<String>,
}

// Mirrors the canonical Python hook files plus the native Rust install contract.
// UserPromptSubmit is intentionally launcher-ordered so the lightweight
// workflow-classification reminder runs before user-prompt-submit.
pub(super) const CANONICAL_AMPLIHACK_HOOK_CONTRACT: &[CanonicalHookContractEntry] = &[
    CanonicalHookContractEntry {
        event: "SessionStart",
        hook_file: "session_start.py",
        native_subcmd: Some("session-start"),
        timeout: Some(10),
        matcher: None,
    },
    CanonicalHookContractEntry {
        event: "Stop",
        hook_file: "stop.py",
        native_subcmd: Some("stop"),
        timeout: Some(120),
        matcher: None,
    },
    CanonicalHookContractEntry {
        event: "PreToolUse",
        hook_file: "pre_tool_use.py",
        native_subcmd: Some("pre-tool-use"),
        timeout: None,
        matcher: Some("*"),
    },
    CanonicalHookContractEntry {
        event: "PostToolUse",
        hook_file: "post_tool_use.py",
        native_subcmd: Some("post-tool-use"),
        timeout: None,
        matcher: Some("*"),
    },
    CanonicalHookContractEntry {
        event: "UserPromptSubmit",
        hook_file: "workflow_classification_reminder.py",
        native_subcmd: Some("workflow-classification-reminder"),
        timeout: Some(5),
        matcher: None,
    },
    CanonicalHookContractEntry {
        event: "UserPromptSubmit",
        hook_file: "user_prompt_submit.py",
        native_subcmd: Some("user-prompt-submit"),
        timeout: Some(10),
        matcher: None,
    },
    CanonicalHookContractEntry {
        event: "PreCompact",
        hook_file: "pre_compact.py",
        native_subcmd: Some("pre-compact"),
        timeout: Some(30),
        matcher: None,
    },
];

pub(super) const AMPLIHACK_HOOK_SPECS: &[HookSpec] = &[
    HookSpec {
        event: "SessionStart",
        cmd: HookCommandKind::BinarySubcmd {
            subcmd: "session-start",
        },
        timeout: Some(10),
        matcher: None,
    },
    HookSpec {
        event: "Stop",
        cmd: HookCommandKind::BinarySubcmd { subcmd: "stop" },
        timeout: Some(120),
        matcher: None,
    },
    HookSpec {
        event: "PreToolUse",
        cmd: HookCommandKind::BinarySubcmd {
            subcmd: "pre-tool-use",
        },
        timeout: None,
        matcher: Some("*"),
    },
    HookSpec {
        event: "PostToolUse",
        cmd: HookCommandKind::BinarySubcmd {
            subcmd: "post-tool-use",
        },
        timeout: None,
        matcher: Some("*"),
    },
    // Classification reminder must come BEFORE user-prompt-submit so the
    // topic-boundary routing guidance is injected first.
    HookSpec {
        event: "UserPromptSubmit",
        cmd: HookCommandKind::BinarySubcmd {
            subcmd: "workflow-classification-reminder",
        },
        timeout: Some(5),
        matcher: None,
    },
    HookSpec {
        event: "UserPromptSubmit",
        cmd: HookCommandKind::BinarySubcmd {
            subcmd: "user-prompt-submit",
        },
        timeout: Some(10),
        matcher: None,
    },
    HookSpec {
        event: "PreCompact",
        cmd: HookCommandKind::BinarySubcmd {
            subcmd: "pre-compact",
        },
        timeout: Some(30),
        matcher: None,
    },
];

pub(super) const XPIA_HOOK_SPECS: &[HookSpec] = &[
    HookSpec {
        event: "SessionStart",
        cmd: HookCommandKind::BinarySubcmd {
            subcmd: "session-start",
        },
        timeout: Some(10),
        matcher: None,
    },
    HookSpec {
        event: "PreToolUse",
        cmd: HookCommandKind::BinarySubcmd {
            subcmd: "pre-tool-use",
        },
        timeout: None,
        matcher: Some("*"),
    },
    HookSpec {
        event: "PostToolUse",
        cmd: HookCommandKind::BinarySubcmd {
            subcmd: "post-tool-use",
        },
        timeout: None,
        matcher: Some("*"),
    },
];

#[derive(Debug, Default, Serialize, Deserialize)]
pub(super) struct InstallManifest {
    pub files: Vec<String>,
    pub dirs: Vec<String>,
    #[serde(default)]
    pub binaries: Vec<String>,
    #[serde(default)]
    pub hook_registrations: Vec<String>,
}
