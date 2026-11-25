//! Selection of assets published to a tool, separate from framework staging.
//!
//! All publishers use these defaults. Complete framework assets and the
//! directory index are independent of this policy. Tool-specific exclusions
//! in full-list mode remain the responsibility of each publisher.

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

/// Leaf names of the skills listed by default, including nested skills.
pub const DEFAULT_SKILLS: [&str; 32] = [
    "crusty-old-engineer",
    "qa-team",
    "documentation-writing",
    "dev-orchestrator",
    "journalistic-writing-style",
    "auto-drive-to-merge",
    "quality-audit",
    "skill-and-agents-directory",
    "default-workflow",
    "pm-architect",
    "tla-plus-expert",
    "pr-guide",
    "investigation-workflow",
    "github",
    "test-gap-analyzer",
    "reviewing-code",
    "mermaid-diagram-generator",
    "multitask",
    "code-philosophy",
    "code-atlas",
    "lsp-setup",
    "claude-profile-rotation",
    "skill-builder",
    "property-based-testing",
    "supply-chain-audit",
    "smart-test",
    "amplihack-expert",
    "agentic-workflow-first",
    "pdf",
    "xlsx",
    "pptx",
    "docx",
];

/// Agent paths relative to the bundle's agents folder, without extensions.
pub const DEFAULT_AGENTS: [&str; 12] = [
    "core/reviewer",
    "core/builder",
    "core/tester",
    "core/architect",
    "core/optimizer",
    "core/api-designer",
    "specialized/security",
    "specialized/philosophy-guardian",
    "specialized/documentation-writer",
    "specialized/rust-programming-expert",
    "specialized/analyzer",
    "specialized/tla-plus-expert",
];

/// Choice of the smaller default lists or each tool's full inventory.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ListSet {
    #[default]
    Default,
    All,
}

impl FromStr for ListSet {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "default" => Ok(Self::Default),
            "all" => Ok(Self::All),
            _ => anyhow::bail!("lists.set must be default or all"),
        }
    }
}

impl fmt::Display for ListSet {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str(match self {
            Self::Default => "default",
            Self::All => "all",
        })
    }
}

/// Type of a canonical entry identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EntryKind {
    Skill,
    Agent,
}

/// Stable identity stored in `lists.enabled`.
///
/// Parsing validates the canonical form only. Existence and resolution of
/// aliases use the complete installed index at the CLI boundary.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct EntryId {
    kind: EntryKind,
    name: String,
}

impl EntryId {
    pub fn kind(&self) -> EntryKind {
        self.kind
    }

    /// Skill leaf name or agent path without its extension.
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn is_default(&self) -> bool {
        match self.kind {
            EntryKind::Skill => DEFAULT_SKILLS.contains(&self.name.as_str()),
            EntryKind::Agent => DEFAULT_AGENTS.contains(&self.name.as_str()),
        }
    }
}

impl FromStr for EntryId {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (kind, name) = match value.split_once(':') {
            Some(("skill", name)) => (EntryKind::Skill, name),
            Some(("agent", name)) => (EntryKind::Agent, name),
            _ => anyhow::bail!("entry must use skill:<name> or agent:<bundle-relative-path>"),
        };
        anyhow::ensure!(
            name.split('/').all(|part| {
                !part.is_empty()
                    && part != "."
                    && part != ".."
                    && part.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')
                    })
            }) && (kind == EntryKind::Agent || !name.contains('/'))
                && (kind != EntryKind::Agent || !name.ends_with(".md")),
            "entry must use a skill leaf name or an agent path without its extension"
        );
        Ok(Self {
            kind,
            name: name.to_owned(),
        })
    }
}

impl fmt::Display for EntryId {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self.kind {
            EntryKind::Skill => "skill",
            EntryKind::Agent => "agent",
        };
        write!(out, "{kind}:{}", self.name)
    }
}

/// Effective selection shared by managed installation and launch routes.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ListSelection {
    pub set: ListSet,
    enabled: BTreeSet<EntryId>,
}

impl ListSelection {
    /// Extensions remain recorded in all mode for a later switch to default.
    pub fn new(set: ListSet, enabled: impl IntoIterator<Item = EntryId>) -> Self {
        Self {
            set,
            enabled: enabled.into_iter().filter(|id| !id.is_default()).collect(),
        }
    }

    pub fn enabled(&self) -> &BTreeSet<EntryId> {
        &self.enabled
    }

    pub fn includes_skill(&self, leaf: &str) -> bool {
        self.includes(EntryKind::Skill, leaf, &DEFAULT_SKILLS)
    }

    pub fn includes_agent(&self, bundle_path: &str) -> bool {
        self.includes(EntryKind::Agent, bundle_path, &DEFAULT_AGENTS)
    }

    fn includes(&self, kind: EntryKind, name: &str, defaults: &[&str]) -> bool {
        self.set == ListSet::All
            || defaults.contains(&name)
            || self
                .enabled
                .iter()
                .any(|id| id.kind == kind && id.name == name)
    }
}
