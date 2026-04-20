//! Evaluation framework: benchmark execution, scoring, and report generation.
//!
//! Ports the `amplihack.eval.*` Python framework to native Rust.  The crate is
//! structured as three independent bricks:
//!
//! * **`benchmark`** — define and run named benchmarks with wall-clock timing.
//! * **`scorer`** — compute aggregate scores and compare runs.
//! * **`reporter`** — serialise results to JSON and human-readable text.
//!
//! Each module has a single responsibility and exposes a clear public API.
//!
//! # Quick-start
//!
//! ```rust
//! use amplihack_eval::{Benchmark, BenchmarkResult, Reporter, Scorer, ScorerConfig};
//!
//! let mut result = BenchmarkResult::new("my-benchmark");
//! result.add_case("case-1", true, 0.95, 10);
//! result.add_case("case-2", false, 0.40, 15);
//! result.finish();
//!
//! let score = Scorer::new(ScorerConfig::default()).score(&result);
//! let text = Reporter::text(&score);
//! println!("{text}");
//! ```

pub mod benchmark;
pub mod error;
pub mod reporter;
pub mod scorer;

pub use benchmark::{Benchmark, BenchmarkResult, CaseResult};
pub use error::EvalError;
pub use reporter::Reporter;
pub use scorer::{RunComparison, RunScore, Scorer, ScorerConfig};
