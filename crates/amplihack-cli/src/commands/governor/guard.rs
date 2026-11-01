//! What admission hands back: a [`SlotGuard`] that releases its slot when
//! dropped, or a [`Denied`] that becomes exit code 80.

use super::EXIT_GOVERNOR_DENIED;
use super::model::FailOpenCode;
use super::store::{State, Store};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::time::Duration;

/// How long a release waits for the lock.
const RELEASE_LOCK_WAIT: Duration = Duration::from_secs(1);

enum Held {
    /// Admitted without a record: the governor is off, or it failed open.
    Nothing,
    /// A top-level slot of this process.
    Slot { root: PathBuf, pid: u32 },
    /// A nested launch counted on its parent's slot.
    Nested { root: PathBuf, parent_pid: u32 },
}

/// Holds an admission for as long as it lives. Bind it to a named variable
/// (`let _slot = ...`) so it is dropped when the work ends, not at once.
#[must_use = "dropping the guard releases the slot at once"]
pub struct SlotGuard {
    held: Held,
    fail_open: Option<FailOpenCode>,
    tree_id: Option<String>,
}

impl SlotGuard {
    pub(super) fn inert(fail_open: Option<FailOpenCode>) -> Self {
        Self {
            held: Held::Nothing,
            fail_open,
            tree_id: None,
        }
    }

    pub(super) fn slot(
        root: PathBuf,
        pid: u32,
        tree_id: String,
        fail_open: Option<FailOpenCode>,
    ) -> Self {
        Self {
            held: Held::Slot { root, pid },
            fail_open,
            tree_id: Some(tree_id),
        }
    }

    pub(super) fn nested_under(root: PathBuf, parent_pid: u32, tree_id: String) -> Self {
        Self {
            held: Held::Nested { root, parent_pid },
            fail_open: None,
            tree_id: Some(tree_id),
        }
    }

    /// The fail-open code when the governor admitted without deciding.
    pub fn fail_open(&self) -> Option<&'static str> {
        self.fail_open.map(FailOpenCode::as_str)
    }

    /// Admitted under a parent's slot.
    pub fn nested(&self) -> bool {
        matches!(self.held, Held::Nested { .. })
    }

    /// The tree id recorded with the slot.
    pub fn tree_id(&self) -> Option<&str> {
        self.tree_id.as_deref()
    }
}

/// Applies `change` to `state.json` under the lock. Faults are logged only.
fn release(root: &std::path::Path, change: impl FnOnce(&mut State)) {
    let result = Store::open(root).and_then(|store| {
        let lock = store.lock_exclusive(RELEASE_LOCK_WAIT)?;
        let mut state: State = store.load(&lock)?;
        change(&mut state);
        store.save_state(&lock, &state)
    });
    if let Err(fault) = result {
        tracing::warn!(
            code = fault.code.as_str(),
            "governor: slot release failed; reclaimed when this process exits"
        );
    }
}

impl Drop for SlotGuard {
    fn drop(&mut self) {
        // The id is `None` only for `Held::Nothing`, which matches no slot,
        // so the empty default is never compared with a stored id.
        let tree = self.tree_id.clone().unwrap_or_default();
        let held = std::mem::replace(&mut self.held, Held::Nothing);
        let outcome = catch_unwind(AssertUnwindSafe(|| match held {
            Held::Nothing => {}
            Held::Slot { root, pid } => release(&root, |s| {
                s.slots
                    .retain(|slot| !(slot.pid == pid && slot.tree_id == tree));
            }),
            Held::Nested { root, parent_pid } => release(&root, |s| {
                if let Some(slot) = s
                    .slots
                    .iter_mut()
                    .find(|slot| slot.pid == parent_pid && slot.tree_id == tree)
                {
                    slot.nested_children = slot.nested_children.saturating_sub(1);
                }
            }),
        }));
        if outcome.is_err() {
            tracing::warn!("governor: slot release panicked; reclaimed when this process exits");
        }
    }
}

/// A start the governor refused: weekly exhaustion with no reset before the
/// deadline, or a wait that ran out.
#[derive(Debug)]
pub struct Denied {
    code: &'static str,
    message: String,
}

impl Denied {
    pub(super) fn new(code: &'static str, message: String) -> Self {
        Self { code, message }
    }

    #[cfg(test)]
    pub fn code(&self) -> &'static str {
        self.code
    }

    /// Prints the "not started" line to stderr and returns the exit error
    /// for [`EXIT_GOVERNOR_DENIED`].
    pub fn report(self) -> anyhow::Error {
        eprintln!(
            "amplihack: governor: not started ({}): {}",
            self.code, self.message
        );
        crate::command_error::exit_error(EXIT_GOVERNOR_DENIED)
    }
}
