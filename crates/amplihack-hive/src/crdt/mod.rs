//! Conflict-Free Replicated Data Types (CRDTs) for hive mind fact sharing.
//!
//! CRDTs enable eventual consistency without coordination.  Each agent maintains
//! a local replica and merges incoming state; the merge is commutative,
//! associative, and idempotent, so replicas converge regardless of message order
//! or duplication.

mod gcounter;
mod gset;
mod lww_register;
mod orset;
mod pncounter;

pub use gcounter::GCounter;
pub use gset::GSet;
pub use lww_register::LWWRegister;
pub use orset::ORSet;
pub use pncounter::PNCounter;
