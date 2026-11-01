//! The state handling of one admission round: loading the three files
//! with fail-open rules, saving with logging, and the waiting entry.

use super::guard::SlotGuard;
use super::model::{FailOpen, FailOpenCode, FailOpenRecord};
use super::store::{History, Persisted, Queue, State, Store, StoreLock};
use std::time::Duration;

/// How long a round waits for the lock before failing open.
pub(super) const LOCK_WAIT: Duration = Duration::from_secs(5);

/// The stderr line of a fail-open admission. The message was sanitised and
/// cut to 512 bytes by [`FailOpen::new`] and never holds a path.
pub(super) fn fail_open_line(fault: &FailOpen) -> String {
    format!(
        "amplihack: governor: admitted without a decision ({}): {}",
        fault.code.as_str(),
        fault.message
    )
}

/// Admits without a slot. Every fault but `no-root` also prints one line to
/// stderr, because faults such as `state-dir` and `lock-timeout` happen
/// before anything can be saved for `governor status`. `no-root` is the
/// usual state of a host with no login profiles, so it stays in the log.
pub(super) fn fail_open(fault: &FailOpen) -> SlotGuard {
    tracing::warn!(code = fault.code.as_str(), reason = %fault.message, "governor: admitted without a decision (fail open)");
    if fault.code != FailOpenCode::NoRoot {
        eprintln!("{}", fail_open_line(fault));
    }
    SlotGuard::inert(Some(fault.code))
}

pub(super) fn record(fault: &FailOpen, at: i64) -> FailOpenRecord {
    FailOpenRecord {
        code: fault.code,
        message: fault.message.clone(),
        at,
    }
}

/// The files of one round. A newer file stops the round with no write.
pub(super) struct Loaded {
    pub history: History,
    pub queue: Queue,
    pub state: State,
    pub fault: Option<FailOpen>,
}

/// One file: a newer version stops the round; any other fault is kept in
/// `fault` and the file starts again from its default.
pub(super) fn load_one<T: Persisted>(
    store: &Store,
    lock: &StoreLock,
    fault: &mut Option<FailOpen>,
) -> Result<T, FailOpen> {
    match store.load::<T>(lock) {
        Ok(v) => Ok(v),
        Err(f) if f.code == FailOpenCode::StateVersion => Err(f),
        Err(f) => {
            fault.get_or_insert(f);
            Ok(T::default())
        }
    }
}

pub(super) fn load_all(store: &Store, lock: &StoreLock) -> Result<Loaded, FailOpen> {
    let mut fault = None;
    let history = load_one(store, lock, &mut fault)?;
    let queue = load_one(store, lock, &mut fault)?;
    let state = load_one(store, lock, &mut fault)?;
    Ok(Loaded {
        history,
        queue,
        state,
        fault,
    })
}

/// Saves the round's files. A fault is logged here and is not propagated
/// as an error: D11 requires the governor to fail open, so callers that
/// cannot act on a failed save discard the result. Two callers use it: an
/// admission that was not saved holds nothing, and a waiting entry that was
/// not saved has no ticket.
pub(super) fn save_logged(store: &Store, lock: &StoreLock, l: &Loaded) -> Result<(), FailOpen> {
    store
        .save(lock, &l.history, &l.queue, &l.state)
        .inspect_err(|f| {
            tracing::warn!(code = f.code.as_str(), "governor: state not saved");
        })
}

pub(super) fn take_ticket(queue: &mut Queue, ticket: &mut Option<u64>) {
    if let Some(seq) = ticket.take() {
        queue.entries.retain(|e| e.seq != seq);
    }
}

/// Removes a waiting entry that a round left behind, when the loop ends by
/// failing open.
pub(super) fn drop_ticket(store: &Store, ticket: &mut Option<u64>) {
    if ticket.is_none() {
        return;
    }
    let result = store.lock_exclusive(LOCK_WAIT).and_then(|lock| {
        let mut queue: Queue = store.load(&lock)?;
        take_ticket(&mut queue, ticket);
        store.save_queue(&lock, &queue)
    });
    if let Err(f) = result {
        tracing::warn!(
            code = f.code.as_str(),
            file = Queue::FILE,
            "governor: waiting entry not removed; reclaimed when this process exits"
        );
    }
}
