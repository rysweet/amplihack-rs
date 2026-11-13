//! Shared authentication status and usage-command reassessment.
use super::*;

/// One sorted persisted summary, shared by both commands. No authentication.
pub(in crate::commands::profile) fn line(root: &Path) -> String {
    match load(&RotateState::new(root)) {
        Err(_) => "needs-login: unknown (state unreadable; preserved for repair)".into(),
        Ok(s) if s.profiles.is_empty() => "needs-login: none".into(),
        Ok(s) => format!(
            "needs-login: {}; stored login: amplihack profile login <name>; live login: claude /login",
            s.profiles
                .iter()
                .map(|(n, e)| format!("{n} (first seen {})", timestamp(e.first_seen)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// A usage command reassesses episodes once, before its parallel row readers.
pub(in crate::commands::profile) fn assess_usage(
    store: &ProfileStore,
    home: &Path,
) -> Result<(), ProfileError> {
    let state = State::new(store.root())?;
    // Usage displays the rotator's episodes; it must not enable recording
    // merely by creating the state lock on an unconfigured profile root.
    if !state.exists() {
        return Ok(());
    }
    let Some(_lock) = state.try_lock(Duration::ZERO)? else {
        return Ok(());
    };
    let rs = RotateState::new(store.root());
    if exists(&rs.journal) || exists(&state.dir.join("journal.json")) {
        return Ok(());
    }
    assess(
        store,
        home,
        crate::commands::profile::auto_switch::kit::now(),
    )
}
