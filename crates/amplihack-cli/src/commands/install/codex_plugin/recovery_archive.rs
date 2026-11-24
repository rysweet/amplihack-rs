//! Exact-byte metadata archival shared by startup and explicit reset.
use super::*;
use recovery_archive_intent::{Intent, NAME};
use recovery_archive_storage::{capture, publish};

fn stamp() -> String {
    #[cfg(all(test, unix))]
    if let Some(stamp) = recovery_test_support::archive_stamp() {
        return stamp;
    }
    chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string()
}
fn report(path: &Path) {
    eprintln!(
        "amplihack: preserved Codex recovery journal: {}",
        super::super::paths::sanitize_path(path)
    );
}
fn context(root: &Path) -> String {
    format!(
        "cannot archive Codex recovery evidence at {}; repair metadata/storage and retry --reset-recovery; recovery protection retained",
        super::super::paths::sanitize_path(&root.join("pending.json"))
    )
}
/// Shared locked mutation guard: validate receipts and finish incomplete intents.
pub(super) fn resume(root: &Path) -> Result<bool> {
    let moved = recovery_archive_intent::resume(root).with_context(|| context(root))?;
    if let Some(path) = moved {
        report(&path);
        return Ok(true);
    }
    Ok(false)
}
pub(super) fn archive(root: &Path, expected: Option<&[u8]>) -> Result<bool> {
    (|| {
        if resume(root)? {
            return Ok(true);
        }
        let Some(raw) = capture(&root.join("pending.json"))? else {
            return Ok(false);
        };
        ensure!(
            expected.is_none_or(|bytes| raw.bytes == bytes),
            "Codex pending journal changed since classification; record retained"
        );
        let base = format!("pending.json.stale-{}", stamp());
        let mut name = base.clone();
        let mut suffix = 0u64;
        while path_present(&root.join(&name))? {
            suffix = suffix
                .checked_add(1)
                .context("Codex archive collision suffix exhausted")?;
            name = format!("{base}.{suffix}");
        }
        let mut intent = Intent::new(name, raw.identity);
        let marker = root.join(NAME);
        if let Some(previous) = capture(&marker)? {
            // Revalidate the captured receipt at its replacement boundary.
            let mut receipt: Intent = serde_json::from_slice(&previous.bytes)?;
            receipt.validate_receipt(root)?;
            atomic_json(
                &marker,
                &serde_json::to_value(&intent)?,
                Some(serde_json::from_slice(&previous.bytes)?),
            )?;
        } else {
            publish(&marker, &intent.bytes()?)?;
        }
        let destination = intent.finish(root)?;
        report(&destination);
        Ok(true)
    })()
    .with_context(|| context(root))
}
/// Resolve only HOME metadata; no journal parsing or CODEX_HOME access.
///
/// Reset takes the package lock without waiting: when another process holds
/// it, reset declines at once and leaves the journal untouched, so startup
/// never stalls behind a running installer.
pub(super) fn reset() -> Result<()> {
    let caller = home_dir()?;
    let root = root()?;
    path_scope::with_scope(&root, &caller, || {
        path_scope::check()?;
        let lock_path = caller.join(".amplihack/codex.install.lock");
        let lock = open_package_lock(&root).with_context(|| {
            format!(
                "cannot open Codex recovery lock {}; repair it and retry",
                super::super::paths::sanitize_path(&lock_path)
            )
        })?;
        acquire_lock(&lock).with_context(|| {
            format!(
                "cannot acquire Codex recovery lock {}; another installer is running; retry --reset-recovery after it finishes",
                super::super::paths::sanitize_path(&lock_path)
            )
        })?;
        path_scope::check()?;
        if !archive(&root, None)? {
            eprintln!("amplihack: no pending Codex recovery journal");
        }
        Ok(())
    })
    .with_context(|| context(&root))
}

/// One nonblocking attempt on fs2's package lock; contention is reported at once.
fn acquire_lock(lock: &fs::File) -> std::io::Result<()> {
    FileExt::try_lock_exclusive(lock).map_err(|error| {
        if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() {
            std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "the Codex recovery lock is held by another process",
            )
        } else {
            error
        }
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::commands::install::codex_plugin::recovery_test_support::Fixture;

    #[test]
    fn reset_declines_at_once_while_lock_is_held_and_succeeds_after_release() {
        let f = Fixture::new();
        let bytes = b"unparseable journal";
        fs::write(f.root.join("pending.json"), bytes).unwrap();
        fs::write(f.codex.join("config.toml"), b"native configuration").unwrap();
        let held = open_package_lock(&f.root).unwrap();
        held.lock_exclusive().unwrap();
        // The held lock makes reset return an error at once, not wait.
        let error = reset().unwrap_err();
        let io = error
            .chain()
            .find_map(|cause| cause.downcast_ref::<std::io::Error>())
            .expect("lock contention is reported as an I/O error");
        assert_eq!(io.kind(), std::io::ErrorKind::WouldBlock, "{error:#}");
        assert_eq!(fs::read(f.root.join("pending.json")).unwrap(), bytes);
        assert!(f.archives().is_empty());
        drop(held);
        reset().unwrap();
        assert_eq!(fs::read(f.archives().pop().unwrap()).unwrap(), bytes);
        assert_eq!(
            fs::read(f.codex.join("config.toml")).unwrap(),
            b"native configuration"
        );
    }
}
