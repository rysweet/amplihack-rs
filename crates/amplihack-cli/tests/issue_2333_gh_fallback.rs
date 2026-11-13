//! TDD contract for issue #2333 (GitHub reads survive a GraphQL limit) and
//! the "waits are not lack of progress" slice of #2472.
//!
//! Contract: docs/reference/autodrive-github-reads.md. These tests fail until
//! the Rust reader (`autodrive-gh-read`, `autodrive-gh-update-branch`), the
//! classifier (`autodrive-round-waits`), the completion dispatch's three head
//! outcomes and the WAITING loop result exist.
//!
//! Every `gh` is the fake in `support/autodrive_gh_fake.rs`: it refuses
//! GraphQL with the secondary-limit message and answers REST from fixture
//! files. The head's `ls-remote` path is real git against a local bare origin.
//! No test reaches GitHub, sleeps, polls or reads the clock.
//!
//! | Module      | Covers                                                    |
//! |-------------|-----------------------------------------------------------|
//! | `reads`     | T1, T2 (helper half), T4 (helper), usage and security     |
//! | `waits`     | the classifier CLI (D7, D8, downgrade table, target order) |
//! | `steps`     | bash read sites, progress 7th argument, round evidence    |
//! | `dispatch`  | T2 (dispatch half), T5                                    |
//! | `loops`     | T3, T6, the sidecar, evaluator inputs, crusty exit 76     |

#[path = "issue_2333_gh_fallback_dispatch.rs"]
mod dispatch;
#[path = "issue_2333_gh_fallback_loops.rs"]
mod loops;
#[path = "issue_2333_gh_fallback_reads.rs"]
mod reads;
#[path = "issue_2333_gh_fallback_steps.rs"]
mod steps;
#[path = "issue_2333_gh_fallback_waits.rs"]
mod waits;
