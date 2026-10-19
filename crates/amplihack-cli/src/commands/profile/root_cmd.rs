//! `amplihack profile root [<dir>]`, the root recorded by the first `add`
//! (issue #1592), and the text of `profile list` and `profile show`.

use super::root_config::{
    PROFILE_ROOT_FILE, RootSource, config_dir, home_from_env, save, store_from_env, validate_arg,
};
use super::store::{ProfileStore, ROOT_ENV};
use super::usage_cmd::sanitize_display;
use super::{ProfileError, pointer};
use std::path::Path;

fn shown(path: &Path) -> String {
    sanitize_display(&path.display().to_string(), false)
}

/// `root: <path> (source: <env|config|default>)`, escaped for the terminal.
pub fn root_line(path: &Path, source: RootSource) -> String {
    format!("root: {} (source: {source})", shown(path))
}

/// `root: <path> (pinned by running rotator)`, escaped for the terminal.
pub fn pinned_line(path: &Path) -> String {
    format!("root: {} (pinned by running rotator)", shown(path))
}

/// `profile root`: prints the root in use, or saves `<dir>`. Runs before the
/// root is resolved for anything else, so `root <dir>` can repair a damaged
/// saved file; it never reads that file.
pub fn run(dir: Option<String>) -> Result<(), ProfileError> {
    let Some(raw) = dir else {
        let (store, source) = store_from_env()?;
        println!("{}", root_line(store.root(), source));
        return Ok(());
    };
    let home =
        home_from_env().ok_or_else(|| ProfileError::InvalidRoot("HOME is not set".into()))?;
    let root = validate_arg(&raw, &home)?;
    let file = config_dir(&home).join(PROFILE_ROOT_FILE);
    save(&file, &root)?;
    println!("saved profile root: {} ({})", shown(&root), shown(&file));
    if let Some(v) = std::env::var_os(ROOT_ENV).filter(|v| !v.is_empty()) {
        eprintln!(
            "note: {ROOT_ENV}={} overrides the saved root in this shell",
            sanitize_display(&v.to_string_lossy(), false)
        );
    }
    Ok(())
}

/// After a successful `add`: saves the root it used when nothing is saved
/// yet. The profile exists, so a failure here is a warning, not an error.
pub fn record_after_add(store: &ProfileStore) {
    let Some(home) = home_from_env() else {
        return;
    };
    let file = config_dir(&home).join(PROFILE_ROOT_FILE);
    match std::fs::symlink_metadata(&file) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Ok(_) => return,
        Err(e) => {
            eprintln!("warning: could not record profile root: {e}");
            return;
        }
    }
    match save(&file, store.root()) {
        Ok(()) => println!(
            "recorded profile root {} in {}",
            shown(store.root()),
            shown(&file)
        ),
        Err(e) => eprintln!("warning: could not record profile root: {e}"),
    }
}

pub(super) fn render_list(store: &ProfileStore) -> Result<String, ProfileError> {
    let default = pointer::read(store.root())?;
    let mut out = String::new();
    for n in store.list()? {
        let mark = if default.as_ref().is_some_and(|d| d.as_str() == n) {
            '*'
        } else {
            ' '
        };
        out.push_str(&format!("{mark} {n}\n"));
    }
    Ok(out)
}

pub(super) fn render_show(store: &ProfileStore) -> Result<String, ProfileError> {
    match pointer::read(store.root())? {
        None => Ok("none\n".to_string()),
        Some(name) => {
            let dir = store.resolve(&name)?;
            Ok(format!("name: {name}\ndir: {}\n", dir.display()))
        }
    }
}
