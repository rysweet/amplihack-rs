//! Execute current owning consumers; no copied gate bodies or fake orch helpers.
//! Each authority case has its own selection so RED cannot mask later branches.
#[path = "issue_1538_context_transport/authority.rs"]
mod authority;
#[path = "issue_1538_context_transport/completion.rs"]
mod completion;
#[path = "issue_1538_context_transport/documentation.rs"]
mod documentation;
#[path = "issue_1538_context_transport/finalization.rs"]
mod finalization;
#[path = "issue_1538_context_transport/fixtures.rs"]
mod fixtures;
#[path = "issue_1538_context_transport/native.rs"]
mod native;
#[path = "issue_1538_context_transport/policies.rs"]
mod policies;
#[path = "issue_1538_context_transport/producers.rs"]
mod producers;
#[path = "issue_1538_context_transport/reader.rs"]
mod reader;
#[path = "issue_1538_context_transport/tooling.rs"]
mod tooling;

#[path = "issue_1538_context_transport/metadata.rs"]
mod metadata;
#[path = "issue_1538_context_transport/native_chain.rs"]
mod native_chain;
#[path = "issue_1538_context_transport/precision.rs"]
mod precision;
