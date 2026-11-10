//! Conservative, read-only classification of journals from earlier attempts.
use super::*;

enum Staleness {
    Retain,
    Orphaned,
    Superseded,
}
fn permitted(pending: &Value, key: &str, current: &Value) -> bool {
    if key == "config" {
        config::config_matches(pending, current)
    } else {
        current == &pending["snapshots"][key] || current == &pending["expected"][key]
    }
}
fn classify(pending: &Value, root: &Path, home: &Path, binary: &Path) -> Result<Staleness> {
    let config = serde_json::to_value(snapshot(&home.join("config.toml"))?)?;
    let hooks = serde_json::to_value(snapshot(&home.join("hooks.json"))?)?;
    if config.is_null()
        && hooks.is_null()
        && ["config", "hooks"]
            .iter()
            .any(|key| !permitted(pending, key, &Value::Null))
    {
        return Ok(Staleness::Orphaned);
    }
    if permitted(pending, "config", &config)
        || permitted(pending, "hooks", &hooks)
        || pending.get("backup_cleanup").is_some()
        || path_present(&root.join("previous-package"))?
        || path_present(&root.join("uninstall-package"))?
    {
        return Ok(Staleness::Retain);
    }
    let Some(value) = regular_json(&root.join("ownership.json"))? else {
        return Ok(Staleness::Retain);
    };
    // Unsupported or incomplete evidence is uncertainty, never supersession.
    let verified = (|| -> Result<bool> {
        let owner: Ownership = serde_json::from_value(value)?;
        if !completion_proof::newer(pending, &owner)? {
            return Ok(false);
        }
        completion_proof::verify(root, home, binary, &owner)?;
        Ok(true)
    })();
    match verified {
        Ok(true) => Ok(Staleness::Superseded),
        Ok(false) => Ok(Staleness::Retain),
        Err(error) => {
            tracing::debug!("Codex journal lacks corroborated supersession: {error:#}");
            Ok(Staleness::Retain)
        }
    }
}
/// Only the initial, locked installer boundary may classify stale journals.
pub(super) fn recover_startup(root: &Path, binary: &Path, home: &Path) -> Result<()> {
    recovery_archive::resume(root)?;
    let Some(bytes) = snapshot(&root.join("pending.json"))? else {
        return Ok(());
    };
    let pending: Value = serde_json::from_slice(&bytes).context(
        "malformed Codex recovery journal; record retained; use --reset-recovery after review",
    )?;
    ensure!(
        pending["schema_version"] == 2 && pending["codex_home"] == serde_json::to_value(home)?,
        "unsupported or out-of-scope Codex recovery journal; record retained"
    );
    recovery::validate_journal(&pending, home)?;
    rollback_durability::validate_paths(root, home)?;
    match classify(&pending, root, home, binary)? {
        Staleness::Retain => recover_install(root, binary, home),
        Staleness::Orphaned | Staleness::Superseded => {
            recovery_archive::archive(root, Some(&bytes))?;
            Ok(())
        }
    }
}
