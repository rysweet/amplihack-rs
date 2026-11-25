//! Persistent extensions resolved from the complete installed index.

use std::path::Path;

use anyhow::{Context, Result, ensure};

use crate::user_config;

use super::{index::Entry, resolve};

pub(super) fn change(staged: &Path, entries: &[Entry], name: &str, enable: bool) -> Result<()> {
    let id = resolve::resolve(staged, entries, name)?;
    ensure!(
        enable || !id.is_default(),
        "{id} is in the default set; disable removes only explicit extensions"
    );
    let config = user_config::load_user_only();
    ensure!(
        config.diagnostics.is_empty(),
        "correct configuration problems with `amplihack config edit` before changing lists"
    );
    let path = config
        .user_file
        .context("cannot find the user configuration file")?;
    let selection = user_config::list_selection();
    let mut enabled = selection.enabled().clone();
    let changed = if enable && !id.is_default() {
        enabled.insert(id)
    } else if enable {
        false
    } else {
        enabled.remove(&id)
    };
    if changed {
        super::extensions::write(&path, &enabled)?;
    }
    // Always reapply, including when the desired configuration is unchanged:
    // a previous invocation may have published only some of the tools.
    crate::commands::install::lists::apply()
}
