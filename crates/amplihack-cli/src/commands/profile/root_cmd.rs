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

/// The profiles, one per line after a `NAME`/`KIND` header, the default
/// marked `*`; empty without profiles. Issue #1700: `KIND` is
/// `subscription` or `api-key`. With `warnings`, one line per OAuth
/// subscription profile that also names an API-key setting is added to it.
pub(super) fn render_list_with(
    store: &ProfileStore,
    home: Option<&std::path::Path>,
    warnings: &mut Vec<String>,
) -> Result<String, ProfileError> {
    let default = pointer::read(store.root())?;
    let names = store.list()?;
    if names.is_empty() {
        return Ok(String::new());
    }
    let width = names.iter().map(String::len).max().unwrap_or(0).max(4);
    let kinds = KindView::load(store, home);
    let mut out = format!("  {:<width$}  KIND\n", "NAME");
    for n in names {
        let mark = if default.as_ref().is_some_and(|d| d.as_str() == n) {
            '*'
        } else {
            ' '
        };
        let (label, warning) = kinds.of(store, &n);
        warnings.extend(warning);
        out.push_str(&format!("{mark} {n:<width$}  {label}\n"));
    }
    Ok(out)
}

/// [`render_list_with`] without a home or the warnings.
#[cfg(test)]
pub(super) fn render_list(store: &ProfileStore) -> Result<String, ProfileError> {
    render_list_with(store, None, &mut Vec::new())
}

/// `name:` and `dir:` of the default profile, or `none`. Issue #1700: then
/// its kind with where it came from, and whether it names an API-key
/// setting (never a value).
pub(super) fn render_show_with(
    store: &ProfileStore,
    home: Option<&std::path::Path>,
    warnings: &mut Vec<String>,
) -> Result<String, ProfileError> {
    match pointer::read(store.root())? {
        None => Ok("none\n".to_string()),
        Some(name) => {
            let dir = store.resolve(&name)?;
            let mut out = format!("name: {name}\ndir: {}\n", dir.display());
            #[cfg(unix)]
            {
                let r = super::kind::Ctx::load(store, home).resolve(store, &name);
                out.push_str(&format!("kind: {} ({})\n", r.kind.label(), r.source));
                let yes = if r.key_setting { "yes" } else { "no" };
                out.push_str(&format!("API key configured: {yes}\n"));
                if r.warn_key_setting() {
                    warnings.push(super::kind::key_setting_warning(&name));
                }
            }
            #[cfg(not(unix))]
            let _ = (home, warnings);
            Ok(out)
        }
    }
}

/// [`render_show_with`] without a home or the warnings.
#[cfg(test)]
pub(super) fn render_show(store: &ProfileStore) -> Result<String, ProfileError> {
    render_show_with(store, None, &mut Vec::new())
}

/// Each profile's kind label, and the key-setting warning when it applies.
struct KindView {
    #[cfg(unix)]
    ctx: super::kind::Ctx,
}

impl KindView {
    fn load(store: &ProfileStore, home: Option<&std::path::Path>) -> Self {
        #[cfg(unix)]
        {
            Self {
                ctx: super::kind::Ctx::load(store, home),
            }
        }
        #[cfg(not(unix))]
        {
            let _ = (store, home);
            Self {}
        }
    }

    fn of(&self, store: &ProfileStore, raw: &str) -> (&'static str, Option<String>) {
        #[cfg(unix)]
        if let Ok(name) = super::name::ProfileName::parse(raw) {
            let r = self.ctx.resolve(store, &name);
            let warning = r
                .warn_key_setting()
                .then(|| super::kind::key_setting_warning(&name));
            return (r.kind.label(), warning);
        }
        let _ = (store, raw);
        ("subscription", None)
    }
}
