//! Inject a write in the discovery/subscription gap without a timer or thread.
use super::*;

struct DiscoveryGap {
    child: PathBuf,
    value: Rc<Cell<u64>>,
    subscribed: bool,
    waited: bool,
}

impl Events for DiscoveryGap {
    fn watch(&mut self, dirs: &[PathBuf]) -> Result<(), ProfileError> {
        if !self.subscribed && dirs.contains(&self.child) {
            // The parent was watched first, but editing an existing file in
            // its child does not notify the parent. This happens immediately
            // before the child subscription exists, after its baseline read.
            self.value.set(2);
            self.subscribed = true;
        }
        Ok(())
    }
    fn watched(&self) -> Vec<PathBuf> {
        if self.subscribed {
            vec![self.child.clone()]
        } else {
            Vec::new()
        }
    }
    fn wait(&mut self) -> Result<Vec<Event>, ProfileError> {
        assert!(!self.waited, "shutdown was ignored");
        self.waited = true;
        Ok(vec![Event::Stop])
    }
    fn drain(&mut self) -> Result<Vec<Event>, ProfileError> {
        Ok(Vec::new())
    }
}

#[test]
fn startup_rereads_newly_discovered_objects_after_subscribing_before_acknowledging_readiness() {
    let tmp = tempfile::tempdir_in("/tmp").unwrap();
    let child = tmp.path().join("observations");
    std::fs::create_dir(&child).unwrap();
    let value = Rc::new(Cell::new(1));
    let observed = Cell::new(0);
    let ready = Cell::new(false);
    let mut source = DiscoveryGap {
        child: child.clone(),
        value: value.clone(),
        subscribed: false,
        waited: false,
    };
    let mut coordinator =
        Coordinator::new(resolve(&TuneFlags::default(), None, &|_| None).unwrap());
    watch_loop::run(Deps {
        events: &mut source,
        directories: &[tmp.path().to_path_buf()],
        snapshot: &mut |_| {
            observed.set(value.get());
            Ok(InputState {
                enabled: true,
                control: value.get().to_string(),
                directories: vec![child.clone()],
                ..InputState::default()
            })
        },
        coordinator: &mut coordinator,
        execute: &mut |_| panic!("start-up reconciliation launched work"),
        completions: &mut Vec::new,
        ready: &mut || {
            assert_eq!(
                observed.get(),
                2,
                "ready was acknowledged with a stale baseline from before child registration"
            );
            ready.set(true);
            Ok(())
        },
        now: &|| 1_790_000_000,
    })
    .unwrap();
    assert!(ready.get());
}

struct FailedSource {
    waits: usize,
}

impl Events for FailedSource {
    fn watch(&mut self, _: &[PathBuf]) -> Result<(), ProfileError> {
        Ok(())
    }
    fn watched(&self) -> Vec<PathBuf> {
        Vec::new()
    }
    fn wait(&mut self) -> Result<Vec<Event>, ProfileError> {
        self.waits += 1;
        assert_eq!(self.waits, 1, "unrecoverable watcher failure was retried");
        Err(ProfileError::RotateDaemon(
            "injected native watcher failure".into(),
        ))
    }
    fn drain(&mut self) -> Result<Vec<Event>, ProfileError> {
        Ok(Vec::new())
    }
}

#[test]
fn unrecoverable_watch_failure_is_returned_instead_of_creating_a_retry_loop() {
    let mut source = FailedSource { waits: 0 };
    let mut coordinator =
        Coordinator::new(resolve(&TuneFlags::default(), None, &|_| None).unwrap());
    let result = watch_loop::run(Deps {
        events: &mut source,
        directories: &[],
        snapshot: &mut |_| Ok(InputState::default()),
        coordinator: &mut coordinator,
        execute: &mut |_| panic!("watcher failure launched work"),
        completions: &mut Vec::new,
        ready: &mut || Ok(()),
        now: &|| 1_790_000_000,
    });
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("injected native watcher failure")
    );
    assert_eq!(source.waits, 1);
}

struct RepairGap {
    value: Rc<Cell<u64>>,
    repair: bool,
    waits: usize,
}
impl Events for RepairGap {
    fn watch(&mut self, _: &[PathBuf]) -> Result<(), ProfileError> {
        if self.repair {
            // A replacement is populated before its lost subscription is
            // re-established. Its pathname was already in the requested set.
            self.value.set(2);
            self.repair = false;
        }
        Ok(())
    }
    fn watched(&self) -> Vec<PathBuf> {
        Vec::new()
    }
    fn wait(&mut self) -> Result<Vec<Event>, ProfileError> {
        self.waits += 1;
        match self.waits {
            1 => {
                self.repair = true;
                Ok(vec![Event::Changed])
            }
            2 => Ok(vec![Event::Stop]),
            _ => panic!("shutdown was ignored"),
        }
    }
    fn drain(&mut self) -> Result<Vec<Event>, ProfileError> {
        Ok(Vec::new())
    }
}

#[test]
fn repair_rereads_a_reconnected_directory_even_when_its_requested_path_is_unchanged() {
    let tmp = tempfile::tempdir_in("/tmp").unwrap();
    let value = Rc::new(Cell::new(1));
    let mut events = RepairGap {
        value: value.clone(),
        repair: false,
        waits: 0,
    };
    let mut coordinator =
        Coordinator::new(resolve(&TuneFlags::default(), None, &|_| None).unwrap());
    let mut effects = Vec::new();
    watch_loop::run(Deps {
        events: &mut events,
        directories: &[tmp.path().to_path_buf()],
        snapshot: &mut |_| {
            Ok(InputState {
                enabled: true,
                control: value.get().to_string(),
                ..Default::default()
            })
        },
        coordinator: &mut coordinator,
        execute: &mut |effect| {
            effects.push(effect.clone());
            Ok(())
        },
        completions: &mut Vec::new,
        ready: &mut || {
            assert_eq!(value.get(), 1);
            Ok(())
        },
        now: &|| 1_790_000_000,
    })
    .unwrap();
    assert_eq!(
        effects
            .iter()
            .filter(|effect| matches!(effect, Effect::Decision { .. }))
            .count(),
        1
    );
}
