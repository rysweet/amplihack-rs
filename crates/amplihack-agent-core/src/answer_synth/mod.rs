//! OODA-based adaptive answer synthesis.
//!
//! Ports Python `answer_synthesizer.py` into focused Rust modules.
//!
//! | File            | Responsibility                                        |
//! |-----------------|-------------------------------------------------------|
//! | `types.rs`      | `QuestionLevel`, `IntentType`, `SynthesisConfig`, etc |
//! | `intent.rs`     | Intent detection / classification via LLM             |
//! | `retrieval.rs`  | Adaptive retrieval strategies based on intent          |
//! | `synthesis.rs`  | Core LLM synthesis with intent-aware prompting        |
//! | `refinement.rs` | Agentic refinement loop (evaluate + gap-fill)         |

pub mod intent;
pub mod refinement;
pub mod retrieval;
pub mod synthesis;
pub mod types;

pub use intent::{detect_intent, parse_intent_response};
pub use refinement::{answer_question, answer_question_agentic, evaluate_completeness};
pub use retrieval::{adaptive_retrieve, dedup_and_filter, RetrievalOutcome};
pub use synthesis::synthesize;
pub use types::{
    CompletenessEvaluation, DetectedIntent, IntentType, QuestionLevel, SynthesisConfig,
    TemporalCodeResult, ENUMERATION_KEYWORDS,
};
