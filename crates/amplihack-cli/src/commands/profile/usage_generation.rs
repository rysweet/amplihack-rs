//! Trusted direct measurement adapter. Legacy passive/display values are
//! never republished with a fresh capture time or a current generation.
use super::{
    auto_switch::kit::State, name::ProfileName, store::ProfileStore, usage_cmd::Probe,
    usage_parse::UsageState, usage_source::Source,
};
use amplihack_utils::usage_observation::{
    Capture, Evidence, LoginLocation, ObservationStore, PublishOutcome,
};
use std::path::Path;

pub(super) struct Collection {
    capture: Capture,
    location: LoginLocation,
}

pub(super) fn capture(store: &ProfileStore, name: &ProfileName, live: bool) -> Option<Collection> {
    let captured = (|| -> Result<Option<Collection>, super::ProfileError> {
        let state = State::new(store.root())?;
        if !state.exists() {
            return Ok(None);
        }
        let _lock = state.lock()?;
        let observations = ObservationStore::open(store.root())?;
        let mut guard = observations.under_state_lock();
        let account = if live {
            name.to_string()
        } else {
            state.load_ledger()?.account_in(name.as_str()).to_string()
        };
        let Some(binding) = guard.binding(&account)? else {
            return Ok(None);
        };
        let path = if live {
            let Some(home) = super::root_config::home_from_env() else {
                return Ok(None);
            };
            home.join(".claude/.credentials.json")
        } else {
            store.root().join(name.as_str()).join(".credentials.json")
        };
        let Some(location) = guard.probe_location(&binding, &path)? else {
            return Ok(None);
        };
        Ok(Some(Collection {
            capture: guard.capture_new(&binding, super::auto_switch::kit::now())?,
            location,
        }))
    })();
    match captured {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("usage capture refused: {e}");
            None
        }
    }
}
pub(super) fn publish(root: &Path, collection: &Collection, probe: &Probe) {
    let capture = &collection.capture;
    if probe.source == Some(Source::Passive) || probe.error.is_some() {
        abandon(root, collection);
        return;
    }
    let report = &probe.report;
    if !matches!(
        report.state,
        UsageState::Ok | UsageState::SessionExhausted | UsageState::WeeklyExhausted
    ) {
        abandon(root, collection);
        return;
    }
    let evidence = report.observation.clone();
    let published = (|| -> std::io::Result<_> {
        let store = ObservationStore::open(root)?;
        let mut guard = store.lock()?;
        if !guard.probe_location_unchanged(&capture.binding, &collection.location)? {
            guard.abandon(capture)?;
            return Ok(PublishOutcome::RetiredGeneration);
        }
        guard.publish(capture, &evidence)
    })();
    match published {
        Ok(
            PublishOutcome::Accepted { .. }
            | PublishOutcome::Duplicate
            | PublishOutcome::Superseded
            | PublishOutcome::RetiredGeneration,
        ) => (),
        Ok(outcome) => tracing::warn!(?outcome, "usage observation refused"),
        Err(e) => tracing::warn!("usage observation publication failed: {e}"),
    }
}

pub(super) fn abandon(root: &Path, collection: &Collection) {
    let capture = &collection.capture;
    let abandoned = (|| -> std::io::Result<_> {
        let store = ObservationStore::open(root)?;
        store.lock()?.abandon(capture)
    })();
    if let Err(e) = abandoned {
        tracing::warn!("unused usage capture was not released: {e}");
    }
}
/// A running worker retains its original logical account across movement;
/// limit evidence belongs to that session rather than its later file path.
pub(super) fn publish_limit(root: &Path, collection: &Collection) {
    let capture = &collection.capture;
    let evidence = Evidence {
        limit_reached: true,
        ..Default::default()
    };
    let result = (|| -> std::io::Result<_> {
        let store = ObservationStore::open(root)?;
        let mut guard = store.lock()?;
        if guard.binding(&capture.binding.profile)?.as_ref() != Some(&capture.binding) {
            return Ok(PublishOutcome::RetiredGeneration);
        }
        guard.abandon(capture)?;
        // Ownership was captured before launching the session. Ordering is
        // captured at this new limit event, after any intervening readings.
        let limit = guard.capture_new(&capture.binding, super::auto_switch::kit::now())?;
        guard.publish(&limit, &evidence)
    })();
    match result {
        Ok(
            PublishOutcome::Accepted { .. }
            | PublishOutcome::Duplicate
            | PublishOutcome::Superseded
            | PublishOutcome::RetiredGeneration,
        ) => (),
        Ok(outcome) => tracing::warn!(?outcome, "rerun limit observation refused"),
        Err(e) => tracing::warn!("rerun limit observation publication failed: {e}"),
    }
}

#[cfg(test)]
#[path = "tests_issue_2029_generation.rs"]
mod issue_2029_generation;
