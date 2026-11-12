//! Test-only ownership of runner work and its wait clock. Cancellation is
//! observed while the executor still owns the child, before its pid is reaped.

use crate::test_support::unix_process::FifoWorker;
use amplihack_utils::clock::{Clock, SystemClock};
use std::cell::RefCell;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

thread_local! {
    static CONTROL: RefCell<Option<Arc<Control>>> = const { RefCell::new(None) };
}

pub(super) fn current() -> Option<Arc<Control>> {
    CONTROL.with(|slot| slot.borrow().clone())
}

pub(super) struct Control {
    cancelled: AtomicBool,
    started: AtomicBool,
    time: Option<Mutex<Instant>>,
}

impl Control {
    pub(super) fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

impl Clock for Control {
    fn now(&self) -> Instant {
        let now = self
            .time
            .as_ref()
            .map_or_else(Instant::now, |t| *t.lock().unwrap());
        self.started.store(true, Ordering::SeqCst);
        now
    }

    fn sleep(&self, d: Duration) {
        // Virtual time advances only on the test thread, after readiness.
        // A real bounded poll lets the child run and cancellation be observed.
        SystemClock.sleep(d.min(crate::test_support::wait::POLL));
    }
}

/// Runs recipe work on a [`FifoWorker`] thread with a [`Control`] installed
/// for it. Drop cancels first, so the executor kills the runner group it
/// still owns, then the inner worker releases the FIFO (which also frees
/// descendants left behind by a teardown regression, after the survivor
/// assertions have run) and joins the thread before the caller's env guards
/// or tempdirs drop, even if a readiness assertion or receive ceiling panics.
pub(super) struct RunnerWorker<T> {
    control: Arc<Control>,
    worker: FifoWorker<T>,
}

impl<T: Send + 'static> RunnerWorker<T> {
    pub(super) fn spawn(
        fifo: Option<(&Path, usize)>,
        virtual_time: bool,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Self {
        let control = Arc::new(Control {
            cancelled: AtomicBool::new(false),
            started: AtomicBool::new(false),
            time: virtual_time.then(|| Mutex::new(Instant::now())),
        });
        let worker_control = control.clone();
        let worker = FifoWorker::spawn_with(fifo, move || {
            CONTROL.with(|slot| *slot.borrow_mut() = Some(worker_control));
            work()
        });
        Self { control, worker }
    }

    /// Waits until the executor has read its clock and `ready` holds, then
    /// moves virtual time on by the runner timeout the executor reads
    /// (`recipe_runner_timeout`, from `AMPLIHACK_RECIPE_RUNNER_TIMEOUT_SECS`
    /// or its default). The executor's next poll therefore times out,
    /// whatever timeout the test configured.
    pub(super) fn time_out_after_ready(&self, what: &str, mut ready: impl FnMut() -> bool) {
        crate::test_support::wait::wait_until(what, || {
            self.control.started.load(Ordering::SeqCst) && ready()
        });
        // The first clock read has happened, so advancing cannot precede
        // the executor's deadline origin. Readiness itself consumes no budget.
        *self
            .control
            .time
            .as_ref()
            .expect("virtual runner clock")
            .lock()
            .unwrap() += super::execute::recipe_runner_timeout();
    }

    pub(super) fn wait(&mut self) -> T {
        self.worker.wait()
    }

    pub(super) fn wait_with_ceiling(&mut self, ceiling: Duration) -> T {
        self.worker.wait_with_ceiling(ceiling)
    }
}

impl<T> Drop for RunnerWorker<T> {
    fn drop(&mut self) {
        // Fields drop after this: `worker` then releases and joins.
        self.control.cancelled.store(true, Ordering::SeqCst);
    }
}
