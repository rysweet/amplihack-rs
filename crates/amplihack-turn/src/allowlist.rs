//! Scoped Copilot tool allowlist for the driven agent.
//!
//! An accepted inbound Signal message is equivalent to typing into the agent,
//! so the chat is **least-privilege by default**: with no explicit
//! `--allow-tool`, the driven `copilot` gets only read-only investigation tools
//! ([`ToolAllowlist::read_only_default`]). Broader access is granted only when
//! the operator lists specific tools, and blanket access
//! (`--allow-all-tools`) requires the explicit `--dangerous-all-tools` opt-in.
//!
//! Crucially, "dangerous" mode maps to Copilot's tools-only escape hatch
//! `--allow-all-tools`, **never** the wider `--allow-all` (which would also
//! grant unrestricted paths/URLs).

/// The read-only investigation tools granted by default.
const READ_ONLY_TOOLS: &[&str] = &["view", "grep", "glob"];

/// An effective Copilot tool allowlist for one chat session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolAllowlist {
    /// When `true`, render `--allow-all-tools` and ignore `tools`.
    dangerous: bool,
    /// The scoped tool list (only meaningful when not `dangerous`).
    tools: Vec<String>,
}

impl ToolAllowlist {
    /// The least-privilege default: read-only `view` / `grep` / `glob`.
    #[must_use]
    pub fn read_only_default() -> Self {
        Self {
            dangerous: false,
            tools: READ_ONLY_TOOLS.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    /// Resolve an allowlist from CLI flags.
    ///
    /// Precedence:
    /// - `dangerous == true` ⇒ blanket [`--allow-all-tools`], regardless of
    ///   `flags`.
    /// - non-empty `flags` ⇒ exactly those scoped tools, in order.
    /// - empty `flags` ⇒ the least-privilege [`read_only_default`].
    ///
    /// [`--allow-all-tools`]: ToolAllowlist::to_copilot_args
    /// [`read_only_default`]: ToolAllowlist::read_only_default
    #[must_use]
    pub fn from_flags(flags: &[String], dangerous: bool) -> Self {
        if dangerous {
            Self {
                dangerous: true,
                tools: Vec::new(),
            }
        } else if flags.is_empty() {
            Self::read_only_default()
        } else {
            Self {
                dangerous: false,
                tools: flags.to_vec(),
            }
        }
    }

    /// Whether this allowlist grants blanket tool access.
    #[must_use]
    pub fn is_dangerous(&self) -> bool {
        self.dangerous
    }

    /// Render the allowlist as Copilot CLI arguments.
    ///
    /// Dangerous mode is a single `--allow-all-tools`; otherwise each tool is a
    /// repeated `--allow-tool <TOOL>` pair, preserving order.
    #[must_use]
    pub fn to_copilot_args(&self) -> Vec<String> {
        if self.dangerous {
            return vec!["--allow-all-tools".to_string()];
        }
        let mut args = Vec::with_capacity(self.tools.len() * 2);
        for tool in &self.tools {
            args.push("--allow-tool".to_string());
            args.push(tool.clone());
        }
        args
    }

    /// A human-readable, single-line description of the effective blast radius,
    /// posted in the group's first announcement message.
    #[must_use]
    pub fn describe(&self) -> String {
        if self.dangerous {
            "ALL TOOLS (dangerous: --allow-all-tools)".to_string()
        } else {
            format!("read-only/scoped tools: {}", self.tools.join(", "))
        }
    }
}
