//! Reads that may outlast a rotate cycle (issue #1844). A cycle starts its
//! reads on detached threads and waits only until its deadline; a read
//! still running then reads as "unknown, still running", its result is used
//! by a later cycle, and no second read of the same login starts while one
//! is outstanding. A read in flight longer than [`max_age`] is dropped and
//! may be started again; its thread ends on its own HTTP or probe deadline.

use crate::commands::profile::usage_cmd::PROBE_CANCEL;
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// One read, run on a detached thread.
pub type Job<T> = Box<dyn FnOnce() -> T + Send + 'static>;

/// The jobs started together, run at most `bound` at once.
const DEFAULT_BOUND: usize = 8;
/// Slack past two deadlines: a read may refresh its token, so it can make
/// up to three requests, each within the deadline.
const SLACK: Duration = Duration::from_secs(5);

/// How long a read may stay in flight: `2 * timeout + 5 s`.
pub fn max_age(timeout: Duration) -> Duration {
    timeout.saturating_mul(2).saturating_add(SLACK)
}

/// A finished job: its key, the start it belongs to, and its result, or
/// `None` when it panicked.
type Done<K, T> = (K, u64, Option<T>);

pub struct InFlight<K, T> {
    max_age: Duration,
    bound: usize,
    next: u64,
    running: HashMap<K, (Instant, u64)>,
    tx: Sender<Done<K, T>>,
    rx: Receiver<Done<K, T>>,
}

impl<K, T> InFlight<K, T>
where
    K: Clone + Eq + Hash + Send + 'static,
    T: Send + 'static,
{
    pub fn new(max_age: Duration) -> Self {
        let (tx, rx) = channel();
        Self {
            max_age,
            bound: DEFAULT_BOUND,
            next: 0,
            running: HashMap::new(),
            tx,
            rx,
        }
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
    /// dispatcher that runs at most `bound` at once. [`PROBE_CANCEL`] is
    /// checked before each job starts.
    pub fn start_all(&mut self, jobs: Vec<(K, Job<T>)>, bound: usize) {
        let now = Instant::now();
        let mut queue = Vec::new();
        for (k, job) in jobs {
            if self.running.contains_key(&k) {
                continue;
            }
            let id = self.next;
            self.next += 1;
            self.running.insert(k.clone(), (now, id));
            queue.push((k, id, job));
        }
        if queue.is_empty() {
            return;
        }
        let workers = bound.max(1).min(queue.len());
        let queue = Arc::new(Mutex::new(queue.into_iter()));
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            std::thread::scope(|s| {
                for _ in 0..workers {
                    let queue = Arc::clone(&queue);
                    let tx = tx.clone();
                    s.spawn(move || {
                        loop {
                            if PROBE_CANCEL.load(Ordering::Relaxed) {
                                break;
                            }
                            let next = queue.lock().unwrap_or_else(PoisonError::into_inner).next();
                            let Some((k, id, job)) = next else {
                                break;
                            };
                            let out =
                                std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)).ok();
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
            if let Some(t) = out {
                got.push((k, t));
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

    /// Drops the keys in flight longer than `max_age` at `now`, and returns
    /// them. A late result for one is ignored.
    pub fn drop_stale(&mut self, now: Instant) -> Vec<K> {
        let stale: Vec<K> = self
            .running
            .iter()
            .filter(|(_, (at, _))| now.saturating_duration_since(*at) > self.max_age)
            .map(|(k, _)| k.clone())
            .collect();
        for k in &stale {
            self.running.remove(k);
        }
        stale
    }
}
