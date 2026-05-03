//! Constants, types, and static hook specifications for install/uninstall.

use serde::{Deserialize, Serialize};

/// Upstream archive URL — only used as a network fallback when the
/// bundled framework root cannot be resolved locally (issue #254).
/// Points to amplihack-rs (current canonical repo) per #249.
pub(super) const REPO_ARCHIVE_URL: &str =
    "https://github.com/rysweet/amplihack-rs/archive/refs/heads/main.tar.gz";
/// Upstream git URL — only used as a network fallback (issue #254).
/// Points to amplihack-rs (current canonical repo) per #249.
pub(super) const REPO_GIT_URL: &str = "https://github.com/rysweet/amplihack-rs";
/// Identifies which framework-asset source layout was found in the
/// caller-supplied repository root.
///
/// - [`SourceLayout::Bundle`]: amplihack-rs canonical layout — assets live
///   under `<repo>/amplifier-bundle/`. The top-level `.claude/` is
///   gitignored / absent (issue #416).
/// - [`SourceLayout::LegacyClaude`]: pre-amplihack-rs layout — assets live
///   under `<repo>/.claude/` (or `<repo>/../.claude/` for nested checkouts).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::commands) enum SourceLayout {
    Bundle,
    LegacyClaude,
}

impl SourceLayout {
    /// Stable wire string for the on-disk `.layout` marker. Strict-parsed in
    /// [`super::settings::missing_framework_paths`]; do not localize.
    pub(in crate::commands) fn marker_str(self) -> &'static str {
        match self {
            SourceLayout::Bundle => "bundle",
            SourceLayout::LegacyClaude => "legacy",
        }
    }
}

/// Source→destination mapping for the legacy `.claude/` layout. Identity
/// map of the historical `ESSENTIAL_DIRS`. Preserved for backward compat
/// with installs that still pull from a `.claude/`-rooted source tree.
pub(super) const LEGACY_DIR_MAPPING: &[(&str, &str)] = &[
    ("agents/amplihack", "agents/amplihack"),
    ("commands/amplihack", "commands/amplihack"),
    ("tools/amplihack", "tools/amplihack"),
    ("tools/xpia", "tools/xpia"),
    ("context", "context"),
    ("workflow", "workflow"),
    ("skills", "skills"),
    ("templates", "templates"),
    ("scenarios", "scenarios"),
    ("docs", "docs"),
    ("schemas", "schemas"),
    ("config", "config"),
];

/// Source→destination mapping for the amplihack-rs `amplifier-bundle/`
/// layout. Only directories that actually exist in the bundle are listed
/// (per design D1) — shipping legacy-only essentials would cause an
/// infinite re-install loop because `missing_framework_paths` would
/// report them missing on every boot.
pub(super) const BUNDLE_DIR_MAPPING: &[(&str, &str)] = &[
    ("agents", "agents"),
    ("skills", "skills"),
    ("context", "context"),
    ("tools/amplihack", "tools/amplihack"),
    ("recipes", "recipes"),
    ("behaviors", "behaviors"),
    ("modules", "modules"),
];

/// Bundle-layout categories that are required when present in the source
/// bundle. The current bundle does not ship these top-level directories, but
/// future bundles must not silently drop them if they appear. `tools/xpia`
/// is conditional after Issue #547: the bundled Python hooks were removed,
/// but legacy installs may still have them in destination so the install
/// code's xpia upgrade-detection logic in `settings.rs` and `hooks.rs`
/// remains intact.
pub(super) const SOURCE_CONDITIONAL_BUNDLE_DIR_MAPPING: &[(&str, &str)] = &[
    ("commands", "commands"),
    ("hooks", "hooks"),
    ("tools/xpia", "tools/xpia"),
];

/// Returns the source→destination mapping table for the given layout.
pub(super) fn dir_mapping(layout: SourceLayout) -> &'static [(&'static str, &'static str)] {
    match layout {
        SourceLayout::Bundle => BUNDLE_DIR_MAPPING,
        SourceLayout::LegacyClaude => LEGACY_DIR_MAPPING,
    }
}

/// Destination-relative essential dirs that the layout actually stages.
/// Used by `missing_framework_paths` and the network-fallback hard-error
/// check to verify the staged tree contains every dir the install path
/// promised to copy.
pub(super) fn essential_destinations(layout: SourceLayout) -> &'static [&'static str] {
    // The static slice is built from the destination column at compile time
    // via inline arrays. We mirror the mapping tables here so callers don't
    // pay an allocation on every check.
    match layout {
        SourceLayout::Bundle => &[
            "agents",
            "skills",
            "context",
            "tools/amplihack",
            "recipes",
            "behaviors",
            "modules",
        ],
        SourceLayout::LegacyClaude => &[
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
        ],
    }
}

/// Layout-aware essential files. Bundle layout ships only `statusline.sh`;
/// `AMPLIHACK.md` is absent from the bundle and is not required there.
pub(super) fn essential_files(layout: SourceLayout) -> &'static [&'static str] {
    match layout {
        SourceLayout::Bundle => &["tools/statusline.sh"],
        SourceLayout::LegacyClaude => &["tools/statusline.sh", "AMPLIHACK.md"],
    }
}

/// Legacy alias preserved to minimise churn in tests / external readers
/// that still iterate `ESSENTIAL_DIRS`. Equal to the destination column of
/// `LEGACY_DIR_MAPPING`. New code should use [`essential_destinations`].
#[allow(dead_code)]
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
#[allow(dead_code)]
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
    /// Legacy on-disk filename of the Python shim that historically backed
    /// this hook entry. `None` for events whose `.py` shim was deleted in
    /// favor of a native `amplihack-hooks` subcommand (issue #522). Kept
    /// only as descriptive metadata for drift diagnostics — the install
    /// pipeline does not consult this field to locate or stage hook files.
    pub hook_file: Option<&'static str>,
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
        hook_file: Some("session_start.py"),
        native_subcmd: Some("session-start"),
        timeout: Some(10),
        matcher: None,
    },
    CanonicalHookContractEntry {
        event: "Stop",
        // `stop.py` shim deleted in issue #522 — native `amplihack-hooks stop`
        // is the only implementation.
        hook_file: None,
        native_subcmd: Some("stop"),
        timeout: Some(120),
        matcher: None,
    },
    CanonicalHookContractEntry {
        event: "PreToolUse",
        hook_file: Some("pre_tool_use.py"),
        native_subcmd: Some("pre-tool-use"),
        timeout: None,
        matcher: Some("*"),
    },
    CanonicalHookContractEntry {
        event: "PostToolUse",
        // `post_tool_use.py` shim deleted in issue #522.
        hook_file: None,
        native_subcmd: Some("post-tool-use"),
        timeout: None,
        matcher: Some("*"),
    },
    CanonicalHookContractEntry {
        event: "UserPromptSubmit",
        hook_file: Some("workflow_classification_reminder.py"),
        native_subcmd: Some("workflow-classification-reminder"),
        timeout: Some(5),
        matcher: None,
    },
    CanonicalHookContractEntry {
        event: "UserPromptSubmit",
        // `user_prompt_submit.py` shim deleted in issue #522.
        hook_file: None,
        native_subcmd: Some("user-prompt-submit"),
        timeout: Some(10),
        matcher: None,
    },
    CanonicalHookContractEntry {
        event: "PreCompact",
        hook_file: Some("pre_compact.py"),
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
    /// Default launch tool selected during interactive install (e.g. "claude", "copilot", "codex").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_tool: Option<String>,
    /// Update-check preference selected during interactive install (e.g. "auto-weekly", "disabled").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_check_preference: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Component, Path};

    /// Drift guard (design item #11): no mapping entry — in either layout
    /// table — may contain `..` or any ParentDir component, on either the
    /// source or destination side. Compile-time symbols don't yet exist
    /// (BUNDLE_DIR_MAPPING / LEGACY_DIR_MAPPING land with the fix); this
    /// test fails to compile until they do, which is the desired TDD signal.
    #[test]
    fn dir_mappings_have_no_parent_dir_components() {
        fn assert_no_parent(label: &str, rel: &str) {
            for comp in Path::new(rel).components() {
                assert!(
                    !matches!(comp, Component::ParentDir),
                    "{label} entry `{rel}` must not contain `..` component"
                );
            }
        }
        for (src, dst) in BUNDLE_DIR_MAPPING {
            assert_no_parent("BUNDLE_DIR_MAPPING.src", src);
            assert_no_parent("BUNDLE_DIR_MAPPING.dst", dst);
        }
        for (src, dst) in LEGACY_DIR_MAPPING {
            assert_no_parent("LEGACY_DIR_MAPPING.src", src);
            assert_no_parent("LEGACY_DIR_MAPPING.dst", dst);
        }
    }

    /// The `essential_destinations` helper must agree with the destination
    /// column of the active layout's mapping table.
    #[test]
    fn essential_destinations_match_mapping_dst_columns() {
        let bundle: Vec<&str> = BUNDLE_DIR_MAPPING.iter().map(|(_, d)| *d).collect();
        let legacy: Vec<&str> = LEGACY_DIR_MAPPING.iter().map(|(_, d)| *d).collect();
        assert_eq!(
            essential_destinations(SourceLayout::Bundle),
            bundle.as_slice()
        );
        assert_eq!(
            essential_destinations(SourceLayout::LegacyClaude),
            legacy.as_slice()
        );
    }
}
