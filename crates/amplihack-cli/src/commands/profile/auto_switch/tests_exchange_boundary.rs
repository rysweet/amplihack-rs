//! Private, thread-local exchange boundary for portable rotator tests.
//! Three renames simulate completed placement; they make no atomicity claim.
use std::cell::RefCell;
use std::path::{Path, PathBuf};

thread_local! {
    static ROOT: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    static STOP: RefCell<Option<String>> = const { RefCell::new(None) };
}

pub(in crate::commands::profile) fn interrupted(point: &str, run: impl FnOnce()) {
    STOP.with(|p| *p.borrow_mut() = Some(point.to_owned()));
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(run));
    let reached = STOP.with(|p| p.borrow_mut().take().is_none());
    assert!(reached && outcome.is_err(), "interruption boundary {point}");
}

pub(super) fn interrupt(point: &str) {
    if STOP.with(|p| {
        let mut p = p.borrow_mut();
        if p.as_deref() == Some(point) {
            p.take();
            true
        } else {
            false
        }
    }) {
        panic!("controlled interruption: {point}");
    }
}

pub(in crate::commands::profile) fn active() -> bool {
    ROOT.with(|r| r.borrow().is_some())
}

pub(in crate::commands::profile) fn within<T>(root: &Path, run: impl FnOnce() -> T) -> T {
    assert!(root.starts_with(std::env::temp_dir()));
    assert_ne!(root, std::env::temp_dir());
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            ROOT.with(|r| *r.borrow_mut() = None);
        }
    }
    ROOT.with(|r| assert!(r.borrow_mut().replace(root.to_owned()).is_none()));
    let _reset = Reset;
    run()
}

pub(super) fn exchange(a: &Path, b: &Path) -> std::io::Result<()> {
    ROOT.with(|r| {
        let root = r.borrow();
        let root = root.as_ref().expect("explicit test exchange scope");
        assert!(a.starts_with(root) && b.starts_with(root));
        let aside = root.join("test-exchange-aside");
        assert!(!aside.exists());
        std::fs::rename(a, &aside)?;
        std::fs::rename(b, a)?;
        std::fs::rename(aside, b)
    })
}
