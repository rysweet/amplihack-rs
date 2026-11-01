//! Reads that may outlast a rotate cycle (issue #1844). A cycle starts its
//! reads on detached threads and waits only until its deadline; a read
//! still running then reads as "unknown, still running", its result is used
//! by a later cycle, and no second read of the same login starts while one
//! is outstanding.
//!
//! A read's age counts only the time it is working: not the time it waits
//! in the queue, and not the time it waits for a `claude` slot
//! ([`job_clock`]). A read working longer than [`max_age`], which is more
//! than its direct read and its `claude` fallback together may take, is
//! dropped and may be started again; its thread ends on its own HTTP or
//! probe deadline. A queued or waiting read is never dropped.

use crate::commands::profile::usage_cmd::PROBE_CANCEL;
use crate::commands::profile::usage_source::job_clock::{self, Clock, Phase};
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// One read, run on a detached thread.
pub type Job<T> = Box<dyn FnOnce() -> T + Send + 'static>;

/// The jobs started together, run at most `bound` at once.
const DEFAULT_BOUND: usize = 8;
/// Slack past two deadlines.
const SLACK: Duration = Duration::from_secs(5);

/// How long a read may work before it is dropped: `2 * timeout + 5 s`.
/// The direct read, its token refresh included, ends within one deadline
/// (`usage_direct::read`); a `claude` fallback has one deadline of its own,
/// started once it has its slot, and the clock starts again then too.
pub fn max_age(timeout: Duration) -> Duration {
    timeout.saturating_mul(2).saturating_add(SLACK)
}

/// Why a finished job has no result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lost {
    /// The job panicked; reported once, as a warning.
    Panicked,
    /// The stop flag was set before the job started; not reported.
    Cancelled,
}

/// A finished job: its key, the start it belongs to, and its result or why
/// there is none.
type Done<K, T> = (K, u64, Result<T, Lost>);

pub struct InFlight<K, T> {
    max_age: Duration,
    bound: usize,
    next: u64,
    /// Each key in flight: its clock and the start it belongs to.
    running: HashMap<K, (Clock, u64)>,
    /// Keys whose job panicked, not yet taken by [`Self::take_panicked`].
    panicked: Vec<K>,
    /// `PROBE_CANCEL` unless a test gives its own flag.
    cancel: Arc<AtomicBool>,
    tx: Sender<Done<K, T>>,
    rx: Receiver<Done<K, T>>,
}

impl<K, T> InFlight<K, T>
where
    K: Clone + Eq + Hash + Send + 'static,
    T: Send + 'static,
{
    pub fn new(max_age: Duration) -> Self {
        Self::with_cancel(max_age, PROBE_CANCEL.clone())
    }

    /// As [`Self::new`], with `cancel` in place of `PROBE_CANCEL`.
    pub fn with_cancel(max_age: Duration, cancel: Arc<AtomicBool>) -> Self {
        let (tx, rx) = channel();
        Self {
            max_age,
            bound: DEFAULT_BOUND,
            next: 0,
            running: HashMap::new(),
            panicked: Vec::new(),
            cancel,
            tx,
            rx,
        }
    }

    /// The keys whose job panicked since the last call. Each panic is
    /// returned once; the caller reports it.
    pub fn take_panicked(&mut self) -> Vec<K> {
        std::mem::take(&mut self.panicked)
    }

    /// The bound [`gather`](super::measure::gather) starts jobs with.
    pub fn set_bound(&mut self, bound: usize) {
        self.bound = bound.max(1);
    }

    pub fn bound(&self) -> usize {
        self.bound
    }

    pub fn busy(&self, k: &K) -> bool {
        self.running.contains_key(k)
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.running.len()
    }

    /// Starts every job whose key is not already busy, on one detached
    /// dispatcher that runs at most `bound` at once. The stop flag is
    /// checked before each job starts; once it is set, the jobs still
    /// queued are not run.
    ///
    /// Invariant: every queued job sends exactly one message, its result,
    /// `Panicked` or `Cancelled`, unless the receiver is gone. So every key
    /// this marks busy is cleared, and [`Self::wait_all`] always returns.
    pub fn start_all(&mut self, jobs: Vec<(K, Job<T>)>, bound: usize) {
        let mut queue = Vec::new();
        for (k, job) in jobs {
            if self.running.contains_key(&k) {
                continue;
            }
            let id = self.next;
            self.next += 1;
            let clock = job_clock::new();
            self.running.insert(k.clone(), (Arc::clone(&clock), id));
            queue.push((k, id, clock, job));
        }
        if queue.is_empty() {
            return;
        }
        let workers = bound.max(1).min(queue.len());
        let queue = Arc::new(Mutex::new(queue.into_iter()));
        let tx = self.tx.clone();
        let cancel = Arc::clone(&self.cancel);
        std::thread::spawn(move || {
            std::thread::scope(|s| {
                for _ in 0..workers {
                    let queue = Arc::clone(&queue);
                    let tx = tx.clone();
                    let cancel = Arc::clone(&cancel);
                    s.spawn(move || {
                        loop {
                            let cancelled = cancel.load(Ordering::SeqCst);
                            let next = queue.lock().unwrap_or_else(PoisonError::into_inner).next();
                            let Some((k, id, clock, job)) = next else {
                                break;
                            };
                            // On a stop the queue is drained, not left: each
                            // job still sends its one message.
                            let out = if cancelled {
                                Err(Lost::Cancelled)
                            } else {
                                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    job_clock::run(&clock, job)
                                }))
                                .map_err(|_| Lost::Panicked)
                            };
                            // The cycle that started it may have ended and
                            // the receiver with it: the result is not needed.
                            if tx.send((k, id, out)).is_err() {
                                break;
                            }
                        }
                    });
                }
            });
        });
    }

    /// Takes a finished job, if it belongs to the start in `running`.
    fn accept(&mut self, (k, id, out): Done<K, T>, got: &mut Vec<(K, T)>) {
        if self.running.get(&k).is_some_and(|(_, ours)| *ours == id) {
            self.running.remove(&k);
            match out {
                Ok(t) => got.push((k, t)),
                Err(Lost::Panicked) => self.panicked.push(k),
                Err(Lost::Cancelled) => {}
            }
        }
    }

    /// The results that have arrived; never blocks.
    pub fn collect(&mut self) -> Vec<(K, T)> {
        let mut got = Vec::new();
        while let Ok(done) = self.rx.try_recv() {
            self.accept(done, &mut got);
        }
        got
    }

    /// Results until no job is in flight or `until` passes.
    pub fn wait(&mut self, until: Instant) -> Vec<(K, T)> {
        self.wait_for(Some(until))
    }

    /// Results until no job is in flight.
    pub fn wait_all(&mut self) -> Vec<(K, T)> {
        self.wait_for(None)
    }

    fn wait_for(&mut self, until: Option<Instant>) -> Vec<(K, T)> {
        let mut got = self.collect();
        while !self.running.is_empty() {
            let done = match until {
                Some(t) => {
                    let left = t.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        break;
                    }
                    match self.rx.recv_timeout(left) {
                        Ok(done) => done,
                        Err(RecvTimeoutError::Timeout) => break,
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
                None => match self.rx.recv() {
                    Ok(done) => done,
                    Err(_) => break,
                },
            };
            self.accept(done, &mut got);
        }
        got
    }

    /// Drops the keys whose read has been working longer than `max_age` at
    /// `now`, and returns them. A queued read, or one waiting for a `claude`
    /// slot, is never dropped. A late result for a dropped key is ignored.
    pub fn drop_stale(&mut self, now: Instant) -> Vec<K> {
        let stale: Vec<K> = self
            .running
            .iter()
            .filter(|(_, (clock, _))| match job_clock::read(clock) {
                Phase::Running(at) => now.saturating_duration_since(at) > self.max_age,
                Phase::Queued | Phase::Waiting => false,
            })
            .map(|(k, _)| k.clone())
            .collect();
        for k in &stale {
            self.running.remove(k);
        }
        stale
    }
}
