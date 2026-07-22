//! Fleet identity model selection for `amplihack signal distribute` (#921, D2).
//!
//! The decided default is [`IdentityMode::LinkedDevice`]: each VM is its own
//! linked device on the operator's single Signal number (Signal-native; one
//! chat identity across the fleet). [`IdentityMode::DedicatedNumber`] is the
//! documented extension point for very large fleets (past Signal's
//! linked-device limit) and is **not implemented yet** — selecting it fails
//! fast via [`ensure_supported`] with the UNSUPPORTED exit code (3), before any
//! side effect, rather than silently degrading.

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

use super::error::SignalOpError;

type OpResult<T> = Result<T, SignalOpError>;

/// How each VM presents its Signal identity during a fleet rollout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum IdentityMode {
    /// Each VM is its own linked device on the operator's single number.
    #[default]
    LinkedDevice,
    /// Each VM uses its own dedicated Signal number (not implemented yet).
    DedicatedNumber,
}

/// Reject an identity mode that is not implemented yet **before** any side
/// effect. `LinkedDevice` is supported; `DedicatedNumber` returns
/// [`SignalOpError::Unsupported`] (exit code 3).
pub fn ensure_supported(mode: IdentityMode) -> OpResult<()> {
    match mode {
        IdentityMode::LinkedDevice => Ok(()),
        IdentityMode::DedicatedNumber => Err(SignalOpError::Unsupported(
            "identity mode 'dedicated-number' is not implemented yet; \
             use the default 'linked-device' (each VM is its own linked device)"
                .into(),
        )),
    }
}
