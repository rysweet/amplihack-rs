//! SDK adapter data types — [`SdkType`], [`AgentTool`], [`AdapterResult`],
//! [`Goal`], [`SdkAdapterConfig`]. Ported from Python `sdk_adapters/base.py`.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::AgentError;

/// SDK backend type — which LLM SDK to use.
///
/// Mirrors Python `SDKType(str, Enum)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SdkType {
    /// Anthropic Claude SDK.
    Claude,
    /// GitHub Copilot SDK.
    Copilot,
    /// Microsoft Agent Framework (OpenAI-backed).
    #[default]
    Microsoft,
    /// Lightweight fallback framework.
    Mini,
}

impl std::fmt::Display for SdkType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Claude => write!(f, "claude"),
            Self::Copilot => write!(f, "copilot"),
            Self::Microsoft => write!(f, "microsoft"),
            Self::Mini => write!(f, "mini"),
        }
    }
}

impl std::str::FromStr for SdkType {
    type Err = AgentError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "claude" => Ok(Self::Claude),
            "copilot" => Ok(Self::Copilot),
            "microsoft" => Ok(Self::Microsoft),
            "mini" => Ok(Self::Mini),
            other => Err(AgentError::ConfigError(format!(
                "unknown SDK type: {other}"
            ))),
        }
    }
}

/// Grouping category for agent tools.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToolCategory {
    #[default]
    Core,
    Memory,
    Learning,
    Teaching,
    Spawning,
    Custom,
}

/// A tool that can be registered with an SDK adapter.
///
/// Mirrors Python `AgentTool` dataclass. The `function` field is omitted
/// because Rust handles dispatch through the [`super::base::SdkAdapter`] trait.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTool {
    pub name: String,
    pub description: String,
    /// JSON Schema-style parameter definitions keyed by parameter name.
    #[serde(default)]
    pub parameters: HashMap<String, Value>,
    #[serde(default)]
    pub requires_approval: bool,
    #[serde(default)]
    pub category: ToolCategory,
}

impl AgentTool {
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters: HashMap::new(),
            requires_approval: false,
            category: ToolCategory::Core,
        }
    }

    pub fn with_parameter(mut self, name: impl Into<String>, schema: Value) -> Self {
        self.parameters.insert(name.into(), schema);
        self
    }

    pub fn with_requires_approval(mut self, requires: bool) -> Self {
        self.requires_approval = requires;
        self
    }

    pub fn with_category(mut self, category: ToolCategory) -> Self {
        self.category = category;
        self
    }
}

/// Result of an SDK adapter run. Mirrors Python `AgentResult` dataclass.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AdapterResult {
    pub response: String,
    pub goal_achieved: bool,
    #[serde(default)]
    pub tools_used: Vec<String>,
    #[serde(default)]
    pub turns: u32,
    #[serde(default)]
    pub reasoning_trace: HashMap<String, Value>,
    #[serde(default)]
    pub metadata: HashMap<String, Value>,
}

impl AdapterResult {
    /// Create a successful result.
    pub fn success(response: impl Into<String>) -> Self {
        Self {
            response: response.into(),
            goal_achieved: true,
            ..Default::default()
        }
    }

    /// Create a failure result.
    pub fn failure(response: impl Into<String>) -> Self {
        Self {
            response: response.into(),
            goal_achieved: false,
            ..Default::default()
        }
    }

    pub fn with_tools(mut self, tools: Vec<String>) -> Self {
        self.tools_used = tools;
        self
    }

    pub fn with_turns(mut self, turns: u32) -> Self {
        self.turns = turns;
        self
    }

    pub fn with_metadata(mut self, key: impl Into<String>, value: Value) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }
}

/// A goal for the agent to pursue. Mirrors Python `Goal` dataclass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Goal {
    pub description: String,
    #[serde(default)]
    pub success_criteria: String,
    #[serde(default)]
    pub plan: Vec<String>,
    #[serde(default = "default_goal_status")]
    pub status: String,
}

fn default_goal_status() -> String {
    "pending".to_string()
}

impl Goal {
    pub fn new(description: impl Into<String>) -> Self {
        Self {
            description: description.into(),
            success_criteria: String::new(),
            plan: Vec::new(),
            status: default_goal_status(),
        }
    }

    pub fn with_criteria(mut self, criteria: impl Into<String>) -> Self {
        self.success_criteria = criteria.into();
        self
    }

    pub fn with_plan(mut self, steps: Vec<String>) -> Self {
        self.plan = steps;
        self
    }

    pub fn is_pending(&self) -> bool {
        self.status == "pending"
    }

    pub fn is_achieved(&self) -> bool {
        self.status == "achieved"
    }
}

/// Configuration shared across all SDK adapters.
/// Mirrors the constructor parameters of Python `GoalSeekingAgent.__init__`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SdkAdapterConfig {
    pub name: String,
    #[serde(default)]
    pub instructions: String,
    pub sdk_type: SdkType,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default)]
    pub storage_path: Option<PathBuf>,
    #[serde(default = "default_true")]
    pub enable_memory: bool,
    #[serde(default)]
    pub enable_eval: bool,
    #[serde(default)]
    pub enable_spawning: bool,
    #[serde(default = "default_max_turns")]
    pub max_turns: u32,
    #[serde(default = "default_timeout")]
    pub timeout_secs: f64,
}

fn default_model() -> String {
    std::env::var("EVAL_MODEL").unwrap_or_else(|_| "claude-opus-4-6".to_string())
}

fn default_true() -> bool {
    true
}

fn default_max_turns() -> u32 {
    10
}

fn default_timeout() -> f64 {
    300.0
}

impl SdkAdapterConfig {
    pub fn new(name: impl Into<String>, sdk_type: SdkType) -> Self {
        Self {
            name: name.into(),
            instructions: String::new(),
            sdk_type,
            model: default_model(),
            storage_path: None,
            enable_memory: true,
            enable_eval: false,
            enable_spawning: false,
            max_turns: default_max_turns(),
            timeout_secs: default_timeout(),
        }
    }

    pub fn with_instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = instructions.into();
        self
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    pub fn with_timeout(mut self, secs: f64) -> Self {
        self.timeout_secs = secs;
        self
    }

    pub fn with_storage_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.storage_path = Some(path.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdk_type_display_and_parse() {
        for (s, t) in [
            ("claude", SdkType::Claude),
            ("copilot", SdkType::Copilot),
            ("microsoft", SdkType::Microsoft),
            ("mini", SdkType::Mini),
        ] {
            assert_eq!(t.to_string(), s);
            assert_eq!(s.parse::<SdkType>().unwrap(), t);
        }
        assert_eq!("COPILOT".parse::<SdkType>().unwrap(), SdkType::Copilot);
        assert!("unknown".parse::<SdkType>().is_err());
    }

    #[test]
    fn sdk_type_serde_and_default() {
        assert_eq!(SdkType::default(), SdkType::Microsoft);
        let json = serde_json::to_string(&SdkType::Claude).unwrap();
        assert_eq!(json, r#""claude""#);
        let parsed: SdkType = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, SdkType::Claude);
    }

    #[test]
    fn agent_tool_builder_and_serde() {
        let tool = AgentTool::new("search", "Search memory")
            .with_parameter("query", serde_json::json!({"type": "string"}))
            .with_requires_approval(true)
            .with_category(ToolCategory::Memory);
        assert_eq!(tool.name, "search");
        assert!(tool.requires_approval);
        assert_eq!(tool.category, ToolCategory::Memory);
        assert!(tool.parameters.contains_key("query"));

        let json = serde_json::to_string(&tool).unwrap();
        let parsed: AgentTool = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "search");
        assert!(parsed.requires_approval);
    }

    #[test]
    fn adapter_result_success_and_failure() {
        let r = AdapterResult::success("done")
            .with_tools(vec!["bash".into()])
            .with_turns(3)
            .with_metadata("key", serde_json::json!("val"));
        assert!(r.goal_achieved);
        assert_eq!(r.tools_used, vec!["bash"]);
        assert_eq!(r.turns, 3);

        let f = AdapterResult::failure("error occurred");
        assert!(!f.goal_achieved);
        assert_eq!(f.response, "error occurred");
    }

    #[test]
    fn adapter_result_serde() {
        let r = AdapterResult::success("ok").with_turns(2);
        let json = serde_json::to_string(&r).unwrap();
        let parsed: AdapterResult = serde_json::from_str(&json).unwrap();
        assert!(parsed.goal_achieved);
        assert_eq!(parsed.turns, 2);
    }

    #[test]
    fn goal_builder_and_status() {
        let g = Goal::new("Learn Rust")
            .with_criteria("Pass all tests")
            .with_plan(vec!["Read docs".into(), "Write code".into()]);
        assert!(g.is_pending());
        assert!(!g.is_achieved());
        assert_eq!(g.plan.len(), 2);

        let mut g2 = Goal::new("test");
        g2.status = "achieved".into();
        assert!(g2.is_achieved());
    }

    #[test]
    fn goal_serde() {
        let g = Goal::new("test goal").with_criteria("done");
        let json = serde_json::to_string(&g).unwrap();
        let parsed: Goal = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.description, "test goal");
        assert_eq!(parsed.status, "pending");
    }

    #[test]
    fn config_builder_and_defaults() {
        let cfg = SdkAdapterConfig::new("agent-1", SdkType::Claude)
            .with_instructions("Be helpful")
            .with_model("claude-3")
            .with_timeout(60.0)
            .with_storage_path("/data");
        assert_eq!(cfg.name, "agent-1");
        assert_eq!(cfg.model, "claude-3");
        assert_eq!(cfg.timeout_secs, 60.0);
        assert!(cfg.storage_path.is_some());

        let d = SdkAdapterConfig::new("a", SdkType::Microsoft);
        assert_eq!(d.max_turns, 10);
        assert!((d.timeout_secs - 300.0).abs() < f64::EPSILON);
        assert!(d.enable_memory);
        assert!(!d.enable_eval);
    }

    #[test]
    fn config_serde() {
        let cfg = SdkAdapterConfig::new("test", SdkType::Copilot);
        let json = serde_json::to_string(&cfg).unwrap();
        let parsed: SdkAdapterConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "test");
        assert_eq!(parsed.sdk_type, SdkType::Copilot);
    }
}
