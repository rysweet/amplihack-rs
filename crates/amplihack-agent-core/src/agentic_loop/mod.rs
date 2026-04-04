//! PERCEIVE→REASON→ACT→LEARN agentic loop.
//!
//! Ports the Python `amplihack/agents/goal_seeking/agentic_loop.py`.
//!
//! # Module layout
//!
//! | File               | Responsibility                                 |
//! |--------------------|-------------------------------------------------|
//! | `types.rs`         | Data structures (`LoopState`, `ReasoningTrace`) |
//! | `traits.rs`        | Dependency traits (`LlmClient`, etc.)           |
//! | `json_parse.rs`    | LLM response JSON extraction                   |
//! | `loop_helpers.rs`  | Helpers (truncate, error_action)                |
//! | `loop_core.rs`     | `AgenticLoop` — PRAL phases + run methods       |
//! | `reasoning.rs`     | Iterative reasoning (plan/search/refine)        |
//! | `reasoning_eval.rs`| Sufficiency evaluation + targeted search        |

pub mod json_parse;
pub mod loop_core;
pub mod loop_helpers;
pub mod reasoning;
pub mod reasoning_eval;
pub mod traits;
pub mod types;

// Re-exports for convenience.
pub use loop_core::{AgenticLoop, DEFAULT_MODEL};
pub use traits::{ActionExecutor, LlmClient, MemoryFacade, MemoryRetriever};
pub use types::{
    ActionResult, LlmMessage, LoopState, MemoryFact, ReasoningStep, ReasoningTrace, RetrievalPlan,
    SufficiencyEvaluation,
};
