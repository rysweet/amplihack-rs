//! amplihack-agent-core: Agent lifecycle, session management, and OODA loop.
//!
//! Ports the Python `amplihack/agents/goal_seeking/` subsystem:
//! - `GoalSeekingAgent` with OODA loop (observe/orient/decide/act)
//! - Intent detection and classification
//! - Session management
//! - Priority task queue
//! - Agent lifecycle (start/stop/pause/resume)

pub mod action_executor;
pub mod agent;
pub mod agentic_loop;
pub mod answer_synth;
pub mod code_synthesis;
pub mod cognitive_adapter;
pub mod continuous_eval;
pub mod error;
pub mod flat_retriever_adapter;
pub mod graph_rag_retriever;
pub mod hierarchical_memory_local;
pub mod hierarchical_memory_types;
pub mod input_events;
pub mod input_source;
pub mod intent;
pub mod knowledge_utils;
pub mod learning_ingestion;
pub mod lifecycle;
pub mod memory_export;
pub mod memory_retrieval;
pub mod models;
pub mod partition_routing;
pub(crate) mod safe_calc;
pub mod runtime_factory;
pub mod sdk_adapters;
pub mod session;
pub mod similarity;
pub mod sub_agents;
pub mod task_queue;
pub mod temporal_reasoning;

// Re-exports for ergonomic access.
pub use action_executor::{
    action_calculate, action_read_content, RegistryActionExecutor,
};
pub use agent::{Agent, GoalSeekingAgent};
pub use agentic_loop::{
    ActionExecutor, ActionResult, AgenticLoop, LlmClient, LlmMessage, LoopState, MemoryFacade,
    MemoryFact, MemoryRetriever, ReasoningStep, ReasoningTrace, RetrievalPlan,
    SufficiencyEvaluation, DEFAULT_MODEL,
};
pub use error::{AgentError, Result};
pub use intent::{Intent, IntentDetector, COMMAND_WORDS, QUESTION_WORDS};
pub use lifecycle::{AgentLifecycle, BasicLifecycle, HealthStatus, LifecycleState};
pub use models::{AgentConfig, AgentInfo, AgentState, TaskPriority, TaskResult, TaskSpec};
pub use session::{AgentSession, SessionManager};
pub use task_queue::TaskQueue;
pub use cognitive_adapter::{
    BackendKind, CognitiveAdapter, CognitiveAdapterConfig, CognitiveBackend, HiveFact, HiveStore,
    QualityScorer,
};
pub use sdk_adapters::{
    create_adapter, create_adapter_by_name, AdapterResult, AgentTool, ClaudeAdapter,
    CopilotAdapter, Goal, MicrosoftAdapter, SdkAdapter, SdkAdapterConfig, SdkClient,
    SdkClientResponse, SdkType, ToolCategory,
};
pub use sub_agents::{
    AgentSpawner, AnswerContext, CoordinatorAgent, MemoryAgent, MultiAgentConfig,
    MultiAgentOrchestrator, RetrievalStrategy, SpawnedAgent, SpawnedAgentStatus, SpecialistType,
    SubAgentMemory, TaskRoute,
};
pub use flat_retriever_adapter::FlatRetrieverAdapter;
pub use graph_rag_retriever::GraphRagRetriever;
pub use hierarchical_memory_local::HierarchicalMemoryLocal;
pub use hierarchical_memory_types::{
    KnowledgeEdge, KnowledgeNode, KnowledgeSubgraph, MemoryCategory,
    MemoryClassifier, StoreKnowledgeParams,
};
pub use memory_export::{export_memory, import_memory, ExportFormat, ExportMetadata};
pub use partition_routing::{
    partition_for_agent, stable_agent_index, DEFAULT_EVENT_HUB_PARTITIONS,
};
pub use runtime_factory::{
    create_goal_agent_runtime, create_goal_agent_runtime_with, ConfiguredGoalAgentRuntime,
    GoalAgentRuntime, RuntimeConfig,
};
pub use similarity::{
    compute_similarity, compute_tag_similarity, compute_word_similarity,
    extract_entity_anchor_tokens, extract_query_anchor_tokens, extract_query_phrases,
    rerank_facts_by_query, tokenize_similarity_text, NodeSimilarityInput,
};
