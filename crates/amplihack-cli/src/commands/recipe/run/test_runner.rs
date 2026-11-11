//! Test-only ownership of runner work and its wait clock. Cancellation is
//! observed while the executor still owns the child, before its pid is reaped.

use amplihack_utils::clock::{Clock, SystemClock};
use std::cell::RefCell;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
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

struct Fifo {
    writer: File,
    _reader: File,
    readers: usize,
}

/// Owns the worker until it has joined, even if a readiness assertion or
/// receive ceiling panics. FIFO release also cleans descendants left behind
/// by a teardown regression, after the survivor assertions have run.
pub(super) struct RunnerWorker<T> {
    control: Arc<Control>,
    worker: Option<JoinHandle<()>>,
    result: mpsc::Receiver<T>,
    fifo: Option<Fifo>,
}

impl<T: Send + 'static> RunnerWorker<T> {
    pub(super) fn spawn(
        fifo: Option<(&Path, usize)>,
        virtual_time: bool,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Self {
        let fifo = fifo.map(|(path, readers)| {
            assert!((1..=512).contains(&readers));
            let reader = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(path)
                .expect("open runner FIFO reader");
            let writer = OpenOptions::new()
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(path)
                .expect("open runner FIFO writer");
            Fifo {
                writer,
                _reader: reader,
                readers,
            }
        });
        let control = Arc::new(Control {
            cancelled: AtomicBool::new(false),
            started: AtomicBool::new(false),
            time: virtual_time.then(|| Mutex::new(Instant::now())),
        });
        let worker_control = control.clone();
        let (tx, result) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            CONTROL.with(|slot| *slot.borrow_mut() = Some(worker_control));
            let _ = tx.send(work());
        });
        Self {
            control,
            worker: Some(worker),
            result,
            fifo,
        }
    }

    pub(super) fn advance_after_ready(&self, what: &str, mut ready: impl FnMut() -> bool) {
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
            .unwrap() += Duration::from_secs(1);
    }

    pub(super) fn wait(&mut self) -> T {
        self.wait_with_ceiling(crate::test_support::wait::CEILING)
    }

    pub(super) fn wait_with_ceiling(&mut self, ceiling: Duration) -> T {
        let result = self
            .result
            .recv_timeout(ceiling)
            .expect("hung: recipe runner worker did not return before the ceiling");
        self.worker
            .take()
            .unwrap()
            .join()
            .expect("runner worker panicked");
        result
    }
}

impl<T> Drop for RunnerWorker<T> {
    fn drop(&mut self) {
        self.control.cancelled.store(true, Ordering::SeqCst);
        if let Some(fifo) = &mut self.fifo
            && let Err(error) = fifo.writer.write_all(&vec![b'\n'; fifo.readers])
        {
            eprintln!("releasing runner FIFO readers failed: {error}");
        }
        if let Some(worker) = self.worker.take() {
            // Do not detach on a second ceiling. Cancellation kills the
            // blocked runner group; join before env guards/tempdirs drop.
            let _ = worker.join();
        }
    }
}
