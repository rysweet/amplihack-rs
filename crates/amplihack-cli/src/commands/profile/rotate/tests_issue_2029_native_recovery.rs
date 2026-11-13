//! Controlled overflow at the Events boundary, followed by real native I/O.
//! On macOS this exercises kqueue; Linux CI exercises inotify. This is not
//! a claim that the kernel overflow decoder has been exercised on macOS.
use super::super::TuneFlags;
use super::super::coordinator::{Coordinator, Effect, InputState, LoginInput};
use super::super::settings::resolve;
use super::super::watch_loop::{self, Deps, ReadPass};
use super::*;
use amplihack_utils::usage_observation::{Binding, Capture, Evidence, Record};
use std::path::PathBuf;

fn record(profile: &str, ticket: u64, session: f64) -> Record {
    Record {
        schema_version: 1,
        capture: Capture {
            binding: Binding {
                profile: profile.into(),
                account_id: format!("account-{profile}"),
                generation: format!("generation-{profile}"),
            },
            observation_id: format!("{profile}-{ticket}"),
            ticket,
            captured_at: 1_790_000_000,
        },
        evidence: Evidence {
            session_window: amplihack_utils::usage_observation::SessionWindow::Present,
            session_used: Some(session),
            week_used: Some(30.0),
            ..Evidence::default()
        },
        revision: ticket,
    }
}

fn snapshot(directory: &std::path::Path) -> InputState {
    let records = vec![
        serde_json::from_slice::<Record>(&fs::read(directory.join("work.json")).unwrap()).unwrap(),
        record("spare", 1, 10.0),
    ];
    InputState {
        enabled: true,
        logins: records
            .iter()
            .map(|r| LoginInput {
                profile: r.capture.binding.profile.clone(),
                account_id: r.capture.binding.account_id.clone(),
                generation: r.capture.binding.generation.clone(),
                eligible: true,
                organisation: Some("test-org".into()),
                live: r.capture.binding.profile == "work",
            })
            .collect(),
        observations: records
            .iter()
            .map(super::super::inputs::evidence_input)
            .collect(),
        records,
        directories: vec![directory.to_path_buf()],
        ..InputState::default()
    }
}

struct Recovery {
    native: Box<dyn Events>,
    directory: PathBuf,
    retired: PathBuf,
    phase: u8,
    recovery_seen: bool,
}

impl Events for Recovery {
    fn watch(&mut self, dirs: &[PathBuf]) -> Result<(), crate::commands::profile::ProfileError> {
        self.native.watch(dirs)?;
        if self.phase == 1 {
            // Clear all notifications caused by replacement itself. A later
            // assertion must see the new inode's publication, not this noise.
            self.native.drain()?;
            self.recovery_seen = true;
        }
        Ok(())
    }
    fn watched(&self) -> Vec<PathBuf> {
        self.native.watched()
    }
    fn wait(&mut self) -> Result<Vec<Event>, crate::commands::profile::ProfileError> {
        let phase = self.phase;
        self.phase += 1;
        match phase {
            0 => {
                let baseline = fs::read(self.directory.join("work.json")).unwrap();
                fs::rename(&self.directory, &self.retired).unwrap();
                fs::create_dir(&self.directory).unwrap();
                fs::set_permissions(&self.directory, fs::Permissions::from_mode(0o700)).unwrap();
                fs::write(self.directory.join("work.json"), baseline).unwrap();
                Ok(vec![Event::Overflow, Event::Overflow])
            }
            1 => {
                assert!(self.recovery_seen, "overflow did not repair subscriptions");
                fs::write(
                    self.directory.join(".pending"),
                    serde_json::to_vec(&record("work", 2, 85.0)).unwrap(),
                )
                .unwrap();
                fs::rename(
                    self.directory.join(".pending"),
                    self.directory.join("work.json"),
                )
                .unwrap();
                // Completion is an event-based escape if the watch was lost.
                // There is no wait window or retry if Changed is absent.
                self.native.wake_handle().completion().unwrap();
                let mut events = self.native.wait()?;
                events.extend(self.native.drain()?);
                assert!(
                    events.contains(&Event::Changed),
                    "recovery left the replacement inode unwatched: {events:?}"
                );
                Ok(events)
            }
            2 => Ok(vec![Event::Changed, Event::Changed]),
            3 => Ok(vec![Event::Stop]),
            _ => panic!("the event loop did not terminate after shutdown"),
        }
    }
    fn drain(&mut self) -> Result<Vec<Event>, crate::commands::profile::ProfileError> {
        self.native.drain()
    }
}

#[test]
fn overflow_repairs_native_subscriptions_before_a_subsequent_atomic_publication() {
    let (tmp, dir) = fixture();
    fs::write(
        dir.join("work.json"),
        serde_json::to_vec(&record("work", 1, 20.0)).unwrap(),
    )
    .unwrap();
    let mut source = Recovery {
        native: watch_source::open(std::slice::from_ref(&dir)).unwrap(),
        directory: dir.clone(),
        retired: tmp.path().join("retired"),
        phase: 0,
        recovery_seen: false,
    };
    let mut coordinator =
        Coordinator::new(resolve(&TuneFlags::default(), None, &|_| None).unwrap());
    let mut effects = Vec::new();
    let mut passes = Vec::new();
    watch_loop::run(Deps {
        events: &mut source,
        directories: std::slice::from_ref(&dir),
        snapshot: &mut |pass| {
            passes.push(pass);
            Ok(snapshot(&dir))
        },
        coordinator: &mut coordinator,
        execute: &mut |effect| {
            effects.push(effect.clone());
            Ok(())
        },
        completions: &mut Vec::new,
        ready: &mut || Ok(()),
        now: &|| 1_790_000_000,
    })
    .unwrap();
    assert_eq!(
        passes.iter().filter(|p| **p == ReadPass::Overflow).count(),
        1
    );
    assert_eq!(
        effects
            .iter()
            .filter(|e| matches!(e, Effect::Decision { .. }))
            .count(),
        1
    );
    assert_eq!(
        effects
            .iter()
            .filter(|e| matches!(e, Effect::Revalidate { .. }))
            .count(),
        1
    );
    assert!(
        !effects.iter().any(|e| matches!(e, Effect::Probe(_))),
        "recovery launched a speculative probe"
    );
}
