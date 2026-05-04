//! R4: Single source-of-truth flag matrix for agent binary nested-flag deltas.
//!
//! Maps per-binary capabilities and flags so that command builders can consult
//! a canonical matrix instead of scattering flag knowledge across modules.

// ---------------------------------------------------------------------------
// Agent binary enum
// ---------------------------------------------------------------------------

/// The supported agent binaries that amplihack can launch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AgentBinary {
    Claude,
    Copilot,
    Codex,
}

impl AgentBinary {
    /// Returns the env var value used for `AMPLIHACK_AGENT_BINARY`.
    pub fn env_value(&self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Copilot => "copilot",
            Self::Codex => "codex",
        }
    }
}

impl std::fmt::Display for AgentBinary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.env_value())
    }
}

// ---------------------------------------------------------------------------
// FlagSet — the canonical flag collection for a binary
// ---------------------------------------------------------------------------

/// Canonical set of flags and capabilities for an agent binary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlagSet {
    /// Binary type this flag set is for.
    pub binary: AgentBinary,
    /// Supports `--append-system-prompt <path>`.
    pub supports_append_prompt: bool,
    /// Supports `--add-dir <path>`.
    pub supports_add_dir: bool,
    /// Supports `--model <name>`.
    pub supports_model: bool,
    /// Supports `--dangerously-skip-permissions`.
    pub supports_skip_permissions: bool,
    /// Supports `--resume` / `--continue`.
    pub supports_resume: bool,
    /// Supports `--print` (non-interactive mode).
    pub supports_print: bool,
    /// Supports `--allow-all-tools` (Copilot-specific).
    pub supports_allow_all_tools: bool,
    /// Supports `--remote` (Copilot-specific — offload to GitHub cloud).
    pub supports_remote: bool,
    /// The env var name set to identify the agent binary in nested sessions.
    pub agent_binary_env: &'static str,
}

// ---------------------------------------------------------------------------
// Matrix lookup
// ---------------------------------------------------------------------------

/// Return the canonical `FlagSet` for a given agent binary.
pub fn flags_for(binary: AgentBinary) -> FlagSet {
    match binary {
        AgentBinary::Claude => FlagSet {
            binary: AgentBinary::Claude,
            supports_append_prompt: true,
            supports_add_dir: true,
            supports_model: true,
            supports_skip_permissions: true,
            supports_resume: true,
            supports_print: true,
            supports_allow_all_tools: false,
            supports_remote: false,
            agent_binary_env: "AMPLIHACK_AGENT_BINARY",
        },
        AgentBinary::Copilot => FlagSet {
            binary: AgentBinary::Copilot,
            supports_append_prompt: false,
            supports_add_dir: false,
            supports_model: true,
            supports_skip_permissions: false,
            supports_resume: false,
            supports_print: false,
            supports_allow_all_tools: true,
            supports_remote: true,
            agent_binary_env: "AMPLIHACK_AGENT_BINARY",
        },
        AgentBinary::Codex => FlagSet {
            binary: AgentBinary::Codex,
            supports_append_prompt: false,
            supports_add_dir: false,
            supports_model: false,
            supports_skip_permissions: false,
            supports_resume: false,
            supports_print: false,
            supports_allow_all_tools: false,
            supports_remote: false,
            agent_binary_env: "AMPLIHACK_AGENT_BINARY",
        },
    }
}

/// All known agent binaries.
pub const ALL_BINARIES: &[AgentBinary] = &[
    AgentBinary::Claude,
    AgentBinary::Copilot,
    AgentBinary::Codex,
];

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Basic matrix correctness
    // -----------------------------------------------------------------------

    #[test]
    fn claude_supports_append_prompt() {
        let flags = flags_for(AgentBinary::Claude);
        assert!(flags.supports_append_prompt);
    }

    #[test]
    fn claude_supports_add_dir() {
        let flags = flags_for(AgentBinary::Claude);
        assert!(flags.supports_add_dir);
    }

    #[test]
    fn claude_supports_print() {
        let flags = flags_for(AgentBinary::Claude);
        assert!(flags.supports_print);
    }

    #[test]
    fn claude_does_not_support_allow_all_tools() {
        let flags = flags_for(AgentBinary::Claude);
        assert!(!flags.supports_allow_all_tools);
    }

    #[test]
    fn copilot_supports_allow_all_tools() {
        let flags = flags_for(AgentBinary::Copilot);
        assert!(flags.supports_allow_all_tools);
    }

    #[test]
    fn copilot_supports_remote() {
        let flags = flags_for(AgentBinary::Copilot);
        assert!(flags.supports_remote);
    }

    #[test]
    fn claude_does_not_support_remote() {
        let flags = flags_for(AgentBinary::Claude);
        assert!(!flags.supports_remote);
    }

    #[test]
    fn codex_does_not_support_remote() {
        let flags = flags_for(AgentBinary::Codex);
        assert!(!flags.supports_remote);
    }

    #[test]
    fn copilot_does_not_support_append_prompt() {
        let flags = flags_for(AgentBinary::Copilot);
        assert!(!flags.supports_append_prompt);
    }

    #[test]
    fn copilot_does_not_support_print() {
        let flags = flags_for(AgentBinary::Copilot);
        assert!(!flags.supports_print);
    }

    #[test]
    fn codex_is_minimal_flags() {
        let flags = flags_for(AgentBinary::Codex);
        assert!(!flags.supports_append_prompt);
        assert!(!flags.supports_add_dir);
        assert!(!flags.supports_model);
        assert!(!flags.supports_skip_permissions);
        assert!(!flags.supports_resume);
        assert!(!flags.supports_print);
        assert!(!flags.supports_allow_all_tools);
        assert!(!flags.supports_remote);
    }

    // -----------------------------------------------------------------------
    // Agent binary env value
    // -----------------------------------------------------------------------

    #[test]
    fn all_binaries_share_same_env_var_name() {
        for binary in ALL_BINARIES {
            let flags = flags_for(*binary);
            assert_eq!(flags.agent_binary_env, "AMPLIHACK_AGENT_BINARY");
        }
    }

    #[test]
    fn agent_binary_env_values_are_distinct() {
        let values: Vec<&str> = ALL_BINARIES.iter().map(|b| b.env_value()).collect();
        let unique: std::collections::HashSet<&&str> = values.iter().collect();
        assert_eq!(values.len(), unique.len(), "env values must be unique");
    }

    #[test]
    fn claude_env_value_is_claude() {
        assert_eq!(AgentBinary::Claude.env_value(), "claude");
    }

    #[test]
    fn copilot_env_value_is_copilot() {
        assert_eq!(AgentBinary::Copilot.env_value(), "copilot");
    }

    #[test]
    fn codex_env_value_is_codex() {
        assert_eq!(AgentBinary::Codex.env_value(), "codex");
    }

    // -----------------------------------------------------------------------
    // Matrix consistency — every binary returns the correct binary field
    // -----------------------------------------------------------------------

    #[test]
    fn flagset_binary_field_matches_input() {
        for binary in ALL_BINARIES {
            let flags = flags_for(*binary);
            assert_eq!(flags.binary, *binary);
        }
    }

    // -----------------------------------------------------------------------
    // Nested flag propagation assertions
    // -----------------------------------------------------------------------

    #[test]
    fn claude_nested_flags_include_resume() {
        let flags = flags_for(AgentBinary::Claude);
        assert!(
            flags.supports_resume,
            "Claude must support --resume for session continuation"
        );
    }

    #[test]
    fn claude_nested_flags_include_model() {
        let flags = flags_for(AgentBinary::Claude);
        assert!(
            flags.supports_model,
            "Claude must support --model for model selection"
        );
    }

    #[test]
    fn copilot_nested_flags_include_model() {
        let flags = flags_for(AgentBinary::Copilot);
        assert!(flags.supports_model, "Copilot must support --model");
    }

    #[test]
    fn claude_nested_flags_include_skip_permissions() {
        let flags = flags_for(AgentBinary::Claude);
        assert!(flags.supports_skip_permissions);
    }

    // -----------------------------------------------------------------------
    // Display / formatting
    // -----------------------------------------------------------------------

    #[test]
    fn agent_binary_display() {
        assert_eq!(format!("{}", AgentBinary::Claude), "claude");
        assert_eq!(format!("{}", AgentBinary::Copilot), "copilot");
        assert_eq!(format!("{}", AgentBinary::Codex), "codex");
    }

    // -----------------------------------------------------------------------
    // ALL_BINARIES completeness
    // -----------------------------------------------------------------------

    #[test]
    fn all_binaries_contains_three_variants() {
        assert_eq!(ALL_BINARIES.len(), 3);
    }
}
